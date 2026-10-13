//! Native framing, exact bitmap pixels, checksum and compression rejection controls.

use super::*;
use std::io::Write;

/// Serialize a native chunk header with its archive-dependent value width.
fn header(kind: u32, value: usize, width: usize) -> Vec<u8> {
    let mut bytes = kind.to_le_bytes().to_vec();
    bytes.extend_from_slice(&(value as u64).to_le_bytes()[..width]);
    bytes
}

/// Serialize a CRC-bearing long record with an explicit direct-data checksum.
fn record(kind: u32, payload: &[u8], direct: &[u8], width: usize) -> Vec<u8> {
    let mut bytes = header(kind, payload.len() + 4, width);
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(&checksum(direct).to_le_bytes());
    bytes
}

/// Encode one native buffer in raw or anonymous zlib framing.
fn buffer(bytes: &[u8], compressed: bool, width: usize) -> (Vec<u8>, Vec<u8>) {
    let mut direct = u32::try_from(bytes.len()).unwrap().to_le_bytes().to_vec();
    if bytes.is_empty() {
        return (direct.clone(), direct);
    }
    direct.extend_from_slice(&checksum(bytes).to_le_bytes());
    direct.push(u8::from(compressed));
    let mut encoded = direct.clone();
    if compressed {
        let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        zlib.write_all(bytes).unwrap();
        let zlib = zlib.finish().unwrap();
        encoded.extend(record(ANONYMOUS, &zlib, &zlib, width));
    } else {
        direct.extend_from_slice(bytes);
        encoded.extend_from_slice(bytes);
    }
    (encoded, direct)
}

/// Construct a tiny native DIB with known row order and optional indexed colors.
fn bitmap(indexed: bool, top_down: bool) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut header = 40_u32.to_le_bytes().to_vec();
    header.extend_from_slice(&2_i32.to_le_bytes());
    header.extend_from_slice(&(if top_down { -2_i32 } else { 2_i32 }).to_le_bytes());
    header.extend_from_slice(&1_u16.to_le_bytes());
    header.extend_from_slice(&(if indexed { 8_u16 } else { 24_u16 }).to_le_bytes());
    header.extend_from_slice(&0_u32.to_le_bytes());
    header.extend_from_slice(&(if indexed { 8_u32 } else { 16_u32 }).to_le_bytes());
    header.extend_from_slice(&[0; 8]);
    header.extend_from_slice(&(if indexed { 2_u32 } else { 0_u32 }).to_le_bytes());
    header.extend_from_slice(&0_u32.to_le_bytes());
    let palette = if indexed {
        vec![0, 0, 255, 0, 0, 255, 0, 0]
    } else {
        vec![]
    };
    let mut pixels = if indexed {
        vec![0, 1, 0, 0, 1, 0, 0, 0]
    } else {
        vec![0, 0, 255, 0, 255, 0, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0]
    };
    if !top_down {
        let row_bytes = pixels.len() / 2;
        pixels.rotate_left(row_bytes);
    }
    (header, palette, pixels)
}

/// Return the independently specified 2-by-2 red/green checkerboard pixels.
fn expected() -> RgbaImage {
    RgbaImage::from_raw(
        2,
        2,
        vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 255, 0, 0, 255,
        ],
    )
    .unwrap()
}

/// Wrap one addressed record in a properties table with a final short marker.
fn properties(preview: &[u8], width: usize) -> Vec<u8> {
    let mut table = preview.to_vec();
    table.extend(header(END_TABLE, 0, width));
    table
}

