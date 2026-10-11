//! Static Alembic schemas, read from bounded Ogawa groups or pure Rust HDF5.
//! Storage and transform conventions follow the primary Alembic sources:
//! https://github.com/alembic/alembic/tree/master/lib/Alembic/AbcCoreOgawa
//! https://github.com/alembic/alembic/tree/master/lib/Alembic/AbcGeom
use super::{MAX_INPUT_BYTES, mesh::triangulate_polygon, read_file, transform::Transform};
use crate::{
    Result,
    scene::{Line, Point, Scene, Triangle, Vec3},
};
use hdf5_reader::{Attribute, ByteOrder, Datatype, Hdf5File, group::Group};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

const DATA: u64 = 1 << 63;
const MAX_ITEMS: usize = 5_000_000;
const MAX_NODES: usize = 100_000;
const MAX_DEPTH: usize = 256;
type Metadata = HashMap<String, String>;

#[derive(Default)]
struct Property {
    name: String,
    metadata: Metadata,
    children: Vec<Property>,
    sample: Option<Sample>,
}
struct Sample {
    pod: u8,
    extent: usize,
    big_endian: bool,
    bytes: Vec<u8>,
}
struct Object {
    path: String,
    metadata: Metadata,
    properties: Property,
    children: Vec<Object>,
}
impl Property {
    fn child(&self, name: &str) -> Option<&Self> {
        self.children.iter().find(|p| p.name == name)
    }
    fn numbers(&self) -> Result<Vec<f64>> {
        self.sample
            .as_ref()
            .ok_or_else(|| format!("Alembic property {} has no sample", self.name).into())
            .and_then(Sample::numbers)
    }
    fn required(&self, name: &str) -> Result<&Self> {
        self.child(name)
            .ok_or_else(|| format!("Alembic schema is missing {name}").into())
    }
    fn text(&self) -> Result<&str> {
        let sample = self
            .sample
            .as_ref()
            .ok_or("Alembic string property has no sample")?;
        if sample.pod != 12 {
            return Err("Alembic string property has a non-string type".into());
        }
        let bytes = sample.bytes.strip_suffix(&[0]).unwrap_or(&sample.bytes);
        if bytes.contains(&0) {
            return Err("Alembic string property contains multiple values".into());
        }
        Ok(std::str::from_utf8(bytes)?)
    }
}
impl Sample {
    fn numbers(&self) -> Result<Vec<f64>> {
        let width = pod_width(self.pod)?;
        if !self.bytes.len().is_multiple_of(width) {
            return Err("misaligned Alembic numeric sample".into());
        }
        let count = self.bytes.len() / width;
        if count > MAX_ITEMS {
            return Err("Alembic sample exceeds five million components".into());
        }
        self.bytes
            .chunks_exact(width)
            .map(|bytes| {
                let unsigned = if self.big_endian {
                    bytes.iter().fold(0u64, |v, &b| (v << 8) | u64::from(b))
                } else {
                    bytes
                        .iter()
                        .rev()
                        .fold(0u64, |v, &b| (v << 8) | u64::from(b))
                };
                let value = match self.pod {
                    0 => f64::from(unsigned != 0),
                    1 | 3 | 5 | 7 => unsigned as f64,
                    2 => f64::from(unsigned as i8),
                    4 => f64::from(unsigned as i16),
                    6 => f64::from(unsigned as i32),
                    8 => unsigned as i64 as f64,
                    9 => half(unsigned as u16),
                    10 => f64::from(f32::from_bits(unsigned as u32)),
                    11 => f64::from_bits(unsigned),
                    _ => return Err("Alembic string/wide-string values are not numeric".into()),
                };
                if !value.is_finite() {
                    return Err("Alembic numeric samples must be finite".into());
                }
                Ok(value)
            })
            .collect()
    }
}
fn half(bits: u16) -> f64 {
    let sign = if bits & 0x8000 == 0 { 1. } else { -1. };
    let e = (bits >> 10) & 31;
    let m = bits & 1023;
    sign * match e {
        0 => f64::from(m) * 2f64.powi(-24),
        31 => {
            if m == 0 {
                f64::INFINITY
            } else {
                f64::NAN
            }
        }
        _ => (1. + f64::from(m) / 1024.) * 2f64.powi(i32::from(e) - 15),
    }
}
fn pod_width(pod: u8) -> Result<usize> {
    match pod {
        0..=2 => Ok(1),
        3 | 4 | 9 => Ok(2),
        5 | 6 | 10 => Ok(4),
        7 | 8 | 11 => Ok(8),
        12 => Ok(1),
        13 => Ok(4),
        _ => Err("unknown Alembic POD type".into()),
    }
}
fn metadata(bytes: &[u8]) -> Result<Metadata> {
    if bytes.len() > 1_048_576 {
        return Err("Alembic metadata exceeds one MiB".into());
    }
    let text = std::str::from_utf8(bytes)?;
    Ok(text
        .split(';')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect())
}
#[derive(Default)]
struct Budget {
    nodes: usize,
    decoded: u64,
}
impl Budget {
    fn node(&mut self, depth: usize) -> Result<()> {
        self.nodes += 1;
        if depth > MAX_DEPTH || self.nodes > MAX_NODES {
            return Err("Alembic hierarchy exceeds 256 levels/100000 nodes".into());
        }
        Ok(())
    }
    fn bytes(&mut self, count: usize) -> Result<()> {
        self.decoded = self
            .decoded
            .checked_add(count as u64)
            .ok_or("Alembic sample budget overflow")?;
        if self.decoded > MAX_INPUT_BYTES {
            return Err("Alembic samples exceed aggregate 512 MiB limit".into());
        }
        Ok(())
    }
}

pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = read_file(path)?;
    let root = if bytes.starts_with(b"Ogawa") {
        Ogawa::new(&bytes)?.root()?
    } else if bytes.starts_with(b"\x89HDF\r\n\x1a\n") {
        hdf_archive(bytes)?
    } else {
        return Err("unsupported Alembic archive; expected Ogawa or HDF5".into());
    };
    build_scene(&root)
}

