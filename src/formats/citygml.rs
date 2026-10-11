//! CityGML polygonal geometry, including planar holes and local XLinks.
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::scene::{Point, Scene, Triangle, Vec3};
use crate::{Error, Result};
use roxmltree::{Document, Node, NodeId};

const MAX_ITEMS: usize = 20_000_000;
const MAX_RING: usize = 100_000;
const XLINK: &str = "http://www.w3.org/1999/xlink";
fn error(message: impl Into<String>) -> Error {
    message.into().into()
}
fn gml(node: Node<'_, '_>) -> bool {
    node.is_element()
        && node.tag_name().namespace().is_some_and(|ns| {
            ns == "http://www.opengis.net/gml" || ns.starts_with("http://www.opengis.net/gml/")
        })
}
fn local<'a, 'i>(node: Node<'a, 'i>, name: &str) -> impl Iterator<Item = Node<'a, 'i>> {
    let name = name.to_owned();
    node.children()
        .filter(move |n| n.is_element() && n.tag_name().name() == name)
}
fn id<'a, 'i>(node: Node<'a, 'i>) -> Option<&'a str> {
    node.attributes()
        .find(|a| {
            a.name() == "id"
                && a.namespace()
                    .is_some_and(|ns| ns.starts_with("http://www.opengis.net/gml"))
        })
        .map(|a| a.value())
}
fn floats(text: &str) -> Result<Vec<f64>> {
    let mut values = Vec::new();
    for word in text.split_whitespace() {
        let n: f64 = word.parse()?;
        if !n.is_finite() {
            return Err(error("non-finite CityGML coordinate"));
        }
        values.push(n);
        if values.len() > MAX_ITEMS {
            return Err(error("too many CityGML coordinate values"));
        }
    }
    Ok(values)
}
fn dimension(node: Node<'_, '_>) -> Result<Option<usize>> {
    for ancestor in node.ancestors() {
        if let Some(n) = ancestor.attribute("srsDimension") {
            let n: usize = n.parse()?;
            if n != 2 && n != 3 {
                return Err(error("CityGML coordinates need two or three dimensions"));
            }
            return Ok(Some(n));
        }
    }
    Ok(None)
}
fn position(node: Node<'_, '_>) -> Result<Vec3> {
    let values = floats(node.text().unwrap_or(""))?;
    let dim = dimension(node)?.unwrap_or(values.len());
    if values.len() != dim || (dim != 2 && dim != 3) {
        return Err(error("invalid CityGML position tuple"));
    }
    Ok(Vec3::new(
        values[0],
        values[1],
        if dim == 3 { values[2] } else { 0. },
    ))
}
fn coordinate_list(node: Node<'_, '_>) -> Result<Vec<Vec3>> {
    let values = floats(node.text().unwrap_or(""))?;
    let declared_count = node
        .attribute("count")
        .map(str::parse::<usize>)
        .transpose()?;
    let dim = dimension(node)?
        .or_else(|| declared_count.filter(|&n| n != 0).map(|n| values.len() / n))
        .unwrap_or(3);
    if !matches!(dim, 2 | 3) || values.len() % dim != 0 {
        return Err(error("incomplete CityGML position list"));
    }
    let count = values.len() / dim;
    if declared_count.is_some_and(|n| n != count) {
        return Err(error("CityGML posList count differs from its coordinates"));
    }
    if count > MAX_RING {
        return Err(error("CityGML ring exceeds 100,000 vertices"));
    }
    Ok(values
        .chunks_exact(dim)
        .map(|p| Vec3::new(p[0], p[1], if dim == 3 { p[2] } else { 0. }))
        .collect())
}

