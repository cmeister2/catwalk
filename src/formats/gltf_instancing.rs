//! Static instances defined by `EXT_mesh_gpu_instancing`.
//!
//! Each matrix is local to the mesh node: the caller composes node-world *
//! instance, after morphing and before converting glTF's coordinate system.
use crate::{Result, formats::transform::Transform};
use ::gltf::accessor::{DataType, Dimensions, Iter};
use serde_json::Value;
use std::collections::HashSet;

/// Maximum static instances expanded for one mesh node.
const MAX_INSTANCES: usize = 100_000;
/// Maximum instance attributes, including custom attributes, inspected per node.
const MAX_ATTRIBUTES: usize = 256;

/// Resolve a buffer view only when its declared and supplied byte ranges agree.
fn view_bytes<'a>(view: &::gltf::buffer::View<'_>, buffers: &'a [Vec<u8>]) -> Result<&'a [u8]> {
    let end = view
        .offset()
        .checked_add(view.length())
        .ok_or("glTF instance buffer view size overflow")?;
    if end > view.buffer().length() {
        return Err("glTF instance buffer view exceeds its declared buffer".into());
    }
    buffers
        .get(view.buffer().index())
        .and_then(|bytes| bytes.get(view.offset()..end))
        .ok_or_else(|| "glTF instance buffer view exceeds input".into())
}

/// Typed core references have already passed glTF validation. Extension
/// accessor references are untyped and are checked here, including sparse data.
fn check_accessor(accessor: &::gltf::Accessor<'_>, buffers: &[Vec<u8>]) -> Result<()> {
    let component_size = accessor.data_type().size();
    if let Some(view) = accessor.view() {
        view_bytes(&view, buffers)?;
        if !accessor.offset().is_multiple_of(component_size)
            || !view.offset().is_multiple_of(component_size)
            || view
                .stride()
                .is_some_and(|stride| !stride.is_multiple_of(component_size))
        {
            return Err("glTF instance accessor is not component-aligned".into());
        }
        super::bounded_range(
            accessor.offset(),
            accessor.count(),
            view.stride().unwrap_or(accessor.size()),
            accessor.size(),
            view.length(),
        )?;
    }
    if let Some(sparse) = accessor.sparse() {
        if sparse.count() == 0 || sparse.count() > accessor.count() {
            return Err("glTF instance sparse count exceeds accessor count".into());
        }
        let indices = sparse.indices();
        let size = indices.index_type().size();
        let index_view = indices.view();
        let bytes = view_bytes(&index_view, buffers)?;
        if index_view.stride().is_some()
            || !indices.offset().is_multiple_of(size)
            || !index_view.offset().is_multiple_of(size)
        {
            return Err("glTF instance sparse indices must not have a byte stride".into());
        }
        super::bounded_range(indices.offset(), sparse.count(), size, size, bytes.len())?;
        let mut previous = None;
        for i in 0..sparse.count() {
            let start = indices.offset() + i * size;
            let index = match size {
                1 => bytes[start] as usize,
                2 => u16::from_le_bytes(bytes[start..start + 2].try_into()?) as usize,
                4 => u32::from_le_bytes(bytes[start..start + 4].try_into()?) as usize,
                _ => return Err("glTF instance sparse index type is invalid".into()),
            };
            if index >= accessor.count() || previous.is_some_and(|v| v >= index) {
                return Err(
                    "glTF instance sparse indices must increase within accessor bounds".into(),
                );
            }
            previous = Some(index);
        }
        let values = sparse.values();
        let value_view = values.view();
        let bytes = view_bytes(&value_view, buffers)?;
        if value_view.stride().is_some()
            || !values.offset().is_multiple_of(component_size)
            || !value_view.offset().is_multiple_of(component_size)
        {
            return Err("glTF instance sparse values must not have a byte stride".into());
        }
        super::bounded_range(
            values.offset(),
            sparse.count(),
            accessor.size(),
            accessor.size(),
            bytes.len(),
        )?;
    }
    Ok(())
}

