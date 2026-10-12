//! Static COLLADA scenes, including indexed geometry and instanced transforms.
use std::collections::{HashMap, HashSet};
use std::path::Path;

use roxmltree::{Document, Node, NodeId};

use super::mesh::triangulate_polygon;
use super::transform::Transform;
use crate::scene::{Line, Scene, Triangle, Vec3};
use crate::{Error, Result};

/// Maximum scalar/index list length or individual geometry count.
const MAX_ITEMS: usize = 20_000_000;
/// Maximum visited scene nodes or index lists in new wire/hole primitives.
const MAX_INSTANCES: usize = 100_000;
/// Maximum shared input tuple width or primitive input count.
const MAX_INPUTS: usize = 64;
/// Maximum corners across one polygon and all of its holes.
const MAX_CORNERS: usize = 100_000;
/// Maximum independent rings describing one polygon.
const MAX_RINGS: usize = 1024;
/// Maximum retained numeric sources, mesh primitives and instanced output bytes.
const MAX_STORAGE: usize = 256 * 1024 * 1024;
/// Maximum material, source or semantic identifier length.
const MAX_NAME: usize = 4096;

/// Checked allocation budget shared by decoded source arrays and geometry.
#[derive(Default)]
struct Budget {
    /// Conservatively retained allocation bytes.
    bytes: usize,
}

impl Budget {
    /// Account for retained storage before allocating it.
    fn charge(&mut self, count: usize, size: usize) -> Result<()> {
        self.bytes = count
            .checked_mul(size)
            .and_then(|bytes| self.bytes.checked_add(bytes))
            .ok_or_else(|| error("COLLADA storage size overflow"))?;
        if self.bytes > MAX_STORAGE {
            return Err(error(
                "COLLADA geometry and sources exceed the 256 MiB storage budget",
            ));
        }
        Ok(())
    }
}

/// Grow geometry geometrically while charging all requested spare capacity before allocation.
fn grow<T>(values: &mut Vec<T>, additional: usize, budget: &mut Budget) -> Result<()> {
    let needed = values
        .len()
        .checked_add(additional)
        .ok_or_else(|| error("COLLADA geometry count overflow"))?;
    if needed > MAX_ITEMS {
        return Err(error("too much COLLADA geometry"));
    }
    if needed > values.capacity() {
        let capacity = needed.max(values.capacity().saturating_mul(2)).max(4);
        budget.charge(capacity - values.capacity(), std::mem::size_of::<T>())?;
        let previous = values.capacity();
        values.try_reserve_exact(capacity - values.len())?;
        if values.capacity() > capacity {
            budget.charge(values.capacity() - capacity, std::mem::size_of::<T>())?;
        }
        debug_assert!(values.capacity() >= previous);
    }
    Ok(())
}

/// Load static COLLADA geometry, node transforms and material colors.
///
/// # Errors
/// Returns an error for invalid XML, missing references, malformed numeric arrays,
/// unsupported transforms or primitives, excessive instances, and invalid geometry.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    match super::extension(path)?.as_str() {
        "dae" => collada(std::str::from_utf8(&bytes)?),
        extension => Err(format!("unsupported legacy scene format .{extension}").into()),
    }
}

/// Attach COLLADA context to an import error.
fn error(message: impl Into<String>) -> Error {
    message.into().into()
}
/// Iterate element children with the requested local tag name.
fn children<'a, 'i>(node: Node<'a, 'i>, name: &str) -> impl Iterator<Item = Node<'a, 'i>> {
    let name = name.to_owned();
    node.children()
        .filter(move |n| n.is_element() && n.tag_name().name() == name)
}
/// Require the first element child with the requested tag name.
fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Result<Node<'a, 'i>> {
    children(node, name).next().ok_or_else(|| {
        error(format!(
            "COLLADA <{}> lacks <{name}>",
            node.tag_name().name()
        ))
    })
}
/// Require an XML attribute by name.
fn attribute<'a>(node: Node<'a, '_>, name: &str) -> Result<&'a str> {
    node.attribute(name)
        .ok_or_else(|| error(format!("COLLADA <{}> lacks {name}", node.tag_name().name())))
}
/// Parse a nonnegative count and enforce the scalar item budget.
fn item_count(value: &str) -> Result<usize> {
    let count: usize = value.parse()?;
    if count > MAX_ITEMS {
        return Err(error("COLLADA item count exceeds 20 million"));
    }
    Ok(count)
}
/// Borrow ordinary list text or concatenate bounded text segments across comments without adding whitespace.
fn list_text<'a>(node: Node<'a, '_>) -> Result<std::borrow::Cow<'a, str>> {
    let mut segments = node
        .children()
        .filter(roxmltree::Node::is_text)
        .filter_map(|node| node.text());
    let first = segments.next().unwrap_or("");
    if segments.next().is_none() {
        return Ok(std::borrow::Cow::Borrowed(first));
    }
    let mut size = 0_usize;
    for text in node
        .children()
        .filter(roxmltree::Node::is_text)
        .filter_map(|node| node.text())
    {
        size = size
            .checked_add(text.len())
            .ok_or_else(|| error("COLLADA numeric text size overflow"))?;
        if size as u64 > super::MAX_INPUT_BYTES {
            return Err(error("COLLADA numeric text exceeds the input byte limit"));
        }
    }
    let mut text = String::with_capacity(size);
    for segment in node
        .children()
        .filter(roxmltree::Node::is_text)
        .filter_map(|node| node.text())
    {
        text.push_str(segment);
    }
    Ok(std::borrow::Cow::Owned(text))
}

/// Parse finite whitespace-separated floating point values.
fn numbers(node: Node<'_, '_>) -> Result<Vec<f64>> {
    let mut values = Vec::new();
    let text = list_text(node)?;
    for word in text.split_whitespace() {
        let value: f64 = word.parse()?;
        if !value.is_finite() {
            return Err(error("non-finite COLLADA numeric value"));
        }
        values.push(value);
        if values.len() > MAX_ITEMS {
            return Err(error("too many COLLADA numeric values"));
        }
    }
    Ok(values)
}
/// Parse bounded nonnegative index values.
fn indices(node: Node<'_, '_>) -> Result<Vec<usize>> {
    let mut values = Vec::new();
    let text = list_text(node)?;
    for word in text.split_whitespace() {
        values.push(item_count(word)?);
        if values.len() > MAX_ITEMS {
            return Err(error("too many COLLADA indices"));
        }
    }
    Ok(values)
}
/// Resolve a local fragment reference without allowing external resources.
fn reference(value: &str) -> Result<&str> {
    value
        .strip_prefix('#')
        .filter(|s| !s.is_empty())
        .ok_or_else(|| error("COLLADA references must address an ID in the same file"))
}

