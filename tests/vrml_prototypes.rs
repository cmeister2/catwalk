//! Independent geometry, source and headless-image checks for VRML prototypes and billboards.

#[path = "support/digest_hex.rs"]
/// Format digest bytes with the unchanged lowercase hexadecimal comparison protocol.
mod digest_hex;

use catwalk3d::{
    formats,
    render::{RenderOptions, render, write_png},
    scene::{Point, Scene, Triangle, Vec3},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Remove a test-local source and previous thumbnail even after a failing assertion.
struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    /// Create an isolated directory for the invalid-input subprocess test.
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("catwalk-vrml-invalid-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Resolve a retained VRML example or its upstream notice.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/vrml_prototypes")
        .join(name)
}

/// Compare an independently calculated scalar with room for trigonometric rounding.
fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-10, "{actual} != {expected}");
}

/// Compare a position component by component, rejecting non-finite coordinates.
fn position_near(actual: Vec3, expected: Vec3) {
    assert!(actual.is_finite());
    for (actual, expected) in [actual.x, actual.y, actual.z]
        .into_iter()
        .zip([expected.x, expected.y, expected.z])
    {
        near(actual, expected);
    }
}

/// Check oriented triangles without depending on the emitter's starting corner.
fn triangle_near(actual: &Triangle, expected: &Triangle) {
    assert_eq!(actual.color, expected.color);
    let matching = (0..3).any(|rotation| {
        (0..3).all(|index| {
            (actual.vertices[index] - expected.vertices[(index + rotation) % 3]).length() < 1e-10
        })
    });
    assert!(matching, "{:?} != {:?}", actual.vertices, expected.vertices);
}

/// Compute the bounds independently over emitted primitive positions.
fn bounds(scene: &Scene) -> [Vec3; 2] {
    scene.positions().fold(
        [
            Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
        ],
        |[lower, upper], position| {
            [
                Vec3::new(
                    lower.x.min(position.x),
                    lower.y.min(position.y),
                    lower.z.min(position.z),
                ),
                Vec3::new(
                    upper.x.max(position.x),
                    upper.y.max(position.y),
                    upper.z.max(position.z),
                ),
            ]
        },
    )
}

/// Compare both corners of an independently specified bounding box.
fn assert_bounds(scene: &Scene, expected: [Vec3; 2]) {
    for (actual, expected) in bounds(scene).into_iter().zip(expected) {
        position_near(actual, expected);
    }
}

/// Sum mesh triangle areas without using decoder topology helpers.
fn area(triangles: impl IntoIterator<Item = impl std::borrow::Borrow<Triangle>>) -> f64 {
    triangles
        .into_iter()
        .map(|triangle| {
            let [a, b, c] = triangle.borrow().vertices;
            (b - a).cross(c - a).length() * 0.5
        })
        .sum()
}

/// Sum oriented tetrahedron volumes; callers use this only for closed surfaces.
fn signed_volume(triangles: impl IntoIterator<Item = impl std::borrow::Borrow<Triangle>>) -> f64 {
    triangles
        .into_iter()
        .map(|triangle| {
            let [a, b, c] = triangle.borrow().vertices;
            a.dot(b.cross(c)) / 6.
        })
        .sum()
}

/// Determine whether every triangle corner belongs to one expected box's eight corners.
fn belongs_to_box(triangle: &Triangle, center: Vec3, dimensions: Vec3) -> bool {
    triangle.vertices.into_iter().all(|point| {
        let delta = point - center;
        [
            (delta.x, dimensions.x),
            (delta.y, dimensions.y),
            (delta.z, dimensions.z),
        ]
        .into_iter()
        .all(|(coordinate, dimension)| (coordinate.abs() - dimension / 2.).abs() < 1e-10)
    })
}