/// Decode finite, non-normalized float vectors for instance translation or scale.
fn vector_values(accessor: ::gltf::Accessor<'_>, buffers: &[Vec<u8>]) -> Result<Vec<[f64; 3]>> {
    if accessor.dimensions() != Dimensions::Vec3
        || accessor.data_type() != DataType::F32
        || accessor.normalized()
    {
        return Err("glTF instance translation/scale requires non-normalized float32 VEC3".into());
    }
    let values = Iter::<[f32; 3]>::new(accessor, |buffer| {
        buffers.get(buffer.index()).map(Vec::as_slice)
    })
    .ok_or("glTF instance translation/scale data is unreadable")?
    .map(|value| value.map(f64::from))
    .collect::<Vec<_>>();
    if values.iter().flatten().any(|v| !v.is_finite()) {
        return Err("glTF instance translation/scale is non-finite".into());
    }
    Ok(values)
}

/// Require a finite unit quaternion and normalize permitted quantization rounding error.
fn quaternion(value: [f64; 4], data_type: DataType) -> Result<[f64; 4]> {
    let length = value.iter().map(|v| v * v).sum::<f64>().sqrt();
    // Integer unit quaternions accumulate rounding error. Correct that small
    // error, while rejecting zero and materially non-unit authored rotations.
    let tolerance = match data_type {
        DataType::I8 => 2. / 127.,
        DataType::I16 => 2. / 32767.,
        _ => 1e-3_f64,
    }
    .max(1e-3);
    if !length.is_finite() || length == 0. || (length - 1.).abs() > tolerance {
        return Err("glTF instance rotation must be a finite unit quaternion".into());
    }
    Ok(value.map(|v| v / length))
}

/// Decode floating point or normalized signed integer instance rotations.
fn rotation_values(accessor: ::gltf::Accessor<'_>, buffers: &[Vec<u8>]) -> Result<Vec<[f64; 4]>> {
    let data_type = accessor.data_type();
    if accessor.dimensions() != Dimensions::Vec4
        || !matches!(
            (data_type, accessor.normalized()),
            (DataType::F32, false) | (DataType::I8 | DataType::I16, true)
        )
    {
        return Err("glTF instance rotation requires float32 or normalized int8/int16 VEC4".into());
    }
    macro_rules! read {
        ($ty:ty, $scale:expr, $signed:expr) => {
            Iter::<[$ty; 4]>::new(accessor, |buffer| {
                buffers.get(buffer.index()).map(Vec::as_slice)
            })
            .ok_or("glTF instance rotation data is unreadable")?
            .map(|v| {
                quaternion(
                    v.map(|component| {
                        let value = f64::from(component) / $scale;
                        if $signed { value.max(-1.) } else { value }
                    }),
                    data_type,
                )
            })
            .collect::<Result<Vec<_>>>()
        };
    }
    match data_type {
        DataType::F32 => read!(f32, 1., false),
        DataType::I8 => read!(i8, 127., true),
        DataType::I16 => read!(i16, 32767., true),
        _ => unreachable!(),
    }
}

/// Compose a local instance matrix from translation, normalized rotation and signed scale.
fn matrix(translation: [f64; 3], rotation: [f64; 4], scale: [f64; 3]) -> Transform {
    let [x, y, z, w] = rotation;
    let [sx, sy, sz] = scale;
    let [tx, ty, tz] = translation;
    Transform([
        [
            (1. - 2. * (y * y + z * z)) * sx,
            2. * (x * y + z * w) * sx,
            2. * (x * z - y * w) * sx,
            0.,
        ],
        [
            2. * (x * y - z * w) * sy,
            (1. - 2. * (x * x + z * z)) * sy,
            2. * (y * z + x * w) * sy,
            0.,
        ],
        [
            2. * (x * z + y * w) * sz,
            2. * (y * z - x * w) * sz,
            (1. - 2. * (x * x + y * y)) * sz,
            0.,
        ],
        [tx, ty, tz, 1.],
    ])
}

