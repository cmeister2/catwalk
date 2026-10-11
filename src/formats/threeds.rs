//! Static 3DS triangle meshes and the earliest keys of object hierarchies.
//!
//! Format conventions were checked against the primary Assimp and Blender importers:
//! <https://github.com/assimp/assimp/tree/master/code/AssetLib/3DS>
//! <https://github.com/blender/blender-addons/blob/main/io_scene_3ds/import_3ds.py>
//! Editor vertices are already in world space; keyframer nodes require undoing
//! the mesh matrix before applying their pivot and hierarchy transforms.

use super::{read_file, transform::Transform};
use crate::{
    Result,
    scene::{Scene, Triangle, Vec3},
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

/// Maximum aggregate 3DS vertices or faces accepted before allocation.
const MAX_ELEMENTS: usize = 5_000_000;
/// Maximum parsed objects or hierarchy nodes accepted.
const MAX_OBJECTS: usize = 65_535;

/// Bounds-checked cursor for 3DS mesh and keyframer hierarchy records.
struct Reader<'a> {
    /// Borrowed complete input byte stream.
    bytes: &'a [u8],
    /// Byte position of the next unread record.
    offset: usize,
}

impl<'a> Reader<'a> {
    /// Start a bounds-checked reader at the beginning of the supplied bytes.
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    /// Return the number of unread bytes in this chunk.
    fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }
    /// Consume the requested bytes, rejecting offset overflow and truncated records.
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or("3DS field size overflow")?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or("truncated 3DS field")?;
        self.offset = end;
        Ok(bytes)
    }
    /// Read one unsigned 16-bit word from the current record.
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    /// Read one unsigned 32-bit word from the current record.
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    /// Read a finite little-endian 3DS single-precision value.
    fn float(&mut self) -> Result<f64> {
        let value = f64::from(f32::from_le_bytes(self.take(4)?.try_into()?));
        if !value.is_finite() {
            return Err("3DS contains a non-finite number".into());
        }
        Ok(value)
    }
    /// Read three consecutive finite components into a geometry vector.
    fn vector(&mut self) -> Result<Vec3> {
        Ok(Vec3::new(self.float()?, self.float()?, self.float()?))
    }
    /// Read a bounded NUL-terminated chunk string.
    fn string(&mut self) -> Result<String> {
        let count = self.bytes[self.offset..]
            .iter()
            .position(|&byte| byte == 0)
            .ok_or("unterminated 3DS name")?;
        if count > u16::MAX as usize {
            return Err("3DS name is too long".into());
        }
        // Legacy 3DS names need not be UTF-8. Latin-1 preserves every byte and
        // keeps material and object references unambiguous.
        let value = self
            .take(count)?
            .iter()
            .map(|&byte| char::from(byte))
            .collect();
        self.take(1)?;
        Ok(value)
    }
    /// Read one child chunk header and return its checked payload.
    fn chunk(&mut self) -> Result<Option<(u16, &'a [u8])>> {
        if self.remaining() == 0 {
            return Ok(None);
        }
        let id = self.u16()?;
        let length = self.u32()? as usize;
        if length < 6 {
            return Err("3DS chunk length is smaller than its header".into());
        }
        Ok(Some((id, self.take(length - 6)?)))
    }
    /// Reject bytes remaining after the expected chunk payload.
    fn finish(self) -> Result<()> {
        if self.remaining() != 0 {
            return Err("3DS field has unexpected trailing bytes".into());
        }
        Ok(())
    }
}

/// 3DS editor mesh, material groups and stored placement matrix.
struct Mesh {
    /// Editor object name used by keyframer instances.
    name: String,
    /// Stored vertex coordinates before hierarchy placement.
    vertices: Vec<Vec3>,
    /// Triangle indices into the mesh vertex array.
    faces: Vec<[u16; 3]>,
    /// Named material assignments and their face indices.
    material_groups: Vec<(String, Vec<u16>)>,
    /// Stored editor mesh matrix used to recover hierarchy-local geometry.
    matrix: Transform,
    /// Whether the editor marks this object as hidden.
    hidden: bool,
}

/// 3DS keyframer node with its earliest static transform.
struct Node {
    /// Stored keyframer node chunk kind.
    kind: u16,
    /// Explicit hierarchy identifier or its ordinal fallback.
    id: u16,
    /// Referenced editor object name.
    name: String,
    /// Optional parent node identifier.
    parent: Option<u16>,
    /// Local pivot subtracted before node placement.
    pivot: Vec3,
    /// Earliest position-track value.
    position: Vec3,
    /// Earliest composed clockwise axis-angle rotation.
    rotation: Transform,
    /// Earliest per-axis scale-track value.
    scale: Vec3,
}

impl Node {
    /// Compose a node position, pivot, clockwise rotation and scale into its local transform.
    fn local(&self) -> Transform {
        translation(self.position)
            .then(self.rotation)
            .then(scaling(self.scale))
    }
}

#[derive(Default)]
/// 3DS editor resources and keyframer nodes before scene instantiation.
struct Model {
    /// Decoded editor triangle meshes.
    meshes: Vec<Mesh>,
    /// Names already declared by editor objects.
    object_names: HashSet<String>,
    /// Named diffuse material colors, including materials without a color.
    materials: HashMap<String, Option<[u8; 3]>>,
    /// Decoded static keyframer hierarchy.
    nodes: Vec<Node>,
    /// Aggregate editor vertex count for allocation limits.
    vertices: usize,
    /// Aggregate editor face count for allocation limits.
    faces: usize,
    /// Optional scene-wide editor scale multiplier.
    master_scale: Option<f64>,
}

