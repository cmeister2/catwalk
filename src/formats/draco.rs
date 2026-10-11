//! Pure Rust Draco geometry decoding, also shared with compressed glTF primitives.
use crate::scene::{Point, Scene, Triangle, TriangleAppearance, Vec3};
use crate::{Error, Result};
use draco_core::{
    DataType, DecodeLimits, DecoderBuffer, GeometryAttributeType, Mesh, MeshDecoder,
    PointAttribute, PointCloud, PointCloudDecoder, PointIndex,
};
use std::path::Path;

/// Maximum declared point count accepted before geometry allocation.
const MAX_POINTS: usize = 5_000_000;
/// Maximum decoded triangle count accepted from one stream.
const MAX_FACES: usize = 5_000_000;
/// Maximum estimated retained and expanded storage for a mesh with authored normals.
const MAX_NORMAL_BYTES: usize = 512 * 1024 * 1024;
/// Convert an importer diagnostic into the shared error type.
fn error(message: impl Into<String>) -> Error {
    message.into().into()
}

/// Attribute values are raw scalars in decoded point order. `normalized` and
/// `data_type` allow a container to apply its accessor's normalization rules.
pub(super) struct DecodedAttribute {
    /// Number of scalars in one point attribute tuple.
    pub components: usize,
    /// Original Draco scalar storage type.
    pub data_type: DataType,
    /// Whether container interpretation normalizes integer attribute values.
    pub normalized: bool,
    /// Finite scalar tuples expanded in decoded point order.
    pub values: Vec<f64>,
}
/// Decoded Draco topology with attributes retained for semantic or unique-ID lookup.
pub(super) struct DecodedGeometry {
    /// Number of decoded geometry points.
    pub point_count: usize,
    /// `None` identifies a point cloud, including one with three points.
    pub faces: Option<Vec<[usize; 3]>>,
    /// Decoded attribute buffers and point-to-value mappings.
    attributes: PointCloud,
}
impl DecodedGeometry {
    /// Resolve the Draco unique ID declared by `KHR_draco_mesh_compression`.
    ///
    /// # Errors
    /// Returns an error if the unique ID is missing or ambiguous, its attribute
    /// mapping/storage is invalid, or its scalar tuples exceed limits.
    pub fn attribute(&self, unique_id: u32) -> Result<DecodedAttribute> {
        let mut found = None;
        for i in 0..self.attributes.num_attributes() {
            let attribute = self.attributes.try_attribute(i)?;
            if attribute.unique_id() == unique_id && found.replace(attribute).is_some() {
                return Err(error(
                    "Draco unique ID is ambiguous across several attributes",
                ));
            }
        }
        self.read_attribute(found.ok_or_else(|| error("Draco attribute unique ID was not found"))?)
    }
    /// Resolve point-to-value mappings into finite scalar tuples while enforcing the
    /// expanded-byte limit.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "Decoded point counts are bounded to five million before creating the decoder u32 point index."
    )]
    fn read_attribute(&self, attribute: &PointAttribute) -> Result<DecodedAttribute> {
        let components = attribute.num_components() as usize;
        let scalar_count = self
            .point_count
            .checked_mul(components)
            .ok_or_else(|| error("Draco attribute scalar count overflow"))?;
        if scalar_count as u64 > super::MAX_INPUT_BYTES / 8 {
            return Err(error("Draco expanded attribute exceeds 512 MiB"));
        }
        let width = attribute.data_type().byte_length();
        let stride = usize::try_from(attribute.byte_stride())?;
        let bytes = attribute.buffer().data();
        let mut values = Vec::with_capacity(scalar_count);
        for point in 0..self.point_count {
            let index = attribute.mapped_index(PointIndex(point as u32)).0 as usize;
            let start = index
                .checked_mul(stride)
                .ok_or_else(|| error("Draco attribute offset overflow"))?;
            let end = start
                .checked_add(components * width)
                .ok_or_else(|| error("Draco attribute tuple size overflow"))?;
            let tuple = bytes
                .get(start..end)
                .ok_or_else(|| error("Draco attribute tuple is truncated"))?;
            for bytes in tuple.chunks_exact(width) {
                let value = scalar(attribute.data_type(), bytes)?;
                if !value.is_finite() {
                    return Err(error("non-finite Draco attribute value"));
                }
                values.push(value);
            }
        }
        Ok(DecodedAttribute {
            components,
            data_type: attribute.data_type(),
            normalized: attribute.normalized(),
            values,
        })
    }
    /// Standalone DRC files use semantic tags rather than container-supplied IDs.
    ///
    /// # Errors
    /// Returns an error for invalid attribute metadata or multiple attributes declaring the
    /// requested semantic.
    pub fn attribute_id(&self, kind: GeometryAttributeType) -> Result<Option<u32>> {
        let mut found = None;
        for i in 0..self.attributes.num_attributes() {
            let attribute = self.attributes.try_attribute(i)?;
            if attribute.attribute_type() == kind && found.replace(attribute.unique_id()).is_some()
            {
                return Err(error(
                    "Draco has multiple attributes for one geometry semantic",
                ));
            }
        }
        Ok(found)
    }
    /// Resolve standalone attribute semantics, including legacy streams with duplicate unique IDs.
    fn semantic_attribute(&self, kind: GeometryAttributeType) -> Result<Option<DecodedAttribute>> {
        let Some(id) = self.attribute_id(kind)? else {
            return Ok(None);
        };
        let mut duplicates = 0;
        for i in 0..self.attributes.num_attributes() {
            if self.attributes.try_attribute(i)?.unique_id() == id {
                duplicates += 1;
            }
        }
        if duplicates == 1 {
            self.attribute(id).map(Some)
        } else {
            // Legacy streams can assign zero to every unique ID. Named
            // semantics still identify their geometry attributes correctly.
            self.attributes
                .named_attribute(kind)
                .map(|attribute| self.read_attribute(attribute))
                .transpose()
        }
    }
}
/// Decode a little-endian Draco scalar into a geometry value.
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    reason = "Signed bytes preserve their raw bit pattern; 64-bit scalar values intentionally approximate to f64 for preview geometry."
)]
fn scalar(data_type: DataType, bytes: &[u8]) -> Result<f64> {
    Ok(match data_type {
        DataType::Int8 => f64::from(bytes[0] as i8),
        DataType::Uint8 => f64::from(bytes[0]),
        DataType::Int16 => f64::from(i16::from_le_bytes(bytes.try_into()?)),
        DataType::Uint16 => f64::from(u16::from_le_bytes(bytes.try_into()?)),
        DataType::Int32 => f64::from(i32::from_le_bytes(bytes.try_into()?)),
        DataType::Uint32 => f64::from(u32::from_le_bytes(bytes.try_into()?)),
        DataType::Int64 => i64::from_le_bytes(bytes.try_into()?) as f64,
        DataType::Uint64 => u64::from_le_bytes(bytes.try_into()?) as f64,
        DataType::Float32 => f64::from(f32::from_le_bytes(bytes.try_into()?)),
        DataType::Float64 => f64::from_le_bytes(bytes.try_into()?),
        DataType::Bool if bytes[0] <= 1 => f64::from(bytes[0]),
        _ => return Err(error("invalid Draco scalar data type or boolean")),
    })
}
/// Validate component stride, backing storage and every point-to-attribute mapping.
#[allow(
    clippy::cast_possible_truncation,
    reason = "Decoded point counts are bounded to five million before creating the decoder u32 point index."
)]
fn validate_attribute(attribute: &PointAttribute, count: usize) -> Result<()> {
    let width = attribute.data_type().byte_length();
    let components = attribute.num_components() as usize;
    let stride = usize::try_from(attribute.byte_stride())?;
    if width == 0 || components == 0 || stride < components * width {
        return Err(error("invalid Draco attribute layout"));
    }
    let size = attribute
        .size()
        .checked_mul(stride)
        .ok_or_else(|| error("Draco attribute storage overflow"))?;
    if size > attribute.buffer().data().len() {
        return Err(error("Draco attribute buffer is truncated"));
    }
    for point in 0..count {
        if attribute.mapped_index(PointIndex(point as u32)).0 as usize >= attribute.size() {
            return Err(error("Draco point-to-attribute mapping is out of bounds"));
        }
    }
    Ok(())
}
/// Decode bounded Draco topology and attributes without losing unique attribute IDs.
///
/// # Errors
/// Returns an error for invalid signatures or geometry types, malformed/trailing compressed
/// data, invalid topology/attribute layouts or decoder resource limits.
pub(super) fn decode_geometry(bytes: &[u8]) -> Result<DecodedGeometry> {
    if bytes.len() as u64 > super::MAX_INPUT_BYTES {
        return Err(error("Draco input exceeds 512 MiB"));
    }
    let header = bytes
        .get(..11)
        .ok_or_else(|| error("truncated Draco header"))?;
    if &header[..5] != b"DRACO" {
        return Err(error("invalid Draco signature"));
    }
    let limits = DecodeLimits::default()
        .with_max_points(MAX_POINTS as u64)
        .with_max_faces(MAX_FACES as u64)
        .with_max_decoded_bytes(super::MAX_INPUT_BYTES);
    let mut buffer = DecoderBuffer::new(bytes).with_limits(limits);
    let (attributes, faces) = match header[7] {
        0 => {
            let mut cloud = PointCloud::new();
            PointCloudDecoder::new().decode(&mut buffer, &mut cloud)?;
            (cloud, None)
        }
        1 => {
            let mut mesh = Mesh::new();
            MeshDecoder::new().decode(&mut buffer, &mut mesh)?;
            let count = mesh.num_points();
            let mut faces = Vec::with_capacity(mesh.num_faces());
            for face in mesh.faces() {
                let indices = face.map(|i| i.0 as usize);
                if indices.iter().any(|&i| i >= count) {
                    return Err(error("Draco triangle index is out of bounds"));
                }
                faces.push(indices);
            }
            (mesh.into_point_cloud(), Some(faces))
        }
        _ => return Err(error("unsupported Draco geometry type")),
    };
    if buffer.remaining_size() != 0 {
        return Err(error("trailing bytes after Draco geometry"));
    }
    let count = attributes.num_points();
    if count == 0 || count > MAX_POINTS {
        return Err(error("Draco point count is empty or exceeds five million"));
    }
    if faces.as_ref().is_some_and(|f| f.len() > MAX_FACES) {
        return Err(error("Draco face count exceeds five million"));
    }
    for i in 0..attributes.num_attributes() {
        let attribute = attributes.try_attribute(i)?;
        validate_attribute(attribute, count)?;
    }
    Ok(DecodedGeometry {
        point_count: count,
        faces,
        attributes,
    })
}
/// Convert normalized or raw Draco color scalars to rounded eight-bit channels.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "Normalization uses the stored integer range; channels are clamped and intentionally rounded to eight-bit RGB."
)]
fn color(value: f64, attribute: &DecodedAttribute) -> u8 {
    let factor = match attribute.data_type {
        DataType::Float32 | DataType::Float64 | DataType::Bool => 255.,
        DataType::Uint8 if attribute.normalized => 1.,
        DataType::Uint16 if attribute.normalized => 255. / f64::from(u16::MAX),
        DataType::Uint32 if attribute.normalized => 255. / f64::from(u32::MAX),
        DataType::Uint64 if attribute.normalized => 255. / u64::MAX as f64,
        DataType::Int8 if attribute.normalized => 255. / f64::from(i8::MAX),
        DataType::Int16 if attribute.normalized => 255. / f64::from(i16::MAX),
        DataType::Int32 if attribute.normalized => 255. / f64::from(i32::MAX),
        DataType::Int64 if attribute.normalized => 255. / i64::MAX as f64,
        _ => 1.,
    };
    (value * factor).clamp(0., 255.).round() as u8
}
/// Normalize a finite direction without overflowing or underflowing its length.
/// Zero vectors have no direction and request the triangle's geometric fallback.
fn unit_normal(value: Vec3) -> Option<Vec3> {
    if !value.is_finite() {
        return None;
    }
    let scale = value.x.abs().max(value.y.abs()).max(value.z.abs());
    (scale > 0.).then(|| (value / scale).normalized())
}

