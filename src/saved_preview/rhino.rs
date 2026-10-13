//! Addressed Rhino 3DM properties-table thumbnails, independent of model geometry.

use crate::Result;
use flate2::Crc;
use image::{DynamicImage, ImageDecoder, RgbaImage};
use std::{
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::Path,
};

/// Maximum admitted native document size before reading its properties.
const MAX_SOURCE: u64 = 512 * 1024 * 1024;
/// Maximum leading comment payload; model object tables are never read.
const MAX_COMMENT: u64 = 256 * 1024;
/// Maximum encoded properties table admitted into memory.
const MAX_PROPERTIES: u64 = 16 * 1024 * 1024;
/// Maximum decoded bitmap payload, independently bounded before inflation.
const MAX_DECODED: usize = 64 * 1024 * 1024;
/// Maximum saved-image dimension in either axis.
const MAX_DIMENSION: u32 = 4096;
/// Native start-section comment chunk.
const COMMENT: u32 = 1;
/// Native document-properties table.
const PROPERTIES: u32 = 0x1000_0014;
/// Native raw DIB property, distinct from arbitrary bitmap tables.
const PREVIEW: u32 = 0x2000_8023;
/// Native compressed DIB property.
const COMPRESSED_PREVIEW: u32 = 0x2000_8025;
/// CRC-bearing anonymous child used by compressed buffers.
const ANONYMOUS: u32 = 0x4000_8000;
/// Inline-value flag, which changes the interpretation of a chunk header.
const SHORT: u32 = 0x8000_0000;
/// Required final properties-table marker.
const END_TABLE: u32 = 0xffff_ffff;

/// Extract the addressed document preview at its original dimensions.
///
/// Archives 2–4 and 50–80 are admitted. Only the leading comment and properties
/// framing is read; model geometry, textures and plugin data cannot supply an image.
///
/// # Errors
/// Returns an error for unreadable or oversized sources, unsupported versions,
/// malformed or ambiguous properties, unsupported DIB layouts, invalid checksums,
/// truncated compression streams or exceeded image limits.
pub fn extract(path: &Path) -> Result<Option<RgbaImage>> {
    let mut file = File::open(path)?;
    let source_size = file.metadata()?.len();
    if source_size > MAX_SOURCE {
        return Err("Rhino source exceeds the 512 MiB limit".into());
    }
    let mut start = [0; 32];
    file.read_exact(&mut start)?;
    let width = archive_width(&start)?;
    let (kind, length) = file_header(&mut file, width)?;
    if kind != COMMENT || length > MAX_COMMENT {
        return Err("Rhino start section has no bounded comment chunk".into());
    }
    let properties_offset = 32 + 4 + width as u64 + length;
    if properties_offset > source_size {
        return Err("Rhino comment chunk exceeds its source".into());
    }
    file.seek(SeekFrom::Start(properties_offset))?;
    let (kind, length) = file_header(&mut file, width)?;
    if kind != PROPERTIES || length > MAX_PROPERTIES {
        return Err("Rhino has no bounded leading document-properties table".into());
    }
    if properties_offset + 4 + width as u64 + length > source_size {
        return Err("Rhino properties table exceeds its source".into());
    }
    let mut properties = vec![0; usize::try_from(length)?];
    file.read_exact(&mut properties)?;
    extract_properties(&properties, width)
}

/// Select the chunk-length width from a strict 32-byte native file header.
fn archive_width(start: &[u8; 32]) -> Result<usize> {
    if &start[..24] != b"3D Geometry File Format " {
        return Err("invalid Rhino 3DM file signature".into());
    }
    let version = std::str::from_utf8(&start[24..])?.trim_start_matches(' ');
    let version = match version {
        "2" | "3" | "4" => 4,
        "50" | "60" | "70" | "80" => 8,
        _ => return Err("unsupported Rhino archive version".into()),
    };
    Ok(version)
}

/// Read one top-level long header without allocating or reading its payload.
fn file_header(file: &mut File, width: usize) -> Result<(u32, u64)> {
    let mut header = [0; 12];
    file.read_exact(&mut header[..4 + width])?;
    let mut reader = Reader::new(&header[..4 + width], width);
    let kind = reader.u32()?;
    let value = reader.length()?;
    if kind & SHORT != 0 {
        return Err("Rhino leading section uses an invalid short chunk".into());
    }
    Ok((kind, value))
}