/// Read a bounded 3DS mesh and keyframer hierarchy and decode its preview geometry.
///
/// # Errors
/// Returns an error for malformed chunks, unsupported geometry/tracks, invalid
/// indices/hierarchy/transforms or resource limits.
pub(super) fn load(path: &Path) -> Result<Scene> {
    parse(&read_file(path)?)
}

/// Validate the 3DS main chunk and construct a static scene from editor and earliest keyframe data.
fn parse(bytes: &[u8]) -> Result<Scene> {
    let mut reader = Reader::new(bytes);
    let (id, data) = reader.chunk()?.ok_or("empty 3DS file")?;
    if id != 0x4d4d {
        return Err("expected a 3DS main chunk".into());
    }
    reader.finish()?;
    let mut model = Model::default();
    let mut reader = Reader::new(data);
    while let Some((id, data)) = reader.chunk()? {
        match id {
            0x3d3d => editor(data, &mut model)?,
            0xb000 => keyframer(data, &mut model)?,
            0x0002 => {
                let mut reader = Reader::new(data);
                reader.u32()?;
                reader.finish()?;
            }
            _ => {} // Global view, background, and application metadata.
        }
    }
    build(&model)
}

/// Decode object/material chunks and the optional scene master scale.
fn editor(data: &[u8], model: &mut Model) -> Result<()> {
    let mut reader = Reader::new(data);
    while let Some((id, data)) = reader.chunk()? {
        match id {
            0x4000 => object(data, model)?,
            0xafff => {
                let (name, color) = material(data)?;
                if model
                    .materials
                    .insert(name.to_ascii_lowercase(), color)
                    .is_some()
                {
                    return Err("duplicate 3DS material name".into());
                }
            }
            0x0100 => {
                let mut reader = Reader::new(data);
                let value = reader.float()?;
                reader.finish()?;
                if value == 0.0 {
                    return Err("3DS master scale cannot be zero".into());
                }
                if model.master_scale.replace(value).is_some() {
                    return Err("duplicate 3DS master scale".into());
                }
            }
            0x4f00 => return Err("legacy 3DS editor hierarchy is not implemented".into()),
            _ => {} // Lights, cameras, viewports, units, and environment settings.
        }
    }
    Ok(())
}

/// Decode a named editor object and reject unsupported visible geometry.
fn object(data: &[u8], model: &mut Model) -> Result<()> {
    let mut reader = Reader::new(data);
    let name = reader.string()?;
    if model.object_names.len() >= MAX_OBJECTS || !model.object_names.insert(name.clone()) {
        return Err("3DS has duplicate object names or exceeds its object limit".into());
    }
    let mut mesh = None;
    let mut hidden = false;
    while let Some((id, data)) = reader.chunk()? {
        match id {
            0x4100 => {
                if mesh.is_some() {
                    return Err("multiple 3DS mesh blocks in one object are not implemented".into());
                }
                mesh = Some(mesh_data(data, name.clone())?);
            }
            0x4010 => hidden = true,
            0x4011..=0x4016 | 0x4600 | 0x4700 => {}
            0x80f0 => return Err("3DS parent-name hierarchy is not implemented".into()),
            _ => return Err(format!("unsupported 3DS object geometry chunk 0x{id:04x}").into()),
        }
    }
    if let Some(mut mesh) = mesh {
        model.vertices = model
            .vertices
            .checked_add(mesh.vertices.len())
            .ok_or("3DS vertex count overflow")?;
        model.faces = model
            .faces
            .checked_add(mesh.faces.len())
            .ok_or("3DS face count overflow")?;
        if model.vertices > MAX_ELEMENTS || model.faces > MAX_ELEMENTS {
            return Err("3DS exceeds its geometry limit".into());
        }
        mesh.hidden = hidden;
        model.meshes.push(mesh);
    }
    Ok(())
}

