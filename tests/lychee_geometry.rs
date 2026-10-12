//! Independent stored Lychee model bytes, placements and thumbnail dispatch contracts.

#[path = "support/digest_hex.rs"]
/// Format independently pinned original-byte SHA-256 values.
mod digest_hex;

use catwalk3d::{
    formats, geometry_thumbnail,
    render::{RenderOptions, render},
    saved_preview,
    scene::{Scene, Vec3},
    thumbnail,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

/// Unique process-local directory suffix for independent malformed-byte controls.
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
/// Source face order and outward winding of the asymmetric tetrahedron.
const FACES: [[usize; 3]; 4] = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [2, 0, 3]];
/// Literal identity placement raises the actual lowest vertex to the platform.
const IDENTITY_POINTS: [[f64; 3]; 4] =
    [[-3., -2., 0.], [3., -2., 0.], [-1., 2., 0.], [0., -1., 4.]];
/// Independent Python reference for scale (2,3,0.5), then global rotations (30,45,60).
/// XY origin is (11,-7); actual lowest transformed vertex is raised to Z=9.
const POSED_POINTS: [[f64; 3]; 4] = [
    [
        11.578_820_564_920_418,
        -15.389_758_218_505_378,
        17.485_281_374_238_57,
    ],
    [15.821_461_252_039_704, -8.041_288_990_155_843, 9.],
    [
        6.114_354_470_853_158,
        -4.069_881_438_840_801,
        18.899_494_936_611_664,
    ],
    [
        13.458_868_833_850_206,
        -8.937_266_673_330_44,
        15.528_045_730_290_696,
    ],
];
/// Independent second instance: 90-degree global Z rotation and (-9,8,2) XY/lift.
const SECOND_POINTS: [[f64; 3]; 4] = [[-7., 5., 2.], [-7., 11., 2.], [-11., 7., 2.], [-8., 8., 6.]];

/// Original geometry facts established from pinned source STL and scene bytes.
struct Original {
    /// Stable original source filename suffix.
    profile: &'static str,
    /// Literal original STL hash, independent of native reader output.
    stl_hash: &'static str,
    /// Number of original oriented triangles.
    triangles: usize,
    /// Number of independently indexed coordinate scalars.
    scalars: usize,
    /// Exact native member length at immutable original offset 832.
    mesh_bytes: usize,
    /// Recorded original source center; stored native coordinates already subtract it.
    center: [f64; 3],
    /// Literal independently derived Rz Ry Rx matrix, rather than importer rotation helpers.
    matrix: [[f64; 3]; 3],
    /// Literal placement derived from transformed actual vertices and recorded model lift.
    translation: [f64; 3],
    /// Independent world coordinate minimum.
    minimum: [f64; 3],
    /// Independent world coordinate maximum.
    maximum: [f64; 3],
    /// Unchanged original closed-model surface area.
    area: f64,
    /// Unchanged original closed-model signed volume, preserving source winding.
    volume: f64,
}

