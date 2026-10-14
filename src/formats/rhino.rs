//! Native Rhino meshes, points, bounded curves, NURBS surfaces and B-rep bodies.
//!
//! The pure Rust `cadmpeg` codec owns native archive framing, checksums, object
//! decoding, document units and block expansion. Saved document images never
//! participate. Exact boundary faces use the shared Rust CAD tessellator;
//! independent source meshes retain their original winding and shading normals.

use crate::{
    Result,
    scene::{Line, Point, Scene, Triangle, TriangleAppearance, Vec3},
};
use cadmpeg_codec_rhino::RhinoCodec;
use cadmpeg_core::decode::{DecodeArena, DecodeContext, DecodePolicy};
use cadmpeg_ir::{
    CadIr, Codec, DecodeOptions, SourceGeometryRole, SourceObjectAssociation,
    eval::{decode::curve_point_solved, finite_or_refusal},
    geometry::{Curve, SolvedCurveGeometry, SolvedSurfaceGeometry},
    report::{Severity, decode::DecodeReport},
    schema::EntitySchema,
    tessellation::Tessellation,
    topology::Color,
    transform::Transform,
};
use std::{
    collections::{BTreeSet, HashMap},
    fs::File,
    mem::size_of,
    path::Path,
};

/// Maximum aggregate emitted triangles, points and curve segments.
const MAX_PRIMITIVES: usize = 5_000_000;
/// Conservative bound on retained output, including shading-map overhead.
const MAX_SCENE_BYTES: usize = 512 * 1024 * 1024;
/// Maximum native identities indexed for boundary traversal.
const MAX_GRAPH_NODES: usize = 1_000_000;
/// Maximum source mesh vertices before copying indexed channels.
const MAX_VERTICES: usize = 5_000_000;
/// Bound curve and surface evaluation independently of output size.
const MAX_EVALUATIONS: usize = 4_000_000;
/// Maximum samples per nonempty curve or surface knot span.
const SPAN_SAMPLES: u32 = 64;
/// Maximum evaluations per knot span along each surface axis.
const SURFACE_SPAN_SAMPLES: u32 = 32;
/// Maximum nested composite curves or inline placements.
const MAX_CURVE_DEPTH: usize = 64;

/// Load genuine model geometry from a bounded native 3DM archive.
///
/// # Errors
/// Malformed framing or checksums, incomplete native geometry/attributes,
/// unsupported carriers, missing curve references and resource limits fail.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let mut source = File::open(path)?;
    if source.metadata()?.len() > super::MAX_INPUT_BYTES {
        return Err("Rhino source exceeds the 512 MiB limit".into());
    }
    let decoded = RhinoCodec.decode(
        &mut source,
        &DecodeOptions {
            policy: DecodePolicy::service(),
            ..DecodeOptions::default()
        },
    )?;
    validate_report(decoded.report())?;
    scene(decoded.ir())
}

/// Allow explicitly geometry-preserving metadata losses and reject other warnings.
fn validate_report(report: &DecodeReport) -> Result<()> {
    for loss in &report.losses {
        // A source quad's two triangles describe the same surface. N-gon
        // grouping preserves the already decoded native mesh-face geometry.
        // Writer stamp qualification is provenance, separate from the decoder's
        // archive dialect admission and its checked native geometry layouts.
        let allowed = matches!(
            loss.code.local_code(),
            "mesh.quad-topology-triangulated"
                | "mesh.ngon-grouping-dropped"
                | "source.writer-stamp-unverified"
                | "viewport.userdata-dropped"
                | "history.geometry-not-transferred"
                | "history.dependency-dropped"
                | "history.embedded-geometry-dropped"
        );
        if loss.severity >= Severity::Error || (loss.severity >= Severity::Warning && !allowed) {
            return Err(format!(
                "Rhino native geometry is incomplete ({}): {}",
                loss.code, loss.message
            )
            .into());
        }
    }
    Ok(())
}