struct Bytes<'a> {
    data: &'a [u8],
    offset: usize,
}
impl<'a> Bytes<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(n)
            .ok_or("Alembic offset overflow")?;
        let data = self
            .data
            .get(self.offset..end)
            .ok_or("truncated Alembic record")?;
        self.offset = end;
        Ok(data)
    }
    fn uint(&mut self, n: usize) -> Result<u64> {
        Ok(self
            .take(n)?
            .iter()
            .rev()
            .fold(0, |v, &b| (v << 8) | u64::from(b)))
    }
    fn hint(&mut self, hint: u32) -> Result<usize> {
        let width = match hint {
            0 => 1,
            1 => 2,
            2 => 4,
            _ => return Err("invalid Alembic property size hint".into()),
        };
        Ok(usize::try_from(self.uint(width)?)?)
    }
    fn name(&mut self, len: usize) -> Result<String> {
        if len == 0 || len > 4096 {
            return Err("invalid Alembic name length".into());
        }
        let name = std::str::from_utf8(self.take(len)?)?;
        if name.contains('\0') {
            return Err("Alembic name contains NUL".into());
        }
        Ok(name.to_owned())
    }
    fn done(&self) -> bool {
        self.offset == self.data.len()
    }
}
struct Ogawa<'a> {
    bytes: &'a [u8],
    metadata: Vec<Metadata>,
    archive: Vec<u64>,
    budget: Budget,
    active: HashSet<u64>,
}
impl<'a> Ogawa<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self> {
        if bytes.get(..8) != Some(b"Ogawa\xff\0\x01") {
            return Err("unsupported or unclosed Ogawa archive; expected version1".into());
        }
        let mut reader = Self {
            bytes,
            metadata: vec![Metadata::new()],
            archive: Vec::new(),
            budget: Budget::default(),
            active: HashSet::new(),
        };
        let root = Bytes::new(bytes.get(8..16).ok_or("truncated Ogawa header")?).uint(8)?;
        let archive = reader.group(root)?;
        if archive.len() < 6 {
            return Err("invalid Alembic archive root".into());
        }
        let version = reader.data(archive[0])?;
        if version.len() != 4 || Bytes::new(version).uint(4)? > 0 {
            return Err("unsupported Alembic Ogawa storage version".into());
        }
        let release = reader.data(archive[1])?;
        if release.len() != 4 || Bytes::new(release).uint(4)? < 9999 {
            return Err("unsupported Alembic release version".into());
        }
        let mut indexed = Bytes::new(reader.data(archive[5])?);
        while !indexed.done() {
            let size = indexed.uint(1)? as usize;
            reader.metadata.push(metadata(indexed.take(size)?)?);
            if reader.metadata.len() > 256 {
                return Err("Alembic indexed metadata exceeds 256 entries".into());
            }
        }
        reader.archive = archive;
        Ok(reader)
    }
    fn group(&self, pointer: u64) -> Result<Vec<u64>> {
        if pointer & DATA != 0 {
            return Err("Alembic data record used as a group".into());
        }
        if pointer == 0 {
            return Ok(Vec::new());
        }
        let offset = usize::try_from(pointer)?;
        let mut b = Bytes::new(
            self.bytes
                .get(offset..)
                .ok_or("Ogawa group offset outside input")?,
        );
        let count = usize::try_from(b.uint(8)?)?;
        if count > 500_000 {
            return Err("Alembic group exceeds 500000 children".into());
        }
        let mut entries =
            Bytes::new(b.take(count.checked_mul(8).ok_or("Ogawa group size overflow")?)?);
        (0..count).map(|_| entries.uint(8)).collect()
    }
    fn data(&self, pointer: u64) -> Result<&'a [u8]> {
        if pointer & DATA == 0 {
            return Err("Alembic group record used as data".into());
        }
        let offset = usize::try_from(pointer & !DATA)?;
        if offset == 0 {
            return Ok(&[]);
        }
        let mut b = Bytes::new(
            self.bytes
                .get(offset..)
                .ok_or("Ogawa data offset outside input")?,
        );
        let size = usize::try_from(b.uint(8)?)?;
        b.take(size)
    }
    fn meta(&self, index: usize, b: &mut Bytes<'_>, hint: Option<u32>) -> Result<Metadata> {
        if index == 255 {
            let size = if let Some(h) = hint {
                b.hint(h)?
            } else {
                usize::try_from(b.uint(4)?)?
            };
            metadata(b.take(size)?)
        } else {
            self.metadata
                .get(index)
                .cloned()
                .ok_or_else(|| "invalid Alembic metadata index".into())
        }
    }
    fn root(mut self) -> Result<Object> {
        let md = metadata(self.data(self.archive[3])?)?;
        self.object(self.archive[2], "/".to_owned(), md, 0)
    }
    fn object(&mut self, pointer: u64, path: String, md: Metadata, depth: usize) -> Result<Object> {
        self.budget.node(depth)?;
        if pointer != 0 && !self.active.insert(pointer) {
            return Err("cyclic Alembic object records".into());
        }
        let group = self.group(pointer)?;
        let mut properties = Property::default();
        let mut children = Vec::new();
        if let Some(&p) = group.first() {
            properties = self.compound(p, String::new(), Metadata::new(), depth + 1)?;
        }
        if let Some(&headers) = group.last().filter(|&&p| p & DATA != 0) {
            let raw = self.data(headers)?;
            let mut b = Bytes::new(
                raw.get(
                    ..raw
                        .len()
                        .checked_sub(32)
                        .ok_or("Alembic object headers lack hashes")?,
                )
                .ok_or("invalid Alembic object headers")?,
            );
            let mut names = HashSet::new();
            let mut index = 1;
            while !b.done() {
                let len = usize::try_from(b.uint(4)?)?;
                let name = b.name(len)?;
                if name.contains('/') || !names.insert(name.clone()) {
                    return Err("invalid or duplicate Alembic object name".into());
                }
                let mi = b.uint(1)? as usize;
                let md = self.meta(mi, &mut b, None)?;
                let pointer = *group
                    .get(index)
                    .ok_or("Alembic object header count mismatch")?;
                children.push(self.object(
                    pointer,
                    format!("{}/{name}", path.trim_end_matches('/')),
                    md,
                    depth + 1,
                )?);
                index += 1;
            }
            if index + 1 != group.len() {
                return Err("Alembic child/header count mismatch".into());
            }
        } else if group.len() > 1 {
            return Err("Alembic object lacks child headers".into());
        }
        self.active.remove(&pointer);
        Ok(Object {
            path,
            metadata: md,
            properties,
            children,
        })
    }
    fn compound(
        &mut self,
        pointer: u64,
        name: String,
        md: Metadata,
        depth: usize,
    ) -> Result<Property> {
        self.budget.node(depth)?;
        if pointer != 0 && !self.active.insert(pointer) {
            return Err("cyclic Alembic property records".into());
        }
        let group = self.group(pointer)?;
        let mut children = Vec::new();
        if let Some(&p) = group.last() {
            let mut b = Bytes::new(self.data(p)?);
            let mut index = 0;
            let mut names = HashSet::new();
            while !b.done() {
                let info = b.uint(4)? as u32;
                let kind = info & 3;
                let hint = (info >> 2) & 3;
                let pod = ((info >> 4) & 15) as u8;
                let extent = ((info >> 12) & 255) as usize;
                let mut samples = 0;
                if kind != 0 {
                    pod_width(pod)?;
                    if extent == 0 {
                        return Err("Alembic property has zero extent".into());
                    }
                    samples = b.hint(hint)?;
                    if info & 512 != 0 {
                        let first = b.hint(hint)?;
                        let last = b.hint(hint)?;
                        if first > last || last >= samples {
                            return Err("invalid Alembic sample repetition range".into());
                        }
                    }
                    if info & 256 != 0 {
                        b.hint(hint)?;
                    }
                }
                let len = b.hint(hint)?;
                let name = b.name(len)?;
                if !names.insert(name.clone()) {
                    return Err("duplicate Alembic property name".into());
                }
                let md = self.meta(((info >> 20) & 255) as usize, &mut b, Some(hint))?;
                let child = *group.get(index).ok_or("Alembic property count mismatch")?;
                let prop = if kind == 0 {
                    self.compound(child, name, md, depth + 1)?
                } else {
                    self.budget.node(depth + 1)?;
                    let records = self.group(child)?;
                    let sample = if samples == 0 {
                        None
                    } else {
                        let raw = self.data(
                            *records
                                .first()
                                .ok_or("Alembic sampled property has no data")?,
                        )?;
                        let raw = if raw.is_empty() && kind >= 2 {
                            raw
                        } else {
                            raw.get(16..).ok_or("Alembic sample lacks its data key")?
                        };
                        let width = pod_width(pod)?;
                        if pod < 12
                            && (!raw.len().is_multiple_of(width * extent)
                                || raw.len() / width > MAX_ITEMS)
                        {
                            return Err("invalid or oversized Alembic sample".into());
                        }
                        if kind == 1 && pod < 12 && raw.len() != width * extent {
                            return Err("Alembic scalar sample has wrong extent".into());
                        }
                        if kind >= 2 {
                            let dims = self.data(
                                *records
                                    .get(1)
                                    .ok_or("Alembic array sample lacks dimensions")?,
                            )?;
                            if !dims.is_empty() && pod < 12 {
                                let mut dims = Bytes::new(dims);
                                let mut count = 1u64;
                                while !dims.done() {
                                    count = count
                                        .checked_mul(dims.uint(8)?)
                                        .ok_or("Alembic dimensions overflow")?;
                                }
                                if count != u64::try_from(raw.len() / (width * extent))? {
                                    return Err(
                                        "Alembic array dimensions disagree with sample".into()
                                    );
                                }
                            }
                        }
                        self.budget.bytes(raw.len())?;
                        Some(Sample {
                            pod,
                            extent,
                            big_endian: false,
                            bytes: raw.to_vec(),
                        })
                    };
                    Property {
                        name,
                        metadata: md,
                        children: Vec::new(),
                        sample,
                    }
                };
                children.push(prop);
                index += 1;
            }
            if index + 1 != group.len() {
                return Err("Alembic property/header count mismatch".into());
            }
        }
        self.active.remove(&pointer);
        Ok(Property {
            name,
            metadata: md,
            children,
            sample: None,
        })
    }
}