#[derive(Debug)]
/// Numeric source accessor into a COLLADA float array.
struct Source {
    /// Underlying finite scalar values.
    values: Vec<f64>,
    /// Number of accessible source entries.
    count: usize,
    /// Scalar spacing between entries.
    stride: usize,
    /// First scalar used by the accessor.
    offset: usize,
    /// Named scalar slots in authored order; names do not bind semantic components.
    components: Vec<usize>,
}
impl Source {
    /// Read three ordered named scalar slots according to the consuming input semantic.
    fn vector(&self, index: usize) -> Result<Vec3> {
        if index >= self.count {
            return Err(error("COLLADA source index out of bounds"));
        }
        let mut components = [0.0; 3];
        for (i, component) in components.iter_mut().enumerate() {
            let position = *self
                .components
                .get(i)
                .ok_or_else(|| error("COLLADA semantic requires three named scalar components"))?;
            if position >= self.stride {
                return Err(error("COLLADA source component exceeds accessor stride"));
            }
            let offset = index
                .checked_mul(self.stride)
                .and_then(|n| n.checked_add(self.offset))
                .and_then(|n| n.checked_add(position))
                .ok_or_else(|| error("COLLADA source offset overflow"))?;
            *component = *self
                .values
                .get(offset)
                .ok_or_else(|| error("COLLADA accessor exceeds its array"))?;
        }
        Ok(Vec3::new(components[0], components[1], components[2]))
    }
    /// Read an RGB source entry and encode it for display.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Color components are clamped and rounded to the inclusive byte range before conversion."
    )]
    fn color(&self, index: usize) -> Result<[u8; 3]> {
        let c = self.vector(index)?;
        Ok([c.x, c.y, c.z].map(|c| (c * 255.0).clamp(0.0, 255.0).round() as u8))
    }
}
/// Parse a float source and validate its accessor count, stride, offset and parameters.
fn parse_source(
    node: Node<'_, '_>,
    ids: &HashMap<&str, Node<'_, '_>>,
    budget: &mut Budget,
) -> Result<Source> {
    let technique = child(node, "technique_common")?;
    let accessor = child(technique, "accessor")?;
    let array_id = reference(attribute(accessor, "source")?)?;
    let array = *ids
        .get(array_id)
        .ok_or_else(|| error("COLLADA accessor references a missing array"))?;
    if array.tag_name().name() != "float_array" {
        return Err(error("COLLADA mesh accessor needs a float_array"));
    }
    let array_count = item_count(attribute(array, "count")?)?;
    let text = list_text(array)?;
    if text.split_whitespace().count() != array_count {
        return Err(error("COLLADA float_array count differs from its data"));
    }
    budget.charge(array_count, std::mem::size_of::<f64>())?;
    let mut values = Vec::with_capacity(array_count);
    for word in text.split_whitespace() {
        if values.len() == array_count {
            return Err(error("COLLADA float_array count differs from its data"));
        }
        let value: f64 = word.parse()?;
        if !value.is_finite() {
            return Err(error("non-finite COLLADA numeric value"));
        }
        values.push(value);
    }
    if values.len() != array_count {
        return Err(error("COLLADA float_array count differs from its data"));
    }
    let count = item_count(attribute(accessor, "count")?)?;
    let stride = accessor
        .attribute("stride")
        .map(item_count)
        .transpose()?
        .unwrap_or(1);
    let offset = accessor
        .attribute("offset")
        .map(item_count)
        .transpose()?
        .unwrap_or(0);
    if stride == 0 {
        return Err(error("COLLADA accessor stride must be positive"));
    }
    let mut components = Vec::new();
    for (position, param) in children(accessor, "param").enumerate() {
        if position >= stride || position >= MAX_CORNERS {
            return Err(error(
                "COLLADA accessor parameters exceed stride or item limits",
            ));
        }
        if !matches!(param.attribute("type"), Some("float" | "double")) {
            return Err(error(
                "COLLADA geometry accessor parameters must be scalar floats",
            ));
        }
        if param.attribute("name").is_some() {
            grow(&mut components, 1, budget)?;
            components.push(position);
        }
    }
    let used = components.last().map_or(0, |position| position + 1);
    let end = if count == 0 || used == 0 {
        offset
    } else {
        (count - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(offset))
            .and_then(|n| n.checked_add(used))
            .ok_or_else(|| error("COLLADA accessor size overflow"))?
    };
    if end > values.len() {
        return Err(error("COLLADA accessor exceeds its array"));
    }
    Ok(Source {
        values,
        count,
        stride,
        offset,
        components,
    })
}

#[derive(Clone)]
/// One decoded mesh triangle before node transforms and material binding.
struct MeshTriangle {
    /// Vertices in mesh-local coordinates.
    vertices: [Vec3; 3],
    /// Optional average vertex color.
    color: Option<[u8; 3]>,
    /// Material symbol referenced by the primitive.
    material: Option<std::sync::Arc<str>>,
}

/// One mesh-local wire segment before placement and material binding.
struct MeshLine {
    /// Authored line endpoints.
    vertices: [Vec3; 2],
    /// Optional mean endpoint RGB color.
    color: Option<[u8; 3]>,
    /// Shared material symbol for the owning primitive.
    material: Option<std::sync::Arc<str>>,
}

/// Static mesh surfaces and wire geometry.
#[derive(Default)]
struct Mesh {
    /// Polygon-derived mesh triangles.
    triangles: Vec<MeshTriangle>,
    /// Authored line pairs or consecutive strip segments.
    lines: Vec<MeshLine>,
}
/// One primitive input identifying a source, semantic and interleaved index offset.
struct Input {
    /// Resolved source ID without the fragment marker.
    source: String,
    /// COLLADA input semantic such as vertex, position or color.
    semantic: String,
    /// Offset within each interleaved index tuple.
    offset: usize,
    /// Attribute set used to choose among multiple color inputs.
    set: usize,
}
/// Parse primitive inputs and validate bounded offsets and attribute sets.
fn inputs(node: Node<'_, '_>) -> Result<Vec<Input>> {
    let mut result = Vec::new();
    for input in children(node, "input") {
        if result.len() >= MAX_INPUTS {
            return Err(error("too many COLLADA primitive inputs"));
        }
        let source = reference(attribute(input, "source")?)?;
        let semantic = attribute(input, "semantic")?;
        if source.len() > MAX_NAME || semantic.len() > MAX_NAME {
            return Err(error("COLLADA input identifier exceeds name limits"));
        }
        let value = Input {
            source: source.to_owned(),
            semantic: semantic.to_owned(),
            offset: input
                .attribute("offset")
                .map(item_count)
                .transpose()?
                .unwrap_or(0),
            set: input
                .attribute("set")
                .map(item_count)
                .transpose()?
                .unwrap_or(0),
        };
        if value.offset >= MAX_INPUTS {
            return Err(error("COLLADA input tuple width exceeds 64"));
        }
        result.push(value);
    }
    Ok(result)
}

/// Parse a limited index list before allocating beyond one bounded primitive or ring.
fn limited_indices(node: Node<'_, '_>, limit: usize) -> Result<Vec<usize>> {
    let mut result = Vec::new();
    let text = list_text(node)?;
    for word in text.split_whitespace() {
        if result.len() >= limit.min(MAX_ITEMS) {
            return Err(error("COLLADA primitive index list exceeds its limit"));
        }
        result.push(item_count(word)?);
    }
    Ok(result)
}

/// Decode complete shared index tuples and validate every referenced geometry attribute.
fn decoded_corners(
    raw: &[usize],
    width: usize,
    inputs: &[Input],
    sources: &HashMap<String, Source>,
) -> Result<(Vec<Vec3>, Vec<[u8; 3]>)> {
    if !raw.len().is_multiple_of(width) {
        return Err(error("COLLADA indices do not form complete input tuples"));
    }
    let position = inputs
        .iter()
        .find(|input| input.semantic == "POSITION")
        .ok_or_else(|| error("COLLADA primitive has no position source"))?;
    let color = inputs
        .iter()
        .filter(|input| input.semantic == "COLOR")
        .min_by_key(|input| input.set);
    let mut positions = Vec::with_capacity(raw.len() / width);
    let mut colors = Vec::new();
    for tuple in raw.chunks_exact(width) {
        for input in inputs {
            if tuple[input.offset] >= sources[&input.source].count {
                return Err(error("COLLADA input index out of bounds"));
            }
        }
        positions.push(sources[&position.source].vector(tuple[position.offset])?);
        if let Some(input) = color {
            colors.push(sources[&input.source].color(tuple[input.offset])?);
        }
    }
    Ok((positions, colors))
}

/// Average legacy byte RGB colors without changing existing COLLADA material behavior.
#[allow(
    clippy::cast_possible_truncation,
    reason = "A mean of byte color channels remains representable in a byte."
)]
fn mean_color(colors: &[[u8; 3]]) -> Option<[u8; 3]> {
    (!colors.is_empty()).then(|| {
        std::array::from_fn(|channel| {
            (colors
                .iter()
                .map(|color| u64::from(color[channel]))
                .sum::<u64>()
                / colors.len() as u64) as u8
        })
    })
}

/// Decode bounded line pairs or strips into authored segments with independent source indices.
fn wire_primitive(
    node: Node<'_, '_>,
    width: usize,
    inputs: &[Input],
    sources: &HashMap<String, Source>,
    mesh: &mut Mesh,
    budget: &mut Budget,
    material: Option<&std::sync::Arc<str>>,
) -> Result<()> {
    let count = item_count(attribute(node, "count")?)?;
    let pairs = node.tag_name().name() == "lines";
    let lists: Vec<_> = children(node, "p").take(MAX_INSTANCES + 1).collect();
    if lists.len() > MAX_INSTANCES
        || (pairs && (lists.len() > 1 || (count != 0 && lists.is_empty())))
        || (!pairs && lists.len() != count)
    {
        return Err(error(
            "COLLADA wire primitive count differs from its index lists",
        ));
    }
    for list in lists {
        let limit = if pairs {
            count
                .checked_mul(2)
                .and_then(|count| count.checked_mul(width))
                .ok_or_else(|| error("COLLADA line count overflow"))?
        } else {
            MAX_CORNERS * width
        };
        if limit > MAX_ITEMS {
            return Err(error(
                "COLLADA line index list exceeds the scalar item limit",
            ));
        }
        let raw = limited_indices(list, limit)?;
        if pairs && raw.len() != limit {
            return Err(error("COLLADA line count differs from its indices"));
        }
        if pairs && count == 0 {
            continue;
        }
        if !raw.len().is_multiple_of(width) {
            return Err(error("COLLADA indices do not form complete input tuples"));
        }
        let corner_count = raw.len() / width;
        if corner_count < 2 {
            return Err(error("COLLADA line strip requires at least two corners"));
        }
        let segments = if pairs {
            corner_count / 2
        } else {
            corner_count - 1
        };
        // Reject excessive output before expanding all endpoint positions and colors.
        grow(&mut mesh.lines, segments, budget)?;
        let (positions, colors) = decoded_corners(&raw, width, inputs, sources)?;
        for segment in 0..segments {
            let first = if pairs { segment * 2 } else { segment };
            mesh.lines.push(MeshLine {
                vertices: [positions[first], positions[first + 1]],
                color: if colors.is_empty() {
                    None
                } else {
                    mean_color(&colors[first..first + 2])
                },
                material: material.cloned(),
            });
        }
    }
    Ok(())
}