/// Decode triangle indices, vertex coordinates, material assignments and the stored mesh matrix.
#[allow(
    clippy::too_many_lines,
    reason = "Keep 3DS vertex, face, flag and material-group chunks together while validating shared indices."
)]
fn mesh_data(data: &[u8], name: String) -> Result<Mesh> {
    let mut mesh = Mesh {
        name,
        vertices: Vec::new(),
        faces: Vec::new(),
        material_groups: Vec::new(),
        matrix: Transform::IDENTITY,
        hidden: false,
    };
    let mut has_vertices = false;
    let mut has_faces = false;
    let mut has_matrix = false;
    let mut vertex_flags_length = None;
    let mut reader = Reader::new(data);
    while let Some((id, data)) = reader.chunk()? {
        let mut values = Reader::new(data);
        match id {
            0x4110 => {
                if has_vertices {
                    return Err("duplicate 3DS vertex list".into());
                }
                has_vertices = true;
                let count = usize::from(values.u16()?);
                if count > values.remaining() / 12 {
                    return Err("truncated 3DS vertex list".into());
                }
                mesh.vertices.reserve(count);
                for _ in 0..count {
                    mesh.vertices.push(values.vector()?);
                }
                values.finish()?;
            }
            0x4120 => {
                if has_faces {
                    return Err("duplicate 3DS face list".into());
                }
                has_faces = true;
                let count = usize::from(values.u16()?);
                if count > values.remaining() / 8 {
                    return Err("truncated 3DS face list".into());
                }
                mesh.faces.reserve(count);
                for _ in 0..count {
                    mesh.faces
                        .push([values.u16()?, values.u16()?, values.u16()?]);
                    values.u16()?; // Edge visibility affects wireframes only.
                }
                while let Some((id, data)) = values.chunk()? {
                    match id {
                        0x4130 => {
                            let mut group = Reader::new(data);
                            let name = group.string()?;
                            let count = usize::from(group.u16()?);
                            if count > group.remaining() / 2 {
                                return Err("truncated 3DS material face list".into());
                            }
                            let mut indices = Vec::with_capacity(count);
                            for _ in 0..count {
                                let index = group.u16()?;
                                if usize::from(index) >= mesh.faces.len() {
                                    return Err("3DS material references a missing face".into());
                                }
                                indices.push(index);
                            }
                            group.finish()?;
                            mesh.material_groups.push((name, indices));
                        }
                        0x4150 => {
                            if !data.len().is_multiple_of(4) || data.len() / 4 > mesh.faces.len() {
                                return Err("invalid 3DS smoothing-group list".into());
                            }
                        }
                        _ => {
                            return Err(
                                format!("unsupported 3DS face geometry chunk 0x{id:04x}").into()
                            );
                        }
                    }
                }
            }
            0x4160 => {
                if has_matrix {
                    return Err("duplicate 3DS mesh transform".into());
                }
                has_matrix = true;
                for column in 0..4 {
                    for row in 0..3 {
                        mesh.matrix.0[column][row] = values.float()?;
                    }
                }
                values.finish()?;
            }
            0x4111 => {
                // Unlike vertex positions and UVs, the flag array has no count
                // prefix. Its length is determined by the vertex list.
                if vertex_flags_length.replace(data.len()).is_some() {
                    return Err("duplicate 3DS vertex flag array".into());
                }
            }
            0x4140 => {
                let count = usize::from(values.u16()?);
                if count > values.remaining() / 8 {
                    return Err("truncated 3DS texture-coordinate list".into());
                }
                for _ in 0..count {
                    values.float()?;
                    values.float()?;
                }
                values.finish()?;
            }
            0x4165 | 0x4170 => {} // Editor display color and texture projection metadata.
            _ => return Err(format!("unsupported 3DS mesh geometry chunk 0x{id:04x}").into()),
        }
    }
    if !has_vertices || !has_faces {
        return Err("3DS mesh is missing its vertices or faces".into());
    }
    if vertex_flags_length.is_some_and(|length| length != mesh.vertices.len() * 2) {
        return Err("3DS vertex flag count does not match its vertex list".into());
    }
    if mesh
        .faces
        .iter()
        .flatten()
        .any(|&index| usize::from(index) >= mesh.vertices.len())
    {
        return Err("3DS triangle references a missing vertex".into());
    }
    Ok(mesh)
}

/// Decode a named 3DS material and its optional diffuse color.
fn material(data: &[u8]) -> Result<(String, Option<[u8; 3]>)> {
    let mut name = None;
    let mut diffuse = None;
    let mut reader = Reader::new(data);
    while let Some((id, data)) = reader.chunk()? {
        match id {
            0xa000 => {
                let mut value = Reader::new(data);
                let material_name = value.string()?;
                value.finish()?;
                if name.replace(material_name).is_some() {
                    return Err("duplicate 3DS material name field".into());
                }
            }
            0xa020 => diffuse = Some(color(data)?),
            _ => {} // Texture/PBR properties are outside flat-color thumbnail rendering.
        }
    }
    Ok((name.ok_or("3DS material has no name")?, diffuse))
}

/// Decode byte/float material color chunks with the specified gamma encoding.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Material channels are clamped to the byte range and intentionally rounded before quantization."
)]
fn color(data: &[u8]) -> Result<[u8; 3]> {
    let mut encoded = None;
    let mut linear = None;
    let mut reader = Reader::new(data);
    while let Some((id, data)) = reader.chunk()? {
        let mut value = Reader::new(data);
        let channels = match id {
            0x0010 | 0x0013 => {
                let color = [value.float()?, value.float()?, value.float()?];
                if color.iter().any(|&c| !(0.0..=1.0).contains(&c)) {
                    return Err("3DS material color is outside 0..1".into());
                }
                color
            }
            0x0011 | 0x0012 => {
                let bytes = value.take(3)?;
                std::array::from_fn(|i| f64::from(bytes[i]) / 255.0)
            }
            _ => return Err(format!("unsupported 3DS color chunk 0x{id:04x}").into()),
        };
        value.finish()?;
        if matches!(id, 0x0010 | 0x0011) {
            encoded = Some(channels);
        } else {
            linear = Some(channels);
        }
    }
    let channels = encoded
        .or_else(|| {
            linear.map(|values| {
                values.map(|c| {
                    if c <= 0.003_130_8 {
                        12.92 * c
                    } else {
                        1.055 * c.powf(1.0 / 2.4) - 0.055
                    }
                })
            })
        })
        .ok_or("3DS diffuse material has no color")?;
    Ok(channels.map(|c| (c * 255.0).round() as u8))
}

