//! Native CAD boundary geometry through a bounded Rust STEP import and mesh pass.
//!
//! This reader consumes Catwalk's generated AP242 projection. Product placements
//! and face colours come from the imported document. Numerical import reports are
//! admitted only when every warning has a measured, bounded explanation.

use crate::{
    Result,
    scene::{AlphaMode, Scene, Triangle, TriangleAppearance, Vec3},
};
use ogeom_core::Tolerances;
use ogeom_doc::{Colour, Document, ProductId, ProductKind};
use ogeom_io::{StepImport, StepReport};
use ogeom_topo::{Location, Model, Shape, ShapeType, TShapeId};
use std::{
    collections::{BTreeMap, HashSet},
    mem::size_of,
};

/// Validate the generated placement schema before upstream defaults can hide failures.
#[path = "native_step_parameters.rs"]
mod parameters;

/// Limit the generated input before allocating either exchange syntax or topology.
const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
/// Maximum retained native topology nodes.
const MAX_NODES: usize = 1_000_000;
/// Maximum distinct source boundary faces across a native document.
const MAX_FACES: usize = 100_000;
/// Match the existing Rust CAD mesh pass's per-body face admission.
const MAX_BODY_FACES: usize = 4096;
/// Bound worst-case edge discretization before the kernel allocates body-wide chords.
const MAX_EDGE_SAMPLES: usize = 4_000_000;
/// Shared upper bound for every edge's mesh discretization.
const MAX_SEGMENTS: usize = 4096;
/// Maximum root products before occurrence expansion.
const MAX_ROOTS: usize = 1024;
/// Maximum product or compound nesting.
const MAX_DEPTH: usize = 64;
/// Maximum expanded part occurrences.
const MAX_OCCURRENCES: usize = 4096;
/// Maximum emitted triangles.
const MAX_TRIANGLES: usize = 5_000_000;
/// Bound retained output including normals and appearance-map overhead.
const MAX_SCENE_BYTES: usize = 512 * 1024 * 1024;
/// Native geometry has a tighter chord bound than ordinary thumbnail STEP import.
const RELATIVE_CHORD: f64 = 0.0005;
/// Independent absolute cap on admitted native import approximation in millimetres.
const MAX_IMPORT_ERROR: f64 = 0.01;
/// The pinned kernel admits trim windows only when both endpoint feet miss by at most this amount.
const TRIM_ENDPOINT_ERROR: f64 = 0.001;
/// Standard millimetre numerical tolerances, independent of preview deflection.
const TOLERANCES: Tolerances = Tolerances::millimetres();

/// Import every generated source face, retain product placements, and mesh its trim.
///
/// # Errors
/// Excessive input, missing geometry, incomplete import, unsupported placements,
/// unknown or excessive numerical warnings, and mesh/resource failures are refused.
pub(super) fn load_bytes(bytes: &[u8]) -> Result<Scene> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err("native CAD STEP projection exceeds the 16 MiB geometry limit".into());
    }
    let text = std::str::from_utf8(bytes)?;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| import(text)))
        .map_err(|_| "native CAD geometry could not be tessellated by the Rust kernel")?
}

