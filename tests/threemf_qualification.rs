//! Independent geometry and original-byte qualification of multipart 3MF packages.

use catwalk::{
    formats,
    scene::{Scene, Vec3},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/mesh_scene")
        .join(name)
}

fn bounds(points: impl Iterator<Item = Vec3>) -> ([f64; 3], [f64; 3]) {
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for p in points {
        for (axis, coordinate) in [p.x, p.y, p.z].into_iter().enumerate() {
            low[axis] = low[axis].min(coordinate);
            high[axis] = high[axis].max(coordinate);
        }
    }
    (low, high)
}

fn assert_bounds(actual: ([f64; 3], [f64; 3]), low: [f64; 3], high: [f64; 3]) {
    for (actual, expected) in actual
        .0
        .into_iter()
        .chain(actual.1)
        .zip(low.into_iter().chain(high))
    {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }
}

fn area(scene: &Scene) -> f64 {
    scene
        .triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.vertices;
            (b - a).cross(c - a).length() * 0.5
        })
        .sum()
}

#[test]
fn authored_split_parts_preserve_local_ids_palettes_and_nested_placement() {
    let scene = formats::load(&fixture("3mf-production-colliding-ids.3mf")).unwrap();
    assert_eq!(scene.triangles.len(), 8);
    assert!(scene.points.is_empty() && scene.lines.is_empty());
    // Hand-computed from the two local tetrahedra, child rotation, root scale,
    // component translations and final build translation. Child builds are
    // thousands of millimeters away and must not contribute geometry.
    assert_bounds(
        bounds(scene.triangles.iter().flat_map(|t| t.vertices)),
        [-5., 6., 0.],
        [12., 13., 8.],
    );
    for (color, low, high) in [
        ([230, 105, 55], [10., 9., 6.], [12., 13., 8.]),
        ([55, 150, 220], [-5., 6., 0.], [-4., 8., 3.]),
    ] {
        let triangles: Vec<_> = scene
            .triangles
            .iter()
            .filter(|t| t.color == Some(color))
            .collect();
        assert_eq!(triangles.len(), 4, "local palette collision changed a part");
        assert_bounds(
            bounds(triangles.into_iter().flat_map(|t| t.vertices)),
            low,
            high,
        );
    }
}

#[test]
fn authored_cross_part_build_converts_geometry_and_translation_units() {
    let scene = formats::load(&fixture("3mf-production-mixed-units.3mf")).unwrap();
    assert_eq!(scene.triangles.len(), 4);
    assert!(
        scene
            .triangles
            .iter()
            .all(|t| t.color == Some([185, 90, 190]))
    );
    // Inch mesh: local x scale 2 and translation 1 inch; then root build
    // scale 0.5 and translation (1,2,3) centimeters. Translation is converted
    // in its owning model's units, and the linear scales remain dimensionless.
    assert_bounds(
        bounds(scene.triangles.iter().flat_map(|t| t.vertices)),
        [22.7, 20., 30.],
        [48.1, 45.4, 42.7],
    );
}

struct ExporterReference {
    name: &'static str,
    sha256: &'static str,
    application: &'static str,
    triangles: usize,
    low: [f64; 3],
    high: [f64; 3],
    area: f64,
}

#[test]
fn original_slicer_packages_match_independent_geometry_and_exact_source_bytes() {
    // References were independently computed from the original ZIP/XML using
    // Python ElementTree: part-local IDs, root build selection, native vertices
    // and homogeneous transforms. Python is not needed by these portable tests.
    let references = [
        ExporterReference {
            name: "3mf-bambu-pa-pattern.3mf",
            sha256: "2e2c0e271a50c6f89cc42a97b134d6f678d7d3d2849fa9b48c8203586aba5111",
            application: "BambuStudio-01.07.03.04",
            triangles: 12,
            low: [
                80.294_561_198_000_01,
                83.569_463_398_000_01,
                2.000_000_165_480_742e-10,
            ],
            high: [85.294_561_202, 88.569_463_402, 0.849_999_999_8],
            area: 67.000_000_085_599_67,
        },
        ExporterReference {
            name: "3mf-orca-badge.3mf",
            sha256: "04573823874f90a8b46778ef35a78a210422bfef0051a53d296b3364ade56f93",
            application: "BambuStudio-02.06.00.51",
            triangles: 72_586,
            low: [87.529_541, 80.449_493_3, -2.970_000_001_489_836_4e-8],
            high: [183.470_459, 191.550_506_6, 1.600_000_009],
            area: 26_039.388_578_452_985,
        },
        ExporterReference {
            name: "3mf-prusa-roundtrip.3mf",
            sha256: "f14d9c803e38eee1477e138ac2b59b7c64950aad4506dbfe9d71784c03cf2737",
            application: "PrusaSlicer-2.9.6",
            triangles: 72,
            low: [72.575_141_9, 151.599_876_600_000_02, 0.],
            high: [252.042_659_5, 232.597_759_9, 36.],
            area: 46_655.998_969_248_016,
        },
    ];
    for expected in references {
        let path = fixture(expected.name);
        let original = fs::read(&path).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&original)), expected.sha256);
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&original)).unwrap();
        let mut model = String::new();
        zip.by_name("3D/3dmodel.model")
            .unwrap()
            .read_to_string(&mut model)
            .unwrap();
        let document = roxmltree::Document::parse(&model).unwrap();
        let application = document
            .descendants()
            .find(|node| {
                node.tag_name().name() == "metadata"
                    && node.attribute("name") == Some("Application")
            })
            .unwrap()
            .text();
        assert_eq!(application, Some(expected.application));
        let scene = formats::load(&path).unwrap();
        assert_eq!(
            scene.triangles.len(),
            expected.triangles,
            "{} omitted mesh geometry",
            expected.name
        );
        assert!(scene.points.is_empty() && scene.lines.is_empty());
        assert_bounds(
            bounds(scene.triangles.iter().flat_map(|t| t.vertices)),
            expected.low,
            expected.high,
        );
        let actual_area = area(&scene);
        assert!(
            (actual_area - expected.area).abs() < expected.area * 1e-8,
            "{}: area {actual_area} != {}",
            expected.name,
            expected.area
        );
        assert_eq!(
            fs::read(path).unwrap(),
            original,
            "import modified the original package"
        );
    }
}
