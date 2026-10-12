//! Stored FreeCAD ASCII BREP results, native object placement and diffuse appearance.
//!
//! Persistence follows FreeCAD's typed property writers; scene selection is
//! qualified against the original F3D FCStd reader and its numerical fixtures.
//! <https://github.com/f3d-app/f3d/blob/efb898d0943b4d73b8ef707fb333292493151ef8/plugins/occt/module/vtkF3DFCStdReader.cxx>

/// Addressed legacy face colors and version-three material appearance.
#[path = "fcstd_appearance.rs"]
mod appearance;
/// Typed object declarations, ownership, visibility and rigid placements.
#[path = "fcstd_document.rs"]
mod document;

use super::{
    cad::{self, ShapeStyle},
    fcstd_container::DocumentArchive,
};
use crate::{Result, scene::Scene};
use document::{MAX_COORDINATE, MAX_OBJECTS, ObjectDocument};
use ogeom_math::Transform;
use ogeom_topo::{Model, Shape};
use std::{collections::BTreeMap, mem::size_of, path::Path};

/// Maximum one native shape, or aggregate decoded shape bytes retained across the document.
const MAX_BREP: usize = 16 * 1024 * 1024;
/// Maximum aggregate native shape topology records admitted before import.
const MAX_TOPOLOGY: usize = 100_000;
/// Maximum aggregate shape lexical records used for conservative retained-memory admission.
const MAX_TOKENS: usize = 2_000_000;
/// Maximum stored render-cache and placed-output primitives.
const MAX_PRIMITIVES: usize = 1_000_000;
/// Maximum aggregate native topology traversals charged to selected occurrences.
const MAX_WORK: usize = 2_000_000;
/// Maximum conservative retained source, topology, cached and output geometry allocation.
/// Upstream CAD tessellation retains its own numerical and subdivision bounds;
/// this budget does not bound its temporary internal allocations.
const MAX_ALLOCATION: usize = 512 * 1024 * 1024;

/// Validated public kernel objects for one exact addressed shape member.
struct StoredShape {
    /// Owned upstream topology and geometry arenas, never patched or vendored.
    model: Model,
    /// Complete native root occurrence including placement and orientation.
    root: Shape,
    /// Number of native topology records charged to each selected placement.
    topology: usize,
}

/// Neutral-root tessellation cached separately for each bounded qualified shape style.
struct RenderedShape {
    /// Shared-edge tessellation at identity root placement.
    scene: Scene,
    /// Original stored root transform, applied only for normal feature occurrences.
    root: Transform,
    /// Native topology records charged to each selected placement.
    topology: usize,
}

/// Cumulative admission before source parsing, cache growth or placed output allocation.
#[derive(Default)]
struct Budget {
    /// Aggregate decoded native BREP payload bytes.
    source: usize,
    /// Aggregate native topology records.
    topology: usize,
    /// Aggregate BREP lexical tokens.
    tokens: usize,
    /// Cached and placed primitive records.
    primitives: usize,
    /// Selected occurrence count.
    instances: usize,
    /// Charged topology and point-transform work.
    work: usize,
    /// Conservative cumulative allocation estimate, including vector-growth overlap.
    allocation: usize,
}

impl Budget {
    /// Admit an original native shape before its unchanged kernel objects are allocated.
    ///
    /// # Errors
    ///
    /// Aggregate source, lexical, topology or allocation limits are exceeded.
    fn source(&mut self, text: &str) -> Result<usize> {
        let tokens = text.split_whitespace().count();
        let mut words = text.split_whitespace();
        let topology = words
            .find(|word| *word == "TShapes")
            .and_then(|_| words.next())
            .ok_or("FCStd shape lacks native topology table")?
            .parse::<usize>()?;
        charge(
            &mut self.source,
            text.len(),
            MAX_BREP,
            "aggregate native shape bytes",
        )?;
        charge(
            &mut self.topology,
            topology,
            MAX_TOPOLOGY,
            "aggregate topology",
        )?;
        charge(
            &mut self.tokens,
            tokens,
            MAX_TOKENS,
            "aggregate native lexical records",
        )?;
        let allocation = text
            .len()
            .checked_mul(3)
            .and_then(|value| {
                tokens
                    .checked_mul(64)
                    .and_then(|tokens| value.checked_add(tokens))
            })
            .and_then(|value| {
                topology
                    .checked_mul(1024)
                    .and_then(|topology| value.checked_add(topology))
            })
            .ok_or("FCStd native allocation estimate overflow")?;
        charge(
            &mut self.allocation,
            allocation,
            MAX_ALLOCATION,
            "retained allocation",
        )?;
        Ok(topology)
    }