/// Decode planar exterior and hole rings without filling their authored openings.
fn polygons_with_holes(
    node: Node<'_, '_>,
    width: usize,
    inputs: &[Input],
    sources: &HashMap<String, Source>,
    mesh: &mut Mesh,
    budget: &mut Budget,
    material: Option<&std::sync::Arc<str>>,
) -> Result<()> {
    let polygons: Vec<_> = node
        .children()
        .filter(|node| node.is_element() && matches!(node.tag_name().name(), "p" | "ph"))
        .take(MAX_INSTANCES + 1)
        .collect();
    if polygons.len() > MAX_INSTANCES || polygons.len() != item_count(attribute(node, "count")?)? {
        return Err(error("COLLADA polygon count differs from its boundaries"));
    }
    for polygon in polygons {
        let boundaries = if polygon.tag_name().name() == "ph" {
            let exterior = child(polygon, "p")?;
            if children(polygon, "p").count() != 1 {
                return Err(error("COLLADA hole polygon needs exactly one exterior"));
            }
            let mut boundaries = vec![exterior];
            for hole in children(polygon, "h") {
                if boundaries.len() >= MAX_RINGS {
                    return Err(error("COLLADA polygon has too many rings"));
                }
                boundaries.push(hole);
            }
            if boundaries.len() == 1 {
                return Err(error("COLLADA ph polygon needs at least one hole"));
            }
            boundaries
        } else {
            vec![polygon]
        };
        let mut rings = Vec::new();
        let mut color = None;
        let mut corners = 0_usize;
        for (index, boundary) in boundaries.into_iter().enumerate() {
            let raw = limited_indices(boundary, (MAX_CORNERS - corners) * width)?;
            let (mut positions, colors) = decoded_corners(&raw, width, inputs, sources)?;
            corners = corners
                .checked_add(positions.len())
                .ok_or_else(|| error("COLLADA ring corner count overflow"))?;
            if positions.len() > 1 && positions.first() == positions.last() {
                positions.pop();
            }
            if positions.len() < 3 {
                return Err(error(
                    "COLLADA polygon ring requires at least three corners",
                ));
            }
            if index > 0 && !hole_has_area(&positions) {
                return Err(error("COLLADA zero-area hole rings are unsupported"));
            }
            if index == 0 {
                color = mean_color(&colors);
            }
            rings.push(positions);
        }
        let triangles = if polygon.tag_name().name() == "ph" {
            super::citygml::triangulate_rings(&rings)?
        } else {
            triangulate_polygon(&rings[0])?
        };
        grow(&mut mesh.triangles, triangles.len(), budget)?;
        mesh.triangles
            .extend(triangles.into_iter().map(|vertices| MeshTriangle {
                vertices,
                color,
                material: material.cloned(),
            }));
    }
    Ok(())
}

/// Reject undefined zero signed-area holes using a linear, scale-resistant ring-area sum.
/// This does not impose a blanket self-intersection rule on COLLADA polygons.
fn hole_has_area(ring: &[Vec3]) -> bool {
    let origin = ring[0];
    let mut scale = 0.0_f64;
    let mut overflow = false;
    for &position in ring {
        let edge = position - origin;
        overflow |= !edge.is_finite();
        scale = scale.max(edge.x.abs()).max(edge.y.abs()).max(edge.z.abs());
    }
    if overflow {
        scale = ring.iter().fold(0.0_f64, |scale, position| {
            scale
                .max(position.x.abs())
                .max(position.y.abs())
                .max(position.z.abs())
        });
    }
    if scale == 0.0 {
        return false;
    }
    let relative = |position: Vec3| {
        if overflow {
            position / scale - origin / scale
        } else {
            (position - origin) / scale
        }
    };
    let mut area = Vec3::default();
    for index in 0..ring.len() {
        area = area + relative(ring[index]).cross(relative(ring[(index + 1) % ring.len()]));
    }
    area.length() > 0.0
}
/// Decode indexed triangles, polygons, strips and fans from a COLLADA mesh.
#[allow(
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    reason = "The bounded primitive parser keeps index and source validation together; averages of byte colors stay within byte range."
)]
fn parse_mesh(
    node: Node<'_, '_>,
    ids: &HashMap<&str, Node<'_, '_>>,
    budget: &mut Budget,
) -> Result<Mesh> {
    let mut sources = HashMap::new();
    for source in children(node, "source") {
        let id = attribute(source, "id")?;
        if id.len() > MAX_NAME || sources.len() >= MAX_INSTANCES {
            return Err(error(
                "COLLADA source identifiers or source count exceed limits",
            ));
        }
        budget.charge(
            3,
            std::mem::size_of::<Source>() + std::mem::size_of::<String>(),
        )?;
        budget.charge(id.len(), 1)?;
        sources.insert(id.to_owned(), parse_source(source, ids, budget)?);
    }
    let mut vertex_inputs = HashMap::new();
    for vertices in children(node, "vertices") {
        let id = attribute(vertices, "id")?;
        if id.len() > MAX_NAME || vertex_inputs.len() >= MAX_INSTANCES {
            return Err(error(
                "COLLADA vertices identifiers or declaration count exceed limits",
            ));
        }
        let entries = inputs(vertices)?;
        budget.charge(
            3,
            std::mem::size_of::<Vec<Input>>() + std::mem::size_of::<String>(),
        )?;
        budget.charge(entries.capacity(), std::mem::size_of::<Input>())?;
        budget.charge(id.len(), 1)?;
        for entry in &entries {
            budget.charge(entry.source.len() + entry.semantic.len(), 1)?;
        }
        vertex_inputs.insert(id.to_owned(), entries);
    }
    let mut result = Mesh::default();
    for primitive in node.children().filter(roxmltree::Node::is_element) {
        let name = primitive.tag_name().name();
        if matches!(name, "source" | "vertices" | "extra") {
            continue;
        }
        if !matches!(
            name,
            "triangles"
                | "polylist"
                | "polygons"
                | "trifans"
                | "tristrips"
                | "lines"
                | "linestrips"
        ) {
            return Err(error(format!(
                "unsupported COLLADA mesh primitive <{name}>"
            )));
        }
        let count = item_count(attribute(primitive, "count")?)?;
        let material = primitive
            .attribute("material")
            .map(|symbol| -> Result<std::sync::Arc<str>> {
                if symbol.len() > MAX_NAME {
                    return Err(error("COLLADA material symbol exceeds name limits"));
                }
                budget.charge(symbol.len() + 2 * std::mem::size_of::<usize>(), 1)?;
                Ok(symbol.into())
            })
            .transpose()?;
        let raw_inputs = inputs(primitive)?;
        let width = raw_inputs
            .iter()
            .map(|input| input.offset)
            .max()
            .ok_or_else(|| error("COLLADA primitive has no inputs"))?
            .checked_add(1)
            .ok_or_else(|| error("COLLADA index tuple width overflow"))?;
        let mut expanded = Vec::new();
        for input in raw_inputs {
            if input.semantic == "VERTEX" {
                for vertex_input in vertex_inputs
                    .get(&input.source)
                    .ok_or_else(|| error("COLLADA primitive references missing vertices"))?
                {
                    if expanded.len() >= MAX_INPUTS {
                        return Err(error("COLLADA expanded primitive inputs exceed 64"));
                    }
                    expanded.push(Input {
                        source: vertex_input.source.clone(),
                        semantic: vertex_input.semantic.clone(),
                        offset: input.offset,
                        set: vertex_input.set,
                    });
                }
            } else {
                if expanded.len() >= MAX_INPUTS {
                    return Err(error("COLLADA expanded primitive inputs exceed 64"));
                }
                expanded.push(input);
            }
        }
        let positions: Vec<_> = expanded
            .iter()
            .filter(|input| input.semantic == "POSITION")
            .collect();
        if positions.len() != 1 {
            return Err(error("COLLADA primitive needs exactly one POSITION input"));
        }
        let position = positions[0];
        let position_source = sources
            .get(&position.source)
            .ok_or_else(|| error("COLLADA POSITION references a missing source"))?;
        let color_input = expanded
            .iter()
            .filter(|i| i.semantic == "COLOR")
            .min_by_key(|i| i.set);
        let color_source = color_input
            .map(|input| {
                sources
                    .get(&input.source)
                    .ok_or_else(|| error("COLLADA COLOR references a missing source"))
            })
            .transpose()?;
        // All required source references and indices are checked, including the
        // normals/UVs which a flat thumbnail does not use for appearance.
        for input in &expanded {
            if !sources.contains_key(&input.source) {
                return Err(error("COLLADA input references a missing source"));
            }
        }
        if matches!(name, "lines" | "linestrips") {
            wire_primitive(
                primitive,
                width,
                &expanded,
                &sources,
                &mut result,
                budget,
                material.as_ref(),
            )?;
            continue;
        }
        if name == "polygons" && children(primitive, "ph").next().is_some() {
            polygons_with_holes(
                primitive,
                width,
                &expanded,
                &sources,
                &mut result,
                budget,
                material.as_ref(),
            )?;
            continue;
        }
        let p_nodes: Vec<_> = children(primitive, "p").collect();
        let mut polygons = Vec::new();
        match name {
            "triangles" => {
                let all: Vec<usize> = p_nodes
                    .iter()
                    .map(|p| indices(*p))
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .flatten()
                    .collect();
                let expected = count
                    .checked_mul(3)
                    .and_then(|n| n.checked_mul(width))
                    .ok_or_else(|| error("COLLADA triangle count overflow"))?;
                if all.len() != expected {
                    return Err(error("COLLADA triangles count differs from its indices"));
                }
                for triangle in all.chunks_exact(3 * width) {
                    polygons.push(triangle.to_vec());
                }
            }
            "polylist" => {
                if p_nodes.len() != 1 {
                    return Err(error("COLLADA polylist needs one index list"));
                }
                let counts = indices(child(primitive, "vcount")?)?;
                if counts.len() != count {
                    return Err(error("COLLADA polylist count differs from vcount"));
                }
                let all = indices(p_nodes[0])?;
                let mut cursor: usize = 0;
                for n in counts {
                    if n < 3 {
                        return Err(error("COLLADA polygon has fewer than three vertices"));
                    }
                    let end = n
                        .checked_mul(width)
                        .and_then(|n| cursor.checked_add(n))
                        .ok_or_else(|| error("COLLADA polygon offset overflow"))?;
                    polygons.push(
                        all.get(cursor..end)
                            .ok_or_else(|| error("truncated COLLADA polylist"))?
                            .to_vec(),
                    );
                    cursor = end;
                }
                if cursor != all.len() {
                    return Err(error("extra indices after COLLADA polylist"));
                }
            }
            "polygons" => {
                if children(primitive, "ph").next().is_some() {
                    return Err(error("COLLADA polygons with holes are not supported"));
                }
                if p_nodes.len() != count {
                    return Err(error("COLLADA polygons count differs from its lists"));
                }
                polygons = p_nodes.into_iter().map(indices).collect::<Result<_>>()?;
            }
            "trifans" | "tristrips" => {
                if p_nodes.len() != count {
                    return Err(error("COLLADA strip/fan count differs from its lists"));
                }
                for p in p_nodes {
                    let all = indices(p)?;
                    if all.len() % width != 0 || all.len() / width < 3 {
                        return Err(error("invalid COLLADA strip/fan indices"));
                    }
                    let tuples: Vec<_> = all.chunks_exact(width).collect();
                    for k in 2..tuples.len() {
                        let triplet = if name == "trifans" {
                            [0, k - 1, k]
                        } else if k % 2 == 0 {
                            [k - 2, k - 1, k]
                        } else {
                            [k - 1, k - 2, k]
                        };
                        polygons.push(
                            triplet
                                .into_iter()
                                .flat_map(|i| tuples[i].iter().copied())
                                .collect(),
                        );
                    }
                }
            }
            _ => unreachable!(),
        }
        for polygon in polygons {
            if polygon.len() % width != 0 {
                return Err(error(
                    "COLLADA polygon indices do not form complete input tuples",
                ));
            }
            let mut vertices = Vec::new();
            let mut colors = Vec::new();
            for tuple in polygon.chunks_exact(width) {
                for input in &expanded {
                    if tuple[input.offset] >= sources[&input.source].count {
                        return Err(error("COLLADA input index out of bounds"));
                    }
                }
                vertices.push(position_source.vector(tuple[position.offset])?);
                if let (Some(input), Some(source)) = (color_input, color_source) {
                    colors.push(source.color(tuple[input.offset])?);
                }
            }
            let color = if colors.is_empty() {
                None
            } else {
                Some(std::array::from_fn(|i| {
                    (colors.iter().map(|color| u64::from(color[i])).sum::<u64>()
                        / colors.len() as u64) as u8
                }))
            };
            for vertices in triangulate_polygon(&vertices)? {
                grow(&mut result.triangles, 1, budget)?;
                result.triangles.push(MeshTriangle {
                    vertices,
                    color,
                    material: material.clone(),
                });
                if result.triangles.len() > MAX_ITEMS {
                    return Err(error("too many COLLADA triangles"));
                }
            }
        }
    }
    Ok(result)
}