/// Walk native property boundaries and admit only one addressed preview record.
fn extract_properties(properties: &[u8], width: usize) -> Result<Option<RgbaImage>> {
    let mut reader = Reader::new(properties, width);
    let mut preview = None;
    for _ in 0..4096 {
        let chunk = reader.chunk()?;
        if chunk.kind == END_TABLE {
            if chunk.value != 0 || reader.position != properties.len() {
                return Err("Rhino properties-table end marker is not final".into());
            }
            return preview
                .map(|(payload, compressed)| decode_bitmap(payload, width, compressed))
                .transpose();
        }
        if matches!(chunk.kind, PREVIEW | COMPRESSED_PREVIEW) {
            if preview.is_some() {
                return Err("ambiguous Rhino document-preview properties".into());
            }
            preview = Some((chunk.payload, chunk.kind == COMPRESSED_PREVIEW));
        }
    }
    Err("Rhino properties exceed the 4096-record limit".into())
}

/// Decode a validated RGB DIB, checking native direct and compressed-buffer CRCs.
fn decode_bitmap(record: &[u8], width: usize, compressed: bool) -> Result<RgbaImage> {
    let (payload, expected_crc) = checked_payload(record)?;
    let mut reader = Reader::new(payload, width);
    let header = reader.take(40)?;
    let mut direct_crc = Crc::new();
    direct_crc.update(header);
    let (palette_bytes, image_bytes) = bitmap_layout(header)?;
    let total = palette_bytes + image_bytes;
    let mut bitmap = header.to_vec();
    if compressed {
        let first = read_buffer(&mut reader, &mut direct_crc, &[palette_bytes, total])?;
        let separate = first.len() == palette_bytes;
        bitmap.extend(first);
        if separate {
            bitmap.extend(read_buffer(&mut reader, &mut direct_crc, &[image_bytes])?);
        }
    } else {
        let bytes = reader.take(total)?;
        direct_crc.update(bytes);
        bitmap.extend_from_slice(bytes);
    }
    if reader.position != payload.len() || direct_crc.sum() != expected_crc {
        return Err("Rhino preview record has invalid trailing data or checksum".into());
    }
    let mut decoder =
        image::codecs::bmp::BmpDecoder::new_without_file_header(Cursor::new(bitmap.as_slice()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED as u64);
    decoder.set_limits(limits)?;
    Ok(DynamicImage::from_decoder(decoder)?.into_rgba8())
}

/// Validate dimensions, palette and exact padded row storage before allocating.
fn bitmap_layout(header: &[u8]) -> Result<(usize, usize)> {
    let mut reader = Reader::new(header, 4);
    let size = reader.u32()?;
    let width = i32::from_le_bytes(reader.take(4)?.try_into()?);
    let height = i32::from_le_bytes(reader.take(4)?.try_into()?);
    let planes = u16::from_le_bytes(reader.take(2)?.try_into()?);
    let depth = u16::from_le_bytes(reader.take(2)?.try_into()?);
    let compression = reader.u32()?;
    let stored_size = reader.u32()?;
    reader.take(8)?;
    let colors = reader.u32()?;
    if size != 40 || planes != 1 || compression != 0 || !matches!(depth, 1 | 4 | 8 | 16 | 24 | 32) {
        return Err("unsupported Rhino preview DIB header or encoding".into());
    }
    if width <= 0
        || height == 0
        || width.unsigned_abs() > MAX_DIMENSION
        || height.unsigned_abs() > MAX_DIMENSION
    {
        return Err("Rhino preview exceeds the image dimension limit".into());
    }
    let maximum_colors = if depth <= 8 { 1_u32 << depth } else { 0 };
    if colors > maximum_colors {
        return Err("invalid Rhino preview palette size".into());
    }
    let palette = if colors == 0 { maximum_colors } else { colors };
    let stride = (u64::from(width.unsigned_abs()) * u64::from(depth)).div_ceil(32) * 4;
    let image_bytes = stride * u64::from(height.unsigned_abs());
    let palette_bytes = u64::from(palette) * 4;
    if image_bytes != u64::from(stored_size) || image_bytes + palette_bytes > MAX_DECODED as u64 {
        return Err("Rhino preview has invalid or excessive bitmap storage".into());
    }
    Ok((
        usize::try_from(palette_bytes)?,
        usize::try_from(image_bytes)?,
    ))
}

/// Decode one exactly sized native buffer; nested chunk bytes do not enter its parent's CRC.
fn read_buffer(
    reader: &mut Reader<'_>,
    direct_crc: &mut Crc,
    admitted: &[usize],
) -> Result<Vec<u8>> {
    let size_bytes = reader.take(4)?;
    direct_crc.update(size_bytes);
    let size = usize::try_from(u32::from_le_bytes(size_bytes.try_into()?))?;
    if !admitted.contains(&size) {
        return Err("Rhino compressed preview buffer size differs from its DIB layout".into());
    }
    if size == 0 {
        return Ok(Vec::new());
    }
    let metadata = reader.take(5)?;
    direct_crc.update(metadata);
    let expected_crc = u32::from_le_bytes(metadata[..4].try_into()?);
    let bytes = match metadata[4] {
        0 => {
            let bytes = reader.take(size)?;
            direct_crc.update(bytes);
            bytes.to_vec()
        }
        1 => {
            let chunk = reader.chunk()?;
            if chunk.kind != ANONYMOUS {
                return Err("Rhino compressed preview has no anonymous buffer chunk".into());
            }
            let (encoded, crc) = checked_payload(chunk.payload)?;
            if checksum(encoded) != crc {
                return Err("Rhino compressed preview chunk checksum failed".into());
            }
            let mut decoder = flate2::Decompress::new(true);
            let mut bytes = vec![0; size + 1];
            let status =
                decoder.decompress(encoded, &mut bytes, flate2::FlushDecompress::Finish)?;
            if status != flate2::Status::StreamEnd
                || decoder.total_out() != size as u64
                || decoder.total_in() != encoded.len() as u64
            {
                return Err(
                    "Rhino compressed preview is truncated, excessive or has trailing data".into(),
                );
            }
            bytes.truncate(size);
            bytes
        }
        _ => return Err("unsupported Rhino preview buffer compression".into()),
    };
    if checksum(&bytes) != expected_crc {
        return Err("Rhino decoded preview buffer checksum failed".into());
    }
    Ok(bytes)
}

/// Separate a CRC-bearing chunk's bounded body from its stored checksum.
fn checked_payload(record: &[u8]) -> Result<(&[u8], u32)> {
    let split = record
        .len()
        .checked_sub(4)
        .ok_or("truncated Rhino preview checksum")?;
    Ok((
        &record[..split],
        u32::from_le_bytes(record[split..].try_into()?),
    ))
}

/// Compute the native CRC32 using the existing compression dependency.
fn checksum(bytes: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(bytes);
    crc.sum()
}

/// One bounded native chunk or inline-value record.
struct Chunk<'a> {
    /// Typecode identifying the exact owner and framing flags.
    kind: u32,
    /// Long payload length or inline value for short records.
    value: u64,
    /// Long payload, including its checksum when present.
    payload: &'a [u8],
}

