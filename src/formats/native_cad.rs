//! Neutral native CAD bodies tessellated by Catwalk's existing Rust STEP kernel.

use crate::{Result, scene::Scene};
use cadmpeg_codec_step::StepCodec;
use cadmpeg_core::decode::{DecodeArena, DecodeContext, DecodePolicy};
use cadmpeg_ir::{
    CadIr, SourceGeometryRole,
    appearance::AppearanceTarget,
    codec::write::{EncodeInput, Encoder, target::TargetRequest},
    geometry::{
        CurveGeometry, PlacedSurface, SolvedCurveGeometry, SolvedSurfaceGeometry, SurfaceGeometry,
        analytic::{SphereSurface, TorusSurface},
        pcurve::{LinePcurve, PcurveGeometry},
    },
    report::{Severity, loss::LossCategory},
    schema::EntitySchema,
    topology::{BodyKind, Sense},
    units::FinitePoint2,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
};

/// Match the STEP reader's admitted native face count before export allocation.
const MAX_FACES: usize = 100_000;
/// Bound the generated intermediate representation independently of source size.
const MAX_STEP_BYTES: usize = 16 * 1024 * 1024;
/// Bound the preview's body, topology, product and carrier identity graph.
const MAX_GRAPH_NODES: usize = 1_000_000;

/// The source decoder's association between face sense and carrier normals.
#[derive(Clone, Copy, Default)]
pub(super) enum SurfaceNormalConvention {
    /// Neutral IR face sense follows the parameter chart's oriented normal.
    #[default]
    Parametric,
    /// Native ASM ring-torus normals omit the sign of the tube radius.
    NativeAsm,
}

/// Transfer native B-rep, appearances and placed meshes into a rendered scene.
///
/// The conversion stays in memory and never opens a saved image or invokes a
/// CAD application. AP242 retains native tessellations and boundary surfaces.
///
/// # Errors
/// Missing or excessive geometry, geometry/placement export losses, exceeded
/// intermediate limits, and unsupported STEP tessellation fail explicitly.
pub(super) fn scene_from_ir(ir: &CadIr, normals: SurfaceNormalConvention) -> Result<Scene> {
    super::native_step::load_bytes(&step_projection(ir, normals)?)
}

/// Write the same bounded, loss-checked boundary projection used by rendering.
pub(super) fn step_projection(ir: &CadIr, normals: SurfaceNormalConvention) -> Result<Vec<u8>> {
    if ir.model.bodies.is_empty() && ir.model.tessellations.is_empty() {
        return Err("native CAD document contains no transferred geometry".into());
    }
    if ir.model.faces.len() > MAX_FACES {
        return Err("native CAD document exceeds the 100,000-face preview limit".into());
    }
    let mut preview = preview_geometry(ir)?;
    apply_surface_normal_convention(ir, &mut preview, normals);
    let plan = StepCodec::default().plan(
        EncodeInput::new(&preview, None),
        TargetRequest::Explicit("ap242e3"),
    )?;
    for loss in &plan.report().losses {
        // This association selects a different physical carrier, despite the
        // neutral writer classifying its omission as informational metadata.
        let local_curve_lost =
            loss.code.local_code() == "topology.coedge-use-curve-not-represented";
        if local_curve_lost
            || (loss.severity >= Severity::Warning
                && matches!(
                    loss.code.category(),
                    LossCategory::Geometry | LossCategory::Topology | LossCategory::Product
                ))
        {
            return Err(format!(
                "native CAD geometry export is incomplete ({}): {}",
                loss.code, loss.message
            )
            .into());
        }
    }
    let mut intermediate = StepBytes(Vec::new());
    plan.write_to(&mut intermediate)?;
    Ok(intermediate.0)
}