/// Convert only independently displayed source objects, keeping support topology private.
#[allow(clippy::too_many_lines)]
fn scene(ir: &CadIr) -> Result<Scene> {
    let arena = DecodeArena::new();
    let policy = DecodePolicy::service();
    let (ctx, _) = DecodeContext::from_root_bytes(&[], &arena, &policy)?;
    let mut support = BTreeSet::new();
    for face in &ir.model.faces {
        references(face, &ctx, &mut support)?;
    }
    for edge in &ir.model.edges {
        references(edge, &ctx, &mut support)?;
    }
    for value in &ir.model.procedural_curves {
        references(value, &ctx, &mut support)?;
    }
    for value in &ir.model.procedural_surfaces {
        references(value, &ctx, &mut support)?;
    }
    for curve in &ir.model.curves {
        references(curve, &ctx, &mut support)?;
    }
    let surfaces = ctx.collect_hash_map(
        ir.model
            .surfaces
            .iter()
            .map(|surface| (&surface.id, surface)),
        "Rhino source surface index",
    )?;
    let boundary_objects: BTreeSet<_> = ir
        .model
        .faces
        .iter()
        .filter_map(|face| {
            surfaces
                .get(&face.surface)?
                .source_object
                .as_ref()
                .map(|source| source.object_id.as_str())
        })
        .collect();
    let geometry_objects: BTreeSet<_> = boundary_objects
        .iter()
        .copied()
        .chain(ir.model.surfaces.iter().filter_map(|surface| {
            surface
                .source_object
                .as_ref()
                .map(|source| source.object_id.as_str())
        }))
        .collect();
    // Native B-reps may carry complete per-face render meshes. These are
    // geometry buffers, checked by the same archive decoder as exact faces.
    // A cache is admitted only when every source face has one nonempty mesh;
    // otherwise all source faces go through the exact boundary tessellator.
    let mut face_counts = HashMap::new();
    for face in &ir.model.faces {
        if let Some(source) = surfaces
            .get(&face.surface)
            .and_then(|surface| surface.source_object.as_ref())
            .filter(|source| source.visible != Some(false))
        {
            *face_counts
                .entry(source.object_id.as_str())
                .or_insert(0usize) += 1;
        }
    }
    let mut mesh_slots: HashMap<&str, BTreeSet<usize>> = HashMap::new();
    for mesh in &ir.model.tessellations {
        if let Some(source) = mesh
            .source_object
            .as_ref()
            .filter(|source| source.visible != Some(false))
            && mesh.vertex_count() > 0
            && mesh.triangle_count() > 0
            && let Some(slot) = render_slot(mesh)
        {
            mesh_slots
                .entry(source.object_id.as_str())
                .or_default()
                .insert(slot);
        }
    }
    let cached_objects: BTreeSet<_> = face_counts
        .iter()
        .filter_map(|(&id, &count)| {
            let slots = mesh_slots.get(id)?;
            (slots.len() == count && slots.iter().copied().eq(0..count)).then_some(id)
        })
        .collect();
    let mut scene = Scene::default();
    let mut evaluations = 0;
    for mesh in &ir.model.tessellations {
        if standalone(mesh.source_object.as_ref())
            && (independent(mesh.source_object.as_ref(), &boundary_objects)
                || mesh.source_object.as_ref().is_some_and(|source| {
                    cached_objects.contains(source.object_id.as_str())
                        && render_slot(mesh).is_some()
                }))
        {
            append_mesh(&mut scene, mesh)?;
        }
    }
    for point in &ir.model.points {
        if independent(point.source_object.as_ref(), &geometry_objects) {
            reserve(&scene, 1)?;
            scene.points.push(Point {
                position: vector(point.position().get()),
                color: object_color(point.source_object.as_ref()),
            });
        }
    }
    let curves = ctx.collect_hash_map(
        ir.model
            .curves
            .iter()
            .map(|curve| (curve.id.as_str(), curve)),
        "Rhino source curve index",
    )?;
    for curve in &ir.model.curves {
        if !support.contains(curve.id.as_str())
            && independent(curve.source_object.as_ref(), &geometry_objects)
        {
            append_curve(&mut scene, curve, &curves, &ctx, &mut evaluations)?;
        }
    }
    for surface in &ir.model.surfaces {
        if !support.contains(surface.id.as_str()) && standalone(surface.source_object.as_ref()) {
            append_surface(&mut scene, surface, &ctx, &mut evaluations)?;
        }
    }
    append_bodies(&mut scene, ir, &ctx, &cached_objects)?;
    scene.validate()?;
    Ok(scene)
}

/// Read the pinned codec's native render-cache face ordinal from its typed identity.
///
/// The codec names source B-rep render slots `.render-N` and analysis slots
/// `.analysis-N`. Only a complete, contiguous render slot array is admitted;
/// unfamiliar or instance-rewritten IDs use fresh boundary tessellation.
fn render_slot(mesh: &Tessellation) -> Option<usize> {
    mesh.id.as_str().rsplit_once(".render-")?.1.parse().ok()
}

