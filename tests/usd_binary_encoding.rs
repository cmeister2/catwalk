//! Canonical authored USD binary encodings, package structure and literal geometry.

#[path = "support/digest_hex.rs"]
/// Format source digest bytes with the corpus's lowercase hexadecimal protocol.
mod digest_hex;

use catwalk3d::{
    formats,
    render::{RenderOptions, render},
    scene::{Scene, Triangle, Vec3},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Read},
    path::PathBuf,
};

/// Unchanged authored ASCII source from which the binary fixtures are exported.
const ASCII_SHA256: &str = "8e9257bc4faeb438a0e7982416995139d3ede86d8788afc4c1c1592ebbe571b5";
/// Exact package root, retaining its local reference and independently named prim.
const PACKAGE_ROOT: &[u8] = b"#usda 1.0\n(upAxis = \"Z\"; defaultPrim = \"Assembly\")\ndef Mesh \"Assembly\" (prepend references = @parts/tetra.usdc@) {}\n";
/// Literal Float32 source points promoted to Float64, then translated by (0.3,0.2,0).
const POSITIONS: [[f64; 3]; 4] = [
    [0.3, 0.2, 0.0],
    [2.3, 0.2, 0.0],
    [0.3, 1.800_000_023_841_858, 0.0],
    [
        0.649_999_994_039_535_6,
        0.649_999_988_079_071,
        2.299_999_952_316_284,
    ],
];
/// Independently authored tetrahedron face winding.
const FACES: [[usize; 3]; 4] = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [2, 0, 3]];
/// sRGB bytes corresponding to the authored linear display color (0.15,0.55,0.9).
const COLOR: [u8; 3] = [108, 196, 243];

/// Locate the mandatory authored input without rewriting its bytes.
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/mesh_scene")
        .join(name)
}

/// Compare one Float32-promoted coordinate with its independently placed value.
fn position(actual: Vec3, expected: [f64; 3]) {
    for (actual, expected) in [actual.x, actual.y, actual.z].into_iter().zip(expected) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }
}

/// Construct the independent four-face rendering oracle.
fn literal_scene() -> Scene {
    Scene {
        triangles: FACES
            .map(|face| Triangle {
                vertices: face.map(|index| {
                    let [x, y, z] = POSITIONS[index];
                    Vec3::new(x, y, z)
                }),
                color: Some(COLOR),
            })
            .to_vec(),
        ..Scene::default()
    }
}

/// Check exact topology, color and bounds before comparing rendered output.
fn geometry(scene: &Scene) {
    assert_eq!(scene.triangles.len(), 4);
    assert!(scene.points.is_empty());
    assert!(scene.lines.is_empty());
    assert!(scene.appearances.is_empty());
    for (triangle, face) in scene.triangles.iter().zip(FACES) {
        assert_eq!(triangle.color, Some(COLOR));
        for (actual, index) in triangle.vertices.into_iter().zip(face) {
            position(actual, POSITIONS[index]);
        }
    }
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    for point in scene.positions() {
        for (axis, value) in [point.x, point.y, point.z].into_iter().enumerate() {
            minimum[axis] = minimum[axis].min(value);
            maximum[axis] = maximum[axis].max(value);
        }
    }
    position(
        Vec3::new(minimum[0], minimum[1], minimum[2]),
        [0.3, 0.2, 0.0],
    );
    position(
        Vec3::new(maximum[0], maximum[1], maximum[2]),
        [2.3, 1.800_000_023_841_858, 2.299_999_952_316_284],
    );
}

/// Canonical short integer arrays preserve the unchanged source and all three binary views.
#[test]
fn authored_binary_usd_encodings_match_literal_tetrahedron() {
    let ascii = fs::read(fixture("usda-authored-mesh.usda")).expect("mandatory authored ASCII USD");
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&ascii))),
        ASCII_SHA256
    );
    let native = fs::read(fixture("usdc-native-crate.usdc")).expect("mandatory binary USD");
    assert!(native.starts_with(b"PXR-USDC"));
    assert_eq!(
        fs::read(fixture("usd-native-crate.usd")).expect("mandatory binary USD alias"),
        native,
    );

    let package = fs::read(fixture("usdz-aligned-binary-package.usdz"))
        .expect("mandatory aligned binary USD package");
    let mut archive = zip::ZipArchive::new(Cursor::new(package)).expect("valid authored ZIP");
    assert_eq!(archive.len(), 2);
    for (index, expected_name, expected_bytes) in [
        (0, "root.usda", PACKAGE_ROOT),
        (1, "parts/tetra.usdc", native.as_slice()),
    ] {
        let mut member = archive.by_index(index).expect("mandatory package member");
        assert_eq!(
            member.name().expect("valid member name").as_ref(),
            expected_name
        );
        assert_eq!(member.compression(), zip::CompressionMethod::Stored);
        assert_eq!(member.data_start().expect("local file data offset") % 64, 0);
        let mut bytes = Vec::new();
        member
            .read_to_end(&mut bytes)
            .expect("complete stored member");
        assert_eq!(bytes, expected_bytes);
    }

    let options = RenderOptions {
        width: 96,
        height: 96,
        ..RenderOptions::default()
    };
    let expected = render(&literal_scene(), &options).expect("literal tetrahedron image");
    for name in [
        "usda-authored-mesh.usda",
        "usdc-native-crate.usdc",
        "usd-native-crate.usd",
        "usdz-aligned-binary-package.usdz",
    ] {
        let scene = formats::load(&fixture(name)).unwrap_or_else(|error| panic!("{name}: {error}"));
        geometry(&scene);
        let actual = render(&scene, &options).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(actual.as_raw(), expected.as_raw(), "{name}");
    }
}