    /// Admit cached geometry or a new occurrence before its output vectors are extended.
    ///
    /// # Errors
    ///
    /// Cached/output geometry counts, conservative allocation or occurrence-work limits are exceeded.
    fn geometry(&mut self, scene: &Scene, placed: bool, topology: usize) -> Result<()> {
        let primitives = scene
            .triangles
            .len()
            .checked_add(scene.lines.len())
            .and_then(|value| value.checked_add(scene.points.len()))
            .ok_or("FCStd primitive count overflow")?;
        charge(
            &mut self.primitives,
            primitives,
            MAX_PRIMITIVES,
            "cached and placed primitives",
        )?;
        let allocation = scene
            .triangles
            .len()
            .checked_mul(size_of::<crate::scene::Triangle>())
            .and_then(|value| {
                scene
                    .lines
                    .len()
                    .checked_mul(size_of::<crate::scene::Line>())
                    .and_then(|lines| value.checked_add(lines))
            })
            .and_then(|value| {
                scene
                    .points
                    .len()
                    .checked_mul(size_of::<crate::scene::Point>())
                    .and_then(|points| value.checked_add(points))
            })
            .and_then(|value| {
                scene
                    .appearances
                    .len()
                    .checked_mul(size_of::<crate::scene::TriangleAppearance>() + 64)
                    .and_then(|appearances| value.checked_add(appearances))
            })
            .and_then(|value| value.checked_mul(3))
            .ok_or("FCStd geometry allocation estimate overflow")?;
        charge(
            &mut self.allocation,
            allocation,
            MAX_ALLOCATION,
            "retained allocation",
        )?;
        if placed {
            charge(
                &mut self.instances,
                1,
                MAX_OBJECTS,
                "10000 selected instances",
            )?;
            charge(&mut self.work, topology, MAX_WORK, "selected topology work")?;
            charge(
                &mut self.work,
                primitives
                    .checked_mul(3)
                    .ok_or("FCStd placement work overflow")?,
                MAX_WORK,
                "selected transform work",
            )?;
        }
        Ok(())
    }
}

