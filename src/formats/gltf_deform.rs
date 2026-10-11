//! Static glTF deformation in authored pose. Morphs precede joint skinning.
use super::{LoadContext, PrimitiveData, decoded_attribute};
use crate::formats::transform::Transform;
use crate::{Result, scene::Vec3};
use ::gltf::{
    Semantic,
    accessor::{DataType, Dimensions, Iter},
};
use std::collections::{BTreeSet, HashSet};

const MAX_DEFORMATION_WORK: usize = 50_000_000;

pub(super) fn position_layout(
    accessor: &::gltf::Accessor<'_>,
    quantized: bool,
    morph: bool,
) -> Result<()> {
    let valid = match accessor.data_type() {
        DataType::F32 => !accessor.normalized(),
        DataType::I8 | DataType::I16 => quantized,
        DataType::U8 | DataType::U16 => quantized && !morph,
        _ => false,
    };
    if !valid
        || accessor.dimensions() != Dimensions::Vec3
        || accessor.min().is_none()
        || accessor.max().is_none()
    {
        return Err("glTF position accessor requires VEC3 bounds and float32 or permitted quantized components".into());
    }
    Ok(())
}

pub(super) fn position_vector(values: [f64; 3], accessor: &::gltf::Accessor<'_>) -> Vec3 {
    let values = if accessor.normalized() {
        let scale = match accessor.data_type() {
            DataType::I8 => 127.,
            DataType::U8 => 255.,
            DataType::I16 => 32767.,
            DataType::U16 => 65535.,
            _ => 1.,
        };
        values.map(|v| (v / scale).max(-1.))
    } else {
        values
    };
    Vec3::new(values[0], values[1], values[2])
}

pub(super) fn read_positions(
    accessor: ::gltf::Accessor<'_>,
    buffers: &[Vec<u8>],
    quantized: bool,
    morph: bool,
) -> Result<Vec<Vec3>> {
    position_layout(&accessor, quantized, morph)?;
    macro_rules! read {
        ($ty:ty) => {{
            Iter::<[$ty; 3]>::new(accessor.clone(), |buffer| {
                buffers.get(buffer.index()).map(Vec::as_slice)
            })
            .ok_or("glTF position data is unreadable")?
            .map(|v| position_vector(v.map(f64::from), &accessor))
            .collect()
        }};
    }
    let positions: Vec<Vec3> = match accessor.data_type() {
        DataType::F32 => read!(f32),
        DataType::I8 => read!(i8),
        DataType::U8 => read!(u8),
        DataType::I16 => read!(i16),
        DataType::U16 => read!(u16),
        _ => unreachable!(),
    };
    if positions.iter().any(|v| !v.is_finite()) {
        return Err("glTF position contains non-finite components".into());
    }
    Ok(positions)
}

fn affine(matrix: [[f32; 4]; 4]) -> Result<Transform> {
    let matrix = Transform::from_gltf(matrix)?;
    if (0..3).any(|i| matrix.0[i][3] != 0.) || matrix.0[3][3] != 1. {
        return Err("glTF transform must be affine".into());
    }
    Ok(matrix)
}

