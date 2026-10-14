//! Genuine legacy `SketchUp` archive geometry, placements and image-independent loading.

use catwalk3d::{
    formats, geometry_thumbnail,
    render::RenderOptions,
    scene::{Scene, Vec3},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Independent temporary source-copy identifiers.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Resolve an unchanged legacy native producer fixture.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/sketchup")
        .join(name)
}

/// Unique mutated source, removed during normal return or assertion unwind.
struct Temporary(PathBuf);

impl Temporary {
    /// Save the admitted producer-copy bytes without changing its native extension.
    fn new(data: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "catwalk-skp-legacy-{}-{}.skp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, data).unwrap();
        Self(path)
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        fs::remove_file(&self.0).unwrap();
    }
}

/// Measure placed bounds independently of native bounding-box caches.
fn bounds(scene: &Scene) -> [Vec3; 2] {
    let mut low = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut high = -low;
    for v in scene.positions() {
        low = Vec3::new(low.x.min(v.x), low.y.min(v.y), low.z.min(v.z));
        high = Vec3::new(high.x.max(v.x), high.y.max(v.y), high.z.max(v.z));
    }
    [low, high]
}

/// Measure the area of the actual emitted native triangle geometry.
fn area(scene: &Scene) -> f64 {
    scene
        .triangles
        .iter()
        .map(|t| {
            (t.vertices[1] - t.vertices[0])
                .cross(t.vertices[2] - t.vertices[0])
                .length()
                / 2.
        })
        .sum()
}

#[test]
fn original_legacy_versions_emit_the_same_independently_exported_box() {
    let witnesses: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture("geometry-witnesses.json")).unwrap()).unwrap();
    for name in [
        "legacy-box-v2013.skp",
        "legacy-box-v2014.skp",
        "legacy-box-v2015.skp",
        "legacy-box-v2016.skp",
        "2017-box.skp",
        "2017-box-two-materials.skp",
    ] {
        let source = fs::read(fixture(name)).unwrap();
        let scene = formats::load(&fixture(name)).unwrap();
        assert_eq!(scene.triangles.len(), 12, "{name}");
        assert!(scene.lines.is_empty());
        let witness = if name.contains("two-materials") {
            "box-two-materials"
        } else {
            "box"
        };
        let expected: [[f64; 3]; 2] =
            serde_json::from_value(witnesses[witness]["bounds_mm"].clone()).unwrap();
        for (actual, expected) in bounds(&scene).into_iter().zip(expected) {
            assert!(
                (actual - Vec3::new(expected[0], expected[1], expected[2])).length() < 0.01,
                "{name}"
            );
        }
        let expected = witnesses[witness]["one_sided_area_mm2"].as_f64().unwrap();
        assert!((area(&scene) - expected).abs() < expected * 1e-6, "{name}");
        let volume: f64 = scene
            .triangles
            .iter()
            .map(|t| t.vertices[0].dot(t.vertices[1].cross(t.vertices[2])) / 6.)
            .sum();
        let extent = bounds(&scene)[1] - bounds(&scene)[0];
        let expected_volume = extent.x * extent.y * extent.z;
        assert!(
            (volume - expected_volume).abs() < expected_volume * 1e-6,
            "{name}"
        );
        assert_eq!(fs::read(fixture(name)).unwrap(), source);
    }
}

#[test]
fn legacy_holes_and_component_placements_match_independent_collada_measurements() {
    let witnesses: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture("geometry-witnesses.json")).unwrap()).unwrap();
    for name in [
        "box-component-two-instances",
        "face-with-hole",
        "component-rotate",
    ] {
        let scene = formats::load(&fixture(&format!("2017-{name}.skp"))).unwrap();
        let expected: [[f64; 3]; 2] =
            serde_json::from_value(witnesses[name]["bounds_mm"].clone()).unwrap();
        for (actual, expected) in bounds(&scene).into_iter().zip(expected) {
            assert!(
                (actual - Vec3::new(expected[0], expected[1], expected[2])).length() < 0.01,
                "{name}"
            );
        }
        let expected = witnesses[name]["one_sided_area_mm2"].as_f64().unwrap();
        assert!((area(&scene) - expected).abs() < expected * 1e-6, "{name}");
    }
}

#[test]
fn geometry_does_not_decode_thumbnail_pixels_or_calibrate_map_from_the_layer_tag() {
    for name in ["legacy-box-v2013.skp", "2017-box.skp"] {
        let mut data = fs::read(fixture(name)).unwrap();
        // CDib remains a native object and consumes its original archive-map slot.
        // Removing only its addressed image extent must not shift any reference identity.
        let class = data.windows(4).position(|v| v == b"CDib").unwrap();
        let length = class + 4 + 4;
        let size = usize::try_from(u32::from_le_bytes(
            data[length..length + 4].try_into().unwrap(),
        ))
        .unwrap();
        data[length..length + 4].copy_from_slice(&0_u32.to_le_bytes());
        data.drain(length + 4..length + 4 + size);
        let input = Temporary::new(&data);
        let scene = formats::load(&input.0).unwrap();
        assert_eq!(scene.triangles.len(), 12);
        let image = geometry_thumbnail(
            &input.0,
            &RenderOptions {
                width: 96,
                height: 64,
                transparent: true,
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert!(image.pixels().any(|p| p[3] != 0));
    }
}

#[test]
fn malformed_native_geometry_and_prefix_references_are_rejected() {
    let source = fs::read(fixture("2017-box.skp")).unwrap();
    let mut data = source.clone();
    // The first native runtime-class pointer follows the dynamically decoded header.
    data[0x50..0x52].copy_from_slice(&0x7ffe_u16.to_le_bytes());
    assert!(formats::load(&Temporary::new(&data).0).is_err());
    let mut data = source;
    let vertex = data.windows(7).position(|v| v == b"CVertex").unwrap() + 7;
    // First vertex: null attribute pointer, a presence mask and its pid bytes precede xyz.
    let position = vertex + 3 + usize::try_from(data[vertex + 2].count_ones()).unwrap();
    data[position..position + 8].copy_from_slice(&f64::NAN.to_le_bytes());
    assert!(formats::load(&Temporary::new(&data).0).is_err());
}