/// Read native `FreeCAD` result shapes without executing features, scripts or external resources.
///
/// # Errors
///
/// Container/ownership/schema, selected object modes, original shape import, placements or limits fail.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let (mut archive, document) = open_document(path)?;
    let mut shapes = BTreeMap::new();
    let mut rendered = BTreeMap::new();
    let mut budget = Budget::default();
    let mut scene = Scene::default();
    for index in 0..document.objects.len() {
        let object = &document.objects[index];
        if !selected(&document, index)? {
            continue;
        }
        let ancestors = document.ancestors(index);
        let placements = if object.shape.is_none()
            && object.placements.is_some()
            && (object.link.is_some() || object.base.is_some())
        {
            read_placements(
                &mut archive,
                object
                    .placements
                    .as_deref()
                    .ok_or("FCStd array lacks placement member")?,
            )?
        } else {
            Vec::new()
        };
        if !placements.is_empty() {
            validate_array_defaults(&mut archive, object, placements.len())?;
            if object
                .element_count
                .is_some_and(|count| count != 0 && count != placements.len())
            {
                return Err("FCStd declared array count disagrees with placement list".into());
            }
            let target = target(&document, index)?;
            let source = &document.objects[target];
            let file = source
                .shape
                .as_deref()
                .ok_or("FCStd array target has no stored shape")?;
            let style = array_style(object, source);
            let geometry = render(
                &mut archive,
                &mut shapes,
                &mut rendered,
                &mut budget,
                file,
                &style,
            )?;
            if let Some(geometry) = geometry {
                for placement in placements {
                    let mut root = object.placement * placement;
                    if object.link_transform {
                        root = root * source.placement;
                    }
                    append(&mut scene, geometry, ancestors * root, &mut budget)?;
                }
            }
        } else if object.link.is_some() {
            if object.element_count.is_some_and(|count| count != 0) {
                return Err("FCStd selected link array lacks stored placements".into());
            }
            let target = target(&document, index)?;
            let source = &document.objects[target];
            let file = source
                .shape
                .as_deref()
                .ok_or("FCStd link target has no stored shape")?;
            let root = if object.link_transform {
                object.link_placement * source.placement
            } else {
                object.link_placement
            };
            if let Some(geometry) = render(
                &mut archive,
                &mut shapes,
                &mut rendered,
                &mut budget,
                file,
                &source.style,
            )? {
                append(&mut scene, geometry, ancestors * root, &mut budget)?;
            }
        } else if let Some(file) = &object.shape {
            if let Some(geometry) = render(
                &mut archive,
                &mut shapes,
                &mut rendered,
                &mut budget,
                file,
                &object.style,
            )? {
                append(&mut scene, geometry, ancestors * geometry.root, &mut budget)?;
            }
        } else if object.base.is_some() {
            return Err("FCStd selected array lacks nonempty stored placements".into());
        }
    }
    scene.validate()?;
    Ok(scene)
}

/// Open bounded native document ownership and apply its optional qualified GUI metadata.
///
/// # Errors
///
/// Container identity, object associations or optional GUI appearance fail native admission.
fn open_document(path: &Path) -> Result<(DocumentArchive, ObjectDocument)> {
    let mut archive = DocumentArchive::open(path)?;
    let mut document = ObjectDocument::parse(archive.document())?;
    appearance::apply(&mut archive, &mut document)?;
    Ok((archive, document))
}

/// Select visible native results once and reject unsupported or ambiguous object ownership.
///
/// # Errors
///
/// Visible body placement, selected geometry modes or stored/link ownership is unqualified.
fn selected(document: &ObjectDocument, index: usize) -> Result<bool> {
    let object = &document.objects[index];
    let visible = document.visible(index);
    if visible && object.kind == "PartDesign::Body" && object.placement != Transform::IDENTITY {
        return Err("FCStd nonidentity PartDesign body placement is unqualified".into());
    }
    if !visible || (object.shape.is_some() && document.redundant(index)) {
        return Ok(false);
    }
    if object.unsupported {
        return Err(format!(
            "FCStd selected object has unsupported inline, mesh or scaling geometry: {}",
            object.name
        )
        .into());
    }
    if object.shape.is_some() && object.link.is_some() {
        return Err("FCStd stored Shape and LinkedObject geometry ownership is ambiguous".into());
    }
    Ok(true)
}

/// Inherit source uniform appearance when an array contributes no explicit GUI override.
fn array_style(object: &document::Object, source: &document::Object) -> ShapeStyle {
    let mut style = object.style.clone();
    style.faces.clear();
    if !object.authored.color {
        style.color = source.style.color;
    }
    if !object.authored.opacity {
        style.alpha = source.style.alpha;
    }
    style
}

/// Resolve one qualified local target without silently following external, nested or scaled links.
///
/// # Errors
///
/// Missing, external, subelement, nested-link, container or unqualified targets are rejected.
fn target(document: &ObjectDocument, index: usize) -> Result<usize> {
    let object = &document.objects[index];
    let name = if let Some(link) = &object.link {
        if link.unsupported {
            return Err("FCStd external or subelement link geometry is unsupported".into());
        }
        &link.target
    } else {
        object
            .base
            .as_ref()
            .ok_or("FCStd array lacks source object association")?
    };
    let &target = document
        .names
        .get(name)
        .ok_or("FCStd selected link target is missing")?;
    let source = &document.objects[target];
    if target == index || source.link.is_some() || source.unsupported || !source.children.is_empty()
    {
        return Err("FCStd selected target is cyclic, nested, scaled or a container".into());
    }
    Ok(target)
}

