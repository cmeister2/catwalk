//! Evaluate the first animation at its earliest authored channel timestamp.
//!
//! Every channel is at or before its first key at that time, so the specified
//! endpoint clamping uses its first value for every interpolation mode. Cubic
//! values occupy the middle of each in-tangent/value/out-tangent triple.
use crate::{Result, formats::transform::Transform};
use ::gltf::accessor::{DataType, Dimensions, Iter};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

const MAX_NODES: usize = 100_000;
const MAX_CHANNELS: usize = 400_000;
const MAX_VALUES: usize = 20_000_000;
const MAX_WORK: usize = 50_000_000;

pub(super) struct Pose {
    pub transform: Transform,
    pub weights: Option<Vec<f32>>,
}

#[derive(Clone, Copy)]
struct Trs {
    translation: [f64; 3],
    rotation: [f64; 4],
    scale: [f64; 3],
}
impl Trs {
    fn matrix(self) -> Result<Transform> {
        let [x, y, z, w] = self.rotation;
        let [sx, sy, sz] = self.scale;
        let [tx, ty, tz] = self.translation;
        let matrix = Transform([
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
        ]);
        if matrix.0.iter().flatten().any(|v| !v.is_finite()) {
            return Err("glTF animated transform is non-finite".into());
        }
        Ok(matrix)
    }
}

fn index(value: Option<&Value>, name: &str) -> Result<usize> {
    value
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| format!("invalid glTF animation {name} index").into())
}
fn array<'a>(value: Option<&'a Value>, name: &str) -> Result<&'a [Value]> {
    value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("glTF animation {name} must be an array").into())
}
fn target_count(raw: &Value, node: usize, cache: &mut HashMap<usize, usize>) -> Result<usize> {
    let nodes = array(raw.get("nodes"), "nodes")?;
    let node = nodes.get(node).ok_or("glTF animation node is missing")?;
    let mesh = index(node.get("mesh"), "mesh")?;
    if let Some(&count) = cache.get(&mesh) {
        return Ok(count);
    }
    let mesh_index = mesh;
    let meshes = array(raw.get("meshes"), "meshes")?;
    let mesh = meshes.get(mesh).ok_or("glTF animation mesh is missing")?;
    let primitives = array(mesh.get("primitives"), "primitives")?;
    let mut count = None;
    for primitive in primitives {
        if let Some(targets) = primitive.get("targets") {
            let n = array(Some(targets), "morph targets")?.len();
            if count.is_some_and(|count| n != count) {
                return Err("glTF mesh primitives disagree about morph target count".into());
            }
            count = Some(n);
        }
    }
    let count = count.unwrap_or(0);
    if count == 0 || count > 256 {
        return Err("glTF animated weights require a mesh with bounded morph targets".into());
    }
    cache.insert(mesh_index, count);
    Ok(count)
}

fn quaternion(value: [f32; 4], data_type: DataType) -> Result<[f64; 4]> {
    let value = value.map(f64::from);
    let length = value.iter().map(|v| v * v).sum::<f64>().sqrt();
    // Quantized unit quaternions acquire a small length error on integer decode.
    // Normalize that error, but reject zero or materially non-unit authored keys.
    let tolerance: f64 = match data_type {
        DataType::I8 => 2. / 127.,
        DataType::U8 => 2. / 255.,
        DataType::I16 => 2. / 32767.,
        DataType::U16 => 2. / 65535.,
        _ => 1e-3_f64,
    }
    .max(1e-3);
    if !length.is_finite() || length == 0. || (length - 1.).abs() > tolerance {
        return Err("glTF rotation key must be a finite unit quaternion".into());
    }
    Ok(value.map(|v| v / length))
}

