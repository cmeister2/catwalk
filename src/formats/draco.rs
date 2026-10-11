//! Pure Rust Draco geometry decoding, also shared with compressed glTF primitives.
use crate::scene::{Point, Scene, Triangle, Vec3};
use crate::{Error, Result};
use draco_core::{
    DataType, DecodeLimits, DecoderBuffer, GeometryAttributeType, Mesh, MeshDecoder,
    PointAttribute, PointCloud, PointCloudDecoder, PointIndex,
};
use std::path::Path;

const MAX_POINTS: usize = 5_000_000;
const MAX_FACES: usize = 5_000_000;
fn error(message: impl Into<String>) -> Error {
    message.into().into()
}

/// Attribute values are raw scalars in decoded point order. `normalized` and
/// `data_type` allow a container to apply its accessor's normalization rules.
pub(super) struct DecodedAttribute {
    pub components: usize,
    pub data_type: DataType,
    pub normalized: bool,
    pub values: Vec<f64>,
}
pub(super) struct DecodedGeometry {
    pub point_count: usize,
    /// `None` identifies a point cloud, including one with three points.
    pub faces: Option<Vec<[usize; 3]>>,
    attributes: PointCloud,
}
impl DecodedGeometry {
    /// Resolve the Draco unique ID declared by KHR_draco_mesh_compression.
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
fn scalar(data_type: DataType, bytes: &[u8]) -> Result<f64> {
    Ok(match data_type {
        DataType::Int8 => bytes[0] as i8 as f64,
        DataType::Uint8 => bytes[0] as f64,
        DataType::Int16 => i16::from_le_bytes(bytes.try_into()?) as f64,
        DataType::Uint16 => u16::from_le_bytes(bytes.try_into()?) as f64,
        DataType::Int32 => i32::from_le_bytes(bytes.try_into()?) as f64,
        DataType::Uint32 => u32::from_le_bytes(bytes.try_into()?) as f64,
        DataType::Int64 => i64::from_le_bytes(bytes.try_into()?) as f64,
        DataType::Uint64 => u64::from_le_bytes(bytes.try_into()?) as f64,
        DataType::Float32 => f32::from_le_bytes(bytes.try_into()?) as f64,
        DataType::Float64 => f64::from_le_bytes(bytes.try_into()?),
        DataType::Bool if bytes[0] <= 1 => bytes[0] as f64,
        _ => return Err(error("invalid Draco scalar data type or boolean")),
    })
}
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
fn color(value: f64, attribute: &DecodedAttribute) -> u8 {
    let factor = match attribute.data_type {
        DataType::Float32 | DataType::Float64 | DataType::Bool => 255.,
        DataType::Uint8 if attribute.normalized => 1.,
        DataType::Uint16 if attribute.normalized => 255. / u16::MAX as f64,
        DataType::Uint32 if attribute.normalized => 255. / u32::MAX as f64,
        DataType::Uint64 if attribute.normalized => 255. / u64::MAX as f64,
        DataType::Int8 if attribute.normalized => 255. / i8::MAX as f64,
        DataType::Int16 if attribute.normalized => 255. / i16::MAX as f64,
        DataType::Int32 if attribute.normalized => 255. / i32::MAX as f64,
        DataType::Int64 if attribute.normalized => 255. / i64::MAX as f64,
        _ => 1.,
    };
    (value * factor).clamp(0., 255.).round() as u8
}
pub(super) fn decode(bytes: &[u8]) -> Result<Scene> {
    let decoded = decode_geometry(bytes)?;
    let positions = decoded
        .semantic_attribute(GeometryAttributeType::Position)?
        .ok_or_else(|| error("Draco has no POSITION attribute"))?;
    if positions.components != 3 {
        return Err(error("Draco POSITION must have three components"));
    }
    let positions: Vec<_> = positions
        .values
        .chunks_exact(3)
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
        for indices in faces {
            let vertices = indices.map(|i| positions[i]);
            let color = colors.as_ref().map(|colors| {
                std::array::from_fn(|component| {
                    (indices
                        .iter()
                        .map(|&i| colors[i][component] as u16)
                        .sum::<u16>()
                        / 3) as u8
                })
            });
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
pub(super) fn load(path: &Path) -> Result<Scene> {
    decode(&super::read_file(path)?)
}

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