fn ring(node: Node<'_, '_>) -> Result<Vec<Vec3>> {
    let mut vertices = Vec::new();
    match node.tag_name().name() {
        "LinearRing" => {
            if let Some(list) = local(node, "posList").next() {
                vertices = coordinate_list(list)?;
            } else if let Some(coords) = local(node, "coordinates").next() {
                if coords.attribute("decimal").is_some_and(|d| d != ".") {
                    return Err(error(
                        "nonstandard CityGML decimal separator is not supported",
                    ));
                }
                let component_separator = coords.attribute("cs").unwrap_or(",");
                let tuple_separator = coords.attribute("ts").unwrap_or(" ");
                for tuple in coords
                    .text()
                    .unwrap_or("")
                    .split(tuple_separator)
                    .filter(|s| !s.trim().is_empty())
                {
                    let values = tuple
                        .split(component_separator)
                        .map(str::parse::<f64>)
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    if values.len() != 2 && values.len() != 3 {
                        return Err(error("invalid CityGML coordinates tuple"));
                    }
                    let p = Vec3::new(values[0], values[1], values.get(2).copied().unwrap_or(0.));
                    if !p.is_finite() {
                        return Err(error("non-finite CityGML coordinate"));
                    }
                    vertices.push(p);
                }
            } else {
                for point in node.children().filter(|n| n.is_element()) {
                    match point.tag_name().name() {
                        "pos" => vertices.push(position(point)?),
                        "pointProperty" | "pointRep" => {
                            let point = local(point, "Point")
                                .next()
                                .ok_or_else(|| error("CityGML point property lacks a Point"))?;
                            vertices.push(position(
                                local(point, "pos")
                                    .next()
                                    .ok_or_else(|| error("CityGML Point lacks pos"))?,
                            )?);
                        }
                        _ => {
                            return Err(error(
                                "unsupported CityGML LinearRing coordinate encoding",
                            ));
                        }
                    }
                }
            }
        }
        _ => {
            return Err(error(
                "CityGML polygon boundaries require LinearRing; curved/composite rings are not supported",
            ));
        }
    }
    if vertices.len() < 4 || vertices.first() != vertices.last() {
        return Err(error(
            "CityGML LinearRing must close and contain at least three distinct positions",
        ));
    }
    vertices.pop();
    if vertices.len() > MAX_RING {
        return Err(error("CityGML ring exceeds 100,000 vertices"));
    }
    let mut clean = Vec::new();
    for p in vertices {
        if clean.last() != Some(&p) {
            clean.push(p);
        }
    }
    if clean.len() < 3 {
        return Err(error("degenerate CityGML ring"));
    }
    Ok(clean)
}

pub(super) fn triangulate_rings(rings: &[Vec<Vec3>]) -> Result<Vec<[Vec3; 3]>> {
    if rings
        .iter()
        .any(|ring| ring.len() < 3 || ring.iter().any(|p| !p.is_finite()))
    {
        return Err(error(
            "polygon rings require at least three finite positions",
        ));
    }
    let exterior = rings
        .first()
        .ok_or_else(|| error("CityGML polygon lacks exterior"))?;
    let origin = exterior[0];
    let mut normal = Vec3::default();
    for i in 0..exterior.len() {
        normal = normal + (exterior[i] - origin).cross(exterior[(i + 1) % exterior.len()] - origin);
    }
    if !normal.is_finite() || normal.length() == 0. {
        return Err(error("degenerate CityGML polygon"));
    }
    let normal = normal.normalized();
    let axis = if normal.x.abs() >= normal.y.abs() && normal.x.abs() >= normal.z.abs() {
        0
    } else if normal.y.abs() >= normal.z.abs() {
        1
    } else {
        2
    };
    let mut positions = Vec::new();
    let mut flat = Vec::new();
    let mut holes = Vec::new();
    let mut expected_area = 0.;
    let mut scale = 0_f64;
    for (i, ring) in rings.iter().enumerate() {
        if i != 0 {
            holes.push(positions.len());
        }
        let start = flat.len();
        for &p in ring {
            let local = p - origin;
            scale = scale.max(local.length());
            positions.push(p);
            let xy = match axis {
                0 => [local.y, local.z],
                1 => [local.x, local.z],
                _ => [local.x, local.y],
            };
            flat.extend_from_slice(&xy);
        }
        let xy: Vec<_> = flat[start..].as_chunks::<2>().0.iter().collect();
        let area = (0..xy.len())
            .map(|k| xy[k][0] * xy[(k + 1) % xy.len()][1] - xy[k][1] * xy[(k + 1) % xy.len()][0])
            .sum::<f64>()
            .abs()
            / 2.;
        expected_area += if i == 0 { area } else { -area };
    }
    if !expected_area.is_finite() || expected_area <= 0. {
        return Err(error("CityGML polygon holes consume its exterior"));
    }
    if positions
        .iter()
        .any(|&p| (p - origin).dot(normal).abs() > scale.max(1e-12) * 1e-6)
    {
        return Err(error(
            "non-planar CityGML polygon cannot be triangulated accurately",
        ));
    }
    let indices = earcutr::earcut(&flat, &holes, 2)
        .map_err(|e| error(format!("cannot triangulate CityGML polygon: {e}")))?;
    let mut triangles = Vec::new();
    let mut area = 0.;
    for indices in indices.as_chunks::<3>().0.iter() {
        let mut triangle = [
            positions[indices[0]],
            positions[indices[1]],
            positions[indices[2]],
        ];
        let cross = (triangle[1] - triangle[0]).cross(triangle[2] - triangle[0]);
        if cross.dot(normal) < 0. {
            triangle.swap(1, 2);
        }
        let project = |p: Vec3| {
            let p = p - origin;
            match axis {
                0 => [p.y, p.z],
                1 => [p.x, p.z],
                _ => [p.x, p.y],
            }
        };
        let [a, b, c] = triangle.map(project);
        area += ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.;
        triangles.push(triangle);
    }
    if (area - expected_area).abs() > expected_area * 1e-7 {
        return Err(error(
            "CityGML triangulation did not preserve its polygon and holes",
        ));
    }
    Ok(triangles)
}

