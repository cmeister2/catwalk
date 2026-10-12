//! Original `FreeCAD` native geometry, typed ownership and saved-image dispatch contracts.

#[path = "support/digest_hex.rs"]
/// Display independently pinned original-byte SHA-256 values.
mod digest_hex;
#[path = "support/freecad_fixture.rs"]
/// Author bounded ZIP/XML controls without invoking production `FreeCAD` parsing.
mod freecad_fixture;

use catwalk3d::{
    formats, geometry_thumbnail,
    render::{RenderOptions, render},
    saved_preview,
    scene::{AlphaMode, Scene, Vec3},
    thumbnail,
};
use freecad_fixture::{Control, Member, fixture, original_member, original_members, stored};
use image::RgbaImage;
use sha2::{Digest, Sha256};
use std::{
    fmt::Write as _,
    fs,
    path::Path,
    process::{Command, Output},
};

/// Exact original source hashes from F3D commit efb898d, checked against Git LFS objects.
const ORIGINALS: [(&str, usize, &str); 26] = [
    (
        "arch_floor.FCStd",
        6030,
        "4207b1ddba0fe46ce17c885d153c2b11ec07737fec6e2c0cc0c184737858a5c6",
    ),
    (
        "bracket_1x.FCStd",
        9743,
        "42ba1f1d5dcb8f476cd09021120d8fae818ea6c4162d15e85006738d1e2c3fa4",
    ),
    (
        "broken_links.FCStd",
        8524,
        "c92bb55e07a11f963647a071b5a827fbaa39f99e69372cfb23dfb2ffbe89a93b",
    ),
    (
        "colored_visibility.FCStd",
        7282,
        "f95408d17f86c20b740839ca5503d74762785e63c7d3559827c7ed5a4b30070e",
    ),
    (
        "corrupt_brp.FCStd",
        5325,
        "77217f79eff5c1c03f1ddf66344ea3ec148d26bb0c4b0ddc20f327357e94e866",
    ),
    (
        "cyclic_groups.FCStd",
        6600,
        "40bdabb89a03ae5a839440428ad1d2cb7b825b7fd18bebdfd872070cfe0f449a",
    ),
    (
        "draft_wireframe.FCStd",
        7332,
        "f99cf3e1822495059817fe4aef2d862112505338a6d904a53593454ee969715f",
    ),
    (
        "face_colors.FCStd",
        5647,
        "20a1f2e604abd022e497fe44854b6e60443ede69f5e7156be1a854583b177567",
    ),
    (
        "gui_without_view_providers.FCStd",
        2966,
        "353b1ffcc41d8528ad30290ab8338b3541d659381f90ec8cb72e3dcc577efd52",
    ),
    (
        "headless.FCStd",
        3158,
        "fd15ed96ae0becbcc7c3ebd8df7bdb0359bd5167173f7c7c2650d188a2b830f3",
    ),
    (
        "hidden_container.FCStd",
        4362,
        "38942232df7288c214b87d6ddc28bae9d41836a98b5caa0b29a4b2ffcd0ca00a",
    ),
    (
        "invalid_content.FCStd",
        190,
        "3c4f15612918060ed6231cd69173ba87892a1b23337c1781d879b2693ad3edc2",
    ),
    (
        "legacy_visibility.FCStd",
        6671,
        "cbf92504cc84fa8a0b9bf8ae8067c4a35d23f0ae4ae6f5d6c5017612084378c0",
    ),
    (
        "link.FCStd",
        6734,
        "2cf08804b1102ab29100feaa53723ed9db4af14f740700db8beb09c22ff632f5",
    ),
    (
        "link_array.FCStd",
        4454,
        "ef1bfd1c07a557be5da555ca273f1db5a66de2b1c5bfe64eb39f603594d0dea0",
    ),
    (
        "link_array_transform.FCStd",
        4529,
        "1bbc816a9763b87cfef23f35de333d80ddd3d951117200485200f7b06931ef61",
    ),
    (
        "link_external.FCStd",
        3035,
        "8aa6aa78fc22620c9a9d21c9660e76842d6309b7d6bdd5a5a56e978f8cc8370b",
    ),
    (
        "link_transform.FCStd",
        3165,
        "eed63e674233e45293629ec844bcdd9ac6e1dec372cae25bee39d4799ef31017",
    ),
    (
        "malformed_elements.FCStd",
        6915,
        "2025b55f4185100749840811336dad19f01a6f62334c7fa58cd3e29e43aa6f6c",
    ),
    (
        "malformed_gui.FCStd",
        5235,
        "e6083d149b901e2bf519c73876eb42b4516ffa2a5dce1f3c859bc0123f20fc73",
    ),
    (
        "malformed_no_objectdata.FCStd",
        6505,
        "1af013a5e748be7ae68d50a93df19709d7dd1ef0b9f5a6812a9ae6d1b65a3251",
    ),
    (
        "malformed_no_objects.FCStd",
        6506,
        "d05c0a8c3bbc2420b2d3ac833f7b670cb21e8724bfdced77ebfb348f1e236a90",
    ),
    (
        "no_version.FCStd",
        5784,
        "bd0b33ff3cb7464b0ae394d1a1f711a7ff1cf9b629a37f32a992fe9136a800dc",
    ),
    (
        "part_container.FCStd",
        7174,
        "65c19806aa367049bffafbfd5705721a8ba476d851be22817cd101b8caeb882b",
    ),
    (
        "partdesign_body.FCStd",
        9733,
        "fb2f45af0a9fb488ee315b0995a46ff834c0f4a395bf3caf2451ef3f6aea4158",
    ),
    (
        "transparency.FCStd",
        8771,
        "8d989b597a447ed232bbaadd63eef7160cc7213068a350e0d7fca9d9aefff6af",
    ),
];

/// Independent native box facts, before and after the documented occurrence placement.
struct BoxCase {
    /// Exact original fixture name.
    name: &'static str,
    /// Minimum and maximum world coordinates, independently calculated from native vertices.
    bounds: [[f64; 3]; 2],
    /// Number of complete visible box occurrences, without hidden or aggregate duplicates.
    boxes: usize,
    /// Literal total planar surface area, invariant under the original rigid placements.
    area: f64,
    /// Literal signed volume, retaining original outward winding.
    volume: f64,
    /// Every independently placed original corner, rather than importer-derived bounds.
    corners: &'static [[f64; 3]],
}