/// Decode supported scene hierarchy nodes and reject unsupported geometry-changing tracks.
#[allow(
    clippy::cast_possible_truncation,
    reason = "The node count is checked against MAX_OBJECTS, at most 65535, before generating the ordinal u16 ID."
)]
fn keyframer(data: &[u8], model: &mut Model) -> Result<()> {
    let mut reader = Reader::new(data);
    while let Some((id, data)) = reader.chunk()? {
        match id {
            0xb002..=0xb007 => {
                if model.nodes.len() >= MAX_OBJECTS {
                    return Err("3DS exceeds its hierarchy node limit".into());
                }
                model.nodes.push(node(data, model.nodes.len() as u16, id)?);
            }
            0xb001 | 0xb008..=0xb00a => {} // Ambient tracks and animation timing metadata.
            _ => return Err(format!("unsupported 3DS keyframer chunk 0x{id:04x}").into()),
        }
    }
    Ok(())
}

/// Decode a keyframer node identifier, parent, pivot and earliest transform tracks.
fn node(data: &[u8], index: u16, kind: u16) -> Result<Node> {
    let mut node = Node {
        kind,
        id: index,
        name: String::new(),
        parent: None,
        pivot: Vec3::default(),
        position: Vec3::default(),
        rotation: Transform::IDENTITY,
        scale: Vec3::new(1.0, 1.0, 1.0),
    };
    let mut seen = HashSet::new();
    let mut camera_roll = None;
    let mut reader = Reader::new(data);
    while let Some((id, data)) = reader.chunk()? {
        if !seen.insert(id) {
            return Err("duplicate 3DS hierarchy node field".into());
        }
        let mut value = Reader::new(data);
        match id {
            0xb010 => {
                node.name = value.string()?;
                value.u16()?;
                value.u16()?;
                let parent = value.u16()?;
                node.parent = if parent == u16::MAX {
                    None
                } else {
                    Some(parent)
                };
                value.finish()?;
            }
            0xb011 => {
                value.string()?;
                value.finish()?;
            } // Instance display name, not source mesh name.
            0xb013 => {
                node.pivot = value.vector()?;
                value.finish()?;
            }
            0xb020 => {
                if let Some(key) = vector_track(data)? {
                    // Target positions describe the associated camera/light's
                    // look direction rather than a target node transform.
                    if !matches!(kind, 0xb004 | 0xb006) {
                        node.position = key;
                    }
                }
            }
            0xb021 => {
                node.rotation = rotation_track(data)?;
            }
            0xb022 => {
                if let Some(key) = vector_track(data)? {
                    node.scale = key;
                }
            }
            0xb023 if kind == 0xb003 => {
                scalar_track(data)?;
            }
            0xb024 if kind == 0xb003 => {
                camera_roll = scalar_track(data)?;
            }
            0xb025 if matches!(kind, 0xb005 | 0xb007) => {
                vector_track(data)?;
            }
            0xb027 | 0xb028 if kind == 0xb007 => {
                scalar_track(data)?;
            }
            0xb030 => {
                node.id = value.u16()?;
                value.finish()?;
            }
            0xb014 => {
                let _ = value.vector()?;
                let _ = value.vector()?;
                value.finish()?;
            }
            0xb015 => {
                value.float()?;
                value.finish()?;
            }
            0xb012 => return Err("3DS hierarchy prescale is not implemented".into()),
            0xb026 => return Err("3DS morph geometry is not implemented".into()),
            0xb029 => return Err("3DS animated visibility is not implemented".into()),
            0x80f0 => return Err("3DS parent-name hierarchy is not implemented".into()),
            _ => return Err(format!("unsupported 3DS object track 0x{id:04x}").into()),
        }
    }
    if !seen.contains(&0xb010) {
        return Err("3DS hierarchy node has no object header".into());
    }
    if !seen.contains(&0xb021)
        && let Some(roll) = camera_roll
    {
        node.rotation = rotation(Vec3::new(0.0, 0.0, 1.0), -roll.to_radians())?;
    }
    Ok(node)
}

/// Validate track flags and read the bounded key count.
fn track_header(reader: &mut Reader<'_>) -> Result<usize> {
    let flags = reader.u16()?;
    if flags & 0x0700 != 0 {
        return Err("3DS tracks with unlinked parent axes are not implemented".into());
    }
    reader.u32()?;
    reader.u32()?;
    let count = reader.u32()? as usize;
    if count > MAX_ELEMENTS {
        return Err("3DS track exceeds its key limit".into());
    }
    Ok(count)
}

/// Read a keyframe index and consume its optional spline parameters.
fn key_header(reader: &mut Reader<'_>) -> Result<u32> {
    let frame = reader.u32()?;
    let flags = reader.u16()?;
    if flags & !0x001f != 0 {
        return Err("unsupported 3DS spline flags".into());
    }
    for bit in 0..5 {
        if flags & (1 << bit) != 0 {
            reader.float()?;
        }
    }
    Ok(frame)
}

