//! Native authored Fusion bodies render without consuming a saved preview.

use catwalk3d::render::RenderOptions;
use catwalk3d::scene::Scene;
use catwalk3d::{formats, geometry_thumbnail, saved_preview, thumbnail};
use std::{
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

/// Distinct temporary archives for parallel extraction and CLI controls.
static NEXT_ARCHIVE: AtomicU64 = AtomicU64::new(0);

/// Locate the immutable, independently authored native Fusion body fixture.
fn authored() -> PathBuf {
    fixture("fusion-toy-block.f3d")
}

/// Locate an unmodified, externally authored native source.
fn fixture(filename: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/autodesk_geometry")
        .join(filename)
}

/// Remove every PNG preview while retaining all manifest, Design and ASM bytes.
fn without_previews() -> PathBuf {
    rewrite_previews(None)
}

/// Alter only stored images, retaining every native geometry and placement byte.
fn rewrite_previews(replacement: Option<&[u8]>) -> PathBuf {
    rewrite_source_previews(&authored(), replacement)
}

/// Copy compressed native members verbatim, including Fusion ZIP Zstd members.
fn rewrite_source_previews(path: &Path, replacement: Option<&[u8]>) -> PathBuf {
    let input = fs::read(path).expect("read authored Fusion design");
    let mut source = ZipArchive::new(Cursor::new(input)).expect("open authored Fusion ZIP");
    let mut target = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for index in 0..source.len() {
        let entry = source
            .by_index_raw(index)
            .expect("read Fusion archive member");
        let name = entry.name().expect("valid Fusion member name").into_owned();
        if name.to_ascii_lowercase().ends_with(".png") {
            if let Some(bytes) = replacement {
                target
                    .start_file(&name, options)
                    .expect("copy preview name");
                target.write_all(bytes).expect("replace preview bytes");
            }
            continue;
        }
        target
            .raw_copy_file(entry)
            .expect("copy unchanged compressed native member");
    }
    let bytes = target
        .finish()
        .expect("finish preview-free Fusion archive")
        .into_inner();
    let suffix = NEXT_ARCHIVE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "catwalk-fusion-geometry-{}-{suffix}.f3d",
        std::process::id()
    ));
    fs::write(&path, bytes).expect("write preview-free native design");
    path
}

/// Canonical oriented triangles independent of importer face iteration order.
fn geometry(scene: &Scene) -> Vec<[[u64; 3]; 3]> {
    let mut triangles: Vec<_> = scene
        .triangles
        .iter()
        .map(|triangle| {
            triangle
                .vertices
                .map(|point| [point.x.to_bits(), point.y.to_bits(), point.z.to_bits()])
        })
        .collect();
    triangles.sort_unstable();
    triangles
}

/// Small transparent canvases reveal whether a geometry render is nonempty.
fn options() -> RenderOptions {
    RenderOptions {
        width: 96,
        height: 96,
        supersampling: 1,
        transparent: true,
        ..RenderOptions::default()
    }
}

#[test]
fn authored_fusion_body_is_real_three_dimensional_geometry() {
    let scene = formats::load(&authored()).expect("load native ASM bodies");
    scene.validate().expect("valid finite scene");
    assert!(
        scene.triangles.len() > 67,
        "curved native faces must be tessellated"
    );
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    let mut area = 0.;
    let mut volume = 0.;
    for triangle in &scene.triangles {
        let [a, b, c] = triangle.vertices;
        area += (b - a).cross(c - a).length() / 2.;
        volume += a.dot(b.cross(c)) / 6.;
        for point in triangle.vertices {
            for (axis, value) in [point.x, point.y, point.z].into_iter().enumerate() {
                minimum[axis] = minimum[axis].min(value);
                maximum[axis] = maximum[axis].max(value);
            }
        }
    }
    assert!(
        minimum
            .iter()
            .zip(maximum)
            .all(|(low, high)| high - low > 1.)
    );
    assert!(area.is_finite() && area > 100.);
    assert!(volume.is_finite() && volume.abs() > 100.);
}