/// Forward-only traversal within one already bounded native section.
struct Reader<'a> {
    /// Complete enclosing section bytes.
    bytes: &'a [u8],
    /// Next unread byte position.
    position: usize,
    /// Archive-dependent chunk length width.
    width: usize,
}

impl<'a> Reader<'a> {
    /// Start a reader at the section's first byte.
    fn new(bytes: &'a [u8], width: usize) -> Self {
        Self {
            bytes,
            position: 0,
            width,
        }
    }

    /// Consume an exact slice without crossing the enclosing section.
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(count)
            .ok_or("Rhino chunk extent overflow")?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or("truncated Rhino properties or preview")?;
        self.position = end;
        Ok(bytes)
    }

    /// Read a little-endian native 32-bit value.
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }

    /// Read a chunk length or inline value using the archive framing width.
    fn length(&mut self) -> Result<u64> {
        if self.width == 4 {
            Ok(u64::from(self.u32()?))
        } else {
            Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
        }
    }

    /// Consume one complete chunk, interpreting short values without allocation.
    fn chunk(&mut self) -> Result<Chunk<'a>> {
        let kind = self.u32()?;
        let value = self.length()?;
        let payload = if kind & SHORT == 0 {
            self.take(usize::try_from(value)?)?
        } else {
            &[]
        };
        Ok(Chunk {
            kind,
            value,
            payload,
        })
    }
}

#[cfg(test)]
#[path = "rhino_tests.rs"]
mod tests;