/// Correct the native carrier convention separately from exact chart changes.
///
/// A ring torus's parameter cross product has factor r(R+r cos(v)). ASM's
/// intrinsic carrier normal omits the sign of r; the native transfer currently
/// preserves face sense without that distinction. A negative tube radius thus
/// requires one face-sense reversal when writing a positive STEP carrier.
/// The V+pi chart remains orientation preserving, and all points and UV curves
/// are already retained by the independent chart conversion.
fn apply_surface_normal_convention(
    source: &CadIr,
    preview: &mut CadIr,
    normals: SurfaceNormalConvention,
) {
    match normals {
        SurfaceNormalConvention::Parametric => return,
        SurfaceNormalConvention::NativeAsm => {}
    }
    let affected: BTreeSet<_> = source
        .model
        .surfaces
        .iter()
        .filter(|surface| {
            surface
                .geometry
                .solved()
                .is_some_and(native_signed_ring_torus)
        })
        .map(|surface| &surface.id)
        .collect();
    for face in &mut preview.model.faces {
        if affected.contains(&face.surface) {
            // Neutral IR admits only these two known senses; an unknown source
            // sense must be refused by the decoder and its topology loss gate.
            face.sense = match face.sense {
                Sense::Forward => Sense::Reversed,
                Sense::Reversed => Sense::Forward,
            };
        }
    }
}

/// Placed carriers retain their source signed tube convention.
fn native_signed_ring_torus(mut geometry: &SolvedSurfaceGeometry) -> bool {
    while let SolvedSurfaceGeometry::Transformed(placed) = geometry {
        geometry = placed.basis();
    }
    matches!(geometry, SolvedSurfaceGeometry::Torus(torus)
        if torus.minor_radius().get() < 0.
            && torus.major_radius().get() > -torus.minor_radius().get())
}

/// Keep evaluated source geometry and prune auxiliary construction carriers.
///
/// The neutral IR evaluates a retained solved cache before its construction.
/// Replacing that cache wrapper with the same solved carrier preserves every
/// geometric point, while avoiding construction-only supports as scene roots.
/// Independent source objects and every topology or appearance dependency remain.
pub(super) fn preview_geometry(ir: &CadIr) -> Result<CadIr> {
    let arena = DecodeArena::new();
    let policy = DecodePolicy::service();
    let (ctx, _) = DecodeContext::from_root_bytes(&[], &arena, &policy)?;
    let supports = carrier_supports(ir, &ctx)?;
    let mut preview = evaluated_geometry(ir);
    let graph = geometry_graph(&preview, &ctx)?;
    let mut roots = geometry_roots(&preview, supports, &ctx)?;
    let mut pending: Vec<_> = roots.iter().cloned().collect();
    while let Some(id) = pending.pop() {
        ctx.charge_work(1, "native preview geometry reachability")?;
        if let Some(references) = graph.get(&id) {
            for reference in references {
                if roots.insert(reference.clone()) {
                    pending.push(reference.clone());
                }
            }
        }
    }
    macro_rules! retain_reachable {
        ($($name:ident),+ $(,)?) => {$(
            preview.model.$name.retain(|value| roots.contains(value.identity()));
        )+};
    }
    retain_reachable!(
        surfaces,
        curves,
        points,
        pcurves,
        procedural_surfaces,
        procedural_curves
    );
    canonicalize_signed_surfaces(&mut preview, &ctx)?;
    Ok(preview)
}

/// Construction dependencies are candidates for pruning, never scene roots.
fn carrier_supports(ir: &CadIr, ctx: &DecodeContext<'_>) -> Result<BTreeSet<String>> {
    let mut supports = BTreeSet::new();
    for value in &ir.model.procedural_surfaces {
        collect_references(value, ctx, &mut supports)?;
    }
    for value in &ir.model.procedural_curves {
        collect_references(value, ctx, &mut supports)?;
    }
    // Composite curve members are supports even when no procedural record owns them.
    for value in &ir.model.curves {
        collect_references(value, ctx, &mut supports)?;
    }
    Ok(supports)
}