/// Keep infallible upstream geometry constructors within the importer's panic boundary.
fn import(text: &str) -> Result<Scene> {
    parameters::validate(text)?;
    let imported = ogeom_io::read_step(text, TOLERANCES)?;
    let model = imported.document.model();
    if model.node_count() > MAX_NODES {
        return Err("native CAD import exceeds one million topology nodes".into());
    }
    let occurrences = placed_parts(&imported.document)?;
    let source_faces = source_faces(&imported)?;
    let mut used = HashSet::new();
    let mut prepared = Vec::new();
    let mut error_limit = MAX_IMPORT_ERROR;
    for occurrence in occurrences {
        let faces = boundary_faces(model, &occurrence.shape)?;
        if faces.is_empty() || faces.len() > MAX_BODY_FACES {
            return Err("native CAD occurrence has no faces or exceeds 4096 faces".into());
        }
        admit_edge_sampling(model, &occurrence.shape)?;
        used.extend(faces.iter().map(Shape::node));
        let diagonal = ogeom_algo::shape_bounds(model, &occurrence.shape, TOLERANCES)?.diagonal();
        let chord = diagonal * RELATIVE_CHORD;
        if !chord.is_finite() || chord < TOLERANCES.confusion() * 100.0 {
            return Err("native CAD geometry is too small for the bounded chord tolerance".into());
        }
        error_limit = error_limit.min(chord);
        prepared.push((occurrence, faces.len(), chord));
    }
    if used != source_faces {
        return Err("native CAD product structure omitted or introduced source faces".into());
    }
    validate_report(&imported.report, error_limit)?;
    let mut scene = Scene::default();
    let mut retained_bytes = 0;
    for (occurrence, face_count, chord) in prepared {
        append_occurrence(
            &mut scene,
            &mut retained_bytes,
            &imported.document,
            &occurrence,
            face_count,
            chord,
        )?;
    }
    scene.validate()?;
    Ok(scene)
}

/// A bounded part use with all ancestor placements composed.
struct PlacedPart {
    /// Product identity for the document's per-face colour resolution.
    part: ProductId,
    /// Shape already placed in world space.
    shape: Shape,
}

/// Expand the product tree before invoking the kernel's unbounded flatten helper.
fn placed_parts(document: &Document) -> Result<Vec<PlacedPart>> {
    if document.products().count() > MAX_NODES {
        return Err("native CAD product count exceeds the topology limit".into());
    }
    for (_, product) in document.products() {
        if let ProductKind::Assembly { children } = &product.kind {
            for child in children {
                if document.get(child.product).is_none() {
                    return Err("native CAD product references a missing child".into());
                }
            }
        }
    }
    let roots = document.roots();
    if roots.is_empty() || roots.len() > MAX_ROOTS {
        return Err("native CAD document has no roots or exceeds 1024 roots".into());
    }
    let mut output = Vec::new();
    let mut active = HashSet::new();
    let mut work = 0;
    for root in roots {
        expand_product(
            document,
            root,
            &Location::identity(),
            0,
            &mut active,
            &mut work,
            &mut output,
        )?;
    }
    if output.is_empty() {
        return Err("native CAD product structure contains no placed geometry".into());
    }
    Ok(output)
}

/// Keep hierarchy depth, cycles and every expanded occurrence bounded.
fn expand_product(
    document: &Document,
    id: ProductId,
    placement: &Location,
    depth: usize,
    active: &mut HashSet<ProductId>,
    work: &mut usize,
    output: &mut Vec<PlacedPart>,
) -> Result<()> {
    *work += 1;
    if depth > MAX_DEPTH || *work > MAX_NODES {
        return Err("native CAD product hierarchy exceeds its depth or work limit".into());
    }
    if !active.insert(id) {
        return Err("native CAD product hierarchy contains a cycle".into());
    }
    let product = document.get(id).ok_or("native CAD product is missing")?;
    match &product.kind {
        ProductKind::Part { shape } => {
            if output.len() >= MAX_OCCURRENCES {
                return Err("native CAD exceeds 4096 placed part occurrences".into());
            }
            output.push(PlacedPart {
                part: id,
                shape: shape.moved(placement),
            });
        }
        ProductKind::Assembly { children } => {
            for child in children {
                let below = placement.then(&child.location);
                expand_product(
                    document,
                    child.product,
                    &below,
                    depth + 1,
                    active,
                    work,
                    output,
                )?;
            }
        }
    }
    active.remove(&id);
    Ok(())
}