/// Verify all eight corners, all six outward faces, surface area and closed volume of one box.
fn assert_box(triangles: &[&Triangle], center: Vec3, dimensions: Vec3) {
    assert_eq!(triangles.len(), 12);
    let mut faces = [0; 6];
    let mut corners = 0_u8;
    for triangle in triangles {
        assert!(belongs_to_box(triangle, center, dimensions));
        for vertex in triangle.vertices {
            let delta = vertex - center;
            let index = usize::from(delta.x > 0.)
                + 2 * usize::from(delta.y > 0.)
                + 4 * usize::from(delta.z > 0.);
            corners |= 1 << index;
        }
        let [a, b, c] = triangle.vertices;
        let normal = (b - a).cross(c - a);
        assert!(normal.dot((a + b + c) / 3. - center) > 0.);
        let mut face_count = 0;
        for (axis, (origin, dimension)) in [
            (center.x, dimensions.x),
            (center.y, dimensions.y),
            (center.z, dimensions.z),
        ]
        .into_iter()
        .enumerate()
        {
            for (side, sign) in [-1., 1.].into_iter().enumerate() {
                let plane = origin + sign * dimension / 2.;
                if triangle.vertices.into_iter().all(|point| {
                    let coordinate = [point.x, point.y, point.z][axis];
                    (coordinate - plane).abs() < 1e-10
                }) {
                    faces[2 * axis + side] += 1;
                    face_count += 1;
                }
            }
        }
        assert_eq!(face_count, 1);
    }
    assert_eq!(corners, u8::MAX);
    assert_eq!(faces, [2; 6]);
    near(
        area(triangles.iter().copied()),
        2. * (dimensions.x * dimensions.y
            + dimensions.x * dimensions.z
            + dimensions.y * dimensions.z),
    );
    near(
        signed_volume(triangles.iter().copied()),
        dimensions.x * dimensions.y * dimensions.z,
    );
}

/// Return the typed-array case's literal scene after source-space placements and Y-up conversion.
fn typed_arrays_scene() -> Scene {
    Scene {
        triangles: vec![
            Triangle {
                vertices: [
                    Vec3::new(-3., 0., 0.),
                    Vec3::new(-1., 0., 0.),
                    Vec3::new(-3., 0., 3.),
                ],
                color: Some([255, 0, 0]),
            },
            Triangle {
                vertices: [
                    Vec3::new(2., 0., 1.),
                    Vec3::new(3., 0., 1.),
                    Vec3::new(2., -2., 1.),
                ],
                color: Some([85, 85, 85]),
            },
        ],
        ..Scene::default()
    }
}

/// Return literal constrained, free and reused billboard occurrences for the fixed camera.
fn billboard_scene() -> Scene {
    let root_two = 2_f64.sqrt();
    let root_three = 3_f64.sqrt();
    let root_six = 6_f64.sqrt();
    let root_ten = 10_f64.sqrt();
    Scene {
        triangles: vec![
            Triangle {
                vertices: [
                    Vec3::new(-4., 0., 0.),
                    Vec3::new(-4. + root_two, root_two, 0.),
                    Vec3::new(-4., 0., 3.),
                ],
                color: Some([255, 64, 64]),
            },
            Triangle {
                vertices: [
                    Vec3::new(3., 0., 0.),
                    Vec3::new(3. + root_two, root_two, 0.),
                    Vec3::new(3. - 3. / root_six, 3. / root_six, 6. / root_six),
                ],
                color: Some([64, 255, 64]),
            },
            Triangle {
                vertices: [
                    Vec3::new(-4., -4., 0.),
                    Vec3::new(-4. + 2. / root_ten, -4. + 18. / root_ten, 0.),
                    Vec3::new(-4., -4., 6.),
                ],
                color: Some([255, 64, 64]),
            },
        ],
        points: vec![
            Point {
                position: Vec3::new(-4. + root_two, -root_two, 0.),
                color: Some([255, 64, 64]),
            },
            Point {
                position: Vec3::new(3. + 2. / root_three, -2. / root_three, 2. / root_three),
                color: Some([64, 255, 64]),
            },
            Point {
                position: Vec3::new(-4. + 6. / root_ten, -4. - 6. / root_ten, 0.),
                color: Some([255, 64, 64]),
            },
        ],
        ..Scene::default()
    }
}

/// Render through the actual CLI with all display-related variables removed.
fn cli_png(input: &Path) -> Vec<u8> {
    let output = Command::new(env!("CARGO_BIN_EXE_catwalk"))
        .arg(input)
        .args([
            "--render-geometry",
            "--output",
            "-",
            "--size",
            "256",
            "--transparent",
        ])
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}: {}",
        input.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stderr, [] as [u8; 0]);
    output.stdout
}