/// Copy physical and presentation entities, using the evaluator's solved cache.
fn evaluated_geometry(ir: &CadIr) -> CadIr {
    let mut preview = CadIr::empty();
    preview.tolerances = ir.tolerances;
    macro_rules! copy_arena {
        ($($name:ident),+ $(,)?) => {$(preview.model.$name = ir.model.$name.clone();)+};
    }
    copy_arena!(
        bodies,
        regions,
        shells,
        faces,
        loops,
        coedges,
        edges,
        vertices,
        points,
        surfaces,
        curves,
        pcurves,
        procedural_surfaces,
        procedural_curves,
        subds,
        tessellations,
        appearances,
        appearance_bindings,
        assets,
        product_definitions,
        occurrences,
        assembly_joints
    );
    // A scene needs the authored oriented boundaries, not a reconstructed solid
    // classification. Native ASM shell chains do not promise outer-first order,
    // while STEP solid export reverses every later shell as an inferred void.
    // Surface-model export writes every shell with its own face senses and the
    // same body placements, product references and presentation assignments.
    for body in &mut preview.model.bodies {
        if body.kind == BodyKind::Solid {
            body.kind = BodyKind::Sheet;
        }
    }
    for surface in &mut preview.model.surfaces {
        if let SurfaceGeometry::Procedural {
            cache: Some(cache), ..
        } = &surface.geometry
            && !matches!(cache, SolvedSurfaceGeometry::Unknown { .. })
        {
            surface.geometry = SurfaceGeometry::Solved(cache.clone());
        }
    }
    for curve in &mut preview.model.curves {
        if let CurveGeometry::Procedural {
            cache: Some(cache), ..
        } = &curve.geometry
            && !matches!(cache, SolvedCurveGeometry::Unknown { .. })
        {
            curve.geometry = CurveGeometry::Solved(cache.clone());
        }
    }
    preview
}

/// Index all topology and geometry dependencies without guessing native names.
fn geometry_graph(
    preview: &CadIr,
    ctx: &DecodeContext<'_>,
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut graph = BTreeMap::new();
    macro_rules! index_geometry {
        ($name:ident) => {
            for value in &preview.model.$name {
                if graph.len() >= MAX_GRAPH_NODES {
                    return Err("native CAD preview exceeds the identity graph limit".into());
                }
                let mut references = BTreeSet::new();
                collect_references(value, ctx, &mut references)?;
                if graph
                    .insert(value.identity().to_owned(), references)
                    .is_some()
                {
                    return Err("native CAD preview contains duplicate entity identities".into());
                }
            }
        };
    }
    index_geometry!(surfaces);
    index_geometry!(curves);
    index_geometry!(points);
    index_geometry!(pcurves);
    index_geometry!(procedural_surfaces);
    index_geometry!(procedural_curves);
    index_geometry!(bodies);
    index_geometry!(regions);
    index_geometry!(shells);
    index_geometry!(faces);
    index_geometry!(loops);
    index_geometry!(coedges);
    index_geometry!(edges);
    index_geometry!(vertices);
    index_geometry!(product_definitions);
    index_geometry!(occurrences);
    Ok(graph)
}

/// Body/product roots also retain explicitly independent and styled geometry.
fn geometry_roots(
    preview: &CadIr,
    mut supports: BTreeSet<String>,
    ctx: &DecodeContext<'_>,
) -> Result<BTreeSet<String>> {
    let mut roots = BTreeSet::new();
    for body in &preview.model.bodies {
        roots.insert(body.identity().to_owned());
    }
    for value in &preview.model.product_definitions {
        roots.insert(value.identity().to_owned());
    }
    for value in &preview.model.occurrences {
        roots.insert(value.identity().to_owned());
    }
    let mut topology_owned = BTreeSet::new();
    for face in &preview.model.faces {
        collect_references(face, ctx, &mut topology_owned)?;
    }
    for coedge in &preview.model.coedges {
        collect_references(coedge, ctx, &mut topology_owned)?;
    }
    for edge in &preview.model.edges {
        collect_references(edge, ctx, &mut topology_owned)?;
    }
    for vertex in &preview.model.vertices {
        collect_references(vertex, ctx, &mut topology_owned)?;
    }
    macro_rules! independent_roots {
        ($name:ident) => {
            for value in &preview.model.$name {
                let role = value
                    .source_object
                    .as_ref()
                    .and_then(|source| source.geometry_role);
                if role == Some(SourceGeometryRole::Support) {
                    supports.insert(value.identity().to_owned());
                }
                if role == Some(SourceGeometryRole::Independent)
                    || (role != Some(SourceGeometryRole::Support)
                        && !supports.contains(value.identity())
                        && !topology_owned.contains(value.identity()))
                {
                    roots.insert(value.identity().to_owned());
                }
            }
        };
    }
    independent_roots!(surfaces);
    independent_roots!(curves);
    independent_roots!(points);
    for binding in &preview.model.appearance_bindings {
        match &binding.target {
            AppearanceTarget::Surface(id) => {
                roots.insert(id.as_str().to_owned());
            }
            AppearanceTarget::Curve(id) => {
                roots.insert(id.as_str().to_owned());
            }
            AppearanceTarget::Point(id) => {
                roots.insert(id.as_str().to_owned());
            }
            _ => {}
        }
    }
    Ok(roots)
}

