//! Independent geometry measurements and thumbnail snapshots for retained Blender files.

use catwalk3d::{formats, geometry_thumbnail, render::RenderOptions, saved_preview, scene::Scene};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

/// Resolve a retained Blender geometry fixture under the repository root.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/native_geometry/blend")
        .join(name)
}
/// Measure axis-aligned bounds independently from every emitted position.
fn bounds(scene: &Scene) -> ([f64; 3], [f64; 3]) {
    let mut lower = [f64::INFINITY; 3];
    let mut upper = [f64::NEG_INFINITY; 3];
    for position in scene.positions() {
        for (axis, value) in [position.x, position.y, position.z].into_iter().enumerate() {
            lower[axis] = lower[axis].min(value);
            upper[axis] = upper[axis].max(value);
        }
    }
    (lower, upper)
}
/// Measure total triangulated surface area from edge cross products.
fn area(scene: &Scene) -> f64 {
    scene
        .triangles
        .iter()
        .map(|triangle| {
            (triangle.vertices[1] - triangle.vertices[0])
                .cross(triangle.vertices[2] - triangle.vertices[0])
                .length()
                * 0.5
        })
        .sum()
}
/// Compare an independent measured value with the retained-file floating-point tolerance.
fn near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 2e-5,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn real_blender_files_without_thumbnails_have_measured_geometry() {
    // Counts and bounds come from the source mesh topology and saved transforms,
    // independently inspected from the original blocks. The 5.2 scene
    // includes collection instances, parent inverse matrices and repeated meshes.
    for (file, triangles, lines, points, lower, upper, surface_area) in [
        (
            "default-252-no-thumbnail.blend",
            12,
            0,
            0,
            [-1., -1., -1.],
            [1., 1., 1.],
            Some(24.),
        ),
        (
            "cube-278-no-thumbnail.blend",
            12,
            0,
            0,
            [-1., -1., -1.],
            [1., 1., 1.],
            Some(24.),
        ),
        (
            "shared-278-no-thumbnail.blend",
            24,
            0,
            0,
            [-1., -std::f64::consts::SQRT_2, -std::f64::consts::SQRT_2],
            [5., std::f64::consts::SQRT_2, std::f64::consts::SQRT_2],
            Some(48.),
        ),
        (
            "mesh-280-no-thumbnail.blend",
            24,
            0,
            0,
            [-1., -1., -1.],
            [1., 1., 1.828_840_082_487_000_6],
            Some(27.840_000_305_176_275),
        ),
        (
            "loose-291-no-thumbnail.blend",
            6,
            3,
            3,
            [-1., -1., -1.],
            [2.505_124_330_520_63, 1., 2.396_704_673_767_09],
            Some(12.),
        ),
        (
            "hierarchy-502-no-thumbnail.blend",
            7208,
            0,
            0,
            [-10.367_187_5, -10.25, -4.],
            [5.367_187_5, 12., 6.984_375],
            None,
        ),
    ] {
        let path = fixture(file);
        assert!(
            saved_preview::extract(&path).unwrap().is_none(),
            "{file} contains a saved thumbnail"
        );
        let scene = formats::load(&path).unwrap_or_else(|error| panic!("{file}: {error}"));
        assert_eq!(
            (scene.triangles.len(), scene.lines.len(), scene.points.len()),
            (triangles, lines, points),
            "{file}"
        );
        let (actual_lower, actual_upper) = bounds(&scene);
        for axis in 0..3 {
            near(actual_lower[axis], lower[axis]);
            near(actual_upper[axis], upper[axis]);
        }
        if let Some(expected) = surface_area {
            near(area(&scene), expected);
        }
    }
}

