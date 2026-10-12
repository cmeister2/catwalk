//! Stored, indexed Lychee model meshes with qualified scene placements.
//!
//! This reader deliberately previews the original stored models. Print preparation
//! such as supports, rafts, hollowing and holes is not regenerated. Saved previews
//! remain the preferred automatic image and preserve the application's prepared view.

use super::{lychee_budget::Budget, lychee_scene};
use crate::{
    Result,
    saved_preview::lychee::Container,
    scene::{Scene, Triangle, Vec3},
};
use rmpv::ValueRef;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

/// Maximum scene objects, including hidden/off-scene entries in the association table.
const MAX_OBJECTS: usize = 10_000;
/// Maximum encoded selected mesh, below the aggregate geometry storage ceiling.
const MAX_MESH: usize = 128 * 1024 * 1024;
/// Conservative bounded JSON index and member-span allocation accounting.
const CONTAINER_STORAGE: usize = 32 * 1024 * 1024;
/// Borrowed `MessagePack` map entries, with unique string keys validated by preflight.
type Fields<'a> = [(ValueRef<'a>, ValueRef<'a>)];

/// Load visible, on-scene original model meshes and their saved placements.
///
/// Qualified flat Mango manifest 3.1.0, scene-state version 2 and mesh
/// version 2 are supported. Source positions are already import-centered. The
/// reader applies scale, world-axis X/Y/Z rotations in degrees, authored XY
/// position and a lowest-transformed-vertex Z lift. Preparation objects and
/// preparation metadata do not replace or modify the addressed source meshes.
///
/// # Errors
/// Returns an error for I/O, unsupported/malformed container or scene variants,
/// integrity or `MessagePack` failures, ambiguous/missing model associations,
/// required external/library geometry, invalid meshes/transforms, empty model
/// output, allocation failures, or cumulative resource ceilings.
pub fn load(path: &Path) -> Result<Scene> {
    let data = super::read_file(path)?;
    load_bytes(&data, data.capacity())
}