/// Compose a node's ordered affine transform elements.
#[allow(
    clippy::float_cmp,
    reason = "The COLLADA matrix affine row requires exact protocol constants."
)]
fn node_transform(node: Node<'_, '_>) -> Result<Transform> {
    let mut result = Transform::IDENTITY;
    for element in node.children().filter(roxmltree::Node::is_element) {
        let name = element.tag_name().name();
        if !matches!(
            name,
            "matrix" | "translate" | "rotate" | "scale" | "lookat" | "skew"
        ) {
            continue;
        }
        let values = numbers(element)?;
        let mut matrix = Transform::IDENTITY.0;
        match name {
            "matrix" => {
                if values.len() != 16 {
                    return Err(error("COLLADA matrix needs sixteen numbers"));
                }
                // XML is row-major; Transform stores columns.
                matrix = std::array::from_fn(|column| {
                    std::array::from_fn(|row| values[row * 4 + column])
                });
                if matrix[0][3] != 0.0
                    || matrix[1][3] != 0.0
                    || matrix[2][3] != 0.0
                    || matrix[3][3] != 1.0
                {
                    return Err(error("COLLADA projective node matrices are not supported"));
                }
            }
            "translate" | "scale" => {
                if values.len() != 3 {
                    return Err(error(format!("COLLADA {name} needs three numbers")));
                }
                for i in 0..3 {
                    if name == "translate" {
                        matrix[3][i] = values[i];
                    } else {
                        matrix[i][i] = values[i];
                    }
                }
            }
            "rotate" => {
                if values.len() != 4 {
                    return Err(error("COLLADA rotate needs an axis and angle"));
                }
                let axis = Vec3::new(values[0], values[1], values[2]);
                if axis.length() == 0.0 {
                    return Err(error("COLLADA rotation axis is zero"));
                }
                let axis = axis.normalized();
                let (sin, cos) = values[3].to_radians().sin_cos();
                let a = [axis.x, axis.y, axis.z];
                let row_major = [
                    [
                        cos + a[0] * a[0] * (1. - cos),
                        a[0] * a[1] * (1. - cos) - a[2] * sin,
                        a[0] * a[2] * (1. - cos) + a[1] * sin,
                    ],
                    [
                        a[1] * a[0] * (1. - cos) + a[2] * sin,
                        cos + a[1] * a[1] * (1. - cos),
                        a[1] * a[2] * (1. - cos) - a[0] * sin,
                    ],
                    [
                        a[2] * a[0] * (1. - cos) - a[1] * sin,
                        a[2] * a[1] * (1. - cos) + a[0] * sin,
                        cos + a[2] * a[2] * (1. - cos),
                    ],
                ];
                for column in 0..3 {
                    for row in 0..3 {
                        matrix[column][row] = row_major[row][column];
                    }
                }
            }
            "lookat" => matrix = lookat(&values)?.0,
            "skew" => matrix = skew(&values)?.0,
            _ => unreachable!(),
        }
        result = result.then(Transform(matrix));
        if result.0.iter().flatten().any(|value| !value.is_finite()) {
            return Err(error("COLLADA transform overflow"));
        }
    }
    Ok(result)
}

/// Normalize finite transform directions without overflowing or underflowing their magnitudes.
fn direction(vector: Vec3) -> Result<Vec3> {
    let scale = vector.x.abs().max(vector.y.abs()).max(vector.z.abs());
    if !scale.is_finite() || scale == 0.0 {
        return Err(error(
            "COLLADA transform direction must be finite and nonzero",
        ));
    }
    Ok((vector / scale).normalized())
}