#[test]
fn removing_all_saved_images_keeps_native_fusion_geometry_identical() {
    let original = formats::load(&authored()).expect("authored geometry");
    let path = without_previews();
    let no_preview = formats::load(&path).expect("native design has no image dependency");
    assert_eq!(geometry(&original), geometry(&no_preview));
    assert!(
        saved_preview::extract(&path)
            .expect("valid image-free Fusion archive")
            .is_none()
    );
    fs::remove_file(path).expect("remove temporary source");
}

#[test]
fn ordinary_thumbnail_falls_back_to_fusion_geometry_when_image_is_absent() {
    let path = without_previews();
    let expected = geometry_thumbnail(&path, &options()).expect("fresh native render");
    let actual = thumbnail(&path, &options()).expect("automatic native fallback");
    assert_eq!(actual, expected);
    assert!(actual.pixels().filter(|pixel| pixel[3] != 0).count() > 100);
    fs::remove_file(path).expect("remove temporary source");
}

#[test]
fn corrupt_saved_images_do_not_block_the_independent_geometry_reader() {
    let path = rewrite_previews(Some(b"this is deliberately not a PNG"));
    assert!(saved_preview::extract(&path).is_err());
    let expected = geometry_thumbnail(&path, &options()).expect("fresh native render");
    let actual = thumbnail(&path, &options()).expect("fallback past corrupt saved image");
    assert_eq!(actual, expected);
    assert!(actual.pixels().filter(|pixel| pixel[3] != 0).count() > 100);
    fs::remove_file(path).expect("remove temporary source");
}

#[test]
fn oversized_native_fusion_source_is_refused_before_payload_decoding() {
    let suffix = NEXT_ARCHIVE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "catwalk-fusion-oversized-{}-{suffix}.f3d",
        std::process::id()
    ));
    fs::File::create(&path)
        .expect("create sparse oversized source")
        .set_len(256 * 1024 * 1024 + 1)
        .expect("set source beyond native service input ceiling");
    let error = formats::load(&path).expect_err("reject source above input ceiling");
    assert!(error.to_string().contains("resource limit"), "{error}");
    fs::remove_file(path).expect("remove sparse source");
}

#[test]
fn cli_forced_fusion_geometry_renders_without_any_preview() {
    let path = without_previews();
    let output = Command::new(env!("CARGO_BIN_EXE_catwalk"))
        .arg(&path)
        .args([
            "--render-geometry",
            "--size",
            "96",
            "--samples",
            "1",
            "--transparent",
            "--output",
            "-",
        ])
        .output()
        .expect("run Catwalk geometry CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let image = image::load_from_memory(&output.stdout)
        .expect("actual PNG output")
        .to_rgba8();
    assert_eq!(image.dimensions(), (96, 96));
    assert!(image.pixels().filter(|pixel| pixel[3] != 0).count() > 100);
    fs::remove_file(path).expect("remove temporary source");
}

#[test]
fn saved_image_only_fusion_carrier_cannot_masquerade_as_geometry() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/saved_previews/fusion-authored-deflate-preview.f3d");
    assert!(formats::load(&path).is_err());
    assert!(geometry_thumbnail(&path, &options()).is_err());
}