/// Compute a fallback face direction using scaled positions and independently scaled edges.
fn face_normal(vertices: [Vec3; 3]) -> Option<Vec3> {
    let scale = vertices
        .iter()
        .flat_map(|point| [point.x.abs(), point.y.abs(), point.z.abs()])
        .fold(0., f64::max);
    if scale == 0. {
        return None;
    }
    let points = vertices.map(|point| point / scale);
    let a = unit_normal(points[1] - points[0])?;
    let b = unit_normal(points[2] - points[0])?;
    unit_normal(a.cross(b))
}

/// Bound normal-bearing mesh expansion before allocating its attribute tuples or appearances.
///
/// # Errors
///
/// Checked size accounting overflows or estimated retained storage exceeds 512 MiB.
fn normal_storage(
    points: usize,
    faces: usize,
    attribute_bytes: usize,
    attribute_count: usize,
    color_components: usize,
) -> Result<()> {
    let point_size = 2 * std::mem::size_of::<Vec3>()
        + 3 * std::mem::size_of::<f64>()
        + std::mem::size_of::<Option<Vec3>>()
        + usize::from(color_components != 0) * std::mem::size_of::<[u8; 3]>();
    let point_size = color_components
        .checked_mul(std::mem::size_of::<f64>())
        .and_then(|bytes| bytes.checked_add(point_size))
        .ok_or_else(|| error("Draco authored-normal storage accounting overflows"))?;
    let face_size = std::mem::size_of::<[usize; 3]>() + std::mem::size_of::<Triangle>();
    // The appearance B-tree retains unused key/value slots in partially filled
    // nodes. Charge three slots per face plus an entire root's slack, with
    // pointer/header overhead per slot; do not estimate only populated entries.
    let appearance_slot =
        std::mem::size_of::<TriangleAppearance>() + 5 * std::mem::size_of::<usize>();
    let appearance_bytes = faces
        .checked_mul(3)
        .and_then(|slots| slots.checked_add(12))
        .and_then(|slots| slots.checked_mul(appearance_slot))
        .ok_or_else(|| error("Draco authored-normal storage accounting overflows"))?;
    let bytes = points
        .checked_mul(point_size)
        .and_then(|bytes| faces.checked_mul(face_size)?.checked_add(bytes))
        .and_then(|bytes| bytes.checked_add(appearance_bytes))
        .and_then(|bytes| bytes.checked_add(attribute_bytes))
        .and_then(|bytes| {
            // Identity mappings use less storage; charge a dense point mapping conservatively.
            let mapping = points.checked_mul(std::mem::size_of::<u32>())?;
            let metadata = std::mem::size_of::<PointAttribute>()
                .checked_add(4 * std::mem::size_of::<usize>())?;
            attribute_count
                .checked_mul(mapping.checked_add(metadata)?)?
                .checked_add(bytes)
        })
        .ok_or_else(|| error("Draco authored-normal storage accounting overflows"))?;
    if bytes > MAX_NORMAL_BYTES {
        return Err(error(
            "Draco authored-normal mesh exceeds 512 MiB expanded storage",
        ));
    }
    Ok(())
}