/// Original Lychee 5.4.3 keyboard cases, pinned at JRiggles/DELPHI-kbd a666598d.
const ORIGINALS: [Original; 2] = [
    Original {
        profile: "lp",
        stl_hash: "59f649fc05a795d1d0705c5447a43803082b9486bc2b0269b38c563adc3a0661",
        triangles: 6360,
        scalars: 9534,
        mesh_bytes: 114_476,
        center: [
            60.415_000_915_527_344,
            -35.805_000_305_175_78,
            1.599_999_904_632_568_4,
        ],
        matrix: [
            [
                -0.413_175_911_166_534_85,
                0.869_607_129_873_848_6,
                0.270_312_978_053_720_6,
            ],
            [
                -0.492_403_876_506_104_07,
                0.036_357_421_172_698_634,
                -0.869_607_129_873_848_6,
            ],
            [
                -0.766_044_443_118_978,
                -0.492_403_876_506_104_07,
                0.413_175_911_166_534_85,
            ],
        ],
        translation: [0., 0., 69.838_566_101_596_84],
        minimum: [-61.663_065_692_531_42, -35.367_789_790_370_594, 2.],
        maximum: [
            61.663_067_397_567_71,
            35.367_789_514_402_16,
            137.677_135_125_421_33,
        ],
        area: 10_990.742_991_058_978,
        volume: 12_567.248_849_375_99,
    },
    Original {
        profile: "hp",
        stl_hash: "6a2da7a8e4ba6dfb00761c706ac6a26de554337cda215552d8a2cb890067303d",
        triangles: 6400,
        scalars: 9594,
        mesh_bytes: 115_196,
        center: [60.415_000_915_527_344, -35.805_000_305_175_78, 2.75],
        matrix: [
            [
                -0.346_578_346_422_015_57,
                0.880_260_984_878_137_5,
                0.324_074_140_117_277_37,
            ],
            [
                -0.360_393_465_245_114_34,
                0.194_011_851_445_656_03,
                -0.912_401_201_066_311_2,
            ],
            [-0.866_025_403_784_438_6, -0.433_012_701_892_219_46, 0.25],
        ],
        translation: [
            1.975_972_611_350_799e-15,
            3.930_768_786_062_684e-15,
            73.120_875_757_603_9,
        ],
        minimum: [-58.823_851_640_298_89, -34.671_271_515_524_3, 2.],
        maximum: [
            58.823_852_962_390_36,
            34.671_271_515_524_32,
            144.241_751_515_207_8,
        ],
        area: 12_984.370_197_686_483,
        volume: 14_560.122_021_961_757,
    },
];

/// Locate a corpus asset without depending on the process working directory.
fn fixture(directory: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats")
        .join(directory)
        .join(name)
}

/// Locate an untouched keyboard scene or its independently retained original STL.
fn original_input(original: &Original, extension: &str) -> PathBuf {
    fixture(
        if extension == "lys" {
            "lychee"
        } else {
            "lychee_geometry"
        },
        &format!("delphi-edge-case-{}.{}", original.profile, extension),
    )
}

/// Decode one independently addressed little-endian original scalar/index.
fn integer(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

/// Decode one little-endian IEEE float32 without passing through the model loader.
fn scalar(bytes: &[u8], offset: usize) -> f32 {
    f32::from_bits(integer(bytes, offset))
}

/// Reproduce the original source's single required float32 rounding, exactly.
#[allow(clippy::cast_possible_truncation)]
fn centered_scalar(value: f32, center: f64) -> f32 {
    (f64::from(value) - center) as f32
}

/// Compare independently derived geometric scalar values by absolute error.
fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} != {expected}"
    );
}

/// Compare every axis with an independently supplied literal coordinate.
fn position(actual: Vec3, expected: [f64; 3]) {
    for (a, e) in [actual.x, actual.y, actual.z].into_iter().zip(expected) {
        close(a, e, 1e-9);
    }
}

/// Check the scene extrema without depending on importer bounds helpers.
fn bounds(scene: &Scene, minimum: [f64; 3], maximum: [f64; 3]) {
    let mut lower = [f64::INFINITY; 3];
    let mut upper = [f64::NEG_INFINITY; 3];
    for point in scene.positions() {
        for (axis, value) in [point.x, point.y, point.z].into_iter().enumerate() {
            lower[axis] = lower[axis].min(value);
            upper[axis] = upper[axis].max(value);
        }
    }
    for (a, e) in lower.into_iter().zip(minimum) {
        close(a, e, 1e-9);
    }
    for (a, e) in upper.into_iter().zip(maximum) {
        close(a, e, 1e-9);
    }
}