/// Literal original BREP-vertex oracles and elementary box area/volume controls.
const BOXES: [BoxCase; 8] = [
    BoxCase {
        name: "colored_visibility.FCStd",
        bounds: [[0.0, 0.0, 0.0], [10.0, 5.0, 3.0]],
        boxes: 1,
        area: 190.0,
        volume: 150.0,
        corners: &[
            [0.0, 0.0, 3.0],
            [0.0, 0.0, 0.0],
            [0.0, 5.0, 3.0],
            [0.0, 5.0, 0.0],
            [10.0, 0.0, 3.0],
            [10.0, 0.0, 0.0],
            [10.0, 5.0, 3.0],
            [10.0, 5.0, 0.0],
        ],
    },
    BoxCase {
        name: "part_container.FCStd",
        bounds: [[3.0, -10.0, 10.0], [13.0, 2.220_446_049_250_313e-15, 20.0]],
        boxes: 1,
        area: 600.0,
        volume: 1000.0,
        corners: &[
            [3.0, -10.0, 10.000_000_000_000_002],
            [3.0, 0.0, 10.0],
            [3.0, -9.999_999_999_999_998, 20.0],
            [3.0, 2.220_446_049_250_313e-15, 20.0],
            [13.0, -10.0, 10.000_000_000_000_002],
            [13.0, 0.0, 10.0],
            [13.0, -9.999_999_999_999_998, 20.0],
            [13.0, 2.220_446_049_250_313e-15, 20.0],
        ],
    },
    BoxCase {
        name: "link.FCStd",
        bounds: [[0.0, 20.0, 0.0], [2.0, 28.0, 2.0]],
        boxes: 1,
        area: 72.0,
        volume: 32.0,
        corners: &[
            [0.0, 20.0, 2.0],
            [0.0, 20.0, 0.0],
            [0.0, 28.0, 2.0],
            [0.0, 28.0, 0.0],
            [2.0, 20.0, 2.0],
            [2.0, 20.0, 0.0],
            [2.0, 28.0, 2.0],
            [2.0, 28.0, 0.0],
        ],
    },
    BoxCase {
        name: "link_transform.FCStd",
        bounds: [
            [1.000_000_000_000_000_9, 20.0, 0.0],
            [6.732_050_807_568_878, 27.928_203_230_275_51, 2.0],
        ],
        boxes: 1,
        area: 72.0,
        volume: 32.0,
        corners: &[
            [5.0, 20.0, 2.0],
            [5.0, 20.0, 0.0],
            [1.000_000_000_000_000_9, 26.928_203_230_275_51, 2.0],
            [1.000_000_000_000_000_9, 26.928_203_230_275_51, 0.0],
            [6.732_050_807_568_878, 21.0, 2.0],
            [6.732_050_807_568_878, 21.0, 0.0],
            [2.732_050_807_568_878_5, 27.928_203_230_275_51, 2.0],
            [2.732_050_807_568_878_5, 27.928_203_230_275_51, 0.0],
        ],
    },
    BoxCase {
        name: "link_array.FCStd",
        bounds: [[0.0, 0.0, 0.0], [22.0, 12.0, 3.0]],
        boxes: 3,
        area: 396.0,
        volume: 216.0,
        corners: &[
            [0.0, 0.0, 3.0],
            [0.0, 0.0, 0.0],
            [0.0, 12.0, 3.0],
            [0.0, 12.0, 0.0],
            [2.0, 0.0, 3.0],
            [2.0, 0.0, 0.0],
            [2.0, 12.0, 3.0],
            [2.0, 12.0, 0.0],
            [10.0, 0.0, 3.0],
            [10.0, 0.0, 0.0],
            [10.0, 12.0, 3.0],
            [10.0, 12.0, 0.0],
            [12.0, 0.0, 3.0],
            [12.0, 0.0, 0.0],
            [12.0, 12.0, 3.0],
            [12.0, 12.0, 0.0],
            [20.0, 0.0, 3.0],
            [20.0, 0.0, 0.0],
            [20.0, 12.0, 3.0],
            [20.0, 12.0, 0.0],
            [22.0, 0.0, 3.0],
            [22.0, 0.0, 0.0],
            [22.0, 12.0, 3.0],
            [22.0, 12.0, 0.0],
        ],
    },
    BoxCase {
        name: "link_array_transform.FCStd",
        bounds: [
            [2.999_999_999_999_995_6, -7.0, 0.0],
            [25.0, 5.000_000_000_000_001, 3.0],
        ],
        boxes: 2,
        area: 264.0,
        volume: 144.0,
        corners: &[
            [5.000_000_000_000_001, 5.0, 3.0],
            [5.000_000_000_000_001, 5.0, 0.0],
            [4.999_999_999_999_996, -7.0, 3.0],
            [4.999_999_999_999_996, -7.0, 0.0],
            [3.000_000_000_000_001, 5.000_000_000_000_001, 3.0],
            [3.000_000_000_000_001, 5.000_000_000_000_001, 0.0],
            [2.999_999_999_999_995_6, -7.0, 3.0],
            [2.999_999_999_999_995_6, -7.0, 0.0],
            [25.0, 5.0, 3.0],
            [25.0, 5.0, 0.0],
            [24.999_999_999_999_993, -7.0, 3.0],
            [24.999_999_999_999_993, -7.0, 0.0],
            [23.0, 5.000_000_000_000_001, 3.0],
            [23.0, 5.000_000_000_000_001, 0.0],
            [22.999_999_999_999_993, -7.0, 3.0],
            [22.999_999_999_999_993, -7.0, 0.0],
        ],
    },
    BoxCase {
        name: "headless.FCStd",
        bounds: [[0.0, 0.0, 0.0], [27.071_067_811_865_47, 55.0, 3.0]],
        boxes: 2,
        area: 380.0,
        volume: 300.0,
        corners: &[
            [20.0, 0.0, 3.0],
            [20.0, 0.0, 0.0],
            [16.464_466_094_067_26, 3.535_533_905_932_735, 3.0],
            [16.464_466_094_067_26, 3.535_533_905_932_735, 0.0],
            [27.071_067_811_865_47, 7.071_067_811_865_48, 3.0],
            [27.071_067_811_865_47, 7.071_067_811_865_48, 0.0],
            [23.535_533_905_932_73, 10.606_601_717_798_215, 3.0],
            [23.535_533_905_932_73, 10.606_601_717_798_215, 0.0],
            [0.0, 50.0, 3.0],
            [0.0, 50.0, 0.0],
            [0.0, 55.0, 3.0],
            [0.0, 55.0, 0.0],
            [10.0, 50.0, 3.0],
            [10.0, 50.0, 0.0],
            [10.0, 55.0, 3.0],
            [10.0, 55.0, 0.0],
        ],
    },
    BoxCase {
        name: "hidden_container.FCStd",
        bounds: [[20.0, 0.0, 0.0], [30.0, 10.0, 10.0]],
        boxes: 1,
        area: 600.0,
        volume: 1000.0,
        corners: &[
            [20.0, 0.0, 10.0],
            [20.0, 0.0, 0.0],
            [20.0, 10.0, 10.0],
            [20.0, 10.0, 0.0],
            [30.0, 0.0, 10.0],
            [30.0, 0.0, 0.0],
            [30.0, 10.0, 10.0],
            [30.0, 10.0, 0.0],
        ],
    },
];

/// Compare a scalar to a literal independent geometric oracle.
///
/// # Panics
/// Panics if the value is nonfinite or differs by more than the supplied tolerance.
fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        actual.is_finite() && (actual - expected).abs() <= tolerance,
        "{actual} != {expected} within {tolerance}"
    );
}

/// Return coordinate components without calling an importer transform helper.
fn coordinates(point: Vec3) -> [f64; 3] {
    [point.x, point.y, point.z]
}

/// Compare actual extrema against independently supplied world-coordinate values.
///
/// # Panics
/// Panics if geometry is empty or any extent fails its literal oracle.
fn bounds(scene: &Scene, expected: [[f64; 3]; 2]) {
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    for point in scene.positions() {
        for (axis, value) in coordinates(point).into_iter().enumerate() {
            minimum[axis] = minimum[axis].min(value);
            maximum[axis] = maximum[axis].max(value);
        }
    }
    for (actual, expected) in minimum.into_iter().zip(expected[0]) {
        close(actual, expected, 1e-8);
    }
    for (actual, expected) in maximum.into_iter().zip(expected[1]) {
        close(actual, expected, 1e-8);
    }
}

/// Calculate surface area and oriented volume directly from emitted triangles.
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

/// Require a complete solid with every literal original corner and no duplicated occurrences.
///
/// # Panics
/// Panics if topology, surface area, winding, volume, or any literal corner disagrees.
fn boxes(scene: &Scene, original: &BoxCase) {
    assert_eq!(
        scene.triangles.len(),
        original.boxes * 12,
        "{}",
        original.name
    );
    assert!(scene.lines.is_empty() && scene.points.is_empty());
    bounds(scene, original.bounds);
    let (area, volume) = area_volume(scene);
    close(area, original.area, 1e-7);
    close(volume, original.volume, 1e-7);
    let positions: Vec<_> = scene.positions().collect();
    for expected in original.corners {
        assert!(
            positions.iter().any(|point| coordinates(*point)
                .into_iter()
                .zip(*expected)
                .all(|(actual, expected)| (actual - expected).abs() <= 1e-8)),
            "missing original {} corner {expected:?}",
            original.name
        );
    }
    for corners in original.corners.as_chunks::<8>().0 {
        face_quad_coverage(scene, corners);
    }
}

/// Require real native face coverage, rather than two copies of one triangle or a bounding cage.
///
/// # Panics
/// Panics unless each original quad has two outward triangles sharing its diagonal,
/// covering all four actual corners with the full original face area.
fn face_quad_coverage(scene: &Scene, corners: &[[f64; 3]; 8]) {
    let center = Vec3::new(
        corners.iter().map(|point| point[0]).sum::<f64>() / 8.,
        corners.iter().map(|point| point[1]).sum::<f64>() / 8.,
        corners.iter().map(|point| point[2]).sum::<f64>() / 8.,
    );
    for indices in [
        [0, 1, 2, 3],
        [4, 5, 6, 7],
        [0, 1, 4, 5],
        [2, 3, 6, 7],
        [1, 3, 5, 7],
        [0, 2, 4, 6],
    ] {
        let face = indices.map(|index| corners[index]);
        let triangles: Vec<_> = scene
            .triangles
            .iter()
            .filter_map(|triangle| {
                let local: Option<Vec<_>> = triangle
                    .vertices
                    .iter()
                    .map(|point| {
                        face.iter().position(|expected| {
                            coordinates(*point)
                                .into_iter()
                                .zip(*expected)
                                .all(|(actual, expected)| (actual - expected).abs() <= 1e-8)
                        })
                    })
                    .collect();
                local.map(|local| (triangle, local))
            })
            .collect();
        assert_eq!(
            triangles.len(),
            2,
            "native quad {face:?} must be complete exactly once"
        );
        let shared: Vec<_> = triangles[0]
            .1
            .iter()
            .copied()
            .filter(|index| triangles[1].1.contains(index))
            .collect();
        assert!(
            shared == [0, 3] || shared == [3, 0] || shared == [1, 2] || shared == [2, 1],
            "triangles must share a native quad diagonal, got {shared:?}"
        );
        let a = Vec3::new(face[0][0], face[0][1], face[0][2]);
        let b = Vec3::new(face[1][0], face[1][1], face[1][2]);
        let c = Vec3::new(face[2][0], face[2][1], face[2][2]);
        let expected_area = (b - a).cross(c - a).length();
        let mut area = 0.;
        let mut seen = [false; 4];
        for (triangle, local) in triangles {
            let [a, b, c] = triangle.vertices;
            let normal = (b - a).cross(c - a);
            assert!(normal.length() > 1e-8);
            assert!(
                normal.dot((a + b + c) / 3. - center) > 0.,
                "native face winding points inward"
            );
            area += normal.length() / 2.;
            for index in local {
                seen[index] = true;
            }
        }
        assert_eq!(seen, [true; 4]);
        close(area, expected_area, 1e-8);
    }
}

/// Enumerate the native box corner order from independently supplied axis-aligned extents.
fn axis_corners(minimum: [f64; 3], maximum: [f64; 3]) -> [[f64; 3]; 8] {
    let [x0, y0, z0] = minimum;
    let [x1, y1, z1] = maximum;
    [
        [x0, y0, z1],
        [x0, y0, z0],
        [x0, y1, z1],
        [x0, y1, z0],
        [x1, y0, z1],
        [x1, y0, z0],
        [x1, y1, z1],
        [x1, y1, z0],
    ]
}

/// Produce the exact lowercase SHA-256 used by the independently pinned source inventory.
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", digest_hex::Hex(Sha256::digest(bytes)))
}

/// Use deterministic headless render settings with an exact source-sized canvas.
fn options(size: u32) -> RenderOptions {
    RenderOptions {
        width: size,
        height: size,
        transparent: true,
        ..Default::default()
    }
}

/// Invoke only this project's public CLI without display-server environment variables.
///
/// # Panics
/// Panics if the already-built integration-test binary cannot be launched.
fn command(path: &Path, size: u32, flags: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_catwalk"));
    command
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR")
        .arg(path)
        .args(["--output", "-"]);
    if !flags.contains(&"--extract-saved-preview") {
        command.args(["--size", &size.to_string(), "--transparent"]);
    }
    command.args(flags).output().unwrap()
}

