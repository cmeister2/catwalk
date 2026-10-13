//! Read addressed model thumbnails from qualified `SketchUp` 2013–2017 and 2026 files.
//!
//! The early legacy `CDib` and the modern metadata model-image record select
//! the document thumbnail. Textures, scene and component images are not candidates.

use crate::{Result, formats};
use image::{ImageFormat, ImageReader, RgbaImage};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
    path::Path,
};

/// Maximum selected encoded PNG size.
const MAX_IMAGE: usize = 8 * 1024 * 1024;
/// Maximum decoded metadata member size.
const MAX_METADATA: usize = 64 * 1024;
/// Maximum entries admitted before ZIP directory allocation.
const MAX_ENTRIES: usize = 4096;
/// Qualified modern document model-thumbnail path.
const MODEL_IMAGE: &str = "meta/model_thumbnail.png";

/// Extract the addressed document model thumbnail at its original dimensions.
///
/// Qualified legacy files declare major versions 13–17, `CVersionMap` schema 0
/// and `CDib` schema 3. Modern files declare major version 26, the qualified
/// VFF prefix and matching metadata version. Both paths support PNG images.
/// An absent model-thumbnail record returns `None` without selecting another image.
/// Geometry is neither decoded nor rendered.
///
/// # Errors
/// Returns an error for unreadable files, unsupported versions or carriers,
/// malformed framing, ambiguous ZIP members, checksum or PNG decoding failures,
/// and exceeded source, metadata, member or image limits.
pub fn extract(path: &Path) -> Result<Option<RgbaImage>> {
    extract_bytes(&formats::read_file(path)?)
}

/// Validate document identity and dispatch by the qualified container version.
fn extract_bytes(data: &[u8]) -> Result<Option<RgbaImage>> {
    let mut source = Reader { remaining: data };
    if source.string()? != "SketchUp Model" {
        return Err("input is not a SketchUp document".into());
    }
    let version = source.string()?;
    let plain = version
        .strip_prefix('{')
        .and_then(|v| v.strip_suffix('}'))
        .ok_or("invalid SketchUp version string")?;
    let parts = plain.split('.').collect::<Vec<_>>();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("invalid SketchUp version string".into());
    }
    match parts[0].parse::<u32>()? {
        major @ 13..=17 => legacy(&mut source, major),
        26 => modern(&mut source, plain),
        _ => Err("unqualified SketchUp version (supported major versions: 13–17 and 26)".into()),
    }
}

/// Follow the version map and fixed document fields to the first model `CDib`.
fn legacy(source: &mut Reader<'_>, major: u32) -> Result<Option<RgbaImage>> {
    source.take(16)?; // Per-model GUID.
    if !source.string()?.is_empty() {
        return Err("unsupported SketchUp legacy header field".into());
    }
    source.u32()?; // Per-save seed.
    source.class("CVersionMap", 0)?;
    let mut schemas = BTreeMap::new();
    loop {
        let name = source.string()?;
        let schema = source.u32()?;
        if name == "End-Of-Version-Map" {
            if schema != 0 {
                return Err("invalid SketchUp version-map terminator".into());
            }
            break;
        }
        if schemas.len() >= 256 {
            return Err("SketchUp version map exceeds the 256-class limit".into());
        }
        if name.is_empty() || schemas.insert(name, schema).is_some() {
            return Err("ambiguous SketchUp version map".into());
        }
    }
    if schemas.get("CDib") != Some(&3) {
        return Err("unqualified SketchUp CDib schema".into());
    }
    for expected in [2, 1200, 6] {
        if source.u32()? != expected {
            return Err("unsupported SketchUp legacy document framing".into());
        }
    }
    if major == 17 {
        source.u32()?; // Next persistent entity identifier.
        if source.u32()? != 0 {
            return Err("unsupported SketchUp legacy reserved field".into());
        }
    }
    if source.remaining.starts_with(&[0, 0]) {
        source.take(2)?;
        return Ok(None);
    }
    source.class("CDib", 3)?;
    if source.u32()? != 4 {
        return Err("unsupported SketchUp model-thumbnail image subtype (expected PNG)".into());
    }
    let length = usize::try_from(source.u32()?)?;
    if length > MAX_IMAGE {
        return Err("SketchUp model thumbnail exceeds the 8 MiB limit".into());
    }
    decode_png(source.take(length)?).map(Some)
}

