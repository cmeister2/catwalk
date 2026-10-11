//! Unit-normalized 3MF meshes and multipart production-extension assembly builds.
use super::{MAX_INPUT_BYTES, transform::Transform};
use crate::{
    Result,
    scene::{Scene, Triangle, Vec3},
};
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Seek, SeekFrom},
    path::Path,
};

/// XML namespace of the supported 3MF core model schema.
const CORE: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
/// XML namespace of the supported 3MF material resource extension.
const MATERIAL: &str = "http://schemas.microsoft.com/3dmanufacturing/material/2015/02";
/// XML namespace of the supported multipart 3MF production extension.
const PRODUCTION: &str = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";
/// Open Packaging Conventions relationship XML namespace.
const RELATIONSHIPS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
/// Relationship type identifying a 3MF model XML part.
const MODEL_RELATIONSHIP: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
/// Maximum source or expanded preview triangle count.
const MAX_TRIANGLES: usize = 5_000_000;
/// Maximum model parts referenced by a multipart package.
const MAX_PARTS: usize = 256;
/// Maximum aggregate resources or physical ZIP directory entries.
const MAX_RESOURCES: usize = 100_000;
/// Maximum aggregate source object references.
const MAX_REFERENCES: usize = 1_000_000;
/// Maximum recursive reference, geometry or partition depth.
const MAX_DEPTH: usize = 256;

/// A local or production-part object reference with its unit-normalized placement.
struct Reference {
    /// Optional canonical production-part path; absence refers to the current part.
    part: Option<String>,
    /// Object resource identifier scoped to the referenced model part.
    object: u32,
    /// Local-to-parent affine placement in normalized scene units.
    transform: Transform,
}

/// One mesh or component object with its part-local resource identity.
struct Object {
    /// Authored unit-normalized mesh triangles belonging to the object.
    triangles: Vec<Triangle>,
    /// Placed references to component objects contained by this object.
    components: Vec<Reference>,
}

/// Part-local objects and build references decoded from one model XML part.
struct ModelPart {
    /// Decoded objects indexed by identifiers local to this XML part.
    objects: HashMap<u32, Object>,
    /// Authored build-item references; only the root part drives instantiation.
    build: Vec<Reference>,
}

/// Aggregate source/decompression/resource limits shared across referenced model parts.
#[derive(Default)]
struct Budget {
    /// Aggregate decompressed XML bytes read across referenced package parts.
    bytes: u64,
    /// Aggregate source mesh vertex count across decoded model parts.
    vertices: usize,
    /// Aggregate source mesh triangle count across decoded model parts.
    triangles: usize,
    /// Aggregate number of decoded resource records.
    resources: usize,
    /// Aggregate number of decoded object references.
    references: usize,
}

/// Increment one aggregate model count without crossing its limit.
///
/// # Errors
///
/// The count overflows or exceeds the supplied budget.
fn consume(count: &mut usize, limit: usize, label: &str) -> Result<()> {
    *count = count.checked_add(1).ok_or("3MF count overflow")?;
    if *count > limit {
        return Err(format!("3MF exceeds {label} limit").into());
    }
    Ok(())
}

/// Read a fixed-width raw ZIP directory field.
///
/// # Errors
///
/// The requested byte range is truncated or cannot represent the fixed-size field.
fn zip_number<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N]> {
    Ok(bytes
        .get(offset..)
        .and_then(|tail| tail.get(..N))
        .ok_or("truncated 3MF ZIP directory field")?
        .try_into()?)
}

/// Decode a little-endian 16-bit ZIP directory field.
///
/// # Errors
///
/// The requested field is truncated.
fn zip_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(zip_number(bytes, offset)?))
}

/// Decode a little-endian 32-bit ZIP directory field.
///
/// # Errors
///
/// The requested field is truncated.
fn zip_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(zip_number(bytes, offset)?))
}

/// Decode a little-endian 64-bit ZIP directory field.
///
/// # Errors
///
/// The requested field is truncated.
fn zip_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(zip_number(bytes, offset)?))
}

