//! Bounded AMF triangle geometry and explicitly qualified constellation placements.
//!
//! Syntax and units follow ISO's publicly supplied schema:
//! <https://standards.iso.org/iso/52915/ISOASTM52915(2016)AMF.xsd>
//! The original Slic3r 1.3.1 exporter writes Z rotation in radians and adds
//! uniform scale: <https://github.com/slic3r/Slic3r/blob/026c1380e0ad12176dd2fc8cdf8f6f181deeb071/xs/src/libslic3r/IO/AMF.cpp>

use crate::{
    Result,
    scene::{AlphaMode, Scene, Triangle, TriangleAppearance, Vec3},
};
use roxmltree::{Document, Node, ParsingOptions};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
    path::Path,
};

/// Maximum encoded source or expanded XML size.
const MAX_BYTES: usize = 64 * 1024 * 1024;
/// Maximum XML node count, admitted before DOM allocation.
const MAX_NODES: u32 = 2_000_000;
/// Maximum vertices and triangles across all authored objects and placed output.
const MAX_GEOMETRY: usize = 1_000_000;
/// Maximum objects, materials, constellations and instance records.
const MAX_RECORDS: usize = 100_000;
/// Maximum nested constellation depth.
const MAX_DEPTH: usize = 64;
/// Conservative retained geometry and instance-output accounting limit.
const MAX_RETAINED: usize = 256 * 1024 * 1024;
/// Maximum charged instance traversal and position-transform work.
const MAX_WORK: usize = 50_000_000;
/// Constant source sRGB channels and linear alpha.
type Color = [f64; 4];

/// Source geometry referenced by one object's triangles.
struct Mesh {
    /// Unit-normalized positions.
    positions: Vec<Vec3>,
    /// Validated index triples and inherited constant colors.
    faces: Vec<Face>,
}
/// A source triangle with its resolved appearance.
struct Face {
    /// Zero-based positions within the owning object.
    indices: [usize; 3],
    /// Most specific constant color, or the renderer default.
    color: Option<Color>,
}
/// Unit conversion and the one explicitly identified legacy placement dialect.
struct Configuration {
    /// Millimeters per authored length unit.
    units: f64,
    /// Whether the documented Slic3r 1.3.1 rotation/scale convention applies.
    legacy_slic3r: bool,
    /// Whether another Slic3r version declares an unqualified rotation convention.
    unknown_slic3r: bool,
}
/// One qualified uniform scale, single-axis rotation and translation.
#[derive(Clone, Copy)]
struct Placement {
    /// Translation in millimeters.
    translation: Vec3,
    /// Rotation angles in radians; at most one axis is nonzero.
    rotation: [f64; 3],
    /// Dimensionless uniform scale.
    scale: f64,
}
/// An object or nested constellation occurrence.
struct Instance {
    /// Global object or constellation identifier.
    target: u64,
    /// Placement of the referenced definition.
    placement: Placement,
}
/// A globally identified printable definition.
enum Definition {
    /// Authored triangle mesh.
    Object(Mesh),
    /// Nested references to other definitions.
    Constellation(Vec<Instance>),
}
/// Cumulative admission of geometry and retained data before growth.
#[derive(Default)]
struct Budget {
    /// Source position count.
    vertices: usize,
    /// Source and output triangle count, charged independently.
    source_faces: usize,
    /// Placed triangles emitted into the scene.
    output_faces: usize,
    /// Definition, material and occurrence count.
    records: usize,
    /// Conservative cumulative retained-byte estimate.
    bytes: usize,
    /// Charged scene traversal and transform operations.
    work: usize,
    /// Whether the appearance tree's initial reserved node has been admitted.
    appearance_root: bool,
}
impl Budget {
    /// Admit bounded traversal and transform work before evaluating it.
    ///
    /// # Errors
    /// Returns an error for work-count overflow or excessive instance expansion.
    fn work(&mut self, amount: usize) -> Result<()> {
        self.work = self
            .work
            .checked_add(amount)
            .ok_or("AMF work-size overflow")?;
        if self.work > MAX_WORK {
            return Err("AMF expansion exceeds the work limit".into());
        }
        Ok(())
    }
    /// Admit retained bytes before extending an owned collection.
    ///
    /// # Errors
    /// Returns an error for overflow or the cumulative retained-data limit.
    fn retain(&mut self, bytes: usize) -> Result<()> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or("AMF retained-size overflow")?;
        if self.bytes > MAX_RETAINED {
            return Err("AMF retained geometry exceeds the 256 MiB limit".into());
        }
        Ok(())
    }
    /// Admit one resource or reference before insertion.
    ///
    /// # Errors
    /// Returns an error when the record or retained-data budget is exceeded.
    fn record(&mut self) -> Result<()> {
        self.records += 1;
        if self.records > MAX_RECORDS {
            return Err("AMF exceeds the 100000-record limit".into());
        }
        self.retain(512)
    }
    /// Admit a source position before insertion.
    ///
    /// # Errors
    /// Returns an error when the vertex or retained-data budget is exceeded.
    fn vertex(&mut self) -> Result<()> {
        self.vertices += 1;
        if self.vertices > MAX_GEOMETRY {
            return Err("AMF exceeds the vertex limit".into());
        }
        self.retain(2 * std::mem::size_of::<Vec3>())
    }
    /// Admit a source triangle before insertion.
    ///
    /// # Errors
    /// Returns an error when the triangle or retained-data budget is exceeded.
    fn face(&mut self) -> Result<()> {
        self.source_faces += 1;
        if self.source_faces > MAX_GEOMETRY {
            return Err("AMF exceeds the source triangle limit".into());
        }
        self.retain(2 * std::mem::size_of::<Face>())
    }
    /// Admit a placed triangle and any appearance before output growth.
    ///
    /// # Errors
    /// Returns an error when expanded geometry or retained output exceeds its budget.
    fn output(&mut self, colored: bool) -> Result<()> {
        self.output_faces += 1;
        if self.output_faces > MAX_GEOMETRY {
            return Err("AMF constellation exceeds the output triangle limit".into());
        }
        // Three reserved key/value slots per entry cover partially occupied B-tree
        // nodes; twelve initial slots account for the first node even for one face.
        // Each slot also includes pointer/header slack, separately from Vec growth.
        let slot = std::mem::size_of::<(usize, TriangleAppearance)>() + 32;
        let appearance = if colored {
            3 * slot + if self.appearance_root { 0 } else { 12 * slot }
        } else {
            0
        };
        self.retain(2 * std::mem::size_of::<Triangle>() + appearance)?;
        self.appearance_root |= colored;
        Ok(())
    }
}

