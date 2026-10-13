//! Root-document Inventor property thumbnails; geometry is never interpreted.

use crate::Result;
use image::RgbaImage;
use std::{
    collections::HashSet,
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::Path,
    time::{Duration, Instant},
};

#[path = "inventor_bitmap.rs"]
/// Normalize supported Inventor BMP and DIB clipboard images.
mod bitmap;
#[path = "inventor_container.rs"]
/// Preflight compound-file metadata and enforce cooperative I/O limits.
pub(super) mod container;

/// Maximum size of an Inventor compound file admitted for preview extraction.
const MAX_SOURCE: u64 = 512 * 1024 * 1024;
/// Maximum selected property stream, clipboard image and normalized PNG size.
const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Maximum uncompressed image buffer size during normalization.
const MAX_DECODED: usize = 64 * 1024 * 1024;
/// PNG signature used to recognize and reconstruct normalized image streams.
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// Inventor native summary property-set identifier, preferred over the legacy summary.
const NATIVE_SUMMARY: [u8; 16] = [
    0x39, 0xde, 0x38, 0x3d, 0x88, 0x05, 0x14, 0x4c, 0xbb, 0x37, 0x18, 0xf4, 0xd5, 0xdd, 0x31, 0xc7,
];
/// Standard legacy summary property-set identifier accepted as a fallback.
const LEGACY_SUMMARY: [u8; 16] = [
    0xe0, 0x85, 0x9f, 0xf2, 0xf9, 0x4f, 0x68, 0x10, 0xab, 0x91, 0x08, 0x00, 0x2b, 0x27, 0xb3, 0xd9,
];