/// Seek and read an exact raw package byte range.
///
/// # Errors
///
/// Seeking or reading fails, including a truncated requested range.
fn zip_read(reader: &mut (impl Read + Seek), offset: u64, length: usize) -> Result<Vec<u8>> {
    reader.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// Locate a valid ZIP64 end record, including prefixed/extensible archives.
///
/// # Errors
///
/// The end record is missing, inconsistent, truncated or unreadable.
fn zip64_end(
    reader: &mut (impl Read + Seek),
    locator: u64,
    declared: u64,
) -> Result<(u64, Vec<u8>)> {
    // The usual record has 56 bytes. Also accept an extensible record at the
    // locator's declared offset, including archives with a leading prefix.
    for offset in [declared, locator.saturating_sub(56)] {
        if offset.checked_add(56).is_none_or(|end| end > locator) {
            continue;
        }
        let header = zip_read(reader, offset, 56)?;
        if &header[..4] == b"PK\x06\x06" {
            let size = zip_u64(&header, 4)?;
            if size >= 44
                && offset.checked_add(size).and_then(|n| n.checked_add(12)) == Some(locator)
            {
                return Ok((offset, header));
            }
        }
    }
    // A ZIP64 record with both an archive prefix and extensible data cannot
    // be located directly from the relative locator offset. Scan backwards
    // in bounded chunks, retaining overlap for a complete fixed header.
    let mut end = locator;
    while end > declared {
        let start = end.saturating_sub(65_536).max(declared);
        let bytes = zip_read(reader, start, usize::try_from(end - start)?)?;
        for (index, header) in bytes.windows(56).enumerate().rev() {
            if &header[..4] != b"PK\x06\x06" {
                continue;
            }
            let offset = start + index as u64;
            let size = zip_u64(header, 4)?;
            if size >= 44
                && offset.checked_add(size).and_then(|n| n.checked_add(12)) == Some(locator)
            {
                return Ok((offset, header.to_vec()));
            }
        }
        if start == declared {
            break;
        }
        end = start + 55;
    }
    Err("invalid 3MF ZIP64 end-of-directory record".into())
}

/// Inspect the raw central directory before `ZipArchive` coalesces identical
/// names. PKWARE APPNOTE sections 4.3.12--4.3.16 define these fixed headers;
/// compressed entry data and ZIP64 per-entry extras remain the ZIP reader's
/// responsibility. Count and name checks apply to every physical record.
#[allow(
    clippy::too_many_lines,
    reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
)]
fn validate_zip_directory(reader: &mut (impl Read + Seek), length: u64) -> Result<usize> {
    if !(22..=MAX_INPUT_BYTES).contains(&length) {
        return Err("invalid or oversized 3MF ZIP package".into());
    }
    let tail_length = length.min(65_557);
    let tail_start = length - tail_length;
    let tail = zip_read(reader, tail_start, usize::try_from(tail_length)?)?;
    let (index, eocd) = tail
        .windows(22)
        .enumerate()
        .rev()
        .find(|(index, record)| {
            &record[..4] == b"PK\x05\x06"
                && index + 22 + usize::from(u16::from_le_bytes([record[20], record[21]]))
                    == tail.len()
        })
        .ok_or("3MF ZIP end-of-directory record is missing")?;
    let eocd_start = tail_start + index as u64;
    let disk = zip_u16(eocd, 4)?;
    let directory_disk = zip_u16(eocd, 6)?;
    let disk_entries = zip_u16(eocd, 8)?;
    let entries = zip_u16(eocd, 10)?;
    let size = zip_u32(eocd, 12)?;
    let offset = zip_u32(eocd, 16)?;
    let needs_zip64 = [disk, directory_disk, disk_entries, entries].contains(&u16::MAX)
        || [size, offset].contains(&u32::MAX);
    let locator = if eocd_start >= 20 {
        zip_read(reader, eocd_start - 20, 20)?
    } else {
        Vec::new()
    };
    let (entries, size, offset, directory_end) = if locator.starts_with(b"PK\x06\x07") {
        if zip_u32(&locator, 4)? != 0 || zip_u32(&locator, 16)? != 1 {
            return Err("multi-disk 3MF ZIP packages are unsupported".into());
        }
        let declared = zip_u64(&locator, 8)?;
        let (end_start, record) = zip64_end(reader, eocd_start - 20, declared)?;
        if declared > end_start
            || zip_u32(&record, 16)? != 0
            || zip_u32(&record, 20)? != 0
            || zip_u64(&record, 24)? != zip_u64(&record, 32)?
        {
            return Err("invalid or multi-disk 3MF ZIP64 directory".into());
        }
        let full_entries = zip_u64(&record, 32)?;
        let full_size = zip_u64(&record, 40)?;
        let full_offset = zip_u64(&record, 48)?;
        for (legacy, sentinel, actual) in [
            (u64::from(disk), u64::from(u16::MAX), 0),
            (u64::from(directory_disk), u64::from(u16::MAX), 0),
            (u64::from(disk_entries), u64::from(u16::MAX), full_entries),
            (u64::from(entries), u64::from(u16::MAX), full_entries),
            (u64::from(size), u64::from(u32::MAX), full_size),
            (u64::from(offset), u64::from(u32::MAX), full_offset),
        ] {
            if legacy != sentinel && legacy != actual {
                return Err("inconsistent 3MF ZIP and ZIP64 directory fields".into());
            }
        }
        (full_entries, full_size, full_offset, end_start)
    } else {
        if needs_zip64 {
            return Err("3MF ZIP64 locator is missing".into());
        }
        if disk != 0 || directory_disk != 0 || disk_entries != entries {
            return Err("multi-disk 3MF ZIP packages are unsupported".into());
        }
        (
            u64::from(entries),
            u64::from(size),
            u64::from(offset),
            eocd_start,
        )
    };
    if entries > MAX_RESOURCES as u64 {
        return Err("3MF package has too many archive entries".into());
    }
    let directory_start = directory_end
        .checked_sub(size)
        .filter(|&start| start >= offset)
        .ok_or("invalid 3MF ZIP central-directory extent")?;
    let mut position = directory_start;
    let mut names = HashSet::new();
    for _ in 0..entries {
        if position
            .checked_add(46)
            .is_none_or(|end| end > directory_end)
        {
            return Err("truncated 3MF ZIP central-directory header".into());
        }
        let header = zip_read(reader, position, 46)?;
        if !header.starts_with(b"PK\x01\x02") {
            return Err("invalid 3MF ZIP central-directory header".into());
        }
        let name_length = u64::from(zip_u16(&header, 28)?);
        let extra_length = u64::from(zip_u16(&header, 30)?);
        let comment_length = u64::from(zip_u16(&header, 32)?);
        let next = position
            .checked_add(46 + name_length + extra_length + comment_length)
            .filter(|&end| end <= directory_end)
            .ok_or("truncated 3MF ZIP central-directory entry")?;
        let name = zip_read(reader, position + 46, usize::try_from(name_length)?)?;
        if !names.insert(name) {
            return Err("duplicate 3MF package part name".into());
        }
        position = next;
    }
    if position != directory_end {
        // A central-directory digital-signature record is optional and is
        // metadata only. Its bounded payload is not a model or an entry.
        if position
            .checked_add(6)
            .is_none_or(|end| end > directory_end)
        {
            return Err("3MF ZIP directory size or entry count is inconsistent".into());
        }
        let signature = zip_read(reader, position, 6)?;
        if !signature.starts_with(b"PK\x05\x05")
            || position + 6 + u64::from(zip_u16(&signature, 4)?) != directory_end
        {
            return Err("3MF ZIP directory size or entry count is inconsistent".into());
        }
    }
    Ok(usize::try_from(entries)?)
}

/// Model part names are OPC package URIs, never filesystem paths. Cache the
/// indexed ZIP entries so aliases cannot select an ambiguous duplicate part.
struct Package {
    /// Opened ZIP reader retaining the package file handle.
    archive: zip::ZipArchive<std::fs::File>,
    /// Canonical package part names mapped to unambiguous ZIP entry indices.
    entries: HashMap<String, usize>,
}

