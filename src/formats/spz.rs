//! SPZ 1–4 center/color previews, with Rust gzip and Zstandard decoding.
use crate::{
    Result,
    scene::{Point, Scene, Vec3},
};
use std::{
    io::{Cursor, Read},
    path::Path,
};

/// Maximum declared point count accepted before geometry allocation.
const MAX_POINTS: usize = 20_000_000;
/// Maximum sampled points retained in the preview scene.
const PREVIEW_POINTS: usize = 1_000_000;
/// Degree-zero spherical-harmonic normalization used to reconstruct splat RGB.
const SH_C0: f64 = 0.282_094_791_773_878_14;

/// Read a bounded SPZ splat-center stream and decode its preview geometry.
///
/// # Errors
/// Returns an error for unsupported headers, malformed gzip/Zstandard data, invalid point
/// counts, nonfinite coordinates or stream-size mismatches.
pub(super) fn load(path: &Path) -> Result<Scene> {
    decode(&super::read_file(path)?)
}

/// Read a little-endian SPZ header word at a checked offset.
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or("truncated SPZ header")?
            .try_into()?,
    ))
}

/// Validate version, point count, harmonic degree, coordinate precision and reserved header fields.
fn header(bytes: &[u8], legacy: bool) -> Result<(usize, u32, usize, u8)> {
    if bytes.len() < if legacy { 16 } else { 32 } || &bytes[..4] != b"NGSP" {
        return Err("missing SPZ NGSP header".into());
    }
    let version = u32_at(bytes, 4)?;
    if (legacy && !(1..=3).contains(&version)) || (!legacy && version != 4) {
        return Err("SPZ supports gzip versions 1–3 and Zstandard version 4".into());
    }
    let count = u32_at(bytes, 8)? as usize;
    if count == 0 || count > MAX_POINTS {
        return Err("SPZ point count must be between 1 and 20 million".into());
    }
    let degree = bytes[12] as usize;
    if degree > 4 || bytes[13] > 30 {
        return Err("invalid SPZ spherical harmonic degree or coordinate precision".into());
    }
    if bytes[14] & !1 != 0 {
        return Err("SPZ extensions and unknown flags are unsupported".into());
    }
    if (legacy && bytes[15] != 0) || (!legacy && bytes[20..32].iter().any(|v| *v != 0)) {
        return Err("nonzero SPZ reserved header fields".into());
    }
    Ok((count, version, degree * (degree + 2) * 3, bytes[13]))
}

/// Decompress exactly the declared attribute bytes without exceeding the decoded-input limit.
fn bounded_decode(mut reader: impl Read, expected: usize) -> Result<Vec<u8>> {
    if expected as u64 > super::MAX_INPUT_BYTES {
        return Err("SPZ decoded attributes exceed 512 MiB".into());
    }
    let mut output = Vec::new();
    reader
        .by_ref()
        .take(expected as u64 + 1)
        .read_to_end(&mut output)?;
    if output.len() != expected {
        return Err("SPZ decoded attribute length does not match its header".into());
    }
    Ok(output)
}

/// Expand an IEEE binary16 coordinate, preserving subnormal and nonfinite encodings.
fn half(value: u16) -> f64 {
    let sign = if value & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = (value >> 10) & 31;
    let mantissa = value & 1023;
    sign * match exponent {
        0 => f64::from(mantissa) * 2f64.powi(-24),
        31 => {
            if mantissa == 0 {
                f64::INFINITY
            } else {
                f64::NAN
            }
        }
        _ => (1.0 + f64::from(mantissa) / 1024.0) * 2f64.powi(i32::from(exponent) - 15),
    }
}