/// Read plain XML or a single-file ZIP and generate its actual triangle geometry.
///
/// # Errors
/// Returns an error for malformed or unsupported AMF geometry, ambiguous archives,
/// missing references, invalid coordinates or indices, and exceeded reader limits.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let source = std::fs::File::open(path)?;
    if source.metadata()?.len() > MAX_BYTES as u64 {
        return Err("AMF encoded source exceeds the 64 MiB limit".into());
    }
    let mut encoded = Vec::new();
    source
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut encoded)?;
    if encoded.len() > MAX_BYTES {
        return Err("AMF encoded source exceeds the 64 MiB limit".into());
    }
    parse(&xml_bytes(encoded)?)
}

/// Return a bounded XML payload without extracting archive paths onto disk.
///
/// # Errors
/// Returns an error for unsupported compression, multiple members, nonregular or
/// unsafe member names, CRC/stream errors, or an oversized XML payload.
fn xml_bytes(encoded: Vec<u8>) -> Result<Vec<u8>> {
    if !encoded.starts_with(b"PK") {
        return Ok(encoded);
    }
    single_member_zip(&encoded)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(encoded))?;
    if archive.len() != 1 {
        return Err("AMF ZIP must contain exactly one model file".into());
    }
    let mut member = archive.by_index(0)?;
    let name = member.name()?;
    if member.is_dir()
        || member.is_symlink()
        || name.starts_with('/')
        || name.contains('\\')
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || !name.to_ascii_lowercase().ends_with(".amf")
    {
        return Err("AMF ZIP has an unsupported model member name or type".into());
    }
    if !matches!(
        member.compression(),
        zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
    ) {
        return Err("AMF ZIP supports stored and deflated XML only".into());
    }
    if member.size() > MAX_BYTES as u64 {
        return Err("AMF expanded XML exceeds the 64 MiB limit".into());
    }
    let mut xml = Vec::new();
    member
        .by_ref()
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut xml)?;
    if xml.len() > MAX_BYTES || xml.len() as u64 != member.size() {
        return Err("AMF ZIP XML length is invalid or exceeds the limit".into());
    }
    Ok(xml)
}

