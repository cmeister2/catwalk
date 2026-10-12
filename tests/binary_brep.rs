//! Independent standalone binary BREP topology, source integrity and placement qualification.

use catwalk3d::{
    formats,
    render::{RenderOptions, render},
    scene::{Scene, Vec3},
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::PathBuf};

/// Exact source and license hashes at the pinned original F3D revision.
const ORIGINAL_HASHES: [(&str, &str); 4] = [
    (
        "f3d.bin.brep",
        "0c1788d106be3473758bfc81013667f976d4b319378aab2e79e77eae91912265",
    ),
    (
        "f3d.brep",
        "f1a8028786df34e897e9b8cdb0cddb3d9d4e39e7431e3126092075d98a3e5201",
    ),
    (
        "F3D-LICENSE.md",
        "42e90e43576bfcc8945239179c49141c387c407a3a3c3696789d9f061cc4afb4",
    ),
    (
        "F3D-DATA_LICENSES.md",
        "4229d1453e125ace50ca1825d420340bbc6b2ee4f2e6ffabde8709052534e12a",
    ),
];
/// Eight original Cartesian vertices read independently from the unchanged ASCII source.
const ORIGINAL_POINTS: [[f64; 3]; 8] = [
    [2., -0.25, 4.5],
    [-2., 1., 1.5],
    [-2., -1.75, -0.5],
    [0., 0.25, -0.5],
    [-2., 0.25, -4.5],
    [2., 1.75, 0.5],
    [2., -1., -1.5],
    [0., -0.25, 0.5],
];
/// Original oriented triangle loops following the ASCII wire/edge/vertex references.
const ORIGINAL_FACES: [[usize; 3]; 8] = [
    [0, 1, 2],
    [3, 0, 2],
    [3, 2, 1],
    [0, 3, 1],
    [4, 5, 6],
    [7, 5, 4],
    [7, 6, 5],
    [7, 4, 6],
];

/// Locate a retained or authored input without altering the source corpus.
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/binary_brep")
        .join(name)
}

/// Compare independent analytic values using absolute numeric error.
fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

/// Compare a position with its literal independently derived placement.
fn position(actual: Vec3, expected: [f64; 3]) {
    for (a, e) in [actual.x, actual.y, actual.z].into_iter().zip(expected) {
        close(a, e);
    }
}

/// Compare scene extrema across its surfaces, segments and isolated points.
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
        close(a, e);
    }
    for (a, e) in upper.into_iter().zip(maximum) {
        close(a, e);
    }
}

/// Identify an emitted corner against all eight literal original Cartesian points.
fn original_point_id(point: Vec3) -> usize {
    ORIGINAL_POINTS
        .iter()
        .position(|expected| {
            [point.x, point.y, point.z]
                .into_iter()
                .zip(expected)
                .all(|(a, e)| (a - e).abs() < 1e-9)
        })
        .unwrap_or_else(|| panic!("unexpected original BREP corner {point:?}"))
}

/// Canonicalize only the starting corner, preserving an oriented triangle's winding.
fn cycle(mut ids: [usize; 3]) -> [usize; 3] {
    let index = (0..3).min_by_key(|&i| ids[i]).unwrap();
    ids.rotate_left(index);
    ids
}

/// Match every emitted triangle to its original source loop without relying on mesh ordering.
fn original_topology(scene: &Scene) {
    assert_eq!(scene.triangles.len(), 8);
    assert!(scene.lines.is_empty() && scene.points.is_empty());
    let expected: BTreeSet<_> = ORIGINAL_FACES.into_iter().map(cycle).collect();
    let actual: BTreeSet<_> = scene
        .triangles
        .iter()
        .map(|triangle| cycle(triangle.vertices.map(original_point_id)))
        .collect();
    assert_eq!(actual, expected);
    let mut vertices = BTreeSet::new();
    let mut area = 0.;
    let mut volume = 0.;
    for triangle in &scene.triangles {
        let [a, b, c] = triangle.vertices;
        vertices.extend(triangle.vertices.map(original_point_id));
        area += (b - a).cross(c - a).length() / 2.;
        volume += a.dot(b.cross(c)) / 6.;
    }
    assert_eq!(vertices.len(), 8);
    close(area, 55.018_917_559_102_576);
    close(volume, 12.5);
    bounds(scene, [-2., -1.75, -4.5], [2., 1.75, 4.5]);
}

/// Compare every free line endpoint, segment count and independently known total length.
fn lines(scene: &Scene, expected: &[[[f64; 3]; 2]], length: f64) {
    assert!(scene.triangles.is_empty());
    assert_eq!(scene.lines.len(), expected.len());
    let mut total = 0.;
    for (line, endpoints) in scene.lines.iter().zip(expected) {
        for (a, e) in line.vertices.into_iter().zip(endpoints) {
            position(a, *e);
        }
        total += (line.vertices[1] - line.vertices[0]).length();
    }
    close(total, length);
}

