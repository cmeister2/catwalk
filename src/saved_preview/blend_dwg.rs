//! Qualified saved preview extraction. No model geometry is loaded or regenerated.
//!
//! Carrier layouts are described in Blender's BLO_core_blend_header.hh,
//! BLO_core_bhead.hh and blendthumb_extract.cc, and LibreDWG's header.spec/dwg.c.
//! This independently written implementation serves the saved-preview API only.

use crate::Result;
use image::{DynamicImage, ImageDecoder, RgbaImage};
use std::{
    fs,
    io::{Cursor, Read},
    path::Path,
};

const MAX_INPUT: usize = 256 * 1024 * 1024;
const MAX_DECOMPRESSED: usize = 512 * 1024 * 1024;
const MAX_PREVIEW: usize = 64 * 1024 * 1024;
const MAX_PIXELS: u64 = 16_000_000;
const BEGIN: [u8; 16] = [
    0x1f, 0x25, 0x6d, 0x07, 0xd4, 0x36, 0x28, 0x28, 0x9d, 0x57, 0xca, 0x3f, 0x9d, 0x44, 0x10, 0x2b,
];
const END: [u8; 16] = [
    0xe0, 0xda, 0x92, 0xf8, 0x2b, 0xc9, 0xd7, 0xd7, 0x62, 0xa8, 0x35, 0xc0, 0x62, 0xbb, 0xef, 0xd4,
];

fn read_input(path: &Path) -> Result<Vec<u8>> {
    if fs::metadata(path)?.len() > MAX_INPUT as u64 {
        return Err("saved preview input exceeds 256 MiB".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_INPUT as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_INPUT {
        return Err("saved preview input grew beyond the size limit".into());
    }
    Ok(bytes)
}

fn bounded_decode(reader: impl Read, remaining: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(remaining as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > remaining {
        return Err("decompressed Blender file exceeds 512 MiB".into());
    }
    Ok(bytes)
}

fn blend_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.starts_with(b"BLENDER") {
        return Ok(bytes.to_vec());
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return bounded_decode(flate2::read::MultiGzDecoder::new(bytes), MAX_DECOMPRESSED);
    }
    if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        // Blender writes independent concatenated Zstandard frames. Decode each frame,
        // including its checksum, instead of mistaking the first frame for a whole file.
        let mut cursor = Cursor::new(bytes);
        let mut output = Vec::new();
        let mut frames = 0;
        while cursor.position() < bytes.len() as u64 {
            frames += 1;
            if frames > 4096 {
                return Err("too many Blender Zstandard frames".into());
            }
            let before = cursor.position();
            let position = usize::try_from(before)?;
            let magic = u32_at(bytes, position)?;
            if magic & 0xffff_fff0 == 0x184d_2a50 {
                // Blender appends a standard skippable Zstd seek table. It carries
                // frame indexing, not image data. Bound and skip its declared size.
                let skip = u32_at(bytes, position + 4)? as usize;
                let end = position
                    .checked_add(8)
                    .and_then(|start| start.checked_add(skip))
                    .ok_or("Blender Zstd skippable-frame length overflow")?;
                if end > bytes.len() {
                    return Err("truncated Blender Zstd skippable frame".into());
                }
                cursor.set_position(end as u64);
                continue;
            }
            let mut decoder = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(
                cursor,
                64 * 1024 * 1024,
            )?;
            output.extend(bounded_decode(
                &mut decoder,
                MAX_DECOMPRESSED - output.len(),
            )?);
            cursor = decoder.into_inner();
            if cursor.position() <= before {
                return Err("Blender Zstandard decoder made no progress".into());
            }
        }
        return Ok(output);
    }
    Err("input is not a raw, gzip or Zstandard Blender file".into())
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset.checked_add(4).ok_or("integer offset overflow")?)
            .ok_or("truncated saved preview integer")?
            .try_into()?,
    ))
}

fn dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(
            "saved preview dimensions exceed the 8192-axis / 16-million-pixel limit".into(),
        );
    }
    Ok(())
}