/// Visit typed geometry identities through the dependency library's bounded API.
fn collect_references(
    value: &impl EntitySchema,
    ctx: &DecodeContext<'_>,
    references: &mut BTreeSet<String>,
) -> Result<()> {
    value.visit_references(ctx, &mut |identity| {
        if !references.contains(identity) {
            let identity = ctx.copy_retained_text(identity, "native preview identity")?;
            ctx.insert_btree_set(references, identity, "native preview references")?;
        }
        Ok(())
    })?;
    Ok(())
}

/// An exact affine change to an analytic surface's parameter chart.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ChartChange {
    /// Signed scale of azimuth and second parameter.
    scale: [f64; 2],
    /// Additive shift after that scale.
    offset: [f64; 2],
    /// Whether the oriented surface chart reverses.
    reversed: bool,
}

/// Convert signed analytic charts without letting STEP silently change trimming.
fn canonicalize_signed_surfaces(ir: &mut CadIr, ctx: &DecodeContext<'_>) -> Result<()> {
    let mut changes = BTreeMap::new();
    for surface in &mut ir.model.surfaces {
        if let SurfaceGeometry::Solved(geometry) = &mut surface.geometry {
            let (canonical, change) = canonical_surface(geometry)?;
            *geometry = canonical;
            if let Some(change) = change {
                changes.insert(surface.id.clone(), change);
            }
        }
    }
    let loops: BTreeMap<_, _> = ir
        .model
        .loops
        .iter()
        .map(|loop_| (&loop_.id, &loop_.face))
        .collect();
    let faces: BTreeMap<_, _> = ir
        .model
        .faces
        .iter()
        .map(|face| (&face.id, &face.surface))
        .collect();
    let mut pcurve_changes = BTreeMap::new();
    let identity = ChartChange {
        scale: [1., 1.],
        offset: [0., 0.],
        reversed: false,
    };
    let mut register = |id: &cadmpeg_ir::ids::PcurveId, change: ChartChange| -> Result<()> {
        if let Some(previous) = pcurve_changes.insert(id.clone(), change)
            && previous != change
        {
            return Err("native pcurve is shared by incompatible signed surface charts".into());
        }
        Ok(())
    };
    for coedge in &ir.model.coedges {
        let face = loops
            .get(&coedge.owner_loop)
            .ok_or("native coedge has no owning face")?;
        let surface = faces.get(face).ok_or("native loop has no owning face")?;
        let change = changes.get(*surface).copied().unwrap_or(identity);
        for use_ in &coedge.pcurves {
            register(&use_.pcurve, change)?;
        }
    }
    for loop_ in &ir.model.loops {
        let surface = faces
            .get(&loop_.face)
            .ok_or("native vertex loop has no owning face")?;
        let change = changes.get(*surface).copied().unwrap_or(identity);
        // Singular vertex loops and anchored ring poles own UV carriers too.
        // Register unchanged uses so a shared carrier cannot change one face
        // while silently changing the parameter chart of another face.
        for use_ in loop_.vertex_pcurves() {
            register(&use_.pcurve, change)?;
        }
    }
    for pcurve in &mut ir.model.pcurves {
        if let Some(change) = pcurve_changes.get(&pcurve.id) {
            remap_pcurve(&mut pcurve.geometry, *change, ctx)?;
        }
    }
    for face in &mut ir.model.faces {
        if changes
            .get(&face.surface)
            .is_some_and(|change| change.reversed)
        {
            face.sense = match face.sense {
                Sense::Forward => Sense::Reversed,
                Sense::Reversed => Sense::Forward,
            };
        }
    }
    Ok(())
}