/// Read local mesh instance matrices, returning `None` when the extension is absent.
/// Omitted transform attributes use identity defaults.
///
/// # Errors
/// Returns an error for missing mesh or accessor references, unsupported attribute
/// layouts, mismatched or excessive counts, malformed sparse data, non-finite
/// transforms, or invalid rotation quaternions.
pub(super) fn transforms(
    raw: &Value,
    node: usize,
    accessors: &[::gltf::Accessor<'_>],
    buffers: &[Vec<u8>],
) -> Result<Option<Vec<Transform>>> {
    let node = raw
        .get("nodes")
        .and_then(Value::as_array)
        .and_then(|nodes| nodes.get(node))
        .ok_or("glTF instance node is missing")?;
    let Some(extension) = node
        .get("extensions")
        .and_then(|extensions| extensions.get("EXT_mesh_gpu_instancing"))
    else {
        return Ok(None);
    };
    let mesh = node
        .get("mesh")
        .and_then(Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or("glTF instancing requires a mesh node")?;
    if raw
        .get("meshes")
        .and_then(Value::as_array)
        .and_then(|meshes| meshes.get(mesh))
        .is_none()
    {
        return Err("glTF instance mesh index is out of bounds".into());
    }
    let attributes = extension
        .as_object()
        .and_then(|extension| extension.get("attributes"))
        .and_then(Value::as_object)
        .ok_or("glTF instancing requires an attribute object")?;
    if attributes.is_empty() || attributes.len() > MAX_ATTRIBUTES {
        return Err("glTF instance attributes are empty or exceed the attribute limit".into());
    }
    let mut count = None;
    let mut checked = HashSet::new();
    let mut translation = None;
    let mut rotation = None;
    let mut scale = None;
    for (semantic, index) in attributes {
        if !matches!(semantic.as_str(), "TRANSLATION" | "ROTATION" | "SCALE")
            && !semantic.starts_with('_')
        {
            return Err(format!("unknown glTF instance semantic {semantic}").into());
        }
        let index = index
            .as_u64()
            .and_then(|index| usize::try_from(index).ok())
            .ok_or("glTF instance accessor index must be a nonnegative integer")?;
        let accessor = accessors
            .get(index)
            .ok_or("glTF instance accessor index is out of bounds")?;
        let n = accessor.count();
        if n == 0 || n > MAX_INSTANCES {
            return Err("glTF instance count is empty or exceeds the 100000-instance limit".into());
        }
        if count.is_some_and(|count| count != n) {
            return Err("glTF instance attributes disagree about instance count".into());
        }
        count = Some(n);
        if checked.insert(index) {
            check_accessor(accessor, buffers)?;
        }
        match semantic.as_str() {
            "TRANSLATION" => translation = Some(vector_values(accessor.clone(), buffers)?),
            "ROTATION" => rotation = Some(rotation_values(accessor.clone(), buffers)?),
            "SCALE" => scale = Some(vector_values(accessor.clone(), buffers)?),
            // Custom attributes do not define standardized appearance effects.
            // They still provide a count, even with no transform attributes.
            _ => (),
        }
    }
    let count = count.ok_or("glTF instance count is missing")?;
    let matrices = (0..count)
        .map(|i| {
            matrix(
                translation.as_ref().map_or([0.; 3], |v| v[i]),
                rotation.as_ref().map_or([0., 0., 0., 1.], |v| v[i]),
                scale.as_ref().map_or([1.; 3], |v| v[i]),
            )
        })
        .collect();
    Ok(Some(matrices))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Vec3;
    use serde_json::json;

    struct Fixture {
        raw: Value,
        bytes: Vec<u8>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                raw: json!({
                    "asset":{"version":"2.0"},
                    "nodes":[{"mesh":0,"extensions":{"EXT_mesh_gpu_instancing":{"attributes":{}}}}],
                    "meshes":[{"primitives":[]}],
                    "bufferViews":[],"accessors":[]
                }),
                bytes: Vec::new(),
            }
        }

        fn attribute(
            &mut self,
            semantic: &str,
            component_type: u32,
            dimensions: &str,
            count: usize,
            normalized: bool,
            bytes: &[u8],
        ) -> usize {
            while !self.bytes.len().is_multiple_of(4) {
                self.bytes.push(0);
            }
            let views = self.raw["bufferViews"].as_array_mut().unwrap();
            let view = views.len();
            views.push(json!({"buffer":0,"byteOffset":self.bytes.len(),"byteLength":bytes.len()}));
            self.bytes.extend(bytes);
            let accessors = self.raw["accessors"].as_array_mut().unwrap();
            let accessor = accessors.len();
            accessors.push(json!({"bufferView":view,"componentType":component_type,"type":dimensions,"count":count,"normalized":normalized}));
            self.raw["nodes"][0]["extensions"]["EXT_mesh_gpu_instancing"]["attributes"][semantic] =
                json!(accessor);
            accessor
        }

        fn floats<const N: usize>(&mut self, semantic: &str, values: &[[f32; N]]) -> usize {
            let bytes: Vec<_> = values
                .iter()
                .flatten()
                .flat_map(|value| value.to_le_bytes())
                .collect();
            self.attribute(
                semantic,
                5126,
                if N == 3 { "VEC3" } else { "VEC4" },
                values.len(),
                false,
                &bytes,
            )
        }

        fn read(&self) -> Result<Option<Vec<Transform>>> {
            let mut raw = self.raw.clone();
            raw["buffers"] = json!([{"byteLength":self.bytes.len()}]);
            let document =
                ::gltf::Document::from_json_without_validation(serde_json::from_value(raw)?);
            let accessors = document.accessors().collect::<Vec<_>>();
            transforms(&self.raw, 0, &accessors, std::slice::from_ref(&self.bytes))
        }
    }

    fn close(actual: Vec3, expected: Vec3) {
        assert!(
            (actual.x - expected.x).abs() < 1e-6,
            "x: {} != {}",
            actual.x,
            expected.x
        );
        assert!(
            (actual.y - expected.y).abs() < 1e-6,
            "y: {} != {}",
            actual.y,
            expected.y
        );
        assert!(
            (actual.z - expected.z).abs() < 1e-6,
            "z: {} != {}",
            actual.z,
            expected.z
        );
    }

    #[test]
    fn trs_order_signed_scales_and_parent_world_composition() {
        let mut fixture = Fixture::new();
        fixture.floats("TRANSLATION", &[[4., 5., 6.], [-3., 2., 1.]]);
        let q = std::f32::consts::FRAC_1_SQRT_2;
        fixture.floats("ROTATION", &[[0., 0., q, q], [0., 0., 0., 1.]]);
        fixture.floats("SCALE", &[[2., 3., 4.], [-1., 0., 2.]]);
        let matrices = fixture.read().unwrap().unwrap();
        assert_eq!(matrices.len(), 2);
        close(
            matrices[0].apply(Vec3::new(1., 2., 3.)),
            Vec3::new(-2., 7., 18.),
        );
        close(
            matrices[1].apply(Vec3::new(1., 2., 3.)),
            Vec3::new(-4., 2., 7.),
        );
        let parent = Transform([
            [0., 2., 0., 0.],
            [-2., 0., 0., 0.],
            [0., 0., 2., 0.],
            [10., 20., 30., 1.],
        ]);
        close(
            parent.then(matrices[0]).apply(Vec3::new(1., 2., 3.)),
            Vec3::new(-4., 16., 66.),
        );
    }

    #[test]
    fn defaults_and_custom_only_instances() {
        let mut fixture = Fixture::new();
        fixture.floats("TRANSLATION", &[[1., 2., 3.], [4., 5., 6.]]);
        let matrices = fixture.read().unwrap().unwrap();
        close(
            matrices[1].apply(Vec3::new(7., 8., 9.)),
            Vec3::new(11., 13., 15.),
        );
        let mut custom = Fixture::new();
        custom.attribute("_ID", 5121, "SCALAR", 3, false, &[1, 2, 3]);
        let matrices = custom.read().unwrap().unwrap();
        assert_eq!(matrices.len(), 3);
        for matrix in matrices {
            close(matrix.apply(Vec3::new(7., 8., 9.)), Vec3::new(7., 8., 9.));
        }
        custom.raw["nodes"][0]
            .as_object_mut()
            .unwrap()
            .remove("extensions");
        assert!(custom.read().unwrap().is_none());
    }

    #[test]
    fn signed_normalized_quaternions_correct_quantization_error() {
        for (kind, bytes) in [
            (5120, vec![0, 0, 90, 90]),
            (
                5122,
                [0i16, 0, 23170, 23170]
                    .into_iter()
                    .flat_map(i16::to_le_bytes)
                    .collect(),
            ),
        ] {
            let mut fixture = Fixture::new();
            fixture.attribute("ROTATION", kind, "VEC4", 1, true, &bytes);
            let matrix = fixture.read().unwrap().unwrap()[0];
            close(matrix.apply(Vec3::new(1., 0., 0.)), Vec3::new(0., 1., 0.));
        }
        let mut fixture = Fixture::new();
        fixture.attribute("ROTATION", 5120, "VEC4", 1, true, &[128, 0, 0, 0]);
        close(
            fixture.read().unwrap().unwrap()[0].apply(Vec3::new(0., 1., 0.)),
            Vec3::new(0., -1., 0.),
        );
    }

    #[test]
    fn interleaved_translation_and_sparse_scale_read_actual_data() {
        let mut fixture = Fixture::new();
        let bytes: Vec<_> = [1f32, 2., 3., 999., 4., 5., 6., 999.]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        fixture.attribute("TRANSLATION", 5126, "VEC3", 2, false, &bytes);
        fixture.raw["bufferViews"][0]["byteStride"] = json!(16);
        let scale = fixture.floats("SCALE", &[[1., 1., 1.], [1., 1., 1.]]);
        let index_offset = fixture.bytes.len();
        fixture.bytes.extend([1, 0, 0, 0]);
        let values_offset = fixture.bytes.len();
        fixture
            .bytes
            .extend([2f32, -3., 4.].into_iter().flat_map(f32::to_le_bytes));
        let views = fixture.raw["bufferViews"].as_array_mut().unwrap();
        let indices_view = views.len();
        views.push(json!({"buffer":0,"byteOffset":index_offset,"byteLength":1}));
        let values_view = views.len();
        views.push(json!({"buffer":0,"byteOffset":values_offset,"byteLength":12}));
        fixture.raw["accessors"][scale]["sparse"] = json!({"count":1,"indices":{"bufferView":indices_view,"componentType":5121},"values":{"bufferView":values_view}});
        let matrices = fixture.read().unwrap().unwrap();
        close(
            matrices[0].apply(Vec3::new(1., 1., 1.)),
            Vec3::new(2., 3., 4.),
        );
        close(
            matrices[1].apply(Vec3::new(1., 1., 1.)),
            Vec3::new(6., 2., 10.),
        );
        fixture.bytes[index_offset] = 2;
        assert!(
            fixture
                .read()
                .err()
                .unwrap()
                .to_string()
                .contains("sparse indices")
        );
        fixture.bytes[index_offset] = 1;
        fixture.bytes.pop();
        assert!(fixture.read().is_err());
    }

    #[test]
    fn malformed_counts_semantics_and_references_fail_explicitly() {
        let mut fixture = Fixture::new();
        assert!(fixture.read().is_err());
        fixture.floats("TRANSLATION", &[[1., 2., 3.], [4., 5., 6.]]);
        fixture.attribute("_ID", 5121, "SCALAR", 1, false, &[1]);
        assert!(
            fixture
                .read()
                .err()
                .unwrap()
                .to_string()
                .contains("instance count")
        );
        fixture.raw["nodes"][0]["extensions"]["EXT_mesh_gpu_instancing"]["attributes"]
            .as_object_mut()
            .unwrap()
            .remove("_ID");
        for value in [json!(-1), json!(0.5), json!(99), Value::Null, json!("0")] {
            fixture.raw["nodes"][0]["extensions"]["EXT_mesh_gpu_instancing"]["attributes"]["TRANSLATION"] =
                value;
            assert!(fixture.read().is_err());
        }
        fixture.raw["nodes"][0]["extensions"]["EXT_mesh_gpu_instancing"]["attributes"] =
            json!({"POSITION":0});
        assert!(
            fixture
                .read()
                .err()
                .unwrap()
                .to_string()
                .contains("unknown glTF instance semantic")
        );
        fixture.raw["nodes"][0]["extensions"]["EXT_mesh_gpu_instancing"]["attributes"] =
            json!({"TRANSLATION":0});
        fixture.raw["accessors"][0]["count"] = json!(MAX_INSTANCES + 1);
        assert!(
            fixture
                .read()
                .err()
                .unwrap()
                .to_string()
                .contains("instance limit")
        );
        fixture.raw["accessors"][0]["count"] = json!(0);
        assert!(fixture.read().is_err());
        fixture.raw["accessors"][0]["count"] = json!(2);
        fixture.raw["nodes"][0]
            .as_object_mut()
            .unwrap()
            .remove("mesh");
        assert!(
            fixture
                .read()
                .err()
                .unwrap()
                .to_string()
                .contains("mesh node")
        );
        fixture.raw["nodes"][0]["mesh"] = json!(99);
        assert!(
            fixture
                .read()
                .err()
                .unwrap()
                .to_string()
                .contains("mesh index")
        );
    }

    #[test]
    fn invalid_layouts_nonfinite_values_and_nonunit_rotations_fail() {
        for (semantic, kind, dimensions, normalized, bytes) in [
            ("TRANSLATION", 5126, "VEC4", false, vec![0; 16]),
            ("TRANSLATION", 5126, "VEC3", true, vec![0; 12]),
            ("SCALE", 5123, "VEC3", false, vec![0; 6]),
            ("ROTATION", 5126, "VEC3", false, vec![0; 12]),
            ("ROTATION", 5120, "VEC4", false, vec![0, 0, 0, 127]),
            ("ROTATION", 5121, "VEC4", true, vec![0, 0, 0, 255]),
        ] {
            let mut fixture = Fixture::new();
            fixture.attribute(semantic, kind, dimensions, 1, normalized, &bytes);
            assert!(
                fixture.read().is_err(),
                "{semantic}/{kind}/{dimensions}/{normalized}"
            );
        }
        for values in [
            [0., 0., 0., 0.],
            [0., 0., 0., 2.],
            [f32::NAN, 0., 0., 1.],
            [0., 0., 0., f32::INFINITY],
        ] {
            let mut fixture = Fixture::new();
            fixture.floats("ROTATION", &[values]);
            assert!(fixture.read().is_err());
        }
        for semantic in ["TRANSLATION", "SCALE"] {
            let mut fixture = Fixture::new();
            fixture.floats(semantic, &[[f32::INFINITY, 0., 1.]]);
            assert!(fixture.read().is_err());
        }
        let mut fixture = Fixture::new();
        fixture.floats("TRANSLATION", &[[1., 2., 3.]]);
        fixture.raw["accessors"][0]["byteOffset"] = json!(1);
        assert!(fixture.read().is_err());
        fixture.raw["accessors"][0]["byteOffset"] = json!(0);
        fixture.bytes.pop();
        assert!(fixture.read().is_err());
    }
}
