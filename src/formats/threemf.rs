use super::{MAX_INPUT_BYTES, transform::Transform};
use crate::{
    Result,
    scene::{Scene, Triangle, Vec3},
};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::Path,
};

const CORE: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
const MAX_TRIANGLES: usize = 5_000_000;

struct Object {
    triangles: Vec<Triangle>,
    components: Vec<(u32, Transform)>,
}

pub(super) fn load(path: &Path) -> Result<Scene> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let root = if let Ok(mut relationships) = archive.by_name("_rels/.rels") {
        let xml = read_entry(&mut relationships)?;
        let doc = roxmltree::Document::parse(&xml)?;
        let relation = doc
            .descendants()
            .find(|n| {
                n.has_tag_name("Relationship")
                    && n.attribute("Type").is_some_and(|t| t.ends_with("/3dmodel"))
            })
            .ok_or("3MF package has no model relationship")?;
        if relation.attribute("TargetMode") == Some("External") {
            return Err("external 3MF model relationships are unsupported".into());
        }
        package_path(
            relation
                .attribute("Target")
                .ok_or("3MF model relationship has no target")?,
        )?
    } else {
        "3D/3dmodel.model".to_owned()
    };
    let mut entry = archive.by_name(&root)?;
    let xml = read_entry(&mut entry)?;
    parse(&xml)
}

fn read_entry<R: Read>(reader: &mut R) -> Result<String> {
    let mut bytes = Vec::new();
    reader.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err("3MF XML exceeds the 512 MiB limit".into());
    }
    Ok(String::from_utf8(bytes)?)
}

fn package_path(target: &str) -> Result<String> {
    let bytes = target.as_bytes();
    let mut decoded = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err("invalid escape in 3MF package target".into());
            }
            decoded.push(u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3])?,
                16,
            )?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let value = String::from_utf8(decoded)?;
    if value.contains([':', '\0', '\\', '?', '#']) || value.split('/').any(|p| p == "..") {
        return Err("invalid 3MF package target".into());
    }
    Ok(value.trim_start_matches('/').to_owned())
}

fn attribute<'a, 'input>(node: roxmltree::Node<'a, 'input>, name: &str) -> Result<&'a str> {
    node.attribute(name)
        .ok_or_else(|| format!("3MF {} is missing {name}", node.tag_name().name()).into())
}

fn id(node: roxmltree::Node<'_, '_>, name: &str) -> Result<u32> {
    Ok(attribute(node, name)?.parse()?)
}

fn color(value: &str) -> Result<[u8; 3]> {
    let value = value
        .strip_prefix('#')
        .ok_or("3MF color must start with #")?;
    if (value.len() != 6 && value.len() != 8) || !value.is_ascii() {
        return Err("3MF color must be #RRGGBB or #RRGGBBAA".into());
    }
    Ok([
        u8::from_str_radix(&value[0..2], 16)?,
        u8::from_str_radix(&value[2..4], 16)?,
        u8::from_str_radix(&value[4..6], 16)?,
    ])
}