/// Produce the common RGBA8 thumbnail representation from a qualified carrier.
/// Use [`extract_png`] to preserve a native PNG's original bit depth.
///
/// # Errors
///
/// Returns an error if carrier extraction fails or the extracted PNG cannot be decoded within
/// the image allocation and dimension limits.
pub fn extract(path: &Path) -> Result<Option<RgbaImage>> {
    let Some(png) = extract_png(path)? else {
        return Ok(None);
    };
    let mut reader =
        image::ImageReader::with_format(Cursor::new(png.as_slice()), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(MAX_DECODED as u64);
    reader.limits(limits);
    Ok(Some(reader.decode()?.to_rgba8()))
}

/// Extract a normalized PNG, preserving native PNG precision and supported alpha.
/// Missing or unsupported carriers return `None`; malformed data returns an
/// error. A usable legacy summary can rescue an unusable native summary.
///
/// # Errors
///
/// Returns an error for I/O failures, malformed compound-file metadata or qualified image data,
/// or exceeded extraction budgets when no candidate summary supplies a usable thumbnail.
pub fn extract_png(path: &Path) -> Result<Option<Vec<u8>>> {
    let started = Instant::now();
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    if size > MAX_SOURCE {
        return Err("Inventor source exceeds the 512 MiB extraction limit".into());
    }
    let mut source = container::BudgetFile::new(file, size, started);
    let Some(metadata) = container::preflight(&mut source, size)? else {
        return Ok(None);
    };
    source.seek(SeekFrom::Start(0))?;
    let mut compound = cfb::CompoundFile::open(source)?;
    let mut candidates = Vec::new();
    let root_properties = compound
        .read_root_storage()
        .filter(|entry| entry.is_stream() && entry.name().starts_with('\u{5}'))
        .map(|entry| (entry.path().to_path_buf(), entry.len()))
        .collect::<Vec<_>>();
    for (path, size) in root_properties {
        check_time(started)?;
        // Validate the complete mini-stream chain even when only its prefix
        // is read to identify an unrelated property set.
        if size < 4096 {
            metadata.validate_stream(
                path.file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("invalid Inventor property path")?,
                size,
            )?;
        }
        let mut prefix = Vec::new();
        compound
            .open_stream(&path)?
            .take(68)
            .read_to_end(&mut prefix)?;
        if let Some(priority) = summary_priority(&prefix)? {
            candidates.push((priority, path, size));
        }
    }
    candidates.sort_by_key(|candidate| candidate.0);
    let mut failure = None;
    for (_, path, size) in candidates {
        check_time(started)?;
        let result = (|| -> Result<Option<Vec<u8>>> {
            if size > MAX_BYTES as u64 {
                return Err("Inventor summary stream exceeds the 8 MiB extraction limit".into());
            }
            metadata.validate_stream(
                path.file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("invalid Inventor summary path")?,
                size,
            )?;
            let mut data = Vec::new();
            compound
                .open_stream(&path)?
                .take(MAX_BYTES as u64 + 1)
                .read_to_end(&mut data)?;
            if data.len() as u64 != size {
                return Err("Inventor summary stream has an invalid length".into());
            }
            let Some(clipboard) = property_thumbnail(&data, started)? else {
                return Ok(None);
            };
            normalize(clipboard, started)
        })();
        match result {
            Ok(Some(image)) => return Ok(Some(image)),
            Ok(None) => {}
            Err(error) => failure = Some(error),
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

/// Enforce the cooperative five-second budget between container and image parsing steps.
pub(crate) fn check_time(started: Instant) -> Result<()> {
    if started.elapsed() > Duration::from_secs(5) {
        return Err(
            "Compound document processing exceeded the cooperative five-second budget".into(),
        );
    }
    Ok(())
}

/// Return a byte range with checked offset arithmetic and a truncation error.
pub(crate) fn bytes(data: &[u8], offset: usize, count: usize) -> Result<&[u8]> {
    data.get(
        offset
            ..offset
                .checked_add(count)
                .ok_or("compound document range overflow")?,
    )
    .ok_or_else(|| "truncated compound document metadata or image".into())
}

/// Read a checked little-endian 16-bit property or bitmap integer.
pub(crate) fn u16_le(data: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(bytes(data, offset, 2)?.try_into()?))
}

/// Read a checked little-endian 32-bit property or bitmap integer.
pub(crate) fn u32_le(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes(data, offset, 4)?.try_into()?))
}

/// Identify a qualified root property set and rank native summaries before legacy summaries.
fn summary_priority(data: &[u8]) -> Result<Option<usize>> {
    if data.len() < 48 || u16_le(data, 0)? != 0xfffe {
        return Ok(None);
    }
    let count = u32_le(data, 24)? as usize;
    if !(1..=2).contains(&count) {
        return Ok(None);
    }
    let mut priority = None;
    for index in 0..count {
        let fmtid = bytes(data, 28 + index * 20, 16)?;
        if let Some(value) = [NATIVE_SUMMARY, LEGACY_SUMMARY]
            .iter()
            .position(|known| fmtid == known)
        {
            priority = Some(priority.map_or(value, |existing: usize| existing.min(value)));
        }
    }
    Ok(priority)
}

/// Validate property tables and return the optional thumbnail clipboard value.
fn property_thumbnail(data: &[u8], started: Instant) -> Result<Option<&[u8]>> {
    if ![0, 1].contains(&u16_le(data, 2)?) {
        return Err("invalid Inventor property-set version".into());
    }
    let count = u32_le(data, 24)? as usize;
    for index in 0..count {
        check_time(started)?;
        let fmtid = bytes(data, 28 + index * 20, 16)?;
        if ![NATIVE_SUMMARY, LEGACY_SUMMARY]
            .iter()
            .any(|known| fmtid == known)
        {
            continue;
        }
        let offset = u32_le(data, 44 + index * 20)? as usize;
        if offset < 28 + count * 20 || !offset.is_multiple_of(4) {
            return Err("invalid Inventor property section offset".into());
        }
        let section = bytes(data, offset, u32_le(data, offset)? as usize)?;
        let properties = u32_le(section, 4)? as usize;
        if properties > section.len().saturating_sub(8) / 8 {
            return Err("invalid Inventor property count".into());
        }
        let mut identifiers = HashSet::new();
        let mut offsets = Vec::with_capacity(properties);
        let mut thumbnail = None;
        for index in 0..properties {
            check_time(started)?;
            let identifier = u32_le(section, 8 + index * 8)?;
            let offset = u32_le(section, 12 + index * 8)? as usize;
            if offset < 8 + properties * 8 || offset >= section.len() || !offset.is_multiple_of(4) {
                return Err("invalid Inventor property value offset".into());
            }
            if !identifiers.insert(identifier) {
                return Err("duplicate Inventor property identifier".into());
            }
            offsets.push(offset);
            if identifier == 17 {
                thumbnail = Some(offset);
            }
        }
        let Some(start) = thumbnail else { continue };
        let end = offsets
            .into_iter()
            .filter(|&offset| offset > start)
            .min()
            .unwrap_or(section.len());
        let value = bytes(section, start, end - start)?;
        if u32_le(value, 0)? != 71 {
            return Ok(None);
        }
        let size = u32_le(value, 4)? as usize;
        if size < 4 {
            return Err("invalid Inventor clipboard data size".into());
        }
        return Ok(Some(bytes(value, 8, size)?));
    }
    Ok(None)
}

/// Dispatch qualified clipboard PNG, BMP or DIB data and enforce normalized-output limits.
fn normalize(clipboard: &[u8], started: Instant) -> Result<Option<Vec<u8>>> {
    if clipboard.len() > MAX_BYTES {
        return Err("Inventor clipboard image exceeds the 8 MiB limit".into());
    }
    let format = u32_le(clipboard, 0)?;
    let data = if format == u32::MAX {
        match u32_le(clipboard, 4)? {
            8 | 17 => bytes(clipboard, 8, clipboard.len().saturating_sub(8))?,
            3 => {
                if u16_le(clipboard, 8)? != 8 || u16_le(clipboard, 14)? != 0 {
                    return Ok(None);
                }
                bytes(clipboard, 16, clipboard.len().saturating_sub(16))?
            }
            _ => return Ok(None),
        }
    } else if format == 4 && bytes(clipboard, 4, 4)? == b"PNG\0" {
        bytes(clipboard, 8, clipboard.len().saturating_sub(8))?
    } else {
        return Ok(None);
    };
    let output = if data.starts_with(PNG_SIGNATURE) {
        normalize_png(data, started)?
    } else if data.starts_with(b"BM") {
        bitmap::normalize(data, true, started)?
    } else if [40, 108, 124].contains(&u32_le(data, 0)?) {
        bitmap::normalize(data, false, started)?
    } else {
        return Ok(None);
    };
    if output.as_ref().is_some_and(|bytes| bytes.len() > MAX_BYTES) {
        return Err("Inventor normalized PNG exceeds the 8 MiB output limit".into());
    }
    Ok(output)
}

/// Require nonempty dimensions within the Inventor extraction axis and pixel limits.
fn dimensions(width: usize, height: usize) -> Result<()> {
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || width * height > 16 * 1024 * 1024
    {
        return Err("Inventor image dimensions exceed the extraction limits".into());
    }
    Ok(())
}

/// Append one bounded PNG chunk with its length and freshly computed CRC.
#[allow(
    clippy::trivially_copy_pass_by_ref,
    clippy::cast_possible_truncation,
    reason = "Chunk callers provide borrowed byte-array tags and payloads bounded to eight MiB"
)]
fn append_chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    output.extend_from_slice(&(data.len() as u32).to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(data);
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    output.extend_from_slice(&crc.sum().to_be_bytes());
}

