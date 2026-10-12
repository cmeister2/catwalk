//! Independent source, geometry and headless-render qualification for unstructured VTK lines.

#[path = "support/digest_hex.rs"]
/// Format digest bytes with the unchanged lowercase hexadecimal comparison protocol.
mod digest_hex;

use catwalk3d::{
    formats,
    render::{RenderOptions, render, write_png},
    scene::{Line, Point, Scene, Triangle, Vec3},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Equivalent authored grids with distinct geometry and topology encodings.
const AUTHORED_FILES: [&str; 4] = [
    "vtk-unstructured-lines-ascii.vtk",
    "vtk-unstructured-lines-binary-v5.vtk",
    "vtu-unstructured-lines-ascii.vtu",
    "vtkhdf-unstructured-lines.vtkhdf",
];

/// Literal Cartesian positions used by the authored topology witnesses.
const POSITIONS: [Vec3; 9] = [
    Vec3::new(-2., -1., 0.5),
    Vec3::new(-1., 2., 1.),
    Vec3::new(2., -2., 3.),
    Vec3::new(0., 0., 0.),
    Vec3::new(3., 1., 2.),
    Vec3::new(4., 3., -1.),
    Vec3::new(1., 5., 1.),
    Vec3::new(-1., 1., 2.),
    Vec3::new(-2., 3., 2.),
];

/// A test-local directory removed after its fixtures and subprocesses finish.
struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    /// Create a uniquely named directory for one test in this process.
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("catwalk-vtk-lines-{name}-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Resolve a retained VTK input or required upstream notice.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/vtk_lines")
        .join(name)
}

/// Compute bounds over every emitted primitive independently of importer internals.
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

/// Construct the expected scene directly from five literal cells, without decoding a file.
fn reference_scene() -> Scene {
    Scene {
        points: [0, 1, 2]
            .map(|index| Point {
                position: POSITIONS[index],
                color: None,
            })
            .into(),
        lines: [[3, 4], [4, 5], [5, 6]]
            .map(|[a, b]| Line {
                vertices: [POSITIONS[a], POSITIONS[b]],
                color: None,
            })
            .into(),
        triangles: vec![Triangle {
            vertices: [POSITIONS[3], POSITIONS[7], POSITIONS[8]],
            color: None,
        }],
        ..Scene::default()
    }
}

/// Verify ordered endpoints, retained point cells, oriented surface geometry and independent measures.
fn assert_authored_geometry(name: &str) {
    let actual = formats::load(&fixture(name)).unwrap();
    let expected = reference_scene();
    assert_eq!(actual.points.len(), 3, "{name}");
    assert_eq!(actual.lines.len(), 3, "{name}");
    assert_eq!(actual.triangles.len(), 1, "{name}");
    assert!(actual.appearances.is_empty(), "{name}");
    for (actual, expected) in actual.points.iter().zip(&expected.points) {
        assert_eq!(actual.position, expected.position, "{name}");
        assert_eq!(actual.color, None, "{name}");
    }
    for (actual, expected) in actual.lines.iter().zip(&expected.lines) {
        assert_eq!(actual.vertices, expected.vertices, "{name}");
        assert_eq!(actual.color, None, "{name}");
    }
    assert_eq!(
        actual.triangles[0].vertices, expected.triangles[0].vertices,
        "{name}"
    );
    assert_eq!(actual.triangles[0].color, None, "{name}");
    assert_eq!(
        bounds(&actual),
        [Vec3::new(-2., -2., -1.), Vec3::new(4., 5., 3.)],
        "{name}"
    );
    let length: f64 = actual
        .lines
        .iter()
        .map(|line| (line.vertices[1] - line.vertices[0]).length())
        .sum();
    assert!(
        (length - (2. * 14_f64.sqrt() + 17_f64.sqrt())).abs() < 1e-12,
        "{name}"
    );
    let [a, b, c] = actual.triangles[0].vertices;
    assert!(
        ((b - a).cross(c - a).length() * 0.5 - 21_f64.sqrt() * 0.5).abs() < 1e-12,
        "{name}"
    );
}

/// Render one model through the actual CLI, with every display-related variable removed.
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
fn original_f3d_dataset_and_required_notices_retain_pinned_bytes() {
    // F3D commit 09ba978ad03eb40466548f340c0db7c18843a354 publishes this
    // model's SHA-256 through its LFS pointer. The license mapping explicitly
    // assigns unlisted datasets, including pdiag.vtu, to the F3D BSD license.
    for (name, expected, size) in [
        (
            "pdiag.vtu",
            "61152cf6369593d3cb4d7213496cd0fc0da298e37de156f93f965e8585b2e342",
            153_597,
        ),
        (
            "F3D-LICENSE.md",
            "42e90e43576bfcc8945239179c49141c387c407a3a3c3696789d9f061cc4afb4",
            1_607,
        ),
        (
            "F3D-DATA_LICENSES.md",
            "4229d1453e125ace50ca1825d420340bbc6b2ee4f2e6ffabde8709052534e12a",
            9_103,
        ),
    ] {
        let bytes = fs::read(fixture(name)).unwrap();
        assert_eq!(bytes.len(), size);
        assert_eq!(
            format!("{:x}", digest_hex::Hex(Sha256::digest(bytes))),
            expected,
            "{name}"
        );
    }
}