/// Inspect the classic central-directory count before ZIP name coalescing.
///
/// # Errors
/// Returns an error for split/ZIP64 archives, inconsistent directory ranges or
/// any physical entry count other than one. PKWARE APPNOTE defines these headers.
fn single_member_zip(bytes: &[u8]) -> Result<()> {
    let first = bytes.len().saturating_sub(22 + usize::from(u16::MAX));
    let footer = (first..bytes.len().saturating_sub(21))
        .rev()
        .find(|&offset| {
            bytes.get(offset..offset + 4) == Some(b"PK\x05\x06")
                && zip_u16(bytes, offset + 20)
                    .is_ok_and(|comment| offset + 22 + usize::from(comment) == bytes.len())
        })
        .ok_or("AMF ZIP has no complete classic end record")?;
    if zip_u16(bytes, footer + 4)? != 0
        || zip_u16(bytes, footer + 6)? != 0
        || zip_u16(bytes, footer + 8)? != 1
        || zip_u16(bytes, footer + 10)? != 1
    {
        return Err("AMF ZIP must have exactly one physical entry and cannot use split or ZIP64 directories".into());
    }
    let size = usize::try_from(zip_u32(bytes, footer + 12)?)?;
    let start = usize::try_from(zip_u32(bytes, footer + 16)?)?;
    if size < 46
        || start.checked_add(size) != Some(footer)
        || bytes.get(start..start + 4) != Some(b"PK\x01\x02")
    {
        return Err("invalid AMF ZIP central-directory extent".into());
    }
    let length = 46
        + usize::from(zip_u16(bytes, start + 28)?)
        + usize::from(zip_u16(bytes, start + 30)?)
        + usize::from(zip_u16(bytes, start + 32)?);
    if length != size
        || zip_u16(bytes, start + 34)? != 0
        || zip_u32(bytes, start + 20)? == u32::MAX
        || zip_u32(bytes, start + 24)? == u32::MAX
        || zip_u32(bytes, start + 42)? == u32::MAX
    {
        return Err("AMF ZIP has an inconsistent entry or unsupported ZIP64 encoding".into());
    }
    Ok(())
}
/// Read one checked classic ZIP header word.
///
/// # Errors
/// Returns an error for a truncated header or an overflowing range.
fn zip_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let field = bytes
        .get(offset..offset.checked_add(2).ok_or("AMF ZIP offset overflow")?)
        .ok_or("truncated AMF ZIP header")?;
    Ok(u16::from_le_bytes(field.try_into()?))
}
/// Read one checked classic ZIP header double word.
///
/// # Errors
/// Returns an error for a truncated header or an overflowing range.
fn zip_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let field = bytes
        .get(offset..offset.checked_add(4).ok_or("AMF ZIP offset overflow")?)
        .ok_or("truncated AMF ZIP header")?;
    Ok(u32::from_le_bytes(field.try_into()?))
}

/// Validate XML structure, source geometry and the complete reference graph.
///
/// # Errors
/// Returns an error for unsupported syntax, malformed geometry, invalid references,
/// a cyclic constellation or an exceeded cumulative resource limit.
fn parse(xml: &[u8]) -> Result<Scene> {
    if xml.len() > MAX_BYTES {
        return Err("AMF XML exceeds the 64 MiB limit".into());
    }
    let text = std::str::from_utf8(xml)?;
    let document = Document::parse_with_options(
        text,
        ParsingOptions {
            nodes_limit: MAX_NODES,
            ..ParsingOptions::default()
        },
    )?;
    let root = document.root_element();
    let Configuration {
        units,
        legacy_slic3r,
        unknown_slic3r,
    } = configuration(root)?;
    let mut budget = Budget::default();
    let mut materials = BTreeMap::new();
    for node in root.children().filter(|node| node.has_tag_name("material")) {
        budget.record()?;
        attributes(node, &["id"])?;
        children(node, &["color", "metadata"])?;
        let id = identifier(node, "id")?;
        if id == 0 || materials.insert(id, color(node)?).is_some() {
            return Err("AMF material ID is reserved or duplicated".into());
        }
    }
    let mut definitions = BTreeMap::new();
    let mut references = BTreeSet::new();
    for node in root.children().filter(Node::is_element) {
        let definition = match node.tag_name().name() {
            "object" => Some(Definition::Object(mesh(
                node,
                units,
                &materials,
                &mut budget,
            )?)),
            "constellation" => Some(Definition::Constellation(instances(
                node,
                units,
                legacy_slic3r,
                unknown_slic3r,
                &mut budget,
            )?)),
            _ => None,
        };
        if let Some(definition) = definition {
            budget.record()?;
            let id = identifier(node, "id")?;
            if let Definition::Constellation(instances) = &definition {
                references.extend(instances.iter().map(|instance| instance.target));
            }
            if definitions.insert(id, definition).is_some() {
                return Err("duplicate AMF object or constellation ID".into());
            }
        }
    }
    if !definitions
        .values()
        .any(|definition| matches!(definition, Definition::Object(_)))
    {
        return Err("AMF has no object geometry".into());
    }
    for reference in &references {
        if !definitions.contains_key(reference) {
            return Err("AMF instance references a missing definition".into());
        }
    }
    let mut visiting = BTreeSet::new();
    let mut complete = BTreeMap::new();
    for &id in definitions.keys() {
        check_graph(id, &definitions, &mut visiting, &mut complete, 0)?;
    }
    let mut scene = Scene::default();
    for &id in definitions.keys().filter(|id| !references.contains(id)) {
        emit(id, &definitions, &mut Vec::new(), &mut budget, &mut scene)?;
    }
    if scene.triangles.is_empty() {
        return Err("AMF has no printable triangles".into());
    }
    Ok(scene)
}