/// Resolve all selected occurrences and preflight their complete output before emitting triangles.
fn load_bytes(data: &[u8], source_capacity: usize) -> Result<Scene> {
    let mut budget = Budget::default();
    budget.storage(source_capacity, 1)?;
    budget.storage(1, CONTAINER_STORAGE)?;
    let container = Container::parse(data)?;
    let encoded = container
        .member(
            "scene.bin",
            lychee_scene::MAX_SCENE,
            "Lychee scene state exceeds the 16 MiB limit",
        )?
        .ok_or("Lychee stored-model scene has no addressed scene.bin")?;
    let decoded = lychee_scene::decode_bytes(encoded, &mut budget)?;
    let value = lychee_scene::decode(&decoded, &mut budget)?;
    let root = map(&value)?;
    validate_scene(root)?;
    let objects = map(field(map(field(root, "objects")?)?, "present")?)?;
    let by_id = map(field(objects, "byId")?)?;
    let ids = array(field(objects, "allIds")?)?;
    if by_id.len() > MAX_OBJECTS || ids.len() > MAX_OBJECTS || ids.len() != by_id.len() {
        return Err("Lychee object association count exceeds limits or differs from allIds".into());
    }
    budget.storage(by_id.len(), 256)?;
    let mut objects_by_id = HashMap::with_capacity(by_id.len());
    for (id, object) in by_id {
        objects_by_id.insert(text(id)?, object);
    }
    let mut seen = HashSet::with_capacity(ids.len());
    budget.storage(ids.len(), std::mem::size_of::<Placement<'_>>())?;
    let mut placements = Vec::new();
    placements.try_reserve_exact(ids.len())?;
    if placements.capacity() > ids.len() {
        budget.storage(
            placements.capacity() - ids.len(),
            std::mem::size_of::<Placement<'_>>(),
        )?;
    }
    let mut meshes = HashMap::with_capacity(ids.len());
    budget.storage(ids.len(), 256)?;
    for id in ids {
        let id = text(id)?;
        if !seen.insert(id) {
            return Err("duplicate Lychee object allIds association".into());
        }
        let object = map(objects_by_id
            .get(id)
            .ok_or("Lychee allIds references a missing object")?)?;
        if text(field(object, "id")?)? != id {
            return Err("Lychee object ID differs from its association key".into());
        }
        if !selected(object)? {
            continue;
        }
        let mesh = selected_mesh(object, &container, &mut meshes, &mut budget)?;
        let pose = Pose::new(object, mesh, &mut budget)?;
        budget.triangles(mesh.triangles())?;
        placements.push(Placement { mesh, pose });
    }
    emit(&placements, &mut budget)
}

/// Require the scene-state family qualified by original Lychee 5.4.3 files.
fn validate_scene(root: &Fields<'_>) -> Result<()> {
    if field(root, "version")?.as_u64() != Some(2) {
        return Err("unsupported Lychee stored-model scene version".into());
    }
    Ok(())
}

/// Select printing source models, excluding hidden/off-scene and preparation tool objects.
fn selected(object: &Fields<'_>) -> Result<bool> {
    if !boolean(field(object, "visible")?)? || !boolean(field(object, "isOnScene")?)? {
        return Ok(false);
    }
    if optional_bool(object, "isHole")? || optional_bool(object, "isOperationObject")? {
        return Ok(false);
    }
    if text(field(object, "type")?)? != "file" {
        return Err("unsupported Lychee required library or model object type".into());
    }
    if !boolean(field(object, "loaded")?)? || !boolean(field(object, "initialized")?)? {
        return Err("Lychee selected stored model is not loaded and initialized".into());
    }
    Ok(true)
}

/// Resolve a selected exact member stem, caching borrowed mesh validation across instances.
fn selected_mesh<'a>(
    object: &Fields<'_>,
    container: &Container<'a>,
    meshes: &mut HashMap<String, Mesh<'a>>,
    budget: &mut Budget,
) -> Result<Mesh<'a>> {
    let properties = map(field(object, "properties")?)?;
    let hash = text(field(properties, "hash")?)?;
    if hash.is_empty()
        || hash.len() > 252
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("invalid Lychee stored model member association".into());
    }
    if let Some(mesh) = meshes.get(hash) {
        return Ok(*mesh);
    }
    budget.storage(hash.len() * 2 + 4, 1)?;
    let name = format!("{hash}.bin");
    let bytes = container
        .member(
            &name,
            MAX_MESH,
            "Lychee selected mesh exceeds the 128 MiB limit",
        )?
        .ok_or(
            "Lychee selected model has no addressed stored mesh; external geometry is unsupported",
        )?;
    let mesh = Mesh::parse(bytes, budget)?;
    meshes.insert(hash.to_owned(), mesh);
    Ok(mesh)
}

/// Emit only planned stored model triangles, preserving source winding and exact associations.
fn emit(placements: &[Placement<'_>], budget: &mut Budget) -> Result<Scene> {
    let count = placements.iter().try_fold(0_usize, |count, placement| {
        count
            .checked_add(placement.mesh.triangles())
            .ok_or("Lychee output triangle count overflow")
    })?;
    if count == 0 {
        return Err("Lychee scene contains no visible stored model triangles".into());
    }
    let mut scene = Scene::default();
    scene.triangles.try_reserve_exact(count)?;
    if scene.triangles.capacity() > count {
        budget.storage(
            scene.triangles.capacity() - count,
            std::mem::size_of::<Triangle>(),
        )?;
    }
    for placement in placements {
        for corners in placement.mesh.indices.as_chunks::<12>().0 {
            let mut vertices = [Vec3::default(); 3];
            for (vertex, word) in vertices.iter_mut().zip(corners.as_chunks::<4>().0) {
                let index = usize::try_from(u32::from_le_bytes(*word))?;
                *vertex = placement.pose.point(placement.mesh.point(index)?)?;
            }
            scene.triangles.push(Triangle {
                vertices,
                color: None,
            });
        }
    }
    Ok(scene)
}

/// Borrowed validated native arrays; sharing a mesh never duplicates its coordinate storage.
#[derive(Clone, Copy)]
struct Mesh<'a> {
    /// Exact source triangle-list words in little-endian encoding.
    indices: &'a [u8],
    /// Exact source float32 XYZ coordinate words.
    coordinates: &'a [u8],
}