#[test]
fn web3d_models_and_license_retain_original_published_bytes() {
    for (name, size, hash) in [
        (
            "Figure31_9SpinGroupPrototype.wrl",
            3_154,
            "a054724500cace784c4e04af48d3fe283496eedb84236d978ab775b1c8a6c4d1",
        ),
        (
            "ProtoInstanceExample.wrl",
            1_550,
            "cfe5ea5413f73180f2d2501c9d6d5d408e861a9ba2b3fb46ba5fe6a8c572df4d",
        ),
        (
            "WEB3D-LICENSE.txt",
            1_663,
            "0c90db51ca37d07075567650ac50d8f338d462c4b0f6ae449b6403b6eb237e4c",
        ),
    ] {
        let bytes = fs::read(fixture(name)).unwrap();
        assert_eq!(bytes.len(), size, "{name}");
        assert_eq!(
            format!("{:x}", digest_hex::Hex(Sha256::digest(bytes))),
            hash,
            "{name}"
        );
    }
}

#[test]
fn original_spingroup_keeps_caller_nodes_and_only_the_first_prototype_body() {
    let scene = formats::load(&fixture("Figure31_9SpinGroupPrototype.wrl")).unwrap();
    assert_eq!(scene.triangles.len(), 36);
    assert!(scene.lines.is_empty() && scene.points.is_empty() && scene.appearances.is_empty());
    for (dimensions, color) in [
        (Vec3::new(25., 2., 2.), [0, 255, 77]),
        (Vec3::new(2., 2., 25.), [0, 255, 77]),
        (Vec3::new(2., 25., 2.05), [0, 77, 255]),
    ] {
        let triangles: Vec<_> = scene
            .triangles
            .iter()
            .filter(|triangle| {
                triangle.color == Some(color)
                    && belongs_to_box(triangle, Vec3::default(), dimensions)
            })
            .collect();
        assert_box(&triangles, Vec3::default(), dimensions);
    }
    assert_bounds(
        &scene,
        [Vec3::new(-12.5, -12.5, -12.5), Vec3::new(12.5, 12.5, 12.5)],
    );
    near(area(&scene.triangles), 626.7);
    // These are three overlapping closed boxes, so this is summed signed volume, not their union.
    near(signed_volume(&scene.triangles), 302.5);
}

#[test]
fn original_proto_instance_uses_typed_dimensions_and_overridden_appearance() {
    let scene = formats::load(&fixture("ProtoInstanceExample.wrl")).unwrap();
    assert_eq!(scene.triangles.len(), 128);
    assert!(scene.lines.is_empty() && scene.points.is_empty() && scene.appearances.is_empty());
    let mut rings = [[false; 32]; 2];
    let mut centers = [false; 2];
    let mut faces = [0; 3];
    for triangle in &scene.triangles {
        assert_eq!(triangle.color, Some([0, 255, 0]));
        let [a, b, c] = triangle.vertices;
        let normal = (b - a).cross(c - a);
        let midpoint = (a + b + c) / 3.;
        if (a.z - b.z).abs() < 1e-10 && (a.z - c.z).abs() < 1e-10 {
            faces[usize::from(a.z > 0.)] += 1;
            assert!(normal.z * midpoint.z > 0.);
        } else {
            faces[2] += 1;
            assert!(normal.x * midpoint.x + normal.y * midpoint.y > 0.);
        }
        for point in triangle.vertices {
            near(point.z.abs(), 1.5);
            let level = usize::from(point.z > 0.);
            let radius = point.x.hypot(point.y);
            if radius < 1e-10 {
                near(radius, 0.);
                centers[level] = true;
            } else {
                near(radius, 4.);
                let matched = (0_u32..32).find(|&sector| {
                    let angle = f64::from(sector) * std::f64::consts::TAU / 32.;
                    (point.x - 4. * angle.cos()).abs() < 1e-10
                        && (point.y - 4. * angle.sin()).abs() < 1e-10
                });
                let sector = usize::try_from(matched.expect("unexpected cylinder vertex")).unwrap();
                rings[level][sector] = true;
            }
        }
    }
    assert!(rings.into_iter().flatten().all(|present| present));
    assert_eq!(centers, [true; 2]);
    assert_eq!(faces, [32, 32, 64]);
    assert_bounds(&scene, [Vec3::new(-4., -4., -1.5), Vec3::new(4., 4., 1.5)]);
    near(area(&scene.triangles), 175.163_408_645_360_22);
    near(signed_volume(&scene.triangles), 149.829_367_308_386_5);
}