pub(super) fn global_transforms(
    model: &::gltf::Gltf,
    roots: &[::gltf::Node<'_>],
    poses: &[super::animation::Pose],
) -> Result<Vec<Option<Transform>>> {
    if model.nodes().len() > 100_000 {
        return Err("glTF exceeds the node limit".into());
    }
    fn walk(
        node: ::gltf::Node<'_>,
        parent: Transform,
        depth: usize,
        globals: &mut [Option<Transform>],
        poses: &[super::animation::Pose],
    ) -> Result<()> {
        if depth >= 256 || globals[node.index()].is_some() {
            return Err("glTF scene hierarchy is cyclic, shared, or too deep".into());
        }
        let local = poses[node.index()].transform;
        // Both authored matrices and animated TRS must remain affine.
        if (0..3).any(|i| local.0[i][3] != 0.) || local.0[3][3] != 1. {
            return Err("glTF transform must be affine".into());
        }
        let world = parent.then(local);
        if world.0.iter().flatten().any(|v| !v.is_finite()) {
            return Err("glTF global transform overflow".into());
        }
        globals[node.index()] = Some(world);
        for child in node.children() {
            walk(child, world, depth + 1, globals, poses)?;
        }
        Ok(())
    }
    let mut globals = vec![None; model.nodes().len()];
    for node in roots {
        walk(node.clone(), Transform::IDENTITY, 0, &mut globals, poses)?;
    }
    Ok(globals)
}

pub(super) struct InfluenceSet {
    joints: Vec<[u16; 4]>,
    weights: Vec<[f64; 4]>,
}

pub(super) fn read_influences(
    primitive: ::gltf::mesh::Primitive<'_>,
    buffers: &[Vec<u8>],
    geometry: Option<&crate::formats::draco::DecodedGeometry>,
    count: usize,
) -> Result<Vec<InfluenceSet>> {
    let mut sets = BTreeSet::new();
    for (semantic, _) in primitive.attributes() {
        if let Semantic::Joints(set) | Semantic::Weights(set) = semantic {
            sets.insert(set);
        }
    }
    if sets.len() > 64 || count.saturating_mul(sets.len()).saturating_mul(4) > MAX_DEFORMATION_WORK
    {
        return Err("glTF skin exceeds the influence limit".into());
    }
    let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(Vec::as_slice));
    let compressed = primitive
        .extension_value("KHR_draco_mesh_compression")
        .and_then(|v| v.get("attributes"));
    let mut result = Vec::new();
    for set in sets {
        let joint_accessor = primitive
            .get(&Semantic::Joints(set))
            .ok_or("glTF WEIGHTS set lacks matching JOINTS")?;
        let weight_accessor = primitive
            .get(&Semantic::Weights(set))
            .ok_or("glTF JOINTS set lacks matching WEIGHTS")?;
        if joint_accessor.count() != count
            || weight_accessor.count() != count
            || joint_accessor.dimensions() != Dimensions::Vec4
            || weight_accessor.dimensions() != Dimensions::Vec4
            || !matches!(joint_accessor.data_type(), DataType::U8 | DataType::U16)
            || joint_accessor.normalized()
            || !matches!(
                (weight_accessor.data_type(), weight_accessor.normalized()),
                (DataType::F32, false) | (DataType::U8 | DataType::U16, true)
            )
        {
            return Err("glTF joint/weight accessor has an invalid layout".into());
        }
        let joints = if let Some((geometry, id)) =
            geometry.zip(compressed.and_then(|v| v.get(format!("JOINTS_{set}"))))
        {
            let decoded = decoded_attribute(geometry, id, joint_accessor)?;
            let mut joints = Vec::with_capacity(count);
            for values in decoded.values.chunks_exact(4) {
                if values
                    .iter()
                    .any(|v| !v.is_finite() || *v < 0. || *v > u16::MAX as f64 || v.fract() != 0.)
                {
                    return Err("glTF Draco joint index is invalid".into());
                }
                joints.push(std::array::from_fn(|i| values[i] as u16));
            }
            joints
        } else {
            reader
                .read_joints(set)
                .ok_or("glTF joint data is unreadable")?
                .into_u16()
                .collect()
        };
        let weights = if let Some((geometry, id)) =
            geometry.zip(compressed.and_then(|v| v.get(format!("WEIGHTS_{set}"))))
        {
            let scale = match weight_accessor.data_type() {
                DataType::U8 => 255.,
                DataType::U16 => 65535.,
                _ => 1.,
            };
            decoded_attribute(geometry, id, weight_accessor)?
                .values
                .chunks_exact(4)
                .map(|v| std::array::from_fn(|i| v[i] / scale))
                .collect()
        } else {
            reader
                .read_weights(set)
                .ok_or("glTF weight data is unreadable")?
                .into_f32()
                .map(|v| v.map(f64::from))
                .collect()
        };
        result.push(InfluenceSet { joints, weights });
    }
    Ok(result)
}