#[test]
fn original_f3d_lines_match_every_independently_decoded_endpoint() {
    let scene = formats::load(&fixture("pdiag.vtu")).unwrap();
    assert!(scene.triangles.is_empty());
    assert!(scene.points.is_empty());
    assert!(scene.appearances.is_empty());
    assert_eq!(scene.lines.len(), 3_497);
    assert_eq!(
        scene
            .lines
            .iter()
            .filter(|line| line.vertices[0] != line.vertices[1])
            .count(),
        3_215
    );
    let mut digest = Sha256::new();
    for line in &scene.lines {
        assert_eq!(line.color, None);
        for point in line.vertices {
            for component in [point.x, point.y, point.z] {
                digest.update(component.to_le_bytes());
            }
        }
    }
    // Independently decoded from the original UInt32-header/zlib/base64 data
    // using binary32 promotion: 3,497 cells × six little-endian f64 values.
    // This includes all 282 coincident-endpoint cells and original signed zeros.
    assert_eq!(
        format!("{:x}", digest_hex::Hex(digest.finalize())),
        "e2045cd0886cdae8ffe2299f7c602743e6be45329df2d7c7300881d3d08b8edc"
    );
    let origin = Vec3::new(9.999_999_974_752_427e-7, 9.999_999_974_752_427e-7, 0.);
    assert_eq!(
        scene.lines[0].vertices,
        [origin, Vec3::new(origin.x, 1.000_000_953_674_316_4, 0.)]
    );
    assert_eq!(
        scene.lines.last().unwrap().vertices,
        [
            origin,
            Vec3::new(0.926_642_715_930_938_7, 0.926_642_715_930_938_7, 0.)
        ]
    );
    assert_eq!(
        bounds(&scene),
        [
            origin,
            Vec3::new(0.926_642_715_930_938_7, 1.000_000_953_674_316_4, 0.)
        ]
    );
    let length: f64 = scene
        .lines
        .iter()
        .map(|line| (line.vertices[1] - line.vertices[0]).length())
        .sum();
    assert!((length - 141.331_205_738_288_02).abs() < 1e-10);
}

#[test]
fn classic_ascii_unstructured_grid_retains_mixed_primitives() {
    assert_authored_geometry(AUTHORED_FILES[0]);
}

#[test]
fn big_endian_version_five_unstructured_grid_retains_mixed_primitives() {
    assert_authored_geometry(AUTHORED_FILES[1]);
}

#[test]
fn xml_unstructured_grid_retains_mixed_primitives() {
    assert_authored_geometry(AUTHORED_FILES[2]);
}

#[test]
fn vtkhdf_unstructured_grid_keeps_its_existing_mixed_primitive_support() {
    assert_authored_geometry(AUTHORED_FILES[3]);
}

#[test]
fn four_headless_cli_encodings_match_the_literal_scene_png() {
    let options = RenderOptions {
        width: 256,
        height: 256,
        transparent: true,
        ..RenderOptions::default()
    };
    let expected = render(&reference_scene(), &options).unwrap();
    let mut bytes = Vec::new();
    write_png(&expected, &mut bytes).unwrap();
    assert!(expected.pixels().any(|pixel| pixel[3] > 0));
    for name in AUTHORED_FILES {
        assert_eq!(cli_png(&fixture(name)), bytes, "{name}");
    }
}

#[test]
fn nested_multiblock_children_keep_their_line_and_point_cells() {
    let directory = TemporaryDirectory::new("multiblock");
    fs::copy(fixture(AUTHORED_FILES[1]), directory.0.join("binary.vtk")).unwrap();
    fs::copy(fixture(AUTHORED_FILES[2]), directory.0.join("ascii.vtu")).unwrap();
    fs::write(directory.0.join("child.vtm"), r#"<VTKFile type="vtkMultiBlockDataSet"><vtkMultiBlockDataSet><DataSet index="0" file="binary.vtk"/></vtkMultiBlockDataSet></VTKFile>"#).unwrap();
    let input = directory.0.join("parent.vtm");
    fs::write(&input, r#"<VTKFile type="vtkMultiBlockDataSet"><vtkMultiBlockDataSet><DataSet index="0" file="ascii.vtu"/><Block index="1"><DataSet index="0" file="child.vtm"/></Block></vtkMultiBlockDataSet></VTKFile>"#).unwrap();
    let actual = formats::load(&input).unwrap();
    let mut expected = reference_scene();
    let repeated = reference_scene();
    expected.points.extend(repeated.points);
    expected.lines.extend(repeated.lines);
    expected.triangles.extend(repeated.triangles);
    assert_eq!(actual.points.len(), 6);
    assert_eq!(actual.lines.len(), 6);
    assert_eq!(actual.triangles.len(), 2);
    assert_eq!(bounds(&actual), bounds(&expected));
    let options = RenderOptions {
        width: 256,
        height: 256,
        transparent: true,
        ..RenderOptions::default()
    };
    let mut bytes = Vec::new();
    write_png(&render(&expected, &options).unwrap(), &mut bytes).unwrap();
    assert_eq!(cli_png(&input), bytes);
}

#[test]
fn malformed_line_cell_fails_before_replacing_an_existing_thumbnail() {
    let directory = TemporaryDirectory::new("invalid-line");
    let input = directory.0.join("invalid.vtk");
    fs::write(&input, "# vtk DataFile Version 3.0\ninvalid line\nASCII\nDATASET UNSTRUCTURED_GRID\nPOINTS 3 double\n0 0 0 1 0 0 2 0 0\nCELLS 1 4\n3 0 1 2\nCELL_TYPES 1\n3\n").unwrap();
    let destination = directory.0.join("preview.png");
    let previous = cli_png(&fixture(AUTHORED_FILES[2]));
    fs::write(&destination, &previous).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_catwalk"))
        .arg(&input)
        .arg("--output")
        .arg(&destination)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stdout, [] as [u8; 0]);
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("invalid vertex count for VTK cell type 3")
    );
    assert_eq!(fs::read(&destination).unwrap(), previous);
}
