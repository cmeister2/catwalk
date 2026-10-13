//! Extract saved thumbnails from supported Fusion archive carriers.
//!
//! Fusion archive layout is observational rather than a published specification.
//! Only the active design's exact preview carrier is selected; simulation assets,
//! canvases, geometry blobs, and arbitrary image signatures are not candidates.

use crate::Result;
use image::{ImageFormat, ImageReader, RgbaImage};
use std::{
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::Path,
    time::{Duration, Instant},
};

/// Maximum accepted Fusion archive size in bytes.
const MAX_SOURCE: u64 = 512 * 1024 * 1024;
/// Maximum compressed or decompressed size of either selected archive member.
const MAX_MEMBER: u64 = 8 * 1024 * 1024;
/// Maximum central-directory size accepted before ZIP parsing.
const MAX_DIRECTORY: u64 = 4 * 1024 * 1024;
/// Maximum archive entry count admitted by the central-directory preflight.
const MAX_ENTRIES: usize = 4096;
/// Qualified path of the root document saved PNG inside a Fusion archive.
const PREVIEW: &str = "FusionAssetName[Active]/Previews/small.png";
/// Manifest path used to qualify the root document type and version.
const ACTIVE_MANIFEST: &str = "FusionAssetName[Active]/Manifest.dat";

/// Extract a saved Fusion PNG without opening or changing the design geometry.
///
/// Qualified containers are single-disk ZIP32 with manifest tag `3-2-0-0`, an
/// active Fusion asset manifest, and stored, Deflate, or Zstandard members.
/// `None` denotes an absent active-design preview. Unsupported or malformed
/// carriers return an error.
///
/// # Errors
///
/// Returns an error for I/O failures, filename decoding, malformed or unqualified archive metadata or image data,
/// unsupported compression, failed checksums, or exceeded extraction budgets.
pub fn extract_fusion(path: &Path) -> Result<Option<RgbaImage>> {
    let started = Instant::now();
    let mut source = File::open(path)?;
    let source_size = source.metadata()?.len();
    if source_size > MAX_SOURCE {
        return Err("Fusion source exceeds the 512 MiB size limit".into());
    }
    let expected_entries = check_directory(&mut source, source_size)?;
    source.seek(SeekFrom::Start(0))?;
    let mut archive = zip::ZipArchive::new(source)?;
    if archive.len() != expected_entries || archive.len() > MAX_ENTRIES {
        return Err("Fusion archive has inconsistent or duplicate member names".into());
    }
    let mut root_manifest = None;
    let mut active_manifest = None;
    let mut preview = None;
    for index in 0..archive.len() {
        check_time(started)?;
        let entry = archive.by_index_raw(index)?;
        match entry.name()?.as_ref() {
            "Manifest.dat" => root_manifest = Some(index),
            ACTIVE_MANIFEST => active_manifest = Some(index),
            PREVIEW => preview = Some(index),
            _ => {}
        }
    }
    let root_index = root_manifest.ok_or("Fusion archive has no root manifest")?;
    let manifest = read_member(&mut archive, root_index, started)?;
    check_manifest(&manifest)?;
    let active_index = active_manifest.ok_or("Fusion archive has no qualified active asset")?;
    let active = read_member(&mut archive, active_index, started)?;
    if active.len() < 34
        || u32_le(&active, 0)? != 15
        || active.get(4..34)
            != Some(
                "FusionAssetName"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>()
                    .as_slice(),
            )
    {
        return Err(
            "Fusion active asset manifest is not a qualified FusionAssetName carrier".into(),
        );
    }
    let Some(preview_index) = preview else {
        return Ok(None);
    };
    let encoded = read_member(&mut archive, preview_index, started)?;
    let mut reader = ImageReader::with_format(Cursor::new(encoded.as_slice()), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode()?.to_rgba8();
    check_time(started)?;
    Ok(Some(image))
}

/// Enforce the cooperative extraction deadline between reads and parsing stages.
fn check_time(started: Instant) -> Result<()> {
    if started.elapsed() > Duration::from_secs(5) {
        return Err("Fusion preview processing exceeded the cooperative five-second budget".into());
    }
    Ok(())
}

/// Read a checked little-endian 16-bit integer from archive metadata.
fn u16_le(data: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        data.get(offset..offset + 2)
            .ok_or("truncated Fusion ZIP metadata")?
            .try_into()?,
    ))
}

/// Read a checked little-endian 32-bit integer from archive metadata.
fn u32_le(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        data.get(offset..offset + 4)
            .ok_or("truncated Fusion metadata")?
            .try_into()?,
    ))
}