pub(super) fn skin_matrices(
    skin: ::gltf::Skin<'_>,
    buffers: &[Vec<u8>],
    globals: &[Option<Transform>],
) -> Result<Vec<Transform>> {
    let joints: Vec<_> = skin.joints().collect();
    if joints.is_empty() || joints.len() > 65_536 {
        return Err("glTF skin has an invalid joint count".into());
    }
    let inverse = if let Some(accessor) = skin.inverse_bind_matrices() {
        if accessor.count() < joints.len()
            || accessor.data_type() != DataType::F32
            || accessor.dimensions() != Dimensions::Mat4
            || accessor.normalized()
        {
            return Err(
                "glTF inverse bind accessor must contain float32 MAT4 values for every joint"
                    .into(),
            );
        }
        skin.reader(|buffer| buffers.get(buffer.index()).map(Vec::as_slice))
            .read_inverse_bind_matrices()
            .ok_or("glTF inverse bind data is unreadable")?
            .take(joints.len())
            .map(affine)
            .collect::<Result<Vec<_>>>()?
    } else {
        vec![Transform::IDENTITY; joints.len()]
    };
    let mut indices = HashSet::new();
    joints
        .iter()
        .zip(inverse)
        .map(|(joint, inverse)| {
            if !indices.insert(joint.index()) {
                return Err("glTF skin repeats a joint".into());
            }
            let matrix = globals[joint.index()]
                .ok_or("glTF skin joint is outside the selected scene")?
                .then(inverse);
            if matrix.0.iter().flatten().any(|v| !v.is_finite()) {
                return Err("glTF skin matrix overflow".into());
            }
            Ok(matrix)
        })
        .collect()
}

pub(super) fn apply_skin(
    positions: Vec<Vec3>,
    sets: &[InfluenceSet],
    matrices: Option<&[Transform]>,
    transform: Transform,
) -> Result<Vec<Vec3>> {
    let Some(matrices) = matrices else {
        return Ok(positions.into_iter().map(|v| transform.apply(v)).collect());
    };
    if sets.is_empty() {
        return Err("glTF skinned mesh has no joint influences".into());
    }
    let mut result = Vec::with_capacity(positions.len());
    let mut used = HashSet::new();
    for (vertex, position) in positions.into_iter().enumerate() {
        let mut sum = 0.;
        let mut world = Vec3::new(0., 0., 0.);
        used.clear();
        for set in sets {
            for (&joint, &weight) in set.joints[vertex].iter().zip(&set.weights[vertex]) {
                let matrix = matrices
                    .get(joint as usize)
                    .ok_or("glTF skin references a missing joint")?;
                if !weight.is_finite() || weight < 0. {
                    return Err("glTF skin weight must be finite and nonnegative".into());
                }
                if weight > 0. {
                    if !used.insert(joint) {
                        return Err("glTF vertex repeats a weighted joint".into());
                    }
                    world = world + matrix.apply(position) * weight;
                    sum += weight;
                }
            }
        }
        if !sum.is_finite() || sum <= 0. {
            return Err("glTF vertex has no positive skin weight".into());
        }
        // The mesh node's transform is intentionally excluded by the glTF skin
        // definition. Joint globals already place the weighted vertex in world.
        result.push(world * (1. / sum));
    }
    Ok(result)
}

fn accessor<'a>(
    context: &LoadContext<'_, 'a>,
    index: &serde_json::Value,
    count: usize,
) -> Result<::gltf::Accessor<'a>> {
    let index = index
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or("glTF morph accessor index is invalid")?;
    let accessor = context
        .accessors
        .get(index)
        .ok_or("glTF morph accessor is missing")?
        .clone();
    if accessor.count() != count {
        return Err("glTF morph accessor count differs from its base attribute".into());
    }
    Ok(accessor)
}