/// Resolve mesh-only authored normal directions in decoded point order.
///
/// # Errors
///
/// Normal semantics or tuples are ambiguous, malformed, nonfinite or exceed storage limits.
fn mesh_normals(decoded: &DecodedGeometry) -> Result<Option<Vec<Option<Vec3>>>> {
    let Some(faces) = &decoded.faces else {
        return Ok(None);
    };
    if decoded
        .attribute_id(GeometryAttributeType::Normal)?
        .is_none()
    {
        return Ok(None);
    }
    let normal = decoded
        .attributes
        .named_attribute(GeometryAttributeType::Normal)
        .ok_or_else(|| error("Draco NORMAL semantic has no attribute"))?;
    if normal.num_components() != 3 {
        return Err(error("Draco NORMAL must have three components"));
    }
    let mut attribute_bytes = 0usize;
    for index in 0..decoded.attributes.num_attributes() {
        attribute_bytes = attribute_bytes
            .checked_add(
                decoded
                    .attributes
                    .try_attribute(index)?
                    .buffer()
                    .data()
                    .len(),
            )
            .ok_or_else(|| error("Draco attribute storage accounting overflows"))?;
    }
    let colors = decoded
        .attributes
        .named_attribute(GeometryAttributeType::Color)
        .map_or(0, |attribute| usize::from(attribute.num_components()));
    normal_storage(
        decoded.point_count,
        faces.len(),
        attribute_bytes,
        usize::try_from(decoded.attributes.num_attributes())?,
        colors,
    )?;
    let normal = decoded
        .semantic_attribute(GeometryAttributeType::Normal)?
        .ok_or_else(|| error("Draco NORMAL semantic has no decoded attribute"))?;
    Ok(Some(
        normal
            .values
            .as_chunks::<3>()
            .0
            .iter()
            .map(|values| {
                unit_normal(Vec3::new(
                    normal_component(values[0], &normal),
                    normal_component(values[1], &normal),
                    normal_component(values[2], &normal),
                ))
            })
            .collect(),
    ))
}