fn parse(xml: &str) -> Result<Scene> {
    let doc = roxmltree::Document::parse(xml)?;
    let model = doc.root_element();
    if !model.has_tag_name((CORE, "model")) {
        return Err("expected a 3MF core model element".into());
    }
    if let Some(extensions) = model.attribute("requiredextensions") {
        for prefix in extensions.split_whitespace() {
            let namespace = model.lookup_namespace_uri(Some(prefix));
            // Material color groups are sufficient for a shaded preview; more complex materials are rejected below.
            if namespace != Some("http://schemas.microsoft.com/3dmanufacturing/material/2015/02") {
                return Err(format!("required 3MF extension {prefix} is not implemented").into());
            }
        }
    }
    let resources = model
        .children()
        .find(|n| n.has_tag_name((CORE, "resources")))
        .ok_or("3MF model has no resources")?;
    let mut all_ids = HashSet::new();
    let mut colors: HashMap<u32, Vec<[u8; 3]>> = HashMap::new();
    for resource in resources.children().filter(|n| n.is_element()) {
        let resource_id = id(resource, "id")?;
        if !all_ids.insert(resource_id) {
            return Err("duplicate 3MF resource ID".into());
        }
        match resource.tag_name().name() {
            "basematerials" | "colorgroup" => {
                let palette = resource
                    .children()
                    .filter(|n| n.is_element())
                    .map(|n| {
                        color(attribute(
                            n,
                            if n.tag_name().name() == "base" {
                                "displaycolor"
                            } else {
                                "color"
                            },
                        )?)
                    })
                    .collect::<Result<Vec<_>>>()?;
                colors.insert(resource_id, palette);
            }
            "object" => {}
            // Complex texture/material properties cannot be rendered faithfully by this importer.
            "texture2d" | "texture2dgroup" | "compositematerials" | "multiproperties" => {
                return Err("3MF textures and composite materials are not implemented".into());
            }
            _ => {
                return Err(
                    format!("unsupported 3MF resource {}", resource.tag_name().name()).into(),
                );
            }
        }
    }
    let mut objects = HashMap::new();
    for resource in resources
        .children()
        .filter(|n| n.has_tag_name((CORE, "object")))
    {
        let object_id = id(resource, "id")?;
        let object_pid: Option<u32> = resource.attribute("pid").map(str::parse).transpose()?;
        let object_pindex: usize = resource
            .attribute("pindex")
            .map(str::parse)
            .transpose()?
            .unwrap_or(0);
        let mut object = Object {
            triangles: Vec::new(),
            components: Vec::new(),
        };
        if let Some(mesh) = resource.children().find(|n| n.has_tag_name((CORE, "mesh"))) {
            let vertices_node = mesh
                .children()
                .find(|n| n.has_tag_name((CORE, "vertices")))
                .ok_or("3MF mesh has no vertices")?;
            let mut vertices = Vec::new();
            for node in vertices_node
                .children()
                .filter(|n| n.has_tag_name((CORE, "vertex")))
            {
                let vertex = Vec3::new(
                    attribute(node, "x")?.parse()?,
                    attribute(node, "y")?.parse()?,
                    attribute(node, "z")?.parse()?,
                );
                if !vertex.is_finite() {
                    return Err("3MF contains non-finite vertices".into());
                }
                vertices.push(vertex);
            }
            let triangles = mesh
                .children()
                .find(|n| n.has_tag_name((CORE, "triangles")))
                .ok_or("3MF mesh has no triangles")?;
            for node in triangles
                .children()
                .filter(|n| n.has_tag_name((CORE, "triangle")))
            {
                if object.triangles.len() >= MAX_TRIANGLES {
                    return Err("3MF exceeds triangle limit".into());
                }
                let indices = [id(node, "v1")?, id(node, "v2")?, id(node, "v3")?];
                if indices.iter().any(|&i| i as usize >= vertices.len()) {
                    return Err("3MF triangle references a missing vertex".into());
                }
                let pid: Option<u32> = node
                    .attribute("pid")
                    .map(str::parse)
                    .transpose()?
                    .or(object_pid);
                let triangle_color = if let Some(pid) = pid {
                    let palette = colors
                        .get(&pid)
                        .ok_or("3MF color resource is missing or unsupported")?;
                    let p1: usize = node
                        .attribute("p1")
                        .map(str::parse)
                        .transpose()?
                        .unwrap_or(object_pindex);
                    let p2: usize = node
                        .attribute("p2")
                        .map(str::parse)
                        .transpose()?
                        .unwrap_or(p1);
                    let p3: usize = node
                        .attribute("p3")
                        .map(str::parse)
                        .transpose()?
                        .unwrap_or(p1);
                    if [p1, p2, p3].iter().any(|&i| i >= palette.len()) {
                        return Err("3MF color index is out of range".into());
                    }
                    Some(std::array::from_fn(|channel| {
                        ((palette[p1][channel] as u32
                            + palette[p2][channel] as u32
                            + palette[p3][channel] as u32)
                            / 3) as u8
                    }))
                } else {
                    None
                };
                object.triangles.push(Triangle {
                    vertices: indices.map(|i| vertices[i as usize]),
                    color: triangle_color,
                });
            }
        }
        if let Some(components) = resource
            .children()
            .find(|n| n.has_tag_name((CORE, "components")))
        {
            if !object.triangles.is_empty() {
                return Err("3MF object cannot contain both mesh and components".into());
            }
            for component in components
                .children()
                .filter(|n| n.has_tag_name((CORE, "component")))
            {
                if component.attributes().any(|a| a.name() == "path") {
                    return Err(
                        "3MF production extension with external model parts is not implemented"
                            .into(),
                    );
                }
                object.components.push((
                    id(component, "objectid")?,
                    Transform::from_3mf(component.attribute("transform"))?,
                ));
            }
        }
        objects.insert(object_id, object);
    }
    let build = model
        .children()
        .find(|n| n.has_tag_name((CORE, "build")))
        .ok_or("3MF model has no build")?;
    let mut scene = Scene::default();
    for item in build.children().filter(|n| n.has_tag_name((CORE, "item"))) {
        if item.attributes().any(|a| a.name() == "path") {
            return Err("3MF external build parts are not implemented".into());
        }
        instantiate(
            id(item, "objectid")?,
            Transform::from_3mf(item.attribute("transform"))?,
            &objects,
            &mut scene,
            &mut HashSet::new(),
        )?;
    }
    scene.validate()?;
    Ok(scene)
}