/// Import and tessellate a selected member once per native shape/style pair at identity root.
///
/// # Errors
///
/// Source admission, native ASCII topology, selected display or unchanged CAD tessellation fails.
fn render<'a>(
    archive: &mut DocumentArchive,
    shapes: &mut BTreeMap<String, Option<StoredShape>>,
    rendered: &'a mut BTreeMap<(String, ShapeStyle), RenderedShape>,
    budget: &mut Budget,
    file: &str,
    style: &ShapeStyle,
) -> Result<Option<&'a RenderedShape>> {
    if !shapes.contains_key(file) {
        let bytes = archive.read(file, MAX_BREP)?;
        let stored = if bytes.is_empty() {
            None
        } else {
            if file.to_ascii_lowercase().ends_with(".bin") || super::xbf::is_binary_brep(&bytes) {
                return Err("binary FCStd shape trailers are unsupported".into());
            }
            let text = std::str::from_utf8(&bytes).map_err(|error| {
                format!("FCStd addressed BREP member {file} has invalid ASCII text: {error}")
            })?;
            let topology = budget.source(text)?;
            let (model, root) = cad::read_brep_model(text)?;
            Some(StoredShape {
                model,
                root,
                topology,
            })
        };
        shapes.insert(file.to_owned(), stored);
    }
    let Some(source) = shapes.get(file).and_then(Option::as_ref) else {
        return Ok(None);
    };
    let key = (file.to_owned(), style.clone());
    if !rendered.contains_key(&key) {
        let mut scene = Scene::default();
        cad::append_shape_styled(&mut scene, &source.model, &source.root, style)?;
        if style.alpha != 255 && (!scene.lines.is_empty() || !scene.points.is_empty()) {
            return Err("FCStd transparent free curves or points are unsupported".into());
        }
        budget.geometry(&scene, false, source.topology)?;
        let root = source.root.transform(source.model.datums())?;
        let inverse = root.inverse()?;
        // Native pcurve association uses the original composed root location.
        // Preserve it throughout meshing, then remove only its affine effect
        // from emitted positions before applying normal or linked occurrences.
        for triangle in &mut scene.triangles {
            for vertex in &mut triangle.vertices {
                *vertex = placed(*vertex, inverse)?;
            }
        }
        for line in &mut scene.lines {
            for vertex in &mut line.vertices {
                *vertex = placed(*vertex, inverse)?;
            }
        }
        for point in &mut scene.points {
            point.position = placed(point.position, inverse)?;
        }
        rendered.insert(
            key.clone(),
            RenderedShape {
                scene,
                root,
                topology: source.topology,
            },
        );
    }
    Ok(rendered.get(&key))
}

/// Place an admitted cached original shape without changing native triangle winding or appearance.
///
/// # Errors
///
/// Aggregate output, retained allocation, work or finite-coordinate limits fail before emission.
fn append(
    scene: &mut Scene,
    source: &RenderedShape,
    placement: Transform,
    budget: &mut Budget,
) -> Result<()> {
    budget.geometry(&source.scene, true, source.topology)?;
    scene
        .triangles
        .try_reserve_exact(source.scene.triangles.len())?;
    scene.lines.try_reserve_exact(source.scene.lines.len())?;
    scene.points.try_reserve_exact(source.scene.points.len())?;
    let offset = scene.triangles.len();
    for triangle in &source.scene.triangles {
        let mut triangle = triangle.clone();
        for vertex in &mut triangle.vertices {
            *vertex = placed(*vertex, placement)?;
        }
        scene.triangles.push(triangle);
    }
    for line in &source.scene.lines {
        let mut line = line.clone();
        for vertex in &mut line.vertices {
            *vertex = placed(*vertex, placement)?;
        }
        scene.lines.push(line);
    }
    for point in &source.scene.points {
        let mut point = point.clone();
        point.position = placed(point.position, placement)?;
        scene.points.push(point);
    }
    for (&index, appearance) in &source.scene.appearances {
        scene.appearances.insert(offset + index, appearance.clone());
    }
    Ok(())
}