impl<'a> Mesh<'a> {
    /// Check framing, exact counts, complete finite coordinates and every referenced index.
    fn parse(bytes: &'a [u8], budget: &mut Budget) -> Result<Self> {
        let header = bytes
            .get(..20)
            .ok_or("truncated Lychee stored mesh header")?;
        let mut words = [0_u32; 5];
        for (slot, bytes) in words.iter_mut().zip(header.as_chunks::<4>().0) {
            *slot = u32::from_le_bytes(*bytes);
        }
        if words[0] != 2 || words[1] != 12 || words[4] != 0 {
            return Err("unsupported Lychee stored mesh version or header".into());
        }
        let indices = usize::try_from(words[2])?;
        let scalars = usize::try_from(words[3])?;
        if indices == 0 || scalars == 0 || indices % 3 != 0 || scalars % 3 != 0 {
            return Err("Lychee stored mesh counts must describe complete nonempty triangles and XYZ vertices".into());
        }
        let expected = indices
            .checked_add(scalars)
            .and_then(|count| count.checked_mul(4))
            .and_then(|count| count.checked_add(20))
            .ok_or("Lychee stored mesh count overflow")?;
        if bytes.len() != expected {
            return Err("Lychee stored mesh counts differ from its exact byte span".into());
        }
        let vertices = scalars / 3;
        budget.mesh(vertices, indices)?;
        let coordinate_start = 20 + indices * 4;
        let mesh = Self {
            indices: &bytes[20..coordinate_start],
            coordinates: &bytes[coordinate_start..],
        };
        for word in mesh.indices.as_chunks::<4>().0 {
            if usize::try_from(u32::from_le_bytes(*word))? >= vertices {
                return Err("Lychee stored mesh index is outside its vertices".into());
            }
        }
        for index in 0..vertices {
            let _ = mesh.point(index)?;
        }
        Ok(mesh)
    }

    /// Return source triangle count after exact array validation.
    fn triangles(self) -> usize {
        self.indices.len() / 12
    }

    /// Decode one checked native XYZ without allocating a coordinate array.
    fn point(self, index: usize) -> Result<Vec3> {
        let start = index
            .checked_mul(12)
            .ok_or("Lychee coordinate offset overflow")?;
        let end = start
            .checked_add(12)
            .ok_or("Lychee coordinate span overflow")?;
        let bytes = self
            .coordinates
            .get(start..end)
            .ok_or("Lychee coordinate index exceeds stored mesh")?;
        let mut values = [0.; 3];
        for (slot, word) in values.iter_mut().zip(bytes.as_chunks::<4>().0) {
            *slot = f64::from(f32::from_le_bytes(*word));
        }
        let point = Vec3::new(values[0], values[1], values[2]);
        if !point.is_finite() {
            return Err("nonfinite Lychee stored mesh coordinate".into());
        }
        Ok(point)
    }
}

/// One occurrence whose complete source and placement were validated before output reservation.
struct Placement<'a> {
    /// Shared addressed source mesh arrays.
    mesh: Mesh<'a>,
    /// Qualified source-to-world placement.
    pose: Pose,
}

/// Scale, world-axis Euler sines/cosines and authored XY/lowest-vertex-Z translation.
struct Pose {
    /// Per-axis source model scale.
    scale: Vec3,
    /// Sine/cosine pairs for world X, Y and Z rotations in that order.
    rotation: [(f64, f64); 3],
    /// Final XY authored position and actual-vertex-derived Z origin translation.
    translation: Vec3,
}