fn hdf_archive(bytes: Vec<u8>) -> Result<Object> {
    let file = Hdf5File::from_vec(bytes)?;
    let root = file.root_group()?;
    let version = root.attribute("abc_version")?.read_scalar::<i32>()?;
    if !(-8..=0).contains(&version) {
        return Err("unsupported Alembic HDF5 storage version".into());
    }
    hdf_object(
        &file.group("/ABC")?,
        "/".to_owned(),
        &mut Budget::default(),
        0,
    )
}
fn hdf_meta(attributes: &[Attribute], name: &str) -> Result<Metadata> {
    match attributes.iter().find(|a| a.name == name) {
        Some(a) => metadata(a.read_string()?.as_bytes()),
        None => Ok(Metadata::new()),
    }
}
fn hdf_object(group: &Group, path: String, budget: &mut Budget, depth: usize) -> Result<Object> {
    budget.node(depth)?;
    let attrs = group.attributes()?;
    let metadata = hdf_meta(&attrs, ".prop.meta")?;
    let properties = if group.group(".prop").is_ok() {
        hdf_compound(
            &group.group(".prop")?,
            String::new(),
            Metadata::new(),
            budget,
            depth + 1,
        )?
    } else {
        Property::default()
    };
    let mut children = Vec::new();
    for child in group.groups()? {
        let name = child
            .name()
            .rsplit('/')
            .next()
            .ok_or("Alembic HDF5 child lacks a name")?;
        if name == ".prop" {
            continue;
        }
        children.push(hdf_object(
            &child,
            format!("{}/{name}", path.trim_end_matches('/')),
            budget,
            depth + 1,
        )?);
    }
    Ok(Object {
        path,
        metadata,
        properties,
        children,
    })
}
fn byte_order(dtype: &Datatype) -> Result<bool> {
    match dtype {
        Datatype::FixedPoint { byte_order, .. } | Datatype::FloatingPoint { byte_order, .. } => {
            Ok(*byte_order == ByteOrder::BigEndian)
        }
        Datatype::Array { base, .. } | Datatype::Enum { base, .. } => byte_order(base),
        _ => Err("unsupported Alembic HDF5 numeric datatype".into()),
    }
}
fn hdf_compound(
    group: &Group,
    name: String,
    md: Metadata,
    budget: &mut Budget,
    depth: usize,
) -> Result<Property> {
    budget.node(depth)?;
    let attrs = group.attributes()?;
    let mut children = Vec::new();
    for attr in attrs.iter().filter(|a| a.name.ends_with(".info")) {
        let name = attr.name.strip_suffix(".info").unwrap().to_owned();
        if attr.num_elements()? > 5 {
            return Err("Alembic HDF5 property header exceeds five words".into());
        }
        let info = attr.read_1d::<u32>()?;
        let bits = *info.first().ok_or("empty Alembic HDF5 property header")?;
        let md = hdf_meta(&attrs, &format!("{name}.meta"))?;
        let kind = bits & 3;
        let prop = if kind == 0 {
            match group.group(&name) {
                Ok(child) => hdf_compound(&child, name, md, budget, depth + 1)?,
                Err(_) => Property {
                    name,
                    metadata: md,
                    ..Property::default()
                },
            }
        } else {
            budget.node(depth + 1)?;
            let pod = ((bits >> 2) & 15) as u8;
            let extent = ((bits >> 8) & 255) as usize;
            if extent == 0 {
                return Err("Alembic HDF5 sample has zero extent".into());
            }
            pod_width(pod)?;
            let sample_name = format!("{name}.smp0");
            let sample = if kind == 1 {
                attrs
                    .iter()
                    .find(|a| a.name == sample_name)
                    .map(|a| {
                        budget.bytes(a.raw_data.len())?;
                        let big_endian = if pod < 12 {
                            byte_order(&a.datatype)?
                        } else {
                            false
                        };
                        Ok::<_, crate::Error>(Sample {
                            pod,
                            extent,
                            big_endian,
                            bytes: if pod == 12 {
                                a.read_string()?.into_bytes()
                            } else {
                                a.raw_data.clone()
                            },
                        })
                    })
                    .transpose()?
            } else if let Ok(dataset) = group.dataset(&sample_name) {
                let element = hdf5_reader::datatype_api::dtype_element_size(dataset.dtype())?;
                let total = usize::try_from(dataset.num_elements()?)?
                    .checked_mul(element)
                    .ok_or("Alembic HDF5 array byte size overflow")?;
                budget.bytes(total)?;
                let big_endian = if pod < 12 {
                    byte_order(dataset.dtype())?
                } else {
                    false
                };
                Some(Sample {
                    pod,
                    extent,
                    big_endian,
                    bytes: if pod == 12 {
                        dataset.read_string()?.into_bytes()
                    } else {
                        dataset.read_raw_bytes()?
                    },
                })
            } else {
                None
            };
            Property {
                name,
                metadata: md,
                children: Vec::new(),
                sample,
            }
        };
        children.push(prop);
    }
    Ok(Property {
        name,
        metadata: md,
        children,
        sample: None,
    })
}