/// Read the earliest vector key while validating all later keys.
fn vector_track(data: &[u8]) -> Result<Option<Vec3>> {
    let mut reader = Reader::new(data);
    let count = track_header(&mut reader)?;
    if count > reader.remaining() / 18 {
        return Err("truncated 3DS vector track".into());
    }
    let mut earliest = None;
    for _ in 0..count {
        let frame = key_header(&mut reader)?;
        let value = reader.vector()?;
        if earliest.is_none_or(|(first, _)| frame < first) {
            earliest = Some((frame, value));
        }
    }
    reader.finish()?;
    Ok(earliest.map(|(_, value)| value))
}

/// Read the earliest scalar key while validating all later keys.
fn scalar_track(data: &[u8]) -> Result<Option<f64>> {
    let mut reader = Reader::new(data);
    let count = track_header(&mut reader)?;
    if count > reader.remaining() / 10 {
        return Err("truncated 3DS scalar track".into());
    }
    let mut earliest = None;
    for _ in 0..count {
        let frame = key_header(&mut reader)?;
        let value = reader.float()?;
        if earliest.is_none_or(|(first, _)| frame < first) {
            earliest = Some((frame, value));
        }
    }
    reader.finish()?;
    Ok(earliest.map(|(_, value)| value))
}

/// Read and compose the earliest incremental axis-angle rotation keys.
fn rotation_track(data: &[u8]) -> Result<Transform> {
    let mut reader = Reader::new(data);
    let count = track_header(&mut reader)?;
    if count > reader.remaining() / 22 {
        return Err("truncated 3DS rotation track".into());
    }
    let mut earliest = None;
    for _ in 0..count {
        let frame = key_header(&mut reader)?;
        let angle = reader.float()?;
        let axis = reader.vector()?;
        if earliest.is_none_or(|(first, _, _)| frame < first) {
            earliest = Some((frame, angle, axis));
        }
    }
    reader.finish()?;
    if let Some((_, angle, axis)) = earliest {
        rotation(axis, -angle)
    } else {
        Ok(Transform::IDENTITY)
    }
}

/// Construct an affine translation from a position vector.
fn translation(v: Vec3) -> Transform {
    let mut matrix = Transform::IDENTITY;
    matrix.0[3] = [v.x, v.y, v.z, 1.0];
    matrix
}

/// Construct a diagonal affine scale from axis factors.
fn scaling(v: Vec3) -> Transform {
    let mut matrix = Transform::IDENTITY;
    matrix.0[0][0] = v.x;
    matrix.0[1][1] = v.y;
    matrix.0[2][2] = v.z;
    matrix
}

/// Convert a finite axis-angle rotation to the 3DS clockwise transform convention.
fn rotation(axis: Vec3, angle: f64) -> Result<Transform> {
    if angle == 0.0 {
        return Ok(Transform::IDENTITY);
    }
    if axis.length() == 0.0 {
        return Err("3DS rotation has a zero axis".into());
    }
    let axis = axis.normalized();
    let (sin, cos) = angle.sin_cos();
    let component = [axis.x, axis.y, axis.z];
    let mut matrix = Transform::IDENTITY;
    for (column, &coordinate) in component.iter().enumerate() {
        let unit = match column {
            0 => Vec3::new(1.0, 0.0, 0.0),
            1 => Vec3::new(0.0, 1.0, 0.0),
            _ => Vec3::new(0.0, 0.0, 1.0),
        };
        let value = unit * cos + axis.cross(unit) * sin + axis * (coordinate * (1.0 - cos));
        matrix.0[column] = [value.x, value.y, value.z, 0.0];
    }
    Ok(matrix)
}

/// Invert the stored mesh matrix and report whether its determinant reverses orientation.
fn inverse(matrix: Transform) -> Result<(Transform, bool)> {
    let [a, b, c, t] = matrix
        .0
        .map(|column| Vec3::new(column[0], column[1], column[2]));
    let determinant = a.dot(b.cross(c));
    if determinant == 0.0 || !determinant.is_finite() {
        return Err("3DS mesh transform cannot be inverted".into());
    }
    let rows = [
        b.cross(c) / determinant,
        c.cross(a) / determinant,
        a.cross(b) / determinant,
    ];
    let mut result = Transform::IDENTITY;
    result.0[0] = [rows[0].x, rows[1].x, rows[2].x, 0.0];
    result.0[1] = [rows[0].y, rows[1].y, rows[2].y, 0.0];
    result.0[2] = [rows[0].z, rows[1].z, rows[2].z, 0.0];
    let offset = -result.apply(t);
    result.0[3] = [offset.x, offset.y, offset.z, 1.0];
    if result.0.iter().flatten().any(|v| !v.is_finite()) {
        return Err("3DS inverse mesh transform is non-finite".into());
    }
    Ok((result, determinant < 0.0))
}

/// Resolve and cache parent-composed node transforms while rejecting hierarchy cycles.
fn node_world(
    index: usize,
    nodes: &[Node],
    ids: &HashMap<u16, usize>,
    states: &mut [u8],
    worlds: &mut [Transform],
    depth: usize,
) -> Result<Transform> {
    if depth > 256 || states[index] == 1 {
        return Err("3DS hierarchy is cyclic or too deep".into());
    }
    if states[index] == 2 {
        return Ok(worlds[index]);
    }
    states[index] = 1;
    let parent = if let Some(id) = nodes[index].parent {
        node_world(
            *ids.get(&id)
                .ok_or("3DS hierarchy references a missing parent")?,
            nodes,
            ids,
            states,
            worlds,
            depth + 1,
        )?
    } else {
        Transform::IDENTITY
    };
    let world = parent.then(nodes[index].local());
    if world.0.iter().flatten().any(|v| !v.is_finite()) {
        return Err("3DS hierarchy transform is non-finite".into());
    }
    worlds[index] = world;
    states[index] = 2;
    Ok(world)
}