#[test]
fn typed_arrays_apply_defaults_and_per_instance_vector_index_color_overrides() {
    let actual = formats::load(&fixture("vrml-proto-typed-arrays.vrml")).unwrap();
    let expected = typed_arrays_scene();
    assert_eq!(actual.triangles.len(), 2);
    assert!(actual.lines.is_empty() && actual.points.is_empty() && actual.appearances.is_empty());
    for (actual, expected) in actual.triangles.iter().zip(&expected.triangles) {
        triangle_near(actual, expected);
    }
    near(area(&actual.triangles), 4.);
    assert_bounds(&actual, [Vec3::new(-3., -2., 0.), Vec3::new(3., 0., 3.)]);
}

#[test]
fn first_body_defines_geometry_and_material_type_without_rendering_auxiliary_nodes() {
    let scene = formats::load(&fixture("wrl-proto-first-body.wrl")).unwrap();
    assert_eq!(scene.triangles.len(), 24);
    assert!(scene.lines.is_empty() && scene.points.is_empty() && scene.appearances.is_empty());
    for (center, dimensions, color) in [
        (
            Vec3::new(-3., 0., 0.),
            Vec3::new(2., 3., 1.),
            [64, 128, 255],
        ),
        (
            Vec3::new(3., -2., 1.),
            Vec3::new(1., 1., 2.),
            [255, 128, 64],
        ),
    ] {
        let triangles: Vec<_> = scene
            .triangles
            .iter()
            .filter(|triangle| triangle.color == Some(color))
            .collect();
        assert_box(&triangles, center, dimensions);
    }
    near(area(&scene.triangles), 32.);
    near(signed_volume(&scene.triangles), 8.);
    assert_bounds(
        &scene,
        [Vec3::new(-4., -2.5, -0.5), Vec3::new(3.5, 1.5, 2.)],
    );
}

#[test]
fn nested_private_def_use_instances_keep_caller_definitions_and_is_scopes() {
    let scene = formats::load(&fixture("wrl-proto-nested-scopes.wrl")).unwrap();
    assert_eq!(scene.triangles.len(), 28);
    assert!(scene.lines.is_empty() && scene.points.is_empty() && scene.appearances.is_empty());
    let expected_red = [
        [
            Vec3::new(-4., 0., 0.),
            Vec3::new(-3., 0., 0.),
            Vec3::new(-4., 0., 1.),
        ],
        [
            Vec3::new(-2., 0., 0.),
            Vec3::new(-1., 0., 0.),
            Vec3::new(-2., 0., 1.),
        ],
        [
            Vec3::new(2., -1., 1.),
            Vec3::new(3., -1., 1.),
            Vec3::new(2., -1., 2.),
        ],
        [
            Vec3::new(4., -1., 1.),
            Vec3::new(5., -1., 1.),
            Vec3::new(4., -1., 2.),
        ],
    ];
    let red: Vec<_> = scene
        .triangles
        .iter()
        .filter(|triangle| triangle.color == Some([255, 0, 0]))
        .collect();
    assert_eq!(red.len(), expected_red.len());
    for (actual, vertices) in red.into_iter().zip(expected_red) {
        triangle_near(
            actual,
            &Triangle {
                vertices,
                color: Some([255, 0, 0]),
            },
        );
    }
    for center in [Vec3::new(-4., 0., 0.), Vec3::new(-4., -2., 0.)] {
        let triangles: Vec<_> = scene
            .triangles
            .iter()
            .filter(|triangle| {
                triangle.color == Some([0, 255, 0])
                    && belongs_to_box(triangle, center, Vec3::new(1., 1., 1.))
            })
            .collect();
        assert_box(&triangles, center, Vec3::new(1., 1., 1.));
    }
    near(area(&scene.triangles), 14.);
    assert_bounds(
        &scene,
        [Vec3::new(-4.5, -2.5, -0.5), Vec3::new(5., 0.5, 2.)],
    );
}