fn color_deltas(accessor: ::gltf::Accessor<'_>, buffers: &[Vec<u8>]) -> Result<Vec<[f64; 4]>> {
    let dimension = accessor.dimensions().multiplicity();
    if !matches!(accessor.dimensions(), Dimensions::Vec3 | Dimensions::Vec4) {
        return Err("glTF color morph must be VEC3 or VEC4".into());
    }
    macro_rules! read {
        ($ty:ty, $size:literal, $scale:expr, $signed:expr) => {{
            Iter::<[$ty; $size]>::new(accessor.clone(), |buffer| {
                buffers.get(buffer.index()).map(Vec::as_slice)
            })
            .ok_or("glTF color morph data is unreadable")?
            .map(|v| {
                std::array::from_fn(|i| {
                    if i >= $size {
                        0.
                    } else {
                        let value = f64::from(v[i]) / $scale;
                        if $signed { value.max(-1.) } else { value }
                    }
                })
            })
            .collect()
        }};
    }
    let normalized = accessor.normalized();
    let values = match (accessor.data_type(), dimension, normalized) {
        (DataType::F32, 3, false) => read!(f32, 3, 1., false),
        (DataType::F32, 4, false) => read!(f32, 4, 1., false),
        (DataType::I8, 3, true) => read!(i8, 3, 127., true),
        (DataType::I8, 4, true) => read!(i8, 4, 127., true),
        (DataType::U8, 3, true) => read!(u8, 3, 255., false),
        (DataType::U8, 4, true) => read!(u8, 4, 255., false),
        (DataType::I16, 3, true) => read!(i16, 3, 32767., true),
        (DataType::I16, 4, true) => read!(i16, 4, 32767., true),
        (DataType::U16, 3, true) => read!(u16, 3, 65535., false),
        (DataType::U16, 4, true) => read!(u16, 4, 65535., false),
        _ => {
            return Err("glTF color morph must use float32 or normalized 8/16-bit integers".into());
        }
    };
    Ok(values)
}

pub(super) fn morph_target_counts(raw: &serde_json::Value) -> Result<Vec<usize>> {
    raw.get("meshes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .map(|mesh| {
            let primitives = mesh["primitives"]
                .as_array()
                .ok_or("glTF mesh primitives are missing")?;
            let count = primitives
                .iter()
                .find_map(|p| p.get("targets").and_then(serde_json::Value::as_array))
                .map_or(0, Vec::len);
            if count > 256
                || primitives
                    .iter()
                    .filter_map(|p| p.get("targets").and_then(serde_json::Value::as_array))
                    .any(|targets| targets.len() != count)
            {
                return Err(
                    "glTF mesh morph targets have inconsistent counts or exceed the limit".into(),
                );
            }
            Ok(count)
        })
        .collect()
}