fn instantiate(
    id: u32,
    transform: Transform,
    objects: &HashMap<u32, Object>,
    scene: &mut Scene,
    active: &mut HashSet<u32>,
) -> Result<()> {
    if active.len() >= 256 || !active.insert(id) {
        return Err("3MF components are cyclic or too deep".into());
    }
    let object = objects.get(&id).ok_or("3MF references a missing object")?;
    if scene.triangles.len().saturating_add(object.triangles.len()) > MAX_TRIANGLES {
        return Err("3MF instantiated scene exceeds triangle limit".into());
    }
    scene
        .triangles
        .extend(object.triangles.iter().map(|triangle| Triangle {
            vertices: triangle.vertices.map(|v| transform.apply(v)),
            color: triangle.color,
        }));
    for (child, child_transform) in &object.components {
        instantiate(
            *child,
            transform.then(*child_transform),
            objects,
            scene,
            active,
        )?;
    }
    active.remove(&id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model(resources: &str, build: &str) -> String {
        format!(
            "<model xmlns=\"{CORE}\"><resources>{resources}</resources><build>{build}</build></model>"
        )
    }
    const MESH: &str = "<object id=\"1\"><mesh><vertices><vertex x=\"0\" y=\"0\" z=\"0\"/><vertex x=\"1\" y=\"0\" z=\"0\"/><vertex x=\"0\" y=\"1\" z=\"0\"/></vertices><triangles><triangle v1=\"0\" v2=\"1\" v3=\"2\"/></triangles></mesh></object>";
    #[test]
    fn build_and_component_transforms_compose() {
        let xml = model(
            &format!(
                "{MESH}<object id=\"2\"><components><component objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 10 0 0\"/></components></object>"
            ),
            "<item objectid=\"2\" transform=\"2 0 0 0 2 0 0 0 2 0 3 0\"/>",
        );
        let scene = parse(&xml).unwrap();
        assert_eq!(scene.triangles[0].vertices[0], Vec3::new(20., 3., 0.));
        assert_eq!(scene.triangles[0].vertices[1], Vec3::new(22., 3., 0.));
    }
    #[test]
    fn cycles_missing_objects_and_bad_indices_fail() {
        assert!(
            parse(&model(
                "<object id=\"1\"><components><component objectid=\"1\"/></components></object>",
                "<item objectid=\"1\"/>"
            ))
            .is_err()
        );
        assert!(parse(&model(MESH, "<item objectid=\"2\"/>")).is_err());
        assert!(
            parse(&model(
                &MESH.replace("v3=\"2\"", "v3=\"3\""),
                "<item objectid=\"1\"/>"
            ))
            .is_err()
        );
    }
    #[test]
    fn package_paths_decode_and_validate() {
        assert_eq!(package_path("/3D/a%20b.model").unwrap(), "3D/a b.model");
        assert!(package_path("../a.model").is_err());
        assert!(package_path("file:/tmp/a.model").is_err());
    }
}