// Preserve validated critical pixel chunks and tRNS, stripping other metadata.
// The scanline validation is independent of decoder palette error recovery.
/// Validate PNG structure and scanlines while preserving critical pixel chunks and transparency.
#[allow(
    clippy::too_many_lines,
    reason = "PNG chunk order and validation share state across the complete parsing loop"
)]
fn normalize_png(data: &[u8], started: Instant) -> Result<Option<Vec<u8>>> {
    let mut offset = 8;
    let mut seen = HashSet::new();
    let mut output = PNG_SIGNATURE.to_vec();
    let mut compressed = Vec::new();
    let mut header = None;
    let mut palette_size = None;
    let mut idat_closed = false;
    while offset < data.len() {
        check_time(started)?;
        let size = u32::from_be_bytes(bytes(data, offset, 4)?.try_into()?) as usize;
        let kind: [u8; 4] = bytes(data, offset + 4, 4)?.try_into()?;
        if !kind.iter().all(u8::is_ascii_alphabetic) || kind[2] & 32 != 0 {
            return Err("invalid Inventor PNG chunk name".into());
        }
        let value = bytes(data, offset + 8, size)?;
        let expected_crc = u32::from_be_bytes(bytes(data, offset + 8 + size, 4)?.try_into()?);
        let mut crc = flate2::Crc::new();
        crc.update(&kind);
        crc.update(value);
        if crc.sum() != expected_crc {
            return Err("invalid Inventor PNG checksum".into());
        }
        if seen.is_empty() && kind != *b"IHDR" {
            return Err("Inventor PNG has no initial header".into());
        }
        if seen.contains(b"IDAT") && kind != *b"IDAT" {
            idat_closed = true;
        }
        match &kind {
            b"IHDR" => {
                if seen.contains(&kind) || size != 13 {
                    return Err("invalid Inventor PNG header".into());
                }
                let width = u32::from_be_bytes(value[..4].try_into()?) as usize;
                let height = u32::from_be_bytes(value[4..8].try_into()?) as usize;
                dimensions(width, height)?;
                let depth = value[8];
                let color = value[9];
                let valid_depth = match color {
                    0 => [1, 2, 4, 8, 16].contains(&depth),
                    2 | 4 | 6 => [8, 16].contains(&depth),
                    3 => [1, 2, 4, 8].contains(&depth),
                    _ => false,
                };
                if !valid_depth || value[10..13] != [0, 0, 0] {
                    return Ok(None);
                }
                let channels = match color {
                    0 | 3 => 1,
                    2 => 3,
                    4 => 2,
                    6 => 4,
                    _ => unreachable!(),
                };
                let row_size = (width * channels * usize::from(depth)).div_ceil(8);
                if (row_size + 1) * height > MAX_DECODED {
                    return Err("Inventor decoded PNG exceeds the 64 MiB limit".into());
                }
                header = Some((width, height, depth, color, row_size));
                append_chunk(&mut output, &kind, value);
            }
            b"PLTE" => {
                let (_, _, depth, color, _) =
                    header.ok_or("Inventor PNG palette precedes header")?;
                if seen.contains(&kind)
                    || seen.contains(b"IDAT")
                    || size == 0
                    || !size.is_multiple_of(3)
                    || size > 768
                    || [0, 4].contains(&color)
                {
                    return Err("invalid Inventor PNG palette".into());
                }
                palette_size = Some(size / 3);
                if color == 3 && size / 3 > 1 << depth {
                    return Err("Inventor PNG palette exceeds its bit depth".into());
                }
                append_chunk(&mut output, &kind, value);
            }
            b"tRNS" => {
                let color = header.ok_or("Inventor PNG transparency precedes header")?.3;
                let valid = match color {
                    0 => size == 2,
                    2 => size == 6,
                    3 => palette_size.is_some_and(|count| size > 0 && size <= count),
                    _ => false,
                };
                if seen.contains(&kind) || seen.contains(b"IDAT") || !valid {
                    return Err("invalid Inventor PNG transparency".into());
                }
                append_chunk(&mut output, &kind, value);
            }
            b"IDAT" => {
                if idat_closed
                    || (header.ok_or("Inventor PNG pixels precede header")?.3 == 3
                        && palette_size.is_none())
                {
                    return Err("invalid Inventor PNG image chunk order".into());
                }
                compressed.extend_from_slice(value);
            }
            b"IEND" => {
                if size != 0 || !seen.contains(b"IDAT") || offset + 12 != data.len() {
                    return Err("invalid Inventor PNG end record".into());
                }
                let header = header.ok_or("Inventor PNG has no header")?;
                validate_scanlines(&compressed, header, palette_size, started)?;
                append_chunk(&mut output, b"IDAT", &compressed);
                append_chunk(&mut output, b"IEND", &[]);
                return Ok(Some(output));
            }
            _ if kind[0] & 32 == 0 => return Ok(None),
            _ => {}
        }
        seen.insert(kind);
        offset += size + 12;
    }
    Err("truncated Inventor PNG".into())
}