/// Decode a lookat node placement, preserving eye translation and its local negative-Z forward axis.
fn lookat(values: &[f64]) -> Result<Transform> {
    if values.len() != 9 {
        return Err(error("COLLADA lookat needs eye, interest and up vectors"));
    }
    let eye = Vec3::new(values[0], values[1], values[2]);
    let target = Vec3::new(values[3], values[4], values[5]);
    let mut forward = target - eye;
    if !forward.is_finite() {
        let scale = [eye, target].iter().fold(0.0_f64, |scale, position| {
            scale
                .max(position.x.abs())
                .max(position.y.abs())
                .max(position.z.abs())
        });
        forward = target / scale - eye / scale;
    }
    let forward = direction(forward)?;
    let up = direction(Vec3::new(values[6], values[7], values[8]))?;
    let right = direction(forward.cross(up))?;
    let up = direction(right.cross(forward))?;
    let backward = -forward;
    Ok(Transform([
        [right.x, right.y, right.z, 0.0],
        [up.x, up.y, up.z, 0.0],
        [backward.x, backward.y, backward.z, 0.0],
        [eye.x, eye.y, eye.z, 1.0],
    ]))
}

/// Decode `RenderMan` angular skew for normalized, possibly nonorthogonal authored axes.
/// The rotation direction tilts toward the translation direction by the specified degrees.
fn skew(values: &[f64]) -> Result<Transform> {
    if values.len() != 7 {
        return Err(error("COLLADA skew needs degrees and two axis vectors"));
    }
    let rotation = direction(Vec3::new(values[1], values[2], values[3]))?;
    let translation = direction(Vec3::new(values[4], values[5], values[6]))?;
    let cosine = rotation.dot(translation).clamp(-1.0, 1.0);
    let sine = rotation.cross(translation).length();
    if sine == 0.0 {
        return Err(error("COLLADA skew axes must not be parallel"));
    }
    let separation = sine.atan2(cosine);
    let angle = values[0].to_radians();
    if angle == 0.0 {
        return Ok(Transform::IDENTITY);
    }
    if angle <= separation - std::f64::consts::PI || angle >= separation {
        return Err(error(
            "COLLADA skew angle makes the transformed axes singular or reverses their order",
        ));
    }
    let perpendicular = direction(translation.cross(rotation.cross(translation)))?;
    // cot(phi-angle)-cot(phi), expressed without subtracting large cotangents.
    let gain = (angle.sin() / sine) / (separation - angle).sin();
    if !gain.is_finite() {
        return Err(error("COLLADA skew coefficient exceeds numeric range"));
    }
    let t = [translation.x, translation.y, translation.z];
    let n = [perpendicular.x, perpendicular.y, perpendicular.z];
    let mut matrix = Transform::IDENTITY.0;
    for column in 0..3 {
        for row in 0..3 {
            matrix[column][row] += gain * t[row] * n[column];
        }
    }
    Ok(Transform(matrix))
}

/// Resolved COLLADA resources and traversal state for the selected visual scene.
struct Collada<'a, 'i> {
    /// XML elements indexed by their unique IDs.
    ids: HashMap<&'a str, Node<'a, 'i>>,
    /// Decoded triangles indexed by geometry ID.
    meshes: HashMap<String, Mesh>,
    /// Display colors indexed by material ID.
    materials: HashMap<String, [u8; 3]>,
    /// Source up-axis conversion into the renderer coordinate system.
    axis: String,
    /// Source length unit in meters.
    unit: f64,
    /// Active node IDs for cyclic instance detection.
    active: HashSet<NodeId>,
    /// Number of expanded geometry instances.
    instances: usize,
    /// Accumulated transformed geometry.
    scene: Scene,
    /// Shared retained source and output storage budget.
    budget: Budget,
}
impl Collada<'_, '_> {
    /// Transform a source position and apply declared units and up-axis conversion.
    fn world_position(&self, position: Vec3) -> Vec3 {
        let p = position * self.unit;
        match self.axis.as_str() {
            "Y_UP" => Vec3::new(p.x, -p.z, p.y),
            "X_UP" => Vec3::new(-p.z, p.y, p.x),
            _ => p,
        }
    }
    /// Instantiate one decoded geometry with node material bindings and transform.
    fn geometry(
        &mut self,
        id: &str,
        world: Transform,
        bindings: &HashMap<String, String>,
    ) -> Result<()> {
        let mesh = self
            .meshes
            .get(id)
            .ok_or_else(|| error("COLLADA instance_geometry references a missing mesh"))?;
        grow(
            &mut self.scene.triangles,
            mesh.triangles.len(),
            &mut self.budget,
        )?;
        grow(&mut self.scene.lines, mesh.lines.len(), &mut self.budget)?;
        for triangle in &mesh.triangles {
            let vertices = triangle
                .vertices
                .map(|v| self.world_position(world.apply(v)));
            if vertices.iter().any(|v| !v.is_finite()) {
                return Err(error("COLLADA transformed vertex is non-finite"));
            }
            let color = triangle
                .material
                .as_ref()
                .and_then(|symbol| bindings.get(symbol.as_ref()))
                .and_then(|id| self.materials.get(id))
                .copied()
                .or(triangle.color);
            self.scene.triangles.push(Triangle { vertices, color });
            if self.scene.triangles.len() > MAX_ITEMS {
                return Err(error("too much instanced COLLADA geometry"));
            }
        }
        for line in &mesh.lines {
            let vertices = line
                .vertices
                .map(|vertex| self.world_position(world.apply(vertex)));
            if vertices.iter().any(|vertex| !vertex.is_finite()) {
                return Err(error("COLLADA transformed line endpoint is non-finite"));
            }
            let color = line
                .material
                .as_ref()
                .and_then(|symbol| bindings.get(symbol.as_ref()))
                .and_then(|id| self.materials.get(id))
                .copied()
                .or(line.color);
            self.scene.lines.push(Line { vertices, color });
        }
        Ok(())
    }
    /// Walk visual scene nodes and referenced node instances with cycle and depth bounds.
    fn walk(&mut self, node: Node<'_, '_>, parent: Transform, depth: usize) -> Result<()> {
        if depth > 256 || self.instances >= MAX_INSTANCES {
            return Err(error("COLLADA scene nesting or instance limit exceeded"));
        }
        if !self.active.insert(node.id()) {
            return Err(error("cyclic COLLADA instance_node"));
        }
        self.instances += 1;
        let world = parent.then(node_transform(node)?);
        for element in node.children().filter(roxmltree::Node::is_element) {
            match element.tag_name().name() {
                "instance_geometry" => {
                    let id = reference(attribute(element, "url")?)?;
                    let mut bindings = HashMap::new();
                    for binding in element
                        .descendants()
                        .filter(|n| n.is_element() && n.tag_name().name() == "instance_material")
                    {
                        let target = reference(attribute(binding, "target")?)?;
                        if self
                            .ids
                            .get(target)
                            .is_none_or(|n| n.tag_name().name() != "material")
                        {
                            return Err(error(
                                "COLLADA material binding references a missing material",
                            ));
                        }
                        bindings
                            .insert(attribute(binding, "symbol")?.to_owned(), target.to_owned());
                    }
                    self.geometry(id, world, &bindings)?;
                }
                "node" => self.walk(element, world, depth + 1)?,
                "instance_node" => {
                    let id = reference(attribute(element, "url")?)?;
                    let referenced = *self
                        .ids
                        .get(id)
                        .ok_or_else(|| error("COLLADA instance_node references a missing node"))?;
                    if referenced.tag_name().name() != "node" {
                        return Err(error("COLLADA instance_node target is not a node"));
                    }
                    self.walk(referenced, world, depth + 1)?;
                }
                "instance_controller" => {
                    return Err(error(
                        "COLLADA skin and morph controllers are not supported",
                    ));
                }
                _ => {}
            }
        }
        self.active.remove(&node.id());
        Ok(())
    }
}

