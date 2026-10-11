//! LAS/LAZ point previews using Rust LAS parsing and LAZ decompression.
use crate::{
    Result,
    scene::{Point, Scene, Vec3},
};
use std::{io::Cursor, path::Path};

/// Maximum LAS header point count accepted for streaming validation.
const MAX_DECLARED_POINTS: u64 = 20_000_000;
/// Maximum sampled LAS points retained for thumbnail rendering.
const MAX_PREVIEW_POINTS: u64 = 1_000_000;

/// Read a bounded LAS or LAZ point cloud and decode its preview geometry.
///
/// # Errors
/// Returns an error for unreadable/malformed LAS or LAZ records, invalid counts, truncated
/// point data or nonfinite coordinates.
pub(super) fn load(path: &Path) -> Result<Scene> {
    decode(super::read_file(path)?)
}

/// Decode LAS or LAZ records, sample at most one million points and normalize stored RGB channels.
#[allow(
    clippy::cast_possible_truncation,
    reason = "RGB words are bounded to u16 and rescaled into the byte range before their final conversion."
)]
fn decode(bytes: Vec<u8>) -> Result<Scene> {
    let mut reader = las::Reader::new(Cursor::new(bytes))?;
    let count = reader.header().number_of_points();
    if count == 0 || count > MAX_DECLARED_POINTS {
        return Err("LAS point count must be between 1 and 20 million".into());
    }
    let step = count.div_ceil(MAX_PREVIEW_POINTS);
    let mut scene = Scene::default();
    let mut full_colors = Vec::new();
    let mut seen = 0u64;
    let mut max_color = 0u16;
    while seen < count {
        let batch = reader.read_points((count - seen).min(65_536))?;
        if batch.is_empty() {
            return Err("LAS file has fewer points than declared".into());
        }
        for item in batch.points() {
            let point = item?;
            if seen.is_multiple_of(step) {
                let position = Vec3::new(point.x, point.y, point.z);
                if !position.is_finite() {
                    return Err("LAS point coordinates are not finite".into());
                }
                let color = point.color.map(|c| [c.red, c.green, c.blue]);
                if let Some(color) = color {
                    max_color = max_color.max(*color.iter().max().unwrap_or(&0));
                }
                scene.points.push(Point {
                    position,
                    color: None,
                });
                full_colors.push(color);
            }
            seen += 1;
        }
    }
    if seen != count {
        return Err("LAS point count does not match its header".into());
    }
    let divisor = if max_color <= 255 { 1u32 } else { 257u32 };
    for (point, color) in scene.points.iter_mut().zip(full_colors) {
        point.color =
            color.map(|channels| channels.map(|c| ((u32::from(c) + divisor / 2) / divisor) as u8));
    }
    scene.validate()?;
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn encoded(compressed: bool, rgb: [u16; 3]) -> Vec<u8> {
        let mut builder = las::Builder::from((1, 4));
        builder.point_format = las::point::Format::new(2).unwrap();
        builder.point_format.is_compressed = compressed;
        let mut writer =
            las::Writer::new(Cursor::new(Vec::new()), builder.into_header().unwrap()).unwrap();
        for i in 0..3 {
            let point = las::Point {
                x: 1000. + f64::from(i),
                y: 2000.,
                z: 10.,
                color: Some(las::Color::new(rgb[0], rgb[1], rgb[2])),
                ..Default::default()
            };
            writer.write_point(point).unwrap();
        }
        writer.into_inner().unwrap().into_inner()
    }

    #[test]
    fn las_and_laz_decode_real_compressed_records_and_scaled_colors() {
        for compressed in [false, true] {
            let scene = decode(encoded(compressed, [65535, 32768, 0])).unwrap();
            assert_eq!(scene.points.len(), 3);
            assert_eq!(scene.points[2].position, Vec3::new(1002., 2000., 10.));
            assert_eq!(scene.points[0].color, Some([255, 128, 0]));
            assert!(
                crate::render::render(&scene, &crate::render::RenderOptions::default()).is_ok()
            );
        }
    }

    #[test]
    fn eight_bit_colors_and_truncated_records() {
        assert_eq!(
            decode(encoded(false, [255, 128, 10])).unwrap().points[0].color,
            Some([255, 128, 10])
        );
        let mut bytes = encoded(false, [255, 0, 0]);
        bytes.truncate(bytes.len() - 20);
        assert!(decode(bytes).is_err());
        assert!(decode(b"not a LAS file".to_vec()).is_err());
    }
}
