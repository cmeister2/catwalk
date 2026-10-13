//! Addressed document thumbnails from qualified `SolidWorks` compound files.
//!
//! The document-level `PreviewPNG` stream takes precedence over `Preview`.
//! Configuration thumbnails and arbitrary embedded images are not selected.
//! This reader extracts stored images; part and assembly geometry is not decoded.

use super::inventor::container::{BudgetFile, Metadata, preflight};
use crate::Result;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbaImage};
use std::{
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::Path,
    time::{Duration, Instant},
};

/// Maximum compound document admitted for thumbnail extraction.
const MAX_SOURCE: u64 = 512 * 1024 * 1024;
/// Maximum selected preview stream size.
const MAX_STREAM: u64 = 8 * 1024 * 1024;
/// Maximum decoded preview allocation.
const MAX_DECODED: u64 = 64 * 1024 * 1024;

/// Extract a document-level PNG or supported DIB thumbnail at its original size.
///
/// Qualified inputs are CFB v3/v4 part and assembly files with a root
/// `ISolidWorksInformation` stream. An absent or empty thumbnail returns `None`.
/// A valid `Preview` DIB can rescue an unreadable `PreviewPNG` image. Modern
/// compressed-section containers and configuration-specific images are unsupported.
///
/// # Errors
/// Returns errors for I/O failures, unsupported containers, malformed selected
/// streams or images, or exceeded compound metadata, stream and raster limits.
pub fn extract(path: &Path) -> Result<Option<RgbaImage>> {
    let started = Instant::now();
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    if size > MAX_SOURCE {
        return Err("SolidWorks source exceeds the 512 MiB extraction limit".into());
    }
    let mut source = BudgetFile::new(file, size, started);
    let metadata = preflight(&mut source, size)?.ok_or(
        "SolidWorks preview extraction requires compound storage; compressed-section containers are unsupported",
    )?;
    source.seek(SeekFrom::Start(0))?;
    let mut compound = cfb::CompoundFile::open(source)?;
    if !compound.is_stream("/ISolidWorksInformation") {
        return Err("compound file has no root SolidWorks document information stream".into());
    }
    let mut failure = None;
    for (name, png) in [("PreviewPNG", true), ("Preview", false)] {
        check_time(started)?;
        if !compound.exists(name) {
            continue;
        }
        let result = (|| -> Result<Option<RgbaImage>> {
            let entry = compound.entry(name)?;
            if !entry.is_stream() {
                return Err("SolidWorks document preview is not a stream".into());
            }
            let size = entry.len();
            if size > MAX_STREAM {
                return Err("SolidWorks preview stream exceeds the 8 MiB limit".into());
            }
            if size == 0 {
                return Ok(None);
            }
            // CFB lookup is case-insensitive; original chain metadata keeps
            // the directory's authored spelling.
            let data = read_stream(&mut compound, &metadata, entry.name(), size)?;
            let image = if png {
                decode_png(&data)?
            } else {
                decode_dib(&data)?
            };
            check_time(started)?;
            Ok(Some(image))
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

/// Read exactly the selected root stream after original sector-chain validation.
fn read_stream(
    compound: &mut cfb::CompoundFile<BudgetFile>,
    metadata: &Metadata,
    name: &str,
    size: u64,
) -> Result<Vec<u8>> {
    metadata.validate_stream(name, size)?;
    let mut data = Vec::new();
    compound
        .open_stream(name)?
        .take(MAX_STREAM + 1)
        .read_to_end(&mut data)?;
    if data.len() as u64 != size {
        return Err("SolidWorks preview stream does not match its declared length".into());
    }
    Ok(data)
}

/// Apply cooperative deadline checks between extraction stages.
fn check_time(started: Instant) -> Result<()> {
    if started.elapsed() > Duration::from_secs(5) {
        return Err("SolidWorks preview processing exceeded the five-second budget".into());
    }
    Ok(())
}

/// Set explicit allocation and axis limits for both qualified raster carriers.
fn limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(MAX_DECODED);
    limits
}

/// Decode the addressed PNG stream without searching for an image signature.
fn decode_png(data: &[u8]) -> Result<RgbaImage> {
    let mut reader = ImageReader::with_format(Cursor::new(data), ImageFormat::Png);
    reader.limits(limits());
    Ok(reader.decode()?.into_rgba8())
}

/// Decode the addressed `Preview` stream's length-prefixed DIB, including RLE palettes.
fn decode_dib(data: &[u8]) -> Result<RgbaImage> {
    let prefix = data
        .get(..4)
        .ok_or("truncated SolidWorks DIB preview length")?;
    let size = usize::try_from(u32::from_le_bytes(prefix.try_into()?))?;
    let dib = data
        .get(4..)
        .filter(|dib| dib.len() == size)
        .ok_or("SolidWorks DIB preview length does not match the stream")?;
    let mut decoder = image::codecs::bmp::BmpDecoder::new_without_file_header(Cursor::new(dib))?;
    decoder.set_limits(limits())?;
    Ok(DynamicImage::from_decoder(decoder)?.into_rgba8())
}

#[cfg(test)]
#[path = "solidworks_tests.rs"]
/// Authored container, selection, bitmap and malformed-input regressions.
mod tests;