fn build_scene(root: &Object) -> Result<Scene> {
    let mut objects = HashMap::new();
    index_objects(root, &mut objects);
    let mut scene = Scene::default();
    walk(
        root,
        Transform::IDENTITY,
        false,
        &objects,
        &mut HashSet::new(),
        0,
        &mut scene,
    )?;
    scene.validate()?;
    Ok(scene)
}
fn index_objects<'a>(object: &'a Object, index: &mut HashMap<&'a str, &'a Object>) {
    index.insert(&object.path, object);
    for child in &object.children {
        index_objects(child, index);
    }
}
fn walk(
    object: &Object,
    parent: Transform,
    hidden: bool,
    objects: &HashMap<&str, &Object>,
    active: &mut HashSet<String>,
    depth: usize,
    scene: &mut Scene,
) -> Result<()> {
    if depth > MAX_DEPTH || !active.insert(object.path.clone()) {
        return Err("cyclic or excessively deep Alembic instancing".into());
    }
    if object.metadata.get("isInstance").is_some_and(|v| v == "1") {
        let path = object.properties.required(".instanceSource")?.text()?;
        let source = objects
            .get(path)
            .ok_or("Alembic instance source does not exist")?;
        walk(source, parent, hidden, objects, active, depth + 1, scene)?;
        active.remove(&object.path);
        return Ok(());
    }
    let visibility = object
        .properties
        .child("visible")
        .map(Property::numbers)
        .transpose()?;
    let hidden = match visibility.as_deref() {
        Some([0.]) => true,
        Some([1.]) => false,
        Some([-1.]) | None => hidden,
        _ => return Err("invalid Alembic visibility sample".into()),
    };
    let schema = object
        .metadata
        .get("schema")
        .map(String::as_str)
        .unwrap_or("");
    let mut world = parent;
    if schema.starts_with("AbcGeom_Xform_") || object.properties.child(".xform").is_some() {
        let p = object.properties.required(".xform")?;
        let local = xform(p)?;
        let inherits = p.child(".inherits").map(Property::numbers).transpose()?;
        world = match inherits.as_deref() {
            Some([0.]) => local,
            Some([1.]) | None => parent.then(local),
            _ => return Err("invalid Alembic transform inheritance sample".into()),
        };
    }
    if !hidden {
        let geom = object.properties.child(".geom");
        if schema.starts_with("AbcGeom_PolyMesh_") || schema.starts_with("AbcGeom_SubD_") {
            mesh(geom.ok_or("Alembic mesh lacks .geom")?, world, scene)?;
        } else if schema.starts_with("AbcGeom_Points_") {
            point_cloud(geom.ok_or("Alembic points lack .geom")?, world, scene)?;
        } else if schema.starts_with("AbcGeom_Curve_") {
            curves(geom.ok_or("Alembic curves lack .geom")?, world, scene)?;
        } else if !schema.is_empty()
            && !schema.starts_with("AbcGeom_Xform_")
            && !schema.starts_with("AbcGeom_Camera_")
            && !schema.starts_with("AbcGeom_FaceSet_")
            && !schema.starts_with("AbcMaterial_")
        {
            return Err(format!("unsupported Alembic geometry schema {schema}").into());
        } else if schema.is_empty()
            && (geom.is_some_and(|g| g.child("P").is_some())
                || object.metadata.contains_key("Usd:typeName"))
        {
            return Err("Alembic custom or incomplete geometry schemas are unsupported".into());
        }
    }
    for child in &object.children {
        walk(child, world, hidden, objects, active, depth + 1, scene)?;
    }
    active.remove(&object.path);
    geometry_limit(scene)
}
fn xform(p: &Property) -> Result<Transform> {
    let Some(ops) = p.child(".ops") else {
        if p.child(".vals").is_some() {
            return Err("Alembic transform has values but no operations".into());
        }
        return Ok(Transform::IDENTITY);
    };
    let ops = ops.numbers()?;
    let vals = p.required(".vals")?.numbers()?;
    let mut offset = 0;
    let mut result = Transform::IDENTITY;
    for op in ops {
        if !(0. ..=255.).contains(&op) || op.fract() != 0. {
            return Err("invalid Alembic transform operation encoding".into());
        }
        let kind = (op as u8) >> 4;
        let len = match kind {
            0 | 1 => 3,
            2 => 4,
            3 => 16,
            4..=6 => 1,
            _ => return Err("unsupported Alembic transform operation".into()),
        };
        let v = vals
            .get(offset..offset + len)
            .ok_or("Alembic transform has too few channels")?;
        offset += len;
        let m = match kind {
            0 => Transform([
                [v[0], 0., 0., 0.],
                [0., v[1], 0., 0.],
                [0., 0., v[2], 0.],
                [0., 0., 0., 1.],
            ]),
            1 => Transform([
                [1., 0., 0., 0.],
                [0., 1., 0., 0.],
                [0., 0., 1., 0.],
                [v[0], v[1], v[2], 1.],
            ]),
            2 => rotate(Vec3::new(v[0], v[1], v[2]), v[3])?,
            3 => Transform(std::array::from_fn(|c| {
                std::array::from_fn(|r| v[c * 4 + r])
            })),
            4..=6 => rotate(
                match kind {
                    4 => Vec3::new(1., 0., 0.),
                    5 => Vec3::new(0., 1., 0.),
                    _ => Vec3::new(0., 0., 1.),
                },
                v[0],
            )?,
            _ => unreachable!(),
        };
        if m.0.iter().flatten().any(|v| !v.is_finite())
            || m.0[0][3] != 0.
            || m.0[1][3] != 0.
            || m.0[2][3] != 0.
            || m.0[3][3] != 1.
        {
            return Err("Alembic transforms must be finite and affine".into());
        }
        result = result.then(m);
    }
    if offset != vals.len() {
        return Err("Alembic transform has unused channels".into());
    }
    if result.0.iter().flatten().any(|v| !v.is_finite()) {
        return Err("Alembic transform exceeds finite range".into());
    }
    Ok(result)
}
fn rotate(axis: Vec3, degrees: f64) -> Result<Transform> {
    if axis.length() == 0. {
        return Err("Alembic rotation has a zero axis".into());
    }
    let a = axis.normalized();
    let (s, c) = degrees.to_radians().sin_cos();
    let t = 1. - c;
    let (x, y, z) = (a.x, a.y, a.z);
    Ok(Transform([
        [t * x * x + c, t * x * y + s * z, t * x * z - s * y, 0.],
        [t * x * y - s * z, t * y * y + c, t * y * z + s * x, 0.],
        [t * x * z + s * y, t * y * z - s * x, t * z * z + c, 0.],
        [0., 0., 0., 1.],
    ]))
}
fn positions(p: &Property) -> Result<Vec<Vec3>> {
    let p = p.required("P")?;
    if p.sample.as_ref().is_none_or(|s| s.extent != 3) {
        return Err("Alembic positions need extent three".into());
    }
    let values = p.numbers()?;
    if !values.len().is_multiple_of(3) {
        return Err("Alembic coordinates are not triples".into());
    }
    Ok(values
        .as_chunks::<3>()
        .0
        .iter()
        .map(|v| Vec3::new(v[0], v[1], v[2]))
        .collect())
}
fn indices(p: &Property) -> Result<Vec<usize>> {
    p.numbers()?
        .into_iter()
        .map(|v| {
            if !(0. ..=MAX_ITEMS as f64).contains(&v) || v.fract() != 0. {
                return Err("invalid Alembic integer/index sample".into());
            }
            Ok(v as usize)
        })
        .collect()
}
fn point_cloud(p: &Property, world: Transform, scene: &mut Scene) -> Result<()> {
    let positions = positions(p)?;
    let colors = Colors::new(p)?;
    for (i, position) in positions.into_iter().enumerate() {
        scene.points.push(Point {
            position: world.apply(position),
            color: colors.as_ref().map(|c| c.color(&[i], i, i)).transpose()?,
        });
    }
    geometry_limit(scene)
}
fn mesh(p: &Property, world: Transform, scene: &mut Scene) -> Result<()> {
    let vertices = positions(p)?;
    let counts = indices(p.required(".faceCounts")?)?;
    let ids = indices(p.required(".faceIndices")?)?;
    let holes = p
        .child(".holes")
        .map(indices)
        .transpose()?
        .unwrap_or_default()
        .into_iter()
        .collect::<HashSet<_>>();
    let colors = Colors::new(p)?;
    let mut offset = 0;
    for (face, count) in counts.into_iter().enumerate() {
        if !(3..=65_536).contains(&count) {
            return Err("Alembic polygon needs 3..65536 vertices".into());
        }
        let indices = ids
            .get(offset..offset + count)
            .ok_or("Alembic face counts exceed the index array")?;
        let polygon = indices
            .iter()
            .map(|&i| {
                vertices
                    .get(i)
                    .copied()
                    .ok_or_else(|| "Alembic vertex index outside positions".into())
            })
            .collect::<Result<Vec<_>>>()?;
        let color = colors
            .as_ref()
            .map(|c| c.color(indices, face, offset))
            .transpose()?;
        if !holes.contains(&face) {
            for vertices in triangulate_polygon(&polygon)? {
                scene.triangles.push(Triangle {
                    vertices: vertices.map(|v| world.apply(v)),
                    color,
                });
            }
        }
        offset += count;
        geometry_limit(scene)?;
    }
    if offset != ids.len() {
        return Err("Alembic polygon indices contain unused entries".into());
    }
    Ok(())
}
struct Colors {
    values: Vec<Vec3>,
    indices: Option<Vec<usize>>,
    scope: String,
}
impl Colors {
    fn new(p: &Property) -> Result<Option<Self>> {
        let params = p.child(".arbGeomParams").unwrap_or(p);
        let Some(color) = params.children.iter().find(|p| {
            matches!(p.name.as_str(), "Cs" | "Cd" | "displayColor")
                || matches!(
                    p.metadata.get("interpretation").map(String::as_str),
                    Some("rgb" | "rgba")
                )
        }) else {
            return Ok(None);
        };
        let values = color.child(".vals").unwrap_or(color);
        let sample = values
            .sample
            .as_ref()
            .ok_or("Alembic color has no values")?;
        if sample.extent != 3 && sample.extent != 4 {
            return Err("Alembic colors need RGB/RGBA extent".into());
        }
        let n = values.numbers()?;
        if !n.len().is_multiple_of(sample.extent) {
            return Err("Alembic color sample is misaligned".into());
        }
        let values = n
            .chunks_exact(sample.extent)
            .map(|v| Vec3::new(v[0], v[1], v[2]))
            .collect();
        let indices = color.child(".indices").map(indices).transpose()?;
        let scope = color
            .metadata
            .get("geoScope")
            .or_else(|| {
                color
                    .child(".vals")
                    .and_then(|p| p.metadata.get("geoScope"))
            })
            .map(String::as_str)
            .unwrap_or("con")
            .to_owned();
        if !matches!(scope.as_str(), "con" | "uni" | "var" | "vtx" | "fvr") {
            return Err("unsupported Alembic color geometry scope".into());
        }
        Ok(Some(Self {
            values,
            indices,
            scope,
        }))
    }
    fn color(&self, ids: &[usize], face: usize, corner: usize) -> Result<[u8; 3]> {
        let mut sum = Vec3::default();
        for (offset, &vertex) in ids.iter().enumerate() {
            let i = match self.scope.as_str() {
                "con" => 0,
                "uni" => face,
                "fvr" => corner + offset,
                _ => vertex,
            };
            let i = if let Some(indices) = &self.indices {
                *indices
                    .get(i)
                    .ok_or("Alembic color index array is too short")?
            } else {
                i
            };
            sum = sum
                + *self
                    .values
                    .get(i)
                    .ok_or("Alembic color array is too short")?;
        }
        let c = sum / (ids.len() as f64);
        Ok([srgb(c.x), srgb(c.y), srgb(c.z)])
    }
}
fn srgb(v: f64) -> u8 {
    let v = v.clamp(0., 1.);
    ((if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    }) * 255.)
        .round() as u8
}
fn curves(p: &Property, world: Transform, scene: &mut Scene) -> Result<()> {
    let positions = positions(p)?;
    let counts = indices(p.required("nVertices")?)?;
    let options = p.required("curveBasisAndType")?.numbers()?;
    let [kind, periodic, basis, ..] = options.as_slice() else {
        return Err("Alembic curves lack basis/type channels".into());
    };
    if (*kind != 0. && *kind != 1.) || (*periodic != 0. && *periodic != 1.) {
        return Err("Alembic variable-order curves are unsupported".into());
    }
    if p.child("w").is_some() {
        return Err("Alembic rational curves are unsupported".into());
    }
    let colors = Colors::new(p)?;
    let mut offset = 0;
    for (curve, count) in counts.into_iter().enumerate() {
        let v = positions
            .get(offset..offset + count)
            .ok_or("Alembic curve counts exceed positions")?;
        if count < 2 {
            return Err("Alembic curve needs at least two vertices".into());
        }
        let ids = (offset..offset + count).collect::<Vec<_>>();
        let color = colors
            .as_ref()
            .map(|c| c.color(&ids, curve, offset))
            .transpose()?;
        let periodic = *periodic == 1.;
        if *kind == 1. {
            for pair in v.windows(2) {
                scene.lines.push(Line {
                    vertices: [world.apply(pair[0]), world.apply(pair[1])],
                    color,
                });
            }
            if periodic {
                scene.lines.push(Line {
                    vertices: [world.apply(v[count - 1]), world.apply(v[0])],
                    color,
                });
            }
        } else {
            let step = match *basis {
                1. => 3,
                4. => 2,
                5. => 4,
                _ => 1,
            };
            if !matches!(*basis, 1. | 2. | 3. | 4. | 5.) || count < 4 {
                return Err("unsupported Alembic cubic curve basis/count".into());
            }
            let spans = if periodic {
                if !count.is_multiple_of(step) {
                    return Err("Alembic periodic Bezier count is not divisible by three".into());
                }
                count / step
            } else {
                if !(count - 4).is_multiple_of(step) {
                    return Err("Alembic cubic count does not match its basis step".into());
                }
                (count - 4) / step + 1
            };
            for span in 0..spans {
                let q = std::array::from_fn(|i| v[(span * step + i) % count]);
                let mut previous = cubic(q, *basis, 0.);
                for sample in 1..=24 {
                    let next = cubic(q, *basis, f64::from(sample) / 24.);
                    scene.lines.push(Line {
                        vertices: [world.apply(previous), world.apply(next)],
                        color,
                    });
                    previous = next;
                }
                geometry_limit(scene)?;
            }
        }
        offset += count;
        geometry_limit(scene)?;
    }
    if offset != positions.len() {
        return Err("Alembic curves contain unused positions".into());
    }
    Ok(())
}
fn cubic(q: [Vec3; 4], basis: f64, t: f64) -> Vec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    let weights = match basis {
        1. => [
            (1. - t).powi(3),
            3. * t * (1. - t).powi(2),
            3. * t2 * (1. - t),
            t3,
        ],
        2. => [
            (-t3 + 3. * t2 - 3. * t + 1.) / 6.,
            (3. * t3 - 6. * t2 + 4.) / 6.,
            (-3. * t3 + 3. * t2 + 3. * t + 1.) / 6.,
            t3 / 6.,
        ],
        3. => [
            -0.5 * t3 + t2 - 0.5 * t,
            1.5 * t3 - 2.5 * t2 + 1.,
            -1.5 * t3 + 2. * t2 + 0.5 * t,
            0.5 * t3 - 0.5 * t2,
        ],
        4. => [
            2. * t3 - 3. * t2 + 1.,
            t3 - 2. * t2 + t,
            -2. * t3 + 3. * t2,
            t3 - t2,
        ],
        _ => [t3, t2, t, 1.],
    };
    q.into_iter()
        .zip(weights)
        .fold(Vec3::default(), |v, (q, w)| v + q * w)
}
fn geometry_limit(scene: &Scene) -> Result<()> {
    if scene
        .triangles
        .len()
        .saturating_add(scene.points.len())
        .saturating_add(scene.lines.len())
        > MAX_ITEMS
    {
        return Err("Alembic scene exceeds five million elements".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new(bytes: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!(
                "catwalk-alembic-{}-{}.abc",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::write(&path, bytes).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    fn md(schema: &str) -> Metadata {
        HashMap::from([("schema".to_owned(), schema.to_owned())])
    }
    fn compound(name: &str, children: Vec<Property>) -> Property {
        Property {
            name: name.to_owned(),
            children,
            ..Property::default()
        }
    }
    fn value(name: &str, extent: usize, values: &[f64]) -> Property {
        let (pod, bytes) = match name {
            ".ops" | "curveBasisAndType" | ".inherits" => {
                (1, values.iter().map(|&v| v as u8).collect())
            }
            ".faceCounts" | ".faceIndices" | ".indices" | ".holes" | "nVertices" => (
                6,
                values
                    .iter()
                    .flat_map(|&v| (v as i32).to_le_bytes())
                    .collect(),
            ),
            _ => (11, values.iter().flat_map(|v| v.to_le_bytes()).collect()),
        };
        Property {
            name: name.to_owned(),
            sample: Some(Sample {
                pod,
                extent,
                big_endian: false,
                bytes,
            }),
            ..Property::default()
        }
    }
    fn string(name: &str, text: &str) -> Property {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        Property {
            name: name.to_owned(),
            sample: Some(Sample {
                pod: 12,
                extent: 1,
                big_endian: false,
                bytes,
            }),
            ..Property::default()
        }
    }
    fn object(
        path: &str,
        schema: &str,
        properties: Vec<Property>,
        children: Vec<Object>,
    ) -> Object {
        Object {
            path: path.to_owned(),
            metadata: md(schema),
            properties: compound("", properties),
            children,
        }
    }
    fn triangle(path: &str) -> Object {
        object(
            path,
            "AbcGeom_PolyMesh_v1",
            vec![compound(
                ".geom",
                vec![
                    value("P", 3, &[0., 0., 0., 1., 0., 0., 0., 1., 0.]),
                    value(".faceCounts", 1, &[3.]),
                    value(".faceIndices", 1, &[0., 1., 2.]),
                ],
            )],
            Vec::new(),
        )
    }
    fn root(child: Object) -> Object {
        object("/", "", Vec::new(), vec![child])
    }
    fn xform(ops: &[f64], values: &[f64], inherits: bool) -> Property {
        compound(
            ".xform",
            vec![
                value(".ops", ops.len(), ops),
                value(".vals", values.len(), values),
                value(".inherits", 1, &[f64::from(inherits)]),
            ],
        )
    }
    fn close(actual: Vec3, expected: [f64; 3]) {
        assert!(
            (actual - Vec3::new(expected[0], expected[1], expected[2])).length() < 1e-9,
            "{actual:?} != {expected:?}"
        );
    }
    struct Writer(Vec<u8>);
    impl Writer {
        fn data(&mut self, data: &[u8]) -> u64 {
            let ptr = self.0.len() as u64;
            self.0.extend_from_slice(&(data.len() as u64).to_le_bytes());
            self.0.extend_from_slice(data);
            ptr | DATA
        }
        fn group(&mut self, children: &[u64]) -> u64 {
            if children.is_empty() {
                return 0;
            }
            let ptr = self.0.len() as u64;
            self.0
                .extend_from_slice(&(children.len() as u64).to_le_bytes());
            for child in children {
                self.0.extend_from_slice(&child.to_le_bytes());
            }
            ptr
        }
        fn property(&mut self, p: &Property) -> u64 {
            let mut children = Vec::new();
            let mut headers = Vec::new();
            for p in &p.children {
                let (kind, pod, extent, records) = if let Some(sample) = &p.sample {
                    let width = pod_width(sample.pod).unwrap();
                    let scalar = sample.pod >= 12 || sample.bytes.len() == width * sample.extent;
                    let kind = if scalar { 1 } else { 2 };
                    let mut data = vec![0; 16];
                    data.extend_from_slice(&sample.bytes);
                    let pointer = self.data(&data);
                    let mut records = vec![pointer];
                    if !scalar {
                        records.push(DATA);
                    }
                    if p.name == "P" {
                        let mut second = vec![0; 16];
                        second.extend(
                            sample
                                .bytes
                                .as_chunks::<8>()
                                .0
                                .iter()
                                .flat_map(|b| (f64::from_le_bytes(*b) + 100.).to_le_bytes()),
                        );
                        records.push(self.data(&second));
                        if !scalar {
                            records.push(DATA);
                        }
                    }
                    (kind, sample.pod, sample.extent, self.group(&records))
                } else {
                    (0, 0, 0, self.property(p))
                };
                children.push(records);
                let info = kind
                    | (2 << 2)
                    | (u32::from(pod) << 4)
                    | ((extent as u32) << 12)
                    | (255 << 20)
                    | if p.name == "P" {
                        0x200
                    } else if kind != 0 {
                        0x800
                    } else {
                        0
                    };
                headers.extend_from_slice(&info.to_le_bytes());
                if kind != 0 {
                    headers
                        .extend_from_slice(&if p.name == "P" { 2u32 } else { 1u32 }.to_le_bytes());
                    if p.name == "P" {
                        headers.extend_from_slice(&1u32.to_le_bytes());
                        headers.extend_from_slice(&1u32.to_le_bytes());
                    }
                }
                headers.extend_from_slice(&(p.name.len() as u32).to_le_bytes());
                headers.extend_from_slice(p.name.as_bytes());
                let metadata = p
                    .metadata
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(";");
                headers.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
                headers.extend_from_slice(metadata.as_bytes());
            }
            if children.is_empty() {
                return 0;
            }
            children.push(self.data(&headers));
            self.group(&children)
        }
        fn object(&mut self, o: &Object) -> u64 {
            let mut children = vec![self.property(&o.properties)];
            let mut headers = Vec::new();
            for child in &o.children {
                children.push(self.object(child));
                let name = child.path.rsplit('/').next().unwrap();
                headers.extend_from_slice(&(name.len() as u32).to_le_bytes());
                headers.extend_from_slice(name.as_bytes());
                headers.push(255);
                let meta = child
                    .metadata
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(";");
                headers.extend_from_slice(&(meta.len() as u32).to_le_bytes());
                headers.extend_from_slice(meta.as_bytes());
            }
            headers.resize(headers.len() + 32, 0);
            children.push(self.data(&headers));
            self.group(&children)
        }
        fn archive(o: &Object) -> Vec<u8> {
            let mut w = Self(b"Ogawa\xff\0\x01\0\0\0\0\0\0\0\0".to_vec());
            let version = w.data(&0u32.to_le_bytes());
            let release = w.data(&10803u32.to_le_bytes());
            let object = w.object(o);
            let root = w.group(&[version, release, object, DATA, DATA, DATA]);
            w.0[8..16].copy_from_slice(&root.to_le_bytes());
            w.0
        }
    }

    #[test]
    fn actual_ogawa_records_read_earliest_geometry_sample() {
        let root = root(triangle("/mesh"));
        let bytes = Writer::archive(&root);
        let fixture = Fixture::new(&bytes);
        let scene = load(&fixture.0).unwrap();
        assert_eq!(scene.triangles.len(), 1);
        close(scene.triangles[0].vertices[1], [1., 0., 0.]);
        assert_eq!(scene.points.len(), 0);
    }
    #[test]
    fn hierarchical_transform_order_and_inheritance_reset() {
        let child = object(
            "/parent/child",
            "AbcGeom_Xform_v3",
            vec![xform(&[16., 96., 0.], &[2., 0., 0., 90., 2., 3., 1.], true)],
            vec![triangle("/parent/child/mesh")],
        );
        let reset = object(
            "/parent/reset",
            "AbcGeom_Xform_v3",
            vec![xform(&[16.], &[4., 5., 6.], false)],
            vec![triangle("/parent/reset/mesh")],
        );
        let parent = object(
            "/parent",
            "AbcGeom_Xform_v3",
            vec![xform(&[16.], &[10., 0., 0.], true)],
            vec![child, reset],
        );
        let scene = build_scene(&root(parent)).unwrap();
        close(scene.triangles[0].vertices[0], [12., 0., 0.]);
        close(scene.triangles[0].vertices[1], [12., 2., 0.]);
        close(scene.triangles[0].vertices[2], [9., 0., 0.]);
        close(scene.triangles[1].vertices[0], [4., 5., 6.]);
    }
    #[test]
    fn matrix_axis_angle_and_axis_rotations() {
        let matrix = xform(
            &[48.],
            &[
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 3., 4., 5., 1.,
            ],
            true,
        );
        close(
            super::xform(&matrix).unwrap().apply(Vec3::new(1., 2., 3.)),
            [4., 6., 8.],
        );
        let axis = xform(&[32.], &[0., 0., 2., 90.], true);
        close(
            super::xform(&axis).unwrap().apply(Vec3::new(1., 0., 0.)),
            [0., 1., 0.],
        );
        for (op, p, e) in [
            (64., [0., 1., 0.], [0., 0., 1.]),
            (80., [0., 0., 1.], [1., 0., 0.]),
            (96., [1., 0., 0.], [0., 1., 0.]),
        ] {
            close(
                super::xform(&xform(&[op], &[90.], true))
                    .unwrap()
                    .apply(Vec3::new(p[0], p[1], p[2])),
                e,
            );
        }
    }
    #[test]
    fn indexed_face_colors_and_subdivision_holes() {
        let mut color = compound(
            "Cs",
            vec![
                value(".vals", 3, &[1., 0., 0., 0., 1., 0.]),
                value(".indices", 1, &[0., 1.]),
            ],
        );
        color
            .metadata
            .insert("geoScope".to_owned(), "uni".to_owned());
        let geom = compound(
            ".geom",
            vec![
                value("P", 3, &[0., 0., 0., 1., 0., 0., 1., 1., 0., 0., 1., 0.]),
                value(".faceCounts", 1, &[3., 3.]),
                value(".faceIndices", 1, &[0., 1., 2., 0., 2., 3.]),
                value(".holes", 1, &[1.]),
                compound(".arbGeomParams", vec![color]),
            ],
        );
        let root = root(object("/subd", "AbcGeom_SubD_v1", vec![geom], Vec::new()));
        let scene = build_scene(&root).unwrap();
        assert_eq!(scene.triangles.len(), 1);
        assert_eq!(scene.triangles[0].color, Some([255, 0, 0]));
    }
    #[test]
    fn nested_instances_preserve_source_local_and_instance_parent_transforms() {
        let a = object(
            "/a",
            "AbcGeom_Xform_v3",
            vec![xform(&[16.], &[-2., 0., 0.], true)],
            vec![triangle("/a/mesh")],
        );
        let mut instance = object(
            "/b/i",
            "",
            vec![string(".instanceSource", "/a")],
            Vec::new(),
        );
        instance
            .metadata
            .insert("isInstance".to_owned(), "1".to_owned());
        let b = object(
            "/b",
            "AbcGeom_Xform_v3",
            vec![xform(&[16.], &[0., 5., 0.], true)],
            vec![instance],
        );
        let mut instance = object(
            "/c/i",
            "",
            vec![string(".instanceSource", "/b")],
            Vec::new(),
        );
        instance
            .metadata
            .insert("isInstance".to_owned(), "1".to_owned());
        let c = object(
            "/c",
            "AbcGeom_Xform_v3",
            vec![xform(&[16.], &[0., 0., 5.], true)],
            vec![instance],
        );
        let root = object("/", "", Vec::new(), vec![a, b, c]);
        let bytes = Writer::archive(&root);
        let fixture = Fixture::new(&bytes);
        let scene = load(&fixture.0).unwrap();
        assert_eq!(scene.triangles.len(), 3);
        close(scene.triangles[0].vertices[0], [-2., 0., 0.]);
        close(scene.triangles[1].vertices[0], [-2., 5., 0.]);
        close(scene.triangles[2].vertices[0], [-2., 5., 5.]);
    }
    #[test]
    fn point_cloud_colors_and_visibility() {
        let mut color = value("Cd", 3, &[0.25, 0., 0.]);
        color
            .metadata
            .insert("geoScope".to_owned(), "con".to_owned());
        let points = object(
            "/points",
            "AbcGeom_Points_v1",
            vec![compound(
                ".geom",
                vec![
                    value("P", 3, &[0., 0., 0., 1., 1., 1.]),
                    compound(".arbGeomParams", vec![color]),
                ],
            )],
            Vec::new(),
        );
        let mut hidden = triangle("/hidden");
        hidden.properties.children.push(value("visible", 1, &[0.]));
        let scene = build_scene(&object("/", "", Vec::new(), vec![points, hidden])).unwrap();
        assert_eq!(scene.points.len(), 2);
        assert!(scene.triangles.is_empty());
        assert_eq!(scene.points[0].color, Some([137, 0, 0]));
    }
    #[test]
    fn periodic_linear_and_sampled_cubic_curves() {
        let geom = compound(
            ".geom",
            vec![
                value("P", 3, &[0., 0., 0., 1., 0., 0., 1., 1., 0.]),
                value("nVertices", 1, &[3.]),
                value("curveBasisAndType", 4, &[1., 1., 0., 0.]),
            ],
        );
        let scene = build_scene(&root(object(
            "/curves",
            "AbcGeom_Curve_v2",
            vec![geom],
            Vec::new(),
        )))
        .unwrap();
        assert_eq!(scene.lines.len(), 3);
        close(scene.lines[2].vertices[1], [0., 0., 0.]);
        let geom = compound(
            ".geom",
            vec![
                value("P", 3, &[0., 0., 0., 0., 1., 0., 1., 1., 0., 1., 0., 0.]),
                value("nVertices", 1, &[4.]),
                value("curveBasisAndType", 4, &[0., 0., 1., 1.]),
            ],
        );
        let scene = build_scene(&root(object(
            "/curves",
            "AbcGeom_Curve_v2",
            vec![geom],
            Vec::new(),
        )))
        .unwrap();
        assert_eq!(scene.lines.len(), 24);
        close(scene.lines[0].vertices[0], [0., 0., 0.]);
        close(scene.lines[23].vertices[1], [1., 0., 0.]);
        close(scene.lines[11].vertices[1], [0.5, 0.75, 0.]);
    }
    #[test]
    fn typed_samples_big_endian_half_and_invalid_numbers() {
        let sample = Sample {
            pod: 10,
            extent: 3,
            big_endian: true,
            bytes: [1f32, 2., 3.]
                .iter()
                .flat_map(|v| v.to_be_bytes())
                .collect(),
        };
        assert_eq!(sample.numbers().unwrap(), vec![1., 2., 3.]);
        let sample = Sample {
            pod: 9,
            extent: 2,
            big_endian: false,
            bytes: [0x3c00u16, 0xc000u16]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect(),
        };
        assert_eq!(sample.numbers().unwrap(), vec![1., -2.]);
        let sample = Sample {
            pod: 11,
            extent: 1,
            big_endian: false,
            bytes: f64::NAN.to_le_bytes().to_vec(),
        };
        assert!(sample.numbers().is_err());
    }
    #[test]
    fn malformed_archives_and_unsupported_schemas_fail_cleanly() {
        let bytes = Writer::archive(&root(triangle("/mesh")));
        for len in [0, 5, 15, bytes.len() - 8] {
            assert!(Ogawa::new(&bytes[..len]).is_err());
        }
        let mut bytes = bytes.clone();
        bytes[5] = 0;
        assert!(Ogawa::new(&bytes).is_err());
        let mut bytes = bytes.clone();
        bytes[5] = 255;
        bytes[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(Ogawa::new(&bytes).is_err());
        let unsupported = root(object(
            "/patch",
            "AbcGeom_NuPatch_v1",
            Vec::new(),
            Vec::new(),
        ));
        assert!(
            build_scene(&unsupported)
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
        let mut instance = object("/i", "", vec![string(".instanceSource", "/i")], Vec::new());
        instance
            .metadata
            .insert("isInstance".to_owned(), "1".to_owned());
        assert!(
            build_scene(&root(instance))
                .unwrap_err()
                .to_string()
                .contains("cyclic")
        );
        assert!(super::xform(&xform(&[16.], &[1.], true)).is_err());
    }
}
