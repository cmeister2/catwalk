//! Stored SketchUp half-edge surfaces, wires and component placements.
//!
//! Format facts follow the OpenSKP format specification and retail producer
//! files; this independent reader does not incorporate its GPL parser code.

use super::{citygml::triangulate_rings, transform::Transform};
use crate::{
    Result,
    scene::{AlphaMode, Line, Scene, Triangle, TriangleAppearance, Vec3},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
    path::Path,
};

/// Maximum decompressed model or material member size.
const MAX_MEMBER: usize = 64 * 1024 * 1024;
/// Maximum records and emitted primitives per document.
const MAX_ITEMS: usize = 1_000_000;

/// Read the native model without selecting any saved image.
///
/// # Errors
/// Returns an error for unsupported versions, invalid native records,
/// unresolved topology or placements, failed checksums and exceeded budgets.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let data = super::read_file(path)?;
    let mut source = Reader { bytes: &data };
    if source.string()? != "SketchUp Model" {
        return Err("input is not a SketchUp model".into());
    }
    let version = source.string()?;
    let version = version
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or("invalid SketchUp version")?;
    let major = version
        .split('.')
        .next()
        .ok_or("missing SketchUp version")?
        .parse::<u32>()?;
    if (13..=17).contains(&major) {
        return super::sketchup_legacy::load(&data);
    }
    if major != 26 {
        return Err("unsupported SketchUp geometry version (expected 13–17 or 26)".into());
    }
    if source.take(9)? != b"VFF\x08\x00\x01\x00\x11\x00" {
        return Err("unsupported SketchUp VFF prefix".into());
    }
    source.take(4)?;
    modern(source.bytes, version)
}

/// Forward-only bounded reader shared by document identity and record framing.
struct Reader<'a> {
    /// Unconsumed bytes.
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    /// Consume an addressed range, refusing truncation.
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let (head, tail) = self
            .bytes
            .split_at_checked(length)
            .ok_or("truncated SketchUp geometry")?;
        self.bytes = tail;
        Ok(head)
    }
    /// Read an unsigned little-endian word.
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    /// Read an unsigned little-endian double word.
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    /// Decode a bounded MFC Unicode identity string.
    fn string(&mut self) -> Result<String> {
        if self.take(3)? != b"\xff\xfe\xff" {
            return Err("unsupported SketchUp identity string".into());
        }
        let length = usize::from(self.take(1)?[0]);
        if length > 128 {
            return Err("SketchUp identity string exceeds limit".into());
        }
        let units = self
            .take(length * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>();
        Ok(String::from_utf16(&units)?)
    }
}

/// Borrowed children of one TLV record container.
struct Fields<'a>(Vec<(u16, &'a [u8])>);

impl<'a> Fields<'a> {
    /// Get a single field while rejecting ambiguous duplicates.
    fn get(&self, tag: u16) -> Result<Option<&'a [u8]>> {
        let mut values = self.0.iter().filter(|(t, _)| *t == tag);
        let result = values.next().map(|(_, b)| *b);
        if values.next().is_some() {
            return Err("duplicate SketchUp record field".into());
        }
        Ok(result)
    }
    /// Require exactly one addressed child.
    fn required(&self, tag: u16) -> Result<&'a [u8]> {
        self.get(tag)?
            .ok_or_else(|| format!("missing SketchUp record {tag:#x}").into())
    }
    /// Require all records in a typed pool to have the expected tag.
    fn items(&self, tag: u16) -> Result<Vec<&'a [u8]>> {
        let mut items = Vec::new();
        for &(actual, bytes) in &self.0 {
            if actual == tag {
                items.push(bytes);
            } else if actual != 0x63 {
                return Err(format!("unsupported SketchUp pool record {actual:#x}").into());
            }
        }
        Ok(items)
    }
}

/// Cumulative record, archive expansion and output limits.
#[derive(Default)]
struct Budget {
    /// Records parsed, including repeated borrowed field reads.
    records: usize,
    /// Selected decompressed archive bytes.
    expanded: usize,
}

