//! Licensed native-part qualification using independent geometry and saved-image removal evidence.
use catwalk::render::{RenderOptions, render};
use catwalk_autodesk_geometry::load_geometry;
use std::path::{Path, PathBuf};

/// Locate a retained original or preview-stripped native document.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("evidence/fixtures")
        .join(name)
}

/// Compare original/stripped geometry and pixels against counts and an immutable PNG baseline.
fn compare(original: &str, stripped: &str, bodies: u64, faces: u64) -> catwalk::scene::Scene {
    let original = fixture(original);
    let stripped = fixture(stripped);
    assert!(
        catwalk::saved_preview::extract(&original)
            .unwrap()
            .is_some()
    );
    assert!(
        catwalk::saved_preview::extract(&stripped)
            .unwrap()
            .is_none()
    );
    let (a, ra) = load_geometry(&original).unwrap();
    let (b, rb) = load_geometry(&stripped).unwrap();
    assert_eq!(ra["source_bodies"], bodies);
    assert_eq!(ra["source_faces"], faces);
    assert_eq!(ra["source_bodies"], rb["source_bodies"]);
    assert_eq!(ra["source_faces"], rb["source_faces"]);
    assert_eq!(ra["source_edges"], rb["source_edges"]);
    assert_eq!(ra["geometry_sha256"], rb["geometry_sha256"]);
    assert!(a.triangles.len() > usize::try_from(faces).unwrap());
    assert!(a.lines.is_empty() && a.points.is_empty());
    let options = RenderOptions {
        width: 256,
        height: 256,
        transparent: true,
        supersampling: 2,
        ..RenderOptions::default()
    };
    let left = render(&a, &options).unwrap();
    let right = render(&b, &options).unwrap();
    assert_eq!(left.as_raw(), right.as_raw());
    assert!(right.pixels().any(|p| p[3] > 0));
    let snapshot = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("evidence/outputs")
        .join(format!(
            "{}.png",
            stripped
                .file_name()
                .unwrap()
                .to_string_lossy()
                .split("-without-preview")
                .next()
                .unwrap()
        ));
    let expected = image::open(snapshot).unwrap().to_rgba8();
    assert_eq!(right.dimensions(), expected.dimensions());
    let mut difference = image::RgbaImage::new(right.width(), right.height());
    let mut maximum = 0;
    for ((actual, expected), difference) in right
        .pixels()
        .zip(expected.pixels())
        .zip(difference.pixels_mut())
    {
        for channel in 0..4 {
            let component = |pixel: &image::Rgba<u8>| {
                if channel == 3 {
                    pixel[3]
                } else {
                    u8::try_from((u16::from(pixel[channel]) * u16::from(pixel[3]) + 127) / 255)
                        .unwrap()
                }
            };
            let delta = component(actual).abs_diff(component(expected));
            maximum = maximum.max(delta);
            difference[channel] = delta.saturating_mul(32);
        }
        difference[3] = 255;
    }
    let artifacts = std::env::var_os("CATWALK_AUTODESK_ARTIFACTS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("target/real-geometry-snapshots"),
        PathBuf::from,
    );
    std::fs::create_dir_all(&artifacts).unwrap();
    let name = stripped.file_stem().unwrap().to_string_lossy();
    right
        .save(artifacts.join(format!("{name}-actual.png")))
        .unwrap();
    expected
        .save(artifacts.join(format!("{name}-expected.png")))
        .unwrap();
    difference
        .save(artifacts.join(format!("{name}-diff.png")))
        .unwrap();
    std::fs::write(
        artifacts.join(format!("{name}-geometry.json")),
        serde_json::to_vec_pretty(&rb).unwrap(),
    )
    .unwrap();
    // Premultiplication ignores meaningless RGB values in completely transparent pixels.
    assert!(
        maximum <= 2,
        "snapshot maximum channel difference {maximum} exceeds 2"
    );
    b
}

#[test]
fn fusion_extrusion_without_image_renders_the_complete_body() {
    compare(
        "fusion-extrusion.f3d",
        "fusion-extrusion-without-preview.f3d",
        1,
        107,
    );
}

#[test]
fn fusion_history_is_not_rendered_as_duplicate_bodies() {
    compare(
        "fusion-switch.f3d",
        "fusion-switch-without-preview.f3d",
        1,
        19,
    );
}

#[test]
fn current_inventor_zstandard_part_without_image_renders_three_curved_faces() {
    let scene = compare(
        "inventor-cylinder.ipt",
        "inventor-cylinder-without-preview.ipt",
        1,
        3,
    );
    // The saved BRep is a radius-12.5 mm cylinder, 10 mm long, along Y.
    let area: f64 = scene
        .triangles
        .iter()
        .map(|t| {
            (t.vertices[1] - t.vertices[0])
                .cross(t.vertices[2] - t.vertices[0])
                .length()
                * 0.5
        })
        .sum();
    let expected = 2.0 * std::f64::consts::PI * 12.5 * (12.5 + 10.0);
    assert!((area / expected - 1.0).abs() < 0.01);
    for (axis, min, max) in [(0, -12.5, 12.5), (1, 0.0, 10.0), (2, -12.5, 12.5)] {
        let values = scene
            .positions()
            .map(|p| [p.x, p.y, p.z][axis])
            .collect::<Vec<_>>();
        assert!((values.iter().copied().fold(f64::INFINITY, f64::min) - min).abs() < 0.1);
        assert!((values.iter().copied().fold(f64::NEG_INFINITY, f64::max) - max).abs() < 0.1);
    }
}

#[test]
fn older_inventor_part_without_image_preserves_all_pip_faces() {
    compare(
        "inventor-dice.ipt",
        "inventor-dice-without-preview.ipt",
        1,
        49,
    );
}

#[test]
fn unresolved_assembly_returns_an_explicit_dependency_error() {
    let error = load_geometry(&fixture("inventor-subassembly-without-preview.iam")).unwrap_err();
    assert!(error.to_string().contains("external prototypes"));
}