/// Sample visible SPZ centers, convert Y-up coordinates and reconstruct RGB from degree-zero
/// harmonics.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Reconstructed color channels are clamped to the byte range and intentionally rounded before quantization."
)]
fn scene(count: usize, version: u32, fractional: u8, attributes: &[Vec<u8>]) -> Result<Scene> {
    let mut scene = Scene::default();
    let stride = count.div_ceil(PREVIEW_POINTS);
    let bytes_per_coordinate = if version == 1 { 2 } else { 3 };
    let factor = 2f64.powi(-i32::from(fractional));
    for index in 0..count {
        let mut coordinates = [0.0; 3];
        for (axis, value) in coordinates.iter_mut().enumerate() {
            let offset = (index * 3 + axis) * bytes_per_coordinate;
            *value = if version == 1 {
                half(u16::from_le_bytes(
                    attributes[0][offset..offset + 2].try_into()?,
                ))
            } else {
                let b = &attributes[0][offset..offset + 3];
                // Signed 24-bit fixed point, sign extended before scaling.
                let packed = i32::from(b[0]) | i32::from(b[1]) << 8 | i32::from(b[2]) << 16;
                f64::from((packed << 8) >> 8) * factor
            };
        }
        let [x, y, z] = coordinates;
        let position = Vec3::new(x, -z, y); // SPZ is Right-Up-Back; scene is Z-up.
        if !position.is_finite() {
            return Err("SPZ has non-finite coordinates".into());
        }
        if index.is_multiple_of(stride) && attributes[1][index] != 0 {
            let color = std::array::from_fn(|channel| {
                let encoded = f64::from(attributes[2][index * 3 + channel]) / 255.0;
                ((0.5 + SH_C0 * (encoded - 0.5) / 0.15).clamp(0.0, 1.0) * 255.0).round() as u8
            });
            scene.points.push(Point {
                position,
                color: Some(color),
            });
        }
    }
    scene.validate()?;
    Ok(scene)
}

