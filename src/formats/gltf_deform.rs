//! Static glTF deformation in authored pose. Morphs precede joint skinning.
use super::{LoadContext, PrimitiveData, decoded_attribute};
use crate::formats::transform::Transform;
use crate::{Result, scene::Vec3};
use ::gltf::{
    Semantic,
    accessor::{DataType, Dimensions, Iter},
};
use std::collections::{BTreeSet, HashSet};

/// Maximum joint influence, morph component and expanded vertex operations per model.
const MAX_DEFORMATION_WORK: usize = 50_000_000;

/// Validate a position accessor layout and required bounds.
///
/// # Errors
/// Returns an error for missing bounds, non-vector layouts or component types not
/// permitted for base positions or morph deltas by the quantization extension.
pub(super) fn position_layout(
    accessor: &::gltf::Accessor<'_>,
    quantized: bool,
    morph: bool,
) -> Result<()> {
    let valid = match accessor.data_type() {
        DataType::F32 => !accessor.normalized(),
        DataType::I8 | DataType::I16 => quantized,
        DataType::U8 | DataType::U16 => quantized && !morph,
        DataType::U32 => false,
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

/// Convert raw position components using the accessor's integer normalization rule.
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

/// Decode bounded position values without changing their source coordinate system.
///
/// # Errors
/// Returns an error for invalid layouts, unreadable data or non-finite components.
/// Accessor byte ranges must already have passed the importer's range validation.
#[allow(
    clippy::needless_pass_by_value,
    reason = "glTF accessors and skins are cheap document handles matching the reader API ownership convention."
)]
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
        DataType::U32 => unreachable!(),
    };
    if positions.iter().any(|v| !v.is_finite()) {
        return Err("glTF position contains non-finite components".into());
    }
    Ok(positions)
}

/// Validate floating point or permitted normalized signed integer normal layouts.
///
/// # Errors
/// Returns an error when dimensions, normalization or component types are unsupported.
pub(super) fn normal_layout(accessor: &::gltf::Accessor<'_>, quantized: bool) -> Result<()> {
    let valid = matches!(
        (accessor.data_type(), accessor.normalized()),
        (DataType::F32, false)
    ) || quantized
        && matches!(
            (accessor.data_type(), accessor.normalized()),
            (DataType::I8 | DataType::I16, true)
        );
    if !valid || accessor.dimensions() != Dimensions::Vec3 {
        return Err(
            "glTF normal accessor requires float32 or permitted normalized int8/int16 VEC3".into(),
        );
    }
    Ok(())
}

/// Apply signed integer normalization without normalizing the vector length.
pub(super) fn normal_vector(values: [f64; 3], accessor: &::gltf::Accessor<'_>) -> Vec3 {
    let values = if accessor.normalized() {
        let scale = match accessor.data_type() {
            DataType::I8 => 127.,
            DataType::I16 => 32767.,
            _ => 1.,
        };
        values.map(|v| (v / scale).max(-1.))
    } else {
        values
    };
    Vec3::new(values[0], values[1], values[2])
}

/// Base normals and morph normal deltas have the same component layouts.
/// Leave their lengths intact until all morphs and transforms are applied.
///
/// # Errors
/// Returns an error for invalid normal layouts, unreadable data or non-finite
/// components. Accessor byte ranges must already have passed range validation.
#[allow(
    clippy::needless_pass_by_value,
    reason = "glTF accessors and skins are cheap document handles matching the reader API ownership convention."
)]
pub(super) fn read_normals(
    accessor: ::gltf::Accessor<'_>,
    buffers: &[Vec<u8>],
    quantized: bool,
) -> Result<Vec<Vec3>> {
    normal_layout(&accessor, quantized)?;
    macro_rules! read {
        ($ty:ty) => {
            Iter::<[$ty; 3]>::new(accessor.clone(), |buffer| {
                buffers.get(buffer.index()).map(Vec::as_slice)
            })
            .ok_or("glTF normal data is unreadable")?
            .map(|v| normal_vector(v.map(f64::from), &accessor))
            .collect()
        };
    }
    let normals: Vec<Vec3> = match accessor.data_type() {
        DataType::F32 => read!(f32),
        DataType::I8 => read!(i8),
        DataType::I16 => read!(i16),
        _ => unreachable!(),
    };
    if normals.iter().any(|v| !v.is_finite()) {
        return Err("glTF normal contains non-finite components".into());
    }
    Ok(normals)
}