/// Open the bounded modern archive and follow only the metadata's model image.
fn modern(source: &mut Reader<'_>, version: &str) -> Result<Option<RgbaImage>> {
    if source.take(9)? != b"VFF\x08\x00\x01\x00\x11\x00" {
        return Err("unqualified SketchUp VFF container prefix".into());
    }
    source.take(4)?; // Per-save noise.
    let bytes = source.remaining;
    if !bytes.starts_with(b"PK\x03\x04") {
        return Err("SketchUp VFF container has no addressed ZIP archive".into());
    }
    let physical = formats::zip_directory::validate(
        &mut Cursor::new(bytes),
        u64::try_from(bytes.len())?,
        MAX_ENTRIES,
    )?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    if archive.len() != physical {
        return Err("ambiguous SketchUp ZIP member names".into());
    }
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index)?;
        let name = entry.name()?;
        let canonical = name.strip_suffix('/').unwrap_or(&name);
        if canonical.is_empty()
            || canonical.len() > 4096
            || canonical.contains(['\\', '\0'])
            || canonical
                .split('/')
                .any(|p| p.is_empty() || matches!(p, "." | ".."))
            || entries.insert(canonical.to_owned(), index).is_some()
        {
            return Err("invalid or ambiguous SketchUp ZIP member path".into());
        }
    }
    let model_index = *entries
        .get("model.dat")
        .ok_or("SketchUp archive has no model.dat member")?;
    if archive.by_index_raw(model_index)?.is_dir() {
        return Err("SketchUp model.dat member is a directory".into());
    }
    let meta_index = *entries
        .get("meta/meta.dat")
        .ok_or("SketchUp archive has no metadata member")?;
    let metadata = member(&mut archive, meta_index, MAX_METADATA)?;
    if !model_image_present(&metadata, version)? {
        return Ok(None);
    }
    let index = *entries
        .get(MODEL_IMAGE)
        .ok_or("addressed SketchUp model thumbnail is missing")?;
    decode_png(&member(&mut archive, index, MAX_IMAGE)?).map(Some)
}

/// Validate modern document identity and the exact model-image metadata descriptor.
fn model_image_present(metadata: &[u8], version: &str) -> Result<bool> {
    let root = records(metadata)?;
    if root.len() != 1 {
        return Err("SketchUp metadata must contain one root record".into());
    }
    let fields = records(root.get(&0x64).ok_or("invalid SketchUp metadata root")?)?;
    if fields.get(&0x75).copied() != Some(version.as_bytes())
        || fields.get(&0x76).copied() != Some(&[26, 0])
        || fields.get(&0x77).copied() != Some(&[1, 0])
        || fields.get(&0x73).copied() != Some(&[1, 0])
        || fields.get(&0x74).copied() != Some(&[17, 0])
    {
        return Err("unqualified or inconsistent SketchUp metadata version".into());
    }
    let Some(carrier) = fields.get(&0x67).filter(|b| !b.is_empty()) else {
        return Ok(false);
    };
    let image = records(carrier)?;
    if image.len() != 1 {
        return Err("invalid SketchUp model-thumbnail record".into());
    }
    let image = records(
        image
            .get(&0x2328)
            .ok_or("invalid SketchUp model-image container")?,
    )?;
    if image.len() != 2
        || image.get(&0x2329).copied() != Some(&[4, 0, 0, 0])
        || image.get(&0x232a).copied() != Some(MODEL_IMAGE.as_bytes())
    {
        return Err("unsupported SketchUp model-thumbnail descriptor".into());
    }
    Ok(true)
}