/// Collect boundary faces without descending into their wire and edge topology.
fn boundary_faces(model: &Model, root: &Shape) -> Result<Vec<Shape>> {
    let mut pending = vec![(root.clone(), 0usize)];
    let mut faces = Vec::new();
    let mut work = 0;
    while let Some((shape, depth)) = pending.pop() {
        work += 1;
        if depth > MAX_DEPTH || work > MAX_NODES {
            return Err("native CAD topology traversal exceeds its depth or work limit".into());
        }
        let node = model
            .node(&shape)
            .ok_or("native CAD shape has missing topology")?;
        if node.kind() == ShapeType::Face {
            if faces.len() >= MAX_FACES {
                return Err("native CAD source exceeds 100,000 faces".into());
            }
            faces.push(shape);
        } else if node.kind() == ShapeType::Compound || node.kind() > ShapeType::Face {
            if pending.len().saturating_add(node.children().len()) > MAX_NODES {
                return Err("native CAD pending topology exceeds its work limit".into());
            }
            pending.extend(node.children().iter().rev().map(|child| {
                (
                    child.beneath(shape.location(), shape.orientation()),
                    depth + 1,
                )
            }));
        }
    }
    Ok(faces)
}

/// Every imported physical body must be covered by the placed document geometry.
fn source_faces(imported: &StepImport) -> Result<HashSet<TShapeId>> {
    let mut faces = HashSet::new();
    for shape in imported.solids.iter().chain(&imported.shells) {
        for face in boundary_faces(imported.document.model(), shape)? {
            faces.insert(face.node());
            if faces.len() > MAX_FACES {
                return Err("native CAD source exceeds 100,000 faces".into());
            }
        }
    }
    if faces.is_empty() {
        return Err("native CAD STEP projection contains no boundary geometry".into());
    }
    Ok(faces)
}

/// Refuse excessive source edge sampling before the body-wide mesh pass starts.
fn admit_edge_sampling(model: &Model, root: &Shape) -> Result<()> {
    let mut pending = vec![(root.clone(), 0usize)];
    let mut edges = HashSet::new();
    let mut work = 0;
    while let Some((shape, depth)) = pending.pop() {
        work += 1;
        if depth > MAX_DEPTH || work > MAX_NODES {
            return Err("native CAD edge traversal exceeds its depth or work limit".into());
        }
        let node = model
            .node(&shape)
            .ok_or("native CAD edge has missing topology")?;
        if node.kind() == ShapeType::Edge {
            edges.insert((shape.node(), shape.location().clone()));
            if edges.len() > MAX_EDGE_SAMPLES / MAX_SEGMENTS {
                return Err("native CAD source exceeds its four-million edge-sample budget".into());
            }
        } else if node.kind() == ShapeType::Compound || node.kind() > ShapeType::Edge {
            if pending.len().saturating_add(node.children().len()) > MAX_NODES {
                return Err("native CAD pending edges exceed their work limit".into());
            }
            pending.extend(node.children().iter().map(|child| {
                (
                    child.beneath(shape.location(), shape.orientation()),
                    depth + 1,
                )
            }));
        }
    }
    Ok(())
}

/// Refuse missing geometry and accept only fully accounted numerical deviations.
fn validate_report(report: &StepReport, limit: f64) -> Result<()> {
    if !report.scale_mm.is_finite()
        || (report.scale_mm - 1.0).abs() > 1e-12
        || !report.untrimmed_faces.is_empty()
    {
        return Err("native CAD STEP import has invalid units or untrimmed faces".into());
    }
    for name in report.skipped.keys() {
        if !matches!(
            name.as_str(),
            "APPLICATION_CONTEXT"
                | "APPLICATION_PROTOCOL_DEFINITION"
                | "AXIS2_PLACEMENT_3D"
                | "CARTESIAN_POINT"
                | "DIRECTION"
                | "GEOMETRIC_REPRESENTATION_CONTEXT"
                | "LENGTH_UNIT"
                | "NAMED_UNIT"
                | "PRODUCT_CONTEXT"
                | "PRODUCT_DEFINITION_CONTEXT"
                | "UNCERTAINTY_MEASURE_WITH_UNIT"
        ) {
            return Err(format!("native CAD STEP import omitted required {name} records").into());
        }
    }
    let mut counts = BTreeMap::new();
    for warning in &report.warnings {
        let (kind, distance) = numerical_warning(warning)
            .ok_or("native CAD STEP import reported an unqualified warning")?;
        let trim_window = kind == "vertex-window";
        if !distance.is_finite()
            || distance < 0.0
            || (trim_window && limit < TRIM_ENDPOINT_ERROR)
            || (!trim_window && distance > limit * 1.01)
        {
            return Err("native CAD STEP import warning exceeds its numerical tolerance".into());
        }
        *counts.entry(kind).or_insert(0usize) += 1;
    }
    for summary in &report.summary {
        if !summary.worst.is_finite()
            || summary.worst < 0.0
            || (summary.kind == "vertex-window" && limit < TRIM_ENDPOINT_ERROR)
            || (summary.kind != "vertex-window" && summary.worst > limit)
            || counts.remove(summary.kind) != Some(summary.count)
        {
            return Err(
                "native CAD STEP import numerical report is incomplete or excessive".into(),
            );
        }
    }
    if !counts.is_empty() {
        return Err("native CAD STEP import warnings lack a measured summary".into());
    }
    Ok(())
}