/// Measure independently accumulated surface area and signed closed-model volume.
fn area_volume(scene: &Scene) -> (f64, f64) {
    let mut area = 0.;
    let mut volume = 0.;
    for triangle in &scene.triangles {
        let [a, b, c] = triangle.vertices;
        area += (b - a).cross(c - a).length() / 2.;
        volume += a.dot(b.cross(c)) / 6.;
    }
    (area, volume)
}

/// Preserve every authored face corner, source winding and selected occurrence order.
fn authored_triangles(scene: &Scene, occurrences: &[[[f64; 3]; 4]]) {
    assert_eq!(scene.triangles.len(), 4 * occurrences.len());
    assert!(scene.lines.is_empty() && scene.points.is_empty());
    for (triangles, points) in scene.triangles.as_chunks::<4>().0.iter().zip(occurrences) {
        for (triangle, face) in triangles.iter().zip(FACES) {
            for (point, index) in triangle.vertices.into_iter().zip(face) {
                position(point, points[index]);
            }
        }
    }
}

/// Use the same fixed render settings as the reviewed thumbnail corpus.
fn options() -> RenderOptions {
    RenderOptions {
        width: 256,
        height: 256,
        transparent: true,
        ..Default::default()
    }
}

/// Invoke the public binary headlessly and retain its output for exact comparisons.
fn command(input: &Path, options: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_catwalk"));
    command
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR")
        .arg(input)
        .args(["--output", "-"]);
    if !options.contains(&"--extract-saved-preview") {
        command.args(["--size", "256", "--transparent"]);
    }
    command.args(options).output().unwrap()
}

/// Decode successful PNG output and require uncontaminated CLI diagnostics.
fn cli_png(output: &Output) -> image::RgbaImage {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stderr, [] as [u8; 0]);
    assert!(output.stdout.starts_with(b"\x89PNG\r\n\x1a\n"));
    image::load_from_memory(&output.stdout)
        .unwrap()
        .into_rgba8()
}

#[test]
fn original_stls_and_all_centered_native_triangle_corners_remain_bitwise_exact() {
    for original in &ORIGINALS {
        let stl = fs::read(original_input(original, "stl")).unwrap();
        assert_eq!(stl.len(), 84 + 50 * original.triangles);
        assert_eq!(
            usize::try_from(integer(&stl, 80)).unwrap(),
            original.triangles
        );
        assert_eq!(
            format!("{:x}", digest_hex::Hex(Sha256::digest(&stl))),
            original.stl_hash
        );
        let lys = fs::read(original_input(original, "lys")).unwrap();
        // Immutable original absolute geometry span established from member hashes.
        let mesh = &lys[832..832 + original.mesh_bytes];
        assert_eq!(integer(mesh, 0), 2);
        assert_eq!(integer(mesh, 4), 12);
        assert_eq!(
            usize::try_from(integer(mesh, 8)).unwrap(),
            3 * original.triangles
        );
        assert_eq!(
            usize::try_from(integer(mesh, 12)).unwrap(),
            original.scalars
        );
        assert_eq!(integer(mesh, 16), 0);
        let coordinates = 20 + original.triangles * 12;
        for triangle in 0..original.triangles {
            for corner in 0..3 {
                let index =
                    usize::try_from(integer(mesh, 20 + (triangle * 3 + corner) * 4)).unwrap();
                for axis in 0..3 {
                    let expected = centered_scalar(
                        scalar(&stl, 84 + triangle * 50 + 12 + corner * 12 + axis * 4),
                        original.center[axis],
                    );
                    let actual = scalar(mesh, coordinates + (index * 3 + axis) * 4);
                    assert_eq!(
                        actual.to_bits(),
                        expected.to_bits(),
                        "{} triangle {triangle}, corner {corner}, axis {axis}",
                        original.profile
                    );
                }
            }
        }
    }
}