impl Budget {
    /// Parse one level without guessing whether leaf payloads are containers.
    fn fields<'a>(&mut self, bytes: &'a [u8]) -> Result<Fields<'a>> {
        let mut source = Reader { bytes };
        let mut result = Vec::new();
        while !source.bytes.is_empty() {
            self.records += 1;
            if self.records > MAX_ITEMS {
                return Err("SketchUp record limit exceeded".into());
            }
            let tag = source.u16()?;
            let length = usize::try_from(source.u32()?)?;
            result.push((tag, source.take(length)?));
        }
        Ok(Fields(result))
    }
    /// Decode only an explicitly selected ZIP member and verify its CRC.
    fn member(
        &mut self,
        archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
        name: &str,
    ) -> Result<Vec<u8>> {
        let mut entry = archive.by_name(name)?;
        if entry.is_dir()
            || entry.encrypted()
            || entry.size() > MAX_MEMBER as u64
            || entry.compressed_size() > MAX_MEMBER as u64
            || !matches!(
                entry.compression(),
                zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
            )
        {
            return Err("unsupported or oversized SketchUp geometry member".into());
        }
        self.expanded = self
            .expanded
            .checked_add(usize::try_from(entry.size())?)
            .ok_or("SketchUp expansion size overflow")?;
        if self.expanded as u64 > super::MAX_INPUT_BYTES {
            return Err("SketchUp selected expansion limit exceeded".into());
        }
        let expected = usize::try_from(entry.size())?;
        let mut bytes = Vec::new();
        entry
            .by_ref()
            .take(MAX_MEMBER as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() != expected {
            return Err("SketchUp geometry member size mismatch".into());
        }
        Ok(bytes)
    }
}

/// Decode a finite, exact-length sequence of little-endian doubles.
fn numbers<const N: usize>(bytes: &[u8]) -> Result<[f64; N]> {
    if bytes.len() != N * 8 {
        return Err("invalid SketchUp numeric record length".into());
    }
    let mut result = [0.; N];
    for (value, bytes) in result.iter_mut().zip(bytes.as_chunks::<8>().0) {
        *value = f64::from_le_bytes(*bytes);
        if !value.is_finite() {
            return Err("non-finite SketchUp coordinate".into());
        }
    }
    Ok(result)
}

/// Decode a nonzero variable-width persistent identifier.
fn identifier(bytes: &[u8]) -> Result<u64> {
    if bytes.is_empty() || bytes.len() > 8 {
        return Err("invalid SketchUp entity identifier".into());
    }
    let mut result = [0; 8];
    result[..bytes.len()].copy_from_slice(bytes);
    let value = u64::from_le_bytes(result);
    if value == 0 {
        return Err("null SketchUp entity identifier".into());
    }
    Ok(value)
}

/// Decode an optional single flags byte.
fn flag(bytes: Option<&[u8]>) -> Result<u8> {
    match bytes {
        None => Ok(0),
        Some([value]) => Ok(*value),
        _ => Err("invalid SketchUp flags field".into()),
    }
}

/// Common drawing-element properties.
#[derive(Clone, Copy, Default)]
struct Drawing {
    /// Persistent identifier; the root model has none.
    id: Option<u64>,
    /// Hidden entity flag.
    hidden: bool,
    /// Owning layer identifier.
    layer: Option<u64>,
    /// Front material identifier.
    material: Option<u64>,
}

/// Extract a persistent id from an entity-base record.
fn entity_id(bytes: &[u8], budget: &mut Budget) -> Result<Option<u64>> {
    budget
        .fields(bytes)?
        .get(0x05de)?
        .map(identifier)
        .transpose()
}

/// Decode the drawing header without descending into unrelated attributes.
fn drawing(bytes: &[u8], budget: &mut Budget) -> Result<Drawing> {
    let fields = budget.fields(bytes)?;
    Ok(Drawing {
        id: entity_id(fields.required(0x05dc)?, budget)?,
        hidden: flag(fields.get(0x07d3)?)? & 1 != 0,
        layer: fields.get(0x07d2)?.map(identifier).transpose()?,
        material: fields.get(0x07d1)?.map(identifier).transpose()?,
    })
}

