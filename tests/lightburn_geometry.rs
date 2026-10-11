//! Retained `LightBurn` vector geometry, source provenance and headless thumbnail qualification.

use catwalk3d::{formats, scene::Vec3};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Retained no-thumbnail vector projects qualified against rendered snapshots.
const RENDER_CASES: &[&str] = &[
    "classic-geometry.lbrn",
    "line.lbrn2",
    "square.lbrn2",
    "circle.lbrn2",
    "rings.lbrn2",
    "crucifix.lbrn2",
    "butterfly_vectorized.lbrn2",
    "board-with-soldermask-only.lbrn2",
    "repro04-motor-controller.lbrn2",
];

/// Resolve a retained `LightBurn` project or snapshot under the repository root.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/native_geometry/lightburn")
        .join(name)
}

/// Measure drawing-plane bounds independently from all emitted segment endpoints.
fn bounds(scene: &catwalk3d::scene::Scene) -> [Vec3; 2] {
    scene.positions().fold(
        [
            Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
        ],
        |[min, max], point| {
            [
                Vec3::new(min.x.min(point.x), min.y.min(point.y), min.z.min(point.z)),
                Vec3::new(max.x.max(point.x), max.y.max(point.y), max.z.max(point.z)),
            ]
        },
    )
}

#[test]
fn licensed_external_sources_are_unchanged_and_have_no_thumbnail_carrier() {
    let provenance: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture("provenance.json")).unwrap()).unwrap();
    for entry in provenance.as_array().unwrap() {
        let name = entry["file"].as_str().unwrap();
        let input = fixture(name);
        let bytes = std::fs::read(&input).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            entry["sha256"].as_str().unwrap(),
            "fixture source was changed: {name}"
        );
        let document = roxmltree::Document::parse(std::str::from_utf8(&bytes).unwrap()).unwrap();
        assert!(
            !document
                .descendants()
                .any(|node| node.has_tag_name("Thumbnail"))
        );
        assert!(
            catwalk3d::saved_preview::lightburn::extract(&input)
                .unwrap()
                .is_none()
        );
        assert!(fixture(entry["license_file"].as_str().unwrap()).is_file());
        let scene = formats::load(&input).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(scene.triangles.is_empty());
        assert!(scene.points.is_empty());
        assert!(!scene.lines.is_empty());
        assert!(scene.positions().all(|point| point.z == 0.0));
    }
}

#[test]
fn real_exporter_line_rectangle_and_circle_have_independent_expected_coordinates() {
    let line = formats::load(&fixture("line.lbrn2")).unwrap();
    assert_eq!(line.lines.len(), 1);
    // Independently applying the file's six affine coefficients to its two
    // vertices gives approximately (50,50) and (60,60), with screen Y flipped.
    assert!((line.lines[0].vertices[0] - Vec3::new(50.0, -50.0, 0.0)).length() < 1e-5);
    assert!((line.lines[0].vertices[1] - Vec3::new(60.0, -60.0, 0.0)).length() < 1e-5);
    let square = formats::load(&fixture("square.lbrn2")).unwrap();
    assert_eq!(square.lines.len(), 4);
    assert_eq!(
        bounds(&square),
        [Vec3::new(50.0, -60.0, 0.0), Vec3::new(60.0, -50.0, 0.0)]
    );
    let circle = formats::load(&fixture("circle.lbrn2")).unwrap();
    assert_eq!(bounds(&circle), bounds(&square));
    for point in circle.positions() {
        assert!(((point - Vec3::new(55.0, -55.0, 0.0)).length() - 5.0).abs() < 1e-12);
    }
}

#[test]
fn authored_classic_no_thumbnail_geometry_has_bezier_and_group_placements() {
    let input = fixture("classic-geometry.lbrn");
    assert!(
        catwalk3d::saved_preview::lightburn::extract(&input)
            .unwrap()
            .is_none()
    );
    let scene = formats::load(&input).unwrap();
    assert_eq!(
        bounds(&scene),
        [Vec3::new(10.0, 18.0, 0.0), Vec3::new(27.0, 34.0, 0.0)]
    );
    assert!(
        scene
            .positions()
            .any(|point| point == Vec3::new(14.0, 26.0, 0.0))
    );
    for color in [[255, 0, 0], [0, 0, 255], [0, 255, 0]] {
        assert!(scene.lines.iter().any(|line| line.color == Some(color)));
    }
}

#[test]
fn repeated_curved_native_path_instances_are_all_placed_and_colored() {
    let scene = formats::load(&fixture("crucifix.lbrn2")).unwrap();
    // Four paths use precisely the same shared VertID and PrimID. Each needs
    // its own transform; omitting any instance or drawing all at the origin
    // changes this independent translation relationship.
    assert_eq!(scene.lines.len() % 4, 0);
    let count = scene.lines.len() / 4;
    assert!(count > 300);
    let shifts = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(70.337_845 - 98.711_327, -(33.350_594 - 31.964_798), 0.0),
        Vec3::new(48.94648 - 98.711_327, -(59.945_423 - 31.964_798), 0.0),
        Vec3::new(84.688_347 - 98.711_327, -(63.251_072 - 31.964_798), 0.0),
    ];
    for (instance, shift) in shifts.into_iter().enumerate() {
        for (original, placed) in scene.lines[..count]
            .iter()
            .zip(&scene.lines[instance * count..(instance + 1) * count])
        {
            for (a, b) in original.vertices.into_iter().zip(placed.vertices) {
                assert!((b - a - shift).length() < 1e-9);
            }
            assert_eq!(
                placed.color,
                Some(if instance == 1 {
                    [255, 0, 0]
                } else {
                    [0, 0, 255]
                })
            );
        }
    }
}