#[test]
fn original_protocol_notice_is_retained_byte_for_byte() {
    let notice = fs::read(fixture("lychee_geometry", "DF-PLUGIN-LYS-LICENSE")).unwrap();
    assert_eq!(notice.len(), 1076);
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&notice))),
        "6b7df03f394de02ccf135e7ea137402e5e2e2d4c499800d73c806c563371ed9b"
    );
    let notice = std::str::from_utf8(&notice).unwrap();
    assert!(notice.contains("Copyright (c) 2026 Open Resin Alliance"));
    assert!(notice.contains("The above copyright notice and this permission notice"));
}

#[test]
fn both_original_fresh_models_match_every_stl_corner_and_literal_world_bounds() {
    assert!(formats::NATIVE_GEOMETRY_EXTENSIONS.contains(&"lys"));
    for original in &ORIGINALS {
        let stl = fs::read(original_input(original, "stl")).unwrap();
        let scene = formats::load(&original_input(original, "lys")).unwrap();
        let direct = formats::lychee::load(&original_input(original, "lys")).unwrap();
        assert_eq!(scene.triangles.len(), original.triangles);
        assert_eq!(direct.triangles.len(), original.triangles);
        assert!(scene.lines.is_empty() && scene.points.is_empty());
        for (triangle, emitted) in scene.triangles.iter().enumerate() {
            for corner in 0..3 {
                let local = std::array::from_fn::<_, 3, _>(|axis| {
                    f64::from(centered_scalar(
                        scalar(&stl, 84 + triangle * 50 + 12 + corner * 12 + axis * 4),
                        original.center[axis],
                    ))
                });
                let expected = std::array::from_fn(|axis| {
                    original.matrix[axis]
                        .into_iter()
                        .zip(local)
                        .map(|(a, b)| a * b)
                        .sum::<f64>()
                        + original.translation[axis]
                });
                position(emitted.vertices[corner], expected);
                position(direct.triangles[triangle].vertices[corner], expected);
            }
        }
        bounds(&scene, original.minimum, original.maximum);
        let (area, volume) = area_volume(&scene);
        close(area, original.area, 1e-6);
        close(volume, original.volume, 1e-5);
    }
}

#[test]
fn absent_saved_preview_falls_back_to_the_exact_native_model_in_api_and_cli() {
    let input = fixture("lychee_geometry", "missing-preview.lys");
    assert!(saved_preview::extract(&input).unwrap().is_none());
    let scene = formats::load(&input).unwrap();
    authored_triangles(&scene, &[IDENTITY_POINTS]);
    bounds(&scene, [-3., -2., 0.], [3., 2., 4.]);
    let (area, volume) = area_volume(&scene);
    close(area, 46.616_267_642_812_57, 1e-10);
    close(volume, 16., 1e-10);
    let expected = render(&scene, &options()).unwrap();
    assert!(expected.pixels().any(|pixel| pixel[3] != 0));
    assert_eq!(thumbnail(&input, &options()).unwrap(), expected);
    assert_eq!(geometry_thumbnail(&input, &options()).unwrap(), expected);
    assert_eq!(cli_png(&command(&input, &[])), expected);
    assert_eq!(cli_png(&command(&input, &["--render-geometry"])), expected);
    let extracted = command(&input, &["--extract-saved-preview"]);
    assert_eq!(extracted.status.code(), Some(3));
    assert_eq!(extracted.stdout, [] as [u8; 0]);
}

#[test]
fn nonuniform_scale_noncommuting_euler_order_and_actual_vertex_lift_match_literals() {
    let scene = formats::load(&fixture("lychee_geometry", "asymmetric-placement.lys")).unwrap();
    authored_triangles(&scene, &[POSED_POINTS]);
    bounds(
        &scene,
        [6.114_354_470_853_158, -15.389_758_218_505_378, 9.],
        [
            15.821_461_252_039_704,
            -4.069_881_438_840_801,
            18.899_494_936_611_664,
        ],
    );
    let (area, volume) = area_volume(&scene);
    close(area, 154.190_948_844_983_35, 1e-10);
    close(volume, 48., 1e-10);
}