#[test]
fn billboards_rotate_asymmetric_geometry_per_occurrence_and_parent_transform() {
    let actual = formats::load(&fixture("wrl-billboard-occurrences.wrl")).unwrap();
    let expected = billboard_scene();
    assert_eq!(actual.triangles.len(), 3);
    assert_eq!(actual.points.len(), 3);
    assert!(actual.lines.is_empty() && actual.appearances.is_empty());
    for (actual, expected) in actual.triangles.iter().zip(&expected.triangles) {
        triangle_near(actual, expected);
    }
    for (actual, expected) in actual.points.iter().zip(&expected.points) {
        assert_eq!(actual.color, expected.color);
        position_near(actual.position, expected.position);
    }
    near(area(&actual.triangles), 6. + 6. * 8.2_f64.sqrt());
    assert_bounds(
        &actual,
        [
            Vec3::new(-4., -4. - 6. / 10_f64.sqrt(), 0.),
            Vec3::new(3. + 2_f64.sqrt(), -4. + 18. / 10_f64.sqrt(), 6.),
        ],
    );
}

#[test]
fn headless_cli_images_match_independently_constructed_literal_scenes() {
    let options = RenderOptions {
        width: 256,
        height: 256,
        transparent: true,
        ..RenderOptions::default()
    };
    for (name, expected_scene) in [
        ("vrml-proto-typed-arrays.vrml", typed_arrays_scene()),
        ("wrl-billboard-occurrences.wrl", billboard_scene()),
    ] {
        let actual = image::load_from_memory(&cli_png(&fixture(name)))
            .unwrap()
            .to_rgba8();
        let expected = render(&expected_scene, &options).unwrap();
        assert_eq!(actual.dimensions(), (256, 256));
        assert!(actual.pixels().filter(|pixel| pixel[3] > 0).count() > 100);
        for (actual, expected) in actual.pixels().zip(expected.pixels()) {
            // Premultiply translucent edge RGB; trigonometric rounding may differ by two byte levels.
            for channel in 0..3 {
                let actual = u16::from(actual[channel]) * u16::from(actual[3]) / 255;
                let expected = u16::from(expected[channel]) * u16::from(expected[3]) / 255;
                assert!(actual.abs_diff(expected) <= 2, "{name}");
            }
            assert!(actual[3].abs_diff(expected[3]) <= 2, "{name}");
        }
    }
}

#[test]
fn invalid_typed_bindings_and_billboard_axes_preserve_existing_output() {
    let directory = TemporaryDirectory::new();
    let input = directory.0.join("invalid.wrl");
    let output = directory.0.join("previous.png");
    let mut previous = Vec::new();
    write_png(
        &image::RgbaImage::from_pixel(2, 2, image::Rgba([5, 17, 29, 255])),
        &mut previous,
    )
    .unwrap();
    for source in [
        "PROTO Bad [field SFColor tint 1 0 0] {Transform {translation IS tint children Shape {geometry Box {}}}} Bad {}",
        "PROTO Bad [field SFFloat height 1] {Shape {geometry Cylinder {height IS missing}}} Bad {}",
        "PROTO Bad [field SFVec3f size 1 1 1] {Shape {geometry Box {size IS size}}} Bad {size [1 2 3]}",
        "PROTO Bad [exposedField SFVec3f size 1 1 1] {Shape {geometry Box {size IS size}}} Bad {}",
        "Billboard {axisOfRotation 1 1 1 children Shape {geometry Box {}}}",
        "EXTERNPROTO Remote [] [\"remote.wrl#Remote\"] Remote {}",
        "Script {url [\"javascript:print(1)\"]}",
    ] {
        fs::write(&input, format!("#VRML V2.0 utf8\n{source}\n")).unwrap();
        fs::write(&output, &previous).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_catwalk"))
            .arg(&input)
            .arg("--output")
            .arg(&output)
            .arg("--render-geometry")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_RUNTIME_DIR")
            .output()
            .unwrap();
        assert!(!result.status.success(), "{source}");
        assert_eq!(result.stdout, [] as [u8; 0]);
        assert_ne!(result.stderr, [] as [u8; 0]);
        assert_eq!(fs::read(&output).unwrap(), previous, "{source}");
    }
}
