//! Every-extension corpus checks and committed decoded-pixel image comparisons.

#[path = "support/format_corpus.rs"]
mod corpus;

use image::{Rgba, RgbaImage};
use std::path::Path;

#[test]
fn every_advertised_format_renders_and_matches_its_committed_png() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
    let artifacts = corpus::artifact_directory(workspace);
    let failures = corpus::run(
        workspace,
        &artifacts,
        Some(Path::new(env!("CARGO_BIN_EXE_catwalk"))),
        false,
    )
    .expect("format corpus must be valid and cover every advertised extension");
    assert!(
        failures.is_empty(),
        "{} format snapshot failures:\n{}\nActual, expected and diff images: {}",
        failures.len(),
        failures.join("\n"),
        artifacts.display()
    );
}

#[test]
fn snapshot_comparison_detects_missing_geometry_color_and_alpha_changes() {
    let expected = RgbaImage::from_pixel(8, 8, Rgba([120, 60, 30, 255]));
    let mut actual = expected.clone();
    let (equal, _) = corpus::compare(&expected, &actual).unwrap();
    assert!(equal.matches());
    actual.put_pixel(3, 4, Rgba([0, 0, 0, 0]));
    let (missing, diff) = corpus::compare(&expected, &actual).unwrap();
    assert!(!missing.matches());
    assert_eq!(missing.changed_pixels, 1);
    assert_ne!(diff.get_pixel(3, 4), diff.get_pixel(0, 0));
    actual = expected.clone();
    actual.put_pixel(3, 4, Rgba([128, 60, 30, 255]));
    assert!(!corpus::compare(&expected, &actual).unwrap().0.matches());
    actual.put_pixel(3, 4, Rgba([120, 60, 30, 248]));
    assert!(!corpus::compare(&expected, &actual).unwrap().0.matches());
}

#[test]
fn transparent_rgb_is_ignored_and_small_rounding_is_bounded() {
    let expected = RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 0]));
    let actual = RgbaImage::from_pixel(2, 2, Rgba([200, 100, 50, 0]));
    assert!(corpus::compare(&expected, &actual).unwrap().0.matches());
    let expected = RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 255]));
    let actual = RgbaImage::from_pixel(2, 2, Rgba([12, 20, 30, 255]));
    assert!(corpus::compare(&expected, &actual).unwrap().0.matches());
    let actual = RgbaImage::from_pixel(2, 2, Rgba([13, 20, 30, 255]));
    assert!(!corpus::compare(&expected, &actual).unwrap().0.matches());
    assert!(corpus::compare(&expected, &RgbaImage::new(3, 2)).is_err());
}