#[test]
fn shared_mesh_occurrences_are_placed_independently_and_hidden_models_are_excluded() {
    let scene = formats::load(&fixture("lychee_geometry", "shared-visibility.lys")).unwrap();
    authored_triangles(&scene, &[POSED_POINTS, SECOND_POINTS]);
    bounds(
        &scene,
        [-11., -15.389_758_218_505_378, 2.],
        [15.821_461_252_039_704, 11., 18.899_494_936_611_664],
    );
    let (area, volume) = area_volume(&scene);
    close(area, 200.807_216_487_795_92, 1e-10);
    close(volume, 64., 1e-10);
}

#[test]
fn preparation_metadata_and_derived_meshes_leave_original_models_unchanged() {
    let input = fixture("lychee_geometry", "preparation-ignored.lys");
    let before = fs::read(&input).unwrap();
    let scene = formats::load(&input).unwrap();
    authored_triangles(&scene, &[POSED_POINTS]);
    let clean = formats::load(&fixture("lychee_geometry", "asymmetric-placement.lys")).unwrap();
    let native = geometry_thumbnail(&input, &options()).unwrap();
    assert_eq!(native, render(&clean, &options()).unwrap());
    let saved = saved_preview::extract(&input).unwrap().unwrap();
    let authored = image::open(fixture("lychee_geometry", "authored-preview.png"))
        .unwrap()
        .into_rgba8();
    assert_eq!(saved, authored);
    assert_eq!(saved.dimensions(), (16, 8));
    let automatic = thumbnail(&input, &options()).unwrap();
    assert_eq!(automatic.get_pixel(32, 128).0, [218, 60, 39, 255]);
    assert_eq!(automatic.get_pixel(224, 128).0, [40, 171, 94, 255]);
    assert_eq!(automatic.get_pixel(128, 0)[3], 0);
    assert_ne!(automatic, native);
    assert_eq!(cli_png(&command(&input, &[])), automatic);
    assert_eq!(cli_png(&command(&input, &["--render-geometry"])), native);
    assert_eq!(
        cli_png(&command(&input, &["--extract-saved-preview"])),
        authored
    );
    assert_eq!(fs::read(input).unwrap(), before);
}

#[test]
fn original_saved_images_remain_preferred_and_forced_geometry_is_a_fresh_model_render() {
    for original in &ORIGINALS {
        let input = original_input(original, "lys");
        let before = fs::read(&input).unwrap();
        let native = geometry_thumbnail(&input, &options()).unwrap();
        let saved = thumbnail(&input, &options()).unwrap();
        let baseline = image::open(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/snapshots/formats")
                .join(format!(
                    "lychee-delphi-{}-saved-thumbnail.png",
                    original.profile
                )),
        )
        .unwrap()
        .into_rgba8();
        assert_eq!(saved, baseline);
        assert_ne!(native, saved);
        assert_eq!(
            native,
            render(&formats::load(&input).unwrap(), &options()).unwrap()
        );
        assert_eq!(cli_png(&command(&input, &["--render-geometry"])), native);
        assert_eq!(cli_png(&command(&input, &[])), saved);
        assert_eq!(fs::read(input).unwrap(), before);
    }
}

/// Temporary owner that removes only its uniquely created malformed-input directory.
struct Control(PathBuf);

impl Control {
    /// Write a bounded diagnostic container without modifying any corpus input.
    fn new(bytes: &[u8]) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "catwalk-lychee-native-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("control.lys"), bytes).unwrap();
        Self(directory)
    }

    /// Load the private diagnostic through the public dispatch API.
    fn load(&self) -> catwalk3d::Result<Scene> {
        formats::load(&self.0.join("control.lys"))
    }

    /// Address the uniquely owned input for public thumbnail and binary calls.
    fn path(&self) -> PathBuf {
        self.0.join("control.lys")
    }
}

