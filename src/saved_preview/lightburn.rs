//! Extract `LightBurn`'s qualified saved XML thumbnail without rendering geometry.

use crate::Result;
use base64::Engine;
use image::RgbaImage;
use std::{
    fs::File,
    io::{Cursor, Read},
    path::Path,
};

/// Maximum XML project size admitted for saved-preview parsing.
const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
/// Maximum base64 thumbnail attribute length before decoding.
const MAX_ENCODED_PREVIEW_BYTES: usize = 12 * 1024 * 1024;
/// Maximum decoded PNG payload size.
const MAX_PREVIEW_BYTES: usize = 8 * 1024 * 1024;
/// Largest accepted PNG width or height.
const MAX_IMAGE_DIMENSION: u32 = 8192;
/// Maximum decoded saved-thumbnail pixel count.
const MAX_IMAGE_PIXELS: u64 = 16_000_000;

/// Extract a qualified `FormatVersion` 0 or 1 saved PNG, preserving its pixels.
/// Missing or explicitly empty previews return `None`; corrupt carriers error.
///
/// # Errors
///
/// Returns an error for I/O failures, malformed XML, unqualified or ambiguous thumbnail carriers,
/// invalid base64 or PNG data, or exceeded input and image limits.
pub fn extract(path: &Path) -> Result<Option<RgbaImage>> {
    let file = File::open(path)?;
    if file.metadata()?.len() > MAX_SOURCE_BYTES as u64 {
        return Err("LightBurn source exceeds the 64 MiB saved-preview limit".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    extract_bytes(&bytes)
}

/// Validate the XML thumbnail carrier, decode bounded base64 PNG data and preserve its pixels.
fn extract_bytes(bytes: &[u8]) -> Result<Option<RgbaImage>> {
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err("LightBurn source exceeds the 64 MiB saved-preview limit".into());
    }
    let text = std::str::from_utf8(bytes)?;
    let document = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 500_000,
            ..Default::default()
        },
    )?;
    let root = document.root_element();
    if root.tag_name().name() != "LightBurnProject" || root.tag_name().namespace().is_some() {
        return Err("expected an unnamespaced LightBurnProject XML root".into());
    }
    // Old projects without this attribute were observed without a thumbnail.
    // Do not treat an unqualified carrier as a supported format version.
    if !matches!(root.attribute("FormatVersion"), Some("0" | "1")) {
        let has_preview = root
            .children()
            .any(|node| node.is_element() && node.tag_name().name() == "Thumbnail");
        if root.attribute("FormatVersion").is_none() && !has_preview {
            return Ok(None);
        }
        return Err(
            "LightBurn saved preview FormatVersion is not qualified (expected 0 or 1)".into(),
        );
    }
    let mut thumbnails = root
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "Thumbnail");
    let Some(thumbnail) = thumbnails.next() else {
        return Ok(None);
    };
    if thumbnails.next().is_some() || thumbnail.tag_name().namespace().is_some() {
        return Err("ambiguous or namespaced LightBurn Thumbnail carrier".into());
    }
    let source = thumbnail
        .attribute("Source")
        .ok_or("LightBurn Thumbnail is missing its Source attribute")?;
    if source.trim().is_empty() {
        return Ok(None);
    }
    if source.len() > MAX_ENCODED_PREVIEW_BYTES {
        return Err("LightBurn encoded preview exceeds the 12 MiB saved-preview limit".into());
    }
    // XML attribute normalization may introduce whitespace. It is not part of
    // the base64 payload; data URLs and other prefixes remain invalid.
    let encoded: Vec<u8> = source
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    let png = base64::engine::general_purpose::STANDARD.decode(&encoded)?;
    if png.len() > MAX_PREVIEW_BYTES {
        return Err("LightBurn decoded preview exceeds the 8 MiB saved-preview limit".into());
    }
    if png.len() < 33
        || !png.starts_with(b"\x89PNG\r\n\x1a\n")
        || png[8..16] != [0, 0, 0, 13, b'I', b'H', b'D', b'R']
    {
        return Err("LightBurn Thumbnail Source is not a PNG with a valid IHDR carrier".into());
    }
    let width = u32::from_be_bytes(png[16..20].try_into()?);
    let height = u32::from_be_bytes(png[20..24].try_into()?);
    if width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err("LightBurn saved preview exceeds the image dimension or pixel limit".into());
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(png), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    Ok(Some(reader.decode()?.to_rgba8()))
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    reason = "Test builders serialize small fixed carrier fields and deliberately quantize their sample colors"
)]
mod tests {
    use super::*;