fn feature(node: Node<'_, '_>) -> bool {
    node.is_element()
        && node
            .tag_name()
            .namespace()
            .is_some_and(|ns| ns.contains("/citygml/"))
        && matches!(
            node.tag_name().name(),
            "Building"
                | "BuildingPart"
                | "Bridge"
                | "BridgePart"
                | "Tunnel"
                | "TunnelPart"
                | "GenericCityObject"
                | "CityFurniture"
                | "SolitaryVegetationObject"
                | "PlantCover"
                | "LandUse"
                | "WaterBody"
                | "Road"
                | "Railway"
                | "Track"
                | "Square"
                | "TransportationComplex"
                | "ReliefFeature"
        )
}
fn lod(node: Node<'_, '_>) -> Option<u8> {
    let name = node.tag_name().name().as_bytes();
    if name.len() > 3 && name.starts_with(b"lod") && name[3].is_ascii_digit() {
        Some(name[3] - b'0')
    } else {
        None
    }
}
fn highest_lod(node: Node<'_, '_>) -> Option<u8> {
    node.children()
        .filter(|n| n.is_element() && !feature(*n))
        .filter_map(|n| lod(n).or_else(|| highest_lod(n)))
        .max()
}
struct Importer<'a, 'i> {
    ids: HashMap<&'a str, Node<'a, 'i>>,
    materials: HashMap<String, [u8; 3]>,
    active: HashSet<NodeId>,
    emitted: HashSet<NodeId>,
    scene: Scene,
    visits: usize,
}
impl<'a, 'i> Importer<'a, 'i> {
    fn resolve(&self, node: Node<'a, 'i>) -> Result<Node<'a, 'i>> {
        if let Some(href) = node.attribute((XLINK, "href")) {
            let id = href
                .strip_prefix('#')
                .filter(|s| !s.is_empty())
                .ok_or_else(|| error("CityGML external XLink resources are not supported"))?;
            self.ids
                .get(id)
                .copied()
                .ok_or_else(|| error("CityGML XLink references a missing ID"))
        } else {
            Ok(node)
        }
    }
    fn boundary(&self, property: Node<'a, 'i>) -> Result<Vec<Vec3>> {
        let node = self.resolve(property)?;
        let ring_node = if node.tag_name().name() == "LinearRing" {
            node
        } else {
            let children: Vec<_> = node.children().filter(|n| n.is_element()).collect();
            if children.len() != 1 {
                return Err(error("CityGML boundary needs exactly one ring"));
            }
            self.resolve(children[0])?
        };
        ring(ring_node)
    }
    fn walk(
        &mut self,
        original: Node<'a, 'i>,
        selected_lod: Option<u8>,
        inherited_color: Option<[u8; 3]>,
        reverse: bool,
        depth: usize,
    ) -> Result<()> {
        if !original.is_element() {
            return Ok(());
        }
        if depth > 256 || self.visits > MAX_ITEMS {
            return Err(error("CityGML reference/nesting limit exceeded"));
        }
        self.visits += 1;
        if lod(original).is_some_and(|level| selected_lod.is_some_and(|selected| selected != level))
        {
            return Ok(());
        }
        let name = original.tag_name().name();
        if matches!(
            name,
            "boundedBy" | "Envelope" | "appearance" | "appearanceMember" | "Appearance"
        ) && (!gml(original) || matches!(name, "boundedBy" | "Envelope"))
        {
            // CityGML bldg:boundedBy is a thematic surface wrapper, whereas
            // gml:boundedBy is only spatial extent metadata.
            if name != "boundedBy" || gml(original) {
                return Ok(());
            }
        }
        let node = self.resolve(original)?;
        if !self.active.insert(node.id()) {
            return Err(error("cyclic CityGML XLink"));
        }
        let name = node.tag_name().name();
        let selected_lod = if feature(node) {
            highest_lod(node).or(selected_lod)
        } else {
            selected_lod
        };
        let color = id(node)
            .and_then(|id| self.materials.get(id))
            .copied()
            .or(inherited_color);
        let reverse = match node.attribute("orientation") {
            Some("-") => !reverse,
            Some("+") | None => reverse,
            _ => return Err(error("invalid CityGML surface orientation")),
        };
        if gml(node) && matches!(name, "Polygon" | "Triangle" | "Rectangle" | "PolygonPatch") {
            if self.emitted.insert(node.id()) {
                let exterior = local(node, "exterior")
                    .next()
                    .ok_or_else(|| error("CityGML polygon lacks exterior ring"))?;
                let mut rings = vec![self.boundary(exterior)?];
                for interior in local(node, "interior") {
                    rings.push(self.boundary(interior)?);
                }
                for mut vertices in triangulate_rings(&rings)? {
                    if reverse {
                        vertices.swap(1, 2);
                    }
                    self.scene.triangles.push(Triangle { vertices, color });
                    if self.scene.triangles.len() > MAX_ITEMS {
                        return Err(error("too many CityGML triangles"));
                    }
                }
            }
        } else if gml(node) && name == "Point" {
            if self.emitted.insert(node.id()) {
                let pos = local(node, "pos")
                    .next()
                    .ok_or_else(|| error("CityGML Point needs pos coordinates"))?;
                self.scene.points.push(Point {
                    position: position(pos)?,
                    color,
                });
            }
        } else if (gml(node)
            && matches!(
                name,
                "Curve"
                    | "LineString"
                    | "Arc"
                    | "Circle"
                    | "CompositeCurve"
                    | "MultiCurve"
                    | "MultiLineString"
                    | "BSpline"
                    | "Bezier"
                    | "Sphere"
                    | "Cone"
                    | "Cylinder"
                    | "Tin"
            ))
            || name == "ImplicitGeometry"
        {
            return Err(error(format!("unsupported CityGML geometry <{name}>")));
        } else {
            for child in node.children().filter(|n| n.is_element()) {
                self.walk(child, selected_lod, color, reverse, depth + 1)?;
            }
        }
        self.active.remove(&node.id());
        Ok(())
    }
}

pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    parse(std::str::from_utf8(&bytes)?)
}
fn parse(source: &str) -> Result<Scene> {
    let doc = Document::parse(source)?;
    let root = doc.root_element();
    if root.tag_name().name() != "CityModel"
        || !root
            .tag_name()
            .namespace()
            .is_some_and(|ns| ns.contains("citygml"))
    {
        return Err(error("expected a CityGML CityModel document"));
    }
    let mut ids = HashMap::new();
    let mut materials = HashMap::new();
    for node in root.descendants().filter(|n| n.is_element()) {
        if let Some(id) = id(node)
            && ids.insert(id, node).is_some()
        {
            return Err(error("duplicate CityGML gml:id"));
        }
        if node.tag_name().name() == "X3DMaterial"
            && let Some(diffuse) = local(node, "diffuseColor").next()
        {
            let values = floats(diffuse.text().unwrap_or(""))?;
            if values.len() != 3 {
                return Err(error("CityGML diffuseColor needs RGB"));
            }
            let color = std::array::from_fn(|i| (values[i] * 255.).clamp(0., 255.).round() as u8);
            for target in local(node, "target") {
                let id = target
                    .text()
                    .unwrap_or("")
                    .trim()
                    .strip_prefix('#')
                    .ok_or_else(|| error("CityGML material target must be a local fragment"))?;
                if materials
                    .insert(id.to_owned(), color)
                    .is_some_and(|previous| previous != color)
                {
                    return Err(error(
                        "multiple CityGML appearance themes for one target are not supported",
                    ));
                }
            }
        }
    }
    let mut importer = Importer {
        ids,
        materials,
        active: HashSet::new(),
        emitted: HashSet::new(),
        scene: Scene::default(),
        visits: 0,
    };
    importer.walk(root, None, None, false, 0)?;
    importer.scene.validate()?;
    Ok(importer.scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model(body: &str) -> String {
        format!(
            r##"<core:CityModel xmlns:core="http://www.opengis.net/citygml/2.0" xmlns:bldg="http://www.opengis.net/citygml/building/2.0" xmlns:gml="http://www.opengis.net/gml" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:app="http://www.opengis.net/citygml/appearance/2.0">{body}</core:CityModel>"##
        )
    }
    fn polygon(id: &str, exterior: &str, holes: &[&str]) -> String {
        let holes: String = holes.iter().map(|coords| format!("<gml:interior><gml:LinearRing><gml:posList>{coords}</gml:posList></gml:LinearRing></gml:interior>")).collect();
        format!(
            r##"<gml:Polygon gml:id="{id}"><gml:exterior><gml:LinearRing><gml:posList>{exterior}</gml:posList></gml:LinearRing></gml:exterior>{holes}</gml:Polygon>"##
        )
    }
    fn area(scene: &Scene) -> f64 {
        scene
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    / 2.
            })
            .sum()
    }
    #[test]
    fn polygon_holes_preserve_area_in_arbitrary_plane() {
        let poly = polygon(
            "p",
            "0 0 0 4 0 0 4 4 0 0 4 0 0 0 0",
            &["1 1 0 1 3 0 3 3 0 3 1 0 1 1 0"],
        );
        let scene = parse(&model(&poly)).unwrap();
        assert!((area(&scene) - 12.).abs() < 1e-12);
        let poly = polygon(
            "p",
            "10 0 0 10 4 0 10 4 4 10 0 4 10 0 0",
            &["10 1 1 10 1 3 10 3 3 10 3 1 10 1 1"],
        );
        assert!((area(&parse(&model(&poly)).unwrap()) - 12.).abs() < 1e-12);
    }
    #[test]
    fn building_lod_xlinks_and_materials() {
        let high = polygon("high", "0 0 2 2 0 2 2 2 2 0 2 2 0 0 2", &[]);
        let low = polygon("low", "0 0 0 1 0 0 0 1 0 0 0 0", &[]);
        let source = model(&format!(
            r##"<core:cityObjectMember><bldg:Building><bldg:lod1MultiSurface>{low}</bldg:lod1MultiSurface><bldg:lod2MultiSurface>{high}<gml:surfaceMember xlink:href="#high"/></bldg:lod2MultiSurface></bldg:Building></core:cityObjectMember><app:appearanceMember><app:Appearance><app:surfaceDataMember><app:X3DMaterial><app:diffuseColor>1 0 0</app:diffuseColor><app:target>#high</app:target></app:X3DMaterial></app:surfaceDataMember></app:Appearance></app:appearanceMember>"##
        ));
        let scene = parse(&source).unwrap();
        assert_eq!(scene.triangles.len(), 2);
        assert!((area(&scene) - 4.).abs() < 1e-12);
        assert!(scene.triangles.iter().all(|t| t.color == Some([255, 0, 0])));
    }
    #[test]
    fn rejects_unclosed_nonplanar_and_missing_reference() {
        assert!(parse(&model(&polygon("p", "0 0 0 1 0 0 0 1 0", &[]))).is_err());
        assert!(parse(&model(&polygon("p", "0 0 0 1 0 0 1 1 1 0 1 0 0 0 0", &[]))).is_err());
        assert!(parse(&model(r##"<gml:surfaceMember xlink:href="#missing"/>"##)).is_err());
        assert!(
            parse(&model(
                r##"<gml:surfaceMember xlink:href="remote.gml#p"/>"##
            ))
            .is_err()
        );
    }
}