/// Decode successful CLI PNG bytes without allowing diagnostic text in the image stream.
///
/// # Panics
/// Panics for failed commands, diagnostics, or an invalid PNG stream.
fn cli_png(output: &Output) -> RgbaImage {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stderr.as_slice(), b"");
    assert!(output.stdout.starts_with(b"\x89PNG\r\n\x1a\n"));
    image::load_from_memory(&output.stdout)
        .unwrap()
        .into_rgba8()
}

/// Decode one original saved PNG independently of the `FreeCAD` extraction dispatch.
///
/// # Panics
/// Panics if a retained original member or its documented PNG is missing/malformed.
fn original_png(name: &str) -> RgbaImage {
    let bytes = original_member(name, "thumbnails/Thumbnail.png").unwrap();
    image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .unwrap()
        .into_rgba8()
}

/// Every original byte stream and both original upstream licensing files are pinned.
#[test]
fn all_original_source_bytes_and_complete_original_licenses_are_retained_exactly() {
    for (name, size, digest) in ORIGINALS {
        let bytes = fs::read(fixture(name)).unwrap();
        assert_eq!(bytes.len(), size, "{name}");
        assert_eq!(hash(&bytes), digest, "{name}");
    }
    for (name, size, digest) in [
        (
            "LICENSE.md",
            1607,
            "42e90e43576bfcc8945239179c49141c387c407a3a3c3696789d9f061cc4afb4",
        ),
        (
            "DATA_LICENSES.md",
            9103,
            "4229d1453e125ace50ca1825d420340bbc6b2ee4f2e6ffabde8709052534e12a",
        ),
    ] {
        let bytes = fs::read(fixture(name)).unwrap();
        assert_eq!(bytes.len(), size);
        assert_eq!(hash(&bytes), digest);
    }
}

/// Eight genuine stored box examples preserve native corners, placements and exact solid counts.
#[test]
fn eight_original_native_box_world_corners_bounds_area_volume_and_winding_are_exact() {
    for original in &BOXES {
        let scene = formats::load(&fixture(original.name)).unwrap();
        boxes(&scene, original);
    }
}

/// The minimal owned native reader also preserves the same unchanged original BREP outside `FCStd`.
#[test]
fn original_addressed_brep_standalone_has_complete_quads_and_matches_the_document_solid() {
    let bytes = original_member("colored_visibility.FCStd", "PartShape.brp").unwrap();
    let input = Control::new(&bytes, "brep").unwrap();
    boxes(&formats::load(&input.path).unwrap(), &BOXES[0]);
    assert_eq!(fs::read(&input.path).unwrap(), bytes);
}

/// The same oriented native edge multiset yields complete quads regardless of persisted wire child order.
#[test]
fn standalone_original_native_wire_permutation_preserves_real_face_coverage() {
    let bytes = original_member("face_colors.FCStd", "PartShape.brp").unwrap();
    let source = String::from_utf8(bytes).unwrap();
    let permuted = source.replacen("-32 0 -30 0 +28 0 +27 0 *", "+27 0 +28 0 -30 0 -32 0 *", 1);
    assert_ne!(permuted, source);
    let input = Control::new(permuted.as_bytes(), "brep").unwrap();
    let scene = formats::load(&input.path).unwrap();
    assert_eq!(scene.triangles.len(), 12);
    bounds(&scene, [[0.; 3], [4.; 3]]);
    let (area, volume) = area_volume(&scene);
    close(area, 96., 1e-8);
    close(volume, 64., 1e-8);
    face_quad_coverage(&scene, &axis_corners([0.; 3], [4.; 3]));
}

/// Repeated oriented edges and disconnected native wires reject before emitting an incomplete standalone shape.
#[test]
fn standalone_repeated_or_disconnected_native_wire_children_fail_explicitly() {
    let source =
        String::from_utf8(original_member("face_colors.FCStd", "PartShape.brp").unwrap()).unwrap();
    for value in ["-32 0 -30 0 +28 0 +27 0 -32 0 *", "-32 0 +28 0 +27 0 *"] {
        let malformed = source.replacen("-32 0 -30 0 +28 0 +27 0 *", value, 1);
        assert_ne!(malformed, source);
        let input = Control::new(malformed.as_bytes(), "brep").unwrap();
        let error = formats::load(&input.path)
            .unwrap_err()
            .to_string()
            .to_lowercase();
        assert!(
            ["wire", "repeated", "disconnected", "closed"]
                .iter()
                .any(|term| error.contains(term)),
            "{error}"
        );
    }
}

/// Legacy GUI fallback hides history, while missing providers retain headless document visibility.
#[test]
fn original_legacy_visibility_and_empty_gui_provider_fallback_preserve_object_selection() {
    boxes(
        &formats::load(&fixture("legacy_visibility.FCStd")).unwrap(),
        &BOXES[0],
    );
    boxes(
        &formats::load(&fixture("gui_without_view_providers.FCStd")).unwrap(),
        &BOXES[6],
    );
}

/// A persisted `BuildingPart` aggregate and its nested slab must emit only one complete slab.
#[test]
fn original_arch_aggregate_and_nested_group_do_not_duplicate_the_slab() {
    let scene = formats::load(&fixture("arch_floor.FCStd")).unwrap();
    assert_eq!(scene.triangles.len(), 12);
    bounds(&scene, [[0., 0., 0.], [30., 30., 2.]]);
    let (area, volume) = area_volume(&scene);
    close(area, 2040., 1e-8);
    close(volume, 1800., 1e-8);
    face_quad_coverage(&scene, &axis_corners([0., 0., 0.], [30., 30., 2.]));
}

/// The original body tip and side box are selected once; aggregate and hidden history are omitted.
#[test]
fn original_partdesign_body_tip_and_side_match_their_actual_stored_native_members_once() {
    let input = fixture("partdesign_body.FCStd");
    let scene = formats::load(&input).unwrap();
    let tip = Control::new(
        &original_member("partdesign_body.FCStd", "PartShape4.brp").unwrap(),
        "brep",
    )
    .unwrap();
    let side = Control::new(
        &original_member("partdesign_body.FCStd", "PartShape5.brp").unwrap(),
        "brep",
    )
    .unwrap();
    let tip_scene = formats::load(&tip.path).unwrap();
    let side_scene = formats::load(&side.path).unwrap();
    assert_eq!(
        scene.triangles.len(),
        tip_scene.triangles.len() + side_scene.triangles.len()
    );
    let expected_positions: Vec<_> = tip_scene
        .positions()
        .chain(side_scene.positions())
        .collect();
    assert_eq!(scene.positions().collect::<Vec<_>>(), expected_positions);
    bounds(&scene, [[-3., -3., 0.], [24., 10., 20.]]);
    let (area, volume) = area_volume(&scene);
    let (tip_area, tip_volume) = area_volume(&tip_scene);
    let (side_area, side_volume) = area_volume(&side_scene);
    close(side_area, 96., 1e-8);
    close(side_volume, 64., 1e-8);
    close(area, tip_area + 96., 1e-8);
    close(volume, tip_volume + 64., 1e-8);
    // Native dimensions: positive-quadrant box 10³ and cylinder radius 3, height 20.
    // Their lower quarter-cylinder overlap has volume 22.5π; the native curved
    // face exposes 270° below z10 and 360° above it. Preview facets allow 2% error.
    let analytic_tip_area = 540. + 118.5 * std::f64::consts::PI;
    let analytic_tip_volume = 1_000. + 157.5 * std::f64::consts::PI;
    close(tip_area, analytic_tip_area, analytic_tip_area * 0.02);
    close(tip_volume, analytic_tip_volume, analytic_tip_volume * 0.02);
    close(
        area,
        analytic_tip_area + 96.,
        (analytic_tip_area + 96.) * 0.02,
    );
    close(
        volume,
        analytic_tip_volume + 64.,
        (analytic_tip_volume + 64.) * 0.02,
    );
}

/// Native face colors are associated with actual planes in shell order, independent of triangulation order.
#[test]
fn original_face_colors_and_unknown_program_version_follow_native_face_positions() {
    for name in ["face_colors.FCStd", "no_version.FCStd"] {
        let scene = formats::load(&fixture(name)).unwrap();
        let mut counts = [0_usize; 6];
        let planes = [
            (0, 0., [255, 0, 0]),
            (0, 4., [0, 255, 0]),
            (1, 0., [0, 0, 255]),
            (1, 4., [255, 255, 0]),
            (2, 0., [255, 0, 255]),
            (2, 4., [0, 255, 255]),
        ];
        assert_eq!(scene.triangles.len(), 12);
        face_quad_coverage(&scene, &axis_corners([0.; 3], [4.; 3]));
        for (index, triangle) in scene.triangles.iter().enumerate() {
            let face = planes
                .iter()
                .position(|(axis, value, _)| {
                    triangle
                        .vertices
                        .iter()
                        .all(|point| (coordinates(*point)[*axis] - value).abs() <= 1e-8)
                })
                .unwrap();
            assert_eq!(triangle.color, Some(planes[face].2));
            assert_eq!(
                scene
                    .appearances
                    .get(&index)
                    .map_or(1., |appearance| appearance.base_color[3]),
                1.
            );
            counts[face] += 1;
        }
        assert_eq!(counts, [2; 6]);
    }
}

