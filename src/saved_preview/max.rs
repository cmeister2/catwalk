//! Saved 3ds Max thumbnails from addressed SummaryInformation metafile records.

use super::inventor::{bytes, check_time, container, u16_le, u32_le};
use crate::Result;
use image::{Rgba, RgbaImage};
use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::Instant,
};

/// Maximum admitted scene-file size, excluding external assets which are never read.
const MAX_SOURCE: u64 = 512 * 1024 * 1024;
/// Maximum selected summary property stream and clipboard carrier size.
const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Maximum summary properties admitted before table-identity allocation.
const MAX_PROPERTIES: usize = 4096;
/// Exact root summary stream used by the qualified Autodesk producer documents.
const SUMMARY_STREAM: &str = "\u{5}SummaryInformation";
/// Standard `SummaryInformation` property-set identifier in its serialized byte order.
const SUMMARY_FMTID: [u8; 16] = [
    0xe0, 0x85, 0x9f, 0xf2, 0xf9, 0x4f, 0x68, 0x10, 0xab, 0x91, 0x08, 0x00, 0x2b, 0x27, 0xb3, 0xd9,
];

/// Extract a qualified saved image at its original dimensions without loading scene geometry.
/// Missing thumbnails and complete unsupported image carriers return `None`.
///
/// # Errors
/// Returns an error for malformed compound-file, property-set, metafile or bitmap data,
/// unreadable files, or exceeded extraction budgets.
pub fn extract(path: &Path) -> Result<Option<RgbaImage>> {
    let started = Instant::now();
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    if size > MAX_SOURCE {
        return Err("3ds Max source exceeds the 512 MiB extraction limit".into());
    }
    if size < 512 {
        return Err("truncated 3ds Max compound document".into());
    }
    let mut source = container::BudgetFile::new(file, size, started);
    let Some(metadata) = container::preflight(&mut source, size)? else {
        return Ok(None);
    };
    source.seek(SeekFrom::Start(0))?;
    let mut compound = cfb::CompoundFile::open(source)?;
    // A renamed generic OLE document is not admitted as a 3ds Max scene.
    if !["/Scene", "/ClassData", "/ClassDirectory3"]
        .iter()
        .all(|name| compound.is_stream(name))
    {
        return Ok(None);
    }
    let stream_path = format!("/{SUMMARY_STREAM}");
    if !compound.is_stream(&stream_path) {
        return Ok(None);
    }
    let entry = compound.entry(&stream_path)?;
    let size = entry.len();
    if size > MAX_BYTES as u64 {
        return Err("3ds Max summary exceeds the 8 MiB extraction limit".into());
    }
    metadata.validate_stream(entry.name(), size)?;
    let mut data = Vec::new();
    compound
        .open_stream(&stream_path)?
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut data)?;
    if data.len() as u64 != size {
        return Err("3ds Max summary has an invalid stream length".into());
    }
    let Some(clipboard) = property_thumbnail(&data, started)? else {
        return Ok(None);
    };
    metafile_thumbnail(clipboard, started)
}

/// Select PID 17 only through the qualified, nonoverlapping property table.
fn property_thumbnail(data: &[u8], started: Instant) -> Result<Option<&[u8]>> {
    if u16_le(data, 0)? != 0xfffe {
        return Err("invalid 3ds Max property-set byte order".into());
    }
    if u16_le(data, 2)? > 1 || u32_le(data, 24)? != 1 {
        return Ok(None);
    }
    if bytes(data, 28, 16)? != SUMMARY_FMTID {
        return Ok(None);
    }
    let offset = usize::try_from(u32_le(data, 44)?)?;
    if offset < 48 || !offset.is_multiple_of(4) {
        return Err("invalid 3ds Max property-section offset".into());
    }
    let size = usize::try_from(u32_le(data, offset)?)?;
    let section = bytes(data, offset, size)?;
    let count = usize::try_from(u32_le(section, 4)?)?;
    if count > MAX_PROPERTIES || count > section.len().saturating_sub(8) / 8 {
        return Err("invalid 3ds Max property count".into());
    }
    let table_end = 8 + count * 8;
    let mut ids = HashSet::new();
    let mut offsets = HashSet::new();
    let mut thumbnail = None;
    for index in 0..count {
        check_time(started)?;
        let id = u32_le(section, 8 + index * 8)?;
        let offset = usize::try_from(u32_le(section, 12 + index * 8)?)?;
        if offset < table_end || offset >= size || !offset.is_multiple_of(4) {
            return Err("invalid 3ds Max property-value offset".into());
        }
        if !ids.insert(id) || !offsets.insert(offset) {
            return Err("duplicate 3ds Max property identifier or value offset".into());
        }
        if id == 17 {
            thumbnail = Some(offset);
        }
    }
    let Some(offset) = thumbnail else {
        return Ok(None);
    };
    let end = offsets
        .into_iter()
        .filter(|&next| next > offset)
        .min()
        .unwrap_or(size);
    let value = bytes(section, offset, end - offset)?;
    if u32_le(value, 0)? != 71 {
        return Ok(None);
    }
    let size = usize::try_from(u32_le(value, 4)?)?;
    if size < 8 {
        return Err("invalid 3ds Max clipboard size".into());
    }
    Ok(Some(bytes(value, 8, size)?))
}

