//! Independent static COLLADA primitive, accessor and affine-placement qualification.

#[path = "support/digest_hex.rs"]
/// Format digest bytes with the unchanged lowercase hexadecimal comparison protocol.
mod digest_hex;

use catwalk3d::{
    formats,
    scene::{Scene, Vec3},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

/// Sequence making temporary test-document paths unique within this process.
static NEXT_INPUT: AtomicU64 = AtomicU64::new(0);
/// Original input and license hashes independently verified at the pinned CTS revision.
const RETAINED: [(&str, &str); 3] = [
    (
        "node_lookat4.dae",
        "48f2ac8f317f591a4af93910896f967dc7541fb5582342f5e4178a1028a8d429",
    ),
    (
        "node_skew_cube.dae",
        "6b5f6a0b3b9e859e3a20aada2efc12adfaa779369ba2d7f87fc0889f4aa24c6b",
    ),
    (
        "KHRONOS-CTS-LICENSE",
        "0c416509cf218accdbc54afc342271935b0ecdc1398283129a32375aa7bcc9a4",
    ),
];
/// Original six CTS pyramid triangles, independently read from its index list.
const PYRAMID_FACES: [[usize; 3]; 6] = [
    [0, 1, 4],
    [1, 2, 4],
    [2, 3, 4],
    [3, 0, 4],
    [0, 2, 1],
    [0, 3, 2],
];
/// Original twelve CTS cube triangles, taking POSITION from each two-index tuple.
const CUBE_FACES: [[usize; 3]; 12] = [
    [0, 1, 2],
    [2, 1, 3],
    [2, 3, 4],
    [4, 3, 5],
    [4, 5, 6],
    [6, 5, 7],
    [6, 7, 0],
    [0, 7, 1],
    [1, 7, 3],
    [3, 7, 5],
    [6, 0, 4],
    [4, 0, 2],
];
/// Independently wound authored tetrahedron faces shared by the affine witnesses.
const TETRA_FACES: [[usize; 3]; 4] = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [2, 0, 3]];

/// Temporary document owner that removes only its own generated directory.
struct Input {
    /// Unique directory containing one .dae file.
    directory: PathBuf,
}

impl Input {
    /// Write one small authored, invalid or unsupported test document for the loading API.
    fn new(source: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "catwalk-collada-case-{}-{}",
            std::process::id(),
            NEXT_INPUT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("case.dae"), source).unwrap();
        Self { directory }
    }

    /// Decode the document while its temporary source remains available.
    fn load(&self) -> catwalk3d::Result<Scene> {
        formats::load(&self.directory.join("case.dae"))
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// Locate a source model or original notice in the dedicated corpus directory.
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/collada_primitives")
        .join(name)
}

/// Compare independently derived geometry values using absolute error.
fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} != {expected}"
    );
}

/// Compare a position with a literal expected placement.
fn position(actual: Vec3, expected: [f64; 3]) {
    for (a, e) in [actual.x, actual.y, actual.z].into_iter().zip(expected) {
        close(a, e, 1e-11);
    }
}

/// Require all selected triangle corners to match source index and placement witnesses.
fn triangles(scene: &Scene, faces: &[[usize; 3]], points: &[[f64; 3]]) {
    assert_eq!(scene.triangles.len(), faces.len());
    assert!(scene.lines.is_empty() && scene.points.is_empty());
    for (triangle, face) in scene.triangles.iter().zip(faces) {
        for (p, index) in triangle.vertices.into_iter().zip(face) {
            position(p, points[*index]);
        }
    }
}

/// Accumulate independent Euclidean area and signed algebraic volume.
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

/// Compare position extrema against independent source geometry bounds.
fn bounds(scene: &Scene, minimum: [f64; 3], maximum: [f64; 3]) {
    let mut lower = [f64::INFINITY; 3];
    let mut upper = [f64::NEG_INFINITY; 3];
    for p in scene.positions() {
        for (axis, value) in [p.x, p.y, p.z].into_iter().enumerate() {
            lower[axis] = lower[axis].min(value);
            upper[axis] = upper[axis].max(value);
        }
    }
    for (a, e) in lower.into_iter().zip(minimum) {
        close(a, e, 1e-11);
    }
    for (a, e) in upper.into_iter().zip(maximum) {
        close(a, e, 1e-11);
    }
}

#[test]
fn retained_primary_models_and_license_preserve_exact_source_bytes() {
    for (name, expected) in RETAINED {
        assert_eq!(
            format!(
                "{:x}",
                digest_hex::Hex(Sha256::digest(fs::read(fixture(name)).unwrap()))
            ),
            expected,
            "{name}"
        );
    }
    for name in ["node_lookat4.dae", "node_skew_cube.dae"] {
        let source = fs::read_to_string(fixture(name)).unwrap();
        assert!(source.contains("Maya 8.5 | ColladaMaya v3.02 | FCollada v3.2"));
        assert!(source.contains("Khronos"));
    }
}