/// Resolve document resources and render the referenced visual scene.
#[allow(
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Resource resolution and scene setup form one bounded pass; material colors are clamped and rounded to byte range."
)]
fn collada(source: &str) -> Result<Scene> {
    let document = Document::parse(source)?;
    let root = document.root_element();
    if root.tag_name().name() != "COLLADA"
        || !matches!(
            root.attribute("version"),
            Some("1.4.0" | "1.4.1" | "1.5.0" | "1.5.1")
        )
    {
        return Err(error("expected COLLADA 1.4 or 1.5 document"));
    }
    let mut ids = HashMap::new();
    for node in root.descendants().filter(roxmltree::Node::is_element) {
        if let Some(id) = node.attribute("id")
            && ids.insert(id, node).is_some()
        {
            return Err(error(format!("duplicate COLLADA ID {id}")));
        }
        if node.tag_name().name() == "asset"
            && node.parent() != Some(root)
            && (children(node, "unit").next().is_some()
                || children(node, "up_axis").next().is_some())
        {
            return Err(error(
                "COLLADA local coordinate-system asset overrides are not supported",
            ));
        }
    }
    let asset = children(root, "asset").next();
    let unit = asset
        .and_then(|asset| children(asset, "unit").next())
        .map(|node| -> Result<f64> {
            let unit: f64 = attribute(node, "meter")?.parse()?;
            if !unit.is_finite() || unit <= 0.0 {
                return Err(error("COLLADA unit scale must be finite and positive"));
            }
            Ok(unit)
        })
        .transpose()?
        .unwrap_or(1.0);
    let axis = asset
        .and_then(|asset| children(asset, "up_axis").next())
        .map(list_text)
        .transpose()?
        .unwrap_or(std::borrow::Cow::Borrowed("Y_UP"))
        .trim()
        .to_owned();
    if !matches!(axis.as_str(), "X_UP" | "Y_UP" | "Z_UP") {
        return Err(error("invalid COLLADA up_axis"));
    }
    let mut budget = Budget::default();
    let mut meshes = HashMap::new();
    for library in children(root, "library_geometries") {
        for geometry in children(library, "geometry") {
            let mesh = child(geometry, "mesh")
                .map_err(|_| error("COLLADA spline/BREP geometry is not supported"))?;
            meshes.insert(
                attribute(geometry, "id")?.to_owned(),
                parse_mesh(mesh, &ids, &mut budget)?,
            );
        }
    }
    let mut materials = HashMap::new();
    for node in root
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "material")
    {
        let effect_id = reference(attribute(child(node, "instance_effect")?, "url")?)?;
        let effect = *ids
            .get(effect_id)
            .ok_or_else(|| error("COLLADA material references a missing effect"))?;
        if effect.tag_name().name() != "effect" {
            return Err(error("COLLADA material target is not an effect"));
        }
        if let Some(profile) = children(effect, "profile_COMMON").next() {
            let color = profile
                .descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "diffuse")
                .find_map(|diffuse| children(diffuse, "color").next())
                .or_else(|| {
                    profile
                        .descendants()
                        .filter(|n| n.is_element() && n.tag_name().name() == "emission")
                        .find_map(|emission| children(emission, "color").next())
                });
            if let Some(color) = color {
                let values = numbers(color)?;
                if values.len() != 3 && values.len() != 4 {
                    return Err(error("COLLADA diffuse color needs RGB or RGBA"));
                }
                materials.insert(
                    attribute(node, "id")?.to_owned(),
                    std::array::from_fn(|i| (values[i] * 255.0).clamp(0.0, 255.0).round() as u8),
                );
            }
        }
    }
    let mut importer = Collada {
        ids,
        meshes,
        materials,
        axis,
        unit,
        active: HashSet::new(),
        instances: 0,
        scene: Scene::default(),
        budget,
    };
    let selected = children(root, "scene")
        .next()
        .and_then(|scene| children(scene, "instance_visual_scene").next());
    let visual_scene = if let Some(selected) = selected {
        let id = reference(attribute(selected, "url")?)?;
        let node = *importer
            .ids
            .get(id)
            .ok_or_else(|| error("COLLADA scene references a missing visual_scene"))?;
        if node.tag_name().name() != "visual_scene" {
            return Err(error("COLLADA scene target is not a visual_scene"));
        }
        Some(node)
    } else {
        let scenes: Vec<_> = root
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "visual_scene")
            .collect();
        if scenes.len() > 1 {
            return Err(error(
                "COLLADA has multiple visual scenes but no selected scene",
            ));
        }
        scenes.first().copied()
    };
    if let Some(scene) = visual_scene {
        for node in children(scene, "node") {
            importer.walk(node, Transform::IDENTITY, 0)?;
        }
    } else {
        let geometry_ids: Vec<_> = importer.meshes.keys().cloned().collect();
        for id in geometry_ids {
            importer.geometry(&id, Transform::IDENTITY, &HashMap::new())?;
        }
    }
    importer.scene.validate()?;
    Ok(importer.scene)
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "Exact authored integer coordinates, identity transforms and power-of-two values are protocol witnesses in these tests."
)]
mod tests {
    use super::*;
    /// Construct a unit-scaled static mesh document with simple ordered scalar accessors.
    fn geometry_document(positions: &str, count: usize, primitive: &str, nodes: &str) -> String {
        format!(
            r##"<COLLADA version="1.4.1"><asset><up_axis>Z_UP</up_axis></asset><library_geometries><geometry id="mesh"><mesh><source id="pos"><float_array id="array" count="{}">{positions}</float_array><technique_common><accessor source="#array" count="{count}" stride="3"><param name="A" type="float"/><param name="B" type="float"/><param name="C" type="float"/></accessor></technique_common></source><vertices id="vertices"><input semantic="POSITION" source="#pos"/></vertices>{primitive}</mesh></geometry></library_geometries><library_visual_scenes><visual_scene id="scene">{nodes}</visual_scene></library_visual_scenes><scene><instance_visual_scene url="#scene"/></scene></COLLADA>"##,
            count * 3
        )
    }
    fn document(primitive: &str, nodes: &str) -> String {
        format!(
            r##"<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
          <asset><unit meter="0.001"/><up_axis>Z_UP</up_axis></asset>
          <library_geometries><geometry id="mesh"><mesh>
            <source id="pos"><float_array id="array" count="12">9 0 0 0 9 1000 0 0 9 0 1000 0</float_array>
              <technique_common><accessor source="#array" count="3" stride="4"><param/><param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/></accessor></technique_common>
            </source><vertices id="vertices"><input semantic="POSITION" source="#pos"/></vertices>
            {primitive}
          </mesh></geometry></library_geometries>
          <library_visual_scenes><visual_scene id="scene">{nodes}</visual_scene></library_visual_scenes>
          <scene><instance_visual_scene url="#scene"/></scene></COLLADA>"##
        )
    }
    const TRIANGLE: &str = r##"<triangles count="1"><input semantic="VERTEX" source="#vertices" offset="0"/><p>0 1 2</p></triangles>"##;

    #[test]
    fn collada_accessors_units_and_nested_instances() {
        let nodes = r##"<node><translate>1000 0 0</translate><node><scale>2 2 2</scale><instance_geometry url="#mesh"/></node></node>
            <node><matrix>1 0 0 0 0 1 0 3000 0 0 1 0 0 0 0 1</matrix><instance_geometry url="#mesh"/></node>"##;
        let scene =
            collada(&document(TRIANGLE, nodes).replace("<param/>", "<param type=\"float\"/>"))
                .unwrap();
        assert_eq!(scene.triangles.len(), 2);
        assert_eq!(scene.triangles[0].vertices[1], Vec3::new(3., 0., 0.));
        assert_eq!(scene.triangles[1].vertices[0], Vec3::new(0., 3., 0.));
    }