/// Decode an archive whose first byte is the addressed ZIP start.
fn modern(bytes: &[u8], version: &str) -> Result<Scene> {
    let physical =
        super::zip_directory::validate(&mut Cursor::new(bytes), bytes.len() as u64, 4096)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    if archive.len() != physical {
        return Err("ambiguous SketchUp archive members".into());
    }
    let mut names = BTreeSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index)?;
        let name = entry.name()?;
        let name = name.trim_end_matches('/');
        if name.is_empty()
            || name.contains(['\\', '\0'])
            || name.split('/').any(|s| matches!(s, "" | "." | ".."))
            || !names.insert(name.to_owned())
        {
            return Err("invalid or duplicate SketchUp archive path".into());
        }
    }
    let mut budget = Budget::default();
    let meta = budget.member(&mut archive, "meta/meta.dat")?;
    let metadata = budget.fields(&meta)?;
    let metadata = budget.fields(metadata.required(0x64)?)?;
    if metadata.required(0x75)? != version.as_bytes() || metadata.required(0x76)? != [26, 0] {
        return Err("inconsistent SketchUp model version".into());
    }
    let model = budget.member(&mut archive, "model.dat")?;
    let root = budget.fields(&model)?;
    let root = budget.fields(root.required(0x01f4)?)?;
    let materials = read_materials(&root, &mut archive, &mut budget)?;
    let layers = read_layers(&root, &mut budget)?;
    let mut definitions = BTreeMap::new();
    if let Some(bytes) = root.get(0x01f9)? {
        let outer = budget.fields(bytes)?;
        let manager = budget.fields(outer.required(0x1770)?)?;
        let pool = budget.fields(manager.required(0x1771)?)?;
        for bytes in pool.items(0x157c)? {
            let fields = budget.fields(bytes)?;
            let geometry = container(fields.required(0x1388)?, &mut budget)?;
            let id = geometry
                .drawing
                .id
                .ok_or("SketchUp definition has no identifier")?;
            let behaviour = fields.get(0x1b58)?.map(|b| budget.fields(b)).transpose()?;
            let billboard = behaviour
                .as_ref()
                .map(|b| flag(b.get(0x1b5d)?))
                .transpose()?
                .unwrap_or(0)
                != 0;
            if definitions.insert(id, (geometry, billboard)).is_some() {
                return Err("duplicate SketchUp definition identifier".into());
            }
        }
    }
    let own = budget.fields(root.required(0x01f6)?)?;
    let own = container(own.required(0x1388)?, &mut budget)?;
    let mut renderer = Model {
        definitions,
        materials,
        layers: layers.states,
        default_layer: layers.default,
        active_layer: layers.active,
        scene: Scene::default(),
        stack: BTreeSet::new(),
        visits: 0,
    };
    renderer.emit(&own, Transform::IDENTITY, None, 0)?;
    Ok(renderer.scene)
}

/// A supported diffuse material, represented in linear color.
#[derive(Clone, Copy)]
struct Material {
    /// Linear diffuse RGBA multiplier.
    rgba: [f64; 4],
}

/// Load exact material documents addressed by the native model's material ids.
fn read_materials(
    root: &Fields<'_>,
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    budget: &mut Budget,
) -> Result<BTreeMap<u64, Material>> {
    let mut result = BTreeMap::new();
    let Some(bytes) = root.get(0x01f7)? else {
        return Ok(result);
    };
    let outer = budget.fields(bytes)?;
    let manager = budget.fields(outer.required(0x30d4)?)?;
    let pool = budget.fields(manager.required(0x30d5)?)?;
    for bytes in pool.items(0x32c8)? {
        let fields = budget.fields(bytes)?;
        let id =
            entity_id(fields.required(0x05dc)?, budget)?.ok_or("SketchUp material has no id")?;
        let name = std::str::from_utf8(fields.required(0x32cc)?)?;
        if name.is_empty() || name.contains(['/', '\\', '\0']) || matches!(name, "." | "..") {
            return Err("invalid SketchUp material folder".into());
        }
        let data = budget.member(archive, &format!("materials/{name}/material.xml"))?;
        let doc = roxmltree::Document::parse(std::str::from_utf8(&data)?)?;
        let mut material_nodes = doc
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "material");
        let node = material_nodes
            .next()
            .ok_or("invalid SketchUp material document")?;
        if material_nodes.next().is_some() {
            return Err("ambiguous SketchUp material document".into());
        }
        let mut rgba = [1.; 4];
        for (index, attribute) in ["colorRed", "colorGreen", "colorBlue"].iter().enumerate() {
            let value = node
                .attribute(*attribute)
                .ok_or("missing SketchUp material color")?
                .parse::<u8>()?;
            let value = f64::from(value) / 255.;
            rgba[index] = if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            };
        }
        if node.attribute("useTrans") == Some("1") {
            let transparency = node
                .attribute("trans")
                .ok_or("missing SketchUp transparency")?
                .parse::<f64>()?;
            if !transparency.is_finite() || !(0. ..=1.).contains(&transparency) {
                return Err("invalid SketchUp material transparency".into());
            }
            rgba[3] = 1. - transparency;
        }
        if result.insert(id, Material { rgba }).is_some() {
            return Err("duplicate SketchUp material identifier".into());
        }
    }
    Ok(result)
}

