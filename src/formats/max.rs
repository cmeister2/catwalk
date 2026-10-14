//! Native 3ds Max editable meshes, polygon faces and static Box instances.
//!
//! Geometry comes only from typed Scene references and native mesh buffers.
//! Procedural objects, modifier stacks, animated meshes and unsupported controllers
//! fail explicitly rather than expose an unevaluated base object as final geometry.

mod chunks;

use super::transform::Transform;
use crate::{
    Result,
    scene::{Scene, Triangle, Vec3},
};
use chunks::{Budget, Chunk, float_at, slice, u16_at, u32_at};
use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::Read,
    path::Path,
};

/// Maximum original compound document admitted before indexing.
const MAX_FILE: u64 = 256 * 1024 * 1024;
/// Maximum encoded and decoded size of either selected native stream.
const MAX_STREAM: usize = 64 * 1024 * 1024;
/// Cumulative output triangle limit, including repeated instances.
const MAX_TRIANGLES: usize = 1_000_000;
/// Maximum vertices per selected mesh, before allocation.
const MAX_VERTICES: usize = 1_000_000;
/// Native editable triangle object class identifier.
const EDIT_MESH: u64 = 0xe44f_10b3;
/// Native editable polygon object class identifier.
const EDIT_POLY: u64 = 0x192f_6098_1bf8_338d;
/// Independent three-float position controller class identifier.
const POSITION_XYZ: u64 = 0xffee_238a_118f_7e02;
/// Independent three-float scale controller class identifier.
const SCALE_XYZ: u64 = 0xfeee_238b_118f_7c01;

/// Native class identity resolved through the original class directory.
#[derive(Clone, Copy)]
struct Class {
    /// Two serialized `Class_ID` words joined without changing word order.
    id: u64,
    /// Autodesk superclass determining the object's role in the scene.
    superclass: u32,
}

/// Scene reference graph and its shared import deadline.
struct Document<'a> {
    /// Typed class directory in producer-defined ordinal order.
    classes: Vec<Class>,
    /// Scene objects in producer-defined reference-index order.
    objects: &'a [Chunk<'a>],
    /// Cumulative chunk count and elapsed import time.
    budget: Budget,
}