// Inspect only the bounded ZIP end record before the ZIP reader allocates its
// member table. ZIP64 and split archives have not been qualified here.
/// Validate the bounded, single-disk ZIP end record before allocating archive metadata.
fn check_directory(source: &mut File, source_size: u64) -> Result<usize> {
    let tail_size = source_size.min(65_535 + 22);
    source.seek(SeekFrom::Start(source_size - tail_size))?;
    let mut tail = vec![0; tail_size as usize];
    source.read_exact(&mut tail)?;
    let end = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&offset| {
            tail.get(offset..offset + 4) == Some(b"PK\x05\x06")
                && u16_le(&tail, offset + 20)
                    .is_ok_and(|size| offset + 22 + usize::from(size) == tail.len())
        })
        .ok_or("Fusion input has no complete ZIP end record")?;
    let disk = u16_le(&tail, end + 4)?;
    let directory_disk = u16_le(&tail, end + 6)?;
    let disk_count = u16_le(&tail, end + 8)?;
    let total_count = u16_le(&tail, end + 10)?;
    let directory_size = u64::from(u32_le(&tail, end + 12)?);
    let directory_offset = u64::from(u32_le(&tail, end + 16)?);
    if disk != 0 || directory_disk != 0 || disk_count != total_count {
        return Err("split Fusion ZIP archives are unqualified".into());
    }
    if total_count == u16::MAX
        || directory_size == u64::from(u32::MAX)
        || directory_offset == u64::from(u32::MAX)
    {
        return Err("ZIP64 Fusion archives are unqualified".into());
    }
    if usize::from(total_count) > MAX_ENTRIES || directory_size > MAX_DIRECTORY {
        return Err("Fusion ZIP directory exceeds the extraction limits".into());
    }
    if directory_offset + directory_size != source_size - tail_size + end as u64 {
        return Err("Fusion ZIP directory has an invalid range".into());
    }
    Ok(usize::from(total_count))
}

/// Require the observed Fusion document version, type and UTF-16 extension fields.
fn check_manifest(data: &[u8]) -> Result<()> {
    // The observed leading fields are length-prefixed UTF-8 format tag and
    // document type, followed by a length-prefixed UTF-16 filename extension.
    let mut offset = 0;
    for expected in ["3-2-0-0", "FusionDocType"] {
        let length = u32_le(data, offset)? as usize;
        offset += 4;
        if length != expected.len()
            || data.get(offset..offset + length) != Some(expected.as_bytes())
        {
            return Err("unqualified Fusion manifest version or document type".into());
        }
        offset += length;
    }
    if u32_le(data, offset)? != 4
        || data.get(offset + 4..offset + 12) != Some(b".\x00f\x003\x00d\x00")
    {
        return Err("Fusion manifest does not identify an .f3d document".into());
    }
    Ok(())
}

/// Read a selected member while enforcing its byte and elapsed-time budgets.
fn bounded_read(mut reader: impl Read, started: Instant) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        check_time(started)?;
        let received = reader.read(&mut buffer)?;
        if received == 0 {
            return Ok(output);
        }
        if output.len() as u64 + received as u64 > MAX_MEMBER {
            return Err("Fusion selected member exceeds the 8 MiB size limit".into());
        }
        output.extend_from_slice(&buffer[..received]);
    }
}