impl Package {
    /// Open a bounded ZIP package and index unambiguous canonical part names.
    ///
    /// # Errors
    ///
    /// File/ZIP validation, duplicate names or package limits fail.
    fn open(path: &Path) -> Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let length = file.metadata()?.len();
        if length > MAX_INPUT_BYTES {
            return Err("3MF package exceeds the 512 MiB limit".into());
        }
        let physical_entries = validate_zip_directory(&mut file, length)?;
        let mut archive = zip::ZipArchive::new(file)?;
        if archive.len() != physical_entries {
            // Also catch aliases which ZIP filename decoding maps to the
            // same Unicode name before its public archive index is built.
            return Err("duplicate decoded 3MF package part name".into());
        }
        if archive.len() > MAX_RESOURCES {
            return Err("3MF package has too many archive entries".into());
        }
        let mut entries = HashMap::new();
        for index in 0..archive.len() {
            let entry = archive.by_index(index)?;
            if entry.is_dir() {
                continue;
            }
            // ZIP item names are decoded names, whereas relationship and
            // production targets are URIs. A literal '%' in an item name must
            // not be interpreted as a second URI escape.
            let path = part_name(entry.name())?;
            if entries.insert(path, index).is_some() {
                return Err("duplicate 3MF package part name".into());
            }
        }
        Ok(Self { archive, entries })
    }

    /// Test whether the canonical package part name exists in the archive index.
    fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    /// Read a referenced UTF-8 XML part within the aggregate decompression budget.
    ///
    /// # Errors
    ///
    /// The part is missing, unreadable, invalidly encoded or exceeds decompression limits.
    fn read(&mut self, path: &str, budget: &mut Budget) -> Result<String> {
        let index = *self
            .entries
            .get(path)
            .ok_or_else(|| format!("3MF references a missing package part {path}"))?;
        let mut entry = self.archive.by_index(index)?;
        if entry.size() > MAX_INPUT_BYTES.saturating_sub(budget.bytes) {
            return Err("3MF decompressed XML exceeds the aggregate 512 MiB limit".into());
        }
        let mut bytes = Vec::new();
        entry
            .by_ref()
            .take(MAX_INPUT_BYTES.saturating_sub(budget.bytes) + 1)
            .read_to_end(&mut bytes)?;
        budget.bytes = budget
            .bytes
            .checked_add(bytes.len() as u64)
            .filter(|&n| n <= MAX_INPUT_BYTES)
            .ok_or("3MF decompressed XML exceeds the aggregate 512 MiB limit")?;
        Ok(String::from_utf8(bytes)?)
    }
}

/// Load the root 3MF build and referenced production-extension model parts.
///
/// # Errors
///
/// ZIP/XML structure, local references, resources, geometry or preview budgets are invalid or unsupported.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let mut package = Package::open(path)?;
    let mut budget = Budget::default();
    let root = if package.contains("_rels/.rels") {
        let xml = package.read("_rels/.rels", &mut budget)?;
        let targets = model_relationships(&xml, "")?;
        if targets.len() != 1 {
            return Err("3MF package must have exactly one root model relationship".into());
        }
        targets
            .into_iter()
            .next()
            .ok_or("3MF root model is missing")?
    } else {
        package_path("3D/3dmodel.model")?
    };
    let xml = package.read(&root, &mut budget)?;
    let root_part = parse_part(&xml, true, &mut budget)?;
    let mut targets: HashSet<_> = root_part
        .objects
        .values()
        .flat_map(|object| &object.components)
        .chain(&root_part.build)
        .filter_map(|reference| reference.part.clone())
        .collect();
    if targets.len() >= MAX_PARTS {
        return Err("3MF package exceeds the model-part limit".into());
    }
    if !targets.is_empty() {
        let (directory, name) = root.rsplit_once('/').unwrap_or(("", root.as_str()));
        let relationships = if directory.is_empty() {
            format!("_rels/{name}.rels")
        } else {
            format!("{directory}/_rels/{name}.rels")
        };
        let xml = package.read(&relationships, &mut budget)?;
        let declared = model_relationships(&xml, directory)?;
        if targets.iter().any(|target| !declared.contains(target)) {
            return Err("3MF production part lacks a model relationship".into());
        }
    }
    targets.remove(&root);
    let mut targets: Vec<_> = targets.into_iter().collect();
    targets.sort();
    let mut parts = HashMap::from([(root.clone(), root_part)]);
    for target in targets {
        let xml = package.read(&target, &mut budget)?;
        parts.insert(target, parse_part(&xml, false, &mut budget)?);
    }
    render_parts(&root, &parts)
}

/// Percent-decode a package URI without permitting filesystem/remote targets.
///
/// # Errors
///
/// Escapes, UTF-8, delimiters or forbidden path characters are invalid.
fn decode_uri(target: &str) -> Result<String> {
    let bytes = target.as_bytes();
    let mut decoded = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err("invalid escape in 3MF package target".into());
            }
            decoded.push(u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3])?,
                16,
            )?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    Ok(String::from_utf8(decoded)?)
}

/// Validate and canonicalize an already decoded package part name.
///
/// # Errors
///
/// The name contains invalid, empty, relative or traversal segments.
fn part_name(value: &str) -> Result<String> {
    let path = value.strip_prefix('/').unwrap_or(value);
    if value.contains([':', '\0', '\\', '?', '#'])
        || path.is_empty()
        || path.split('/').any(|p| matches!(p, "" | "." | ".."))
    {
        return Err("invalid 3MF package target".into());
    }
    Ok(path.to_ascii_lowercase())
}

/// Decode and canonicalize a package-local model part URI.
///
/// # Errors
///
/// The URI is malformed, external or contains invalid/traversal segments.
fn package_path(target: &str) -> Result<String> {
    part_name(&decode_uri(target)?)
}

/// Resolve a relationship target relative to its package part directory.
///
/// # Errors
///
/// The target is external, malformed or cannot resolve to a valid package-local part.
fn relationship_target(target: &str, directory: &str) -> Result<String> {
    let target = decode_uri(target)?;
    if target.contains([':', '\0', '\\', '?', '#']) || target.starts_with("//") {
        return Err("invalid 3MF relationship target".into());
    }
    let mut segments: Vec<_> = if target.starts_with('/') || directory.is_empty() {
        Vec::new()
    } else {
        directory.split('/').collect()
    };
    for segment in target.strip_prefix('/').unwrap_or(&target).split('/') {
        match segment {
            "" => return Err("invalid 3MF relationship target".into()),
            "." => {}
            ".." => {
                segments
                    .pop()
                    .ok_or("3MF relationship escapes the package")?;
            }
            value => segments.push(value),
        }
    }
    part_name(&segments.join("/"))
}

/// Collect supported internal model-part relationship targets.
///
/// # Errors
///
/// XML relationships are invalid, external, duplicated or above the part limit.
fn model_relationships(xml: &str, directory: &str) -> Result<HashSet<String>> {
    let doc = roxmltree::Document::parse(xml)?;
    let root = doc.root_element();
    if !root.has_tag_name((RELATIONSHIPS, "Relationships")) {
        return Err("expected 3MF OPC Relationships element".into());
    }
    let mut ids = HashSet::new();
    let mut targets = HashSet::new();
    for relation in root
        .children()
        .filter(|n| n.has_tag_name((RELATIONSHIPS, "Relationship")))
    {
        if !ids.insert(attribute(relation, "Id")?) {
            return Err("duplicate 3MF relationship ID".into());
        }
        if relation.attribute("Type") != Some(MODEL_RELATIONSHIP) {
            continue;
        }
        if relation
            .attribute("TargetMode")
            .is_some_and(|mode| mode != "Internal")
        {
            return Err("external 3MF model relationships are unsupported".into());
        }
        let target = attribute(relation, "Target")?;
        let target = relationship_target(target, directory)?;
        if !targets.insert(target) {
            return Err("duplicate 3MF model relationship target".into());
        }
        if targets.len() > MAX_PARTS {
            return Err("3MF relationships exceed the model-part limit".into());
        }
    }
    Ok(targets)
}