/// Replace only the addressed authored PNG and independently reframe every member span.
/// Preserve exact source scene/mesh bytes and declare one whole-member SHA-256.
fn preview_variant(png: &[u8], hash: &str) -> Control {
    let source = fs::read(fixture("lychee_geometry", "preparation-ignored.lys")).unwrap();
    let json_count = usize::try_from(integer(&source, 12)).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&source[16..16 + json_count]).unwrap();
    let data_start = 8 + usize::try_from(integer(&source, 4)).unwrap();
    let members = manifest["mangoFiles"].as_object_mut().unwrap();
    assert!(members.contains_key("preview.png"));
    let mut data = Vec::new();
    for (name, metadata) in members.iter_mut() {
        let start = data_start
            + metadata["offset"]
                .as_str()
                .unwrap()
                .parse::<usize>()
                .unwrap();
        let size = usize::try_from(metadata["size"].as_u64().unwrap()).unwrap();
        let value = if name == "preview.png" {
            png
        } else {
            &source[start..start + size]
        };
        metadata["offset"] = serde_json::json!(data.len().to_string());
        metadata["size"] = serde_json::json!(value.len());
        if name == "preview.png" {
            metadata["integrity"] = serde_json::json!({"algorithm":"SHA256","hash":hash});
        }
        data.extend_from_slice(value);
    }
    let json = serde_json::to_vec(&manifest).unwrap();
    let padded = (json.len() + 3) & !3;
    let count = u32::try_from(json.len()).unwrap();
    let padded_count = u32::try_from(padded).unwrap();
    let mut bytes = Vec::new();
    for word in [4, padded_count + 8, padded_count + 4, count] {
        bytes.extend(word.to_le_bytes());
    }
    bytes.extend(json);
    bytes.resize(16 + padded, 0);
    bytes.extend(data);
    Control::new(&bytes)
}

/// Assert native fallback and extraction-only failure against literal model placements.
fn unusable_png_falls_back(control: &Control) {
    let input = control.path();
    let before = fs::read(&input).unwrap();
    let scene = control.load().unwrap();
    authored_triangles(&scene, &[POSED_POINTS]);
    let clean = formats::load(&fixture("lychee_geometry", "asymmetric-placement.lys")).unwrap();
    let expected = render(&clean, &options()).unwrap();
    assert!(saved_preview::extract(&input).is_err());
    assert_eq!(thumbnail(&input, &options()).unwrap(), expected);
    assert_eq!(geometry_thumbnail(&input, &options()).unwrap(), expected);
    assert_eq!(cli_png(&command(&input, &[])), expected);
    assert_eq!(cli_png(&command(&input, &["--render-geometry"])), expected);
    let extracted = command(&input, &["--extract-saved-preview"]);
    assert_eq!(extracted.status.code(), Some(1));
    assert_eq!(extracted.stdout, [] as [u8; 0]);
    assert_ne!(extracted.stderr, [] as [u8; 0]);
    assert_eq!(fs::read(input).unwrap(), before);
}

#[test]
fn corrupt_selected_png_with_correct_integrity_falls_back_to_stored_models() {
    let mut corrupt = fs::read(fixture("lychee_geometry", "authored-preview.png")).unwrap();
    // Keep the PNG carrier and dimensions valid while corrupting its compressed body.
    let at = corrupt
        .windows(4)
        .position(|value| value == b"IDAT")
        .unwrap()
        + 4;
    corrupt[at] ^= 0xff;
    let hash = format!("{:x}", digest_hex::Hex(Sha256::digest(&corrupt)));
    let control = preview_variant(&corrupt, &hash);
    let error = saved_preview::extract(&control.path())
        .unwrap_err()
        .to_string();
    assert!(!error.contains("integrity"));
    unusable_png_falls_back(&control);
}