impl Pose {
    /// Resolve the saved original-model pose without applying import center or preparation state.
    fn new(object: &Fields<'_>, mesh: Mesh<'_>, budget: &mut Budget) -> Result<Self> {
        let scale = vector(field(object, "scale")?)?;
        let angles = vector(field(object, "rotation")?)?;
        let position = vector(field(object, "position")?)?;
        let mut pose = Self {
            scale,
            rotation: [angles.x, angles.y, angles.z].map(|value| value.to_radians().sin_cos()),
            translation: position,
        };
        budget.work(mesh.coordinates.len() / 12)?;
        let mut minimum = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
        let mut maximum = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for index in 0..mesh.coordinates.len() / 12 {
            let point = pose.linear(mesh.point(index)?)?;
            minimum = Vec3::new(
                minimum.x.min(point.x),
                minimum.y.min(point.y),
                minimum.z.min(point.z),
            );
            maximum = Vec3::new(
                maximum.x.max(point.x),
                maximum.y.max(point.y),
                maximum.z.max(point.z),
            );
        }
        pose.translation.z -= minimum.z;
        if !pose.translation.is_finite()
            || !(minimum + pose.translation).is_finite()
            || !(maximum + pose.translation).is_finite()
        {
            return Err("Lychee placement or model Z lift overflows finite coordinates".into());
        }
        Ok(pose)
    }

    /// Apply component scale followed by rotations about the fixed world X, Y and Z axes.
    fn linear(&self, point: Vec3) -> Result<Vec3> {
        let (x, y, z) = (
            point.x * self.scale.x,
            point.y * self.scale.y,
            point.z * self.scale.z,
        );
        let [(sx, cx), (sy, cy), (sz, cz)] = self.rotation;
        let (y, z) = (cx * y - sx * z, sx * y + cx * z);
        let (x, z) = (cy * x + sy * z, -sy * x + cy * z);
        let (x, y) = (cz * x - sz * y, sz * x + cz * y);
        let transformed = Vec3::new(x, y, z);
        if !transformed.is_finite() {
            return Err("nonfinite Lychee scaled or rotated coordinate".into());
        }
        Ok(transformed)
    }

    /// Apply validated linear placement and final authored/lift translation.
    fn point(&self, point: Vec3) -> Result<Vec3> {
        let point = self.linear(point)? + self.translation;
        if !point.is_finite() {
            return Err("nonfinite Lychee placed model coordinate".into());
        }
        Ok(point)
    }
}

/// Require a map without echoing arbitrary source values or private metadata.
fn map<'a, 'b>(value: &'b ValueRef<'a>) -> Result<&'b Fields<'a>> {
    match value {
        ValueRef::Map(map) => Ok(map),
        _ => Err("Lychee required scene field must be a map".into()),
    }
}

/// Require a homogeneous association sequence at its typed use site.
fn array<'a, 'b>(value: &'b ValueRef<'a>) -> Result<&'b [ValueRef<'a>]> {
    match value {
        ValueRef::Array(values) => Ok(values),
        _ => Err("Lychee required scene field must be an array".into()),
    }
}

/// Find one optional field among previously validated unique map keys.
fn optional<'a, 'b>(fields: &'b Fields<'a>, name: &str) -> Option<&'b ValueRef<'a>> {
    fields.iter().find_map(|(key, value)| match key {
        ValueRef::String(key) if key.as_str() == Some(name) => Some(value),
        _ => None,
    })
}

/// Require one known field without placing arbitrary source data in diagnostics.
fn field<'a, 'b>(fields: &'b Fields<'a>, name: &str) -> Result<&'b ValueRef<'a>> {
    optional(fields, name)
        .ok_or_else(|| format!("Lychee scene is missing required {name} field").into())
}

/// Require valid UTF-8 string metadata after structural preflight.
fn text<'a>(value: &'a ValueRef<'_>) -> Result<&'a str> {
    match value {
        ValueRef::String(value) => value
            .as_str()
            .ok_or_else(|| "Lychee string field is not UTF-8".into()),
        _ => Err("Lychee required scene field must be a string".into()),
    }
}

/// Require a Boolean rather than coercing absent, numeric or string flags.
fn boolean(value: &ValueRef<'_>) -> Result<bool> {
    match value {
        ValueRef::Boolean(value) => Ok(*value),
        _ => Err("Lychee scene flag must be Boolean".into()),
    }
}