/// Saturate the signed normalized minimum before direction normalization cancels its common scale.
#[allow(
    clippy::cast_precision_loss,
    reason = "The normalized 64-bit signed limit intentionally uses the decoded attribute's f64 approximation."
)]
fn normal_component(value: f64, attribute: &DecodedAttribute) -> f64 {
    if !attribute.normalized {
        return value;
    }
    let limit = match attribute.data_type {
        DataType::Int8 => f64::from(i8::MAX),
        DataType::Int16 => f64::from(i16::MAX),
        DataType::Int32 => f64::from(i32::MAX),
        DataType::Int64 => i64::MAX as f64,
        _ => return value,
    };
    value.max(-limit)
}

/// Choose authored corner directions, replacing zero entries with the geometric face direction.
/// Entirely zero normal triples and degenerate fallback triangles retain legacy flat rendering.
fn corner_normals(
    indices: [usize; 3],
    vertices: [Vec3; 3],
    normals: &[Option<Vec3>],
) -> Option<[Vec3; 3]> {
    let values = indices.map(|index| normals[index]);
    if values.iter().all(Option::is_none) {
        return None;
    }
    if let [Some(a), Some(b), Some(c)] = values {
        Some([a, b, c])
    } else {
        let fallback = face_normal(vertices)?;
        Some(values.map(|value| value.unwrap_or(fallback)))
    }
}

