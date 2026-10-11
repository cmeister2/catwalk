//! Static COLLADA scenes, including indexed geometry and instanced transforms.
use std::collections::{HashMap, HashSet};
use std::path::Path;

use roxmltree::{Document, Node, NodeId};

use super::mesh::triangulate_polygon;
use super::transform::Transform;
use crate::scene::{Scene, Triangle, Vec3};
use crate::{Error, Result};

const MAX_ITEMS: usize = 20_000_000;
const MAX_INSTANCES: usize = 100_000;

pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    match super::extension(path)?.as_str() {
        "dae" => collada(std::str::from_utf8(&bytes)?),
        extension => Err(format!("unsupported legacy scene format .{extension}").into()),
    }
}

fn error(message: impl Into<String>) -> Error {
    message.into().into()
}
fn children<'a, 'i>(node: Node<'a, 'i>, name: &str) -> impl Iterator<Item = Node<'a, 'i>> {
    let name = name.to_owned();
    node.children()
        .filter(move |n| n.is_element() && n.tag_name().name() == name)
}
fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Result<Node<'a, 'i>> {
    children(node, name).next().ok_or_else(|| {
        error(format!(
            "COLLADA <{}> lacks <{name}>",
            node.tag_name().name()
        ))
    })
}
fn attribute<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Result<&'a str> {
    node.attribute(name)
        .ok_or_else(|| error(format!("COLLADA <{}> lacks {name}", node.tag_name().name())))
}
fn item_count(value: &str) -> Result<usize> {
    let count: usize = value.parse()?;
    if count > MAX_ITEMS {
        return Err(error("COLLADA item count exceeds 20 million"));
    }
    Ok(count)
}
fn numbers(node: Node<'_, '_>) -> Result<Vec<f64>> {
    let mut values = Vec::new();
    for word in node.text().unwrap_or("").split_whitespace() {
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
fn indices(node: Node<'_, '_>) -> Result<Vec<usize>> {
    let mut values = Vec::new();
    for word in node.text().unwrap_or("").split_whitespace() {
        values.push(item_count(word)?);
        if values.len() > MAX_ITEMS {
            return Err(error("too many COLLADA indices"));
        }
    }
    Ok(values)
}
fn reference(value: &str) -> Result<&str> {
    value
        .strip_prefix('#')
        .filter(|s| !s.is_empty())
        .ok_or_else(|| error("COLLADA references must address an ID in the same file"))
}

#[derive(Debug)]
struct Source {
    values: Vec<f64>,
    count: usize,
    stride: usize,
    offset: usize,
    components: Vec<Option<String>>,
}
impl Source {
    fn vector(&self, index: usize, names: [&str; 3]) -> Result<Vec3> {
        if index >= self.count {
            return Err(error("COLLADA source index out of bounds"));
        }
        let mut components = [0.0; 3];
        for (i, name) in names.into_iter().enumerate() {
            let position = self
                .components
                .iter()
                .position(|component| component.as_deref() == Some(name))
                .ok_or_else(|| error(format!("COLLADA source lacks {name} component")))?;
            if position >= self.stride {
                return Err(error("COLLADA source component exceeds accessor stride"));
            }
            let offset = index
                .checked_mul(self.stride)
                .and_then(|n| n.checked_add(self.offset))
                .and_then(|n| n.checked_add(position))
                .ok_or_else(|| error("COLLADA source offset overflow"))?;
            components[i] = *self
                .values
                .get(offset)
                .ok_or_else(|| error("COLLADA accessor exceeds its array"))?;
        }
        Ok(Vec3::new(components[0], components[1], components[2]))
    }
    fn color(&self, index: usize) -> Result<[u8; 3]> {
        let c = self.vector(index, ["R", "G", "B"])?;
        Ok([c.x, c.y, c.z].map(|c| (c * 255.0).clamp(0.0, 255.0).round() as u8))
    }
}
fn parse_source(node: Node<'_, '_>, ids: &HashMap<&str, Node<'_, '_>>) -> Result<Source> {
    let technique = child(node, "technique_common")?;
    let accessor = child(technique, "accessor")?;
    let array_id = reference(attribute(accessor, "source")?)?;
    let array = *ids
        .get(array_id)
        .ok_or_else(|| error("COLLADA accessor references a missing array"))?;
    if array.tag_name().name() != "float_array" {
        return Err(error("COLLADA mesh accessor needs a float_array"));
    }
    let values = numbers(array)?;
    if item_count(attribute(array, "count")?)? != values.len() {
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
    let end = count
        .checked_mul(stride)
        .and_then(|n| n.checked_add(offset))
        .ok_or_else(|| error("COLLADA accessor size overflow"))?;
    if end > values.len() {
        return Err(error("COLLADA accessor exceeds its array"));
    }
    let mut components = Vec::new();
    for param in children(accessor, "param") {
        if !matches!(param.attribute("type"), Some("float" | "double")) {
            return Err(error(
                "COLLADA geometry accessor parameters must be scalar floats",
            ));
        }
        components.push(param.attribute("name").map(str::to_ascii_uppercase));
    }
    if components.len() > stride {
        return Err(error(
            "COLLADA accessor has more parameters than its stride",
        ));
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
struct MeshTriangle {
    vertices: [Vec3; 3],
    color: Option<[u8; 3]>,
    material: Option<String>,
}
struct Input {
    source: String,
    semantic: String,
    offset: usize,
    set: usize,
}
fn inputs(node: Node<'_, '_>) -> Result<Vec<Input>> {
    children(node, "input")
        .map(|input| {
            Ok(Input {
                source: reference(attribute(input, "source")?)?.to_owned(),
                semantic: attribute(input, "semantic")?.to_owned(),
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
            })
        })
        .collect()
}
fn parse_mesh(node: Node<'_, '_>, ids: &HashMap<&str, Node<'_, '_>>) -> Result<Vec<MeshTriangle>> {
    let mut sources = HashMap::new();
    for source in children(node, "source") {
        sources.insert(
            attribute(source, "id")?.to_owned(),
            parse_source(source, ids)?,
        );
    }
    let mut vertex_inputs = HashMap::new();
    for vertices in children(node, "vertices") {
        vertex_inputs.insert(attribute(vertices, "id")?.to_owned(), inputs(vertices)?);
    }
    let mut result = Vec::new();
    for primitive in node.children().filter(|n| n.is_element()) {
        let name = primitive.tag_name().name();
        if matches!(name, "source" | "vertices" | "extra") {
            continue;
        }
        if !matches!(
            name,
            "triangles" | "polylist" | "polygons" | "trifans" | "tristrips"
        ) {
            return Err(error(format!(
                "unsupported COLLADA mesh primitive <{name}>"
            )));
        }
        let count = item_count(attribute(primitive, "count")?)?;
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
                    expanded.push(Input {
                        source: vertex_input.source.clone(),
                        semantic: vertex_input.semantic.clone(),
                        offset: input.offset,
                        set: vertex_input.set,
                    });
                }
            } else {
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
                vertices.push(position_source.vector(tuple[position.offset], ["X", "Y", "Z"])?);
                if let (Some(input), Some(source)) = (color_input, color_source) {
                    colors.push(source.color(tuple[input.offset])?);
                }
            }
            let color = if colors.is_empty() {
                None
            } else {
                Some(std::array::from_fn(|i| {
                    (colors.iter().map(|color| color[i] as u64).sum::<u64>() / colors.len() as u64)
                        as u8
                }))
            };
            for vertices in triangulate_polygon(&vertices)? {
                result.push(MeshTriangle {
                    vertices,
                    color,
                    material: primitive.attribute("material").map(str::to_owned),
                });
                if result.len() > MAX_ITEMS {
                    return Err(error("too many COLLADA triangles"));
                }
            }
        }
    }
    Ok(result)
}

fn node_transform(node: Node<'_, '_>) -> Result<Transform> {
    let mut result = Transform::IDENTITY;
    for element in node.children().filter(|n| n.is_element()) {
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
            "lookat" | "skew" => {
                return Err(error(format!(
                    "COLLADA <{name}> transforms are not supported"
                )));
            }
            _ => unreachable!(),
        }
        result = result.then(Transform(matrix));
        if result.0.iter().flatten().any(|value| !value.is_finite()) {
            return Err(error("COLLADA transform overflow"));
        }
    }
    Ok(result)
}

struct Collada<'a, 'i> {
    ids: HashMap<&'a str, Node<'a, 'i>>,
    meshes: HashMap<String, Vec<MeshTriangle>>,
    materials: HashMap<String, [u8; 3]>,
    axis: String,
    unit: f64,
    active: HashSet<NodeId>,
    instances: usize,
    scene: Scene,
}
impl Collada<'_, '_> {
    fn world_position(&self, position: Vec3) -> Vec3 {
        let p = position * self.unit;
        match self.axis.as_str() {
            "Y_UP" => Vec3::new(p.x, -p.z, p.y),
            "X_UP" => Vec3::new(-p.z, p.y, p.x),
            _ => p,
        }
    }
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
        for triangle in mesh {
            let vertices = triangle
                .vertices
                .map(|v| self.world_position(world.apply(v)));
            if vertices.iter().any(|v| !v.is_finite()) {
                return Err(error("COLLADA transformed vertex is non-finite"));
            }
            let color = triangle
                .material
                .as_ref()
                .and_then(|symbol| bindings.get(symbol))
                .and_then(|id| self.materials.get(id))
                .copied()
                .or(triangle.color);
            self.scene.triangles.push(Triangle { vertices, color });
            if self.scene.triangles.len() > MAX_ITEMS {
                return Err(error("too much instanced COLLADA geometry"));
            }
        }
        Ok(())
    }
    fn walk(&mut self, node: Node<'_, '_>, parent: Transform, depth: usize) -> Result<()> {
        if depth > 256 || self.instances >= MAX_INSTANCES {
            return Err(error("COLLADA scene nesting or instance limit exceeded"));
        }
        if !self.active.insert(node.id()) {
            return Err(error("cyclic COLLADA instance_node"));
        }
        self.instances += 1;
        let world = parent.then(node_transform(node)?);
        for element in node.children().filter(|n| n.is_element()) {
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
    for node in root.descendants().filter(|n| n.is_element()) {
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
        .and_then(|axis| axis.text())
        .unwrap_or("Y_UP")
        .trim()
        .to_owned();
    if !matches!(axis.as_str(), "X_UP" | "Y_UP" | "Z_UP") {
        return Err(error("invalid COLLADA up_axis"));
    }
    let mut meshes = HashMap::new();
    for library in children(root, "library_geometries") {
        for geometry in children(library, "geometry") {
            let mesh = child(geometry, "mesh")
                .map_err(|_| error("COLLADA spline/BREP geometry is not supported"))?;
            meshes.insert(
                attribute(geometry, "id")?.to_owned(),
                parse_mesh(mesh, &ids)?,
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
mod tests {
    use super::*;
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
}