#[test]
fn raw_and_compressed_rgb_and_indexed_rows_preserve_exact_pixels() {
    for width in [4, 8] {
        for indexed in [false, true] {
            for top_down in [false, true] {
                let (header, palette, pixels) = bitmap(indexed, top_down);
                let combined = [palette.as_slice(), pixels.as_slice()].concat();
                let raw = [header.as_slice(), combined.as_slice()].concat();
                let raw = record(PREVIEW, &raw, &raw, width);
                assert_eq!(
                    extract_properties(&properties(&raw, width), width)
                        .unwrap()
                        .unwrap(),
                    expected()
                );
                for compression in [false, true] {
                    for separate in [false, true] {
                        let mut payload = header.clone();
                        let mut direct = header.clone();
                        let buffers = if separate {
                            vec![palette.as_slice(), pixels.as_slice()]
                        } else {
                            vec![combined.as_slice()]
                        };
                        for bytes in buffers {
                            let (encoded, crc_bytes) = buffer(bytes, compression, width);
                            payload.extend(encoded);
                            direct.extend(crc_bytes);
                        }
                        let preview = record(COMPRESSED_PREVIEW, &payload, &direct, width);
                        assert_eq!(
                            extract_properties(&properties(&preview, width), width)
                                .unwrap()
                                .unwrap(),
                            expected()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn addressed_properties_ignore_unrelated_images_and_reject_ambiguity() {
    let (header, palette, pixels) = bitmap(false, false);
    let raw = [header, palette, pixels].concat();
    let distractor = record(0x2000_8090, &raw, &raw, 8);
    assert!(
        extract_properties(&properties(&distractor, 8), 8)
            .unwrap()
            .is_none()
    );
    let preview = record(PREVIEW, &raw, &raw, 8);
    let duplicate = [preview.as_slice(), preview.as_slice()].concat();
    assert!(
        extract_properties(&properties(&duplicate, 8), 8)
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
    let mut table = properties(&preview, 8);
    table.push(0);
    assert!(extract_properties(&table, 8).is_err());
    table.truncate(table.len() - 13);
    assert!(extract_properties(&table, 8).is_err());
}

#[test]
fn direct_encoded_and_decoded_checksums_are_independent() {
    let (header, _, pixels) = bitmap(false, false);
    let (encoded, direct_buffer) = buffer(&pixels, true, 8);
    let payload = [header.as_slice(), encoded.as_slice()].concat();
    let direct = [header.as_slice(), direct_buffer.as_slice()].concat();
    let preview = record(COMPRESSED_PREVIEW, &payload, &direct, 8);
    for position in [12 + 44, 12 + payload.len() - 1, preview.len() - 1] {
        let mut damaged = preview.clone();
        damaged[position] ^= 1;
        assert!(
            extract_properties(&properties(&damaged, 8), 8)
                .unwrap_err()
                .to_string()
                .contains("checksum")
        );
    }
}

#[test]
fn dimensions_lengths_and_compression_are_bounded_before_allocation() {
    let (mut header, _, pixels) = bitmap(false, false);
    header[4..8].copy_from_slice(&4097_i32.to_le_bytes());
    assert!(
        bitmap_layout(&header)
            .unwrap_err()
            .to_string()
            .contains("dimension")
    );
    let (header, _, _) = bitmap(false, false);
    let mut payload = header.clone();
    payload.extend_from_slice(&u32::MAX.to_le_bytes());
    let preview = record(COMPRESSED_PREVIEW, &payload, &payload, 8);
    assert!(
        extract_properties(&properties(&preview, 8), 8)
            .unwrap_err()
            .to_string()
            .contains("buffer size")
    );
    let (mut encoded, mut direct) = buffer(&pixels, true, 8);
    encoded[8] = 2;
    direct[8] = 2;
    let payload = [header.as_slice(), encoded.as_slice()].concat();
    let direct = [header.as_slice(), direct.as_slice()].concat();
    let preview = record(COMPRESSED_PREVIEW, &payload, &direct, 8);
    assert!(
        extract_properties(&properties(&preview, 8), 8)
            .unwrap_err()
            .to_string()
            .contains("compression")
    );
}

#[test]
fn zlib_stream_truncation_trailing_data_and_expansion_fail() {
    let (header, _, pixels) = bitmap(false, false);
    let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    zlib.write_all(&pixels).unwrap();
    let zlib = zlib.finish().unwrap();
    let mut extra = zlib.clone();
    extra.push(0);
    let mut too_long = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    too_long.write_all(&[0; 17]).unwrap();
    for encoded in [
        zlib[..zlib.len() - 1].to_vec(),
        extra,
        too_long.finish().unwrap(),
    ] {
        let mut direct = header.clone();
        direct.extend_from_slice(&u32::try_from(pixels.len()).unwrap().to_le_bytes());
        direct.extend_from_slice(&checksum(&pixels).to_le_bytes());
        direct.push(1);
        let payload = [
            direct.as_slice(),
            record(ANONYMOUS, &encoded, &encoded, 8).as_slice(),
        ]
        .concat();
        let preview = record(COMPRESSED_PREVIEW, &payload, &direct, 8);
        assert!(extract_properties(&properties(&preview, 8), 8).is_err());
    }
}

#[test]
fn native_signature_and_version_bands_are_explicit() {
    for version in ["2", "3", "4", "50", "60", "70", "80"] {
        let start = format!("3D Geometry File Format {version:>8}");
        assert_eq!(
            archive_width(start.as_bytes().try_into().unwrap()).unwrap(),
            if version.len() == 1 { 4 } else { 8 }
        );
    }
    for version in ["1", "5", "90", " 60 ", "+60"] {
        let start = format!("3D Geometry File Format {version:>8}");
        assert!(archive_width(start.as_bytes().try_into().unwrap()).is_err());
    }
    assert!(archive_width(&[0; 32]).is_err());
}