/// Decode gzip SPZ versions one through three or independently compressed Zstandard v4
/// attribute streams.
fn decode(bytes: &[u8]) -> Result<Scene> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = flate2::bufread::GzDecoder::new(bytes);
        let mut data = Vec::new();
        decoder
            .by_ref()
            .take(super::MAX_INPUT_BYTES + 1)
            .read_to_end(&mut data)?;
        if data.len() as u64 > super::MAX_INPUT_BYTES || !decoder.into_inner().is_empty() {
            return Err("SPZ gzip stream is oversized or contains trailing bytes".into());
        }
        let (count, version, sh, fractional) = header(&data, true)?;
        let sizes = [
            count * if version == 1 { 6 } else { 9 },
            count,
            count * 3,
            count * 3,
            count * if version >= 3 { 4 } else { 3 },
            count * sh,
        ];
        let expected = 16 + sizes.iter().sum::<usize>();
        if data.len() != expected {
            return Err("SPZ gzip payload length does not match its header".into());
        }
        let mut offset = 16;
        let attributes: Vec<Vec<u8>> = sizes
            .iter()
            .map(|size| {
                let attribute = data[offset..offset + size].to_vec();
                offset += size;
                attribute
            })
            .collect();
        scene(count, version, fractional, &attributes)
    } else {
        let (count, version, sh, fractional) = header(bytes, false)?;
        let sizes = [
            count * 9,
            count,
            count * 3,
            count * 3,
            count * 4,
            count * sh,
        ];
        if sizes.iter().sum::<usize>() as u64 > super::MAX_INPUT_BYTES {
            return Err("SPZ decoded attributes exceed 512 MiB".into());
        }
        let sizes: Vec<usize> = sizes.into_iter().filter(|size| *size != 0).collect();
        if bytes[15] as usize != sizes.len() {
            return Err("SPZ stream count does not match its attributes".into());
        }
        let toc = u32_at(bytes, 16)? as usize;
        if toc != 32 {
            return Err("SPZ extension header data is unsupported".into());
        }
        let mut offset = toc + sizes.len() * 16;
        let mut attributes = Vec::new();
        for (index, expected) in sizes.into_iter().enumerate() {
            let entry = bytes
                .get(toc + index * 16..toc + (index + 1) * 16)
                .ok_or("truncated SPZ stream table")?;
            let compressed = usize::try_from(u64::from_le_bytes(entry[..8].try_into()?))?;
            let uncompressed = usize::try_from(u64::from_le_bytes(entry[8..].try_into()?))?;
            if uncompressed != expected || compressed == 0 {
                return Err("invalid SPZ stream lengths".into());
            }
            let end = offset
                .checked_add(compressed)
                .ok_or("SPZ stream size overflow")?;
            let input = bytes
                .get(offset..end)
                .ok_or("truncated SPZ compressed stream")?;
            let mut decoder = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(
                Cursor::new(input),
                128 * 1024 * 1024,
            )?;
            attributes.push(bounded_decode(&mut decoder, expected)?);
            if decoder.into_inner().position() != input.len() as u64 {
                return Err("SPZ Zstandard stream contains trailing bytes".into());
            }
            offset = end;
        }
        if offset != bytes.len() {
            return Err("unexpected bytes after SPZ streams".into());
        }
        scene(count, version, fractional, &attributes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn attributes(version: u32) -> Vec<Vec<u8>> {
        let mut positions = Vec::new();
        for values in [[-4096i32, 8192, 12288], [0, 0, 0]] {
            for value in values {
                positions.extend(&value.to_le_bytes()[..3]);
            }
        }
        vec![
            positions,
            vec![255, 0],
            vec![128, 192, 64, 128, 128, 128],
            vec![100; 6],
            vec![0; if version == 2 { 6 } else { 8 }],
        ]
    }
    fn gzip(version: u32) -> Vec<u8> {
        let mut bytes = b"NGSP".to_vec();
        bytes.extend(version.to_le_bytes());
        bytes.extend(2u32.to_le_bytes());
        bytes.extend([0, 12, 0, 0]);
        for attribute in attributes(version) {
            bytes.extend(attribute);
        }
        let mut writer = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        writer.write_all(&bytes).unwrap();
        writer.finish().unwrap()
    }
    // Independent Zstandard raw-block framing; does not use the decoder as an encoder.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "The authored raw Zstandard test payload fits the intentionally encoded single-byte frame size."
    )]
    fn raw_zstd(data: &[u8]) -> Vec<u8> {
        let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0x20, data.len() as u8];
        let block = ((data.len() as u32) << 3) | 1;
        frame.extend(&block.to_le_bytes()[..3]);
        frame.extend(data);
        frame
    }
    fn v4() -> Vec<u8> {
        let attributes = attributes(4);
        let mut bytes = b"NGSP".to_vec();
        bytes.extend(4u32.to_le_bytes());
        bytes.extend(2u32.to_le_bytes());
        bytes.extend([0, 12, 0, 5]);
        bytes.extend(32u32.to_le_bytes());
        bytes.extend([0; 12]);
        let frames: Vec<_> = attributes.iter().map(|a| raw_zstd(a)).collect();
        for (a, f) in attributes.iter().zip(&frames) {
            bytes.extend((f.len() as u64).to_le_bytes());
            bytes.extend((a.len() as u64).to_le_bytes());
        }
        for f in frames {
            bytes.extend(f);
        }
        bytes
    }
    #[test]
    fn legacy_and_zstandard_positions_colors_and_alpha() {
        for bytes in [gzip(2), gzip(3), v4()] {
            let scene = decode(&bytes).unwrap();
            assert_eq!(scene.points.len(), 1);
            assert_eq!(scene.points[0].position, Vec3::new(-1., -3., 2.));
            assert_eq!(scene.points[0].color, Some([128, 249, 8]));
            assert!(
                crate::render::render(&scene, &crate::render::RenderOptions::default()).is_ok()
            );
        }
        assert_eq!(half(0xbc00), -1.0);
        assert_eq!(half(0x4000), 2.0);
        assert!(half(0x7c00).is_infinite());
    }
    #[test]
    fn malformed_compression_headers_and_streams_fail() {
        let mut bad = v4();
        bad[14] = 2;
        assert!(decode(&bad).is_err());
        let mut bad = v4();
        bad[12] = 5;
        assert!(decode(&bad).is_err());
        let mut bad = v4();
        bad[15] = 6;
        assert!(decode(&bad).is_err());
        let mut bad = v4();
        bad[40] = 200;
        assert!(decode(&bad).is_err());
        let mut bad = v4();
        bad.pop();
        assert!(decode(&bad).is_err());
        let mut bad = gzip(2);
        bad.pop();
        assert!(decode(&bad).is_err());
        let mut bad = gzip(2);
        bad.push(0);
        assert!(decode(&bad).is_err());
        assert!(decode(b"bad").is_err());
    }
}