/// Validate the document header and explicitly identify its unit/dialect rules.
///
/// # Errors
/// Returns an error for unsupported root syntax, versions or units, ambiguous
/// producer metadata, and oversized or nested producer text fields.
fn configuration(root: Node<'_, '_>) -> Result<Configuration> {
    require_name(root, "amf")?;
    attributes(root, &["unit", "version"])?;
    children(root, &["object", "material", "constellation", "metadata"])?;
    if root.attribute("version").is_some_and(|version| {
        !matches!(
            version
                .parse::<f64>()
                .map(|value| value.to_string())
                .as_deref(),
            Ok("1" | "1.1" | "1.2")
        )
    }) {
        return Err("unsupported AMF version".into());
    }
    let units = match root.attribute("unit").unwrap_or("millimeter") {
        "millimeter" => 1.0,
        "meter" => 1000.0,
        "micron" => 0.001,
        "inch" => 25.4,
        "feet" => 304.8,
        _ => return Err("unsupported AMF length unit".into()),
    };
    let mut producer = None;
    for metadata in root.children().filter(|node| node.has_tag_name("metadata")) {
        if metadata
            .attribute("type")
            .is_some_and(|kind| kind.eq_ignore_ascii_case("cad"))
        {
            if producer.is_some() {
                return Err("ambiguous AMF producer metadata".into());
            }
            producer = Some(text_field(metadata)?);
        }
    }
    let legacy_slic3r = producer
        .as_deref()
        .is_some_and(|text| text == "Slic3r 1.3.1" || text.starts_with("Slic3r 1.3.1-"));
    let unknown_slic3r = !legacy_slic3r
        && producer
            .as_deref()
            .is_some_and(|text| text.starts_with("Slic3r "));
    Ok(Configuration {
        units,
        legacy_slic3r,
        unknown_slic3r,
    })
}