fn blend_thumbnail(bytes: &[u8]) -> Result<Option<RgbaImage>> {
    if !bytes.starts_with(b"BLENDER") {
        return Err("decompressed input is not a Blender file".into());
    }
    let (mut offset, header_size, length_offset, large, version): (usize, usize, usize, bool, u32) =
        if bytes.get(7..9) == Some(b"17") {
            if bytes.get(9..13) != Some(b"-01v") {
                return Err("unsupported Blender low-level file format".into());
            }
            let digits = bytes
                .get(13..17)
                .ok_or("truncated Blender format-1 header")?;
            if !digits.iter().all(u8::is_ascii_digit) {
                return Err("invalid Blender file version".into());
            }
            let version: u32 = std::str::from_utf8(digits)?.parse()?;
            (17, 32, 16, true, version)
        } else {
            let pointer = *bytes.get(7).ok_or("truncated Blender header")?;
            let endian = *bytes.get(8).ok_or("truncated Blender header")?;
            if endian == b'V' {
                return Err("big-endian Blender saved previews are not qualified".into());
            }
            if endian != b'v' {
                return Err("invalid Blender endianness marker".into());
            }
            let block_size = match pointer {
                b'_' => 20,
                b'-' => 24,
                _ => return Err("invalid Blender pointer-size marker".into()),
            };
            let digits = bytes
                .get(9..12)
                .ok_or("truncated Blender format-0 header")?;
            if !digits.iter().all(u8::is_ascii_digit) {
                return Err("invalid Blender file version".into());
            }
            let version: u32 = std::str::from_utf8(digits)?.parse()?;
            (12, block_size, 4, false, version)
        };
    if version < 250 {
        return Err(
            "Blender versions before 2.50 predate the qualified saved-thumbnail carrier".into(),
        );
    }
    // The official writer places TEST after the optional REND records, before GLOB.
    // Never search arbitrary datablocks or packed assets for image-like bytes.
    for _ in 0..4096 {
        let header_end = offset
            .checked_add(header_size)
            .ok_or("Blender block header overflow")?;
        let block = bytes
            .get(offset..header_end)
            .ok_or("truncated Blender block header")?;
        let size = if large {
            usize::try_from(i64::from_le_bytes(
                block[length_offset..length_offset + 8].try_into()?,
            ))
            .map_err(|_| "invalid Blender block size")?
        } else {
            usize::try_from(i32::from_le_bytes(
                block[length_offset..length_offset + 4].try_into()?,
            ))
            .map_err(|_| "invalid Blender block size")?
        };
        let end = header_end
            .checked_add(size)
            .ok_or("Blender block size overflow")?;
        let payload = bytes
            .get(header_end..end)
            .ok_or("truncated Blender block payload")?;
        match &block[..4] {
            b"TEST" => {
                if !(8..=MAX_PREVIEW).contains(&size) {
                    return Err("invalid Blender TEST thumbnail length".into());
                }
                let width = u32_at(payload, 0)?;
                let height = u32_at(payload, 4)?;
                dimensions(width, height)?;
                let expected = 8 + width as usize * height as usize * 4;
                if size != expected {
                    return Err("Blender TEST dimensions do not match RGBA payload length".into());
                }
                let image = RgbaImage::from_raw(width, height, payload[8..].to_vec())
                    .ok_or("invalid Blender RGBA payload")?;
                return Ok(Some(image::imageops::flip_vertical(&image)));
            }
            b"REND" => offset = end,
            b"GLOB" | b"ENDB" => return Ok(None),
            _ => return Err("unexpected Blender block before saved thumbnail".into()),
        }
    }
    Err("too many Blender render-information blocks".into())
}

/// Extract the saved scene thumbnail. A valid GLOB/ENDB without TEST returns None.
pub fn extract_blend(path: &Path) -> Result<Option<RgbaImage>> {
    blend_thumbnail(&blend_bytes(&read_input(path)?)?)
}

fn image_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(MAX_PREVIEW as u64);
    limits
}

fn decode_dwg_image(kind: u8, payload: &[u8]) -> Result<RgbaImage> {
    if payload.is_empty() || payload.len() > MAX_PREVIEW {
        return Err("DWG saved image length exceeds preview limits".into());
    }
    if kind == 2 {
        // DWG stores CF_DIB, not a BMP file header. Let the Rust BMP reader
        // interpret the authored DIB header, palette and row padding directly.
        let mut decoder =
            image::codecs::bmp::BmpDecoder::new_without_file_header(Cursor::new(payload))?;
        let (width, height) = decoder.dimensions();
        dimensions(width, height)?;
        decoder.set_limits(image_limits())?;
        Ok(DynamicImage::from_decoder(decoder)?.into_rgba8())
    } else if kind == 6 {
        let mut reader =
            image::ImageReader::with_format(Cursor::new(payload), image::ImageFormat::Png);
        reader.limits(image_limits());
        let decoder = reader.into_decoder()?;
        let (width, height) = decoder.dimensions();
        dimensions(width, height)?;
        Ok(DynamicImage::from_decoder(decoder)?.into_rgba8())
    } else {
        Err(format!("DWG saved preview type {kind} is not a qualified raster carrier").into())
    }
}