/// Decode a literal `sRGB` byte to its linear-light channel without importer color helpers.
fn linear(value: u8) -> f64 {
    let value = f64::from(value) / 255.;
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Require exact flat RGB and correctly decoded straight alpha on one actual face.
///
/// # Panics
/// Panics if color, appearance alpha, blending, or linear RGB disagrees.
fn appearance(scene: &Scene, index: usize, rgb: [u8; 3], alpha: u8) {
    assert_eq!(scene.triangles[index].color, Some(rgb));
    if alpha == 255 {
        close(
            scene
                .appearances
                .get(&index)
                .map_or(1., |appearance| appearance.base_color[3]),
            1.,
            1e-12,
        );
    } else {
        let appearance = scene.appearances.get(&index).unwrap();
        assert!(matches!(appearance.alpha_mode, AlphaMode::Blend));
        for (actual, expected) in appearance.base_color.into_iter().zip([
            linear(rgb[0]),
            linear(rgb[1]),
            linear(rgb[2]),
            f64::from(alpha) / 255.,
        ]) {
            close(actual, expected, 1e-12);
        }
    }
}

/// Original pre-1.1 object transparency and per-face transparency use their actual geometry positions.
#[test]
fn original_legacy_transparency_is_exact_on_the_correct_object_and_face() {
    let scene = formats::load(&fixture("transparency.FCStd")).unwrap();
    assert_eq!(scene.triangles.len(), 36);
    for (minimum, maximum) in [
        ([0., 0., 0.], [10., 3., 6.]),
        ([-2., -4., -2.], [12., -3., 8.]),
        ([16., 0., 0.], [20., 4., 4.]),
    ] {
        face_quad_coverage(&scene, &axis_corners(minimum, maximum));
    }
    let mut groups = [0_usize; 4];
    for (index, triangle) in scene.triangles.iter().enumerate() {
        let center = triangle.vertices.iter().map(|p| p.x).sum::<f64>() / 3.;
        if center > 15. {
            if triangle.vertices.iter().all(|point| point.y.abs() <= 1e-8) {
                appearance(&scene, index, [230, 230, 26], 51);
                groups[3] += 1;
            } else {
                appearance(&scene, index, [26, 204, 26], 255);
                groups[2] += 1;
            }
        } else if triangle.vertices.iter().any(|point| point.y < -1.) {
            appearance(&scene, index, [51, 102, 230], 102);
            groups[1] += 1;
        } else {
            appearance(&scene, index, [204, 26, 26], 255);
            groups[0] += 1;
        }
    }
    assert_eq!(groups, [12, 12, 10, 2]);
}

/// Original `FreeCAD` 1.1 materials follow native outer, top, bottom and inner face occurrences.
#[test]
fn original_one_one_materials_follow_native_outer_top_bottom_inner_face_order() {
    let native =
        String::from_utf8(original_member("bracket_1x.FCStd", "Tube.Shape.brp").unwrap()).unwrap();
    assert!(native.contains("Sh\n\n0101100\n+17 0 +12 0 -7 0 -4 0 *"));
    let scene = formats::load(&fixture("bracket_1x.FCStd")).unwrap();
    let mut tube_faces = [0_usize; 4];
    let mut bracket = Scene::default();
    for (index, triangle) in scene.triangles.iter().enumerate() {
        if triangle.vertices.iter().all(|point| point.x > 50.) {
            let face = if triangle
                .vertices
                .iter()
                .all(|point| (point.z - 18.).abs() <= 1e-8)
            {
                1
            } else if triangle.vertices.iter().all(|point| point.z.abs() <= 1e-8) {
                3
            } else if triangle
                .vertices
                .iter()
                .all(|point| ((point.x - 60.).hypot(point.y - 15.) - 6.).abs() <= 1e-8)
            {
                0
            } else {
                2
            };
            if face == 2 {
                assert!(
                    triangle
                        .vertices
                        .iter()
                        .all(|point| ((point.x - 60.).hypot(point.y - 15.) - 4.).abs() <= 1e-8)
                );
            }
            // Native shell faces 17/12/7/4 use surfaces 1/2/3/4: outer/top/bottom/inner.
            // These geometric classifier indices are outer/top/inner/bottom.
            let colors = [[204, 26, 26], [26, 179, 51], [230, 204, 26], [38, 77, 204]];
            appearance(
                &scene,
                index,
                colors[face],
                if face == 3 { 128 } else { 255 },
            );
            tube_faces[face] += 1;
        } else {
            appearance(&scene, index, [230, 140, 26], 255);
            bracket.triangles.push(triangle.clone());
        }
    }
    assert!(tube_faces.into_iter().all(|count| count >= 2));
    let zs: Vec<_> = bracket.positions().map(|point| point.z).collect();
    close(zs.iter().copied().fold(f64::INFINITY, f64::min), 5., 1e-8);
    close(
        zs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        25.,
        1e-8,
    );
}

/// Exact original source PNG and independently decoded RGBA values are preferred over fresh geometry.
#[test]
fn original_saved_pngs_are_exact_preferred_and_geometry_bypass_renders_the_actual_model() {
    for (name, size, png_hash, pixel_hash) in [
        (
            "colored_visibility.FCStd",
            128,
            "12706ac6cb0f85364847ec9f1387ef6ef11958b55cfb86a9f618be4fa86d9e4c",
            "f7c385e119d87a0f795bf51c473da4080b3738eaecb37fa3724b7c83f8a76930",
        ),
        (
            "bracket_1x.FCStd",
            256,
            "7a1f8d13f287cad87f6804b18da603c15165b141671aa5971c7d25a493669182",
            "205995d89567924b9983b758bf067ec3a1e32e23d682b5bad4930fcefc14738c",
        ),
    ] {
        let path = fixture(name);
        let before = fs::read(&path).unwrap();
        let original = original_png(name);
        assert_eq!(original.dimensions(), (size, size));
        assert_eq!(
            hash(&original_member(name, "thumbnails/Thumbnail.png").unwrap()),
            png_hash
        );
        assert_eq!(hash(original.as_raw()), pixel_hash);
        assert_eq!(saved_preview::extract(&path).unwrap().unwrap(), original);
        assert_eq!(thumbnail(&path, &options(size)).unwrap(), original);
        let scene = formats::load(&path).unwrap();
        let fresh = render(&scene, &options(size)).unwrap();
        assert_eq!(geometry_thumbnail(&path, &options(size)).unwrap(), fresh);
        assert_ne!(fresh, original);
        assert_eq!(cli_png(&command(&path, size, &[])), original);
        assert_eq!(
            cli_png(&command(&path, size, &["--render-geometry"])),
            fresh
        );
        assert_eq!(
            cli_png(&command(&path, size, &["--extract-saved-preview"])),
            original
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }
}

/// No-GUI originals use fresh native geometry and extraction-only retains its no-image exit status.
#[test]
fn original_headless_document_falls_back_to_fresh_geometry_without_display_or_saved_carrier() {
    let path = fixture("headless.FCStd");
    assert!(saved_preview::extract(&path).unwrap().is_none());
    let expected = render(&formats::load(&path).unwrap(), &options(128)).unwrap();
    assert!(expected.pixels().any(|pixel| pixel[3] != 0));
    assert_eq!(thumbnail(&path, &options(128)).unwrap(), expected);
    assert_eq!(geometry_thumbnail(&path, &options(128)).unwrap(), expected);
    assert_eq!(cli_png(&command(&path, 128, &[])), expected);
    let extracted = command(&path, 128, &["--extract-saved-preview"]);
    assert_eq!(extracted.status.code(), Some(3));
    assert_eq!(extracted.stdout.as_slice(), b"");
}

/// Wireframe display preserves the four actual rectangle boundaries without filling its face.
#[test]
fn original_draft_wireframe_keeps_the_rectangle_perimeter_and_only_the_solid_box_faces() {
    let scene = formats::load(&fixture("draft_wireframe.FCStd")).unwrap();
    assert_eq!(scene.triangles.len(), 12);
    let (area, volume) = area_volume(&scene);
    close(area, 160., 1e-8);
    close(volume, 100., 1e-8);
    let mut perimeter = [0.; 4];
    let edges = [(0, 0.), (0, 10.), (1, 0.), (1, 5.)];
    for line in &scene.lines {
        for point in line.vertices {
            close(point.z, 2., 1e-8);
        }
        let edge = edges
            .iter()
            .position(|(axis, value)| {
                line.vertices
                    .iter()
                    .all(|point| (coordinates(*point)[*axis] - value).abs() <= 1e-8)
            })
            .unwrap();
        perimeter[edge] += (line.vertices[1] - line.vertices[0]).length();
    }
    for (actual, expected) in perimeter.into_iter().zip([5., 5., 10., 10.]) {
        close(actual, expected, 1e-8);
    }
}

/// One explicitly authored declaration and its separately joined property values.
struct Object {
    /// Exact stable native object identifier.
    name: String,
    /// Native runtime class, independent of any display label.
    kind: String,
    /// Complete explicitly authored direct property elements.
    properties: Vec<String>,
}

/// Construct a small typed object without consulting importer object definitions.
fn object(name: &str, kind: &str, properties: Vec<String>) -> Object {
    Object {
        name: name.into(),
        kind: kind.into(),
        properties,
    }
}

/// Construct one native property with its exact declared carrier type and direct child.
fn property(name: &str, kind: &str, value: &str) -> String {
    format!("<Property name=\"{name}\" type=\"{kind}\">{value}</Property>")
}

/// Select a stored shape only through a typed `Part` property and exact direct file address.
fn shape(file: &str) -> String {
    property(
        "Shape",
        "Part::PropertyPartShape",
        &format!("<Part file=\"{file}\"/>"),
    )
}

/// Author a native typed group without relying on production hierarchy helpers.
///
/// # Panics
/// Panics if in-memory XML formatting unexpectedly fails.
fn group(children: &[&str]) -> String {
    let mut links = String::new();
    for name in children {
        write!(&mut links, "<Link value=\"{name}\"/>")
            .expect("formatting XML into an owned String cannot fail");
    }
    property(
        "Group",
        "App::PropertyLinkList",
        &format!("<LinkList count=\"{}\">{links}</LinkList>", children.len()),
    )
}

/// Author a native local link, leaving external/subelement attributes explicit in tests.
fn link(target: &str) -> String {
    property(
        "LinkedObject",
        "App::PropertyXLink",
        &format!("<XLink name=\"{target}\" file=\"\"/>"),
    )
}

/// Serialize explicitly chosen finite or deliberately invalid placement scalars.
fn placement(name: &str, values: [f64; 7]) -> String {
    let [px, py, pz, qx, qy, qz, qw] = values;
    property(
        name,
        "App::PropertyPlacement",
        &format!(
            "<PropertyPlacement Px=\"{px}\" Py=\"{py}\" Pz=\"{pz}\" Q0=\"{qx}\" Q1=\"{qy}\" Q2=\"{qz}\" Q3=\"{qw}\"/>"
        ),
    )
}

/// Emit exact schema-four declarations and separate directly owned property records.
///
/// # Panics
/// Panics if in-memory XML formatting unexpectedly fails.
fn document(objects: &[Object]) -> String {
    let mut declarations = String::new();
    let mut values = String::new();
    for object in objects {
        write!(
            &mut declarations,
            "<Object name=\"{}\" type=\"{}\"/>",
            object.name, object.kind
        )
        .expect("formatting XML into an owned String cannot fail");
        write!(
            &mut values,
            "<Object name=\"{}\"><Properties Count=\"{}\">{}</Properties></Object>",
            object.name,
            object.properties.len(),
            object.properties.concat()
        )
        .expect("formatting XML into an owned String cannot fail");
    }
    format!(
        "<Document SchemaVersion=\"4\" FileVersion=\"1\" ProgramVersion=\"0.21\"><Objects Count=\"{count}\">{declarations}</Objects><ObjectData Count=\"{count}\">{values}</ObjectData></Document>",
        count = objects.len()
    )
}

/// Write a bounded stored container with the actual original selected red-box BREP.
///
/// # Panics
/// Panics if an original member is unavailable or the authored ZIP exceeds helper limits.
fn container(xml: &str, extra: Vec<Member>) -> Vec<u8> {
    let mut members = vec![
        Member::new("Document.xml", xml.as_bytes()),
        Member::new(
            "Selected.brp",
            original_member("colored_visibility.FCStd", "PartShape.brp").unwrap(),
        ),
    ];
    members.extend(extra);
    stored(&members).unwrap()
}

/// Load one authored control and require a meaningful input-related rejection.
///
/// # Panics
/// Panics if input unexpectedly loads, an unrelated extension error occurs, or diagnostics lack context.
fn reject(bytes: &[u8], terms: &[&str]) -> String {
    let control = Control::new(bytes, "FCStd").unwrap();
    let error = formats::load(&control.path)
        .unwrap_err()
        .to_string()
        .to_lowercase();
    assert!(
        !error.contains("unsupported model extension") && !error.contains("no geometry reader")
    );
    assert!(
        terms.iter().any(|term| error.contains(term)),
        "unexpected rejection: {error}"
    );
    error
}

/// Load a small authored carrier through the public model dispatcher.
///
/// # Panics
/// Panics if temporary input cannot be written or native geometry cannot be loaded.
fn control_scene(bytes: &[u8]) -> Scene {
    let control = Control::new(bytes, "FCStd").unwrap();
    formats::load(&control.path).unwrap()
}

/// Original upstream malformed inputs fail instead of returning an incomplete local scene.
#[test]
fn original_malformed_documents_cycles_and_unqualified_external_links_reject_meaningfully() {
    for (name, terms) in [
        ("invalid_content.FCStd", &["document", "zip"] as &[&str]),
        ("malformed_no_objects.FCStd", &["objects", "document"]),
        ("malformed_no_objectdata.FCStd", &["objectdata", "document"]),
        (
            "malformed_elements.FCStd",
            &["object", "property", "member", "count"],
        ),
        (
            "malformed_gui.FCStd",
            &["xml", "gui", "unexpected", "expected"],
        ),
        (
            "corrupt_brp.FCStd",
            &["brep", "topology", "header", "native"],
        ),
        (
            "cyclic_groups.FCStd",
            // Original Objects/ObjectData each declare 9 records but contain 11.
            &["declared record count"],
        ),
        (
            "broken_links.FCStd",
            // Original Objects declares 11 records but contains 10 declarations.
            &["declared record count"],
        ),
        ("link_external.FCStd", &["external", "unsupported"]),
    ] {
        let error = formats::load(&fixture(name))
            .unwrap_err()
            .to_string()
            .to_lowercase();
        assert!(
            terms.iter().any(|term| error.contains(term)),
            "{name}: {error}"
        );
    }
}

/// Native declarations and values must join by exact object identifier, never by ZIP order or label.
#[test]
fn shape_admission_requires_exact_declared_owner_and_the_correct_direct_typed_carrier() {
    let good = document(&[object("Box", "Part::Feature", vec![shape("Selected.brp")])]);
    let unjoined = good.replacen(
        "<Object name=\"Box\"><Properties",
        "<Object name=\"Other\"><Properties",
        1,
    );
    reject(&container(&unjoined, vec![]), &["object", "owner"]);
    for value in [
        property(
            "Shape",
            "App::PropertyString",
            "<Part file=\"Selected.brp\"/>",
        ),
        property(
            "Other",
            "Part::PropertyPartShape",
            "<Part file=\"Selected.brp\"/>",
        ),
        property(
            "Shape",
            "Part::PropertyPartShape",
            "<Wrapper><Part file=\"Selected.brp\"/></Wrapper>",
        ),
        property(
            "Shape",
            "Part::PropertyPartShape",
            "<Part><ElementMap2 file=\"Selected.brp\"/></Part>",
        ),
    ] {
        reject(
            &container(
                &document(&[object("Box", "Part::Feature", vec![value])]),
                vec![],
            ),
            &["type", "shape", "carrier", "file", "geometry"],
        );
    }
}

/// Foreign root, object, property and `Part` namespaces cannot spoof native shape ownership or saved admission.
#[test]
fn foreign_namespaces_never_admit_native_or_saved_carriers() {
    let good = document(&[object("Box", "Part::Feature", vec![shape("Selected.brp")])]);
    for (from, to) in [
        ("<Document ", "<Document xmlns=\"urn:foreign\" "),
        (
            "<Object name=\"Box\"><Properties",
            "<Object xmlns=\"urn:foreign\" name=\"Box\"><Properties",
        ),
        (
            "<Property name=\"Shape\"",
            "<Property xmlns=\"urn:foreign\" name=\"Shape\"",
        ),
        ("<Part file=", "<Part xmlns=\"urn:foreign\" file="),
    ] {
        let xml = good.replacen(from, to, 1);
        let bytes = container(
            &xml,
            vec![Member::new(
                "thumbnails/Thumbnail.png",
                original_member("colored_visibility.FCStd", "thumbnails/Thumbnail.png").unwrap(),
            )],
        );
        reject(&bytes, &["namespace", "native", "document"]);
        let input = Control::new(&bytes, "FCStd").unwrap();
        assert!(saved_preview::extract(&input.path).is_err());
        assert!(thumbnail(&input.path, &options(128)).is_err());
    }
}

/// The exact `Part` file address wins even when misleading native filenames and descendant maps exist.
#[test]
fn addressed_native_shape_and_null_selection_do_not_fall_back_to_unrelated_archive_members() {
    let selected = original_member("colored_visibility.FCStd", "PartShape.brp").unwrap();
    let doc = document(&[object("Box", "Part::Feature", vec![shape("Chosen.brp")])]);
    let bytes = container(
        &doc,
        vec![
            Member::new("Chosen.brp", selected),
            Member::new("PartShape.brp", b"bad native decoy".as_slice()),
        ],
    );
    boxes(&control_scene(&bytes), &BOXES[0]);
    let missing = document(&[object("Box", "Part::Feature", vec![shape("Missing.brp")])]);
    reject(
        &container(&missing, vec![]),
        &["missing", "absent", "member"],
    );
    let null_and_real = document(&[
        object("Null", "Part::Feature", vec![shape("Null.brp")]),
        object("Real", "Part::Feature", vec![shape("Selected.brp")]),
    ]);
    boxes(
        &control_scene(&container(
            &null_and_real,
            vec![
                Member::new("Null.brp", Vec::new()),
                Member::new("Decoy.brp", b"bad unrelated BREP".as_slice()),
            ],
        )),
        &BOXES[0],
    );
    let null_only = document(&[object("Null", "Part::Feature", vec![shape("Null.brp")])]);
    reject(
        &container(&null_only, vec![Member::new("Null.brp", Vec::new())]),
        &["geometry", "empty"],
    );
    for part in [
        "<Part/>",
        "<Part file=\"\"/>",
        "<Part><ElementMap2 file=\"Selected.brp\"/></Part>",
    ] {
        let doc = document(&[
            object("Good", "Part::Feature", vec![shape("Selected.brp")]),
            object(
                "Bad",
                "Part::Feature",
                vec![property("Shape", "Part::PropertyPartShape", part)],
            ),
        ]);
        reject(
            &container(&doc, vec![]),
            &["file", "address", "shape", "carrier"],
        );
    }
}

/// Duplicate native identities and physical ZIP names cannot silently overwrite selected ownership.
#[test]
fn duplicate_declarations_values_properties_and_physical_member_names_are_rejected() {
    let single = object("Box", "Part::Feature", vec![shape("Selected.brp")]);
    reject(
        &container(
            &document(&[single, object("Box", "Part::Feature", vec![])]),
            vec![],
        ),
        &["duplicate object"],
    );
    let good = document(&[object("Box", "Part::Feature", vec![shape("Selected.brp")])]);
    let record = "<Object name=\"Box\"><Properties Count=\"0\"/></Object>";
    let duplicate = good.replace("</ObjectData>", &format!("{record}</ObjectData>"));
    reject(
        &container(&duplicate, vec![]),
        &["duplicate object", "count"],
    );
    let repeated = document(&[object(
        "Box",
        "Part::Feature",
        vec![shape("Selected.brp"), shape("Selected.brp")],
    )]);
    reject(
        &container(&repeated, vec![]),
        &["duplicate object property"],
    );
    reject(
        &container(
            &good,
            vec![Member::new("Document.xml", good.clone().into_bytes())],
        ),
        &["duplicate"],
    );
}

/// Canonical archive names reject traversal, absolute paths, backslash aliases and embedded NUL.
#[test]
fn unsafe_archive_member_names_fail_before_native_or_saved_selection() {
    let good = document(&[object("Box", "Part::Feature", vec![shape("Selected.brp")])]);
    for name in [
        "../Selected.brp",
        "/Selected.brp",
        "a\\Selected.brp",
        "nul\0.brp",
        "a/../Selected.brp",
    ] {
        reject(
            &container(&good, vec![Member::new(name, b"unused".as_slice())]),
            &["path", "name", "zip", "member"],
        );
    }
}

/// Group cycles and multiple parents fail instead of using a truncated or last-parent placement.
#[test]
fn group_cycles_link_cycles_and_multiple_parent_ambiguity_are_bounded_errors() {
    let cycle = document(&[
        object("A", "App::Part", vec![group(&["B"]), shape("Selected.brp")]),
        object("B", "App::Part", vec![group(&["A"])]),
    ]);
    reject(
        &container(&cycle, vec![]),
        &["cycle", "hierarchy", "parent"],
    );
    let duplicate_parent = document(&[
        object("A", "App::Part", vec![group(&["Box"])]),
        object("B", "App::Part", vec![group(&["Box"])]),
        object("Box", "Part::Feature", vec![shape("Selected.brp")]),
    ]);
    reject(&container(&duplicate_parent, vec![]), &["parent"]);
    let links = document(&[
        object("A", "App::Link", vec![link("B")]),
        object("B", "App::Link", vec![link("A")]),
    ]);
    reject(&container(&links, vec![]), &["cyclic", "nested", "target"]);
}

/// Object property placement is not applied twice, and plain organizational groups carry no transform.
#[test]
fn ordinary_shape_placement_and_organizational_group_placement_do_not_move_native_root_again() {
    let moved_property = document(&[object(
        "Box",
        "Part::Feature",
        vec![
            shape("Selected.brp"),
            placement("Placement", [77., 88., 99., 0., 0., 0., 1.]),
        ],
    )]);
    boxes(
        &control_scene(&container(&moved_property, vec![])),
        &BOXES[0],
    );
    let grouped = document(&[
        object(
            "Group",
            "App::DocumentObjectGroup",
            vec![
                group(&["Box"]),
                placement("Placement", [77., 88., 99., 0., 0., 0., 1.]),
            ],
        ),
        object("Box", "Part::Feature", vec![shape("Selected.brp")]),
    ]);
    boxes(&control_scene(&container(&grouped, vec![])), &BOXES[0]);
}

/// Finite quaternion rescaling cannot change a bounded rigid rotation or underflow it to a singularity.
#[test]
fn large_and_subnormal_scale_quaternions_preserve_the_same_finite_rigid_geometry() {
    for scale in [1e308, 1e-300] {
        let doc = document(&[
            object(
                "Part",
                "App::Part",
                vec![
                    group(&["Box"]),
                    placement("Placement", [0., 0., 0., scale, scale, 0., 0.]),
                ],
            ),
            object("Box", "Part::Feature", vec![shape("Selected.brp")]),
        ]);
        let scene = control_scene(&container(&doc, vec![]));
        assert_eq!(scene.triangles.len(), 12);
        bounds(&scene, [[0., 0., -3.], [5., 10., 0.]]);
        let (area, volume) = area_volume(&scene);
        close(area, 190., 1e-8);
        close(volume, 150., 1e-8);
    }
}

/// Native `FreeCAD` `Restore` selects authored axis-angle over contradictory quaternion fields.
#[test]
fn axis_angle_has_native_restore_precedence_over_inconsistent_quaternion_fields() {
    let carrier = property(
        "Placement",
        "App::PropertyPlacement",
        &format!(
            "<PropertyPlacement Px=\"0\" Py=\"0\" Pz=\"0\" Q0=\"0\" Q1=\"0\" Q2=\"0\" Q3=\"1\" A=\"{}\" Ox=\"0\" Oy=\"0\" Oz=\"1\"/>",
            std::f64::consts::FRAC_PI_2
        ),
    );
    let doc = document(&[
        object("Part", "App::Part", vec![group(&["Box"]), carrier]),
        object("Box", "Part::Feature", vec![shape("Selected.brp")]),
    ]);
    bounds(
        &control_scene(&container(&doc, vec![])),
        [[-5., 0., 0.], [0., 10., 3.]],
    );
}

/// Nonfinite and out-of-range scalars reject before evaluating selected CAD geometry.
#[test]
fn nonfinite_placement_translation_rotation_and_singular_quaternions_are_rejected() {
    for values in [
        [f64::NAN, 0., 0., 0., 0., 0., 1.],
        [0., f64::INFINITY, 0., 0., 0., 0., 1.],
        [0., 0., 0., f64::NEG_INFINITY, 0., 0., 1.],
        [1e13, 0., 0., 0., 0., 0., 1.],
        [0.; 7],
    ] {
        let doc = document(&[object(
            "Box",
            "Part::Feature",
            vec![shape("Selected.brp"), placement("Placement", values)],
        )]);
        reject(
            &container(&doc, vec![]),
            &["nonfinite", "excessive", "singular"],
        );
    }
}

/// Unsupported selected native modes cannot be hidden by an otherwise valid local shape.
#[test]
fn external_subelement_scaled_binary_and_inline_selected_geometry_fail_explicitly() {
    for association in [
        "<XLink name=\"Box\" file=\"elsewhere.FCStd\"/>",
        "<XLink name=\"Box\" file=\"\"><Sub value=\"Face1\"/></XLink>",
    ] {
        let doc = document(&[
            object("Box", "Part::Feature", vec![shape("Selected.brp")]),
            object(
                "Link",
                "App::Link",
                vec![property("LinkedObject", "App::PropertyXLink", association)],
            ),
        ]);
        reject(
            &container(&doc, vec![]),
            &["external", "subelement", "unsupported"],
        );
    }
    let scaled = document(&[
        object("Box", "Part::Feature", vec![shape("Selected.brp")]),
        object(
            "Link",
            "App::Link",
            vec![
                link("Box"),
                property("Scale", "App::PropertyFloat", "<Float value=\"2\"/>"),
            ],
        ),
    ]);
    reject(&container(&scaled, vec![]), &["scaled", "unsupported"]);
    let binary = document(&[object("Box", "Part::Feature", vec![shape("Stored.bin")])]);
    reject(
        &container(
            &binary,
            vec![Member::new(
                "Stored.bin",
                b"CASCADE Topology V3\n".as_slice(),
            )],
        ),
        &["binary", "unsupported"],
    );
    for part in [
        "<Part brep=\"1\">ASCII shape text</Part>",
        "<Part binary=\"1\">AA==</Part>",
    ] {
        let doc = document(&[object(
            "Box",
            "Part::Feature",
            vec![property("Shape", "Part::PropertyPartShape", part)],
        )]);
        reject(
            &container(&doc, vec![]),
            &["inline", "binary", "file", "unsupported"],
        );
    }
}

/// XML object counts and ownership depth are checked before selected-shape expansion.
#[test]
fn object_count_and_hierarchy_depth_limits_fail_before_native_shape_expansion() {
    let objects: Vec<_> = (0..10_001)
        .map(|index| object(&format!("Object{index}"), "Part::Feature", vec![]))
        .collect();
    reject(
        &container(&document(&objects), vec![]),
        &["10000", "object declarations"],
    );
    let mut hierarchy: Vec<_> = (0..66)
        .map(|index| {
            object(
                &format!("Part{index}"),
                "App::Part",
                vec![group(&[&format!("Part{}", index + 1)])],
            )
        })
        .collect();
    hierarchy.push(object(
        "Part66",
        "Part::Feature",
        vec![shape("Selected.brp")],
    ));
    reject(
        &container(&document(&hierarchy), vec![]),
        &["64", "hierarchy"],
    );
}

/// Native topology accounting rejects a tiny carrier claiming an excessive shape table before import.
#[test]
fn addressed_brep_declared_topology_is_bounded_before_the_kernel_import() {
    let source =
        String::from_utf8(original_member("colored_visibility.FCStd", "PartShape.brp").unwrap())
            .unwrap();
    let excessive = source.replacen("TShapes 34", "TShapes 100001", 1);
    assert_ne!(excessive, source);
    let doc = document(&[object("Box", "Part::Feature", vec![shape("Large.brp")])]);
    reject(
        &container(&doc, vec![Member::new("Large.brp", excessive.into_bytes())]),
        &["aggregate topology"],
    );
}

/// Small placement-list counts, record spans and numeric scalars are validated before instancing.
#[test]
fn placement_list_large_counts_truncated_records_nonfinite_scalars_and_precision_variants_reject() {
    let doc = document(&[
        object("Box", "Part::Feature", vec![shape("Selected.brp")]),
        object(
            "Array",
            "Part::FeaturePython",
            vec![
                property("Base", "App::PropertyLink", "<Link value=\"Box\"/>"),
                property(
                    "PlacementList",
                    "App::PropertyPlacementList",
                    "<PlacementList file=\"Poses\"/>",
                ),
            ],
        ),
    ]);
    let mut one = 1_u32.to_le_bytes().to_vec();
    for value in [0., 0., 0., 0., 0., 0., 1_f64] {
        one.extend_from_slice(&value.to_le_bytes());
    }
    let mut nonfinite = one.clone();
    nonfinite[4..12].copy_from_slice(&f64::NAN.to_le_bytes());
    let controls = [
        u32::MAX.to_le_bytes().to_vec(),
        one[..one.len() - 1].to_vec(),
        [1_u32.to_le_bytes().as_slice(), &[0_u8; 28]].concat(),
        nonfinite,
    ];
    for data in controls {
        reject(
            &container(&doc, vec![Member::new("Poses", data)]),
            &["placement", "count", "nonfinite"],
        );
    }
}

/// A usable original PNG remains preferred despite unsupported native shape bytes.
#[test]
fn usable_saved_png_does_not_load_bad_native_brep_and_forced_geometry_fails() {
    let doc = document(&[object("Box", "Part::Feature", vec![shape("Bad.brp")])]);
    let bytes = container(
        &doc,
        vec![
            Member::new("Bad.brp", b"deliberately invalid native shape".as_slice()),
            Member::new(
                "thumbnails/Thumbnail.png",
                original_member("colored_visibility.FCStd", "thumbnails/Thumbnail.png").unwrap(),
            ),
        ],
    );
    let input = Control::new(&bytes, "FCStd").unwrap();
    let expected = original_png("colored_visibility.FCStd");
    assert_eq!(
        saved_preview::extract(&input.path).unwrap().unwrap(),
        expected
    );
    assert_eq!(thumbnail(&input.path, &options(128)).unwrap(), expected);
    assert!(geometry_thumbnail(&input.path, &options(128)).is_err());
    assert_eq!(cli_png(&command(&input.path, 128, &[])), expected);
    assert!(
        !command(&input.path, 128, &["--render-geometry"])
            .status
            .success()
    );
}

/// A corrupt addressed PNG falls back to actual geometry and never chooses another image member.
#[test]
fn bad_saved_png_uses_native_geometry_without_selecting_unrelated_valid_pngs() {
    let doc = document(&[object("Box", "Part::Feature", vec![shape("Selected.brp")])]);
    let bytes = container(
        &doc,
        vec![
            Member::new(
                "thumbnails/Thumbnail.png",
                b"corrupt PNG carrier".as_slice(),
            ),
            Member::new(
                "some-other.png",
                original_member("colored_visibility.FCStd", "thumbnails/Thumbnail.png").unwrap(),
            ),
        ],
    );
    let input = Control::new(&bytes, "FCStd").unwrap();
    assert!(saved_preview::extract(&input.path).is_err());
    let expected = render(&formats::load(&input.path).unwrap(), &options(128)).unwrap();
    assert_eq!(thumbnail(&input.path, &options(128)).unwrap(), expected);
    assert_eq!(cli_png(&command(&input.path, 128, &[])), expected);
    assert!(saved_preview::extract(&input.path).is_err());
}

/// Visible arrays with only a `Base` property are selected incomplete geometry.
#[test]
fn visible_array_without_a_complete_placement_list_is_an_explicit_error() {
    let base = property("Base", "App::PropertyLink", "<Link value=\"Box\"/>");
    for properties in [
        vec![base.clone()],
        vec![
            base.clone(),
            property(
                "PlacementList",
                "App::PropertyPlacementList",
                "<PlacementList file=\"Missing\"/>",
            ),
        ],
        vec![
            base,
            property(
                "PlacementList",
                "App::PropertyPlacementList",
                "<PlacementList file=\"\"/>",
            ),
        ],
    ] {
        let doc = document(&[
            object("Box", "Part::Feature", vec![shape("Selected.brp")]),
            object("Array", "Part::FeaturePython", properties),
        ]);
        reject(
            &container(&doc, vec![]),
            &["placement", "array", "unsupported", "missing"],
        );
    }
}

/// Construct one optional schema-one GUI provider with explicitly typed properties.
fn gui(name: &str, properties: &[String]) -> String {
    format!(
        "<Document SchemaVersion=\"1\"><ViewProviderData Count=\"1\"><ViewProvider name=\"{name}\"><Properties Count=\"{}\">{}</Properties></ViewProvider></ViewProviderData></Document>",
        properties.len(),
        properties.concat()
    )
}

/// Return bounded GUI/list members selecting a legacy per-face color file.
fn color_members(count: u32, colors: &[u32]) -> Vec<Member> {
    let gui = gui(
        "Box",
        &[property(
            "DiffuseColor",
            "App::PropertyColorList",
            "<ColorList file=\"Colors\"/>",
        )],
    );
    let mut data = count.to_le_bytes().to_vec();
    for color in colors {
        data.extend_from_slice(&color.to_le_bytes());
    }
    vec![
        Member::new("GuiDocument.xml", gui.into_bytes()),
        Member::new("Colors", data),
    ]
}

/// Color-list counts match real native faces and all selected list spans remain bounded.
#[test]
fn per_face_color_cardinality_truncation_and_face_record_limits_are_checked() {
    let doc = document(&[object("Box", "Part::Feature", vec![shape("Selected.brp")])]);
    reject(
        &container(&doc, color_members(2, &[0xff00_0000, 0x00ff_0000])),
        &["face color count"],
    );
    reject(
        &container(&doc, color_members(6, &[0xff00_0000])),
        &["truncated appearance"],
    );
    reject(
        &container(&doc, color_members(4097, &[])),
        &["4096", "face records"],
    );
}

/// Write one material record and three explicitly length-prefixed native version-three strings.
///
/// # Panics
/// Panics if a test attempts to author a string larger than a `u32` length field.
fn material(strings: [&[u8]; 3], transparency: f32) -> Vec<u8> {
    let mut data = 1_u32.to_le_bytes().to_vec();
    for color in [0x5555_55ff_u32, 0xcc1a_1aff, 0x8888_88ff, 0x0000_00ff] {
        data.extend_from_slice(&color.to_le_bytes());
    }
    data.extend_from_slice(&0.9_f32.to_le_bytes());
    data.extend_from_slice(&transparency.to_le_bytes());
    for string in strings {
        data.extend_from_slice(
            &u32::try_from(string.len())
                .expect("small authored material string")
                .to_le_bytes(),
        );
        data.extend_from_slice(string);
    }
    data
}

/// Return bounded GUI/list members selecting an explicitly versioned material file.
fn material_members(version: &str, data: Vec<u8>) -> Vec<Member> {
    let gui = gui(
        "Box",
        &[property(
            "ShapeAppearance",
            "App::PropertyMaterialList",
            &format!("<MaterialList file=\"Materials\" version=\"{version}\"/>"),
        )],
    );
    vec![
        Member::new("GuiDocument.xml", gui.into_bytes()),
        Member::new("Materials", data),
    ]
}

/// Version-three trailing strings are spanned without resolving texture or UUID resources.
#[test]
fn material_version_three_strings_are_validated_and_ignored_without_external_resource_access() {
    let doc = document(&[object("Box", "Part::Feature", vec![shape("Selected.brp")])]);
    let valid = material(
        [
            b"picture.png",
            b"/not-a-resource/texture.png",
            b"explicit-material-id",
        ],
        0.5,
    );
    let scene = control_scene(&container(&doc, material_members("3", valid.clone())));
    assert_eq!(scene.triangles.len(), 12);
    for index in 0..scene.triangles.len() {
        appearance(&scene, index, [204, 26, 26], 128);
    }
    reject(
        &container(&doc, material_members("2", valid.clone())),
        &["qualified version 3"],
    );
    reject(
        &container(
            &doc,
            material_members("3", valid[..valid.len() - 1].to_vec()),
        ),
        &["truncated material"],
    );
    let mut excessive = material([b"", b"", b""], 0.);
    excessive[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
    reject(
        &container(&doc, material_members("3", excessive)),
        &["material string", "one mib"],
    );
    for value in [f32::NAN, f32::INFINITY, -0.25, 1.25] {
        reject(
            &container(
                &doc,
                material_members("3", material([b"", b"", b""], value)),
            ),
            &["opacity", "finite"],
        );
    }
}

/// Alter one exact authored payload without updating its independently encoded ZIP CRC.
///
/// # Panics
/// Panics if the nonempty payload is absent from the explicitly authored archive.
fn corrupt_payload(bytes: &mut [u8], payload: &[u8]) {
    assert_ne!(payload, b"");
    let offset = bytes
        .windows(payload.len())
        .position(|window| window == payload)
        .unwrap();
    bytes[offset] ^= 1;
}

/// Full selected-member consumption rejects CRC corruption, and PNG CRC failure falls back only to geometry.
#[test]
fn selected_shape_document_and_saved_png_crc_failures_are_not_silently_accepted() {
    let doc = document(&[object("Box", "Part::Feature", vec![shape("Selected.brp")])]);
    let selected = original_member("colored_visibility.FCStd", "PartShape.brp").unwrap();
    let mut shape_crc = container(&doc, vec![]);
    corrupt_payload(&mut shape_crc, &selected);
    reject(&shape_crc, &["crc", "checksum"]);
    let png = original_member("colored_visibility.FCStd", "thumbnails/Thumbnail.png").unwrap();
    let mut png_crc = container(
        &doc,
        vec![Member::new("thumbnails/Thumbnail.png", png.clone())],
    );
    corrupt_payload(&mut png_crc, &png);
    let input = Control::new(&png_crc, "FCStd").unwrap();
    assert!(saved_preview::extract(&input.path).is_err());
    assert_eq!(
        thumbnail(&input.path, &options(128)).unwrap(),
        geometry_thumbnail(&input.path, &options(128)).unwrap()
    );
    let mut document_crc = container(&doc, vec![Member::new("thumbnails/Thumbnail.png", png)]);
    corrupt_payload(&mut document_crc, doc.as_bytes());
    let input = Control::new(&document_crc, "FCStd").unwrap();
    assert!(formats::load(&input.path).is_err());
    assert!(saved_preview::extract(&input.path).is_err());
    assert!(thumbnail(&input.path, &options(128)).is_err());
}

/// DTDs, unsupported document schemas and nonnative root identity never admit a usable saved PNG.
#[test]
fn native_document_identity_dtd_and_version_controls_reject_before_saved_or_geometry_dispatch() {
    let good = document(&[object("Box", "Part::Feature", vec![shape("Selected.brp")])]);
    for xml in [
        good.replace("SchemaVersion=\"4\"", "SchemaVersion=\"3\""),
        good.replace("FileVersion=\"1\"", "FileVersion=\"2\""),
        good.replace("Document", "ForeignDocument"),
        format!(
            "<!DOCTYPE Document [<!ENTITY unused SYSTEM \"file:///unused-external-source\">]>{good}"
        ),
    ] {
        let bytes = container(
            &xml,
            vec![Member::new(
                "thumbnails/Thumbnail.png",
                original_member("colored_visibility.FCStd", "thumbnails/Thumbnail.png").unwrap(),
            )],
        );
        let input = Control::new(&bytes, "FCStd").unwrap();
        assert!(formats::load(&input.path).is_err());
        assert!(saved_preview::extract(&input.path).is_err());
        assert!(thumbnail(&input.path, &options(128)).is_err());
    }
}

/// Encoded source and physical directory counts are bounded before allocating an archive index.
#[test]
fn encoded_source_and_physical_zip_entry_limits_are_checked_before_archive_allocation() {
    let doc = document(&[object("Box", "Part::Feature", vec![shape("Selected.brp")])]);
    let bytes = container(&doc, vec![]);
    let input = Control::new(&bytes, "FCStd").unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&input.path)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    let error = formats::load(&input.path).unwrap_err().to_string();
    assert!(error.contains("64 MiB"));
    let members: Vec<_> = (0..16_385)
        .map(|index| Member::new(format!("unused-{index}"), Vec::new()))
        .collect();
    reject(
        &container(&doc, members),
        &["entries", "entry", "records", "16384"],
    );
}

/// Legacy and version-three appearance side-members are validated in their unchanged real document owners.
#[test]
fn original_material_list_truncation_and_native_gui_version_errors_are_meaningful() {
    let mut members = original_members("bracket_1x.FCStd").unwrap();
    let list = members
        .iter_mut()
        .find(|member| member.name == "ShapeAppearance1")
        .unwrap();
    assert_eq!(list.data.len(), 148);
    list.data.pop();
    reject(
        &stored(&members).unwrap(),
        &["truncated material", "scalar"],
    );
    let mut members = original_members("colored_visibility.FCStd").unwrap();
    let gui = members
        .iter_mut()
        .find(|member| member.name == "GuiDocument.xml")
        .unwrap();
    let source = String::from_utf8(gui.data.clone()).unwrap();
    let changed = source.replacen("SchemaVersion=\"1\"", "SchemaVersion=\"2\"", 1);
    assert_ne!(changed, source);
    gui.data = changed.into_bytes();
    reject(&stored(&members).unwrap(), &["gui", "root"]);
}

/// Element counts and scale fields require their own native carrier types, even for identity values.
#[test]
fn selected_array_and_link_count_scale_carriers_cannot_spoof_their_property_types() {
    for (name, value) in [
        ("Scale", "<Float value=\"1\"/>"),
        ("ScaleVector", "<Vector x=\"1\" y=\"1\" z=\"1\"/>"),
    ] {
        let doc = document(&[
            object("Box", "Part::Feature", vec![shape("Selected.brp")]),
            object(
                "Link",
                "App::Link",
                vec![link("Box"), property(name, "App::PropertyString", value)],
            ),
        ]);
        reject(&container(&doc, vec![]), &["requires type", "property"]);
    }
    let mut poses = 1_u32.to_le_bytes().to_vec();
    for value in [0., 0., 0., 0., 0., 0., 1_f64] {
        poses.extend_from_slice(&value.to_le_bytes());
    }
    let doc = document(&[
        object("Box", "Part::Feature", vec![shape("Selected.brp")]),
        object(
            "Array",
            "Part::FeaturePython",
            vec![
                property("Base", "App::PropertyLink", "<Link value=\"Box\"/>"),
                property(
                    "PlacementList",
                    "App::PropertyPlacementList",
                    "<PlacementList file=\"Poses\"/>",
                ),
                property(
                    "ElementCount",
                    "App::PropertyString",
                    "<Integer value=\"1\"/>",
                ),
            ],
        ),
    ]);
    reject(
        &container(&doc, vec![Member::new("Poses", poses)]),
        &["requires type", "property"],
    );
}

/// Replace one literal original document carrier without changing any other original member.
///
/// # Panics
/// Panics unless the document contains exactly one occurrence of the requested literal.
fn replace_document_literal(members: &mut [Member], before: &str, after: &str) {
    let document = members
        .iter_mut()
        .find(|member| member.name == "Document.xml")
        .unwrap();
    let original = String::from_utf8(document.data.clone()).unwrap();
    assert_eq!(original.matches(before).count(), 1);
    document.data = original.replacen(before, after, 1).into_bytes();
}

/// Author the native count followed by `f64` XYZ identity vectors independently of the loader.
fn identity_scales(count: u32) -> Vec<u8> {
    let mut bytes = count.to_le_bytes().to_vec();
    for _ in 0..count {
        for _ in 0..3 {
            bytes.extend_from_slice(&1_f64.to_le_bytes());
        }
    }
    bytes
}

/// Original identity scale lists and empty or shorter all-visible masks preserve every actual instance.
#[test]
fn native_array_default_and_explicit_identity_lists_preserve_all_original_box_geometry() {
    let original = original_member("link_array.FCStd", "ScaleList").unwrap();
    assert_eq!(original, identity_scales(3));
    assert_eq!(
        hash(&original),
        "a7ab85fff3bf1e13339442f5b42cf4656dc4bc25c42239f26ec825dc5cae62ee"
    );
    for (count, bits) in [(0_u32, ""), (1, "1"), (2, "11"), (3, "111")] {
        let mut members = original_members("link_array.FCStd").unwrap();
        let scales = members
            .iter_mut()
            .find(|member| member.name == "ScaleList")
            .unwrap();
        scales.data = identity_scales(count);
        replace_document_literal(
            &mut members,
            "<BoolList value=\"\"/>",
            &format!("<BoolList value=\"{bits}\"/>"),
        );
        boxes(&control_scene(&stored(&members).unwrap()), &BOXES[4]);
    }
}

/// Per-instance scaling must fail explicitly, even when a tiny nonidentity value could look unchanged.
#[test]
fn native_array_nonidentity_and_nonfinite_f64_scales_never_render_unscaled_instances() {
    for value in [
        0_f64,
        -1.,
        0.5,
        2.,
        1. + f64::EPSILON,
        f64::NAN,
        f64::INFINITY,
    ] {
        let mut members = original_members("link_array.FCStd").unwrap();
        let scales = members
            .iter_mut()
            .find(|member| member.name == "ScaleList")
            .unwrap();
        assert_eq!(scales.data.len(), 76);
        scales.data[4..12].copy_from_slice(&value.to_le_bytes());
        let error = reject(&stored(&members).unwrap(), &["array-instance scale"]);
        assert!(
            error.contains("nonidentity") || error.contains("nonfinite"),
            "{error}"
        );
    }
}

/// Selected vector lists require exact bounded binary extents and a direct correctly typed address.
#[test]
fn native_array_scale_list_count_extent_address_and_type_controls_are_meaningful() {
    let original = original_member("link_array.FCStd", "ScaleList").unwrap();
    let mut mismatched = original.clone();
    mismatched[..4].copy_from_slice(&2_u32.to_le_bytes());
    let mut excessive = original.clone();
    excessive[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    for invalid in [
        Vec::new(),
        vec![3, 0, 0],
        original[..75].to_vec(),
        mismatched,
        excessive,
        identity_scales(4),
    ] {
        let mut members = original_members("link_array.FCStd").unwrap();
        members
            .iter_mut()
            .find(|member| member.name == "ScaleList")
            .unwrap()
            .data = invalid;
        reject(
            &stored(&members).unwrap(),
            &["scale list count", "scale list count/extent"],
        );
    }
    let mut members = original_members("link_array.FCStd").unwrap();
    members.retain(|member| member.name != "ScaleList");
    reject(
        &stored(&members).unwrap(),
        &["missing", "not found", "member"],
    );
    for (before, after) in [
        (
            "name=\"ScaleList\" type=\"App::PropertyVectorList\"",
            "name=\"ScaleList\" type=\"App::PropertyString\"",
        ),
        (
            "<VectorList file=\"ScaleList\"/>",
            "<Vector file=\"ScaleList\"/>",
        ),
        (
            "<VectorList file=\"ScaleList\"/>",
            "<Wrapped><VectorList file=\"ScaleList\"/></Wrapped>",
        ),
    ] {
        let mut members = original_members("link_array.FCStd").unwrap();
        replace_document_literal(&mut members, before, after);
        reject(
            &stored(&members).unwrap(),
            &["requires type", "missing direct vectorlist"],
        );
    }
}

/// Native false visibility bits are unsupported selected geometry; malformed masks cannot hide in defaults.
#[test]
fn native_array_visibility_bits_counts_and_carrier_types_are_validated_before_expansion() {
    for bits in ["0", "101", "110"] {
        let mut members = original_members("link_array.FCStd").unwrap();
        replace_document_literal(
            &mut members,
            "<BoolList value=\"\"/>",
            &format!("<BoolList value=\"{bits}\"/>"),
        );
        reject(
            &stored(&members).unwrap(),
            &["hidden array-instance visibility is unsupported"],
        );
    }
    for bits in ["10x".to_owned(), "1111".to_owned(), "1".repeat(10_001)] {
        let mut members = original_members("link_array.FCStd").unwrap();
        replace_document_literal(
            &mut members,
            "<BoolList value=\"\"/>",
            &format!("<BoolList value=\"{bits}\"/>"),
        );
        reject(
            &stored(&members).unwrap(),
            &["visibility bits", "visibility bit count"],
        );
    }
    for (before, after) in [
        (
            "name=\"VisibilityList\" type=\"App::PropertyBoolList\"",
            "name=\"VisibilityList\" type=\"App::PropertyString\"",
        ),
        ("<BoolList value=\"\"/>", "<Boolean value=\"true\"/>"),
        ("<BoolList value=\"\"/>", "<BoolList/>"),
    ] {
        let mut members = original_members("link_array.FCStd").unwrap();
        replace_document_literal(&mut members, before, after);
        reject(
            &stored(&members).unwrap(),
            &[
                "requires type",
                "missing direct boollist",
                "missing value attribute",
            ],
        );
    }
}