#[test]
fn cts_lookat_pyramid_matches_literal_corners_area_and_inward_winding() {
    let scene = formats::load(&fixture("node_lookat4.dae")).unwrap();
    // Literal positions follow the original diamond-base pyramid, look-at basis,
    // centimetre units and Y-up conversion, independently of importer helpers.
    let points = [
        [-0.75, 0.5, 0.],
        [-0.75, 0., -0.5],
        [-0.75, -0.5, 0.],
        [-0.75, 0., 0.5],
        [1.25, 0., 0.],
    ];
    triangles(&scene, &PYRAMID_FACES, &points);
    bounds(&scene, [-0.75, -0.5, -0.5], [1.25, 0.5, 0.5]);
    let (area, volume) = area_volume(&scene);
    close(area, 1f64.midpoint(33f64.sqrt()), 1e-11);
    close(volume, -1. / 3., 1e-11);
}

#[test]
fn cts_skew_cube_displaces_along_the_second_authored_axis() {
    let scene = formats::load(&fixture("node_skew_cube.dae")).unwrap();
    let k = std::f64::consts::FRAC_1_SQRT_2;
    let points = [
        [-0.5, k - 0.5, -0.5],
        [0.5, -0.5, -0.5],
        [-0.5, -0.5, 0.5],
        [0.5, -0.5 - k, 0.5],
        [-0.5, 0.5, 0.5],
        [0.5, 0.5 - k, 0.5],
        [-0.5, 0.5 + k, -0.5],
        [0.5, 0.5, -0.5],
    ];
    triangles(&scene, &CUBE_FACES, &points);
    bounds(&scene, [-0.5, -0.5 - k, -0.5], [0.5, 0.5 + k, 0.5]);
    let (area, volume) = area_volume(&scene);
    close(area, 4. + 2. * 2f64.sqrt(), 1e-11);
    close(volume, 1., 1e-11);
}

#[test]
fn holed_polygon_preserves_both_openings_area_plane_and_outer_winding() {
    let scene = formats::load(&fixture("dae-holed-plane.dae")).unwrap();
    assert_eq!(scene.triangles.len(), 14);
    assert!(scene.lines.is_empty());
    bounds(&scene, [0., 0., 0.], [4., 3., 1.375]);
    close(area_volume(&scene).0, 10.5 * 69f64.sqrt() / 8., 1e-11);
    for triangle in &scene.triangles {
        let [a, b, c] = triangle.vertices;
        assert!((b - a).cross(c - a).dot(Vec3::new(-0.25, -0.125, 1.)) > 0.);
        for p in triangle.vertices {
            close(p.z, p.x / 4. + p.y / 8., 1e-11);
        }
        let center = (a + b + c) / 3.;
        assert!(!(center.x > 0.5 && center.x < 1.5 && center.y > 0.5 && center.y < 1.5));
        assert!(!(center.x > 2.5 && center.x < 3. && center.y > 1. && center.y < 2.));
    }
}

#[test]
fn line_pairs_and_closed_strips_preserve_endpoints_colors_units_and_instances() {
    let scene = formats::load(&fixture("dae-wire-instances.dae")).unwrap();
    assert!(scene.triangles.is_empty() && scene.points.is_empty());
    assert_eq!(scene.lines.len(), 24);
    bounds(&scene, [0., 0., 0.], [0.4, 0.3, 0.25]);
    let length: f64 = scene
        .lines
        .iter()
        .map(|line| (line.vertices[1] - line.vertices[0]).length())
        .sum();
    close(
        length,
        0.1 * (28. + 2. * 21f64.sqrt() + 4. * 6f64.sqrt()),
        1e-11,
    );
    let edges = [
        [0, 1],
        [1, 2],
        [2, 3],
        [3, 0],
        [0, 4],
        [1, 4],
        [2, 4],
        [3, 4],
        [0, 1],
        [1, 2],
        [2, 3],
        [3, 0],
    ];
    let first = [
        [0., 0., 0.],
        [0.2, 0., 0.],
        [0.2, 0.1, 0.],
        [0., 0.1, 0.],
        [0.1, 0.05, 0.2],
    ];
    let second = [
        [0.4, 0.1, 0.05],
        [0.2, 0.1, 0.05],
        [0.2, 0.3, 0.05],
        [0.4, 0.3, 0.05],
        [0.3, 0.2, 0.25],
    ];
    let colors = [
        [127, 127, 0],
        [0, 127, 127],
        [127, 127, 127],
        [255, 127, 0],
        [255, 0, 127],
        [127, 127, 127],
        [127, 0, 255],
        [255, 127, 127],
        [127, 127, 0],
        [0, 127, 127],
        [127, 127, 127],
        [255, 127, 0],
    ];
    for (instance, points) in [first, second].into_iter().enumerate() {
        for (index, edge) in edges.into_iter().enumerate() {
            let line = &scene.lines[instance * 12 + index];
            for (actual, vertex) in line.vertices.into_iter().zip(edge) {
                position(actual, points[vertex]);
            }
            assert_eq!(line.color, Some(colors[index]));
        }
    }
}