/// Expand standalone Draco positions, optional RGB and authored mesh normals into a preview.
///
/// # Errors
/// Returns an error for malformed topology, invalid or ambiguous attributes, nonfinite values,
/// truncated storage or decoder resource limits.
pub(super) fn decode(bytes: &[u8]) -> Result<Scene> {
    decoded_scene(decode_geometry(bytes)?)
}

/// Expand validated point-order attributes while retaining the legacy normal-absent path.
///
/// # Errors
///
/// Selected positions, colors or mesh normals are malformed or exceed preview limits.
#[allow(
    clippy::cast_possible_truncation,
    reason = "A mean of three byte colors remains within the byte range before quantization."
)]
fn decoded_scene(decoded: DecodedGeometry) -> Result<Scene> {
    let normals = mesh_normals(&decoded)?;
    let positions = decoded
        .semantic_attribute(GeometryAttributeType::Position)?
        .ok_or_else(|| error("Draco has no POSITION attribute"))?;
    if positions.components != 3 {
        return Err(error("Draco POSITION must have three components"));
    }
    let positions: Vec<_> = positions
        .values
        .as_chunks::<3>()
        .0
        .iter()
        .map(|v| Vec3::new(v[0], v[1], v[2]))
        .collect();
    let colors = decoded.semantic_attribute(GeometryAttributeType::Color)?;
    if colors
        .as_ref()
        .is_some_and(|c| !matches!(c.components, 3 | 4))
    {
        return Err(error("Draco COLOR must have three or four components"));
    }
    let colors: Option<Vec<[u8; 3]>> = colors.as_ref().map(|attribute| {
        attribute
            .values
            .chunks_exact(attribute.components)
            .map(|v| std::array::from_fn(|i| color(v[i], attribute)))
            .collect()
    });
    let mut scene = Scene::default();
    if let Some(faces) = decoded.faces {
        scene.triangles.try_reserve_exact(faces.len())?;
        for indices in faces {
            let vertices = indices.map(|i| positions[i]);
            let color = colors.as_ref().map(|colors| {
                std::array::from_fn(|component| {
                    (indices
                        .iter()
                        .map(|&i| u16::from(colors[i][component]))
                        .sum::<u16>()
                        / 3) as u8
                })
            });
            if let Some(normals) = normals
                .as_ref()
                .and_then(|normals| corner_normals(indices, vertices, normals))
            {
                scene.appearances.insert(
                    scene.triangles.len(),
                    TriangleAppearance {
                        normals: Some(normals),
                        ..TriangleAppearance::default()
                    },
                );
            }
            scene.triangles.push(Triangle { vertices, color });
        }
    } else {
        for (i, position) in positions.into_iter().enumerate() {
            scene.points.push(Point {
                position,
                color: colors.as_ref().map(|c| c[i]),
            });
        }
    }
    scene.validate()?;
    Ok(scene)
}
/// Read a bounded Draco mesh or point cloud and decode its preview geometry.
///
/// # Errors
/// Returns an error for malformed topology, invalid or ambiguous attributes, nonfinite values,
/// truncated storage or decoder resource limits.
pub(super) fn load(path: &Path) -> Result<Scene> {
    decode(&super::read_file(path)?)
}