/// Require one unqualified schema element with the expected name.
///
/// # Errors
/// Returns an error for a mismatched element name or a namespace extension.
fn require_name(node: Node<'_, '_>, name: &str) -> Result<()> {
    if node.tag_name().name() != name || node.tag_name().namespace().is_some() {
        return Err(format!("unsupported AMF element {}", node.tag_name().name()).into());
    }
    Ok(())
}
/// Reject unknown schema attributes; XML language metadata is harmless.
///
/// # Errors
/// Returns an error when an attribute could introduce unsupported semantics.
fn attributes(node: Node<'_, '_>, allowed: &[&str]) -> Result<()> {
    for attribute in node.attributes() {
        if attribute.namespace() == Some("http://www.w3.org/XML/1998/namespace")
            && attribute.name() == "lang"
        {
            continue;
        }
        if attribute.namespace().is_some() || !allowed.contains(&attribute.name()) {
            return Err(format!("unsupported AMF attribute {}", attribute.name()).into());
        }
    }
    Ok(())
}
/// Reject unsupported geometry and appearance children instead of silently flattening them.
///
/// # Errors
/// Returns an error for an unknown, namespaced or unsupported child element.
fn children(node: Node<'_, '_>, allowed: &[&str]) -> Result<()> {
    for child in node.children().filter(Node::is_element) {
        if child.tag_name().namespace().is_some() || !allowed.contains(&child.tag_name().name()) {
            return Err(format!(
                "unsupported AMF {} child {}",
                node.tag_name().name(),
                child.tag_name().name()
            )
            .into());
        }
    }
    Ok(())
}
/// Select an optional singleton child and reject duplicates.
///
/// # Errors
/// Returns an error for duplicate schema fields.
fn optional<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Result<Option<Node<'a, 'input>>> {
    let mut matching = node.children().filter(|child| child.has_tag_name(name));
    let first = matching.next();
    if matching.next().is_some() {
        return Err(format!("duplicate AMF {name}").into());
    }
    Ok(first)
}
/// Select a required singleton child.
///
/// # Errors
/// Returns an error when a required field is absent or repeated.
fn required<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Result<Node<'a, 'input>> {
    optional(node, name)?.ok_or_else(|| format!("missing AMF {name}").into())
}
/// Read a finite constant scalar without interpreting expressions.
///
/// # Errors
/// Returns an error for nested content, malformed values, or nonfinite numbers.
fn scalar(node: Node<'_, '_>) -> Result<f64> {
    attributes(node, &[])?;
    children(node, &[])?;
    let value = text_field(node)?
        .parse::<f64>()
        .map_err(|_| "AMF requires constant numeric values; expressions are unsupported")?;
    if !value.is_finite() {
        return Err("nonfinite AMF scalar".into());
    }
    Ok(value)
}
/// Join a bounded scalar's text across XML comments without discarding digits.
///
/// # Errors
/// Returns an error for nested elements, absent text or an oversized field.
fn text_field(node: Node<'_, '_>) -> Result<String> {
    children(node, &[])?;
    let mut text = String::new();
    for piece in node
        .children()
        .filter(Node::is_text)
        .filter_map(|node| node.text())
    {
        if text.len() + piece.len() > 128 {
            return Err("AMF scalar field exceeds the 128-byte limit".into());
        }
        text.push_str(piece);
    }
    if text.trim().is_empty() {
        return Err("missing AMF scalar text".into());
    }
    Ok(text.trim().to_owned())
}
/// Read an optional scalar with a schema-specific default.
///
/// # Errors
/// Returns an error for malformed, repeated or unsupported scalar fields.
fn value(node: Node<'_, '_>, name: &str, default: f64) -> Result<f64> {
    optional(node, name)?.map_or(Ok(default), scalar)
}
/// Parse one nonnegative global resource identifier.
///
/// # Errors
/// Returns an error for a missing or malformed ID.
fn identifier(node: Node<'_, '_>, attribute: &str) -> Result<u64> {
    node.attribute(attribute)
        .ok_or("missing AMF resource ID")?
        .trim()
        .parse::<u64>()
        .map_err(|_| "invalid AMF resource ID".into())
}
/// Decode an optional constant sRGB color with linear alpha.
///
/// # Errors
/// Returns an error for expressions, unsupported color profiles or invalid components.
fn color(node: Node<'_, '_>) -> Result<Option<Color>> {
    let Some(color) = optional(node, "color")? else {
        return Ok(None);
    };
    attributes(color, &[])?;
    children(color, &["r", "g", "b", "a"])?;
    let channels = [
        scalar(required(color, "r")?)?,
        scalar(required(color, "g")?)?,
        scalar(required(color, "b")?)?,
        value(color, "a", 1.0)?,
    ];
    if channels
        .iter()
        .any(|channel| !(0.0..=1.0).contains(channel))
    {
        return Err("AMF color must be between zero and one".into());
    }
    Ok(Some(channels))
}