/// Native layer order, visibility and selected drawing layer.
#[derive(Default)]
struct Layers {
    /// Hidden state keyed by persistent layer id.
    states: BTreeMap<u64, bool>,
    /// The first native manager layer is the implicit default tag.
    default: Option<u64>,
    /// The current drawing layer remains displayed even when flagged hidden.
    active: Option<u64>,
}

/// Read default/current layers independently of each layer's color material.
fn read_layers(root: &Fields<'_>, budget: &mut Budget) -> Result<Layers> {
    let mut result = BTreeMap::new();
    let Some(bytes) = root.get(0x01f8)? else {
        return Ok(Layers::default());
    };
    let outer = budget.fields(bytes)?;
    let manager = budget.fields(outer.required(0x3a98)?)?;
    let pool = budget.fields(manager.required(0x3a99)?)?;
    let mut default = None;
    for bytes in pool.items(0x3c8c)? {
        let fields = budget.fields(bytes)?;
        let id = entity_id(fields.required(0x05dc)?, budget)?.ok_or("SketchUp layer has no id")?;
        default.get_or_insert(id);
        if result.insert(id, flag(fields.get(0x3c8e)?)? != 0).is_some() {
            return Err("duplicate SketchUp layer id".into());
        }
    }
    let active = manager
        .get(0x3a9a)?
        .map(identifier)
        .transpose()?
        .or(default);
    if active.is_some_and(|id| !result.contains_key(&id)) {
        return Err("unresolved SketchUp current layer".into());
    }
    Ok(Layers {
        states: result,
        default,
        active,
    })
}

/// Indexed native edge with drawing properties.
struct Edge {
    /// Visibility, material and persistent id.
    drawing: Drawing,
    /// Start/end vertex identifiers.
    vertices: [u64; 2],
}

/// Native face rings assembled from oriented half-edge references.
struct Face {
    /// Visibility and front material.
    drawing: Drawing,
    /// Native normal used to preserve outward winding after triangulation.
    normal: Vec3,
    /// Native plane offset in inches, retaining the authored boundary constraint.
    offset: f64,
    /// Closed edge cycles; each pair holds an edge id and orientation.
    rings: Vec<Vec<(u64, bool)>>,
}

/// One placement of a shared definition.
struct Instance {
    /// Visibility and inherited paint material.
    drawing: Drawing,
    /// Referenced native definition id.
    definition: u64,
    /// Local column-vector affine transform.
    transform: Transform,
}

/// Stored geometry belonging to the model or one shared definition.
#[derive(Default)]
struct Container {
    /// Container visibility and definition id.
    drawing: Drawing,
    /// Native vertex pool.
    vertices: BTreeMap<u64, Vec3>,
    /// Native edge pool.
    edges: BTreeMap<u64, Edge>,
    /// Native surface faces.
    faces: Vec<Face>,
    /// Child placements of groups/components.
    instances: Vec<Instance>,
    /// Stored 3D polyline points.
    polylines: Vec<(Drawing, Vec<Vec3>)>,
    /// Visible image geometry unsupported by this static surface reader.
    images: bool,
    /// A native active section plane requires clipping.
    section: bool,
    /// Drawing metadata of unsupported annotations, checked only when selected.
    annotations: Vec<Drawing>,
}