/// Read a required unqualified 3MF XML attribute.
///
/// # Errors
///
/// The element lacks the named attribute.
fn attribute<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Result<&'a str> {
    node.attribute(name)
        .ok_or_else(|| format!("3MF {} is missing {name}", node.tag_name().name()).into())
}

/// Parse a required numeric 3MF resource/reference identifier.
///
/// # Errors
///
/// The attribute is missing or is not an unsigned 32-bit integer.
fn id(node: roxmltree::Node<'_, '_>, name: &str) -> Result<u32> {
    Ok(attribute(node, name)?.parse()?)
}

/// Find an optional unique child in the 3MF core namespace.
///
/// # Errors
///
/// The element contains duplicate core children of the requested name.
fn core_child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Result<Option<roxmltree::Node<'a, 'input>>> {
    let mut matches = node.children().filter(|n| n.has_tag_name((CORE, name)));
    let child = matches.next();
    if matches.next().is_some() {
        return Err(format!("duplicate 3MF {name} element").into());
    }
    Ok(child)
}

/// Decode supported #RRGGBB or #RRGGBBAA color syntax to its RGB channels.
///
/// # Errors
///
/// The spelling has an invalid prefix, length, character set or hexadecimal components.
fn color(value: &str) -> Result<[u8; 3]> {
    let value = value
        .strip_prefix('#')
        .ok_or("3MF color must start with #")?;
    if (value.len() != 6 && value.len() != 8) || !value.is_ascii() {
        return Err("3MF color must be #RRGGBB or #RRGGBBAA".into());
    }
    Ok([
        u8::from_str_radix(&value[0..2], 16)?,
        u8::from_str_radix(&value[2..4], 16)?,
        u8::from_str_radix(&value[4..6], 16)?,
    ])
}

/// Resolve a supported 3MF model unit to millimetres.
///
/// # Errors
///
/// The unit spelling is unsupported.
fn unit_scale(model: roxmltree::Node<'_, '_>) -> Result<f64> {
    match model.attribute("unit").unwrap_or("millimeter") {
        "micron" => Ok(0.001),
        "millimeter" => Ok(1.),
        "centimeter" => Ok(10.),
        "inch" => Ok(25.4),
        "foot" => Ok(304.8),
        "meter" => Ok(1000.),
        _ => Err("unsupported 3MF model unit".into()),
    }
}

/// Read a placed component/build reference with package-local production-path rules.
///
/// # Errors
///
/// Paths, namespaces, identifiers or transforms are invalid/unsupported, or reference limits are exceeded.
fn reference(
    node: roxmltree::Node<'_, '_>,
    root: bool,
    unit: f64,
    budget: &mut Budget,
) -> Result<Reference> {
    consume(&mut budget.references, MAX_REFERENCES, "object reference")?;
    let mut part = None;
    for value in node.attributes().filter(|value| value.name() == "path") {
        if value.namespace() != Some(PRODUCTION) {
            return Err("3MF path attribute must use the production namespace".into());
        }
        if !root {
            return Err("3MF production paths are allowed only in the root model part".into());
        }
        if !value.value().starts_with('/') {
            return Err("3MF production path must be absolute within the package".into());
        }
        part = Some(package_path(value.value())?);
    }
    let mut transform = Transform::from_3mf(node.attribute("transform"))?;
    // Translations use the referencing model's units; the linear matrix is
    // dimensionless. Normalizing both vertices and translations avoids scaling
    // a parent's translation with a referenced child's different unit.
    for translation in &mut transform.0[3][..3] {
        *translation *= unit;
    }
    if transform.0.iter().flatten().any(|n| !n.is_finite()) {
        return Err("3MF transform exceeds numeric range".into());
    }
    Ok(Reference {
        part,
        object: id(node, "objectid")?,
        transform,
    })
}