/// Read one triangle mesh, preserving every supported volume.
///
/// # Errors
/// Returns an error for unsupported geometry, invalid material references, invalid
/// triangle indices or an exceeded cumulative source-geometry budget.
fn mesh(
    node: Node<'_, '_>,
    units: f64,
    materials: &BTreeMap<u64, Option<Color>>,
    budget: &mut Budget,
) -> Result<Mesh> {
    attributes(node, &["id"])?;
    children(node, &["mesh", "color", "metadata"])?;
    let object_color = color(node)?;
    let mesh = required(node, "mesh")?;
    attributes(mesh, &[])?;
    children(mesh, &["vertices", "volume"])?;
    let vertices = required(mesh, "vertices")?;
    attributes(vertices, &[])?;
    children(vertices, &["vertex"])?;
    let mut positions = Vec::new();
    for vertex in vertices.children().filter(Node::is_element) {
        attributes(vertex, &[])?;
        children(vertex, &["coordinates", "metadata"])?;
        let coordinates = required(vertex, "coordinates")?;
        attributes(coordinates, &[])?;
        children(coordinates, &["x", "y", "z"])?;
        let position = Vec3::new(
            scalar(required(coordinates, "x")?)? * units,
            scalar(required(coordinates, "y")?)? * units,
            scalar(required(coordinates, "z")?)? * units,
        );
        finite(position)?;
        budget.vertex()?;
        positions.push(position);
    }
    if positions.is_empty() {
        return Err("AMF object has no vertices".into());
    }
    let mut faces = Vec::new();
    for volume in mesh.children().filter(|node| node.has_tag_name("volume")) {
        attributes(volume, &["materialid", "type"])?;
        children(volume, &["triangle", "color", "metadata"])?;
        if !matches!(volume.attribute("type"), None | Some("object")) {
            return Err("AMF support/void volumes are unsupported".into());
        }
        validate_modifier(volume)?;
        let material_color = if volume.attribute("materialid").is_some() {
            let id = identifier(volume, "materialid")?;
            if id == 0 {
                return Err("AMF void material is unsupported".into());
            }
            *materials
                .get(&id)
                .ok_or("AMF references a missing material")?
        } else {
            None
        };
        let inherited_color = color(volume)?.or(object_color).or(material_color);
        let mut volume_faces = 0;
        for triangle in volume
            .children()
            .filter(|node| node.has_tag_name("triangle"))
        {
            attributes(triangle, &[])?;
            children(triangle, &["v1", "v2", "v3", "color"])?;
            let mut indices = [0; 3];
            for (index, tag) in indices.iter_mut().zip(["v1", "v2", "v3"]) {
                let field = required(triangle, tag)?;
                attributes(field, &[])?;
                children(field, &[])?;
                *index = text_field(field)?
                    .parse()
                    .map_err(|_| "invalid AMF triangle index")?;
                if *index >= positions.len() {
                    return Err("AMF triangle index is out of range".into());
                }
            }
            if indices[0] == indices[1] || indices[1] == indices[2] || indices[0] == indices[2] {
                return Err("AMF triangle repeats a vertex index".into());
            }
            let [a, b, c] = indices.map(|index| positions[index]);
            let area = (b - a).cross(c - a).length();
            if !area.is_finite() || area <= 0.0 {
                return Err("degenerate or overflowing AMF triangle".into());
            }
            budget.face()?;
            faces.push(Face {
                indices,
                color: color(triangle)?.or(inherited_color),
            });
            volume_faces += 1;
        }
        if volume_faces == 0 {
            return Err("AMF volume has no triangles".into());
        }
    }
    if faces.is_empty() {
        return Err("AMF object has no triangle volumes".into());
    }
    Ok(Mesh { positions, faces })
}

/// Reject slicer modifier volumes while leaving descriptive metadata uninterpreted.
///
/// # Errors
/// Returns an error for a malformed known modifier field or a nonzero modifier.
fn validate_modifier(volume: Node<'_, '_>) -> Result<()> {
    for metadata in volume
        .children()
        .filter(|node| node.has_tag_name("metadata"))
    {
        if metadata.attribute("type") == Some("slic3r.modifier") {
            attributes(metadata, &["type"])?;
            let modifier: u64 = text_field(metadata)?
                .trim()
                .parse()
                .map_err(|_| "invalid AMF slicer modifier value")?;
            if modifier != 0 {
                return Err("AMF slicer modifier volumes are unsupported".into());
            }
        }
    }
    Ok(())
}