/// Canonicalize a placed ring torus or sphere with an exact chart equivalence.
fn canonical_surface(
    geometry: &SolvedSurfaceGeometry,
) -> Result<(SolvedSurfaceGeometry, Option<ChartChange>)> {
    match geometry {
        SolvedSurfaceGeometry::Transformed(placed) => {
            let (basis, change) = canonical_surface(placed.basis())?;
            Ok((
                SolvedSurfaceGeometry::Transformed(PlacedSurface::try_new(
                    Box::new(basis),
                    *placed.transform(),
                )?),
                change,
            ))
        }
        SolvedSurfaceGeometry::Sphere(sphere) if sphere.radius().get() < 0. => {
            // S(C,r,X,Y,Z;u,v) = S(C,-r,-X,Y,-Z;-u,v).
            let raw_axis = sphere.frame().axis().as_raw();
            let raw_reference = sphere.frame().reference().as_raw();
            let axis = cadmpeg_ir::math::Vector3::new(-raw_axis.x, -raw_axis.y, -raw_axis.z);
            let reference = cadmpeg_ir::math::Vector3::new(
                -raw_reference.x,
                -raw_reference.y,
                -raw_reference.z,
            );
            let value = SphereSurface::try_new(
                sphere.center().get(),
                axis,
                reference,
                -sphere.radius().get(),
            )?;
            Ok((
                SolvedSurfaceGeometry::Sphere(value),
                Some(ChartChange {
                    scale: [-1., 1.],
                    offset: [0., 0.],
                    reversed: true,
                }),
            ))
        }
        SolvedSurfaceGeometry::Torus(torus) if torus.minor_radius().get() < 0. => {
            let major = torus.major_radius().get();
            let minor = -torus.minor_radius().get();
            if major <= minor {
                return Err("native signed horn or spindle torus requires a qualified trimmed-surface conversion".into());
            }
            // T(C,R,r,X,Y,Z;u,v) = T(C,R,-r,X,Y,Z;u,v+pi).
            let value = TorusSurface::try_new(
                torus.center().get(),
                *torus.frame().axis().as_raw(),
                *torus.frame().reference().as_raw(),
                major,
                minor,
            )?;
            Ok((
                SolvedSurfaceGeometry::Torus(value),
                Some(ChartChange {
                    scale: [1., 1.],
                    offset: [0., std::f64::consts::PI],
                    reversed: false,
                }),
            ))
        }
        _ => Ok((geometry.clone(), None)),
    }
}

/// Preserve rational weights and parameter intervals during a chart change.
#[allow(
    clippy::float_cmp,
    reason = "chart changes use exact discrete scales and identity offsets"
)]
fn remap_pcurve(
    geometry: &mut PcurveGeometry,
    change: ChartChange,
    ctx: &DecodeContext<'_>,
) -> Result<()> {
    if change.scale == [1., 1.] && change.offset == [0., 0.] {
        return Ok(());
    }
    let point = |value: cadmpeg_ir::math::Point2| {
        cadmpeg_ir::math::Point2::new(
            change.scale[0] * value.u + change.offset[0],
            change.scale[1] * value.v + change.offset[1],
        )
    };
    match geometry {
        PcurveGeometry::Line(line) => {
            let direction = *line.direction().as_raw();
            *line = LinePcurve::try_new(
                point(line.origin().get()),
                cadmpeg_ir::math::Point2::new(
                    change.scale[0] * direction.u,
                    change.scale[1] * direction.v,
                ),
            )?;
        }
        PcurveGeometry::Nurbs { nurbs } => {
            nurbs.try_map_control_points_in_place(
                |value| {
                    FinitePoint2::new(point(value.get()))
                        .ok_or("signed native pcurve produced a non-finite pole")
                },
                ctx,
            )??;
        }
        _ => return Err(
            "signed native analytic surface has an unsupported parameter-curve chart conversion"
                .into(),
        ),
    }
    Ok(())
}

/// A bounded memory sink for the generated STEP exchange structure.
struct StepBytes(Vec<u8>);

impl Write for StepBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_STEP_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other(
                "native CAD intermediate exceeds the 16 MiB preview limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "native_cad_tests.rs"]
mod tests;
