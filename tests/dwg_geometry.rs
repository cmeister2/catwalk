//! Retained DWG line coordinates, provenance and native thumbnail fallback qualification.

use catwalk::{formats, render::RenderOptions, saved_preview, scene::Vec3};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Resolve a retained DWG fixture or its independent reference data.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/native_geometry/dwg")
        .join(name)
}

#[test]
fn real_dwg_lines_match_independent_autocad_coordinates_and_source_hashes() {
    let provenance: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture("provenance.json")).unwrap()).unwrap();
    for row in provenance.as_array().unwrap() {
        let path = fixture(row["file"].as_str().unwrap());
        assert_eq!(
            format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap())),
            row["sha256"].as_str().unwrap()
        );
        let scene = formats::load(&path).unwrap();
        assert!(scene.triangles.is_empty());
        assert!(scene.points.is_empty());
        assert_eq!(scene.lines.len(), 1);
        // AutoCAD's original property dump gives these independent endpoints
        // rounded to five decimal places; the 2007 DXF retains full precision.
        let expected = [
            Vec3::new(21.607_766_723_020_987, 2.811_923_503_435_004, 0.0),
            Vec3::new(11.030_737_292_854_425, 16.637_544_737_991_476, 0.0),
        ];
        for (actual, reference) in scene.lines[0].vertices.into_iter().zip(expected) {
            assert!((actual - reference).length() < 1e-10, "{}", path.display());
        }
    }
}

#[test]
fn a_genuine_dwg_without_an_image_renders_geometry_by_default() {
    let path = fixture("line-2018.dwg");
    assert!(saved_preview::extract(&path).unwrap().is_none());
    let options = RenderOptions {
        width: 128,
        height: 128,
        transparent: true,
        ..catwalk::render::RenderOptions::default()
    };
    let fresh = catwalk::geometry_thumbnail(&path, &options).unwrap();
    assert!(fresh.pixels().filter(|p| p[3] != 0).count() > 100);
    assert_eq!(catwalk::thumbnail(&path, &options).unwrap(), fresh);
}

#[test]
fn geometry_mode_does_not_fabricate_a_model_from_a_saved_image_carrier() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/saved-previews/blend_dwg/authored-png.dwg");
    assert!(saved_preview::extract(&path).unwrap().is_some());
    assert!(catwalk::geometry_thumbnail(&path, &RenderOptions::default()).is_err());
}