/// Read one unit-normalized model part with resource identifiers scoped to that part.
///
/// # Errors
///
/// Core/extension XML, resources, meshes, materials or aggregate budgets are invalid or unsupported.
#[allow(
    clippy::cast_possible_truncation,
    reason = "The mean of three bounded RGB bytes remains within the byte range."
)]
#[allow(
    clippy::too_many_lines,
    reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
)]
fn parse_part(xml: &str, root: bool, budget: &mut Budget) -> Result<ModelPart> {
    let doc = roxmltree::Document::parse(xml)?;
    let model = doc.root_element();
    if !model.has_tag_name((CORE, "model")) {
        return Err("expected a 3MF core model element".into());
    }
    if let Some(extensions) = model.attribute("requiredextensions") {
        for prefix in extensions.split_whitespace() {
            let namespace = model.lookup_namespace_uri(Some(prefix));
            // Material color groups are sufficient for a shaded preview; more complex materials are rejected below.
            if !matches!(namespace, Some(MATERIAL | PRODUCTION)) {
                return Err(format!("required 3MF extension {prefix} is not implemented").into());
            }
        }
    }
    let unit = unit_scale(model)?;
    let resources = core_child(model, "resources")?.ok_or("3MF model has no resources")?;
    let mut all_ids = HashSet::new();
    let mut colors: HashMap<u32, Vec<[u8; 3]>> = HashMap::new();
    for resource in resources.children().filter(roxmltree::Node::is_element) {
        consume(&mut budget.resources, MAX_RESOURCES, "resource")?;
        let resource_id = id(resource, "id")?;
        if resource_id == 0 {
            return Err("3MF resource IDs must be positive".into());
        }
        if !all_ids.insert(resource_id) {
            return Err("duplicate 3MF resource ID".into());
        }
        match resource.tag_name().name() {
            "basematerials" | "colorgroup" => {
                let palette = resource
                    .children()
                    .filter(roxmltree::Node::is_element)
                    .map(|n| {
                        color(attribute(
                            n,
                            if n.tag_name().name() == "base" {
                                "displaycolor"
                            } else {
                                "color"
                            },
                        )?)
                    })
                    .collect::<Result<Vec<_>>>()?;
                colors.insert(resource_id, palette);
            }
            "object" => {}
            // Complex texture/material properties cannot be rendered faithfully by this importer.
            "texture2d" | "texture2dgroup" | "compositematerials" | "multiproperties" => {
                return Err("3MF textures and composite materials are not implemented".into());
            }
            _ => {
                return Err(
                    format!("unsupported 3MF resource {}", resource.tag_name().name()).into(),
                );
            }
        }
    }
    let mut objects = HashMap::new();
    for resource in resources
        .children()
        .filter(|n| n.has_tag_name((CORE, "object")))
    {
        let object_id = id(resource, "id")?;
        let property_group: Option<u32> = resource.attribute("pid").map(str::parse).transpose()?;
        let object_pindex: usize = resource
            .attribute("pindex")
            .map(str::parse)
            .transpose()?
            .unwrap_or(0);
        let mut object = Object {
            triangles: Vec::new(),
            components: Vec::new(),
        };
        let mesh = core_child(resource, "mesh")?;
        let components = core_child(resource, "components")?;
        if mesh.is_some() && components.is_some() {
            return Err("3MF object cannot contain both mesh and components".into());
        }
        if let Some(mesh) = mesh {
            let vertices_node = core_child(mesh, "vertices")?.ok_or("3MF mesh has no vertices")?;
            let mut vertices = Vec::new();
            for node in vertices_node
                .children()
                .filter(|n| n.has_tag_name((CORE, "vertex")))
            {
                consume(&mut budget.vertices, MAX_TRIANGLES, "source vertex")?;
                let vertex = Vec3::new(
                    attribute(node, "x")?.parse()?,
                    attribute(node, "y")?.parse()?,
                    attribute(node, "z")?.parse()?,
                ) * unit;
                if !vertex.is_finite() {
                    return Err("3MF contains non-finite vertices".into());
                }
                vertices.push(vertex);
            }
            let triangles = core_child(mesh, "triangles")?.ok_or("3MF mesh has no triangles")?;
            for node in triangles
                .children()
                .filter(|n| n.has_tag_name((CORE, "triangle")))
            {
                consume(&mut budget.triangles, MAX_TRIANGLES, "source triangle")?;
                let indices = [id(node, "v1")?, id(node, "v2")?, id(node, "v3")?];
                if indices.iter().any(|&i| i as usize >= vertices.len()) {
                    return Err("3MF triangle references a missing vertex".into());
                }
                let pid: Option<u32> = node
                    .attribute("pid")
                    .map(str::parse)
                    .transpose()?
                    .or(property_group);
                let triangle_color = if let Some(pid) = pid {
                    let palette = colors
                        .get(&pid)
                        .ok_or("3MF color resource is missing or unsupported")?;
                    let p1: usize = node
                        .attribute("p1")
                        .map(str::parse)
                        .transpose()?
                        .unwrap_or(object_pindex);
                    let p2: usize = node
                        .attribute("p2")
                        .map(str::parse)
                        .transpose()?
                        .unwrap_or(p1);
                    let p3: usize = node
                        .attribute("p3")
                        .map(str::parse)
                        .transpose()?
                        .unwrap_or(p1);
                    if [p1, p2, p3].iter().any(|&i| i >= palette.len()) {
                        return Err("3MF color index is out of range".into());
                    }
                    Some(std::array::from_fn(|channel| {
                        ((u32::from(palette[p1][channel])
                            + u32::from(palette[p2][channel])
                            + u32::from(palette[p3][channel]))
                            / 3) as u8
                    }))
                } else {
                    None
                };
                object.triangles.push(Triangle {
                    vertices: indices.map(|i| vertices[i as usize]),
                    color: triangle_color,
                });
            }
        }
        if let Some(components) = components {
            for component in components
                .children()
                .filter(|n| n.has_tag_name((CORE, "component")))
            {
                object
                    .components
                    .push(reference(component, root, unit, budget)?);
            }
        }
        objects.insert(object_id, object);
    }
    let mut build = Vec::new();
    if root {
        let element = core_child(model, "build")?.ok_or("3MF root model has no build")?;
        for item in element
            .children()
            .filter(|n| n.has_tag_name((CORE, "item")))
        {
            build.push(reference(item, true, unit, budget)?);
        }
    }
    // Production consumers ignore child builds, including invalid or absent
    // child build entries. Only the selected root model drives the scene.
    Ok(ModelPart { objects, build })
}

/// Validate the multipart object graph and instantiate only the root build items.
///
/// # Errors
///
/// Parts/objects are missing, cyclic or exceed nesting/output limits.
fn render_parts(root: &str, parts: &HashMap<String, ModelPart>) -> Result<Scene> {
    let mut heights = HashMap::new();
    let mut active = HashSet::new();
    for (name, part) in parts {
        for &id in part.objects.keys() {
            validate_object(name, id, parts, &mut heights, &mut active)?;
        }
    }
    let part = parts.get(root).ok_or("3MF root part is missing")?;
    let mut scene = Scene::default();
    let mut instances = 0;
    for item in &part.build {
        instantiate(
            item.part.as_deref().unwrap_or(root),
            item.object,
            item.transform,
            parts,
            &mut scene,
            &mut instances,
        )?;
    }
    scene.validate()?;
    Ok(scene)
}

/// Validate reachable component dependencies and memoize their bounded expanded triangle counts.
///
/// # Errors
///
/// Object/part references are missing, cyclic or exceed depth/triangle limits.
fn validate_object(
    part: &str,
    id: u32,
    parts: &HashMap<String, ModelPart>,
    heights: &mut HashMap<(String, u32), usize>,
    active: &mut HashSet<(String, u32)>,
) -> Result<usize> {
    let key = (part.to_owned(), id);
    if let Some(height) = heights.get(&key) {
        return Ok(*height);
    }
    if active.len() >= MAX_DEPTH || !active.insert(key.clone()) {
        return Err("3MF components are cyclic or too deep".into());
    }
    let object = parts
        .get(part)
        .ok_or("3MF references a missing model part")?
        .objects
        .get(&id)
        .ok_or("3MF references a missing object in its model part")?;
    let mut height = 1;
    for child in &object.components {
        height = height.max(
            1 + validate_object(
                child.part.as_deref().unwrap_or(part),
                child.object,
                parts,
                heights,
                active,
            )?,
        );
    }
    if height > MAX_DEPTH {
        return Err("3MF components exceed the nesting limit".into());
    }
    active.remove(&key);
    heights.insert(key, height);
    Ok(height)
}