fn dwg_thumbnail(bytes: &[u8]) -> Result<Option<RgbaImage>> {
    let version = bytes.get(..6).ok_or("truncated DWG version header")?;
    if !matches!(
        version,
        b"AC1012"
            | b"AC1014"
            | b"AC1015"
            | b"AC1018"
            | b"AC1021"
            | b"AC1024"
            | b"AC1027"
            | b"AC1032"
    ) {
        return Err(format!(
            "DWG version {} has no qualified preview layout",
            String::from_utf8_lossy(version)
        )
        .into());
    }
    let address = u32_at(bytes, 13)? as usize;
    if matches!(
        version,
        b"AC1018" | b"AC1021" | b"AC1024" | b"AC1027" | b"AC1032"
    ) && u32_at(bytes, 24)? & 2 != 0
    {
        return Err("encrypted DWG preview/properties section is unsupported".into());
    }
    if address == 0 {
        return Ok(None);
    }
    if address < 17 {
        return Err("DWG thumbnail address overlaps the file header".into());
    }
    let table_start = address
        .checked_add(16)
        .ok_or("DWG thumbnail address overflow")?;
    if bytes.get(address..table_start) != Some(BEGIN.as_slice()) {
        return Err("DWG preview begin sentinel does not match its declared address".into());
    }
    let length = u32_at(bytes, table_start)? as usize;
    if length > MAX_PREVIEW {
        return Err("DWG preview section exceeds 64 MiB".into());
    }
    let body_start = table_start
        .checked_add(4)
        .ok_or("DWG preview section overflow")?;
    let body_end = body_start
        .checked_add(length)
        .ok_or("DWG preview section overflow")?;
    let sentinel_end = body_end.checked_add(16).ok_or("DWG preview end overflow")?;
    if bytes.get(body_end..sentinel_end) != Some(END.as_slice()) {
        return Err("DWG preview end sentinel does not match the declared section length".into());
    }
    let count = *bytes.get(body_start).ok_or("truncated DWG preview table")? as usize;
    let entries_start = body_start + 1;
    let data_start = entries_start
        .checked_add(count * 9)
        .ok_or("DWG preview table overflow")?;
    if data_start > body_end {
        return Err("DWG preview entries exceed the declared section length".into());
    }
    let mut raster = None;
    let mut unsupported = None;
    let mut ranges = Vec::new();
    for index in 0..count {
        let entry = entries_start + index * 9;
        let kind = bytes[entry];
        let image_start = u32_at(bytes, entry + 1)? as usize;
        let image_length = u32_at(bytes, entry + 5)? as usize;
        let image_end = image_start
            .checked_add(image_length)
            .ok_or("DWG preview entry overflow")?;
        if image_start < data_start || image_end > body_end {
            return Err("DWG preview entry lies outside its declared data area".into());
        }
        if image_length > 0
            && ranges
                .iter()
                .any(|&(start, end)| image_start < end && image_end > start)
        {
            return Err("DWG preview entries overlap".into());
        }
        ranges.push((image_start, image_end));
        match kind {
            1 => {} // Header information is not itself an image.
            2 | 6 => {
                let candidate = decode_dwg_image(kind, &bytes[image_start..image_end])?;
                let candidate_pixels = u64::from(candidate.width()) * u64::from(candidate.height());
                let replace = raster.as_ref().is_none_or(|previous: &RgbaImage| {
                    candidate_pixels > u64::from(previous.width()) * u64::from(previous.height())
                });
                if replace {
                    raster = Some(candidate);
                }
            }
            _ => unsupported = Some(kind),
        }
    }
    if raster.is_none()
        && let Some(kind) = unsupported
    {
        return Err(if kind == 3 {
            "DWG WMF previews need a separate vector-metafile renderer".into()
        } else {
            format!("DWG preview type {kind} is not qualified").into()
        });
    }
    Ok(raster)
}