/// Decode native mesh occurrences without reading thumbnails or external assets.
///
/// # Errors
/// Malformed compound/chunk data, missing references, unsupported final geometry
/// or transforms, and exceeded import limits fail before returning a scene.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let mut budget = Budget::new();
    let file = File::open(path)?;
    if file.metadata()?.len() > MAX_FILE {
        return Err("3ds Max source exceeds the 256 MiB geometry limit".into());
    }
    let mut compound = cfb::CompoundFile::open_strict(file)?;
    if !compound.is_stream("/ClassData") {
        return Err("3ds Max document has no native ClassData stream".into());
    }
    let class_data = stream(&mut compound, "/ClassDirectory3", &budget)?;
    let scene_data = stream(&mut compound, "/Scene", &budget)?;
    let directory = budget.parse(&class_data, 0)?;
    let scene = budget.parse(&scene_data, 0)?;
    if scene.len() != 1 || !matches!(scene[0].id, 0x2004 | 0x2005 | 0x2022 | 0x2023) {
        return Err("unsupported 3ds Max Scene root".into());
    }
    let classes = directory
        .iter()
        .map(|entry| {
            let identity = entry.required(0x2060)?.data;
            if identity.len() != 16 {
                return Err("invalid 3ds Max class identity extent".into());
            }
            Ok(Class {
                id: u64::from_le_bytes(slice(identity, 4, 8)?.try_into()?),
                superclass: u32_at(identity, 12)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Document {
        classes,
        objects: scene[0].list()?,
        budget,
    }
    .scene()
}

/// Read one addressed native stream and, when present, its gzip wrapper.
fn stream(compound: &mut cfb::CompoundFile<File>, name: &str, budget: &Budget) -> Result<Vec<u8>> {
    budget.check()?;
    let entry = compound.entry(name)?;
    if entry.len() > MAX_STREAM as u64 {
        return Err("3ds Max selected native stream exceeds its 64 MiB limit".into());
    }
    let mut bytes = Vec::new();
    compound
        .open_stream(name)?
        .take(MAX_STREAM as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != entry.len() {
        return Err("invalid 3ds Max native stream length".into());
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoded = Vec::new();
        let mut gzip = flate2::bufread::GzDecoder::new(bytes.as_slice());
        gzip.by_ref()
            .take(MAX_STREAM as u64 + 1)
            .read_to_end(&mut decoded)?;
        if decoded.len() > MAX_STREAM || !gzip.into_inner().is_empty() {
            return Err("3ds Max compressed stream exceeds its limit or has trailing bytes".into());
        }
        budget.check()?;
        bytes = decoded;
    }
    Ok(bytes)
}

impl<'a> Document<'a> {
    /// Resolve a scene-object identity without guessing from payload signatures.
    fn class(&self, object: &Chunk<'a>) -> Result<Class> {
        self.classes
            .get(usize::from(object.id))
            .copied()
            .ok_or_else(|| format!("unsupported 3ds Max scene class {:04x}", object.id).into())
    }

    /// Select an object through a validated, non-null native reference.
    fn object(&self, index: usize) -> Result<&Chunk<'a>> {
        self.objects
            .get(index)
            .ok_or_else(|| "3ds Max scene reference is out of range".into())
    }

    /// Normalize the ordered and keyed native reference-table encodings.
    fn references(&self, object: &Chunk<'a>) -> Result<BTreeMap<u32, usize>> {
        let mut references = BTreeMap::new();
        if let Some(keyed) = object.child(0x2035)? {
            if keyed.data.len() < 4 || (keyed.data.len() - 4) % 8 != 0 {
                return Err("invalid 3ds Max keyed reference table".into());
            }
            let count = u32_at(keyed.data, 0)?;
            for record in keyed.data[4..].as_chunks::<8>().0 {
                let key = u32_at(record, 0)?;
                if key >= count
                    || references
                        .insert(key, usize::try_from(u32_at(record, 4)?)?)
                        .is_some()
                {
                    return Err("invalid or duplicate 3ds Max keyed reference".into());
                }
            }
        } else if let Some(ordered) = object.child(0x2034)? {
            if ordered.data.len() % 4 != 0 {
                return Err("invalid 3ds Max reference table".into());
            }
            for (key, record) in ordered.data.as_chunks::<4>().0.iter().enumerate() {
                references.insert(u32::try_from(key)?, usize::try_from(u32_at(record, 0)?)?);
            }
        }
        references.retain(|_, index| *index != u32::MAX as usize);
        for &index in references.values() {
            self.object(index)?;
        }
        Ok(references)
    }

    /// Require one particular scene reference by its semantic slot.
    fn reference(&self, object: &Chunk<'a>, slot: u32) -> Result<&Chunk<'a>> {
        let references = self.references(object)?;
        self.object(
            *references
                .get(&slot)
                .ok_or("missing 3ds Max scene reference")?,
        )
    }

    /// Emit each visible native mesh occurrence at its authored world placement.
    fn scene(self) -> Result<Scene> {
        let mut scene = Scene::default();
        for (index, node) in self.objects.iter().enumerate() {
            self.budget.check()?;
            let Ok(class) = self.class(node) else {
                continue;
            };
            if class.id != 1 || class.superclass != 1 {
                continue;
            }
            if let Some(flags) = node.child(0x963)?
                && u32_at(flags.data, 0)? & 0x40 != 0
            {
                continue;
            }
            let object = self.reference(node, 1)?;
            let class = self.class(object)?;
            // Cameras, lights and helpers carry placements but have no model surfaces.
            if class.superclass != 0x10 && class.superclass != 0x40 {
                continue;
            }
            if class.id == 0x1020 {
                continue;
            } // Camera target.
            let placement = self.world(index, &mut HashSet::new())?.then(offset(node)?);
            let triangles = match class.id {
                EDIT_MESH => {
                    self.static_mesh(object, 0)?;
                    triangle_mesh(object, &self.budget)?
                }
                EDIT_POLY => {
                    self.static_mesh(object, 1)?;
                    polygon_mesh(object, &self.budget)?
                }
                0x10 => self.box_mesh(object)?,
                _ => {
                    return Err(format!(
                        "unsupported 3ds Max final geometry class {:016x}",
                        class.id
                    )
                    .into());
                }
            };
            if scene.triangles.len().saturating_add(triangles.len()) > MAX_TRIANGLES {
                return Err("3ds Max placed triangles exceed the geometry limit".into());
            }
            let color = node
                .child(0x974)?
                .map(|chunk| {
                    let [red, green, blue, _] = u32_at(chunk.data, 0)?.to_le_bytes();
                    Ok::<_, crate::Error>([red, green, blue])
                })
                .transpose()?;
            let mirrored = determinant(placement) < 0.;
            for vertices in triangles {
                self.budget.check()?;
                let mut vertices = vertices.map(|v| placement.apply(v));
                if mirrored {
                    vertices.swap(1, 2);
                }
                scene.triangles.push(Triangle { vertices, color });
            }
        }
        if scene.triangles.is_empty() {
            return Err("3ds Max document contains no supported visible mesh geometry".into());
        }
        Ok(scene)
    }

    /// Require the mesh's native point controller to have no animated vertex targets.
    fn static_mesh(&self, object: &Chunk<'a>, slot: u32) -> Result<()> {
        let controller = self.reference(object, slot)?;
        if self.class(controller)?.id != 0x0d9c_20ff || !self.references(controller)?.is_empty() {
            return Err("animated 3ds Max mesh vertices are unsupported".into());
        }
        Ok(())
    }

    /// Resolve local PRS placements through the complete ancestor chain.
    fn world(&self, index: usize, visiting: &mut HashSet<usize>) -> Result<Transform> {
        self.budget.check()?;
        if !visiting.insert(index) || visiting.len() > 64 {
            return Err("cyclic or excessive 3ds Max node hierarchy".into());
        }
        let node = self.object(index)?;
        let class = self.class(node)?;
        let world = if class.id == 2 && class.superclass == 1 {
            Transform::IDENTITY
        } else if class.id == 1 && class.superclass == 1 {
            let parent = node.required(0x960)?.data;
            if parent.len() != 8 {
                return Err("invalid 3ds Max parent reference".into());
            }
            self.world(usize::try_from(u32_at(parent, 0)?)?, visiting)?
                .then(self.prs(self.reference(node, 0)?)?)
        } else {
            return Err("3ds Max node parent is not a native scene node".into());
        };
        visiting.remove(&index);
        Ok(world)
    }

    /// Decode the constant position/rotation/scale controller family.
    fn prs(&self, controller: &Chunk<'a>) -> Result<Transform> {
        if self.class(controller)?.id != 0x2005 {
            return Err("unsupported 3ds Max transform controller".into());
        }
        let position = self.reference(controller, 0)?;
        let rotation = self.reference(controller, 1)?;
        let scale = self.reference(controller, 2)?;
        let p = self.xyz(position, POSITION_XYZ)?;
        let r = if self.class(rotation)?.id == 0x2012 {
            if u32_at(rotation.required(0x1003)?.data, 0)? != 0 {
                return Err("unsupported 3ds Max Euler axis order".into());
            }
            euler(self.axis_values(rotation)?)
        } else {
            return Err("unsupported 3ds Max rotation controller".into());
        };
        let s = if self.class(scale)?.id == SCALE_XYZ {
            scaling(self.axis_values(scale)?, [0., 0., 0., 1.])?
        } else if self.class(scale)?.id == 0x2010 {
            keyless(scale)?;
            let values = scale.required(0x2505)?.data;
            if values.len() != 28 {
                return Err("invalid 3ds Max scale value".into());
            }
            scaling(vector(values, 0)?, quaternion(values, 12)?)?
        } else {
            return Err("unsupported 3ds Max scale controller".into());
        };
        Ok(translation(p).then(r).then(s))
    }

    /// Read one qualified axis family rather than assuming missing components are zero.
    fn xyz(&self, controller: &Chunk<'a>, id: u64) -> Result<Vec3> {
        if self.class(controller)?.id != id {
            return Err("unsupported 3ds Max position controller".into());
        }
        self.axis_values(controller)
    }

    /// Decode three Bezier Float controllers at their constant or sole time-zero value.
    fn axis_values(&self, controller: &Chunk<'a>) -> Result<Vec3> {
        let mut values = [0.; 3];
        for (axis, value) in values.iter_mut().enumerate() {
            let scalar = self.reference(controller, u32::try_from(axis)?)?;
            if self.class(scalar)?.id != 0x2007 {
                return Err("unsupported 3ds Max scalar transform controller".into());
            }
            let data = scalar.required(0x7127)?;
            *value = if let Some(key) = data.child(0x2525)? {
                if key.data.len() != 28 || u32_at(key.data, 0)? != 0 {
                    return Err("animated 3ds Max transform controller is unsupported".into());
                }
                float_at(key.data, 8)?
            } else {
                keyless(data)?;
                let default = data.required(0x2501)?.data;
                if default.len() != 4 {
                    return Err("invalid 3ds Max scalar value".into());
                }
                float_at(default, 0)?
            };
        }
        Ok(Vec3::new(values[0], values[1], values[2]))
    }

    /// Evaluate a static, unsegmented native Box primitive from its original `ParamBlock`.
    fn box_mesh(&self, object: &Chunk<'a>) -> Result<Vec<[Vec3; 3]>> {
        let parameters = self.reference(object, 0)?;
        if self.class(parameters)?.id != 8 {
            return Err("unsupported 3ds Max Box parameter block".into());
        }
        if !self.references(parameters)?.is_empty() {
            return Err("animated 3ds Max Box parameters are unsupported".into());
        }
        let mut values = BTreeMap::new();
        for parameter in parameters.list()?.iter().filter(|chunk| chunk.id == 2) {
            let id = u32_at(parameter.required(3)?.data, 0)?;
            let data = if id < 3 {
                parameter.required(0x100)?
            } else {
                parameter.required(0x101)?
            };
            if data.data.len() != 4 || values.insert(id, data.data).is_some() {
                return Err("invalid 3ds Max Box parameter".into());
            }
            if parameter.child(0x102)?.is_some() {
                return Err("animated 3ds Max Box parameters are unsupported".into());
            }
        }
        for id in 3..6 {
            if u32_at(
                values.get(&id).ok_or("missing 3ds Max Box segment count")?,
                0,
            )? != 1
            {
                return Err("segmented 3ds Max Box primitives are unsupported".into());
            }
        }
        let dimensions = (0..3)
            .map(|id| float_at(values.get(&id).ok_or("missing 3ds Max Box dimension")?, 0))
            .collect::<Result<Vec<_>>>()?;
        let [length, width, height] = [dimensions[0], dimensions[1], dimensions[2]];
        if length <= 0. || width <= 0. || height == 0. {
            return Err("invalid 3ds Max Box dimensions".into());
        }
        let vertices = [
            Vec3::new(-width / 2., -length / 2., 0.),
            Vec3::new(width / 2., -length / 2., 0.),
            Vec3::new(width / 2., length / 2., 0.),
            Vec3::new(-width / 2., length / 2., 0.),
            Vec3::new(-width / 2., -length / 2., height),
            Vec3::new(width / 2., -length / 2., height),
            Vec3::new(width / 2., length / 2., height),
            Vec3::new(-width / 2., length / 2., height),
        ];
        let faces = [
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        Ok(faces
            .map(|face| {
                let mut triangle = face.map(|index| vertices[index]);
                if height < 0. {
                    triangle.swap(1, 2);
                }
                triangle
            })
            .to_vec())
    }
}

/// Refuse key arrays whose stored default cannot represent a constant controller.
fn keyless(chunk: &Chunk<'_>) -> Result<()> {
    if chunk
        .list()?
        .iter()
        .any(|child| matches!(child.id, 0x2511..=0x2515 | 0x2521..=0x2528))
    {
        return Err("animated 3ds Max controller is unsupported".into());
    }
    Ok(())
}

/// Decode a counted native triangle buffer and validate every referenced vertex.
fn triangle_mesh(object: &Chunk<'_>, budget: &Budget) -> Result<Vec<[Vec3; 3]>> {
    let geometry = object.required(0x08fe)?;
    let vertices = vertices(geometry.required(0x0914)?.data, false)?;
    let faces = geometry.required(0x0912)?.data;
    let count = usize::try_from(u32_at(faces, 0)?)?;
    if count > MAX_TRIANGLES || faces.len() != 4 + count * 20 {
        return Err("invalid or excessive 3ds Max triangle buffer".into());
    }
    let mut triangles = Vec::with_capacity(count);
    for face in faces[4..].as_chunks::<20>().0 {
        budget.check()?;
        let indices = [u32_at(face, 0)?, u32_at(face, 4)?, u32_at(face, 8)?];
        triangles.push(resolve(&vertices, indices)?);
    }
    Ok(triangles)
}

/// Decode native polygon faces, preserving stored triangulation diagonals.
fn polygon_mesh(object: &Chunk<'_>, budget: &Budget) -> Result<Vec<[Vec3; 3]>> {
    let geometry = object.required(0x08fe)?;
    let vertices = vertices(geometry.required(0x0100)?.data, true)?;
    let faces = geometry.required(0x011a)?.data;
    let count = usize::try_from(u32_at(faces, 0)?)?;
    if count > MAX_TRIANGLES {
        return Err("excessive 3ds Max polygon count".into());
    }
    let mut offset = 4;
    let mut triangles = Vec::new();
    for _ in 0..count {
        budget.check()?;
        let corners = usize::try_from(u32_at(faces, offset)?)?;
        offset += 4;
        if !(3..=4096).contains(&corners) {
            return Err("invalid or excessive 3ds Max polygon degree".into());
        }
        let mut polygon = Vec::with_capacity(corners);
        for _ in 0..corners {
            polygon.push(
                *vertices
                    .get(usize::try_from(u32_at(faces, offset)?)?)
                    .ok_or("3ds Max polygon vertex index is out of range")?,
            );
            offset += 4;
        }
        let flags = u16_at(faces, offset)?;
        offset += 2;
        if flags & !0x39 != 0 {
            return Err("unsupported 3ds Max polygon flags".into());
        }
        if flags & 1 != 0 {
            slice(faces, offset, 4)?;
            offset += 4;
        }
        if flags & 8 != 0 {
            slice(faces, offset, 2)?;
            offset += 2;
        }
        if flags & 0x10 != 0 {
            slice(faces, offset, 4)?;
            offset += 4;
        }
        let mut diagonals = Vec::new();
        if flags & 0x20 != 0 {
            for _ in 0..corners - 3 {
                let a = usize::try_from(u32_at(faces, offset)?)?;
                let b = usize::try_from(u32_at(faces, offset + 4)?)?;
                if a >= corners || b >= corners {
                    return Err("3ds Max polygon diagonal index is out of range".into());
                }
                diagonals.push([a, b]);
                offset += 8;
            }
        }
        let generated = if flags & 0x20 != 0 {
            triangulated_polygon(&polygon, &diagonals, budget)?
        } else {
            planar_polygon(&polygon)?
        };
        if triangles.len().saturating_add(generated.len()) > MAX_TRIANGLES {
            return Err("3ds Max polygon triangles exceed their limit".into());
        }
        triangles.extend(generated);
    }
    if offset != faces.len() {
        return Err("invalid 3ds Max polygon buffer extent".into());
    }
    Ok(triangles)
}

/// Recover the exact native face triangulation by removing ears along stored diagonals.
fn triangulated_polygon(
    polygon: &[Vec3],
    diagonals: &[[usize; 2]],
    budget: &Budget,
) -> Result<Vec<[Vec3; 3]>> {
    let mut edges = HashSet::new();
    let edge = |a: usize, b: usize| (a.min(b), a.max(b));
    for i in 0..polygon.len() {
        edges.insert(edge(i, (i + 1) % polygon.len()));
    }
    for &[a, b] in diagonals {
        if a == b || !edges.insert(edge(a, b)) {
            return Err("duplicate or invalid 3ds Max polygon diagonal".into());
        }
    }
    let mut remaining: Vec<_> = (0..polygon.len()).collect();
    let mut triangles = Vec::with_capacity(polygon.len() - 2);
    while remaining.len() > 3 {
        budget.check()?;
        let ear = (0..remaining.len())
            .find(|&i| {
                let before = remaining[(i + remaining.len() - 1) % remaining.len()];
                let after = remaining[(i + 1) % remaining.len()];
                edges.contains(&edge(before, after))
            })
            .ok_or("3ds Max stored polygon diagonals do not form a triangulation")?;
        let before = remaining[(ear + remaining.len() - 1) % remaining.len()];
        let current = remaining[ear];
        let after = remaining[(ear + 1) % remaining.len()];
        triangles.push([polygon[before], polygon[current], polygon[after]]);
        remaining.remove(ear);
    }
    triangles.push([
        polygon[remaining[0]],
        polygon[remaining[1]],
        polygon[remaining[2]],
    ]);
    Ok(triangles)
}

/// Require planarity when a native face omits its evaluated triangulation.
fn planar_polygon(polygon: &[Vec3]) -> Result<Vec<[Vec3; 3]>> {
    let origin = polygon[0];
    let normal = polygon[1..]
        .windows(2)
        .map(|points| (points[0] - origin).cross(points[1] - origin))
        .find(|normal| normal.length() > 0.)
        .unwrap_or_default()
        .normalized();
    let extent = polygon
        .iter()
        .map(|v| (*v - origin).length())
        .fold(0., f64::max);
    if polygon
        .iter()
        .any(|v| (*v - origin).dot(normal).abs() > extent * 1e-6)
    {
        return Err("nonplanar 3ds Max polygon has no stored triangulation".into());
    }
    super::mesh::triangulate_polygon(polygon)
}

/// Read exact native vertex-table extents, including polygon per-vertex flags.
fn vertices(data: &[u8], polygon: bool) -> Result<Vec<Vec3>> {
    let count = usize::try_from(u32_at(data, 0)?)?;
    let stride = if polygon { 16 } else { 12 };
    if count > MAX_VERTICES || data.len() != 4 + count * stride {
        return Err("invalid or excessive 3ds Max vertex buffer".into());
    }
    data[4..]
        .chunks_exact(stride)
        .map(|vertex| vector(vertex, if polygon { 4 } else { 0 }))
        .collect()
}

/// Resolve every corner of one native indexed triangle without unchecked indexing.
fn resolve(vertices: &[Vec3], indices: [u32; 3]) -> Result<[Vec3; 3]> {
    let mut triangle = [Vec3::default(); 3];
    for (corner, index) in triangle.iter_mut().zip(indices) {
        *corner = *vertices
            .get(usize::try_from(index)?)
            .ok_or("3ds Max triangle vertex index is out of range")?;
    }
    Ok(triangle)
}

/// Decode three finite serialized floats at their exact native offset.
fn vector(data: &[u8], offset: usize) -> Result<Vec3> {
    Ok(Vec3::new(
        float_at(data, offset)?,
        float_at(data, offset + 4)?,
        float_at(data, offset + 8)?,
    ))
}

/// Decode the native quaternion component order (x, y, z, w).
fn quaternion(data: &[u8], offset: usize) -> Result<[f64; 4]> {
    Ok([
        float_at(data, offset)?,
        float_at(data, offset + 4)?,
        float_at(data, offset + 8)?,
        float_at(data, offset + 12)?,
    ])
}

/// Construct a finite unit-quaternion affine rotation in Autodesk's convention.
fn rotation(components: [f64; 4]) -> Result<Transform> {
    let length = components.iter().map(|v| v * v).sum::<f64>().sqrt();
    if (length - 1.).abs() > 1e-4 {
        return Err("invalid 3ds Max unit quaternion".into());
    }
    let [x, y, z, w] = components.map(|v| v / length);
    Ok(Transform([
        [
            1. - 2. * (y * y + z * z),
            2. * (x * y - z * w),
            2. * (x * z + y * w),
            0.,
        ],
        [
            2. * (x * y + z * w),
            1. - 2. * (x * x + z * z),
            2. * (y * z - x * w),
            0.,
        ],
        [
            2. * (x * z - y * w),
            2. * (y * z + x * w),
            1. - 2. * (x * x + y * y),
            0.,
        ],
        [0., 0., 0., 1.],
    ]))
}

/// Compose default XYZ Euler angles using Autodesk's clockwise quaternion signs.
fn euler(r: Vec3) -> Transform {
    let (sx, cx) = r.x.sin_cos();
    let (sy, cy) = r.y.sin_cos();
    let (sz, cz) = r.z.sin_cos();
    let x = Transform([
        [1., 0., 0., 0.],
        [0., cx, sx, 0.],
        [0., -sx, cx, 0.],
        [0., 0., 0., 1.],
    ]);
    let y = Transform([
        [cy, 0., -sy, 0.],
        [0., 1., 0., 0.],
        [sy, 0., cy, 0.],
        [0., 0., 0., 1.],
    ]);
    let z = Transform([
        [cz, sz, 0., 0.],
        [-sz, cz, 0., 0.],
        [0., 0., 1., 0.],
        [0., 0., 0., 1.],
    ]);
    z.then(y).then(x)
}

/// Construct a translation independently of the object's pivot offset.
fn translation(v: Vec3) -> Transform {
    let mut t = Transform::IDENTITY;
    t.0[3] = [v.x, v.y, v.z, 1.];
    t
}

/// Apply anisotropic `ScaleValue` axes around the stored quaternion basis.
fn scaling(v: Vec3, q: [f64; 4]) -> Result<Transform> {
    if v.x == 0. || v.y == 0. || v.z == 0. {
        return Err("singular 3ds Max scale".into());
    }
    let r = rotation(q)?;
    let mut inverse = r;
    for column in 0..3 {
        for row in 0..3 {
            inverse.0[column][row] = r.0[row][column];
        }
    }
    let s = Transform([
        [v.x, 0., 0., 0.],
        [0., v.y, 0., 0.],
        [0., 0., v.z, 0.],
        [0., 0., 0., 1.],
    ]);
    Ok(r.then(s).then(inverse))
}

/// Decode object-offset translation, quaternion rotation and `ScaleValue` pivot data.
fn offset(node: &Chunk<'_>) -> Result<Transform> {
    let p = node
        .child(0x96a)?
        .map_or(Ok(Vec3::default()), |c| vector(c.data, 0))?;
    let r = node.child(0x96b)?.map_or(Ok(Transform::IDENTITY), |c| {
        rotation(quaternion(c.data, 0)?)
    })?;
    let s = node.child(0x96c)?.map_or(Ok(Transform::IDENTITY), |c| {
        if c.data.len() != 28 {
            return Err("invalid 3ds Max object-offset ScaleValue".into());
        }
        scaling(vector(c.data, 0)?, quaternion(c.data, 12)?)
    })?;
    Ok(translation(p).then(r).then(s))
}

/// Determine whether the complete affine occurrence reverses surface orientation.
fn determinant(t: Transform) -> f64 {
    let a = Vec3::new(t.0[0][0], t.0[0][1], t.0[0][2]);
    let b = Vec3::new(t.0[1][0], t.0[1][1], t.0[1][2]);
    let c = Vec3::new(t.0[2][0], t.0[2][1], t.0[2][2]);
    a.dot(b.cross(c))
}