#[test]
fn placed_lookat_and_nonbinding_accessor_names_match_literal_tetrahedron() {
    let scene = formats::load(&fixture("dae-lookat-placement.dae")).unwrap();
    triangles(
        &scene,
        &TETRA_FACES,
        &[
            [0.005, -0.004, 0.],
            [0.005, -0.008, 0.],
            [0.005, -0.004, 0.009],
            [0.001, -0.004, 0.],
        ],
    );
    bounds(&scene, [0.001, -0.008, 0.], [0.005, -0.004, 0.009]);
    let (area, volume) = area_volume(&scene);
    close(area, (44. + 712f64.sqrt()) * 1e-6, 1e-14);
    close(volume, 24e-9, 1e-16);
}

#[test]
fn nonorthogonal_angular_skew_has_the_independent_unit_shear_geometry() {
    let scene = formats::load(&fixture("dae-nonorthogonal-skew.dae")).unwrap();
    triangles(
        &scene,
        &TETRA_FACES,
        &[[0., 0., 0.], [2., 0., 0.], [3., 3., 0.], [0., 0., 1.]],
    );
    bounds(&scene, [0., 0., 0.], [3., 3., 1.]);
    let (area, volume) = area_volume(&scene);
    close(area, 4. + 46f64.sqrt() / 2. + 3. * 2f64.sqrt() / 2., 1e-11);
    close(volume, 1., 1e-11);
}

#[test]
fn malformed_wire_counts_indices_slots_and_offsets_fail_explicitly() {
    let source = fs::read_to_string(fixture("dae-wire-instances.dae")).unwrap();
    for (before, after) in [
        ("<lines count=\"8\">", "<lines count=\"7\">"),
        ("<linestrips count=\"1\">", "<linestrips count=\"2\">"),
        ("<p>0 0 1 1 1 1", "<p>99 0 1 1 1 1"),
        ("name=\"first\" type=\"float\"", "type=\"float\""),
        (
            "semantic=\"VERTEX\" source=\"#vertices\" offset=\"0\"",
            "semantic=\"VERTEX\" source=\"#vertices\" offset=\"64\"",
        ),
    ] {
        assert!(source.contains(before));
        let input = Input::new(&source.replacen(before, after, 1));
        assert!(input.load().is_err(), "accepted {after}");
    }
}

#[test]
fn invalid_or_unsupported_holes_fail_explicitly() {
    let source = fs::read_to_string(fixture("dae-holed-plane.dae")).unwrap();
    for (before, after) in [
        ("<polygons count=\"1\">", "<polygons count=\"2\">"),
        ("<h>4 5 6 7</h>", "<h>4 5 6 99</h>"),
        (".5 .5 .1875", "5 .5 1.3125"),
        (".5 .5 .1875", ".5 .5 .4"),
        ("<h>4 5 6 7</h>", "<h>4 6 5 7</h>"),
        ("<h>4 5 6 7</h><h>8 9 10 11</h>", ""),
    ] {
        assert!(source.contains(before));
        let input = Input::new(&source.replacen(before, after, 1));
        assert!(input.load().is_err(), "accepted {after}");
    }
}

#[test]
fn undefined_lookat_and_skew_axes_or_angles_fail_explicitly() {
    let source = fs::read_to_string(fixture("dae-lookat-placement.dae")).unwrap();
    for invalid in ["1 2 3 1 2 3 1 1 0", "1 2 3 2 2 3 1 0 0"] {
        let input = Input::new(&source.replace("1 2 3 2 2 3 1 1 0", invalid));
        assert!(input.load().is_err(), "accepted lookat {invalid}");
    }
    let source = fs::read_to_string(fixture("dae-nonorthogonal-skew.dae")).unwrap();
    for invalid in ["30 0 0 0 1 0 0", "30 1 0 0 1 0 0", "90 0 1 0 1 0 0"] {
        let input = Input::new(&source.replace("18.43494882292201 1 1 0 1 0 0", invalid));
        assert!(input.load().is_err(), "accepted skew {invalid}");
    }
}