/// Tessellate each visible exact body with only its reachable boundary entities.
#[allow(clippy::too_many_lines)]
fn append_bodies(
    scene: &mut Scene,
    ir: &CadIr,
    ctx: &DecodeContext<'_>,
    cached_objects: &BTreeSet<&str>,
) -> Result<()> {
    let mut graph = HashMap::new();
    macro_rules! index { ($($arena:ident),+ $(,)?) => { $(for entity in &ir.model.$arena { let mut refs = BTreeSet::new(); references(entity, ctx, &mut refs)?; if graph.len() >= MAX_GRAPH_NODES { return Err("Rhino boundary exceeds one million identities".into()); } ctx.insert_hash_map(&mut graph,entity.identity(), refs,"Rhino boundary identity graph")?; })+ }; }
    index!(
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
        procedural_curves
    );
    if ir.model.bodies.len() > 4096 {
        return Err("Rhino exceeds the 4096-body tessellation limit".into());
    }
    for body in &ir.model.bodies {
        if body.visible == Some(false) {
            continue;
        }
        let mut used = BTreeSet::new();
        let mut pending = vec![body.id.as_str().to_owned()];
        while let Some(id) = pending.pop() {
            ctx.charge_work(1, "Rhino boundary reachability")?;
            if used.insert(id.clone())
                && let Some(refs) = graph.get(id.as_str())
            {
                pending.extend(refs.iter().cloned());
            }
        }
        if !ir
            .model
            .faces
            .iter()
            .any(|face| used.contains(face.id.as_str()))
        {
            continue;
        }
        let cached = ir
            .model
            .faces
            .iter()
            .filter(|face| used.contains(face.id.as_str()))
            .all(|face| {
                ir.model
                    .surfaces
                    .iter()
                    .find(|surface| surface.id == face.surface)
                    .and_then(|surface| surface.source_object.as_ref())
                    .is_some_and(|source| cached_objects.contains(source.object_id.as_str()))
            });
        if cached {
            continue;
        }
        let mut boundary = CadIr::empty();
        boundary.tolerances = ir.tolerances;
        macro_rules! copy { ($($arena:ident),+ $(,)?) => { $(boundary.model.$arena = ir.model.$arena.iter().filter(|entity| used.contains(entity.identity())).cloned().collect();)+ }; }
        copy!(
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
            procedural_curves
        );
        // Rhino effective object color applies uniformly to the body. Applying it
        // after tessellation keeps unsupported STEP presentation representations
        // outside the geometry import while preserving the authored display value.
        for body in &mut boundary.model.bodies {
            body.color = None;
        }
        for face in &mut boundary.model.faces {
            face.color = None;
        }
        let output = super::native_cad::scene_from_ir(
            &boundary,
            super::native_cad::SurfaceNormalConvention::Parametric,
        )?;
        reserve(scene, output.triangles.len())?;
        let offset = scene.triangles.len();
        let flat = body.color.map(color);
        scene
            .triangles
            .extend(output.triangles.into_iter().map(|mut triangle| {
                triangle.color = flat;
                triangle
            }));
        scene.appearances.extend(
            output
                .appearances
                .into_iter()
                .map(|(index, appearance)| (offset + index, appearance)),
        );
    }
    Ok(())
}

/// Visit bounded native dependencies without interpreting generated identity strings.
fn references(
    value: &impl EntitySchema,
    ctx: &DecodeContext<'_>,
    output: &mut BTreeSet<String>,
) -> Result<()> {
    value.visit_references(ctx, &mut |identity| {
        if !output.contains(identity) {
            let identity = ctx.copy_retained_text(identity, "Rhino boundary reference")?;
            ctx.insert_btree_set(output, identity, "Rhino boundary reference set")?;
        }
        Ok(())
    })?;
    Ok(())
}

/// Identify visible standalone source geometry rather than boundary support carriers.
fn independent(
    source: Option<&SourceObjectAssociation>,
    boundary_objects: &BTreeSet<&str>,
) -> bool {
    source.is_some_and(|source| {
        source.visible != Some(false)
            && source.geometry_role != Some(SourceGeometryRole::Support)
            && !boundary_objects.contains(source.object_id.as_str())
    })
}