/// Apply a qualified finite rigid placement to one native tessellation point.
///
/// # Errors
///
/// A placed coordinate is nonfinite or exceeds the preview coordinate bound.
fn placed(point: crate::scene::Vec3, placement: Transform) -> Result<crate::scene::Vec3> {
    let value = placement.apply(ogeom_math::Point::new(point.x, point.y, point.z));
    let point = crate::scene::Vec3::new(value.x, value.y, value.z);
    if !point.is_finite()
        || [point.x, point.y, point.z]
            .into_iter()
            .any(|value| value.abs() > MAX_COORDINATE)
    {
        return Err("FCStd placed geometry has nonfinite or excessive coordinates".into());
    }
    Ok(point)
}

/// Decode an exact addressed native f64 placement list before reserving its bounded records.
///
/// # Errors
///
/// Selected source, count, exact extent or finite rigid placement components are invalid.
fn read_placements(archive: &mut DocumentArchive, file: &str) -> Result<Vec<Transform>> {
    let bytes = archive.read(file, 4 + MAX_OBJECTS * 56)?;
    let count = usize::try_from(u32::from_le_bytes(
        bytes
            .get(..4)
            .ok_or("FCStd truncated placement list count")?
            .try_into()?,
    ))?;
    if count > MAX_OBJECTS
        || count.checked_mul(56).and_then(|value| value.checked_add(4)) != Some(bytes.len())
    {
        return Err("FCStd invalid or excessive placement list count/extent".into());
    }
    let mut result = Vec::with_capacity(count);
    for values in bytes[4..].as_chunks::<56>().0 {
        let mut components = [0.0; 7];
        for (value, bytes) in components.iter_mut().zip(values.as_chunks::<8>().0) {
            *value = f64::from_le_bytes(*bytes);
        }
        result.push(document::rigid(
            [components[0], components[1], components[2]],
            [components[3], components[4], components[5], components[6]],
        )?);
    }
    Ok(result)
}

/// Admit qualified identity per-instance scales and default/all-visible array masks.
///
/// # Errors
///
/// Selected list addressing, types, count/extent, finite scalars or unsupported instance effects fail.
fn validate_array_defaults(
    archive: &mut DocumentArchive,
    object: &document::Object,
    instances: usize,
) -> Result<()> {
    if let Some(bits) = &object.visibility_bits {
        if bits.len() > instances {
            return Err("FCStd visibility bit count exceeds selected instance count".into());
        }
        if bits.contains('0') {
            return Err("FCStd hidden array-instance visibility is unsupported".into());
        }
    }
    let Some(file) = &object.scales else {
        return Ok(());
    };
    let bytes = archive.read(file, 4 + MAX_OBJECTS * 24)?;
    let count = usize::try_from(u32::from_le_bytes(
        bytes
            .get(..4)
            .ok_or("FCStd truncated scale list count")?
            .try_into()?,
    ))?;
    if count > instances
        || count.checked_mul(24).and_then(|value| value.checked_add(4)) != Some(bytes.len())
    {
        return Err("FCStd invalid or excessive scale list count/extent".into());
    }
    for bytes in bytes[4..].as_chunks::<8>().0 {
        let scale = f64::from_le_bytes(*bytes);
        if !scale.is_finite() {
            return Err("FCStd nonfinite array-instance scale".into());
        }
        if !document::identity_scale(scale) {
            return Err("FCStd nonidentity array-instance scale is unsupported".into());
        }
    }
    Ok(())
}

/// Admit a cumulative count before allocation, expansion or geometry traversal.
///
/// # Errors
///
/// Count arithmetic overflows or the specified document budget is exceeded.
fn charge(count: &mut usize, additional: usize, limit: usize, label: &str) -> Result<()> {
    *count = count
        .checked_add(additional)
        .filter(|&value| value <= limit)
        .ok_or_else(|| format!("FCStd exceeds {label} limit"))?;
    Ok(())
}