/// Decode a checked archive member fully, including its CRC and actual byte size.
fn member(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    index: usize,
    limit: usize,
) -> Result<Vec<u8>> {
    let mut entry = archive.by_index(index)?;
    if entry.is_dir()
        || entry.encrypted()
        || !matches!(
            entry.compression(),
            zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
        )
        || entry.size() > u64::try_from(limit)?
        || entry.compressed_size() > u64::try_from(limit)?
    {
        return Err("unsupported or oversized SketchUp selected ZIP member".into());
    }
    let declared = usize::try_from(entry.size())?;
    let mut data = Vec::with_capacity(declared);
    entry
        .by_ref()
        .take(u64::try_from(limit)? + 1)
        .read_to_end(&mut data)?;
    if data.len() != declared || data.len() > limit {
        return Err("SketchUp selected ZIP member has an invalid decoded size".into());
    }
    Ok(data)
}

/// Read one flat record level with bounded count and unambiguous tags.
fn records(mut bytes: &[u8]) -> Result<BTreeMap<u16, &[u8]>> {
    let mut result = BTreeMap::new();
    while !bytes.is_empty() {
        let mut source = Reader { remaining: bytes };
        let tag = source.u16()?;
        let length = usize::try_from(source.u32()?)?;
        let payload = source.take(length)?;
        if result.len() >= 128 || result.insert(tag, payload).is_some() {
            return Err("SketchUp metadata has too many or duplicate records".into());
        }
        bytes = source.remaining;
    }
    Ok(result)
}

/// Check the PNG header and budget before invoking the shared borrowed image reader.
fn decode_png(bytes: &[u8]) -> Result<RgbaImage> {
    if bytes.len() < 33
        || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes[8..16] != [0, 0, 0, 13, b'I', b'H', b'D', b'R']
    {
        return Err("SketchUp model thumbnail is not a PNG with a valid IHDR".into());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into()?);
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || u64::from(width) * u64::from(height) > 16_000_000
    {
        return Err("SketchUp model thumbnail exceeds the dimension or pixel limit".into());
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode()?.to_rgba8())
}

/// Borrowed forward-only reader for bounded strings, integers and class declarations.
struct Reader<'a> {
    /// Bytes not yet consumed from the addressed stream.
    remaining: &'a [u8],
}

impl<'a> Reader<'a> {
    /// Consume a source-bounded byte range without unchecked offset addition.
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let (head, tail) = self
            .remaining
            .split_at_checked(length)
            .ok_or("truncated SketchUp carrier")?;
        self.remaining = tail;
        Ok(head)
    }

    /// Consume a little-endian two-byte integer.
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }

    /// Consume a little-endian four-byte integer.
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }

    /// Decode an MFC Unicode string while bounding its code-unit allocation.
    fn string(&mut self) -> Result<String> {
        if self.take(3)? != b"\xff\xfe\xff" {
            return Err("unsupported SketchUp string encoding".into());
        }
        let short = self.take(1)?[0];
        let length = if short == 255 {
            let word = self.u16()?;
            if word == u16::MAX {
                usize::try_from(self.u32()?)?
            } else {
                usize::from(word)
            }
        } else {
            usize::from(short)
        };
        if length > 128 {
            return Err("SketchUp identity string exceeds the 128-code-unit limit".into());
        }
        let units = self
            .take(length * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .copied()
            .map(u16::from_le_bytes)
            .collect::<Vec<_>>();
        Ok(String::from_utf16(&units)?)
    }

    /// Verify an inline MFC class declaration at the addressed object position.
    fn class(&mut self, expected: &str, schema: u16) -> Result<()> {
        if self.u16()? != u16::MAX || self.u16()? != schema {
            return Err("unsupported SketchUp class declaration or schema".into());
        }
        let length = usize::from(self.u16()?);
        if length != expected.len() || self.take(length)? != expected.as_bytes() {
            return Err("unexpected SketchUp addressed object class".into());
        }
        Ok(())
    }
}