pub(super) fn apply_morphs(
    node: &::gltf::Node<'_>,
    mesh: &::gltf::Mesh<'_>,
    primitive: usize,
    context: &LoadContext<'_, '_>,
    data: &mut PrimitiveData,
) -> Result<()> {
    let primitives = context.raw["meshes"][mesh.index()]["primitives"]
        .as_array()
        .ok_or("glTF mesh primitives are missing")?;
    let targets = primitives[primitive]
        .get("targets")
        .and_then(serde_json::Value::as_array);
    // A primitive may omit targets entirely: its contribution is zero for
    // every mesh target. Only primitives defining targets must agree in count.
    let count = context.morph_counts[mesh.index()];
    if count > 256 || count.saturating_mul(data.positions.len()) > MAX_DEFORMATION_WORK {
        return Err("glTF morphs exceed the deformation limit".into());
    }
    let weights: Vec<f64> = context.poses[node.index()]
        .weights
        .as_deref()
        .or_else(|| mesh.weights())
        .map(|v| v.iter().copied().map(f64::from).collect())
        .unwrap_or_else(|| vec![0.; count]);
    if weights.len() != count || weights.iter().any(|v| !v.is_finite()) {
        return Err("glTF morph weights must match targets and be finite".into());
    }
    let mut colors = data
        .colors
        .as_ref()
        .map(|values| values.iter().map(|v| v.map(f64::from)).collect::<Vec<_>>());
    for (target, weight) in targets.into_iter().flatten().zip(weights) {
        let target = target
            .as_object()
            .ok_or("glTF morph target must be an attribute object")?;
        for (semantic, index) in target {
            let accessor = accessor(context, index, data.positions.len())?;
            if semantic == "POSITION" {
                let deltas = read_positions(accessor, context.buffers, context.quantized, true)?;
                for (position, delta) in data.positions.iter_mut().zip(deltas) {
                    *position = *position + delta * weight;
                }
            } else if semantic == "COLOR_0" {
                let colors = colors
                    .as_mut()
                    .ok_or("glTF color morph has no base COLOR_0")?;
                if colors.len() != data.positions.len() {
                    return Err("glTF base color count differs from position count".into());
                }
                for (color, delta) in colors
                    .iter_mut()
                    .zip(color_deltas(accessor, context.buffers)?)
                {
                    for i in 0..4 {
                        if !delta[i].is_finite() {
                            return Err("glTF morph contains a non-finite color delta".into());
                        }
                        color[i] += delta[i] * weight;
                    }
                }
            } else if !matches!(semantic.as_str(), "NORMAL" | "TANGENT")
                && !semantic.starts_with("TEXCOORD_")
                && !semantic.starts_with("COLOR_")
                && !semantic.starts_with('_')
            {
                return Err(format!("glTF morph semantic {semantic} is not supported").into());
            }
        }
    }
    data.colors = colors.map(|values| {
        values
            .into_iter()
            .map(|v| v.map(|c| c.clamp(0., 1.) as f32))
            .collect()
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        document: Value,
        bytes: Vec<u8>,
    }
    impl Fixture {
        fn new() -> Self {
            let mut result = Self {
                document: json!({"asset":{"version":"2.0"}, "bufferViews":[], "accessors":[], "meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}], "nodes":[{"mesh":0}], "scenes":[{"nodes":[0]}], "scene":0}),
                bytes: Vec::new(),
            };
            assert_eq!(
                result.floats("VEC3", 3, &[0., 0., 0., 1., 0., 0., 0., 1., 0.]),
                0
            );
            result
        }
        fn view(&mut self, bytes: &[u8]) -> usize {
            while !self.bytes.len().is_multiple_of(4) {
                self.bytes.push(0);
            }
            let views = self.document["bufferViews"].as_array_mut().unwrap();
            let index = views.len();
            views.push(json!({"buffer":0,"byteOffset":self.bytes.len(),"byteLength":bytes.len()}));
            self.bytes.extend(bytes);
            index
        }
        fn data(
            &mut self,
            kind: &str,
            component: u32,
            count: usize,
            bytes: &[u8],
            normalized: bool,
        ) -> usize {
            let view = self.view(bytes);
            let accessors = self.document["accessors"].as_array_mut().unwrap();
            let index = accessors.len();
            let mut accessor =
                json!({"bufferView":view,"componentType":component,"count":count,"type":kind});
            if normalized {
                accessor["normalized"] = json!(true);
            }
            accessors.push(accessor);
            index
        }
        fn floats(&mut self, kind: &str, count: usize, values: &[f32]) -> usize {
            let bytes: Vec<_> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
            let index = self.data(kind, 5126, count, &bytes, false);
            if kind == "VEC3" {
                let minimum: [f32; 3] = std::array::from_fn(|i| {
                    values
                        .chunks_exact(3)
                        .map(|v| v[i])
                        .fold(f32::INFINITY, f32::min)
                });
                let maximum: [f32; 3] = std::array::from_fn(|i| {
                    values
                        .chunks_exact(3)
                        .map(|v| v[i])
                        .fold(f32::NEG_INFINITY, f32::max)
                });
                self.document["accessors"][index]["min"] = json!(minimum);
                self.document["accessors"][index]["max"] = json!(maximum);
            } else if kind == "SCALAR" {
                self.document["accessors"][index]["min"] =
                    json!([values.iter().copied().fold(f32::INFINITY, f32::min)]);
                self.document["accessors"][index]["max"] =
                    json!([values.iter().copied().fold(f32::NEG_INFINITY, f32::max)]);
            }
            index
        }
        fn influences(&mut self, joints: [u8; 4], weights: [u8; 4], set: usize) {
            let joints = self.data("VEC4", 5121, 3, &joints.repeat(3), false);
            let weights = self.data("VEC4", 5121, 3, &weights.repeat(3), true);
            let attributes = &mut self.document["meshes"][0]["primitives"][0]["attributes"];
            attributes[format!("JOINTS_{set}")] = json!(joints);
            attributes[format!("WEIGHTS_{set}")] = json!(weights);
        }
        fn load(&self) -> Result<crate::scene::Scene> {
            let mut document = self.document.clone();
            document["buffers"] = json!([{"byteLength":self.bytes.len(),"uri":format!("data:application/octet-stream;base64,{}",base64::engine::general_purpose::STANDARD.encode(&self.bytes))}]);
            let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "catwalk-gltf-deform-{}-{id}.gltf",
                std::process::id()
            ));
            std::fs::write(&path, serde_json::to_vec(&document)?)?;
            let result = crate::formats::gltf::load(&path);
            let _ = std::fs::remove_file(path);
            result
        }
    }
    fn near(a: Vec3, b: Vec3) {
        assert!((a - b).length() < 1e-6, "{a:?} != {b:?}");
    }

    #[test]
    fn joint_world_and_inverse_bind_ignore_mesh_transform_and_normalize_weights() {
        let mut fixture = Fixture::new();
        fixture.influences([0, 1, 0, 0], [64, 192, 0, 0], 0);
        let mut inverses = Vec::new();
        for translation in [[-2., 0., 0.], [0., -1., 0.], [0., 0., 0.]] {
            let mut matrix = Transform::IDENTITY.0;
            matrix[3][..3].copy_from_slice(&translation);
            inverses.extend(matrix.iter().flatten().map(|&v| v as f32));
        }
        let inverse = fixture.floats("MAT4", 3, &inverses);
        fixture.document["nodes"] = json!([
            {"mesh":0,"skin":0,"translation":[100,200,300],"scale":[3,3,3]},
            {"translation":[10,20,30],"children":[2,3]},
            {"translation":[2,0,0]}, {"translation":[0,4,0]}
        ]);
        fixture.document["scenes"][0]["nodes"] = json!([0, 1]);
        fixture.document["skins"] = json!([{"joints":[2,3],"inverseBindMatrices":inverse}]);
        let scene = fixture.load().unwrap();
        let expected = [
            Vec3::new(10., -30., 22.25),
            Vec3::new(11., -30., 22.25),
            Vec3::new(10., -30., 23.25),
        ];
        for (actual, expected) in scene.triangles[0].vertices.into_iter().zip(expected) {
            near(actual, expected);
        }
    }

    #[test]
    fn all_influence_sets_participate_and_missing_inverse_bind_is_identity() {
        let mut fixture = Fixture::new();
        fixture.influences([0, 1, 0, 0], [32, 32, 0, 0], 0);
        fixture.influences([2, 3, 0, 0], [64, 128, 0, 0], 1);
        fixture.document["nodes"] = json!([{"mesh":0,"skin":0}, {"translation":[0,0,0]}, {"translation":[8,0,0]}, {"translation":[0,8,0]}, {"translation":[0,0,8]}]);
        fixture.document["scenes"][0]["nodes"] = json!([0, 1, 2, 3, 4]);
        fixture.document["skins"] = json!([{"joints":[1,2,3,4]}]);
        let scene = fixture.load().unwrap();
        near(scene.triangles[0].vertices[0], Vec3::new(1., -4., 2.));
        near(scene.triangles[0].vertices[1], Vec3::new(2., -4., 2.));
    }

    #[test]
    fn sparse_morph_uses_node_weight_before_nonuniform_skin() {
        let mut fixture = Fixture::new();
        fixture.influences([0, 0, 0, 0], [255, 0, 0, 0], 0);
        let indices = fixture.view(&[1]);
        let values = fixture.view(
            &[0f32, 2., 0.]
                .into_iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        let morph = fixture.document["accessors"].as_array().unwrap().len();
        fixture.document["accessors"].as_array_mut().unwrap().push(json!({"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[0,2,0],"sparse":{"count":1,"indices":{"bufferView":indices,"componentType":5121},"values":{"bufferView":values}}}));
        fixture.document["meshes"][0]["primitives"][0]["targets"] = json!([{"POSITION":morph}]);
        fixture.document["meshes"][0]["weights"] = json!([0.25]);
        fixture.document["nodes"] = json!([{"mesh":0,"skin":0,"weights":[0.5],"translation":[100,0,0]}, {"translation":[3,4,5],"scale":[2,3,4]}]);
        fixture.document["scenes"][0]["nodes"] = json!([0, 1]);
        fixture.document["skins"] = json!([{"joints":[1]}]);
        let scene = fixture.load().unwrap();
        near(scene.triangles[0].vertices[0], Vec3::new(3., -5., 4.));
        near(scene.triangles[0].vertices[1], Vec3::new(5., -5., 7.));
        near(scene.triangles[0].vertices[2], Vec3::new(3., -5., 7.));
    }

    #[test]
    fn mixed_primitives_preserve_position_and_signed_color_morphs() {
        let mut fixture = Fixture::new();
        let color = fixture.floats("VEC3", 3, &[0.5; 9]);
        let delta = fixture.floats("VEC3", 3, &[2., 0., 0., 2., 0., 0., 2., 0., 0.]);
        let color_delta = fixture.data("VEC3", 5120, 3, &[127u8, 128, 0].repeat(3), true);
        fixture.document["meshes"][0]["primitives"] = json!([
            {"attributes":{"POSITION":0,"COLOR_0":color}},
            {"attributes":{"POSITION":0,"COLOR_0":color},"targets":[{"POSITION":delta,"COLOR_0":color_delta}]}
        ]);
        fixture.document["meshes"][0]["weights"] = json!([0.5]);
        fixture.document["nodes"][0]["translation"] = json!([0, 0, 1]);
        let scene = fixture.load().unwrap();
        assert_eq!(scene.triangles.len(), 2);
        near(scene.triangles[0].vertices[0], Vec3::new(0., -1., 0.));
        near(scene.triangles[1].vertices[0], Vec3::new(1., -1., 0.));
        assert_eq!(scene.triangles[0].color, Some([188, 188, 188]));
        assert_eq!(scene.triangles[1].color, Some([255, 0, 188]));
        fixture.document["meshes"][0]
            .as_object_mut()
            .unwrap()
            .remove("weights");
        let zero = fixture.load().unwrap();
        assert_eq!(zero.triangles[0].vertices, zero.triangles[1].vertices);
        assert_eq!(zero.triangles[0].color, zero.triangles[1].color);
    }

    #[test]
    fn malformed_skin_and_morph_data_fail_explicitly() {
        let mut fixture = Fixture::new();
        fixture.influences([0, 1, 0, 0], [255, 0, 0, 0], 0);
        fixture.document["nodes"] = json!([{"mesh":0,"skin":0}, {}]);
        fixture.document["scenes"][0]["nodes"] = json!([0, 1]);
        fixture.document["skins"] = json!([{"joints":[1]}]);
        assert!(
            fixture
                .load()
                .unwrap_err()
                .to_string()
                .contains("missing joint")
        ); // Zero-weight indices are still checked.
        fixture.document["skins"][0]["joints"] = json!([1, 0]);
        assert!(fixture.load().is_ok());
        fixture.document["scenes"][0]["nodes"] = json!([0]);
        assert!(
            fixture
                .load()
                .unwrap_err()
                .to_string()
                .contains("outside the selected scene")
        );
        fixture.document["scenes"][0]["nodes"] = json!([0, 1]);
        let weights = fixture.document["meshes"][0]["primitives"][0]["attributes"]["WEIGHTS_0"]
            .as_u64()
            .unwrap() as usize;
        let view = fixture.document["accessors"][weights]["bufferView"]
            .as_u64()
            .unwrap() as usize;
        let offset = fixture.document["bufferViews"][view]["byteOffset"]
            .as_u64()
            .unwrap() as usize;
        fixture.bytes[offset..offset + 12].fill(0);
        assert!(
            fixture
                .load()
                .unwrap_err()
                .to_string()
                .contains("no positive skin weight")
        );
        fixture.document["nodes"][0]
            .as_object_mut()
            .unwrap()
            .remove("skin");
        fixture.document["meshes"][0]["primitives"][0]["targets"] = json!([{"POSITION":0}]);
        fixture.document["meshes"][0]["weights"] = json!([1., 2.]);
        assert!(
            fixture
                .load()
                .unwrap_err()
                .to_string()
                .contains("weights must match")
        );
    }

    #[test]
    fn scene_cycles_and_shared_nodes_do_not_recurse() {
        let mut fixture = Fixture::new();
        fixture.document["nodes"][0]["children"] = json!([0]);
        assert!(fixture.load().is_err());
        fixture.document["nodes"] = json!([{"children":[1,1]}, {"mesh":0}]);
        assert!(fixture.load().is_err());
    }

    #[test]
    fn quantized_signed_positions_and_morphs_preserve_authored_dequantization() {
        let mut fixture = Fixture::new();
        let base = fixture.data(
            "VEC3",
            5120,
            3,
            &[128, 0, 0, 0, 127, 0, 0, 0, 0, 127, 0, 0],
            true,
        );
        let view = fixture.document["accessors"][base]["bufferView"]
            .as_u64()
            .unwrap() as usize;
        fixture.document["bufferViews"][view]["byteStride"] = json!(4);
        fixture.document["accessors"][base]["min"] = json!([-128, 0, 0]);
        fixture.document["accessors"][base]["max"] = json!([127, 127, 0]);
        let bytes: Vec<_> = [-2i16, 1, 0, 0]
            .repeat(3)
            .into_iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let delta = fixture.data("VEC3", 5122, 3, &bytes, false);
        let view = fixture.document["accessors"][delta]["bufferView"]
            .as_u64()
            .unwrap() as usize;
        fixture.document["bufferViews"][view]["byteStride"] = json!(8);
        fixture.document["accessors"][delta]["min"] = json!([-2, 1, 0]);
        fixture.document["accessors"][delta]["max"] = json!([-2, 1, 0]);
        fixture.document["meshes"][0]["primitives"][0]["attributes"]["POSITION"] = json!(base);
        fixture.document["meshes"][0]["primitives"][0]["targets"] = json!([{"POSITION":delta}]);
        fixture.document["meshes"][0]["weights"] = json!([0.5]);
        fixture.document["nodes"][0]["translation"] = json!([10, 20, 30]);
        fixture.document["nodes"][0]["scale"] = json!([2, 3, 4]);
        fixture.document["extensionsUsed"] = json!(["KHR_mesh_quantization"]);
        fixture.document["extensionsRequired"] = json!(["KHR_mesh_quantization"]);
        let scene = fixture.load().unwrap();
        for (actual, expected) in scene.triangles[0].vertices.into_iter().zip([
            Vec3::new(6., -30., 21.5),
            Vec3::new(10., -30., 21.5),
            Vec3::new(8., -30., 24.5),
        ]) {
            near(actual, expected);
        }
        fixture
            .document
            .as_object_mut()
            .unwrap()
            .remove("extensionsRequired");
        assert!(fixture.load().is_err());
    }

    #[test]
    fn earliest_animation_drives_skin_and_normalized_morph_weights() {
        let mut fixture = Fixture::new();
        fixture.influences([0, 0, 0, 0], [255, 0, 0, 0], 0);
        let delta = fixture.floats("VEC3", 3, &[2., 0., 0., 2., 0., 0., 2., 0., 0.]);
        fixture.document["meshes"][0]["primitives"][0]["targets"] = json!([{"POSITION":delta}]);
        fixture.document["meshes"][0]["weights"] = json!([0.]);
        fixture.document["nodes"] =
            json!([{"mesh":0,"skin":0,"translation":[100,0,0]}, {"translation":[200,0,0]}]);
        fixture.document["scenes"][0]["nodes"] = json!([0, 1]);
        fixture.document["skins"] = json!([{"joints":[1]}]);
        let times = fixture.floats("SCALAR", 2, &[2., 4.]);
        let translations = fixture.floats("VEC3", 2, &[3., 4., 5., 9., 8., 7.]);
        let later_times = fixture.floats("SCALAR", 2, &[3., 6.]);
        let weights = fixture.data("SCALAR", 5121, 2, &[128, 255], true);
        fixture.document["animations"] = json!([{"samplers":[{"input":times,"output":translations},{"input":later_times,"output":weights}], "channels":[{"sampler":0,"target":{"node":1,"path":"translation"}},{"sampler":1,"target":{"node":0,"path":"weights"}},{"sampler":0,"target":{"path":"translation"}}]}]);
        let scene = fixture.load().unwrap();
        let offset = 3. + 2. * (128. / 255.);
        near(scene.triangles[0].vertices[0], Vec3::new(offset, -5., 4.));
        near(
            scene.triangles[0].vertices[1],
            Vec3::new(offset + 1., -5., 4.),
        );
        near(scene.triangles[0].vertices[2], Vec3::new(offset, -5., 5.));
    }
}