#[test]
fn incorrect_selected_png_integrity_falls_back_to_stored_models() {
    let png = fs::read(fixture("lychee_geometry", "authored-preview.png")).unwrap();
    let control = preview_variant(&png, &"00".repeat(32));
    let error = saved_preview::extract(&control.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("SHA-256 integrity"));
    unusable_png_falls_back(&control);
}

#[test]
fn usable_saved_png_with_valid_integrity_remains_preferred_over_native_models() {
    let png = fs::read(fixture("lychee_geometry", "authored-preview.png")).unwrap();
    let hash = format!("{:x}", digest_hex::Hex(Sha256::digest(&png)));
    let control = preview_variant(&png, &hash);
    let input = control.path();
    authored_triangles(&control.load().unwrap(), &[POSED_POINTS]);
    assert_eq!(
        saved_preview::extract(&input).unwrap().unwrap(),
        image::load_from_memory(&png).unwrap().into_rgba8()
    );
    let automatic = thumbnail(&input, &options()).unwrap();
    assert_eq!(automatic.get_pixel(32, 128).0, [218, 60, 39, 255]);
    assert_eq!(automatic.get_pixel(224, 128).0, [40, 171, 94, 255]);
    assert_ne!(automatic, geometry_thumbnail(&input, &options()).unwrap());
    assert_eq!(cli_png(&command(&input, &[])), automatic);
    assert_eq!(
        cli_png(&command(&input, &["--extract-saved-preview"])),
        image::load_from_memory(&png).unwrap().into_rgba8()
    );
}

impl Drop for Control {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Independently address the authored source member from the neutral manifest.
fn authored_mesh_span(bytes: &[u8]) -> (usize, usize) {
    let count = usize::try_from(integer(bytes, 12)).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&bytes[16..16 + count]).unwrap();
    let member = &manifest["mangoFiles"]["asymmetric-model.bin"];
    let start = 8
        + usize::try_from(integer(bytes, 4)).unwrap()
        + member["offset"].as_str().unwrap().parse::<usize>().unwrap();
    (
        start,
        usize::try_from(member["size"].as_u64().unwrap()).unwrap(),
    )
}

#[test]
fn invalid_addressed_source_mesh_is_rejected_without_using_a_prepared_resource() {
    let original = fs::read(fixture("lychee_geometry", "preparation-ignored.lys")).unwrap();
    let (start, size) = authored_mesh_span(&original);
    assert_eq!(size, 116);
    for (relative, value) in [
        (0, 3_u32),               // Unsupported source mesh version.
        (4, 16),                  // Unqualified header framing.
        (8, 11),                  // Non-triangle scalar count.
        (12, 13),                 // Non-XYZ coordinate count.
        (16, 1),                  // Unsupported reserved header value.
        (20, 4),                  // Source triangle index beyond all four vertices.
        (68, f32::NAN.to_bits()), // Non-finite selected coordinate.
    ] {
        let mut bytes = original.clone();
        bytes[start + relative..start + relative + 4].copy_from_slice(&value.to_le_bytes());
        assert!(
            Control::new(&bytes).load().is_err(),
            "invalid source word at {relative}"
        );
    }
}

#[test]
fn exact_scene_hash_requires_its_member_and_does_not_fall_back_to_another_mesh() {
    let mut bytes = fs::read(fixture("lychee_geometry", "missing-preview.lys")).unwrap();
    let manifest_end = 16 + usize::try_from(integer(&bytes, 12)).unwrap();
    let name = b"asymmetric-model.bin";
    let at = bytes[16..manifest_end]
        .windows(name.len())
        .position(|value| value == name)
        .unwrap()
        + 16;
    // Rename only the addressed member, keeping the qualified scene reference unchanged.
    bytes[at] = b'x';
    let control = Control::new(&bytes);
    assert!(control.load().is_err());
    assert!(
        saved_preview::extract(&control.0.join("control.lys"))
            .unwrap()
            .is_none()
    );
}