/// Process-local serial used to keep temporary snapshot directories distinct.
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
/// A test directory removed when its guard leaves scope.
struct Temporary(PathBuf);
impl Temporary {
    /// Create a process-unique temporary directory for headless CLI output.
    fn new() -> Self {
        let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("catwalk-blender-{}-{id}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
/// Fixed transparent 128-pixel canvas used for retained Blender snapshot qualification.
fn options() -> RenderOptions {
    RenderOptions {
        width: 128,
        height: 128,
        transparent: true,
        supersampling: 2,
        ..RenderOptions::default()
    }
}
/// Compare a retained RGBA snapshot within the reviewed two-level channel tolerance.
fn snapshot(image: &image::RgbaImage, path: &Path) {
    let expected = image::open(path).unwrap().to_rgba8();
    assert_eq!(image.dimensions(), expected.dimensions());
    let maximum = image
        .as_raw()
        .iter()
        .zip(expected.as_raw())
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(
        maximum <= 2,
        "Blender geometry snapshot differs by {maximum} levels: {}",
        path.display()
    );
    assert!(image.pixels().filter(|pixel| pixel[3] > 0).count() > 100);
}

#[test]
fn no_thumbnail_previews_match_reviewed_images_and_render_without_a_display() {
    let directory = Temporary::new();
    for name in [
        "default-252",
        "cube-278",
        "shared-278",
        "mesh-280",
        "loose-291",
        "hierarchy-502",
    ] {
        let path = fixture(&format!("{name}-no-thumbnail.blend"));
        let image = geometry_thumbnail(&path, &options()).unwrap();
        snapshot(&image, &fixture(&format!("{name}-geometry.png")));
        assert_eq!(image, catwalk3d::thumbnail(&path, &options()).unwrap());
        let output = directory.0.join(format!("{name}.png"));
        let result = Command::new(env!("CARGO_BIN_EXE_catwalk"))
            .arg(&path)
            .arg("--output")
            .arg(&output)
            .args([
                "--render-geometry",
                "--size",
                "128",
                "--transparent",
                "--samples",
                "2",
            ])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_RUNTIME_DIR")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stderr, [] as [u8; 0]);
        assert_eq!(image, image::open(output).unwrap().to_rgba8());
    }
}

#[test]
fn compressed_geometry_and_geometry_forcing_ignore_saved_thumbnail_pixels() {
    let directory = Temporary::new();
    let source = fixture("hierarchy-502-no-thumbnail.blend");
    let path = directory.0.join("hierarchy-gzip.blend");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&fs::read(&source).unwrap()).unwrap();
    fs::write(&path, encoder.finish().unwrap()).unwrap();
    let expected = geometry_thumbnail(&source, &options()).unwrap();
    assert_eq!(expected, geometry_thumbnail(&path, &options()).unwrap());

    // A real 2.78 source still contains its saved 128x128 thumbnail. Rendering
    // geometry must equal the copy from which only the TEST block was removed.
    let original = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/saved-previews/blend_dwg/blend-cube.blend");
    assert!(saved_preview::extract(&original).unwrap().is_some());
    let rendered = geometry_thumbnail(&original, &options()).unwrap();
    assert_eq!(
        rendered,
        geometry_thumbnail(&fixture("cube-278-no-thumbnail.blend"), &options()).unwrap()
    );
    assert_ne!(
        rendered,
        catwalk3d::thumbnail(&original, &options()).unwrap()
    );
}

#[test]
fn required_modifiers_are_explicitly_rejected() {
    let original = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/saved-previews/blend_dwg/blend-gzip-252.blend");
    let error = formats::load(&original).unwrap_err().to_string();
    assert!(
        error.contains("modifiers") && error.contains("evaluation"),
        "{error}"
    );
}

#[test]
fn real_zstandard_frames_keep_modern_geometry_and_modifier_validation() {
    let original = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/saved-previews/blend_dwg/blend-extras.blend");
    let source = fs::File::open(&original).unwrap();
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(source).unwrap();
    let mut header = [0_u8; 17];
    decoder.read_exact(&mut header).unwrap();
    assert_eq!(&header, b"BLENDER17-01v0502");
    // The public geometry reader must decode all frames and the seek-table trailer
    // before rejecting the original's required, unevaluated modifiers.
    let error = formats::load(&original).unwrap_err().to_string();
    assert!(error.contains("modifiers"), "{error}");
}