/// Append one placed object and recursively expand its referenced components.
///
/// # Errors
///
/// References or transforms are invalid, cyclic or exceed recursion/output limits.
fn instantiate(
    part: &str,
    id: u32,
    transform: Transform,
    parts: &HashMap<String, ModelPart>,
    scene: &mut Scene,
    instances: &mut usize,
) -> Result<()> {
    consume(instances, MAX_REFERENCES, "instantiated object")?;
    if transform.0.iter().flatten().any(|value| !value.is_finite()) {
        return Err("3MF composed transform exceeds numeric range".into());
    }
    let object = parts
        .get(part)
        .ok_or("3MF references a missing model part")?
        .objects
        .get(&id)
        .ok_or("3MF references a missing object in its model part")?;
    if scene.triangles.len().saturating_add(object.triangles.len()) > MAX_TRIANGLES {
        return Err("3MF instantiated scene exceeds triangle limit".into());
    }
    scene
        .triangles
        .extend(object.triangles.iter().map(|triangle| Triangle {
            vertices: triangle.vertices.map(|v| transform.apply(v)),
            color: triangle.color,
        }));
    for child in &object.components {
        instantiate(
            child.part.as_deref().unwrap_or(part),
            child.object,
            transform.then(child.transform),
            parts,
            scene,
            instances,
        )?;
    }
    Ok(())
}