/// Convert a finite glTF matrix and require an affine final row.
#[allow(
    clippy::float_cmp,
    reason = "Affine matrix row entries require exact protocol constants."
)]
fn affine(matrix: [[f32; 4]; 4]) -> Result<Transform> {
    let matrix = Transform::from_gltf(matrix)?;
    if (0..3).any(|i| matrix.0[i][3] != 0.) || matrix.0[3][3] != 1. {
        return Err("glTF transform must be affine".into());
    }
    Ok(matrix)
}

/// Compute world transforms for selected roots using evaluated local poses.
///
/// # Errors
/// Returns an error for excessive node counts or depth, shared or cyclic nodes,
/// non-affine local transforms, or non-finite composed matrices.
pub(super) fn global_transforms(
    model: &::gltf::Gltf,
    roots: &[::gltf::Node<'_>],
    poses: &[super::animation::Pose],
) -> Result<Vec<Option<Transform>>> {
    #[allow(
        clippy::needless_pass_by_value,
        clippy::float_cmp,
        reason = "Node handles are cheap glTF reader values; affine row entries require exact protocol constants."
    )]
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
    if model.nodes().len() > 100_000 {
        return Err("glTF exceeds the node limit".into());
    }
    let mut globals = vec![None; model.nodes().len()];
    for node in roots {
        walk(node.clone(), Transform::IDENTITY, 0, &mut globals, poses)?;
    }
    Ok(globals)
}

/// Four joint references and weights per vertex for one paired attribute set.
pub(super) struct InfluenceSet {
    /// Joint indices into the skin's joint list, in vertex order.
    joints: Vec<[u16; 4]>,
    /// Nonnegative joint weights before normalization across all influence sets.
    weights: Vec<[f64; 4]>,
}

/// Decode all paired joint and weight attribute sets for a primitive.
///
/// # Errors
/// Returns an error for missing pairs, incompatible layouts or vertex counts,
/// unreadable data, invalid weights, or excessive influence work.
#[allow(
    clippy::needless_pass_by_value,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Cheap primitive handles follow glTF readers; compressed joint indices are checked as integral u16 values before casting."
)]
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
            for values in decoded.values.as_chunks::<4>().0 {
                if values.iter().any(|v| {
                    !v.is_finite() || *v < 0. || *v > f64::from(u16::MAX) || v.fract() != 0.
                }) {
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
                .as_chunks::<4>()
                .0
                .iter()
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

/// Build joint world transforms composed with inverse bind matrices.
///
/// # Errors
/// Returns an error for excessive joint counts, missing selected-scene joints,
/// invalid inverse bind layouts or matrices, and unreadable matrix data.
#[allow(
    clippy::needless_pass_by_value,
    reason = "glTF accessors and skins are cheap document handles matching the reader API ownership convention."
)]
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

/// Apply normalized linear blend skinning to positions.
///
/// # Errors
/// Returns an error for mismatched influence counts, invalid joint references,
/// non-finite or empty total weights, and non-finite transformed positions.
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

/// Transform a normal by an inverse transpose using scaled cofactors to avoid overflow.
/// Zero normals or singular transforms produce zero for geometric normal fallback.
fn transformed_normal(matrix: Transform, normal: Vec3) -> Result<Vec3> {
    if !normal.is_finite() || matrix.0.iter().flatten().any(|v| !v.is_finite()) {
        return Err("glTF normal transform contains non-finite components".into());
    }
    // Scale before computing cofactors to avoid overflow for large finite
    // transforms. The positive uniform factor disappears on normalization.
    let largest = matrix.0[..3]
        .iter()
        .flat_map(|column| column[..3].iter())
        .map(|value| value.abs())
        .fold(0., f64::max);
    let normal_scale = normal.x.abs().max(normal.y.abs()).max(normal.z.abs());
    if largest == 0. || normal_scale == 0. {
        return Ok(Vec3::new(0., 0., 0.));
    }
    let columns: [Vec3; 3] = std::array::from_fn(|i| {
        let column = matrix.0[i];
        Vec3::new(
            column[0] / largest,
            column[1] / largest,
            column[2] / largest,
        )
    });
    let cofactors = [
        columns[1].cross(columns[2]),
        columns[2].cross(columns[0]),
        columns[0].cross(columns[1]),
    ];
    let determinant = columns[0].dot(cofactors[0]);
    // A singular normal matrix has no inverse. Returning zero lets the caller
    // use the transformed face normal instead of inventing a direction.
    if determinant == 0. {
        return Ok(Vec3::new(0., 0., 0.));
    }
    let normal = Vec3::new(
        normal.x / normal_scale,
        normal.y / normal_scale,
        normal.z / normal_scale,
    );
    let value = (cofactors[0] * normal.x + cofactors[1] * normal.y + cofactors[2] * normal.z)
        * determinant.signum();
    let value_scale = value.x.abs().max(value.y.abs()).max(value.z.abs());
    if value_scale == 0. {
        return Ok(Vec3::new(0., 0., 0.));
    }
    let value = Vec3::new(
        value.x / value_scale,
        value.y / value_scale,
        value.z / value_scale,
    );
    let length = value.length();
    if !length.is_finite() {
        return Err("glTF deformed normal is non-finite".into());
    }
    Ok(value * (1. / length))
}