#[cfg(test)]
#[path = "draco_normal_tests.rs"]
mod normal_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_signature_geometry_and_truncated_headers() {
        assert!(decode(b"DRACO").is_err());
        assert!(decode(b"WRONG\x02\x02\x01\x01\0\0").is_err());
        assert!(decode(b"DRACO\x02\x02\x02\x01\0\0").is_err());
    }
    #[test]
    fn independent_google_mesh_encodings() {
        for bytes in [
            include_bytes!("../../tests/fixtures/draco/cube.drc").as_slice(),
            include_bytes!("../../tests/fixtures/draco/cube-valence.drc").as_slice(),
            include_bytes!("../../tests/fixtures/draco/cube-sequential.drc").as_slice(),
        ] {
            let decoded = decode_geometry(bytes).unwrap();
            assert_eq!(decoded.faces.as_ref().unwrap().len(), 12);
            let scene = decode(bytes).unwrap();
            assert_eq!(scene.triangles.len(), 12);
            assert!(
                scene
                    .positions()
                    .all(|p| p.x.abs() <= 1.001 && p.y.abs() <= 1.001 && p.z.abs() <= 1.001)
            );
            assert!(decode(&bytes[..bytes.len() - 1]).is_err());
            let mut trailing = bytes.to_vec();
            trailing.push(42);
            assert!(decode(&trailing).is_err());
        }
    }
    #[test]
    fn independent_google_point_clouds_and_color() {
        let bytes = include_bytes!("../../tests/fixtures/draco/cube-points.drc");
        let scene = decode(bytes).unwrap();
        assert!(!scene.points.is_empty());
        assert!(scene.triangles.is_empty());
        let bytes = include_bytes!("../../tests/fixtures/draco/color-points.drc");
        let scene = decode(bytes).unwrap();
        assert!(scene.points.len() > 100);
        assert!(scene.points.iter().all(|p| p.color.is_some()));
        assert!(
            scene
                .points
                .iter()
                .any(|p| p.color != scene.points[0].color)
        );
    }
    #[test]
    fn mapped_unique_ids_and_all_scalar_storage_types() {
        use draco_core::AttributeValueIndex;
        let mut attribute = PointAttribute::new();
        attribute.init(
            GeometryAttributeType::Position,
            3,
            DataType::Float64,
            false,
            2,
        );
        attribute.set_unique_id(19);
        let bytes: Vec<_> = [1_f64, 2., 3., 4., 5., 6.]
            .into_iter()
            .flat_map(f64::to_le_bytes)
            .collect();
        attribute.buffer_mut().update(&bytes, Some(0));
        attribute.set_explicit_mapping_from(&[AttributeValueIndex(1), AttributeValueIndex(0)]);
        let mut attributes = PointCloud::new();
        attributes.set_num_points(2);
        attributes.add_attribute_preserve_unique_id(attribute);
        let geometry = DecodedGeometry {
            point_count: 2,
            faces: None,
            attributes,
        };
        assert_eq!(
            geometry.attribute(19).unwrap().values,
            [4., 5., 6., 1., 2., 3.]
        );
        assert!(geometry.attribute(0).is_err());
        assert_eq!(
            scalar(DataType::Int64, &(-123_i64).to_le_bytes()).unwrap(),
            -123.
        );
        assert_eq!(
            scalar(DataType::Uint64, &123_u64.to_le_bytes()).unwrap(),
            123.
        );
        assert!(scalar(DataType::Bool, &[2]).is_err());
    }
}