/// Intersect a vertical line with physical triangles, independently of rendering.
fn vertical_hits(scene: &Scene, point_x: f64, point_y: f64) -> Vec<f64> {
    let mut heights = Vec::new();
    for triangle in &scene.triangles {
        let [first_vertex, second_vertex, third_vertex] = triangle.vertices;
        let determinant = (second_vertex.y - third_vertex.y) * (first_vertex.x - third_vertex.x)
            + (third_vertex.x - second_vertex.x) * (first_vertex.y - third_vertex.y);
        if determinant.abs() < 1e-10 {
            continue;
        }
        let first_weight = ((second_vertex.y - third_vertex.y) * (point_x - third_vertex.x)
            + (third_vertex.x - second_vertex.x) * (point_y - third_vertex.y))
            / determinant;
        let second_weight = ((third_vertex.y - first_vertex.y) * (point_x - third_vertex.x)
            + (first_vertex.x - third_vertex.x) * (point_y - third_vertex.y))
            / determinant;
        let third_weight = 1. - first_weight - second_weight;
        if [first_weight, second_weight, third_weight]
            .into_iter()
            .all(|value| value >= -1e-9)
        {
            heights.push(
                first_weight * first_vertex.z
                    + second_weight * second_vertex.z
                    + third_weight * third_vertex.z,
            );
        }
    }
    heights.sort_by(f64::total_cmp);
    heights.dedup_by(|left, right| (*left - *right).abs() < 1e-8);
    heights
}

#[test]
fn authored_hex_nut_preserves_analytic_dimensions_and_the_through_bore() {
    let original = fixture("fusion-hex-nut.f3d");
    let path = rewrite_source_previews(&original, None);
    let scene = formats::load(&path).expect("image-free authored hex nut B-rep");
    scene.validate().expect("finite real triangles");
    assert_eq!(
        geometry(&scene),
        geometry(&formats::load(&original).unwrap())
    );
    assert!(saved_preview::extract(&path).unwrap().is_none());
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for point in scene
        .triangles
        .iter()
        .flat_map(|triangle| triangle.vertices)
    {
        for (axis, value) in [point.x, point.y, point.z].into_iter().enumerate() {
            low[axis] = low[axis].min(value);
            high[axis] = high[axis].max(value);
        }
    }
    // Native analytic side planes specify 20 mm across flats and 10 mm height.
    // The external author provides no independent STEP/STL export; these are
    // source-carrier dimensional controls, not an interchange-volume oracle.
    for (actual, expected) in low.into_iter().zip([-20. / 3_f64.sqrt(), -10., 0.]) {
        assert!((actual - expected).abs() < 1e-7, "{actual} != {expected}");
    }
    for (actual, expected) in high.into_iter().zip([20. / 3_f64.sqrt(), 10., 10.]) {
        assert!((actual - expected).abs() < 1e-7, "{actual} != {expected}");
    }
    // Read from the source's native analytic cylindrical bore carrier.
    assert_eq!(
        vertical_hits(&scene, -0.721_901_473_896_551_2, 0.988_744_536_748_456),
        [] as [f64; 0],
    );
    let material = vertical_hits(&scene, 9., 0.);
    assert_eq!(
        material.len(),
        2,
        "solid material must have both cap boundaries"
    );
    assert!(material[0].abs() < 1e-7);
    assert!((material[1] - 10.).abs() < 1e-7);
    let render = geometry_thumbnail(&path, &options()).expect("real image-free hex nut render");
    assert!(render.pixels().filter(|pixel| pixel[3] != 0).count() > 100);
    fs::remove_file(path).unwrap();
}

#[test]
fn native_only_keylock_assembly_cannot_claim_qualified_final_placements() {
    let original = fixture("fusion-keylock.f3d");
    let error = formats::load(&original).expect_err("refuse unresolved native assembly placement");
    assert!(
        error
            .to_string()
            .contains("Fusion native placement is incomplete"),
        "{error}"
    );
    assert!(
        thumbnail(&original, &options()).is_ok(),
        "ordinary saved-preview thumbnail remains usable"
    );
    let path = rewrite_source_previews(&original, None);
    assert!(saved_preview::extract(&path).unwrap().is_none());
    let error = thumbnail(&path, &options())
        .expect_err("missing preview cannot render unqualified assembly");
    assert!(
        error
            .to_string()
            .contains("Fusion native placement is incomplete"),
        "{error}"
    );
    fs::remove_file(path).unwrap();
}