/// Resolve meshes/materials and instantiate visible editor or keyframer geometry.
fn build(model: &Model) -> Result<Scene> {
    let mut scene = Scene::default();
    let mut ids = HashMap::new();
    for (index, node) in model.nodes.iter().enumerate() {
        if ids.insert(node.id, index).is_some() {
            return Err("duplicate 3DS hierarchy node ID".into());
        }
        if node.name != "$$$DUMMY" && !model.object_names.contains(&node.name) {
            return Err("3DS hierarchy references a missing object".into());
        }
    }
    let mut states = vec![0; model.nodes.len()];
    let mut worlds = vec![Transform::IDENTITY; model.nodes.len()];
    for index in 0..model.nodes.len() {
        node_world(index, &model.nodes, &ids, &mut states, &mut worlds, 0)?;
    }
    let scale = scaling(Vec3::new(
        model.master_scale.unwrap_or(1.0),
        model.master_scale.unwrap_or(1.0),
        model.master_scale.unwrap_or(1.0),
    ));
    let mut meshes_by_name = HashMap::new();
    for (index, mesh) in model.meshes.iter().enumerate() {
        meshes_by_name.insert(mesh.name.as_str(), index);
    }
    let mut referenced = HashSet::new();
    for (index, node) in model.nodes.iter().enumerate() {
        if node.kind != 0xb002 {
            continue;
        }
        if let Some(&mesh_index) = meshes_by_name.get(node.name.as_str()) {
            referenced.insert(mesh_index);
            let mesh = &model.meshes[mesh_index];
            if mesh.hidden {
                continue;
            }
            let (inverse, mirrored) = inverse(mesh.matrix)?;
            let world = scale.then(worlds[index]).then(translation(-node.pivot));
            append(mesh, &model.materials, &mut scene, |p| {
                let mut local = inverse.apply(p);
                if mirrored {
                    local.x = -local.x;
                }
                world.apply(local)
            })?;
        }
    }
    // Editor vertices already contain their mesh transform. Applying it again
    // would move untracked meshes twice; preserve the stored world positions.
    for (index, mesh) in model.meshes.iter().enumerate() {
        if !referenced.contains(&index) && !mesh.hidden {
            append(mesh, &model.materials, &mut scene, |p| scale.apply(p))?;
        }
    }
    scene.validate()?;
    Ok(scene)
}