/// Convert native sRGB display colors to the scene's legacy byte colors.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn color(color: Color) -> [u8; 3] {
    [color.r(), color.g(), color.b()].map(|value| (value * 255.0).round() as u8)
}

/// Resolve the codec's already inherited object/layer/block display color.
fn object_color(source: Option<&SourceObjectAssociation>) -> Option<[u8; 3]> {
    source.and_then(|source| source.color).map(color)
}

/// Identify a visible native carrier whose geometry is independently displayed.
fn standalone(source: Option<&SourceObjectAssociation>) -> bool {
    source.is_some_and(|source| {
        source.visible != Some(false) && source.geometry_role != Some(SourceGeometryRole::Support)
    })
}

/// Convert finite neutral IR coordinates into the renderer's Z-up vectors.
fn vector(point: cadmpeg_ir::math::Point3) -> Vec3 {
    Vec3::new(point.x, point.y, point.z)
}

/// Bound aggregate geometry before every output expansion.
fn reserve(scene: &Scene, count: usize) -> Result<()> {
    if scene
        .triangles
        .len()
        .checked_add(scene.points.len())
        .and_then(|sum| sum.checked_add(scene.lines.len()))
        .and_then(|sum| sum.checked_add(count))
        .is_none_or(|sum| sum > MAX_PRIMITIVES)
    {
        return Err("Rhino geometry exceeds the five-million primitive limit".into());
    }
    let appearance_bytes = size_of::<TriangleAppearance>() + size_of::<usize>() + 64;
    let bytes = scene
        .triangles
        .len()
        .checked_mul(size_of::<Triangle>())
        .and_then(|bytes| bytes.checked_add(scene.points.len().checked_mul(size_of::<Point>())?))
        .and_then(|bytes| bytes.checked_add(scene.lines.len().checked_mul(size_of::<Line>())?))
        .and_then(|bytes| bytes.checked_add(scene.appearances.len().checked_mul(appearance_bytes)?))
        .and_then(|bytes| {
            bytes.checked_add(count.checked_mul(size_of::<Triangle>() + appearance_bytes)?)
        });
    if bytes.is_none_or(|bytes| bytes > MAX_SCENE_BYTES) {
        return Err("Rhino geometry exceeds the 512 MiB scene allocation limit".into());
    }
    Ok(())
}

/// Preserve source triangle winding, double-precision positions and vertex normals.
fn append_mesh(scene: &mut Scene, mesh: &Tessellation) -> Result<()> {
    if mesh.vertex_count() > MAX_VERTICES {
        return Err("Rhino mesh exceeds the five-million vertex limit".into());
    }
    reserve(scene, mesh.triangle_count())?;
    let vertices = mesh.vertices();
    let normals = mesh.vertex_normals();
    let flat = object_color(mesh.source_object.as_ref());
    for indices in mesh.triangles() {
        let indices = indices.map(|index| index as usize);
        let positions = indices.map(|index| vertices.get(index).map(|point| vector(point.get())));
        let [Some(a), Some(b), Some(c)] = positions else {
            return Err("Rhino mesh triangle references a missing vertex".into());
        };
        let index = scene.triangles.len();
        scene.triangles.push(Triangle {
            vertices: [a, b, c],
            color: flat,
        });
        if !normals.is_empty() {
            let shading = indices.map(|index| {
                normals.get(index).map(|normal| {
                    let normal = normal.get();
                    Vec3::new(normal.x, normal.y, normal.z).normalized()
                })
            });
            let [Some(a), Some(b), Some(c)] = shading else {
                return Err("Rhino mesh has incomplete source normals".into());
            };
            if [a, b, c].iter().all(|normal| normal.length() > 0.0) {
                scene.appearances.insert(
                    index,
                    TriangleAppearance {
                        normals: Some([a, b, c]),
                        ..TriangleAppearance::default()
                    },
                );
            }
        }
    }
    Ok(())
}

/// Transfer a source curve, recursively resolving polycurve members and placements.
fn append_curve(
    scene: &mut Scene,
    curve: &Curve,
    curves: &HashMap<&str, &Curve>,
    ctx: &DecodeContext<'_>,
    evaluations: &mut usize,
) -> Result<()> {
    let geometry = curve
        .geometry
        .solved()
        .ok_or("Rhino curve has no solved carrier")?;
    let mut active = BTreeSet::new();
    append_curve_geometry(
        scene,
        geometry,
        curve
            .parameter_range
            .map(|range| [range.lower(), range.upper()]),
        curves,
        Transform::identity(),
        object_color(curve.source_object.as_ref()),
        ctx,
        evaluations,
        &mut active,
        0,
    )
}