#[cfg(test)]
/// Parse a single authored XML part without constructing a ZIP test package.
fn parse(xml: &str) -> Result<Scene> {
    let part = parse_part(xml, true, &mut Budget::default())?;
    render_parts("root.model", &HashMap::from([("root.model".into(), part)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use std::io::Write;

    fn model(resources: &str, build: &str) -> String {
        format!(
            "<model xmlns=\"{CORE}\"><resources>{resources}</resources><build>{build}</build></model>"
        )
    }
    const MESH: &str = "<object id=\"1\"><mesh><vertices><vertex x=\"0\" y=\"0\" z=\"0\"/><vertex x=\"1\" y=\"0\" z=\"0\"/><vertex x=\"0\" y=\"1\" z=\"0\"/></vertices><triangles><triangle v1=\"0\" v2=\"1\" v3=\"2\"/></triangles></mesh></object>";
    #[test]
    fn build_and_component_transforms_compose() {
        let xml = model(
            &format!(
                "{MESH}<object id=\"2\"><components><component objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 10 0 0\"/></components></object>"
            ),
            "<item objectid=\"2\" transform=\"2 0 0 0 2 0 0 0 2 0 3 0\"/>",
        );
        let scene = parse(&xml).unwrap();
        assert_eq!(scene.triangles[0].vertices[0], Vec3::new(20., 3., 0.));
        assert_eq!(scene.triangles[0].vertices[1], Vec3::new(22., 3., 0.));
    }
    #[test]
    fn cycles_missing_objects_and_bad_indices_fail() {
        assert!(
            parse(&model(
                "<object id=\"1\"><components><component objectid=\"1\"/></components></object>",
                "<item objectid=\"1\"/>"
            ))
            .is_err()
        );
        assert!(parse(&model(MESH, "<item objectid=\"2\"/>")).is_err());
        assert!(
            parse(&model(
                &MESH.replace("v3=\"2\"", "v3=\"3\""),
                "<item objectid=\"1\"/>"
            ))
            .is_err()
        );
    }
    #[test]
    fn package_paths_decode_and_validate() {
        assert_eq!(package_path("/3D/a%20b.model").unwrap(), "3d/a b.model");
        assert_eq!(package_path("/3D/100%25.model").unwrap(), "3d/100%.model");
        assert_eq!(
            relationship_target("../parts/a.model", "3d").unwrap(),
            "parts/a.model"
        );
        for path in [
            "../a.model",
            "file:/tmp/a.model",
            "//3D/a.model",
            "/3D/%2e%2e/a.model",
            "/3D/a%2fb.model/../c.model",
            "/3D/a.model?x",
            "/3D/a%00.model",
        ] {
            assert!(package_path(path).is_err(), "accepted {path}");
        }
        assert!(relationship_target("../../a.model", "3d").is_err());
    }

    fn production_model(unit: &str, resources: &str, build: Option<&str>) -> String {
        let build = build
            .map(|items| format!("<build>{items}</build>"))
            .unwrap_or_default();
        format!(
            "<model xmlns=\"{CORE}\" xmlns:p=\"{PRODUCTION}\" requiredextensions=\"p\" unit=\"{unit}\"><resources>{resources}</resources>{build}</model>"
        )
    }

    fn relationships(targets: &[&str]) -> String {
        let relations = targets
            .iter()
            .enumerate()
            .fold(String::new(), |mut text, (i, target)| {
                let _ = write!(
                    text,
                    "<Relationship Type=\"{MODEL_RELATIONSHIP}\" Target=\"{target}\" Id=\"r{i}\"/>"
                );
                text
            });
        format!("<Relationships xmlns=\"{RELATIONSHIPS}\">{relations}</Relationships>")
    }

    /// Each test owns a unique archive and removes it even after an error.
    fn with_package<T>(
        entries: &[(&str, &str)],
        read: impl FnOnce(&Path) -> Result<T>,
    ) -> Result<T> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        struct Temp(std::path::PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let path = Temp(std::env::temp_dir().join(format!(
            "catwalk-3mf-test-{}-{}.3mf",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )));
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path.0)?);
        for &(name, contents) in entries {
            writer.start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )?;
            writer.write_all(contents.as_bytes())?;
        }
        writer.finish()?;
        read(&path.0)
    }

    fn package_scene(entries: &[(&str, &str)]) -> Result<Scene> {
        with_package(entries, load)
    }

    fn split_scene(root: &str, child: &str) -> Result<Scene> {
        package_scene(&[
            ("_rels/.rels", &relationships(&["/3D/main.model"])),
            (
                "3D/_rels/main.model.rels",
                &relationships(&["Objects/part%20one.model"]),
            ),
            ("3D/main.model", root),
            ("3D/Objects/part one.model", child),
        ])
    }

    fn close(actual: Vec3, expected: Vec3) {
        assert!(
            (actual - expected).length() < 1e-9,
            "actual={actual:?}, expected={expected:?}"
        );
    }

    #[test]
    fn multipart_ids_colors_units_and_nested_transforms_are_scoped() {
        let root_mesh = MESH.replace("id=\"1\"", "id=\"1\" pid=\"3\"");
        let child_mesh = root_mesh.clone();
        let root = production_model(
            "centimeter",
            &format!(
                "<basematerials id=\"3\"><base displaycolor=\"#FF0000\"/></basematerials>{root_mesh}<object id=\"2\"><components><component p:path=\"/3D/Objects/part%20one.model\" objectid=\"2\" transform=\"2 0 0 0 2 0 0 0 2 3 0 0\"/></components></object>"
            ),
            Some(
                "<item objectid=\"1\"/><item objectid=\"2\" transform=\"0 1 0 -1 0 0 0 0 1 0 4 0\"/>",
            ),
        );
        let child = production_model(
            "inch",
            &format!(
                "<basematerials id=\"3\"><base displaycolor=\"#0000FF\"/></basematerials>{child_mesh}<object id=\"2\"><components><component objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 1 0 0\"/></components></object>"
            ),
            None,
        );
        let scene = split_scene(&root, &child).unwrap();
        assert_eq!(scene.triangles.len(), 2);
        assert_eq!(scene.triangles[0].color, Some([255, 0, 0]));
        assert_eq!(scene.triangles[1].color, Some([0, 0, 255]));
        close(scene.triangles[0].vertices[1], Vec3::new(10., 0., 0.));
        close(scene.triangles[1].vertices[0], Vec3::new(0., 120.8, 0.));
        close(scene.triangles[1].vertices[1], Vec3::new(0., 171.6, 0.));
        close(scene.triangles[1].vertices[2], Vec3::new(-50.8, 120.8, 0.));
        let v = scene.triangles[1].vertices;
        assert!(((v[1] - v[0]).cross(v[2] - v[0]).length() / 2. - 1290.32).abs() < 1e-8);
    }

    #[test]
    fn direct_build_path_accepts_alternate_prefix_and_ignores_child_build() {
        let root = production_model(
            "millimeter",
            "",
            Some("<item p:path=\"/3D/Objects/part%20one.model\" objectid=\"1\"/>"),
        )
        .replace("xmlns:p=", "xmlns:q=")
        .replace("requiredextensions=\"p\"", "requiredextensions=\"q\"")
        .replace("p:path=", "q:path=");
        let child = production_model(
            "meter",
            MESH,
            Some("<item objectid=\"999\" p:path=\"/missing.model\" transform=\"bad\"/>"),
        );
        let scene = split_scene(&root, &child).unwrap();
        close(scene.triangles[0].vertices[1], Vec3::new(1000., 0., 0.));
        assert_eq!(scene.triangles.len(), 1);
    }

    #[test]
    fn production_namespace_and_required_extensions_remain_strict() {
        let root = production_model(
            "millimeter",
            "",
            Some("<item p:path=\"/3D/Objects/part%20one.model\" objectid=\"1\"/>"),
        );
        let child = production_model("millimeter", MESH, None);
        assert!(split_scene(&root.replace(PRODUCTION, "urn:unknown"), &child).is_err());
        assert!(split_scene(&root.replace("p:path=", "path="), &child).is_err());
        assert!(
            split_scene(
                &root.replace(
                    "/3D/Objects/part%20one.model",
                    "3D/Objects/part%20one.model"
                ),
                &child
            )
            .is_err()
        );
        assert!(
            split_scene(
                &root.replace(
                    "requiredextensions=\"p\"",
                    "xmlns:x=\"urn:unknown\" requiredextensions=\"p x\""
                ),
                &child
            )
            .is_err()
        );
        assert!(split_scene(&root, &child.replace(PRODUCTION, "urn:unknown")).is_err());
    }

    #[test]
    fn missing_parts_relationships_and_object_ids_fail() {
        let root = production_model(
            "millimeter",
            MESH,
            Some("<item p:path=\"/3D/Objects/part%20one.model\" objectid=\"1\"/>"),
        );
        let child = production_model("millimeter", &MESH.replace("id=\"1\"", "id=\"2\""), None);
        let error = split_scene(&root, &child).unwrap_err().to_string();
        assert!(
            error.contains("missing object in its model part"),
            "{error}"
        );
        assert!(
            package_scene(&[
                ("_rels/.rels", &relationships(&["/3D/main.model"])),
                ("3D/main.model", &root)
            ])
            .is_err()
        );
        assert!(
            package_scene(&[
                ("_rels/.rels", &relationships(&["/3D/main.model"])),
                ("3D/main.model", &root),
                (
                    "3D/_rels/main.model.rels",
                    &relationships(&["/3D/Objects/part%20one.model"])
                )
            ])
            .is_err()
        );
        assert!(
            package_scene(&[
                ("_rels/.rels", &relationships(&["/3D/main.model"])),
                ("3D/main.model", &root),
                (
                    "3D/_rels/main.model.rels",
                    &relationships(&["/3D/other.model"])
                ),
                ("3D/Objects/part one.model", &child)
            ])
            .is_err()
        );
    }

    #[test]
    fn duplicate_package_parts_resources_and_relationships_fail() {
        let root = model(MESH, "<item objectid=\"1\"/>");
        assert!(
            package_scene(&[("3D/3dmodel.model", &root), ("3d/3DMODEL.model", &root)]).is_err()
        );
        assert!(parse(&model(&format!("{MESH}{MESH}"), "<item objectid=\"1\"/>")).is_err());
        assert!(
            parse(&model(
                &MESH.replace("id=\"1\"", "id=\"0\""),
                "<item objectid=\"0\"/>"
            ))
            .is_err()
        );
        assert!(model_relationships(&relationships(&["/3D/a.model", "/3d/A.model"]), "").is_err());
        assert!(
            model_relationships(
                &relationships(&["/3D/a.model", "/3D/b.model"]).replace("Id=\"r1\"", "Id=\"r0\""),
                ""
            )
            .is_err()
        );
        assert!(
            model_relationships(
                &relationships(&["/3D/a.model"]).replace("Id=", "TargetMode=\"External\" Id="),
                ""
            )
            .is_err()
        );
    }

    #[test]
    fn child_external_components_and_local_cycles_fail() {
        let root = production_model(
            "millimeter",
            "",
            Some("<item p:path=\"/3D/Objects/part%20one.model\" objectid=\"1\"/>"),
        );
        let child = production_model(
            "millimeter",
            "<object id=\"1\"><components><component p:path=\"/3D/main.model\" objectid=\"1\"/></components></object>",
            None,
        );
        assert!(split_scene(&root, &child).is_err());
        let child = production_model(
            "millimeter",
            "<object id=\"1\"><components><component objectid=\"2\"/></components></object><object id=\"2\"><components><component objectid=\"1\"/></components></object>",
            None,
        );
        assert!(split_scene(&root, &child).is_err());
    }

    #[test]
    fn aggregate_source_and_decompressed_budgets_are_enforced() {
        let xml = model(MESH, "<item objectid=\"1\"/>");
        for mut budget in [
            Budget {
                vertices: MAX_TRIANGLES,
                ..Budget::default()
            },
            Budget {
                triangles: MAX_TRIANGLES,
                ..Budget::default()
            },
            Budget {
                resources: MAX_RESOURCES,
                ..Budget::default()
            },
            Budget {
                references: MAX_REFERENCES,
                ..Budget::default()
            },
        ] {
            assert!(parse_part(&xml, true, &mut budget).is_err());
        }
        let error = with_package(&[("3D/3dmodel.model", &xml)], |path| {
            let mut package = Package::open(path)?;
            package.read(
                "3d/3dmodel.model",
                &mut Budget {
                    bytes: MAX_INPUT_BYTES - xml.len() as u64 + 1,
                    ..Budget::default()
                },
            )
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("aggregate 512 MiB"), "{error}");
        assert!(parse(&xml.replace("x=\"1\"", "x=\"inf\"")).is_err());
        assert!(parse(&xml.replace("<model ", "<model unit=\"unsupported\" ")).is_err());
        assert!(
            parse(&model(
                &MESH.replace("</object>", "<components/></object>"),
                "<item objectid=\"1\"/>"
            ))
            .is_err()
        );
    }

    #[test]
    fn production_model_part_count_is_bounded() {
        let build = (0..MAX_PARTS).fold(String::new(), |mut text, n| {
            let _ = write!(text, "<item p:path=\"/3D/{n}.model\" objectid=\"1\"/>");
            text
        });
        let root = production_model("millimeter", "", Some(&build));
        let error = package_scene(&[("3D/3dmodel.model", &root)])
            .unwrap_err()
            .to_string();
        assert!(error.contains("model-part limit"), "{error}");
    }

    #[test]
    fn deep_and_exponentially_instanced_components_are_bounded() {
        let mut resources = MESH.to_owned();
        for id in 2..=MAX_DEPTH + 1 {
            let _ = write!(
                resources,
                "<object id=\"{id}\"><components><component objectid=\"{}\"/></components></object>",
                id - 1
            );
        }
        assert!(
            parse(&model(
                &resources,
                &format!("<item objectid=\"{}\"/>", MAX_DEPTH + 1)
            ))
            .is_err()
        );
        let mut resources = "<object id=\"1\"><components/></object>".to_owned();
        for id in 2..=21 {
            let _ = write!(
                resources,
                "<object id=\"{id}\"><components><component objectid=\"{}\"/><component objectid=\"{}\"/></components></object>",
                id - 1,
                id - 1
            );
        }
        let error = parse(&model(&resources, "<item objectid=\"21\"/>"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("instantiated object"), "{error}");
    }

    fn zip_bytes(zip64_comment: Option<&[u8]>) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer.set_raw_zip64_comment(zip64_comment.map(Into::into));
        writer.set_comment("a package comment containing PK\u{5}\u{6}");
        writer
            .start_file("3D/3dmodel.model", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer
            .write_all(model(MESH, "<item objectid=\"1\"/>").as_bytes())
            .unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn eocd_offset(bytes: &[u8]) -> usize {
        bytes
            .windows(22)
            .enumerate()
            .rev()
            .find(|(i, record)| {
                record.starts_with(b"PK\x05\x06")
                    && i + 22 + usize::from(zip_u16(record, 20).unwrap()) == bytes.len()
            })
            .unwrap()
            .0
    }

    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "Small authored test payload lengths fit their native field widths; preserve the exact binary fixture encoding."
    )]
    fn raw_zip_duplicates_are_rejected_before_library_coalescing() {
        let mut bytes = zip_bytes(None);
        let eocd = eocd_offset(&bytes);
        let directory = zip_u32(&bytes, eocd + 16).unwrap() as usize;
        let record = bytes[directory..eocd].to_vec();
        bytes.splice(eocd..eocd, record.iter().copied());
        let eocd = eocd + record.len();
        bytes[eocd + 8..eocd + 12].copy_from_slice(&[2, 0, 2, 0]);
        bytes[eocd + 12..eocd + 16].copy_from_slice(&(2 * record.len() as u32).to_le_bytes());
        // The library accepts the two records and silently exposes one entry.
        assert_eq!(
            zip::ZipArchive::new(std::io::Cursor::new(&bytes))
                .unwrap()
                .len(),
            1
        );
        let error = validate_zip_directory(&mut std::io::Cursor::new(&bytes), bytes.len() as u64)
            .unwrap_err()
            .to_string();
        assert!(error.contains("duplicate 3MF package part"), "{error}");
    }

    #[test]
    fn zip64_directory_handles_extensions_comments_and_archive_prefixes() {
        for comment in [Vec::new(), b"ZIP64 metadata".to_vec(), vec![0; 70_000]] {
            let bytes = zip_bytes(Some(&comment));
            assert_eq!(
                validate_zip_directory(&mut std::io::Cursor::new(&bytes), bytes.len() as u64)
                    .unwrap(),
                1
            );
            assert_eq!(
                zip::ZipArchive::new(std::io::Cursor::new(&bytes))
                    .unwrap()
                    .len(),
                1
            );
            let mut prefixed = b"an archive prefix with no filesystem effect".to_vec();
            prefixed.extend_from_slice(&bytes);
            assert_eq!(
                validate_zip_directory(&mut std::io::Cursor::new(&prefixed), prefixed.len() as u64)
                    .unwrap(),
                1
            );
        }
    }

    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "Small authored test payload lengths fit their native field widths; preserve the exact binary fixture encoding."
    )]
    fn raw_zip_directory_counts_and_extents_are_bounded() {
        let original = zip_bytes(None);
        for length in 0..original.len() {
            assert!(
                validate_zip_directory(
                    &mut std::io::Cursor::new(&original[..length]),
                    length as u64
                )
                .is_err()
            );
        }
        for (offset, value) in [
            (8, [0, 0, 0, 0]),
            (12, u32::MAX.to_le_bytes()),
            (16, u32::MAX.to_le_bytes()),
        ] {
            let mut bytes = original.clone();
            let eocd = eocd_offset(&bytes);
            bytes[eocd + offset..eocd + offset + 4].copy_from_slice(&value);
            assert!(
                validate_zip_directory(&mut std::io::Cursor::new(&bytes), bytes.len() as u64)
                    .is_err()
            );
        }
        let mut bytes = zip_bytes(Some(&[]));
        let eocd = eocd_offset(&bytes);
        let locator = eocd - 20;
        let zip64 = zip_u64(&bytes, locator + 8).unwrap() as usize;
        bytes[zip64 + 24..zip64 + 40].copy_from_slice(
            &[
                (MAX_RESOURCES as u64 + 1).to_le_bytes(),
                (MAX_RESOURCES as u64 + 1).to_le_bytes(),
            ]
            .concat(),
        );
        bytes[eocd + 8..eocd + 12].fill(255);
        let error = validate_zip_directory(&mut std::io::Cursor::new(&bytes), bytes.len() as u64)
            .unwrap_err()
            .to_string();
        assert!(error.contains("too many archive entries"), "{error}");
    }
}