/// Apply the inverse transpose of each vertex's weighted affine skin matrix or node transform.
/// Normal lengths are normalized only after morphing and all transforms.
///
/// # Errors
/// Returns an error for invalid normals, mismatched influence lengths, invalid
/// weights or joints, excessive influence work, or non-finite transforms.
pub(super) fn apply_normals(
    normals: &[Vec3],
    sets: &[InfluenceSet],
    matrices: Option<&[Transform]>,
    transform: Transform,
) -> Result<Vec<Vec3>> {
    let Some(matrices) = matrices else {
        return normals
            .iter()
            .map(|&normal| transformed_normal(transform, normal))
            .collect();
    };
    if sets.is_empty()
        || sets
            .iter()
            .any(|set| set.joints.len() != normals.len() || set.weights.len() != normals.len())
        || normals.len().saturating_mul(sets.len()).saturating_mul(4) > MAX_DEFORMATION_WORK
    {
        return Err("glTF normal skin has missing, mismatched, or excessive influences".into());
    }
    let mut used = HashSet::new();
    let mut result = Vec::with_capacity(normals.len());
    for (vertex, &normal) in normals.iter().enumerate() {
        let mut sum = 0.;
        used.clear();
        for set in sets {
            for (&joint, &weight) in set.joints[vertex].iter().zip(&set.weights[vertex]) {
                if joint as usize >= matrices.len() {
                    return Err("glTF normal skin references a missing joint".into());
                }
                if !weight.is_finite() || weight < 0. {
                    return Err("glTF normal skin weight must be finite and nonnegative".into());
                }
                if weight > 0. {
                    if !used.insert(joint) {
                        return Err("glTF normal vertex repeats a weighted joint".into());
                    }
                    sum += weight;
                }
            }
        }
        if !sum.is_finite() || sum <= 0. {
            return Err("glTF normal vertex has no positive skin weight".into());
        }
        let mut matrix = Transform::IDENTITY;
        for column in &mut matrix.0[..3] {
            column[..3].fill(0.);
        }
        for set in sets {
            for (&joint, &weight) in set.joints[vertex].iter().zip(&set.weights[vertex]) {
                if weight == 0. {
                    continue;
                }
                let joint = matrices[joint as usize];
                for (column, joint_column) in matrix.0[..3].iter_mut().zip(&joint.0[..3]) {
                    for (value, joint_value) in column[..3].iter_mut().zip(&joint_column[..3]) {
                        *value += joint_value * (weight / sum);
                    }
                }
            }
        }
        // Joint matrices already place the skinned mesh in world coordinates;
        // as for positions, its mesh node's authored transform is excluded.
        result.push(transformed_normal(matrix, normal)?);
    }
    Ok(result)
}

/// Resolve an untyped morph accessor index and require it to exist.
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

/// Decode permitted RGB or RGBA morph color deltas with finite components.
#[allow(
    clippy::needless_pass_by_value,
    reason = "glTF accessors and skins are cheap document handles matching the reader API ownership convention."
)]
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

/// Validate and record a consistent morph target count for every mesh.
///
/// # Errors
/// Returns an error for inconsistent primitive targets, excessive targets, or
/// default mesh weights whose count or components are invalid.
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