/// Read finite translations and explicitly qualified rotations/scales.
///
/// # Errors
/// Returns an error for missing references, duplicate transform fields, mixed-axis
/// rotations, unknown scale dialects, or unsupported legacy rotation conventions.
fn instances(
    node: Node<'_, '_>,
    units: f64,
    legacy_slic3r: bool,
    unknown_slic3r: bool,
    budget: &mut Budget,
) -> Result<Vec<Instance>> {
    attributes(node, &["id"])?;
    children(node, &["instance", "metadata"])?;
    let mut instances = Vec::new();
    for instance in node.children().filter(|node| node.has_tag_name("instance")) {
        attributes(instance, &["objectid"])?;
        children(
            instance,
            &["deltax", "deltay", "deltaz", "rx", "ry", "rz", "scale"],
        )?;
        let translation = Vec3::new(
            value(instance, "deltax", 0.0)? * units,
            value(instance, "deltay", 0.0)? * units,
            value(instance, "deltaz", 0.0)? * units,
        );
        finite(translation)?;
        let mut rotation = [
            value(instance, "rx", 0.0)?,
            value(instance, "ry", 0.0)?,
            value(instance, "rz", 0.0)?,
        ];
        if rotation.iter().filter(|angle| angle.abs() > 0.0).count() > 1 {
            return Err("AMF mixed-axis instance rotations are unsupported".into());
        }
        if unknown_slic3r && rotation.iter().any(|angle| angle.abs() > 0.0) {
            return Err("unqualified Slic3r AMF rotation convention".into());
        }
        if legacy_slic3r {
            if rotation[0].abs() > 0.0
                || rotation[1].abs() > 0.0
                || rotation[2].abs() > std::f64::consts::TAU + 1e-4
            {
                return Err("unsupported legacy Slic3r AMF rotation".into());
            }
        } else {
            if rotation.iter().any(|angle| angle.abs() > 360.0) {
                return Err("AMF rotation is outside the degree range".into());
            }
            rotation = rotation.map(f64::to_radians);
        }
        let scale = value(instance, "scale", 1.0)?;
        if optional(instance, "scale")?.is_some() && !legacy_slic3r {
            return Err("AMF scale extension requires the qualified Slic3r 1.3.1 dialect".into());
        }
        if scale <= 0.0 {
            return Err("AMF uniform scale must be positive".into());
        }
        budget.record()?;
        instances.push(Instance {
            target: identifier(instance, "objectid")?,
            placement: Placement {
                translation,
                rotation,
                scale,
            },
        });
    }
    if instances.is_empty() {
        return Err("AMF constellation has no instances".into());
    }
    Ok(instances)
}