/// Check exact deflate length, row filters and indexed-color palette references.
#[allow(
    clippy::cast_possible_truncation,
    clippy::manual_midpoint,
    reason = "Inflation is bounded to 64 MiB; PNG filter averages and packed palette masks fit a byte"
)]
fn validate_scanlines(
    compressed: &[u8],
    header: (usize, usize, u8, u8, usize),
    palette_size: Option<usize>,
    started: Instant,
) -> Result<()> {
    let (width, height, depth, color, row_size) = header;
    let expected = (row_size + 1) * height;
    let mut inflater = flate2::Decompress::new(true);
    let mut raw = Vec::with_capacity(expected.min(1024 * 1024));
    let mut buffer = [0; 8192];
    loop {
        check_time(started)?;
        let before_in = inflater.total_in();
        let before_out = inflater.total_out();
        let status = inflater.decompress(
            &compressed[before_in as usize..],
            &mut buffer,
            flate2::FlushDecompress::None,
        )?;
        let received = (inflater.total_out() - before_out) as usize;
        if raw.len() + received > expected {
            return Err("Inventor PNG expands beyond declared dimensions".into());
        }
        raw.extend_from_slice(&buffer[..received]);
        if status == flate2::Status::StreamEnd {
            break;
        }
        if inflater.total_in() == before_in && received == 0 {
            return Err("truncated Inventor PNG compressed stream".into());
        }
    }
    if raw.len() != expected || inflater.total_in() != compressed.len() as u64 {
        return Err("invalid Inventor PNG compressed stream length".into());
    }
    let mut previous = vec![0_u8; row_size];
    for scanline in raw.chunks_exact(row_size + 1) {
        check_time(started)?;
        let filter = scanline[0];
        if filter > 4 {
            return Err("invalid Inventor PNG scanline filter".into());
        }
        if color != 3 {
            continue;
        }
        let mut row = scanline[1..].to_vec();
        for index in 0..row_size {
            let left = if index == 0 { 0 } else { row[index - 1] };
            let up = previous[index];
            let corner = if index == 0 { 0 } else { previous[index - 1] };
            let prediction = match filter {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => {
                    let p = i32::from(left) + i32::from(up) - i32::from(corner);
                    let a = (p - i32::from(left)).abs();
                    let b = (p - i32::from(up)).abs();
                    let c = (p - i32::from(corner)).abs();
                    if a <= b && a <= c {
                        left
                    } else if b <= c {
                        up
                    } else {
                        corner
                    }
                }
                _ => unreachable!(),
            };
            row[index] = row[index].wrapping_add(prediction);
        }
        for x in 0..width {
            let bit = x * usize::from(depth);
            let index =
                (row[bit / 8] >> (8 - usize::from(depth) - bit % 8)) & ((1_u16 << depth) - 1) as u8;
            if usize::from(index) >= palette_size.unwrap_or(0) {
                return Err("invalid Inventor PNG palette index".into());
            }
        }
        previous = row;
    }
    Ok(())
}

#[cfg(test)]
#[path = "inventor_tests.rs"]
#[allow(
    clippy::cast_possible_truncation,
    reason = "Test builders serialize small fixed carrier fields and deliberately quantize their sample colors"
)]
mod tests;