/// Decode one qualified ZIP compression method and verify its declared length and CRC.
fn read_member(
    archive: &mut zip::ZipArchive<File>,
    index: usize,
    started: Instant,
) -> Result<Vec<u8>> {
    let entry = archive.by_index_raw(index)?;
    if entry.encrypted() {
        return Err("encrypted Fusion preview carriers are unqualified".into());
    }
    if entry.size() > MAX_MEMBER || entry.compressed_size() > MAX_MEMBER {
        return Err("Fusion selected member exceeds the 8 MiB size limit".into());
    }
    let expected_size = entry.size();
    let expected_crc = entry.crc32();
    let method = entry.compression();
    let compressed = bounded_read(entry, started)?;
    let data = match method {
        zip::CompressionMethod::Stored => compressed,
        zip::CompressionMethod::Deflated => {
            let mut decoder = flate2::read::DeflateDecoder::new(compressed.as_slice());
            let data = bounded_read(&mut decoder, started)?;
            if decoder.total_in() != compressed.len() as u64 {
                return Err("Fusion Deflate member has trailing bytes".into());
            }
            data
        }
        zip::CompressionMethod::ZSTD => {
            let mut decoder = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(
                compressed.as_slice(),
                MAX_MEMBER,
            )?;
            let data = bounded_read(&mut decoder, started)?;
            if !decoder.into_inner().is_empty() {
                return Err("Fusion Zstandard member has trailing or concatenated frames".into());
            }
            data
        }
        _ => return Err(format!("unqualified Fusion ZIP compression method {method:?}").into()),
    };
    let mut checksum = flate2::Crc::new();
    checksum.update(&data);
    if data.len() as u64 != expected_size || checksum.sum() != expected_crc {
        return Err("Fusion selected member length or checksum is invalid".into());
    }
    Ok(data)
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    reason = "Test builders serialize small fixed carrier fields and deliberately quantize their sample colors"
)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture(assets: &[(&str, &[u8])], compression: zip::CompressionMethod) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(compression);
        for &(name, bytes) in assets {
            writer.start_file(name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn manifest() -> Vec<u8> {
        let mut bytes = Vec::new();
        for item in ["3-2-0-0", "FusionDocType"] {
            bytes.extend_from_slice(&(item.len() as u32).to_le_bytes());
            bytes.extend_from_slice(item.as_bytes());
        }
        bytes.extend_from_slice(&4_u32.to_le_bytes());
        bytes.extend_from_slice(b".\x00f\x003\x00d\x00");
        bytes
    }

    fn active_manifest() -> Vec<u8> {
        let mut bytes = 15_u32.to_le_bytes().to_vec();
        bytes.extend("FusionAssetName".encode_utf16().flat_map(u16::to_le_bytes));
        bytes
    }

    fn image() -> Vec<u8> {
        let pixels = RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([
                40 * x as u8,
                90 * y as u8,
                220,
                if x == 0 { 0 } else { 200 },
            ])
        });
        let mut output = Cursor::new(Vec::new());
        pixels.write_to(&mut output, ImageFormat::Png).unwrap();
        output.into_inner()
    }

    // A ZIP32 writer with deliberately uncompressed Zstandard blocks. Neither
    // this test fixture nor production decoding depends on native zstd code.
    fn zstd_fixture(assets: &[(&str, &[u8])]) -> Vec<u8> {
        let mut output = Vec::new();
        let mut directory = Vec::new();
        for &(name, data) in assets {
            assert!(data.len() < 256);
            let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0x20, data.len() as u8];
            frame.extend_from_slice(&(((data.len() as u32) << 3) | 1).to_le_bytes()[..3]);
            frame.extend_from_slice(data);
            let mut crc = flate2::Crc::new();
            crc.update(data);
            let offset = output.len() as u32;
            output.extend_from_slice(b"PK\x03\x04");
            for value in [63_u16, 0, 93, 0, 0] {
                output.extend_from_slice(&value.to_le_bytes());
            }
            for value in [crc.sum(), frame.len() as u32, data.len() as u32] {
                output.extend_from_slice(&value.to_le_bytes());
            }
            output.extend_from_slice(&(name.len() as u16).to_le_bytes());
            output.extend_from_slice(&0_u16.to_le_bytes());
            output.extend_from_slice(name.as_bytes());
            output.extend_from_slice(&frame);
            directory.extend_from_slice(b"PK\x01\x02");
            for value in [63_u16, 63, 0, 93, 0, 0] {
                directory.extend_from_slice(&value.to_le_bytes());
            }
            for value in [crc.sum(), frame.len() as u32, data.len() as u32] {
                directory.extend_from_slice(&value.to_le_bytes());
            }
            for value in [name.len() as u16, 0, 0, 0, 0] {
                directory.extend_from_slice(&value.to_le_bytes());
            }
            directory.extend_from_slice(&0_u32.to_le_bytes());
            directory.extend_from_slice(&offset.to_le_bytes());
            directory.extend_from_slice(name.as_bytes());
        }
        let directory_offset = output.len() as u32;
        let directory_size = directory.len() as u32;
        output.extend_from_slice(&directory);
        output.extend_from_slice(b"PK\x05\x06");
        for value in [0_u16, 0, assets.len() as u16, assets.len() as u16] {
            output.extend_from_slice(&value.to_le_bytes());
        }
        output.extend_from_slice(&directory_size.to_le_bytes());
        output.extend_from_slice(&directory_offset.to_le_bytes());
        output.extend_from_slice(&0_u16.to_le_bytes());
        output
    }

    fn run(bytes: &[u8], suffix: &str) -> Result<Option<RgbaImage>> {
        let path = std::env::temp_dir().join(format!(
            "catwalk-fusion-preview-{}-{suffix}.f3d",
            std::process::id()
        ));
        std::fs::write(&path, bytes)?;
        let result = extract_fusion(&path);
        std::fs::remove_file(path)?;
        result
    }

    #[test]
    fn exact_active_carrier_preserves_pixels_in_stored_and_deflate_archives() {
        let manifest = manifest();
        let active = active_manifest();
        let png = image();
        for (method, suffix) in [
            (zip::CompressionMethod::Stored, "stored"),
            (zip::CompressionMethod::Deflated, "deflate"),
        ] {
            let bytes = fixture(
                &[
                    ("Manifest.dat", &manifest),
                    (ACTIVE_MANIFEST, &active),
                    (
                        "SimulationModelAssetSubtype/Previews/small.png",
                        b"distractor",
                    ),
                    (PREVIEW, &png),
                ],
                method,
            );
            let image = run(&bytes, suffix).unwrap().unwrap();
            assert_eq!(image.dimensions(), (3, 2));
            assert_eq!(image.get_pixel(2, 1).0, [80, 90, 220, 200]);
            assert_eq!(image.get_pixel(0, 1).0[3], 0);
        }
    }

    #[test]
    fn absence_and_unsupported_manifest_are_distinct_outcomes() {
        let manifest = manifest();
        let active = active_manifest();
        let bytes = fixture(
            &[
                ("Manifest.dat", &manifest),
                (ACTIVE_MANIFEST, &active),
                ("other/Previews/small.png", &image()),
            ],
            zip::CompressionMethod::Stored,
        );
        assert!(run(&bytes, "absent").unwrap().is_none());
        let bytes = fixture(
            &[("Manifest.dat", b"unknown")],
            zip::CompressionMethod::Stored,
        );
        assert!(run(&bytes, "unknown").is_err());
        let bytes = fixture(
            &[
                ("Manifest.dat", &manifest),
                (ACTIVE_MANIFEST, &active),
                (PREVIEW, b"not a PNG"),
            ],
            zip::CompressionMethod::Stored,
        );
        assert!(run(&bytes, "bad-image").is_err());
    }

    #[test]
    fn zstandard_carrier_matches_deflate_and_rejects_checksums_and_duplicates() {
        let manifest = manifest();
        let active = active_manifest();
        let png = image();
        let assets = [
            ("Manifest.dat", manifest.as_slice()),
            (ACTIVE_MANIFEST, active.as_slice()),
            (PREVIEW, png.as_slice()),
        ];
        let bytes = zstd_fixture(&assets);
        let zstd = run(&bytes, "zstd").unwrap().unwrap();
        let deflate = run(
            &fixture(&assets, zip::CompressionMethod::Deflated),
            "comparison",
        )
        .unwrap()
        .unwrap();
        assert_eq!(zstd, deflate);
        let mut damaged = bytes;
        let first_data = 30 + "Manifest.dat".len() + 9;
        damaged[first_data] ^= 1;
        assert!(run(&damaged, "bad-checksum").is_err());
        let duplicate = zstd_fixture(&[
            ("Manifest.dat", &manifest),
            ("Manifest.dat", &manifest),
            (ACTIVE_MANIFEST, &active),
            (PREVIEW, &png),
        ]);
        assert!(run(&duplicate, "duplicate").is_err());
    }

    #[test]
    fn source_directory_and_image_limits_are_checked() {
        let path = std::env::temp_dir().join(format!(
            "catwalk-fusion-preview-{}-oversized.f3d",
            std::process::id()
        ));
        File::create(&path)
            .unwrap()
            .set_len(MAX_SOURCE + 1)
            .unwrap();
        assert!(
            extract_fusion(&path)
                .unwrap_err()
                .to_string()
                .contains("512 MiB")
        );
        std::fs::remove_file(path).unwrap();
        let mut encoded = Cursor::new(Vec::new());
        RgbaImage::new(4097, 1)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        let manifest = manifest();
        let active = active_manifest();
        let bytes = fixture(
            &[
                ("Manifest.dat", &manifest),
                (ACTIVE_MANIFEST, &active),
                (PREVIEW, encoded.get_ref()),
            ],
            zip::CompressionMethod::Stored,
        );
        assert!(run(&bytes, "oversized-image").is_err());
        let mut bytes = fixture(
            &[("Manifest.dat", &manifest)],
            zip::CompressionMethod::Stored,
        );
        let end = bytes.len() - 22;
        bytes[end + 8..end + 10].copy_from_slice(&4097_u16.to_le_bytes());
        bytes[end + 10..end + 12].copy_from_slice(&4097_u16.to_le_bytes());
        assert!(
            run(&bytes, "oversized-directory")
                .unwrap_err()
                .to_string()
                .contains("directory exceeds")
        );
    }
}