/// Require finite geometry after unit conversion or placement.
///
/// # Errors
/// Returns an error for arithmetic overflow or a nonfinite coordinate.
fn finite(position: Vec3) -> Result<()> {
    if !position.x.is_finite() || !position.y.is_finite() || !position.z.is_finite() {
        return Err("nonfinite AMF coordinate after conversion or placement".into());
    }
    Ok(())
}
/// Validate the full graph once, including cycles disconnected from printable roots.
///
/// # Errors
/// Returns an error for cycles, missing references or an excessive nesting depth.
fn check_graph(
    id: u64,
    definitions: &BTreeMap<u64, Definition>,
    visiting: &mut BTreeSet<u64>,
    complete: &mut BTreeMap<u64, usize>,
    depth: usize,
) -> Result<usize> {
    if depth > MAX_DEPTH {
        return Err("AMF constellation nesting exceeds the depth limit".into());
    }
    if let Some(&height) = complete.get(&id) {
        if depth + height > MAX_DEPTH {
            return Err("AMF constellation nesting exceeds the depth limit".into());
        }
        return Ok(height);
    }
    if !visiting.insert(id) {
        return Err("cyclic AMF constellation".into());
    }
    let mut height = 0;
    if let Definition::Constellation(instances) =
        definitions.get(&id).ok_or("missing AMF definition")?
    {
        for instance in instances {
            height = height
                .max(1 + check_graph(instance.target, definitions, visiting, complete, depth + 1)?);
        }
    }
    if depth + height > MAX_DEPTH {
        return Err("AMF constellation nesting exceeds the depth limit".into());
    }
    visiting.remove(&id);
    complete.insert(id, height);
    Ok(height)
}
/// Apply one axis rotation and uniform scale before translation.
fn placed(position: Vec3, placement: Placement) -> Vec3 {
    let mut p = position * placement.scale;
    for (axis, angle) in placement.rotation.into_iter().enumerate() {
        let (sine, cosine) = angle.sin_cos();
        p = match axis {
            0 => Vec3::new(p.x, cosine * p.y - sine * p.z, sine * p.y + cosine * p.z),
            1 => Vec3::new(cosine * p.x + sine * p.z, p.y, -sine * p.x + cosine * p.z),
            _ => Vec3::new(cosine * p.x - sine * p.y, sine * p.x + cosine * p.y, p.z),
        };
    }
    p + placement.translation
}
/// Decode source sRGB channels into the shared renderer's linear appearance space.
fn linear(color: Color) -> Color {
    [
        decode_channel(color[0]),
        decode_channel(color[1]),
        decode_channel(color[2]),
        color[3],
    ]
}
/// Decode one sRGB color component using its standard piecewise transfer curve.
fn decode_channel(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}
/// Emit root definitions exactly once and preserve every placed instance.
///
/// # Errors
/// Returns an error for invalid references, nonfinite placements or expanded geometry limits.
fn emit(
    id: u64,
    definitions: &BTreeMap<u64, Definition>,
    placements: &mut Vec<Placement>,
    budget: &mut Budget,
    scene: &mut Scene,
) -> Result<()> {
    if placements.len() > MAX_DEPTH {
        return Err("AMF output nesting exceeds the depth limit".into());
    }
    budget.work(1)?;
    match definitions.get(&id).ok_or("missing AMF definition")? {
        Definition::Object(mesh) => {
            for face in &mesh.faces {
                budget.output(face.color.is_some())?;
                budget.work(3 * (1 + placements.len() * 3))?;
                let mut vertices = face.indices.map(|index| mesh.positions[index]);
                for vertex in &mut vertices {
                    for &placement in placements.iter().rev() {
                        *vertex = placed(*vertex, placement);
                    }
                    finite(*vertex)?;
                }
                let [a, b, c] = vertices;
                let area = (b - a).cross(c - a).length();
                if !area.is_finite() || area <= 0.0 {
                    return Err("AMF placement collapses or overflows a triangle".into());
                }
                let index = scene.triangles.len();
                scene.triangles.push(Triangle {
                    vertices,
                    color: None,
                });
                if let Some(color) = face.color {
                    scene.appearances.insert(
                        index,
                        TriangleAppearance {
                            base_color: linear(color),
                            alpha_mode: if color[3] < 1.0 {
                                AlphaMode::Blend
                            } else {
                                AlphaMode::Opaque
                            },
                            ..TriangleAppearance::default()
                        },
                    );
                }
            }
        }
        Definition::Constellation(instances) => {
            for instance in instances {
                placements.push(instance.placement);
                emit(instance.target, definitions, placements, budget, scene)?;
                placements.pop();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_limits_refuse_before_retained_collection_growth() {
        let mut vertices = Budget {
            vertices: MAX_GEOMETRY,
            ..Budget::default()
        };
        assert!(vertices.vertex().is_err());
        assert_eq!(vertices.bytes, 0);
        let mut faces = Budget {
            source_faces: MAX_GEOMETRY,
            ..Budget::default()
        };
        assert!(faces.face().is_err());
        assert_eq!(faces.bytes, 0);
        let mut output = Budget {
            output_faces: MAX_GEOMETRY,
            ..Budget::default()
        };
        assert!(output.output(true).is_err());
        assert_eq!(output.bytes, 0);
        let mut records = Budget {
            records: MAX_RECORDS,
            ..Budget::default()
        };
        assert!(records.record().is_err());
        assert_eq!(records.bytes, 0);
    }

    #[test]
    fn retained_and_expansion_work_limits_check_overflow_and_remaining_capacity() {
        let mut retained = Budget {
            bytes: MAX_RETAINED,
            ..Budget::default()
        };
        assert!(retained.retain(1).is_err());
        let mut overflow = Budget {
            bytes: usize::MAX,
            ..Budget::default()
        };
        assert!(
            overflow
                .retain(1)
                .unwrap_err()
                .to_string()
                .contains("overflow")
        );
        let mut work = Budget {
            work: MAX_WORK,
            ..Budget::default()
        };
        assert!(work.work(1).is_err());
        let mut overflow = Budget {
            work: usize::MAX,
            ..Budget::default()
        };
        assert!(
            overflow
                .work(1)
                .unwrap_err()
                .to_string()
                .contains("overflow")
        );
    }

    #[test]
    fn appearance_tree_reservations_are_admitted_before_colored_output() {
        let triangle = 2 * std::mem::size_of::<Triangle>();
        let slot = std::mem::size_of::<(usize, TriangleAppearance)>() + 32;
        let mut plain = Budget {
            bytes: MAX_RETAINED - triangle,
            ..Budget::default()
        };
        plain.output(false).unwrap();
        assert_eq!(plain.bytes, MAX_RETAINED);
        assert!(!plain.appearance_root);

        let mut colored = Budget {
            bytes: MAX_RETAINED - triangle - 15 * slot + 1,
            ..Budget::default()
        };
        assert!(colored.output(true).is_err());
        assert!(!colored.appearance_root);

        let mut admitted = Budget::default();
        admitted.output(true).unwrap();
        assert_eq!(admitted.bytes, triangle + 15 * slot);
        assert!(admitted.appearance_root);
        admitted.output(true).unwrap();
        assert_eq!(admitted.bytes, 2 * triangle + 18 * slot);
    }

    #[test]
    fn xml_size_is_rejected_before_parsing_or_geometry_allocation() {
        let oversized = vec![b' '; MAX_BYTES + 1];
        assert!(
            parse(&oversized)
                .unwrap_err()
                .to_string()
                .contains("64 MiB")
        );
    }
}