/// Apply selected node or mesh morph weights to positions, normals, colors and selected UVs.
///
/// # Errors
/// Returns an error for invalid weights, missing base attributes, unsupported
/// target semantics, incompatible accessor counts or layouts, unreadable data,
/// non-finite results, or exceeded morph work limits.
#[allow(
    clippy::cast_possible_truncation,
    reason = "Morphed color components are clamped to the unit interval before returning to glTF f32 color storage."
)]
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
        .map_or_else(
            || vec![0.; count],
            |v| v.iter().copied().map(f64::from).collect(),
        );
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
            } else if semantic == "NORMAL" {
                let normals = data
                    .normals
                    .as_mut()
                    .ok_or("glTF normal morph has no base NORMAL")?;
                if normals.len() != data.positions.len() {
                    return Err("glTF base normal count differs from position count".into());
                }
                for (normal, delta) in normals.iter_mut().zip(read_normals(
                    accessor,
                    context.buffers,
                    context.quantized,
                )?) {
                    *normal = *normal + delta * weight;
                    if !normal.is_finite() {
                        return Err("glTF morph produced a non-finite normal".into());
                    }
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
            } else if semantic != "TANGENT"
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
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .map(|v| v[i])
                        .fold(f32::INFINITY, f32::min)
                });
                let maximum: [f32; 3] = std::array::from_fn(|i| {
                    values
                        .as_chunks::<3>()
                        .0
                        .iter()
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
    #[allow(
        clippy::cast_possible_truncation,
        reason = "The authored inverse bind test matrices contain small exactly representable f32 values."
    )]
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
                .flat_map(f32::to_le_bytes)
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
    #[allow(
        clippy::cast_possible_truncation,
        reason = "The authored fixture uses small accessor indices and buffer offsets within its allocated test bytes."
    )]
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
    #[allow(
        clippy::cast_possible_truncation,
        reason = "The authored fixture uses small accessor indices and buffer offsets within its allocated test bytes."
    )]
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
            .flat_map(i16::to_le_bytes)
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

    #[test]
    fn normal_inverse_transpose_handles_nonuniform_scale_reflection_and_singular_fallback() {
        let matrix = Transform([
            [0., 2., 0., 0.],
            [-3., 0., 0., 0.],
            [0., 0., 4., 0.],
            [100., 200., 300., 1.],
        ]);
        // Rz(90) * diag(2,3,4): inverse-transpose sends (1,1,0)
        // to (-1/3,1/2,0), then normalization produces (-2,3,0)/sqrt13.
        near(
            apply_normals(&[Vec3::new(1., 1., 0.)], &[], None, matrix).unwrap()[0],
            Vec3::new(-2., 3., 0.) * (1. / 13f64.sqrt()),
        );
        let reflected = Transform([
            [-2., 0., 0., 0.],
            [0., 3., 0., 0.],
            [0., 0., 4., 0.],
            [0., 0., 0., 1.],
        ]);
        near(
            apply_normals(&[Vec3::new(1., 1., 0.)], &[], None, reflected).unwrap()[0],
            Vec3::new(-3., 2., 0.) * (1. / 13f64.sqrt()),
        );
        let mut singular = matrix;
        singular.0[0] = [0.; 4];
        near(
            apply_normals(&[Vec3::new(1., 0., 0.)], &[], None, singular).unwrap()[0],
            Vec3::new(0., 0., 0.),
        );
        near(
            apply_normals(&[Vec3::new(0., 0., 0.)], &[], None, matrix).unwrap()[0],
            Vec3::new(0., 0., 0.),
        );
        let huge = Transform([
            [2e300, 0., 0., 0.],
            [0., 3e300, 0., 0.],
            [0., 0., 4e300, 0.],
            [0., 0., 0., 1.],
        ]);
        near(
            apply_normals(&[Vec3::new(1e300, 1e300, 0.)], &[], None, huge).unwrap()[0],
            Vec3::new(3., 2., 0.) * (1. / 13f64.sqrt()),
        );
    }

    #[test]
    fn normal_skin_uses_inverse_transpose_of_weighted_joint_transform() {
        let first = Transform([
            [2., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [10., 20., 30., 1.],
        ]);
        let second = Transform([
            [1., 0., 0., 0.],
            [0., 3., 0., 0.],
            [0., 0., 1., 0.],
            [-100., -200., -300., 1.],
        ]);
        let set = InfluenceSet {
            joints: vec![[0, 1, 0, 0]],
            weights: vec![[1., 3., 0., 0.]],
        };
        // The weighted affine is diag(1.25,2.5,1). Its inverse-transpose
        // sends (1,1,0) to (0.8,0.4,0), not a blend of unit joint normals.
        let actual = apply_normals(
            &[Vec3::new(1., 1., 0.)],
            &[set],
            Some(&[first, second]),
            Transform([
                [0., 9., 0., 0.],
                [-9., 0., 0., 0.],
                [0., 0., 9., 0.],
                [0., 0., 0., 1.],
            ]),
        )
        .unwrap();
        near(actual[0], Vec3::new(2., 1., 0.) * (1. / 5f64.sqrt()));
        for (joints, weights) in [
            ([0, 0, 0, 0], [1., 1., 0., 0.]),
            ([0, 2, 0, 0], [1., 0., 0., 0.]),
            ([0, 1, 0, 0], [0., 0., 0., 0.]),
            ([0, 1, 0, 0], [-1., 2., 0., 0.]),
            ([0, 1, 0, 0], [f64::NAN, 1., 0., 0.]),
        ] {
            let set = InfluenceSet {
                joints: vec![joints],
                weights: vec![weights],
            };
            assert!(
                apply_normals(
                    &[Vec3::new(1., 0., 0.)],
                    &[set],
                    Some(&[first, second]),
                    Transform::IDENTITY
                )
                .is_err()
            );
        }
        let mismatched = InfluenceSet {
            joints: vec![],
            weights: vec![[1., 0., 0., 0.]],
        };
        assert!(
            apply_normals(
                &[Vec3::new(1., 0., 0.)],
                &[mismatched],
                Some(&[first]),
                Transform::IDENTITY
            )
            .is_err()
        );
    }

    #[test]
    fn normal_morph_is_applied_before_skin_and_final_normalization() {
        let mut fixture = Fixture::new();
        let normal = fixture.floats("VEC3", 3, &[1., 0., 0., 1., 0., 0., 1., 0., 0.]);
        fixture.document["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = json!(normal);
        let delta = fixture.floats("VEC3", 3, &[0., 2., 0., 0., 2., 0., 0., 2., 0.]);
        fixture.document["meshes"][0]["primitives"][0]["targets"] = json!([{"NORMAL":delta}]);
        fixture.document["meshes"][0]["weights"] = json!([0.5]);
        fixture.influences([0, 0, 0, 0], [255, 0, 0, 0], 0);
        let q = std::f32::consts::FRAC_1_SQRT_2;
        fixture.document["nodes"] = json!([
            {"mesh":0,"skin":0,"scale":[7,8,9]},
            {"rotation":[0,0,q,q],"scale":[2,3,4],"translation":[10,20,30]}
        ]);
        fixture.document["scenes"][0]["nodes"] = json!([0, 1]);
        fixture.document["skins"] = json!([{"joints":[1]}]);
        let scene = fixture.load().unwrap();
        let normals = scene.appearances.get(&0).unwrap().normals.unwrap();
        for normal in normals {
            near(normal, Vec3::new(-2., 0., 3.) * (1. / 13f64.sqrt()));
        }
        near(scene.triangles[0].vertices[0], Vec3::new(10., -30., 20.));
    }

    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "The authored fixture uses small accessor indices and buffer offsets within its allocated test bytes."
    )]
    fn quantized_sparse_normal_morph_preserves_component_values() {
        let mut fixture = Fixture::new();
        let normal = fixture.data("VEC3", 5120, 3, &[0, 0, 127, 0].repeat(3), true);
        let view = fixture.document["accessors"][normal]["bufferView"]
            .as_u64()
            .unwrap() as usize;
        fixture.document["bufferViews"][view]["byteStride"] = json!(4);
        fixture.document["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = json!(normal);
        let indices = fixture.view(&[2]);
        let values = fixture.view(
            &[-32768i16, 32767, 0]
                .into_iter()
                .flat_map(i16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        let accessors = fixture.document["accessors"].as_array_mut().unwrap();
        let delta = accessors.len();
        accessors.push(json!({"componentType":5122,"type":"VEC3","count":3,"normalized":true,"sparse":{"count":1,"indices":{"bufferView":indices,"componentType":5121},"values":{"bufferView":values}}}));
        fixture.document["meshes"][0]["primitives"][0]["targets"] = json!([{"NORMAL":delta}]);
        fixture.document["meshes"][0]["weights"] = json!([1.]);
        fixture.document["nodes"][0]["scale"] = json!([2, 3, 4]);
        fixture.document["extensionsUsed"] = json!(["KHR_mesh_quantization"]);
        fixture.document["extensionsRequired"] = json!(["KHR_mesh_quantization"]);
        let scene = fixture.load().unwrap();
        let normals = scene.appearances.get(&0).unwrap().normals.unwrap();
        near(normals[0], Vec3::new(0., -1., 0.));
        near(normals[1], Vec3::new(0., -1., 0.));
        near(normals[2], Vec3::new(-6., -3., 4.) * (1. / 61f64.sqrt()));
        fixture
            .document
            .as_object_mut()
            .unwrap()
            .remove("extensionsRequired");
        assert!(fixture.load().is_err());
        fixture.document["extensionsRequired"] = json!(["KHR_mesh_quantization"]);
        fixture.document["accessors"][delta]["count"] = json!(2);
        assert!(fixture.load().is_err());
    }

    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "The authored fixture uses small accessor indices and offsets within its allocated test bytes."
    )]
    fn normal_morph_requires_base_normals_and_valid_components_even_at_zero_weight() {
        let mut fixture = Fixture::new();
        let delta = fixture.floats("VEC3", 3, &[0., 0., 0., 0., 0., 0., 0., 0., 0.]);
        fixture.document["meshes"][0]["primitives"][0]["targets"] = json!([{"NORMAL":delta}]);
        assert!(
            fixture
                .load()
                .unwrap_err()
                .to_string()
                .contains("base NORMAL")
        );
        let normal = fixture.floats("VEC3", 3, &[0., 0., 1., 0., 0., 1., 0., 0., 1.]);
        fixture.document["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = json!(normal);
        assert!(fixture.load().is_ok());
        fixture.document["accessors"][delta]["normalized"] = json!(true);
        assert!(fixture.load().is_err());
        fixture.document["accessors"][delta]["normalized"] = json!(false);
        let view = fixture.document["accessors"][delta]["bufferView"]
            .as_u64()
            .unwrap() as usize;
        let offset = fixture.document["bufferViews"][view]["byteOffset"]
            .as_u64()
            .unwrap() as usize;
        fixture.bytes[offset..offset + 4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(fixture.load().is_err());
    }

    #[test]
    fn normal_morph_cancellation_keeps_geometry_with_face_normal_fallback() {
        let mut fixture = Fixture::new();
        let normal = fixture.floats("VEC3", 3, &[0., 0., 1., 0., 0., 1., 0., 0., 1.]);
        fixture.document["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = json!(normal);
        let delta = fixture.floats("VEC3", 3, &[0., 0., -1., 0., 0., -1., 0., 0., -1.]);
        fixture.document["meshes"][0]["primitives"][0]["targets"] = json!([{"NORMAL":delta}]);
        fixture.document["meshes"][0]["weights"] = json!([1.]);
        let scene = fixture.load().unwrap();
        assert_eq!(scene.triangles.len(), 1);
        assert!(scene.appearances.get(&0).unwrap().normals.is_none());
        near(scene.triangles[0].vertices[1], Vec3::new(1., 0., 0.));
        near(scene.triangles[0].vertices[2], Vec3::new(0., 0., 1.));
    }

    #[test]
    fn normal_counts_and_empty_sparse_accessors_fail_without_iterator_panics() {
        let mut fixture = Fixture::new();
        let normal = fixture.floats("VEC3", 2, &[0., 0., 1., 0., 0., 1.]);
        fixture.document["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = json!(normal);
        assert!(fixture.load().unwrap_err().to_string().contains("count"));
        fixture.document["accessors"][normal]["count"] = json!(0);
        assert!(fixture.load().is_err());
        fixture.document["accessors"][normal]["count"] = json!(2);
        let indices = fixture.view(&[0]);
        let values = fixture.view(&[0; 12]);
        fixture.document["accessors"][normal]["sparse"] = json!({"count":0,"indices":{"bufferView":indices,"componentType":5121},"values":{"bufferView":values}});
        assert!(fixture.load().is_err());
    }
}