/// Decode all typed surface/wire/placement pools of one entity container.
fn container(bytes: &[u8], budget: &mut Budget) -> Result<Container> {
    let fields = budget.fields(bytes)?;
    for &(tag, _) in &fields.0 {
        if !matches!(tag, 0x63 | 0x07d0 | 0x1389..=0x139b | 0x139e..=0x13a0) {
            return Err(format!("unsupported SketchUp entity-container field {tag:#x}").into());
        }
    }
    let mut result = Container {
        drawing: drawing(fields.required(0x07d0)?, budget)?,
        ..Container::default()
    };
    if let Some(bytes) = fields.get(0x1389)? {
        for bytes in budget.fields(bytes)?.items(0x09c4)? {
            let f = budget.fields(bytes)?;
            let id = entity_id(f.required(0x05dc)?, budget)?.ok_or("SketchUp vertex has no id")?;
            let [x, y, z] = numbers(f.required(0x09c5)?)?;
            if result.vertices.insert(id, Vec3::new(x, y, z)).is_some() {
                return Err("duplicate SketchUp vertex id".into());
            }
        }
    }
    if let Some(bytes) = fields.get(0x138a)? {
        for bytes in budget.fields(bytes)?.items(0x0bb8)? {
            let f = budget.fields(bytes)?;
            let drawing = drawing(f.required(0x07d0)?, budget)?;
            let id = drawing.id.ok_or("SketchUp edge has no id")?;
            let vertices = [
                identifier(f.required(0x0bb9)?)?,
                identifier(f.required(0x0bba)?)?,
            ];
            if result
                .edges
                .insert(id, Edge { drawing, vertices })
                .is_some()
            {
                return Err("duplicate SketchUp edge id".into());
            }
        }
    }
    if let Some(bytes) = fields.get(0x138b)? {
        for bytes in budget.fields(bytes)?.items(0x0dac)? {
            result.faces.push(face(bytes, budget)?);
        }
    }
    for (pool, wrapper) in [(0x138c, 0x1964), (0x138d, 0x1d4c)] {
        if let Some(bytes) = fields.get(pool)? {
            for bytes in budget.fields(bytes)?.items(wrapper)? {
                let bytes = if wrapper == 0x1964 {
                    bytes
                } else {
                    budget.fields(bytes)?.required(0x1964)?
                };
                let f = budget.fields(bytes)?;
                let values: [f64; 13] = numbers(f.required(0x1966)?)?;
                if values[12].to_bits() != 1_f64.to_bits() {
                    return Err("unsupported SketchUp homogeneous transform".into());
                }
                let mut matrix = Transform::IDENTITY.0;
                for row in 0..3 {
                    for column in 0..3 {
                        matrix[column][row] = values[row * 3 + column];
                    }
                }
                matrix[3][..3].copy_from_slice(&values[9..12]);
                result.instances.push(Instance {
                    drawing: drawing(f.required(0x07d0)?, budget)?,
                    definition: identifier(f.required(0x1967)?)?,
                    transform: Transform(matrix),
                });
            }
        }
    }
    if let Some(bytes) = fields.get(0x1395)? {
        for bytes in budget.fields(bytes)?.items(0x4844)? {
            let f = budget.fields(bytes)?;
            let points = f.required(0x4845)?;
            if points.len() % 24 != 0 {
                return Err("invalid SketchUp 3D polyline".into());
            }
            let points = points
                .as_chunks::<24>()
                .0
                .iter()
                .map(|b| numbers(b).map(|[x, y, z]| Vec3::new(x, y, z)))
                .collect::<Result<Vec<_>>>()?;
            result
                .polylines
                .push((drawing(f.required(0x07d0)?, budget)?, points));
        }
    }
    result.images = fields.get(0x1390)?.is_some_and(|b| !b.is_empty());
    result.section = fields.get(0x1394)?.is_some();
    result.annotations = annotation_drawings(&fields, budget)?;
    Ok(result)
}

/// Retain annotation visibility metadata without parsing unsupported drawing carriers.
fn annotation_drawings(fields: &Fields<'_>, budget: &mut Budget) -> Result<Vec<Drawing>> {
    let mut annotations = Vec::new();
    for (pool, wrapper, base) in [
        (0x1398, 0x55f0, None),
        (0x1399, 0x5bcc, Some(0x59d8)),
        (0x139a, 0x5dc0, Some(0x59d8)),
    ] {
        if let Some(bytes) = fields.get(pool)? {
            for bytes in budget.fields(bytes)?.items(wrapper)? {
                let fields = budget.fields(bytes)?;
                let drawing_fields = base.map_or_else(
                    || fields.required(0x07d0),
                    |base| budget.fields(fields.required(base)?)?.required(0x07d0),
                )?;
                annotations.push(drawing(drawing_fields, budget)?);
            }
        }
    }
    Ok(annotations)
}