/// Resolve an optional preparation flag, treating only absence as false.
fn optional_bool(fields: &Fields<'_>, name: &str) -> Result<bool> {
    optional(fields, name).map_or(Ok(false), boolean)
}

/// Require a finite numeric transform component without coercing strings or flags.
fn number(value: &ValueRef<'_>) -> Result<f64> {
    let number = match value {
        ValueRef::Integer(value) => value.as_f64().ok_or("invalid Lychee numeric component")?,
        ValueRef::F32(value) => f64::from(*value),
        ValueRef::F64(value) => *value,
        _ => return Err("Lychee transform component must be numeric".into()),
    };
    if !number.is_finite() {
        return Err("nonfinite Lychee transform component".into());
    }
    Ok(number)
}

/// Require a complete finite XYZ map for a selected object's transform.
fn vector(value: &ValueRef<'_>) -> Result<Vec3> {
    let fields = map(value)?;
    Ok(Vec3::new(
        number(field(fields, "x")?)?,
        number(field(fields, "y")?)?,
        number(field(fields, "z")?)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpv::Value;
    use serde_json::json;

    /// Build a small owned map whose known fields can be changed independently.
    fn fields(values: &[(&str, Value)]) -> Value {
        Value::Map(
            values
                .iter()
                .map(|(key, value)| (Value::from(*key), value.clone()))
                .collect(),
        )
    }

    /// Address one known test field without using the production field resolver.
    fn entry<'a>(value: &'a mut Value, name: &str) -> &'a mut Value {
        let Value::Map(values) = value else {
            panic!("owned test map");
        };
        &mut values
            .iter_mut()
            .find(|(key, _)| key.as_str() == Some(name))
            .unwrap()
            .1
    }

    /// Describe one selected model; arbitrary import-center metadata is unnecessary.
    fn model() -> Value {
        let zero = fields(&[("x", 0.into()), ("y", 0.into()), ("z", 0.into())]);
        let unit = fields(&[("x", 1.into()), ("y", 1.into()), ("z", 1.into())]);
        fields(&[
            ("id", "o1".into()),
            ("visible", true.into()),
            ("isOnScene", true.into()),
            ("type", "file".into()),
            ("loaded", true.into()),
            ("initialized", true.into()),
            ("properties", fields(&[("hash", "model".into())])),
            ("position", zero.clone()),
            ("rotation", zero),
            ("scale", unit),
        ])
    }

    /// Build the complete scene association envelope with an independently selected model.
    fn scene(model: Value) -> Value {
        fields(&[
            ("version", 2.into()),
            (
                "objects",
                fields(&[(
                    "present",
                    fields(&[
                        ("byId", fields(&[("o1", model)])),
                        ("allIds", Value::Array(vec!["o1".into()])),
                    ]),
                )]),
            ),
        ])
    }

    /// Construct an in-memory carrier so library tests need no packaged fixture files.
    fn load_control(scene: &Value) -> Result<Scene> {
        let mut encoded = Vec::new();
        rmpv::encode::write_value(&mut encoded, scene).unwrap();
        // Invert only the byte transport; typed state/mesh validation remains end to end.
        let offsets = lychee_scene::decode_bytes(&vec![0; encoded.len()], &mut Budget::default())?;
        for (byte, offset) in encoded.iter_mut().zip(offsets) {
            *byte = byte.wrapping_sub(offset);
        }
        let mut mesh = Vec::new();
        for word in [2_u32, 12, 3, 9, 0, 0, 1, 2] {
            mesh.extend(word.to_le_bytes());
        }
        for scalar in [0_f32, 0., -2., 2., 0., -2., 0., 3., -2.] {
            mesh.extend(scalar.to_le_bytes());
        }
        let manifest = serde_json::to_vec(&json!({
            "version":"3.1.0", "mangoFiles":{
                "scene.bin":{"offset":"0", "size":encoded.len()},
                "model.bin":{"offset":encoded.len().to_string(), "size":mesh.len()}
            }
        }))
        .unwrap();
        let padded = (manifest.len() + 3) & !3;
        let padded_word = u32::try_from(padded).unwrap();
        let mut bytes = Vec::new();
        for word in [
            4,
            padded_word + 8,
            padded_word + 4,
            u32::try_from(manifest.len()).unwrap(),
        ] {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend(manifest);
        bytes.resize(16 + padded, 0);
        bytes.extend(encoded);
        bytes.extend(mesh);
        load_bytes(&bytes, bytes.capacity())
    }

    #[test]
    fn scene_schema_and_complete_finite_required_transforms_are_enforced() {
        let good = scene(model());
        let loaded = load_control(&good).unwrap();
        assert_eq!(loaded.triangles.len(), 1);
        assert_eq!(loaded.triangles[0].vertices[2], Vec3::new(0., 3., 0.));
        let mut wrong_version = good.clone();
        *entry(&mut wrong_version, "version") = 3.into();
        assert!(
            load_control(&wrong_version)
                .unwrap_err()
                .to_string()
                .contains("scene version")
        );
        for field in ["position", "rotation", "scale"] {
            for bad in [Value::F64(f64::NAN), Value::F64(f64::INFINITY), "1".into()] {
                let mut object = model();
                *entry(entry(&mut object, field), "x") = bad;
                assert!(load_control(&scene(object)).is_err(), "invalid {field}");
            }
            let mut object = model();
            *entry(&mut object, field) = fields(&[("x", 0.into()), ("y", 0.into())]);
            assert!(load_control(&scene(object)).is_err(), "missing {field}.z");
        }
    }

    #[test]
    fn exact_associations_and_required_local_file_objects_are_enforced() {
        for (field, bad) in [
            ("id", "different".into()),
            ("type", "library".into()),
            ("loaded", false.into()),
            ("initialized", false.into()),
            ("visible", 1.into()),
        ] {
            let mut object = model();
            *entry(&mut object, field) = bad;
            assert!(load_control(&scene(object)).is_err(), "invalid {field}");
        }
        for ids in [vec!["missing".into()], vec!["o1".into(), "o1".into()]] {
            let mut state = scene(model());
            *entry(entry(entry(&mut state, "objects"), "present"), "allIds") = Value::Array(ids);
            assert!(load_control(&state).is_err());
        }
        let mut duplicate = scene(model());
        let present = entry(entry(&mut duplicate, "objects"), "present");
        let mut second = model();
        *entry(&mut second, "id") = "o2".into();
        *entry(present, "byId") = fields(&[("o1", model()), ("o2", second)]);
        *entry(present, "allIds") = Value::Array(vec!["o1".into(), "o1".into()]);
        assert!(
            load_control(&duplicate)
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
        let mut object = model();
        *entry(&mut object, "properties") = fields(&[("hash", "missing".into())]);
        assert!(
            load_control(&scene(object))
                .unwrap_err()
                .to_string()
                .contains("addressed stored mesh")
        );
    }

    #[test]
    fn excluded_objects_do_not_require_external_meshes_or_poses() {
        for exclusion in ["visible", "isOnScene", "isHole", "isOperationObject"] {
            let mut ignored = fields(&[
                ("id", "o2".into()),
                ("visible", true.into()),
                ("isOnScene", true.into()),
                ("type", "library".into()),
            ]);
            if matches!(exclusion, "visible" | "isOnScene") {
                *entry(&mut ignored, exclusion) = false.into();
            } else {
                let Value::Map(values) = &mut ignored else {
                    unreachable!()
                };
                values.push((exclusion.into(), true.into()));
            }
            let mut state = scene(model());
            let present = entry(entry(&mut state, "objects"), "present");
            *entry(present, "byId") = fields(&[("o1", model()), ("o2", ignored)]);
            *entry(present, "allIds") = Value::Array(vec!["o1".into(), "o2".into()]);
            assert_eq!(load_control(&state).unwrap().triangles.len(), 1);
        }
    }
}