/// Append a placed mesh with resolved face colors and the 3DS mirrored-winding convention.
fn append(
    mesh: &Mesh,
    materials: &HashMap<String, Option<[u8; 3]>>,
    scene: &mut Scene,
    transform: impl Fn(Vec3) -> Vec3,
) -> Result<()> {
    if scene.triangles.len().saturating_add(mesh.faces.len()) > MAX_ELEMENTS {
        return Err("3DS instantiated scene exceeds its triangle limit".into());
    }
    let mut colors = vec![None; mesh.faces.len()];
    let mut assigned = vec![false; mesh.faces.len()];
    for (name, faces) in &mesh.material_groups {
        let color = *materials
            .get(&name.to_ascii_lowercase())
            .ok_or("3DS face references a missing material")?;
        for &face in faces {
            let index = usize::from(face);
            if assigned[index] {
                return Err("3DS face belongs to multiple material groups".into());
            }
            assigned[index] = true;
            colors[index] = color;
        }
    }
    for (index, &face) in mesh.faces.iter().enumerate() {
        let vertices = face.map(|i| transform(mesh.vertices[usize::from(i)]));
        if vertices.iter().any(|p| !p.is_finite()) {
            return Err("3DS transformed vertex is non-finite".into());
        }
        scene.triangles.push(Triangle {
            vertices,
            color: colors[index],
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(
        clippy::cast_possible_truncation,
        reason = "Every authored chunk payload is small enough for its u32 format length word."
    )]
    fn chunk(id: u16, bytes: &[u8]) -> Vec<u8> {
        let mut data = id.to_le_bytes().to_vec();
        data.extend_from_slice(&((bytes.len() + 6) as u32).to_le_bytes());
        data.extend_from_slice(bytes);
        data
    }
    fn floats(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn words(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn mesh(matrix: Option<&[f32]>, vertices: &[f32], material: bool) -> Vec<u8> {
        let mut vertex_data = 3_u16.to_le_bytes().to_vec();
        vertex_data.extend_from_slice(&floats(vertices));
        let mut face_data = words(&[1, 0, 1, 2, 0]);
        if material {
            let mut group = b"green\0".to_vec();
            group.extend_from_slice(&words(&[1, 0]));
            face_data.extend_from_slice(&chunk(0x4130, &group));
        }
        let mut mesh = chunk(0x4110, &vertex_data);
        mesh.extend_from_slice(&chunk(0x4120, &face_data));
        if let Some(matrix) = matrix {
            mesh.extend_from_slice(&chunk(0x4160, &floats(matrix)));
        }
        let mut object = b"mesh\0".to_vec();
        object.extend_from_slice(&chunk(0x4100, &mesh));
        chunk(0x4000, &object)
    }
    fn file(editor: &[u8], nodes: &[u8]) -> Vec<u8> {
        let mut data = chunk(0x3d3d, editor);
        if !nodes.is_empty() {
            data.extend_from_slice(&chunk(0xb000, nodes));
        }
        chunk(0x4d4d, &data)
    }
    fn track(values: &[f32]) -> Vec<u8> {
        let mut data = words(&[0]);
        data.extend_from_slice(&0_u32.to_le_bytes());
        data.extend_from_slice(&0_u32.to_le_bytes());
        data.extend_from_slice(&1_u32.to_le_bytes());
        data.extend_from_slice(&0_u32.to_le_bytes());
        data.extend_from_slice(&0_u16.to_le_bytes());
        data.extend_from_slice(&floats(values));
        data
    }
    fn hierarchy_node(name: &str, id: u16, parent: u16, tracks: &[u8]) -> Vec<u8> {
        let mut header = name.as_bytes().to_vec();
        header.push(0);
        header.extend_from_slice(&words(&[0, 0, parent]));
        let mut data = chunk(0xb010, &header);
        data.extend_from_slice(&chunk(0xb030, &words(&[id])));
        data.extend_from_slice(tracks);
        chunk(0xb002, &data)
    }

    #[test]
    fn static_vertices_are_world_space_and_material_assignments_resolve() {
        let matrix = [2., 0., 0., 0., 2., 0., 0., 0., 2., 10., 20., 30.];
        let mut editor = mesh(
            Some(&matrix),
            &[10., 20., 30., 12., 20., 30., 10., 22., 30.],
            true,
        );
        let mut material = chunk(0xa000, b"green\0");
        material.extend_from_slice(&chunk(0xa020, &chunk(0x0011, &[20, 180, 60])));
        // Material can follow the objects which reference it.
        editor.extend_from_slice(&chunk(0xafff, &material));
        let scene = parse(&file(&editor, &[])).unwrap();
        assert_eq!(scene.triangles[0].vertices[0], Vec3::new(10., 20., 30.));
        assert_eq!(scene.triangles[0].vertices[1], Vec3::new(12., 20., 30.));
        assert_eq!(scene.triangles[0].color, Some([20, 180, 60]));
    }

    #[test]
    fn hierarchy_undoes_mesh_matrix_and_composes_pivot_clockwise_rotation_scale() {
        let matrix = [2., 0., 0., 0., 2., 0., 0., 0., 2., 10., 20., 30.];
        let editor = mesh(
            Some(&matrix),
            &[10., 20., 30., 12., 20., 30., 10., 22., 30.],
            false,
        );
        let root = hierarchy_node(
            "$$$DUMMY",
            10,
            u16::MAX,
            &chunk(0xb020, &track(&[100., 200., 300.])),
        );
        let mut child_tracks = chunk(0xb020, &track(&[5., 6., 7.]));
        child_tracks.extend_from_slice(&chunk(
            0xb021,
            &track(&[std::f32::consts::FRAC_PI_2, 0., 0., 1.]),
        ));
        child_tracks.extend_from_slice(&chunk(0xb022, &track(&[2., 3., 4.])));
        child_tracks.extend_from_slice(&chunk(0xb013, &floats(&[1., 0., 0.])));
        // Parent references use node IDs and may appear later in the file.
        let mut nodes = hierarchy_node("mesh", 42, 10, &child_tracks);
        nodes.extend_from_slice(&root);
        let scene = parse(&file(&editor, &nodes)).unwrap();
        let expected = [
            Vec3::new(105., 208., 307.),
            Vec3::new(105., 206., 307.),
            Vec3::new(108., 208., 307.),
        ];
        for (&actual, expected) in scene.triangles[0].vertices.iter().zip(expected) {
            assert!(
                (actual - expected).length() < 1e-5,
                "{actual:?} != {expected:?}"
            );
        }
    }

    #[test]
    fn instances_and_negative_determinant_meshes_use_the_3ds_mirror_convention() {
        let matrix = [-1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.];
        let editor = mesh(Some(&matrix), &[0., 0., 0., -1., 0., 0., 0., 1., 0.], false);
        let mut nodes = hierarchy_node("mesh", 0, u16::MAX, &[]);
        nodes.extend_from_slice(&hierarchy_node(
            "mesh",
            1,
            u16::MAX,
            &chunk(0xb020, &track(&[10., 0., 0.])),
        ));
        let scene = parse(&file(&editor, &nodes)).unwrap();
        assert_eq!(scene.triangles.len(), 2);
        assert_eq!(scene.triangles[0].vertices[1], Vec3::new(-1., 0., 0.));
        assert_eq!(scene.triangles[1].vertices[1], Vec3::new(9., 0., 0.));
    }

    #[test]
    fn earliest_key_and_spline_fields_are_read_without_using_later_animation() {
        let mut data = words(&[0]);
        data.extend_from_slice(&0_u32.to_le_bytes());
        data.extend_from_slice(&0_u32.to_le_bytes());
        data.extend_from_slice(&2_u32.to_le_bytes());
        for (frame, flags, values) in [(20_u32, 0_u16, [9., 8., 7.]), (0, 0x1f, [1., 2., 3.])] {
            data.extend_from_slice(&frame.to_le_bytes());
            data.extend_from_slice(&flags.to_le_bytes());
            for _ in 0..flags.count_ones() {
                data.extend_from_slice(&0.5_f32.to_le_bytes());
            }
            data.extend_from_slice(&floats(&values));
        }
        assert_eq!(vector_track(&data).unwrap(), Some(Vec3::new(1., 2., 3.)));
    }

    #[test]
    fn camera_ancestors_apply_first_position_and_clockwise_roll_to_mesh_children() {
        let mut editor = mesh(None, &[0., 0., 0., 1., 0., 0., 0., 1., 0.], false);
        let mut camera_object = b"camera\0".to_vec();
        camera_object
            .extend_from_slice(&chunk(0x4700, &floats(&[0., 0., 0., 0., 0., -1., 0., 80.])));
        editor.extend_from_slice(&chunk(0x4000, &camera_object));
        let mut camera_tracks = chunk(0xb020, &track(&[10., 20., 30.]));
        camera_tracks.extend_from_slice(&chunk(0xb024, &track(&[90.])));
        let mut camera = hierarchy_node("camera", 4, u16::MAX, &camera_tracks);
        camera[..2].copy_from_slice(&0xb003_u16.to_le_bytes());
        let mut nodes = camera;
        nodes.extend_from_slice(&hierarchy_node(
            "mesh",
            10,
            4,
            &chunk(0xb020, &track(&[3., 0., 0.])),
        ));
        let scene = parse(&file(&editor, &nodes)).unwrap();
        let expected = [
            Vec3::new(10., 17., 30.),
            Vec3::new(10., 16., 30.),
            Vec3::new(11., 17., 30.),
        ];
        for (&actual, expected) in scene.triangles[0].vertices.iter().zip(expected) {
            assert!((actual - expected).length() < 1e-9);
        }
    }

    #[test]
    fn vertex_flags_are_a_raw_array_with_one_word_per_vertex() {
        let mut vertices = words(&[3]);
        vertices.extend_from_slice(&floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]));
        let mut data = chunk(0x4111, &words(&[7, 7, 7]));
        data.extend_from_slice(&chunk(0x4110, &vertices));
        data.extend_from_slice(&chunk(0x4120, &words(&[1, 0, 1, 2, 0])));
        assert!(mesh_data(&data, "mesh".to_owned()).is_ok());
        data.extend_from_slice(&chunk(0x4111, &words(&[7])));
        assert!(mesh_data(&data, "mesh".to_owned()).is_err());
    }

    #[test]
    fn color_encoding_and_invalid_fields_are_explicit() {
        assert_eq!(
            color(&chunk(0x0010, &floats(&[0.5, 0.25, 1.]))).unwrap(),
            [128, 64, 255]
        );
        assert_eq!(
            color(&chunk(0x0013, &floats(&[0.5, 0.0, 1.]))).unwrap(),
            [188, 0, 255]
        );
        assert!(color(&chunk(0x0010, &floats(&[f32::NAN, 0., 0.]))).is_err());
        assert!(color(&chunk(0x0010, &floats(&[2., 0., 0.]))).is_err());
    }

    #[test]
    fn truncation_bad_lengths_indices_and_unsupported_geometry_return_errors() {
        let data = file(
            &mesh(None, &[0., 0., 0., 1., 0., 0., 0., 1., 0.], false),
            &[],
        );
        for count in 0..data.len() {
            assert!(
                parse(&data[..count]).is_err(),
                "accepted truncated length {count}"
            );
        }
        let mut invalid = data.clone();
        invalid[2..6].copy_from_slice(&5_u32.to_le_bytes());
        assert!(parse(&invalid).is_err());
        let mut bad_mesh = mesh(None, &[0., 0., 0., 1., 0., 0., 0., 1., 0.], false);
        let face = bad_mesh.windows(2).position(|b| b == [0x20, 0x41]).unwrap();
        bad_mesh[face + 8..face + 10].copy_from_slice(&99_u16.to_le_bytes());
        assert!(parse(&file(&bad_mesh, &[])).is_err());
        let mut object = b"mesh\0".to_vec();
        object.extend_from_slice(&chunk(0x4199, &[]));
        assert!(parse(&file(&chunk(0x4000, &object), &[])).is_err());
    }

    #[test]
    fn invalid_hierarchy_and_unsupported_morph_tracks_return_errors() {
        let editor = mesh(None, &[0., 0., 0., 1., 0., 0., 0., 1., 0.], false);
        assert!(parse(&file(&editor, &hierarchy_node("mesh", 0, 1, &[]))).is_err());
        assert!(parse(&file(&editor, &hierarchy_node("mesh", 0, 0, &[]))).is_err());
        assert!(
            parse(&file(
                &editor,
                &hierarchy_node("mesh", 0, u16::MAX, &chunk(0xb026, &[]))
            ))
            .is_err()
        );
        let singular = [0.; 12];
        let editor = mesh(
            Some(&singular),
            &[0., 0., 0., 1., 0., 0., 0., 1., 0.],
            false,
        );
        assert!(parse(&file(&editor, &hierarchy_node("mesh", 0, u16::MAX, &[]))).is_err());
    }
}