/// Match measured deviations and a separately qualified carrier trim-window event.
///
/// A window event measures distances from unused carrier ends, not a geometric
/// error. The pinned kernel separately limits both projected endpoint residuals
/// to 0.001 mm; this event is refused when that ceiling exceeds our error budget.
fn numerical_warning(value: &str) -> Option<(&'static str, f64)> {
    if let Some(value) = value.strip_prefix("an edge sits up to ") {
        let distance = value.strip_suffix(
            " from the surface it bounds; the file's own slop, carried into the chart",
        )?;
        return Some(("boundary-slop", distance.parse().ok()?));
    }
    if let Some(value) = value.strip_prefix("face #") {
        let (id, value) = value.split_once(": a projected pcurve fit stopped at ")?;
        id.parse::<u64>().ok()?;
        let distance =
            value.strip_suffix("; the face's mesh may sit that far off along this edge")?;
        return Some(("fit-short", distance.parse().ok()?));
    }
    let value = value.strip_prefix('#')?;
    if let Some((id, value)) = value.split_once(": edge endpoints sit ") {
        id.parse::<u64>().ok()?;
        let distances = value.strip_suffix(
            " from its curve's ends; the window between the vertices' feet on the curve was taken",
        )?;
        let (first, second) = distances.split_once(" and ")?;
        let first = first.parse::<f64>().ok()?;
        let second = second.parse::<f64>().ok()?;
        if !first.is_finite() || !second.is_finite() || first < 0.0 || second < 0.0 {
            return None;
        }
        return Some(("vertex-window", first.max(second)));
    }
    let (id, value) = value.split_once(": a curve end misses its vertex by ")?;
    id.parse::<u64>().ok()?;
    let distance = value.strip_suffix("; the vertex tolerance grew to say so")?;
    Some(("vertex-miss", distance.parse().ok()?))
}

/// Mesh complete face trims in world space, retaining document appearance.
fn append_occurrence(
    scene: &mut Scene,
    bytes: &mut usize,
    document: &Document,
    occurrence: &PlacedPart,
    expected_faces: usize,
    chord: f64,
) -> Result<()> {
    // Small curved patches can need finer angular sampling before their mesh
    // meets the same chord bound. The second pass increases precision; it
    // never changes the chord target or accepts a failed deflection flag.
    let mut accepted = None;
    for angular in [0.1, 0.05] {
        let deflection = ogeom_mesh::Deflection {
            chord,
            angular,
            min_segments: 4,
            max_segments: MAX_SEGMENTS,
        };
        let (meshes, _) = ogeom_mesh::face_meshes_for(
            document.model(),
            &occurrence.shape,
            deflection,
            None,
            TOLERANCES,
        )?;
        if meshes.len() != expected_faces {
            return Err("native CAD tessellation omitted a required face".into());
        }
        for (_, mesh) in &meshes {
            let mesh = mesh.as_ref().map_err(ToString::to_string)?;
            if mesh.triangles.is_empty() {
                return Err("native CAD tessellation omitted a required face's geometry".into());
            }
        }
        if meshes
            .iter()
            .all(|(_, mesh)| mesh.as_ref().is_ok_and(|mesh| mesh.deflection_met))
        {
            accepted = Some(meshes);
            break;
        }
        drop(meshes);
    }
    let meshes = accepted
        .ok_or("native CAD faces failed both angular passes at the requested chord tolerance")?;
    for (face, mesh) in meshes {
        let mesh = mesh?;
        if mesh.triangles.is_empty() || !mesh.deflection_met {
            return Err(format!(
                "native CAD face {:?} failed its requested {chord} mm chord tolerance",
                face.node()
            )
            .into());
        }
        let colour = document.resolved_colour(occurrence.part, &face);
        append_mesh(scene, bytes, &mesh, colour)?;
    }
    Ok(())
}