/// Extract a typed, explicitly addressed DIB/PNG preview from a qualified DWG header.
pub fn extract_dwg(path: &Path) -> Result<Option<RgbaImage>> {
    dwg_thumbnail(&read_input(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn block(code: &[u8; 4], payload: &[u8], pointer32: bool, large: bool) -> Vec<u8> {
        let mut block = code.to_vec();
        if large {
            block.extend([0; 12]);
            block.extend((payload.len() as i64).to_le_bytes());
            block.extend(1i64.to_le_bytes());
        } else {
            block.extend((payload.len() as i32).to_le_bytes());
            block.extend(vec![0; if pointer32 { 12 } else { 16 }]);
        }
        block.extend(payload);
        block
    }

    fn blend(pointer32: bool, large: bool, preview: bool) -> Vec<u8> {
        let mut bytes = if large {
            b"BLENDER17-01v0501".to_vec()
        } else if pointer32 {
            b"BLENDER_v280".to_vec()
        } else {
            b"BLENDER-v280".to_vec()
        };
        bytes.extend(block(b"REND", &[0; 72], pointer32, large));
        if preview {
            let mut payload = 1u32.to_le_bytes().to_vec();
            payload.extend(2u32.to_le_bytes());
            payload.extend([255, 0, 0, 255, 0, 0, 255, 128]);
            bytes.extend(block(b"TEST", &payload, pointer32, large));
        }
        bytes.extend(block(b"GLOB", &[0; 8], pointer32, large));
        bytes.extend(block(b"ENDB", &[], pointer32, large));
        bytes
    }

    fn dwg(kind: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 128];
        bytes[..6].copy_from_slice(b"AC1032");
        bytes[13..17].copy_from_slice(&128u32.to_le_bytes());
        bytes.extend(BEGIN);
        bytes.extend((10 + payload.len() as u32).to_le_bytes());
        bytes.push(1);
        bytes.push(kind);
        bytes.extend(158u32.to_le_bytes());
        bytes.extend((payload.len() as u32).to_le_bytes());
        bytes.extend(payload);
        bytes.extend(END);
        bytes
    }

    fn png() -> Vec<u8> {
        let image = RgbaImage::from_raw(1, 2, vec![255, 0, 0, 255, 0, 0, 255, 128]).unwrap();
        let mut cursor = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .unwrap();
        cursor.into_inner()
    }

    #[test]
    fn blend_pointer_and_large_headers_preserve_rgba_and_flip_rows() {
        for (pointer32, large) in [(true, false), (false, false), (false, true)] {
            let image = blend_thumbnail(&blend(pointer32, large, true))
                .unwrap()
                .unwrap();
            assert_eq!(image.dimensions(), (1, 2));
            assert_eq!(image.as_raw(), &[0, 0, 255, 128, 255, 0, 0, 255]);
        }
    }

    #[test]
    fn blend_missing_and_malformed_carriers_are_distinct() {
        assert!(
            blend_thumbnail(&blend(false, false, false))
                .unwrap()
                .is_none()
        );
        let mut bytes = blend(false, false, true);
        let test = 12 + 24 + 72;
        bytes[test + 24..test + 28].copy_from_slice(&8193u32.to_le_bytes());
        assert!(blend_thumbnail(&bytes).is_err());
        assert!(blend_thumbnail(&bytes[..test + 24 + 7]).is_err());
        let mut negative = blend(false, false, true);
        negative[test + 4..test + 8].copy_from_slice(&(-1i32).to_le_bytes());
        assert!(blend_thumbnail(&negative).is_err());
        let mut endian = blend(false, false, true);
        endian[8] = b'V';
        assert!(
            blend_thumbnail(&endian)
                .unwrap_err()
                .to_string()
                .contains("big-endian")
        );
    }

    #[test]
    fn gzip_and_concatenated_zstd_preserve_the_same_saved_pixels() {
        let original = blend(false, false, true);
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&original).unwrap();
        assert_eq!(blend_bytes(&gzip.finish().unwrap()).unwrap(), original);
        // A standards-compliant single-segment Zstd frame with an uncompressed block.
        fn raw_frame(bytes: &[u8]) -> Vec<u8> {
            assert!(bytes.len() < 256);
            let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0x20, bytes.len() as u8];
            let block = ((bytes.len() as u32) << 3) | 1;
            frame.extend(&block.to_le_bytes()[..3]);
            frame.extend(bytes);
            frame
        }
        let mut frames = raw_frame(&original[..16]);
        frames.extend(raw_frame(&original[16..]));
        assert_eq!(blend_bytes(&frames).unwrap(), original);
        let mut seekable = frames.clone();
        seekable.extend(0x184d2a5eu32.to_le_bytes());
        seekable.extend(3u32.to_le_bytes());
        seekable.extend([0; 3]);
        assert_eq!(blend_bytes(&seekable).unwrap(), original);
        seekable.pop();
        assert!(blend_bytes(&seekable).is_err());
        frames.pop();
        assert!(blend_bytes(&frames).is_err());
    }

    #[test]
    fn licensed_blender_files_match_saved_pixel_snapshots() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/saved-previews/blend_dwg");
        for name in ["blend-cube", "blend-280", "blend-extras", "blend-gzip-252"] {
            let actual = extract_blend(&directory.join(format!("{name}.blend")))
                .unwrap()
                .unwrap();
            let expected = image::open(directory.join(format!("{name}.png")))
                .unwrap()
                .into_rgba8();
            assert_eq!(actual, expected, "saved-pixel snapshot: {name}");
        }
    }

    #[test]
    fn portable_authored_carriers_match_pixel_snapshots_and_absence() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/saved-previews/blend_dwg");
        for name in [
            "authored-legacy-32",
            "authored-legacy-64",
            "authored-format1",
        ] {
            let actual = extract_blend(&directory.join(format!("{name}.blend")))
                .unwrap()
                .unwrap();
            let expected = image::open(directory.join(format!("{name}.png")))
                .unwrap()
                .into_rgba8();
            assert_eq!(actual, expected, "authored Blend carrier: {name}");
        }
        for name in ["authored-dib", "authored-png"] {
            let actual = extract_dwg(&directory.join(format!("{name}.dwg")))
                .unwrap()
                .unwrap();
            let expected = image::open(directory.join(format!("{name}.png")))
                .unwrap()
                .into_rgba8();
            assert_eq!(actual, expected, "authored DWG carrier: {name}");
        }
        assert!(
            extract_blend(&directory.join("blend-gzip-no-preview.blend"))
                .unwrap()
                .is_none()
        );
        assert!(
            extract_blend(&directory.join("authored-no-preview.blend"))
                .unwrap()
                .is_none()
        );
        assert!(
            extract_dwg(&directory.join("authored-metadata-only.dwg"))
                .unwrap()
                .is_none()
        );
        assert!(
            extract_dwg(&directory.join("authored-no-preview.dwg"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn dwg_png_uses_the_declared_entry_and_preserves_pixels() {
        let expected = image::load_from_memory(&png()).unwrap().into_rgba8();
        assert_eq!(dwg_thumbnail(&dwg(6, &png())).unwrap().unwrap(), expected);
        let mut missing = vec![0; 32];
        missing[..6].copy_from_slice(b"AC1032");
        assert!(dwg_thumbnail(&missing).unwrap().is_none());
    }

    #[test]
    fn dwg_dib_handles_authored_palette_and_bottom_up_padding() {
        let mut dib = vec![0; 40];
        dib[..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1u32.to_le_bytes());
        dib[8..12].copy_from_slice(&2u32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&8u16.to_le_bytes());
        dib[32..36].copy_from_slice(&2u32.to_le_bytes());
        dib.extend([0, 0, 255, 0, 255, 0, 0, 0]); // DIB palette is BGRA; reserved alpha is ignored.
        dib.extend([0, 0, 0, 0, 1, 0, 0, 0]); // one palette index plus three row-padding bytes.
        let image = dwg_thumbnail(&dwg(2, &dib)).unwrap().unwrap();
        assert_eq!(image.as_raw(), &[0, 0, 255, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn dwg_rejects_wrong_offsets_lengths_encryption_and_vector_carriers() {
        let original = dwg(6, &png());
        for (offset, replacement) in [(13, 129u32), (144, u32::MAX), (150, 0u32), (154, u32::MAX)] {
            let mut broken = original.clone();
            broken[offset..offset + 4].copy_from_slice(&replacement.to_le_bytes());
            assert!(dwg_thumbnail(&broken).is_err(), "offset {offset}");
        }
        let mut encrypted = original.clone();
        encrypted[24] = 2;
        assert!(
            dwg_thumbnail(&encrypted)
                .unwrap_err()
                .to_string()
                .contains("encrypted")
        );
        let vector = dwg(3, &[0; 32]);
        assert!(
            dwg_thumbnail(&vector)
                .unwrap_err()
                .to_string()
                .contains("WMF")
        );
        // A PNG elsewhere in a valid header never becomes a saved preview.
        let mut decoy = vec![0; 32];
        decoy[..6].copy_from_slice(b"AC1032");
        decoy.extend(png());
        assert!(dwg_thumbnail(&decoy).unwrap().is_none());
    }
}