/// Resolve composites with a bounded stack and evaluate finite native knot spans.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn append_curve_geometry(
    scene: &mut Scene,
    geometry: &SolvedCurveGeometry,
    range: Option<[f64; 2]>,
    curves: &HashMap<&str, &Curve>,
    placement: Transform,
    color: Option<[u8; 3]>,
    ctx: &DecodeContext<'_>,
    evaluations: &mut usize,
    active: &mut BTreeSet<String>,
    depth: usize,
) -> Result<()> {
    if depth > MAX_CURVE_DEPTH {
        return Err("Rhino curve nesting exceeds 64 levels".into());
    }
    match geometry {
        SolvedCurveGeometry::Composite { segments, .. } => {
            for segment in &**segments {
                if !active.insert(segment.curve.as_str().to_owned()) {
                    return Err("cyclic Rhino polycurve reference".into());
                }
                let curve = curves
                    .get(segment.curve.as_str())
                    .ok_or("Rhino polycurve references a missing curve")?;
                append_curve_geometry(
                    scene,
                    curve
                        .geometry
                        .solved()
                        .ok_or("Rhino polycurve member has no solved carrier")?,
                    curve
                        .parameter_range
                        .map(|range| [range.lower(), range.upper()]),
                    curves,
                    placement,
                    color,
                    ctx,
                    evaluations,
                    active,
                    depth + 1,
                )?;
                active.remove(segment.curve.as_str());
            }
            return Ok(());
        }
        SolvedCurveGeometry::Transformed(placed) => {
            return append_curve_geometry(
                scene,
                placed.basis(),
                range,
                curves,
                placement.compose(*placed.transform())?,
                color,
                ctx,
                evaluations,
                active,
                depth + 1,
            );
        }
        SolvedCurveGeometry::Polyline(polyline) => {
            reserve(scene, polyline.point_count())?;
            let mut previous = None;
            for point in polyline.points() {
                let point = vector(
                    placement
                        .apply_point(point.get())
                        .ok_or("Rhino polyline placement overflow")?
                        .get(),
                );
                if let Some(previous) = previous {
                    scene.lines.push(Line {
                        vertices: [previous, point],
                        color,
                    });
                }
                previous = Some(point);
            }
            return Ok(());
        }
        _ => {}
    }
    let parameters = match geometry {
        SolvedCurveGeometry::Nurbs(curve) => knot_samples(
            curve.knots().as_slice(),
            curve.degree(),
            curve.pole_count(),
            SPAN_SAMPLES,
        )?,
        SolvedCurveGeometry::Circle(_) | SolvedCurveGeometry::Ellipse(_) => {
            uniform_samples(range.unwrap_or([0.0, std::f64::consts::TAU]), 256)?
        }
        SolvedCurveGeometry::Line(_) => {
            uniform_samples(range.ok_or("Rhino line has no finite source interval")?, 1)?
        }
        SolvedCurveGeometry::Parabola(_) | SolvedCurveGeometry::Hyperbola(_) => uniform_samples(
            range.ok_or("Rhino conic has no finite source interval")?,
            256,
        )?,
        SolvedCurveGeometry::Degenerate(point) => {
            reserve(scene, 1)?;
            scene.points.push(Point {
                position: vector(
                    placement
                        .apply_point(point.point().get())
                        .ok_or("Rhino point placement overflow")?
                        .get(),
                ),
                color,
            });
            return Ok(());
        }
        _ => return Err("unsupported Rhino curve carrier".into()),
    };
    charge_evaluations(evaluations, parameters.len())?;
    reserve(scene, parameters.len())?;
    let mut previous = None;
    for parameter in parameters {
        let point = finite_or_refusal(curve_point_solved(ctx, geometry, parameter))
            .map_err(cadmpeg_core::CodecError::ResourceLimit)?
            .ok_or("Rhino curve evaluation failed")?;
        let point = vector(
            placement
                .apply_point(point.get())
                .ok_or("Rhino curve placement overflow")?
                .get(),
        );
        if let Some(previous) = previous {
            scene.lines.push(Line {
                vertices: [previous, point],
                color,
            });
        }
        previous = Some(point);
    }
    Ok(())
}