/// Decode native face plane and every ordered hole boundary.
fn face(bytes: &[u8], budget: &mut Budget) -> Result<Face> {
    let f = budget.fields(bytes)?;
    let [x, y, z, offset] = numbers(f.required(0x0dad)?)?;
    let normal = Vec3::new(x, y, z);
    if (normal.length() - 1.).abs() > 1e-6 {
        return Err("invalid SketchUp face normal".into());
    }
    let mut rings = Vec::new();
    for bytes in budget.fields(f.required(0x0dae)?)?.items(0x1194)? {
        let f = budget.fields(bytes)?;
        let mut ring = Vec::new();
        for bytes in budget.fields(f.required(0x1195)?)?.items(0x0fa0)? {
            let f = budget.fields(bytes)?;
            let direction = flag(f.get(0x0fa2)?)?;
            if direction > 1 {
                return Err("invalid SketchUp edge-use orientation".into());
            }
            ring.push((identifier(f.required(0x0fa1)?)?, direction != 0));
        }
        if ring.len() < 3 || ring.len() > 100_000 {
            return Err("invalid SketchUp face ring size".into());
        }
        rings.push(ring);
    }
    if rings.is_empty() {
        return Err("SketchUp face has no boundary".into());
    }
    Ok(Face {
        drawing: drawing(f.required(0x07d0)?, budget)?,
        normal,
        offset,
        rings,
    })
}

/// Shared definitions and visibility/material state during bounded scene expansion.
struct Model {
    /// Shared definition geometry and camera-facing behavior.
    definitions: BTreeMap<u64, (Container, bool)>,
    /// Native diffuse materials.
    materials: BTreeMap<u64, Material>,
    /// Layer hidden state keyed by native persistent id.
    layers: BTreeMap<u64, bool>,
    /// Implicit tag used by actual drawing entities without an explicit layer.
    default_layer: Option<u64>,
    /// Current drawing tag, displayed even when its stored hidden flag is set.
    active_layer: Option<u64>,
    /// Output primitives.
    scene: Scene,
    /// Active definition expansion path for cycle refusal.
    stack: BTreeSet<u64>,
    /// Total expanded container visits, including empty placements.
    visits: usize,
}