fn scalar_values(accessor: ::gltf::Accessor<'_>, buffers: &[Vec<u8>]) -> Result<Vec<f32>> {
    macro_rules! read {
        ($ty:ty, $scale:expr, $signed:expr) => {
            Iter::<$ty>::new(accessor, |buffer| {
                buffers.get(buffer.index()).map(Vec::as_slice)
            })
            .ok_or("glTF animation output data is unreadable")?
            .map(|v| {
                let value = f32::from(v) / $scale;
                if $signed { value.max(-1.) } else { value }
            })
            .collect()
        };
    }
    Ok(match accessor.data_type() {
        DataType::I8 => read!(i8, 127., true),
        DataType::U8 => read!(u8, 255., false),
        DataType::I16 => read!(i16, 32767., true),
        DataType::U16 => read!(u16, 65535., false),
        DataType::F32 => read!(f32, 1., false),
        _ => return Err("invalid glTF animation output component type".into()),
    })
}
fn output_values(accessor: ::gltf::Accessor<'_>, buffers: &[Vec<u8>]) -> Result<Vec<f32>> {
    if accessor.dimensions() == Dimensions::Scalar {
        return scalar_values(accessor, buffers);
    }
    macro_rules! read {
        ($ty:ty, $size:literal, $scale:expr, $signed:expr) => {
            Iter::<[$ty; $size]>::new(accessor, |buffer| {
                buffers.get(buffer.index()).map(Vec::as_slice)
            })
            .ok_or("glTF animation output data is unreadable")?
            .flat_map(|v| {
                v.map(|c| {
                    let value = f32::from(c) / $scale;
                    if $signed { value.max(-1.) } else { value }
                })
            })
            .collect()
        };
    }
    Ok(match (accessor.dimensions(), accessor.data_type()) {
        (Dimensions::Vec3, DataType::F32) => read!(f32, 3, 1., false),
        (Dimensions::Vec4, DataType::I8) => read!(i8, 4, 127., true),
        (Dimensions::Vec4, DataType::U8) => read!(u8, 4, 255., false),
        (Dimensions::Vec4, DataType::I16) => read!(i16, 4, 32767., true),
        (Dimensions::Vec4, DataType::U16) => read!(u16, 4, 65535., false),
        (Dimensions::Vec4, DataType::F32) => read!(f32, 4, 1., false),
        _ => return Err("invalid glTF animation output layout".into()),
    })
}