#[test]
fn retained_original_pair_and_required_licenses_have_exact_source_hashes() {
    for (name, expected) in ORIGINAL_HASHES {
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(fixture(name)).unwrap())),
            expected,
            "{name}"
        );
    }
    let mapping = fs::read_to_string(fixture("F3D-DATA_LICENSES.md")).unwrap();
    assert!(
        mapping.contains("All other datasets are licensed under the BSD-3-Clause F3D license.")
    );
    let license = fs::read_to_string(fixture("F3D-LICENSE.md")).unwrap();
    assert!(license.starts_with("BSD 3-Clause License"));
    assert!(license.contains("Copyright 2025 F3D-APP Foundation"));
}

#[test]
fn original_source_describes_two_shells_with_all_eight_triangular_faces() {
    let text = fs::read_to_string(fixture("f3d.brep")).unwrap();
    assert!(text.contains("CASCADE Topology V3, (c) Open Cascade\n"));
    assert!(text.contains("TShapes 39\n"));
    for (kind, expected) in [
        ("Ve", 8),
        ("Ed", 12),
        ("Wi", 8),
        ("Fa", 8),
        ("Sh", 2),
        ("Co", 1),
    ] {
        assert_eq!(
            text.matches(&format!("\n{kind}\n")).count(),
            expected,
            "{kind}"
        );
    }
    let binary = fs::read(fixture("f3d.bin.brep")).unwrap();
    assert!(binary.starts_with(b"\nOpen CASCADE Topology V4, (c) Open Cascade\n"));
}

#[test]
fn original_binary_and_ascii_preserve_every_vertex_oriented_face_area_and_volume() {
    for name in ["f3d.bin.brep", "f3d.brep"] {
        let scene = formats::load(&fixture(name)).unwrap();
        original_topology(&scene);
    }
}

#[test]
fn original_binary_and_ascii_render_the_same_fixed_view() {
    let options = RenderOptions {
        width: 128,
        height: 128,
        transparent: true,
        ..Default::default()
    };
    let binary = formats::load(&fixture("f3d.bin.brep")).unwrap();
    let ascii = formats::load(&fixture("f3d.brep")).unwrap();
    assert_eq!(
        render(&binary, &options).unwrap(),
        render(&ascii, &options).unwrap()
    );
}

#[test]
fn version_one_isolated_points_retain_literal_root_translation() {
    let scene = formats::load(&fixture("brep-binary-v1-points.brep")).unwrap();
    assert!(scene.triangles.is_empty() && scene.lines.is_empty());
    assert_eq!(scene.points.len(), 3);
    for (point, expected) in scene
        .points
        .iter()
        .zip([[3., -2., 1.], [5., -2., 1.], [3., -1., 1.]])
    {
        position(point.position, expected);
    }
    bounds(&scene, [3., -2., 1.], [5., -1., 1.]);
}

#[test]
fn version_two_free_wire_evaluates_its_rotated_curve_carrier() {
    let scene = formats::load(&fixture("brep-binary-v2-wire.brep")).unwrap();
    assert!(scene.points.is_empty());
    lines(
        &scene,
        &[[[1., 2., 3.], [1., 4., 3.]], [[1., 4., 3.], [0., 4., 3.]]],
        3.,
    );
    bounds(&scene, [0., 2., 3.], [1., 4., 3.]);
}

#[test]
fn version_three_root_and_reflected_carrier_keep_free_wires_and_isolated_points() {
    let scene = formats::load(&fixture("brep-binary-v3-placed-wire-points.brep")).unwrap();
    lines(
        &scene,
        &[
            [[2., -1., 4.], [2., -3., 4.]],
            [[2., -3., 4.], [1., -3., 4.]],
        ],
        3.,
    );
    assert_eq!(scene.points.len(), 1);
    position(scene.points[0].position, [4., 5., 3.]);
    bounds(&scene, [1., -3., 3.], [4., 5., 4.]);
}

#[test]
fn version_four_wire_instances_keep_child_placement_and_root_scale() {
    let scene = formats::load(&fixture("brep-binary-v4-repeated-wire.brep")).unwrap();
    assert!(scene.points.is_empty());
    lines(
        &scene,
        &[
            [[12., 4., 5.], [16., 4., 5.]],
            [[16., 4., 5.], [16., 6., 5.]],
            [[20., 4., 7.], [24., 4., 7.]],
            [[24., 4., 7.], [24., 6., 7.]],
        ],
        12.,
    );
    bounds(&scene, [12., 4., 5.], [24., 6., 7.]);
}