/// Admit output allocation before retaining triangles, normals and colour attributes.
fn reserve_output(
    scene: &mut Scene,
    bytes: &mut usize,
    triangles: usize,
    appearance: bool,
) -> Result<()> {
    if triangles > MAX_TRIANGLES.saturating_sub(scene.triangles.len()) {
        return Err("native CAD preview exceeds five million triangles".into());
    }
    let per_triangle = size_of::<Triangle>()
        + if appearance {
            size_of::<TriangleAppearance>() + 4 * size_of::<usize>()
        } else {
            0
        };
    let additional = triangles
        .checked_mul(per_triangle)
        .ok_or("native CAD scene size overflow")?;
    let retained = bytes
        .checked_add(additional)
        .ok_or("native CAD scene size overflow")?;
    if retained > MAX_SCENE_BYTES {
        return Err("native CAD preview exceeds its 512 MiB scene limit".into());
    }
    scene.triangles.try_reserve_exact(triangles)?;
    *bytes = retained;
    Ok(())
}

/// Copy only validated mesh arrays; the kernel already applies occurrence transforms.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Validated unit colour channels are rounded to an unsigned eight-bit source colour."
)]
fn append_mesh(
    scene: &mut Scene,
    bytes: &mut usize,
    mesh: &ogeom_topo::Triangulation,
    colour: Option<Colour>,
) -> Result<()> {
    if colour.is_some_and(|c| {
        [c.r, c.g, c.b, c.a]
            .into_iter()
            .any(|channel| !channel.is_finite() || !(0.0..=1.0).contains(&channel))
    }) {
        return Err("native CAD colour is outside its finite unit range".into());
    }
    if !mesh.normals.is_empty() && mesh.normals.len() != mesh.positions.len() {
        return Err("native CAD mesh has incomplete normals".into());
    }
    let has_appearance = !mesh.normals.is_empty() || colour.is_some();
    reserve_output(scene, bytes, mesh.triangles.len(), has_appearance)?;
    for indices in &mesh.triangles {
        let vertices = indices.map(|i| {
            mesh.positions
                .get(i as usize)
                .map(|p| Vec3::new(p.x, p.y, p.z))
                .ok_or("native CAD mesh index is invalid")
        });
        let [a, b, c] = vertices;
        let vertices = [a?, b?, c?];
        let normals = if mesh.normals.is_empty() {
            None
        } else {
            let normals = indices.map(|i| {
                mesh.normals
                    .get(i as usize)
                    .map(|p| Vec3::new(p.x, p.y, p.z))
                    .ok_or("native CAD normal index is invalid")
            });
            let [a, b, c] = normals;
            Some([a?, b?, c?])
        };
        let rgb = colour.map(|c| [c.r, c.g, c.b].map(|channel| (channel * 255.0).round() as u8));
        scene.triangles.push(Triangle {
            vertices,
            color: rgb,
        });
        if has_appearance {
            let mut appearance = TriangleAppearance {
                normals,
                ..TriangleAppearance::default()
            };
            if let Some(c) = colour {
                let linear = |channel: f64| {
                    if channel <= 0.04045 {
                        channel / 12.92
                    } else {
                        ((channel + 0.055) / 1.055).powf(2.4)
                    }
                };
                appearance.base_color = [linear(c.r), linear(c.g), linear(c.b), c.a];
                if c.a < 1.0 {
                    appearance.alpha_mode = AlphaMode::Blend;
                }
            }
            scene
                .appearances
                .insert(scene.triangles.len() - 1, appearance);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "native_step_tests.rs"]
mod tests;