/// Decode the qualified `CF_METAFILEPICT` carrier: one full-size SRCCOPY DIB followed by EOF.
fn metafile_thumbnail(data: &[u8], started: Instant) -> Result<Option<RgbaImage>> {
    if u32_le(data, 0)? != u32::MAX || u32_le(data, 4)? != 3 {
        return Ok(None);
    }
    // The producer serializes the legacy 16-bit METAFILEPICT fields before WMF.
    if u16_le(data, 8)? != 1 || u16_le(data, 14)? != 0 {
        return Ok(None);
    }
    let width = u16_le(data, 10)?;
    let height = u16_le(data, 12)?;
    let wmf = bytes(data, 16, data.len().saturating_sub(16))?;
    if u16_le(wmf, 0)? != 1 || u16_le(wmf, 2)? != 9 || u16_le(wmf, 4)? != 0x300 {
        return Ok(None);
    }
    if usize::try_from(u32_le(wmf, 6)?)?.checked_mul(2) != Some(wmf.len())
        || u16_le(wmf, 10)? != 0
        || u16_le(wmf, 16)? != 0
    {
        return Err("invalid 3ds Max metafile header".into());
    }
    let record_size = usize::try_from(u32_le(wmf, 18)?)?;
    let record = bytes(
        wmf,
        18,
        record_size
            .checked_mul(2)
            .ok_or("3ds Max record overflow")?,
    )?;
    if u16_le(record, 4)? != 0x0f43 {
        return Ok(None);
    }
    if record_size != usize::try_from(u32_le(wmf, 12)?)? {
        return Err("invalid 3ds Max metafile maximum record size".into());
    }
    let end = 18 + record.len();
    if bytes(wmf, end, 6)? != [3, 0, 0, 0, 0, 0] || end + 6 != wmf.len() {
        return Err("unsupported or malformed 3ds Max metafile record sequence".into());
    }
    if u32_le(record, 6)? != 0x00cc_0020 || u16_le(record, 10)? != 0 {
        return Ok(None);
    }
    // Refuse crop, scale, placement and other drawing operations rather than discard them.
    for (offset, expected) in [(12, height), (14, width), (20, height), (22, width)] {
        if u16_le(record, offset)? != expected {
            return Ok(None);
        }
    }
    for offset in [16, 18, 24, 26] {
        if u16_le(record, offset)? != 0 {
            return Ok(None);
        }
    }
    dib_thumbnail(
        bytes(record, 28, record.len().saturating_sub(28))?,
        width,
        height,
        started,
    )
}

/// Normalize the producer's positive-height 24-bit `BI_RGB` DIB, preserving BGR rows exactly.
fn dib_thumbnail(
    data: &[u8],
    width: u16,
    height: u16,
    started: Instant,
) -> Result<Option<RgbaImage>> {
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err("3ds Max thumbnail dimensions exceed the extraction limits".into());
    }
    if u32_le(data, 0)? != 40
        || u16_le(data, 12)? != 1
        || u16_le(data, 14)? != 24
        || u32_le(data, 16)? != 0
        || u32_le(data, 32)? != 0
        || u32_le(data, 36)? != 0
    {
        return Ok(None);
    }
    if u32_le(data, 4)? != u32::from(width) || u32_le(data, 8)? != u32::from(height) {
        return Err("3ds Max DIB and metafile dimensions disagree".into());
    }
    let stride = (usize::from(width) * 3).div_ceil(4) * 4;
    let size = stride * usize::from(height);
    if usize::try_from(u32_le(data, 20)?)? != size || data.len() != 40 + size {
        return Err("invalid 3ds Max DIB pixel extent".into());
    }
    let mut image = RgbaImage::new(u32::from(width), u32::from(height));
    for y in 0..height {
        check_time(started)?;
        let row = bytes(data, 40 + usize::from(height - y - 1) * stride, stride)?;
        for x in 0..width {
            let offset = usize::from(x) * 3;
            image.put_pixel(
                u32::from(x),
                u32::from(y),
                Rgba([row[offset + 2], row[offset + 1], row[offset], 255]),
            );
        }
    }
    Ok(Some(image))
}

#[cfg(test)]
#[path = "max_tests.rs"]
#[allow(clippy::missing_docs_in_private_items)]
mod tests;
