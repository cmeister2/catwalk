//! Mandatory retained-document and exact saved-pixel qualification through public extraction APIs.
use catwalk3d::saved_preview::{blend_dwg, lightburn};
use std::path::{Path, PathBuf};

/// Resolve a saved-preview source or reviewed pixel snapshot from the repository corpus.
fn fixture(family: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/saved-previews")
        .join(family)
        .join(name)
}

#[test]
fn real_lightburn_legacy_and_current_previews_match_saved_png_pixels() {
    for (source, expected) in [
        ("legacy-example1.lbrn", "legacy-example1.png"),
        ("current-jig.lbrn2", "current-jig.png"),
    ] {
        let actual = lightburn::extract(&fixture("lightburn", source))
            .unwrap()
            .unwrap();
        let expected = image::open(fixture("lightburn", expected))
            .unwrap()
            .to_rgba8();
        assert_eq!(actual.dimensions(), (256, 256));
        assert_eq!(actual, expected);
        assert!(actual.pixels().any(|pixel| pixel[3] != 0));
    }
}

#[test]
fn licensed_blender_files_match_saved_pixel_snapshots() {
    for name in ["blend-cube", "blend-280", "blend-extras", "blend-gzip-252"] {
        let actual = blend_dwg::extract_blend(&fixture("blend_dwg", &format!("{name}.blend")))
            .unwrap()
            .unwrap();
        let expected = image::open(fixture("blend_dwg", &format!("{name}.png")))
            .unwrap()
            .into_rgba8();
        assert_eq!(actual, expected, "saved-pixel snapshot: {name}");
    }
}

#[test]
fn portable_authored_carriers_match_pixel_snapshots_and_absence() {
    for name in [
        "authored-legacy-32",
        "authored-legacy-64",
        "authored-format1",
    ] {
        let actual = blend_dwg::extract_blend(&fixture("blend_dwg", &format!("{name}.blend")))
            .unwrap()
            .unwrap();
        let expected = image::open(fixture("blend_dwg", &format!("{name}.png")))
            .unwrap()
            .into_rgba8();
        assert_eq!(actual, expected, "authored Blend carrier: {name}");
    }
    for name in ["authored-dib", "authored-png"] {
        let actual = blend_dwg::extract_dwg(&fixture("blend_dwg", &format!("{name}.dwg")))
            .unwrap()
            .unwrap();
        let expected = image::open(fixture("blend_dwg", &format!("{name}.png")))
            .unwrap()
            .into_rgba8();
        assert_eq!(actual, expected, "authored DWG carrier: {name}");
    }
    for name in ["blend-gzip-no-preview.blend", "authored-no-preview.blend"] {
        assert!(
            blend_dwg::extract_blend(&fixture("blend_dwg", name))
                .unwrap()
                .is_none()
        );
    }
    for name in ["authored-metadata-only.dwg", "authored-no-preview.dwg"] {
        assert!(
            blend_dwg::extract_dwg(&fixture("blend_dwg", name))
                .unwrap()
                .is_none()
        );
    }
}