    #[test]
    fn collada_y_up_and_rotation_order() {
        let source = document(TRIANGLE, r##"<node><translate>1000 0 0</translate><rotate>0 0 1 90</rotate><instance_geometry url="#mesh"/></node>"##)
            .replace("Z_UP", "Y_UP").replace("<param/>", "<param type=\"float\"/>");
        let scene = collada(&source).unwrap();
        let p = scene.triangles[0].vertices[1];
        assert!((p.x - 1.).abs() < 1e-12 && (p.z - 1.).abs() < 1e-12 && p.y == 0.);
    }

    #[test]
    fn collada_polylist_and_material_bindings() {
        let primitive = r##"<polylist count="1" material="surface"><input semantic="VERTEX" source="#vertices"/><vcount>3</vcount><p>0 1 2</p></polylist>"##;
        let source = document(primitive, r##"<node><instance_geometry url="#mesh"><bind_material><technique_common><instance_material symbol="surface" target="#red"/></technique_common></bind_material></instance_geometry></node>"##)
            .replace("<param/>", "<param type=\"float\"/>")
            .replace("<library_geometries>", r##"<library_effects><effect id="effect"><profile_COMMON><technique sid="common"><lambert><diffuse><color>1 0 0 1</color></diffuse></lambert></technique></profile_COMMON></effect></library_effects><library_materials><material id="red"><instance_effect url="#effect"/></material></library_materials><library_geometries>"##);
        assert_eq!(
            collada(&source).unwrap().triangles[0].color,
            Some([255, 0, 0])
        );
    }

    #[test]
    fn collada_rejects_bad_indices_cycles_and_controllers() {
        let source = document(TRIANGLE, r##"<node id="loop"><instance_geometry url="#mesh"/><instance_node url="#loop"/></node>"##).replace("<param/>", "<param type=\"float\"/>");
        assert!(collada(&source).unwrap_err().to_string().contains("cyclic"));
        let source = document(
            TRIANGLE,
            r##"<node><instance_geometry url="#mesh"/></node>"##,
        )
        .replace("<param/>", "<param type=\"float\"/>")
        .replace("0 1 2</p>", "0 1 3</p>");
        assert!(collada(&source).is_err());
        let source = document(
            TRIANGLE,
            r##"<node><instance_controller url="#skin"/></node>"##,
        )
        .replace("<param/>", "<param type=\"float\"/>");
        assert!(
            collada(&source)
                .unwrap_err()
                .to_string()
                .contains("controllers")
        );
    }

    #[test]
    fn collada_rejects_unsupported_geometry_and_holes() {
        let source = document(TRIANGLE, "")
            .replace("<param/>", "<param type=\"float\"/>")
            .replace("<mesh>", "<spline>")
            .replace("</mesh>", "</spline>");
        assert!(collada(&source).is_err());
        let primitive = r##"<polygons count="1"><input semantic="VERTEX" source="#vertices"/><ph><p>0 1 2</p><h>0 1 2</h></ph></polygons>"##;
        assert!(
            collada(&document(primitive, "").replace("<param/>", "<param type=\"float\"/>"))
                .is_err()
        );
    }

    /// Parameter names are labels; named scalar slots retain authored component order.
    #[test]
    fn accessor_components_follow_named_slots_instead_of_name_bindings() {
        let nodes = r##"<node><instance_geometry url="#mesh"/></node>"##;
        for names in [["A", "F", "X"], ["Z", "X", "Y"]] {
            let params = format!(
                "<param name=\"{}\" type=\"float\"/><param name=\"{}\" type=\"float\"/><param name=\"{}\" type=\"float\"/>",
                names[0], names[1], names[2]
            );
            let source = document(TRIANGLE, nodes)
                .replace("<param/>", "<param type=\"float\"/>")
                .replace("<param name=\"X\" type=\"float\"/><param name=\"Y\" type=\"float\"/><param name=\"Z\" type=\"float\"/>", &params);
            let scene = collada(&source).unwrap();
            assert_eq!(
                scene.triangles[0].vertices,
                [
                    Vec3::new(0.0, 0.0, 0.0),
                    Vec3::new(1.0, 0.0, 0.0),
                    Vec3::new(0.0, 1.0, 0.0)
                ]
            );
        }
    }

    /// The final unused stride padding is optional, but consumed named slots must exist.
    #[test]
    fn accessor_ranges_use_consumed_slots_and_handle_zero_count() {
        let source = document(TRIANGLE, r##"<node><instance_geometry url="#mesh"/></node>"##)
            .replace("count=\"12\">9 0 0 0 9 1000 0 0 9 0 1000 0", "count=\"11\">0 0 0 9 1000 0 0 9 0 1000 0")
            .replace("<param/><param name=\"X\" type=\"float\"/><param name=\"Y\" type=\"float\"/><param name=\"Z\" type=\"float\"/>", "<param name=\"X\" type=\"float\"/><param name=\"Y\" type=\"float\"/><param name=\"Z\" type=\"float\"/><param type=\"float\"/>");
        assert_eq!(collada(&source).unwrap().triangles.len(), 1);
        assert!(
            collada(&source.replace(
                "count=\"11\">0 0 0 9 1000 0 0 9 0 1000 0",
                "count=\"10\">0 0 0 9 1000 0 0 9 0 1000"
            ))
            .is_err()
        );
        let zero = r##"<source id="empty"><float_array id="a" count="0"/><technique_common><accessor source="#a" count="0" stride="3"><param name="A" type="float"/><param name="B" type="float"/><param name="C" type="float"/></accessor></technique_common></source>"##;
        let document = Document::parse(zero).unwrap();
        let ids = document
            .descendants()
            .filter_map(|node| node.attribute("id").map(|id| (id, node)))
            .collect();
        let source = parse_source(document.root_element(), &ids, &mut Budget::default()).unwrap();
        assert_eq!(source.count, 0);
        assert!(source.vector(0).is_err());
    }

    /// Line pairs and strips resolve independent color indices, instances, units and translations.
    #[test]
    fn wire_primitives_preserve_segments_independent_indices_and_placements() {
        let color_source = r##"<source id="colors"><float_array id="colorarray" count="9">1 0 0 0 1 0 0 0 1</float_array><technique_common><accessor source="#colorarray" count="3" stride="3"><param name="A" type="float"/><param name="B" type="float"/><param name="C" type="float"/></accessor></technique_common></source>"##;
        for primitive in [
            r##"<lines count="2"><input semantic="VERTEX" source="#vertices" offset="0"/><input semantic="COLOR" source="#colors" offset="1"/><p>0 2 1 1 1 1 2 0</p></lines>"##,
            r##"<linestrips count="1"><input semantic="VERTEX" source="#vertices" offset="0"/><input semantic="COLOR" source="#colors" offset="1"/><p>0 2 1 1 2 0</p></linestrips>"##,
        ] {
            let source = document(
                primitive,
                r##"<node><translate>1000 0 0</translate><instance_geometry url="#mesh"/></node>"##,
            )
            .replace("<param/>", "<param type=\"float\"/>")
            .replace(
                "<vertices id=\"vertices\">",
                &format!("{color_source}<vertices id=\"vertices\">"),
            );
            let scene = collada(&source).unwrap();
            assert_eq!(scene.lines.len(), 2);
            assert_eq!(
                scene.lines[0].vertices,
                [Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0)]
            );
            assert_eq!(
                scene.lines[1].vertices,
                [Vec3::new(2.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0)]
            );
            assert_eq!(scene.lines[0].color, Some([0, 127, 127]));
            assert_eq!(scene.lines[1].color, Some([127, 127, 0]));
            scene.validate().unwrap();
        }
    }

    /// Optional empty zero-count line lists are valid in a scene containing other geometry.
    #[test]
    fn zero_count_wires_allow_empty_lists_but_reject_authored_indices() {
        let wire =
            r##"<lines count="0"><input semantic="VERTEX" source="#vertices"/><p/></lines>"##;
        let source = document(
            &format!("{TRIANGLE}{wire}"),
            r##"<node><instance_geometry url="#mesh"/></node>"##,
        )
        .replace("<param/>", "<param type=\"float\"/>");
        let scene = collada(&source).unwrap();
        assert_eq!([scene.triangles.len(), scene.lines.len()], [1, 0]);
        assert!(collada(&source.replace("<p/>", "<p>0 1</p>")).is_err());
        assert!(collada(&source.replace("count=\"0\"", "count=\"1\"")).is_err());
    }

    /// Exhausted storage rejects both wire encodings before endpoint expansion.
    #[test]
    fn wire_storage_is_checked_before_expanding_source_indices() {
        let inputs = [Input {
            source: "positions".into(),
            semantic: "POSITION".into(),
            offset: 0,
            set: 0,
        }];
        let sources = HashMap::from([(
            "positions".into(),
            Source {
                values: vec![0.0; 3],
                count: 1,
                stride: 3,
                offset: 0,
                components: vec![0, 1, 2],
            },
        )]);
        for name in ["lines", "linestrips"] {
            let xml = format!("<{name} count=\"1\"><p>1 1</p></{name}>");
            let document = Document::parse(&xml).unwrap();
            let mut mesh = Mesh::default();
            let mut budget = Budget { bytes: MAX_STORAGE };
            let failure = wire_primitive(
                document.root_element(),
                1,
                &inputs,
                &sources,
                &mut mesh,
                &mut budget,
                None,
            )
            .unwrap_err();
            assert!(failure.to_string().contains("storage budget"));
            assert_eq!(mesh.lines.capacity(), 0);
        }
    }

    /// Hole triangles preserve the exterior area and front winding without covering the opening.
    #[test]
    fn planar_polygon_holes_preserve_open_area_and_winding() {
        let primitive = r##"<polygons count="1"><input semantic="VERTEX" source="#vertices"/><ph><p>0 1 2 3</p><h>4 5 6 7</h></ph></polygons>"##;
        let source = geometry_document(
            "0 0 0 4 0 0 4 4 0 0 4 0 1 1 0 1 3 0 3 3 0 3 1 0",
            8,
            primitive,
            r##"<node><instance_geometry url="#mesh"/></node>"##,
        );
        let scene = collada(&source).unwrap();
        assert_eq!(scene.triangles.len(), 8);
        let area: f64 = scene
            .triangles
            .iter()
            .map(|triangle| {
                let vertices = triangle.vertices;
                let cross = (vertices[1] - vertices[0]).cross(vertices[2] - vertices[0]);
                assert!(cross.z > 0.0);
                cross.length() * 0.5
            })
            .sum();
        assert!((area - 12.0).abs() < 1e-12);
        assert!(collada(&source.replace("<h>4 5 6 7</h>", "")).is_err());
        assert!(collada(&source.replace("<h>4 5 6 7</h>", "<h>4 5</h>")).is_err());
        assert!(collada(&source.replace("<h>4 5 6 7</h>", "<h>4 5 6 8</h>")).is_err());
        let error = collada(&source.replace("<h>4 5 6 7</h>", "<h>4 6 5 7</h>")).unwrap_err();
        assert!(error.to_string().contains("zero-area hole"));
    }

    /// Signed-area validation retains a valid concave opening instead of requiring convex rings.
    #[test]
    fn concave_hole_rings_keep_their_authored_opening() {
        let primitive = r##"<polygons count="1"><input semantic="VERTEX" source="#vertices"/><ph><p>0 1 2 3</p><h>4 5 6 7 8</h></ph></polygons>"##;
        let source = geometry_document(
            "0 0 0 4 0 0 4 4 0 0 4 0 1 1 0 1 3 0 2 2 0 3 3 0 3 1 0",
            9,
            primitive,
            r##"<node><instance_geometry url="#mesh"/></node>"##,
        );
        let scene = collada(&source).unwrap();
        let area: f64 = scene
            .triangles
            .iter()
            .map(|triangle| {
                (triangle.vertices[1] - triangle.vertices[0])
                    .cross(triangle.vertices[2] - triangle.vertices[0])
                    .length()
                    * 0.5
            })
            .sum();
        assert!((area - 13.0).abs() < 1e-12);
        scene.validate().unwrap();
    }

    /// A plain sibling polygon retains its prior triangulation rules when another polygon has a hole.
    #[test]
    fn mixed_plain_and_holed_polygons_preserve_existing_plain_faces() {
        let positions = "0 0 0 4 0 0 4 4 0 0 4 0 1 1 0 1 3 0 3 3 0 3 1 0 6 0 0 9 0 0 9 3 1 6 3 0";
        let primitive = r##"<polygons count="2"><input semantic="VERTEX" source="#vertices"/><p>8 9 10 11</p><ph><p>0 1 2 3</p><h>4 5 6 7</h></ph></polygons>"##;
        let scene = collada(&geometry_document(
            positions,
            12,
            primitive,
            r##"<node><instance_geometry url="#mesh"/></node>"##,
        ))
        .unwrap();
        assert_eq!(scene.triangles.len(), 10);
        assert!(
            scene
                .triangles
                .iter()
                .any(|triangle| triangle.vertices.contains(&Vec3::new(9.0, 3.0, 1.0)))
        );
    }

    /// Lookat is node placement, retains original translation and recomputes an orthogonal up vector.
    #[test]
    fn lookat_places_local_axes_and_composes_in_document_order() {
        let matrix = lookat(&[2.0, 3.0, 4.0, 3.0, 3.0, 4.0, 0.0, 1.0, 0.5]).unwrap();
        assert_eq!(matrix.apply(Vec3::default()), Vec3::new(2.0, 3.0, 4.0));
        assert_eq!(
            matrix.apply(Vec3::new(0.0, 0.0, -1.0)),
            Vec3::new(3.0, 3.0, 4.0)
        );
        let document = Document::parse("<node><translate>10 0 0</translate><lookat>2 3 4 <!-- eye --> 2 3 3 <!-- target --> 0 1 0</lookat><scale>2 3 4</scale></node>").unwrap();
        let matrix = node_transform(document.root_element()).unwrap();
        assert_eq!(
            matrix.apply(Vec3::new(1.0, 1.0, 1.0)),
            Vec3::new(14.0, 6.0, 8.0)
        );
        for values in [[0.0; 9], [0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 1.0]] {
            assert!(lookat(&values).is_err());
        }
    }

    /// General angular skew tilts nonorthogonal axes correctly and handles small or large direction magnitudes.
    #[test]
    fn skew_uses_angular_nonorthogonal_axis_semantics() {
        let angle = 18.434_948_822_922_01;
        for scale in [1.0_f64, 1e-250, 1e308] {
            let matrix = skew(&[angle, scale, scale, 0.0, scale, 0.0, 0.0]).unwrap();
            let transformed = matrix.apply(Vec3::new(2.0, 3.0, 4.0));
            assert!((transformed.x - 5.0).abs() < 1e-12);
            assert_eq!([transformed.y, transformed.z], [3.0, 4.0]);
        }
        let angle = (5e-251_f64).to_degrees();
        let matrix = skew(&[angle, 1e308, 1e58, 0.0, 1e-250, 0.0, 0.0]).unwrap();
        assert!((matrix.0[1][0] / 1e250 - 1.0).abs() < 1e-12);
        assert!(skew(&[90.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0]).is_err());
        assert!(skew(&[-90.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0]).is_err());
        assert!(skew(&[1.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).is_err());
        assert!(skew(&[0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]).is_err());
    }

    /// Finite extreme eye differences and direction magnitudes retain their geometric meaning.
    #[test]
    fn lookat_normalization_resists_direction_and_subtraction_overflow() {
        for scale in [1e-250_f64, 1e308] {
            let matrix = lookat(&[2.0, 3.0, 4.0, 3.0, 3.0, 4.0, 0.0, scale, scale]).unwrap();
            assert_eq!(
                matrix.apply(Vec3::new(0.0, 0.0, -1.0)),
                Vec3::new(3.0, 3.0, 4.0)
            );
        }
        let matrix = lookat(&[-1e308, 0.0, 0.0, 1e308, 0.0, 0.0, 0.0, 1e308, 1e308]).unwrap();
        assert_eq!(matrix.0[3][0], -1e308);
        assert_eq!(matrix.0[2][0], -1.0);
    }

    /// Oversized offsets, inconsistent line tuples and malformed counts fail before geometry growth.
    #[test]
    fn wire_limits_and_malformed_index_lists_are_rejected() {
        for primitive in [
            r##"<lines count="1"><input semantic="VERTEX" source="#vertices" offset="64"/><p>0 1</p></lines>"##,
            r##"<lines count="1"><input semantic="VERTEX" source="#vertices"/><p>0 1 2</p></lines>"##,
            r##"<linestrips count="1"><input semantic="VERTEX" source="#vertices"/><p>0</p></linestrips>"##,
            r##"<lines count="1"><input semantic="VERTEX" source="#vertices"/><p>0 3</p></lines>"##,
            r##"<lines count="20000000"><input semantic="VERTEX" source="#vertices" offset="63"/><p/></lines>"##,
        ] {
            let source = document(
                primitive,
                r##"<node><instance_geometry url="#mesh"/></node>"##,
            )
            .replace("<param/>", "<param type=\"float\"/>");
            assert!(collada(&source).is_err(), "{primitive}");
        }
        let mut budget = Budget { bytes: MAX_STORAGE };
        let mut vertices = Vec::<Vec3>::new();
        assert!(grow(&mut vertices, 1, &mut budget).is_err());
        assert_eq!(vertices, []);
        assert!(
            limited_indices(Document::parse("<p>0 1 2</p>").unwrap().root_element(), 2).is_err()
        );
    }

    /// XML comments preserve text-content adjacency even when they split a numeric token.
    #[test]
    fn comments_inside_numeric_tokens_do_not_insert_list_whitespace() {
        let primitive = r##"<triangles count="1"><input semantic="VERTEX" source="#vertices"/><p>0 0<!-- index -->1 2</p></triangles>"##;
        let source = geometry_document(
            "0 0 0 1<!-- coordinate -->2 0 0 0 1 0",
            3,
            primitive,
            r##"<node><translate>1<!-- translation -->2 0 0</translate><instance_geometry url="#mesh"/></node>"##,
        );
        let scene = collada(&source).unwrap();
        assert_eq!(
            scene.triangles[0].vertices,
            [
                Vec3::new(12.0, 0.0, 0.0),
                Vec3::new(24.0, 0.0, 0.0),
                Vec3::new(12.0, 1.0, 0.0)
            ]
        );
        let axis_source = source.replace("Z_UP", "Y_<!-- axis -->UP");
        let axis_scene = collada(&axis_source).unwrap();
        assert_eq!(
            axis_scene.triangles[0].vertices[2],
            Vec3::new(12.0, 0.0, 1.0)
        );
        let document = Document::parse("<p>0 0<!-- index -->1</p>").unwrap();
        assert_eq!(limited_indices(document.root_element(), 2).unwrap(), [0, 1]);
        let document = Document::parse("<p>0 1</p>").unwrap();
        assert!(matches!(
            list_text(document.root_element()).unwrap(),
            std::borrow::Cow::Borrowed(_)
        ));
    }
}