/// Return local poses indexed by node index. Mesh weight defaults remain the
/// deformation module's responsibility when neither node nor animation sets them.
pub(super) fn first_pose(
    model: &::gltf::Gltf,
    buffers: &[Vec<u8>],
    raw: &Value,
) -> Result<Vec<Pose>> {
    if model.nodes().len() > MAX_NODES {
        return Err("glTF animation exceeds the node limit".into());
    }
    if model.buffers().len() != buffers.len()
        || model
            .buffers()
            .any(|b| buffers[b.index()].len() < b.length())
    {
        return Err("glTF animation buffer is missing or truncated".into());
    }
    // Iter readers assume their accessor and sparse ranges have been checked.
    // The caller has validated core JSON references before reaching this helper.
    super::check_accessors(model, buffers)?;
    let mut morph_counts = HashMap::new();
    let mut poses = Vec::with_capacity(model.nodes().len());
    let mut decomposed = Vec::with_capacity(model.nodes().len());
    for node in model.nodes() {
        let (transform, trs) = match node.transform() {
            ::gltf::scene::Transform::Matrix { matrix } => {
                if raw["nodes"][node.index()].as_object().is_some_and(|n| {
                    ["translation", "rotation", "scale"]
                        .iter()
                        .any(|p| n.contains_key(*p))
                }) {
                    return Err("glTF node cannot combine matrix and TRS properties".into());
                }
                let matrix = Transform::from_gltf(matrix)?;
                if (0..3).any(|i| matrix.0[i][3] != 0.) || matrix.0[3][3] != 1. {
                    return Err("glTF node transform must be affine".into());
                }
                (matrix, None)
            }
            ::gltf::scene::Transform::Decomposed {
                translation,
                rotation,
                scale,
            } => {
                let trs = Trs {
                    translation: translation.map(f64::from),
                    rotation: quaternion(rotation, DataType::F32)?,
                    scale: scale.map(f64::from),
                };
                (trs.matrix()?, Some(trs))
            }
        };
        let weights = node.weights().map(<[f32]>::to_vec);
        if let Some(weights) = &weights
            && (weights.len() != target_count(raw, node.index(), &mut morph_counts)?
                || weights.iter().any(|v| !v.is_finite()))
        {
            return Err("glTF node morph weights must match targets and be finite".into());
        }
        poses.push(Pose { transform, weights });
        decomposed.push(trs);
    }
    let Some(animations) = raw.get("animations") else {
        return Ok(poses);
    };
    let Some(animation) = array(Some(animations), "animations")?.first() else {
        return Ok(poses);
    };
    let channels = array(animation.get("channels"), "channels")?;
    let samplers = array(animation.get("samplers"), "samplers")?;
    if channels.len() > MAX_CHANNELS || samplers.len() > MAX_CHANNELS {
        return Err("glTF animation exceeds the channel/sampler limit".into());
    }
    let accessors: Vec<_> = model.accessors().collect();
    let mut targets = HashSet::new();
    let mut work = 0usize;
    for channel in channels {
        let target = channel
            .get("target")
            .ok_or("glTF animation lacks a target")?;
        let Some(node) = target.get("node") else {
            // The current core specification permits channels without a target node.
            continue;
        };
        let node = index(Some(node), "node")?;
        let pose = poses
            .get_mut(node)
            .ok_or("glTF animation node index out of bounds")?;
        let path = target
            .get("path")
            .and_then(Value::as_str)
            .ok_or("invalid glTF animation target path")?;
        if !["translation", "scale", "rotation", "weights"].contains(&path) {
            return Err("unsupported glTF animation target property".into());
        }
        if !targets.insert((node, path)) {
            return Err("glTF animation repeats a node/property target".into());
        }
        if path != "weights" && decomposed[node].is_none() {
            return Err("glTF animated TRS node must not define a matrix".into());
        }
        let sampler = samplers
            .get(index(channel.get("sampler"), "sampler")?)
            .ok_or("glTF animation sampler index out of bounds")?;
        let input = accessors
            .get(index(sampler.get("input"), "input accessor")?)
            .ok_or("glTF animation input accessor index out of bounds")?;
        let output = accessors
            .get(index(sampler.get("output"), "output accessor")?)
            .ok_or("glTF animation output accessor index out of bounds")?;
        let interpolation = match sampler.get("interpolation") {
            None => "LINEAR",
            Some(value) => value
                .as_str()
                .ok_or("invalid glTF animation interpolation mode")?,
        };
        let cubic = match interpolation {
            "LINEAR" | "STEP" => false,
            "CUBICSPLINE" => true,
            _ => return Err("invalid glTF animation interpolation mode".into()),
        };
        if input.data_type() != DataType::F32
            || input.dimensions() != Dimensions::Scalar
            || input.normalized()
            || input.count() == 0
            || cubic && input.count() < 2
        {
            return Err(
                "glTF animation times must be nonempty float scalars; cubic requires two keys"
                    .into(),
            );
        }
        let components = match path {
            "translation" | "scale" => 3,
            "rotation" => 4,
            _ => target_count(raw, node, &mut morph_counts)?,
        };
        let dimensions = match path {
            "translation" | "scale" => Dimensions::Vec3,
            "rotation" => Dimensions::Vec4,
            _ => Dimensions::Scalar,
        };
        let valid_type = match path {
            "translation" | "scale" => output.data_type() == DataType::F32 && !output.normalized(),
            _ => matches!(
                (output.data_type(), output.normalized()),
                (DataType::F32, false)
                    | (
                        DataType::I8 | DataType::U8 | DataType::I16 | DataType::U16,
                        true
                    )
            ),
        };
        let frames = input
            .count()
            .checked_mul(if cubic { 3 } else { 1 })
            .ok_or("glTF animation count overflow")?;
        let expected = frames
            .checked_mul(if path == "weights" { components } else { 1 })
            .ok_or("glTF animation count overflow")?;
        let values = frames
            .checked_mul(components)
            .filter(|&n| n <= MAX_VALUES)
            .ok_or("glTF animation exceeds the value limit")?;
        if output.dimensions() != dimensions || output.count() != expected || !valid_type {
            return Err(
                "glTF animation output layout/count disagrees with its target and keys".into(),
            );
        }
        work = work
            .checked_add(input.count())
            .and_then(|n| n.checked_add(values))
            .filter(|&n| n <= MAX_WORK)
            .ok_or("glTF animation exceeds the aggregate work limit")?;
        let times = Iter::<f32>::new(input.clone(), |buffer| {
            buffers.get(buffer.index()).map(Vec::as_slice)
        })
        .ok_or("glTF animation time data is unreadable")?;
        let mut previous = None;
        let mut first_time = None;
        let mut count = 0usize;
        for time in times {
            if !time.is_finite() || time < 0. || previous.is_some_and(|p| time <= p) {
                return Err(
                    "glTF animation times must be finite, nonnegative and strictly increasing"
                        .into(),
                );
            }
            previous = Some(time);
            first_time.get_or_insert(time);
            count += 1;
        }
        if count != input.count() {
            return Err("glTF animation time data is truncated".into());
        }
        for (bound, expected) in [(input.min(), first_time), (input.max(), previous)] {
            let bound = bound.and_then(|v| {
                v.as_array()
                    .filter(|a| a.len() == 1)
                    .and_then(|a| a[0].as_f64())
            });
            if bound.is_none_or(|v| {
                !v.is_finite() || !((v as f32).is_finite()) || Some(v as f32) != expected
            }) {
                return Err("glTF animation time bounds must match its first and last keys".into());
            }
        }
        let values = output_values(output.clone(), buffers)?;
        if values.len() != frames * components || values.iter().any(|v| !v.is_finite()) {
            return Err("glTF animation output data is truncated or non-finite".into());
        }
        let first = usize::from(cubic) * components;
        match path {
            "weights" => pose.weights = Some(values[first..first + components].to_vec()),
            "rotation" => {
                let mut selected = None;
                for (i, key) in values.as_chunks::<4>().0.iter().enumerate() {
                    if !cubic || i % 3 == 1 {
                        let rotation = quaternion(*key, output.data_type())?;
                        if selected.is_none() {
                            selected = Some(rotation);
                        }
                    }
                }
                decomposed[node]
                    .as_mut()
                    .ok_or("glTF animated matrix node")?
                    .rotation = selected.ok_or("glTF rotation animation has no key")?;
            }
            "translation" | "scale" => {
                let value: [f64; 3] = values[first..first + 3]
                    .try_into()
                    .map(|v: [f32; 3]| v.map(f64::from))?;
                let trs = decomposed[node]
                    .as_mut()
                    .ok_or("glTF animated matrix node")?;
                if path == "translation" {
                    trs.translation = value;
                } else {
                    trs.scale = value;
                }
            }
            _ => return Err("invalid glTF animation property".into()),
        }
    }
    for (pose, trs) in poses.iter_mut().zip(decomposed) {
        if let Some(trs) = trs {
            pose.transform = trs.matrix()?;
        }
    }
    Ok(poses)
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
                raw: json!({"asset":{"version":"2.0"},"nodes":[{}],
                    "buffers":[{"uri":"fixture.bin","byteLength":0}],
                    "bufferViews":[],"accessors":[]}),
                bytes: Vec::new(),
            }
        }
        fn view(&mut self, bytes: &[u8]) -> usize {
            while !self.bytes.len().is_multiple_of(4) {
                self.bytes.push(0);
            }
            let views = self.raw["bufferViews"].as_array_mut().unwrap();
            let index = views.len();
            views.push(json!({"buffer":0,"byteOffset":self.bytes.len(),"byteLength":bytes.len()}));
            self.bytes.extend_from_slice(bytes);
            self.raw["buffers"][0]["byteLength"] = json!(self.bytes.len());
            index
        }
        fn accessor(
            &mut self,
            dimensions: &str,
            kind: u32,
            normalized: bool,
            count: usize,
            bytes: &[u8],
        ) -> usize {
            let view = self.view(bytes);
            let accessors = self.raw["accessors"].as_array_mut().unwrap();
            let index = accessors.len();
            accessors.push(json!({"bufferView":view,"componentType":kind,"normalized":normalized,"count":count,"type":dimensions}));
            index
        }
        fn floats(&mut self, dimensions: &str, values: &[f32]) -> usize {
            let components = match dimensions {
                "VEC3" => 3,
                "VEC4" => 4,
                _ => 1,
            };
            let bytes: Vec<_> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
            self.accessor(dimensions, 5126, false, values.len() / components, &bytes)
        }
        fn times(&mut self, values: &[f32]) -> usize {
            let index = self.floats("SCALAR", values);
            self.raw["accessors"][index]["min"] = json!([values.first().copied().unwrap_or(0.)]);
            self.raw["accessors"][index]["max"] = json!([values.last().copied().unwrap_or(0.)]);
            index
        }
        fn channel(
            &mut self,
            node: usize,
            path: &str,
            input: usize,
            output: usize,
            interpolation: &str,
        ) {
            if self.raw.get("animations").is_none() {
                self.raw["animations"] = json!([{"channels":[],"samplers":[]}]);
            }
            let animation = &mut self.raw["animations"][0];
            let samplers = animation["samplers"].as_array_mut().unwrap();
            let sampler = samplers.len();
            samplers.push(json!({"input":input,"output":output,"interpolation":interpolation}));
            animation["channels"]
                .as_array_mut()
                .unwrap()
                .push(json!({"sampler":sampler,"target":{"node":node,"path":path}}));
        }
        fn morphs(&mut self) {
            let position = self.floats("VEC3", &[0., 0., 0.]);
            self.raw["accessors"][position]["min"] = json!([0., 0., 0.]);
            self.raw["accessors"][position]["max"] = json!([0., 0., 0.]);
            self.raw["meshes"] = json!([{"weights":[0.1,0.9],"primitives":[
                {"attributes":{"POSITION":position}},
                {"attributes":{"POSITION":position},"targets":[{"POSITION":position},{"POSITION":position}]}
            ]}]);
            self.raw["nodes"][0]["mesh"] = json!(0);
        }
        fn pose(&self) -> Result<Vec<Pose>> {
            // The loader adapts missing-node channels for gltf-json1.4's older
            // required-node schema, while preserving the original raw document.
            let mut typed = self.raw.clone();
            if let Some(animations) = typed.get_mut("animations").and_then(Value::as_array_mut) {
                for animation in animations {
                    animation["channels"]
                        .as_array_mut()
                        .unwrap()
                        .retain(|c| c["target"].get("node").is_some());
                }
            }
            let model = ::gltf::Gltf::from_slice_without_validation(&serde_json::to_vec(&typed)?)?;
            first_pose(&model, std::slice::from_ref(&self.bytes), &self.raw)
        }
    }
    fn close(actual: Vec3, expected: Vec3) {
        assert!(
            (actual - expected).length() < 1e-6,
            "{actual:?} != {expected:?}"
        );
    }
    #[test]
    fn default_pose_preserves_trs_matrix_and_node_weight_precedence() {
        let mut fixture = Fixture::new();
        fixture.morphs();
        let q = 0.5f32.sqrt();
        fixture.raw["nodes"] = json!([
            {"translation":[1,2,3],"rotation":[0,0,q,q],"scale":[2,3,-4],"mesh":0,"weights":[0.25,0.75]},
            {"matrix":[2,0,0,0, 0,1,0,0, 0,0,1,0, 10,20,30,1]},
            {"mesh":0}
        ]);
        let pose = fixture.pose().unwrap();
        close(
            pose[0].transform.apply(Vec3::new(1., 0., 0.)),
            Vec3::new(1., 4., 3.),
        );
        close(
            pose[0].transform.apply(Vec3::new(0., 1., 0.)),
            Vec3::new(-2., 2., 3.),
        );
        close(
            pose[0].transform.apply(Vec3::new(0., 0., 1.)),
            Vec3::new(1., 2., -1.),
        );
        close(
            pose[1].transform.apply(Vec3::new(1., 0., 0.)),
            Vec3::new(12., 20., 30.),
        );
        assert_eq!(pose[0].weights, Some(vec![0.25, 0.75]));
        assert!(
            pose[2].weights.is_none(),
            "mesh defaults must remain available to deformation"
        );
    }
    #[test]
    fn first_animation_clamps_differing_starts_and_selects_cubic_values() {
        let mut fixture = Fixture::new();
        fixture.morphs();
        fixture.raw["nodes"][0]["translation"] = json!([99, 99, 99]);
        let t = fixture.times(&[2., 6.]);
        let value = fixture.floats("VEC3", &[3., 4., 5., 50., 60., 70.]);
        fixture.channel(0, "translation", t, value, "LINEAR");
        let t = fixture.times(&[7., 9.]);
        let value = fixture.floats("VEC3", &[2., 3., 4., 10., 11., 12.]);
        fixture.channel(0, "scale", t, value, "STEP");
        let t = fixture.times(&[5., 8.]);
        let q = 0.5f32.sqrt();
        let value = fixture.floats(
            "VEC4",
            &[
                10., 20., 30., 40., 0., 0., q, q, -1., -2., -3., -4., 5., 6., 7., 8., 0., 0., 0.,
                1., 0., 0., 0., 0.,
            ],
        );
        fixture.channel(0, "rotation", t, value, "CUBICSPLINE");
        let t = fixture.times(&[1., 8.]);
        let value = fixture.floats(
            "SCALAR",
            &[
                -10., -20., 0.25, 0.75, 11., 22., 100., 200., 0.8, 0.2, -3., -4.,
            ],
        );
        fixture.channel(0, "weights", t, value, "CUBICSPLINE");
        let second_input = fixture.times(&[0.]);
        let second_output = fixture.floats("VEC3", &[100., 200., 300.]);
        fixture.raw["animations"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "samplers":[{"input":second_input,"output":second_output}],
                "channels":[{"sampler":0,"target":{"node":0,"path":"translation"}}]
            }));
        let pose = fixture.pose().unwrap();
        close(
            pose[0].transform.apply(Vec3::new(1., 2., 3.)),
            Vec3::new(-3., 6., 17.),
        );
        assert_eq!(pose[0].weights, Some(vec![0.25, 0.75]));
    }
    #[test]
    fn normalized_integer_rotation_and_weight_outputs_decode_genuine_values() {
        for (kind, maximum, minimum, size) in [
            (5120, 127, -128, 1),
            (5121, 255, 0, 1),
            (5122, 32767, -32768, 2),
            (5123, 65535, 0, 2),
        ] {
            let mut fixture = Fixture::new();
            fixture.morphs();
            let t = fixture.times(&[0.]);
            let n = (f64::from(maximum) * 0.5f64.sqrt()).round() as i32;
            let encode = |values: &[i32]| -> Vec<u8> {
                values
                    .iter()
                    .flat_map(|&v| v.to_le_bytes()[..size].to_vec())
                    .collect()
            };
            let rotation = fixture.accessor("VEC4", kind, true, 1, &encode(&[0, 0, n, n]));
            fixture.channel(0, "rotation", t, rotation, "LINEAR");
            let weights = fixture.accessor("SCALAR", kind, true, 2, &encode(&[minimum, maximum]));
            fixture.channel(0, "weights", t, weights, "STEP");
            let pose = fixture.pose().unwrap();
            close(
                pose[0].transform.apply(Vec3::new(1., 0., 0.)),
                Vec3::new(0., 1., 0.),
            );
            assert_eq!(
                pose[0].weights,
                Some(vec![if minimum < 0 { -1. } else { 0. }, 1.])
            );
        }
    }
    #[test]
    fn rounded_official_simple_skin_rotation_keys_remain_valid() {
        let mut fixture = Fixture::new();
        let input = fixture.times(&[0., 1., 2.]);
        // Khronos SimpleSkin stores these later keys with three-decimal rounding.
        let output = fixture.floats(
            "VEC4",
            &[0., 0., 0., 1., 0., 0., 0.383, 0.924, 0., 0., 0.707, 0.707],
        );
        fixture.channel(0, "rotation", input, output, "LINEAR");
        let pose = fixture.pose().unwrap();
        close(
            pose[0].transform.apply(Vec3::new(1., 2., 3.)),
            Vec3::new(1., 2., 3.),
        );
        let view = fixture.raw["accessors"][output]["bufferView"]
            .as_u64()
            .unwrap() as usize;
        let start = fixture.raw["bufferViews"][view]["byteOffset"]
            .as_u64()
            .unwrap() as usize;
        // A selected rounded value is normalized before matrix composition too.
        fixture.bytes[start..start + 16].copy_from_slice(
            &[0f32, 0., 0.383, 0.924]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        let pose = fixture.pose().unwrap();
        let rotated = pose[0].transform.apply(Vec3::new(1., 0., 0.));
        assert!((rotated.length() - 1.).abs() < 1e-12);
        assert!(rotated.y > 0.7 && rotated.x > 0.7);
    }
    fn translated() -> Fixture {
        let mut fixture = Fixture::new();
        let t = fixture.times(&[0., 1.]);
        let output = fixture.floats("VEC3", &[1., 2., 3., 4., 5., 6.]);
        fixture.channel(0, "translation", t, output, "LINEAR");
        fixture
    }
    #[test]
    fn malformed_keys_counts_types_and_targets_are_errors() {
        for case in 0..13 {
            let mut fixture = translated();
            match case {
                0 => fixture.bytes[4..8].copy_from_slice(&0f32.to_le_bytes()),
                1 => fixture.bytes[..4].copy_from_slice(&(-1f32).to_le_bytes()),
                2 => fixture.bytes[8..12].copy_from_slice(&f32::NAN.to_le_bytes()),
                3 => fixture.bytes[4..8].copy_from_slice(&f32::INFINITY.to_le_bytes()),
                4 => fixture.raw["accessors"][1]["count"] = json!(1),
                5 => fixture.raw["accessors"][1]["normalized"] = json!(true),
                6 => {
                    fixture.raw["accessors"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("min");
                }
                7 => fixture.raw["animations"][0]["samplers"][0]["output"] = json!(999),
                8 => fixture.raw["animations"][0]["channels"][0]["target"]["node"] = json!(999),
                9 => fixture.raw["animations"][0]["channels"][0]["sampler"] = json!(999),
                10 => {
                    let channel = fixture.raw["animations"][0]["channels"][0].clone();
                    fixture.raw["animations"][0]["channels"]
                        .as_array_mut()
                        .unwrap()
                        .push(channel);
                }
                11 => {
                    fixture.raw["nodes"][0]["matrix"] =
                        json!([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1])
                }
                12 => {
                    fixture.bytes.pop();
                }
                _ => unreachable!(),
            }
            assert!(fixture.pose().is_err(), "malformed case {case}");
        }
        for rotation in [[0., 0., 0., 0.], [0., 0., 0., 2.]] {
            let mut fixture = Fixture::new();
            let t = fixture.times(&[0.]);
            let output = fixture.floats("VEC4", &rotation);
            fixture.channel(0, "rotation", t, output, "STEP");
            assert!(fixture.pose().is_err());
        }
        let mut fixture = Fixture::new();
        let t = fixture.times(&[0.]);
        let output = fixture.floats("VEC3", &[0.; 9]);
        fixture.channel(0, "translation", t, output, "CUBICSPLINE");
        assert!(fixture.pose().is_err());
        let mut fixture = Fixture::new();
        fixture.morphs();
        fixture.raw["meshes"][0]["primitives"][0]["targets"] = json!([{"POSITION":0}]);
        fixture.raw["nodes"][0]["weights"] = json!([0., 1.]);
        assert!(fixture.pose().is_err());
    }
    #[test]
    fn sparse_animation_outputs_obey_checked_indices_and_ranges() {
        let mut fixture = Fixture::new();
        let t = fixture.times(&[0., 2.]);
        let indices = fixture.view(&[0]);
        let bytes: Vec<_> = [5f32, 6., 7.]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let values = fixture.view(&bytes);
        let accessors = fixture.raw["accessors"].as_array_mut().unwrap();
        let output = accessors.len();
        accessors.push(json!({"componentType":5126,"type":"VEC3","count":2,"sparse":{
            "count":1,"indices":{"bufferView":indices,"componentType":5121},"values":{"bufferView":values}
        }}));
        fixture.channel(0, "translation", t, output, "LINEAR");
        let pose = fixture.pose().unwrap();
        close(
            pose[0].transform.apply(Vec3::default()),
            Vec3::new(5., 6., 7.),
        );
        fixture.bytes[fixture.raw["bufferViews"][indices]["byteOffset"]
            .as_u64()
            .unwrap() as usize] = 2;
        assert!(fixture.pose().is_err(), "sparse index is outside accessor");
    }
    #[test]
    fn omitted_target_nodes_are_ignored_but_null_is_invalid() {
        let mut fixture = translated();
        fixture.raw["animations"][0]["channels"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"sampler":999,"target":{"path":"translation"}}));
        let pose = fixture.pose().unwrap();
        close(
            pose[0].transform.apply(Vec3::default()),
            Vec3::new(1., 2., 3.),
        );
        fixture.raw["animations"][0]["channels"][0]["target"]["node"] = Value::Null;
        assert!(fixture.pose().is_err());
    }
}