/// Sample every nonempty spline knot span, preserving repeated-knot boundaries.
fn knot_samples(knots: &[f64], degree: u32, poles: usize, subdivisions: u32) -> Result<Vec<f64>> {
    if degree > 64 || knots.len() > MAX_EVALUATIONS {
        return Err("Rhino spline exceeds its degree or knot limit".into());
    }
    let degree = degree as usize;
    let domain = knots
        .get(degree..=poles)
        .ok_or("Rhino spline has an invalid knot domain")?;
    let mut samples = Vec::new();
    for interval in domain.windows(2) {
        if interval[1] > interval[0] {
            if samples.len() > MAX_EVALUATIONS - subdivisions as usize {
                return Err("Rhino spline exceeds its sample limit".into());
            }
            let mut span = uniform_samples(
                [interval[0], interval[1]],
                if degree == 1 { 1 } else { subdivisions },
            )?;
            if !samples.is_empty() {
                span.remove(0);
            }
            samples.extend(span);
        }
    }
    if samples.len() < 2 {
        return Err("Rhino spline has no finite nonempty knot spans".into());
    }
    Ok(samples)
}

/// Generate a bounded parameter interval without accumulated floating-point drift.
fn uniform_samples([start, end]: [f64; 2], count: u32) -> Result<Vec<f64>> {
    if !start.is_finite() || !end.is_finite() || end <= start || !(end - start).is_finite() {
        return Err("Rhino carrier has an invalid parameter interval".into());
    }
    Ok((0..=count)
        .map(|index| {
            if index == count {
                end
            } else {
                start + (end - start) * f64::from(index) / f64::from(count)
            }
        })
        .collect())
}

/// Charge cumulative native evaluations before scratch or output allocation.
fn charge_evaluations(total: &mut usize, count: usize) -> Result<()> {
    *total = total
        .checked_add(count)
        .ok_or("Rhino geometry evaluation count overflow")?;
    if *total > MAX_EVALUATIONS {
        return Err("Rhino geometry exceeds its four-million evaluation limit".into());
    }
    Ok(())
}

/// Tessellate independent native NURBS patches within their exact knot domains.
#[allow(clippy::many_single_char_names)]
fn append_surface(
    scene: &mut Scene,
    surface: &cadmpeg_ir::geometry::Surface,
    ctx: &DecodeContext<'_>,
    evaluations: &mut usize,
) -> Result<()> {
    let SolvedSurfaceGeometry::Nurbs(nurbs) = surface
        .geometry
        .solved()
        .ok_or("Rhino surface has no solved carrier")?
    else {
        return Err("Rhino free surface carrier has no qualified finite tessellation".into());
    };
    let u = knot_samples(
        nurbs.u_knots().as_slice(),
        nurbs.u_degree(),
        nurbs.u_count(),
        SURFACE_SPAN_SAMPLES,
    )?;
    let v = knot_samples(
        nurbs.v_knots().as_slice(),
        nurbs.v_degree(),
        nurbs.v_count(),
        SURFACE_SPAN_SAMPLES,
    )?;
    let count = u
        .len()
        .checked_mul(v.len())
        .ok_or("Rhino surface sample count overflow")?;
    charge_evaluations(evaluations, count)?;
    reserve(
        scene,
        (u.len() - 1)
            .checked_mul(v.len() - 1)
            .and_then(|count| count.checked_mul(2))
            .ok_or("Rhino surface triangle count overflow")?,
    )?;
    let mut points = Vec::with_capacity(count);
    for &u in &u {
        for &v in &v {
            points.push(vector(
                finite_or_refusal(cadmpeg_ir::eval::decode::surface_point(
                    ctx,
                    &surface.geometry,
                    u,
                    v,
                ))
                .map_err(cadmpeg_core::CodecError::ResourceLimit)?
                .ok_or("Rhino surface evaluation failed")?
                .get(),
            ));
        }
    }
    let color = object_color(surface.source_object.as_ref());
    for row in 0..u.len() - 1 {
        for col in 0..v.len() - 1 {
            let a = points[row * v.len() + col];
            let b = points[(row + 1) * v.len() + col];
            let c = points[(row + 1) * v.len() + col + 1];
            let d = points[row * v.len() + col + 1];
            for mut vertices in [[a, b, c], [a, c, d]] {
                if nurbs.normal_reversed() {
                    vertices.swap(1, 2);
                }
                scene.triangles.push(Triangle { vertices, color });
            }
        }
    }
    Ok(())
}