    fn project(source: &str) -> Vec<u8> {
        format!("<LightBurnProject FormatVersion=\"1\"><Thumbnail Source=\"{source}\"/></LightBurnProject>").into_bytes()
    }

    #[test]
    fn missing_preview_is_distinct_from_corrupt_preview() {
        assert!(
            extract_bytes(b"<LightBurnProject FormatVersion=\"0\"/>")
                .unwrap()
                .is_none()
        );
        assert!(
            extract_bytes(b"<LightBurnProject AppVersion=\"0.9.07\"/>")
                .unwrap()
                .is_none()
        );
        assert!(extract_bytes(&project(" ")).unwrap().is_none());
        assert!(
            extract_bytes(b"<LightBurnProject FormatVersion=\"1\"><Thumbnail/></LightBurnProject>")
                .is_err()
        );
        assert!(extract_bytes(&project("not_base64!")).is_err());
        assert!(
            extract_bytes(&project(
                &base64::engine::general_purpose::STANDARD.encode(b"not a PNG")
            ))
            .is_err()
        );
        assert!(extract_bytes(b"<LightBurnProject").is_err());
    }

    #[test]
    fn ignores_nested_images_and_rejects_unqualified_or_ambiguous_carriers() {
        assert!(extract_bytes(b"<LightBurnProject FormatVersion=\"1\"><Shape><Thumbnail Source=\"bad\"/></Shape></LightBurnProject>").unwrap().is_none());
        for source in [
            b"<Other FormatVersion=\"1\"/>".as_slice(),
            b"<LightBurnProject xmlns=\"urn:unqualified\" FormatVersion=\"1\"/>",
            b"<LightBurnProject FormatVersion=\"2\"><Thumbnail Source=\"bad\"/></LightBurnProject>",
            b"<LightBurnProject><Thumbnail Source=\"bad\"/></LightBurnProject>",
            b"<LightBurnProject FormatVersion=\"1\"><Thumbnail Source=\"\"/><Thumbnail Source=\"\"/></LightBurnProject>",
            b"<LightBurnProject xmlns:x=\"urn:unqualified\" FormatVersion=\"1\"><x:Thumbnail Source=\"\"/></LightBurnProject>",
            b"<!DOCTYPE LightBurnProject [<!ENTITY preview SYSTEM \"file:///etc/passwd\">]><LightBurnProject FormatVersion=\"1\"><Thumbnail Source=\"&preview;\"/></LightBurnProject>",
        ] { assert!(extract_bytes(source).is_err()); }
    }

    #[test]
    fn bounds_saved_png_before_decoding_and_rejects_truncation() {
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            2,
            1,
            image::Rgba([20, 80, 160, 255]),
        ))
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
        let png = encoded.into_inner();
        let mut oversized = png.clone();
        oversized[16..20].copy_from_slice(&9000u32.to_be_bytes());
        assert!(
            extract_bytes(&project(
                &base64::engine::general_purpose::STANDARD.encode(oversized)
            ))
            .unwrap_err()
            .to_string()
            .contains("dimension")
        );
        assert!(
            extract_bytes(&project(
                &base64::engine::general_purpose::STANDARD.encode(&png[..33])
            ))
            .is_err()
        );
        assert!(
            extract_bytes(&project(&"A".repeat(MAX_ENCODED_PREVIEW_BYTES + 1)))
                .unwrap_err()
                .to_string()
                .contains("12 MiB")
        );
    }

    #[test]
    fn preserves_non_square_rgba_pixels_and_accepts_base64_whitespace() {
        let original = RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([x as u8 * 80, y as u8 * 130, 25, (x + y) as u8 * 40])
        });
        let mut encoded_png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(original.clone())
            .write_to(&mut encoded_png, image::ImageFormat::Png)
            .unwrap();
        let base64 = base64::engine::general_purpose::STANDARD.encode(encoded_png.into_inner());
        let with_whitespace = format!(" {} \n {} ", &base64[..12], &base64[12..]);
        assert_eq!(
            extract_bytes(&project(&with_whitespace)).unwrap().unwrap(),
            original
        );
    }
}