#[test]
fn closed_pcb_contours_and_large_motor_controller_project_are_complete() {
    let scene = formats::load(&fixture("board-with-soldermask-only.lbrn2")).unwrap();
    // The input has 65,5,5 line primitives. Three are zero-length closures.
    assert_eq!(scene.lines.len(), 72);
    assert_eq!(
        bounds(&scene),
        [Vec3::new(-2.4, -5.6, 0.0), Vec3::new(7.6, 4.4, 0.0)]
    );
    assert!(
        scene
            .lines
            .iter()
            .all(|line| line.color == Some([255, 0, 0]))
    );
    let input = fixture("repro04-motor-controller.lbrn2");
    let bytes = std::fs::read(&input).unwrap();
    let document = roxmltree::Document::parse(std::str::from_utf8(&bytes).unwrap()).unwrap();
    let mut expected = 0;
    for node in document
        .descendants()
        .filter(|node| node.has_tag_name("VertList"))
    {
        let vertices: Vec<(f64, f64)> = node
            .text()
            .unwrap()
            .split('V')
            .skip(1)
            .map(|vertex| {
                let values: Vec<f64> = vertex
                    .split_whitespace()
                    .map(|value| value.parse().unwrap())
                    .collect();
                (values[0], values[1])
            })
            .collect();
        let parent = node.parent().unwrap();
        let primitives = parent
            .children()
            .find(|node| node.has_tag_name("PrimList"))
            .unwrap()
            .text()
            .unwrap();
        assert!(!primitives.contains('B'));
        for primitive in primitives.split('L').skip(1) {
            let pair: Vec<usize> = primitive
                .split_whitespace()
                .map(|value| value.parse().unwrap())
                .collect();
            if vertices[pair[0]] != vertices[pair[1]] {
                expected += 1;
            }
        }
    }
    let scene = formats::load(&input).unwrap();
    assert_eq!(scene.lines.len(), expected);
    assert!(expected > 10_000);
}

#[test]
fn no_thumbnail_projects_render_plan_view_png_snapshots_and_default_fallback() {
    let options = catwalk3d::render::RenderOptions {
        width: 256,
        height: 256,
        transparent: true,
        ..catwalk3d::render::RenderOptions::default()
    };
    for name in RENDER_CASES {
        let input = fixture(name);
        let actual = catwalk3d::geometry_thumbnail(&input, &options).unwrap();
        let fallback = catwalk3d::thumbnail(&input, &options).unwrap();
        assert_eq!(
            actual, fallback,
            "default no-thumbnail geometry fallback: {name}"
        );
        assert!(
            actual.pixels().filter(|pixel| pixel[3] > 0).count() > 100,
            "empty preview: {name}"
        );
        let expected = image::open(fixture("snapshots").join(format!("{name}.png")))
            .unwrap()
            .to_rgba8();
        assert_eq!(actual.dimensions(), expected.dimensions());
        let premultiplied = |pixel: &image::Rgba<u8>, channel: usize| {
            if channel == 3 {
                u16::from(pixel[3])
            } else {
                (u16::from(pixel[channel]) * u16::from(pixel[3]) + 127) / 255
            }
        };
        let maximum_delta = actual
            .pixels()
            .zip(expected.pixels())
            .flat_map(|(a, b)| {
                (0..4).map(move |channel| {
                    premultiplied(a, channel).abs_diff(premultiplied(b, channel))
                })
            })
            .max()
            .unwrap();
        assert!(
            maximum_delta <= 2,
            "LightBurn image snapshot differs: {name}; max RGBA delta {maximum_delta}"
        );
    }
}

#[test]
fn normal_and_explicit_geometry_previews_ignore_a_poisoned_thumbnail_without_a_display() {
    /// A CLI output directory removed when its test guard leaves scope.
    struct Temporary(PathBuf);
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let directory = Temporary(
        std::env::temp_dir().join(format!("catwalk-lightburn-geometry-{}", std::process::id())),
    );
    std::fs::create_dir(&directory.0).unwrap();
    let source = std::fs::read_to_string(fixture("square.lbrn2")).unwrap();
    let source = source.replace(
        "</LightBurnProject>",
        "<Thumbnail Source=\"not a PNG!\"/></LightBurnProject>",
    );
    let input = directory.0.join("poisoned.lbrn2");
    std::fs::write(&input, source).unwrap();
    let expected = image::open(fixture("snapshots/square.lbrn2.png"))
        .unwrap()
        .to_rgba8();
    let options = catwalk3d::render::RenderOptions {
        width: 256,
        height: 256,
        transparent: true,
        ..catwalk3d::render::RenderOptions::default()
    };
    // Establish that normal thumbnail rendering takes the extraction-error
    // fallback, rather than the simpler absent-image fallback.
    assert!(catwalk3d::saved_preview::lightburn::extract(&input).is_err());
    assert_eq!(catwalk3d::thumbnail(&input, &options).unwrap(), expected);
    assert_eq!(
        catwalk3d::geometry_thumbnail(&input, &options).unwrap(),
        expected
    );
    for force_geometry in [false, true] {
        let output = directory.0.join(if force_geometry {
            "forced.png"
        } else {
            "normal.png"
        });
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_catwalk"));
        command
            .arg(&input)
            .arg("--transparent")
            .args(["--resolution", "256,256", "--samples", "2", "--output"])
            .arg(&output)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_RUNTIME_DIR");
        if force_geometry {
            command.arg("--render-geometry");
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "force_geometry={force_geometry}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stderr, [] as [u8; 0]);
        assert_eq!(image::open(output).unwrap().to_rgba8(), expected);
    }
}
