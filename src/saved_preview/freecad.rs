//! Addressed saved PNG extraction from admitted FreeCAD document archives.

use crate::{Result, formats::fcstd_container::DocumentArchive};
use image::{ImageFormat, ImageReader, RgbaImage};
use std::{io::Cursor, path::Path};

/// Only this native `FreeCAD` archive member can supply a saved image.
const THUMBNAIL: &str = "thumbnails/Thumbnail.png";
/// Maximum decoded archive-member bytes before the PNG codec is opened.
const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;
/// Maximum width or height of the addressed saved image.
const MAX_DIMENSION: u32 = 4096;
/// Maximum stored pixel count, independently bounded before PNG decoding.
const MAX_PIXELS: u64 = 16_000_000;

/// Return the addressed saved PNG at its original size, or `None` when absent.
///
/// The archive must contain an admitted schema-4, file-version-1 `Document.xml`.
/// Geometry members are not loaded or tessellated during extraction.
///
/// # Errors
/// Returns an error for unreadable or malformed archives, unsupported document
/// identity, an invalid addressed PNG, CRC failures or exceeded reader limits.
pub fn extract(path: &Path) -> Result<Option<RgbaImage>> {
    let mut archive = DocumentArchive::open(path)?;
    archive
        .read_optional(THUMBNAIL, MAX_PNG_BYTES)?
        .as_deref()
        .map(decode_png)
        .transpose()
}

/// Validate the native PNG carrier and dimensions before bounded codec allocation.
fn decode_png(bytes: &[u8]) -> Result<RgbaImage> {
    if bytes.len() < 33
        || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes[8..16] != [0, 0, 0, 13, b'I', b'H', b'D', b'R']
    {
        return Err("FreeCAD thumbnail is not a PNG with a valid IHDR carrier".into());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into()?);
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err("FreeCAD saved PNG exceeds the dimension or pixel limit".into());
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode()?.to_rgba8())
}