impl Model {
    /// Check both entity and layer visibility, refusing unresolved layers.
    fn visible(&self, drawing: Drawing) -> Result<bool> {
        let hidden_layer = drawing
            .layer
            .or(self.default_layer)
            .map(|id| {
                self.layers
                    .get(&id)
                    .map(|&hidden| hidden && Some(id) != self.active_layer)
                    .ok_or("unresolved SketchUp layer")
            })
            .transpose()?
            .unwrap_or(false);
        Ok(!drawing.hidden && !hidden_layer)
    }
    /// Expand the referenced model tree under its authored affine placements.
    #[allow(
        clippy::too_many_lines,
        reason = "Keep native topology checks and its bounded surface/wire expansion together"
    )]
    fn emit(
        &mut self,
        container: &Container,
        transform: Transform,
        inherited: Option<u64>,
        depth: usize,
    ) -> Result<()> {
        self.visits += 1;
        if self.visits > MAX_ITEMS {
            return Err("SketchUp instance expansion exceeds work limit".into());
        }
        if depth > 128 {
            return Err("SketchUp instance nesting exceeds limit".into());
        }
        // Root/definition containers store geometry; they are not placed
        // drawing entities on the implicit default tag. Actual faces, edges
        // and placing instances resolve their own layer visibility below.
        if container.section {
            return Err("SketchUp active section clipping is unsupported".into());
        }
        if container.images {
            return Err("SketchUp placed-image geometry is unsupported".into());
        }
        for &drawing in &container.annotations {
            if self.visible(drawing)? {
                return Err("SketchUp visible text/dimension geometry is unsupported".into());
            }
        }
        let mut used = BTreeSet::new();
        for face in &container.faces {
            if !self.visible(face.drawing)? {
                continue;
            }
            for ring in &face.rings {
                for &(id, _) in ring {
                    used.insert(id);
                }
            }
            let mut rings = Vec::new();
            for ring in &face.rings {
                let mut vertices = Vec::new();
                let mut ends = Vec::new();
                for &(id, forward) in ring {
                    let edge = container
                        .edges
                        .get(&id)
                        .ok_or("unresolved SketchUp half-edge")?;
                    let [start, end] = if forward {
                        edge.vertices
                    } else {
                        [edge.vertices[1], edge.vertices[0]]
                    };
                    vertices.push(
                        *container
                            .vertices
                            .get(&start)
                            .ok_or("unresolved SketchUp face vertex")?,
                    );
                    ends.push((start, end));
                }
                // Modern files enumerate adjacent oriented uses in the reverse
                // order of their directed edges; verify the native closed cycle.
                if !(0..ends.len()).all(|i| ends[i].0 == ends[(i + 1) % ends.len()].1) {
                    return Err("SketchUp face boundary is not a closed edge cycle".into());
                }
                rings.push(vertices);
            }
            let origin = rings[0][0];
            let extent = rings
                .iter()
                .flatten()
                .map(|&point| (point - origin).length())
                .fold(0_f64, f64::max);
            let tolerance = extent.max(1.) * 1e-7 + origin.length() * 1e-12;
            for point in rings.iter().flatten() {
                let residual = point.dot(face.normal) + face.offset;
                if !residual.is_finite() || residual.abs() > tolerance {
                    return Err("SketchUp face boundary does not lie in its native plane".into());
                }
            }
            let material = face
                .drawing
                .material
                .or(inherited)
                .map(|id| {
                    self.materials
                        .get(&id)
                        .copied()
                        .ok_or("unresolved SketchUp material")
                })
                .transpose()?;
            for mut vertices in triangulate_rings(&rings)? {
                if (vertices[1] - vertices[0])
                    .cross(vertices[2] - vertices[0])
                    .dot(face.normal)
                    < 0.
                {
                    vertices.swap(1, 2);
                }
                let columns: [Vec3; 3] = std::array::from_fn(|i| {
                    Vec3::new(transform.0[i][0], transform.0[i][1], transform.0[i][2])
                });
                if columns[0].cross(columns[1]).dot(columns[2]) < 0. {
                    vertices.swap(1, 2);
                }
                vertices = vertices.map(|v| transform.apply(v) * 25.4);
                self.scene.triangles.push(Triangle {
                    vertices,
                    color: None,
                });
                if let Some(material) = material {
                    self.scene.appearances.insert(
                        self.scene.triangles.len() - 1,
                        TriangleAppearance {
                            base_color: material.rgba,
                            alpha_mode: if material.rgba[3] < 1. {
                                AlphaMode::Blend
                            } else {
                                AlphaMode::Opaque
                            },
                            ..TriangleAppearance::default()
                        },
                    );
                }
                self.check_output()?;
            }
        }
        for (&id, edge) in &container.edges {
            if used.contains(&id) || !self.visible(edge.drawing)? {
                continue;
            }
            let start = *container
                .vertices
                .get(&edge.vertices[0])
                .ok_or("unresolved SketchUp wire vertex")?;
            let end = *container
                .vertices
                .get(&edge.vertices[1])
                .ok_or("unresolved SketchUp wire vertex")?;
            self.scene.lines.push(Line {
                vertices: [transform.apply(start) * 25.4, transform.apply(end) * 25.4],
                color: None,
            });
            self.check_output()?;
        }
        for (drawing, points) in &container.polylines {
            if !self.visible(*drawing)? {
                continue;
            }
            for pair in points.windows(2) {
                self.scene.lines.push(Line {
                    vertices: [
                        transform.apply(pair[0]) * 25.4,
                        transform.apply(pair[1]) * 25.4,
                    ],
                    color: None,
                });
                self.check_output()?;
            }
        }
        for instance in &container.instances {
            if !self.visible(instance.drawing)? {
                continue;
            }
            let id = instance.definition;
            if !self.stack.insert(id) {
                return Err("cyclic SketchUp definition placement".into());
            }
            // Temporarily take the shared definition to avoid copying large
            // native pools while mutating the output and recursion stack.
            let (definition, billboard) = self
                .definitions
                .remove(&id)
                .ok_or("unresolved SketchUp component definition")?;
            if billboard {
                return Err("SketchUp camera-facing component is unsupported".into());
            }
            self.emit(
                &definition,
                transform.then(instance.transform),
                instance.drawing.material.or(inherited),
                depth + 1,
            )?;
            self.definitions.insert(id, (definition, billboard));
            self.stack.remove(&id);
        }
        Ok(())
    }
    /// Bound expansion independently of the native source pool sizes.
    fn check_output(&self) -> Result<()> {
        if self.scene.triangles.len() + self.scene.lines.len() > MAX_ITEMS {
            return Err("SketchUp expanded geometry exceeds primitive limit".into());
        }
        Ok(())
    }
}
