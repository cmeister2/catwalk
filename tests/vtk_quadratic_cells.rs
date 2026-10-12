//! Independent topology, interpolation and failure controls for fixed-node VTK cells.
//!
//! Face roles come from the primary Kitware/VTK Common/DataModel declarations.
//! Literal metrics were calculated separately with nodal Lagrange, serendipity and
//! barycentric bubble bases on the documented six-interval preview lattice. No
//! importer interpolation helpers are used here. Signed triangle contributions are
//! solid volumes only for closed volume cells; open surface contributions qualify
//! winding without making a solid-volume claim.

#[path = "support/digest_hex.rs"]
/// Format digest bytes with the unchanged lowercase hexadecimal comparison protocol.
mod digest_hex;

use catwalk3d::{
    formats, render,
    scene::{Scene, Vec3},
};
use hdf5_writer::{AttributeBuilder, DatasetBuilder, Hdf5Builder, WriteOptions};
use sha2::{Digest, Sha256};
use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

/// Unique scratch-directory counter for concurrently running integration controls.
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

/// Private temporary input directory removed after its control finishes.
struct Directory(PathBuf);

impl Directory {
    /// Create a fresh directory without colliding with another control.
    fn new() -> Self {
        loop {
            let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("catwalk-vtk-quadratic-{}-{id}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("cannot create VTK control directory: {error}"),
            }
        }
    }

    /// Write one privately generated control and return its path.
    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// One unchanged authored corpus case and independently calculated preview metrics.
struct Case {
    /// VTK numeric cell identifier.
    kind: usize,
    /// Corpus filename without its suffix.
    stem: &'static str,
    /// Number of emitted boundary triangles.
    triangles: usize,
    /// Surface area, signed triangle contribution and sampled curve length.
    metrics: [f64; 3],
    /// Minimum and maximum sampled coordinates.
    bounds: [[f64; 3]; 2],
}

/// Independent six-interval oracles for every supported fixed-node family member.
const CASES: &[Case] = &[
    Case {
        kind: 21,
        stem: "vtu-quadratic-edge",
        triangles: 0,
        metrics: [0., 0., 3.740_090_511_673_095_4],
        bounds: [[0., 0., 0.], [3., 0.9, 0.45]],
    },
    Case {
        kind: 22,
        stem: "vtu-quadratic-triangle",
        triangles: 36,
        metrics: [5.684_380_310_451_558, 1.831_882_115_912_209, 0.],
        bounds: [[-0.35, -0.35, 0.], [3., 2., 0.866_666_666_666_666_8]],
    },
    Case {
        kind: 23,
        stem: "vtu-quadratic-quad",
        triangles: 72,
        metrics: [11.759_279_801_051_923, 5.332_685_566_224_661, 0.],
        bounds: [[-0.5, -0.5, 0.], [3.5, 2.5, 1.2]],
    },
    Case {
        kind: 24,
        stem: "vtu-quadratic-tetra",
        triangles: 144,
        metrics: [22.680_439_104_059_992, 6.375_349_794_238_683, 0.],
        bounds: [
            [-0.4, -0.266_666_666_666_666_66, -0.333_333_333_333_333_37],
            [3., 2., 2.5],
        ],
    },
    Case {
        kind: 25,
        stem: "vtu-quadratic-hexahedron",
        triangles: 432,
        metrics: [67.289_393_933_845_59, 48.996_890_717_878_36, 0.],
        bounds: [[-1.2, -0.8, -1.], [4.2, 2.8, 3.5]],
    },
    Case {
        kind: 26,
        stem: "vtu-quadratic-wedge",
        triangles: 288,
        metrics: [45.079_269_713_820_43, 22.707_646_700_198_136, 0.],
        bounds: [
            [-0.8, -0.533_333_333_333_333_2, -0.666_666_666_666_666_7],
            [3.8, 2.533_333_333_333_333, 3.166_666_666_666_667],
        ],
    },
    Case {
        kind: 27,
        stem: "vtu-quadratic-pyramid",
        triangles: 216,
        metrics: [31.527_505_988_992_58, 13.467_019_051_973_782, 0.],
        bounds: [[-0.6, -0.4, -0.4], [3.6, 2.4, 2.5]],
    },
    Case {
        kind: 28,
        stem: "vtu-biquadratic-quad",
        triangles: 72,
        metrics: [12.165_099_974_763_512, 5.769_094_452_629_738, 0.],
        bounds: [[-0.5, -0.5, 0.], [3.5, 2.5, 1.3]],
    },
    Case {
        kind: 29,
        stem: "vtu-triquadratic-hexahedron",
        triangles: 432,
        metrics: [73.091_060_874_066_83, 55.432_413_250_012_71, 0.],
        bounds: [[-1.5, -1., -1.25], [4.5, 3., 3.75]],
    },
    Case {
        kind: 30,
        stem: "vtu-quadratic-linear-quad",
        triangles: 12,
        metrics: [9.123_361_239_064_23, 2.684_773_662_551_441, 0.],
        bounds: [[0., -0.5, 0.], [3., 2.5, 0.8]],
    },
    Case {
        kind: 31,
        stem: "vtu-quadratic-linear-wedge",
        triangles: 108,
        metrics: [38.022_250_453_847_44, 15.874_938_271_604_938, 0.],
        bounds: [
            [-0.4, -0.266_666_666_666_666_6, -0.666_666_666_666_666_7],
            [3., 2., 3.166_666_666_666_667],
        ],
    },
    Case {
        kind: 32,
        stem: "vtu-biquadratic-quadratic-wedge",
        triangles: 288,
        metrics: [46.348_702_905_095_15, 24.852_820_877_576_253, 0.],
        bounds: [
            [-1., -0.666_666_666_666_666_6, -0.666_666_666_666_666_7],
            [3.8, 2.533_333_333_333_333, 3.166_666_666_666_667],
        ],
    },
    Case {
        kind: 33,
        stem: "vtu-biquadratic-quadratic-hexahedron",
        triangles: 432,
        metrics: [71.094_643_231_578_33, 53.287_239_072_634_59, 0.],
        bounds: [[-1.5, -1., -1.], [4.5, 3., 3.5]],
    },
    Case {
        kind: 34,
        stem: "vtu-biquadratic-triangle",
        triangles: 36,
        metrics: [6.839_613_566_308_802_5, 2.696_226_080_246_914_2, 0.],
        bounds: [[-0.35, -0.35, 0.], [3., 2., 1.3]],
    },
    Case {
        kind: 35,
        stem: "vtu-cubic-line",
        triangles: 0,
        metrics: [0., 0., 5.401_558_552_735_749],
        bounds: [[0., 0., -0.885_937_5], [3., 1.0125, 0.885_937_5]],
    },
    Case {
        kind: 37,
        stem: "vtu-triquadratic-pyramid",
        triangles: 216,
        metrics: [35.089_511_824_416_9, 17.310_901_624_074_923, 0.],
        bounds: [
            [-0.666_666_666_666_666_6, -0.444_444_444_444_444_53, -0.5],
            [3.666_666_666_666_667_4, 2.444_444_444_444_445, 2.5],
        ],
    },
];

/// Independent barycentric quadratic-triangle weights at (u,v)=(1/6,1/3).
const TRI6: &[f64] = &[0., -1. / 9., -1. / 9., 1. / 3., 2. / 9., 2. / 3.];
/// The same barycentric probe with the centroid bubble and its nodal corrections.
const TRI7: &[f64] = &[
    1. / 12.,
    -1. / 36.,
    -1. / 36.,
    0.,
    -1. / 9.,
    1. / 3.,
    3. / 4.,
];
/// Independent serendipity-quadrilateral weights at (u,v)=(1/6,1/3).
const QUAD8: &[f64] = &[
    0.,
    -4. / 27.,
    -1. / 9.,
    -5. / 27.,
    10. / 27.,
    4. / 27.,
    5. / 27.,
    20. / 27.,
];
/// Independent tensor-product quadratic weights at (u,v)=(1/6,1/3).
const QUAD9: &[f64] = &[
    10. / 81.,
    -2. / 81.,
    1. / 81.,
    -5. / 81.,
    10. / 81.,
    -8. / 81.,
    -5. / 81.,
    40. / 81.,
    40. / 81.,
];
/// Quadratic-in-U, linear-in-V bottom-edge weights at (u,v)=(1/6,0).
const QUAD6: &[f64] = &[5. / 9., -1. / 9., 0., 0., 5. / 9., 0.];
/// A primary VTK face's node-role order and its independently derived probe weights.
type Face = (&'static [usize], &'static [f64]);

/// List primary VTK boundary roles without consulting the importer implementation.
#[allow(
    clippy::too_many_lines,
    reason = "The explicit primary topology table keeps every family and unusual face-center role reviewable."
)]
fn faces(kind: usize) -> Vec<Face> {
    match kind {
        22 => vec![(&[0, 1, 2, 3, 4, 5], TRI6)],
        23 => vec![(&[0, 1, 2, 3, 4, 5, 6, 7], QUAD8)],
        28 => vec![(&[0, 1, 2, 3, 4, 5, 6, 7, 8], QUAD9)],
        30 => vec![(&[0, 1, 2, 3, 4, 5], QUAD6)],
        34 => vec![(&[0, 1, 2, 3, 4, 5, 6], TRI7)],
        24 => vec![
            (&[0, 1, 3, 4, 8, 7], TRI6),
            (&[1, 2, 3, 5, 9, 8], TRI6),
            (&[2, 0, 3, 6, 7, 9], TRI6),
            (&[0, 2, 1, 6, 5, 4], TRI6),
        ],
        25 => vec![
            (&[0, 4, 7, 3, 16, 15, 19, 11], QUAD8),
            (&[1, 2, 6, 5, 9, 18, 13, 17], QUAD8),
            (&[0, 1, 5, 4, 8, 17, 12, 16], QUAD8),
            (&[3, 7, 6, 2, 19, 14, 18, 10], QUAD8),
            (&[0, 3, 2, 1, 11, 10, 9, 8], QUAD8),
            (&[4, 5, 6, 7, 12, 13, 14, 15], QUAD8),
        ],
        26 => vec![
            (&[0, 2, 1, 8, 7, 6], TRI6),
            (&[3, 4, 5, 9, 10, 11], TRI6),
            (&[0, 1, 4, 3, 6, 13, 9, 12], QUAD8),
            (&[1, 2, 5, 4, 7, 14, 10, 13], QUAD8),
            (&[2, 0, 3, 5, 8, 12, 11, 14], QUAD8),
        ],
        27 => vec![
            (&[0, 3, 2, 1, 8, 7, 6, 5], QUAD8),
            (&[0, 1, 4, 5, 10, 9], TRI6),
            (&[1, 2, 4, 6, 11, 10], TRI6),
            (&[2, 3, 4, 7, 12, 11], TRI6),
            (&[3, 0, 4, 8, 9, 12], TRI6),
        ],
        29 => vec![
            (&[0, 4, 7, 3, 16, 15, 19, 11, 20], QUAD9),
            (&[1, 2, 6, 5, 9, 18, 13, 17, 21], QUAD9),
            (&[0, 1, 5, 4, 8, 17, 12, 16, 22], QUAD9),
            (&[3, 7, 6, 2, 19, 14, 18, 10, 23], QUAD9),
            (&[0, 3, 2, 1, 11, 10, 9, 8, 24], QUAD9),
            (&[4, 5, 6, 7, 12, 13, 14, 15, 25], QUAD9),
        ],
        31 => vec![
            (&[0, 2, 1, 8, 7, 6], TRI6),
            (&[3, 4, 5, 9, 10, 11], TRI6),
            (&[0, 1, 4, 3, 6, 9], QUAD6),
            (&[1, 2, 5, 4, 7, 10], QUAD6),
            (&[2, 0, 3, 5, 8, 11], QUAD6),
        ],
        32 => vec![
            (&[0, 2, 1, 8, 7, 6], TRI6),
            (&[3, 4, 5, 9, 10, 11], TRI6),
            (&[0, 1, 4, 3, 6, 13, 9, 12, 15], QUAD9),
            (&[1, 2, 5, 4, 7, 14, 10, 13, 16], QUAD9),
            (&[2, 0, 3, 5, 8, 12, 11, 14, 17], QUAD9),
        ],
        33 => vec![
            (&[0, 4, 7, 3, 16, 15, 19, 11, 20], QUAD9),
            (&[1, 2, 6, 5, 9, 18, 13, 17, 21], QUAD9),
            (&[0, 1, 5, 4, 8, 17, 12, 16, 22], QUAD9),
            (&[3, 7, 6, 2, 19, 14, 18, 10, 23], QUAD9),
            (&[0, 3, 2, 1, 11, 10, 9, 8], QUAD8),
            (&[4, 5, 6, 7, 12, 13, 14, 15], QUAD8),
        ],
        37 => vec![
            (&[0, 3, 2, 1, 8, 7, 6, 5, 13], QUAD9),
            (&[0, 1, 4, 5, 10, 9, 14], TRI7),
            (&[1, 2, 4, 6, 11, 10, 15], TRI7),
            (&[2, 3, 4, 7, 12, 11, 16], TRI7),
            (&[3, 0, 4, 8, 9, 12, 17], TRI7),
        ],
        _ => Vec::new(),
    }
}

/// Resolve a retained corpus file relative to this crate's manifest.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/vtk_quadratic")
        .join(name)
}

/// Construct a vector from a literal coordinate triple.
fn vector(value: [f64; 3]) -> Vec3 {
    Vec3::new(value[0], value[1], value[2])
}

/// Require an independently expected finite scalar within a fixed numeric tolerance.
fn close(actual: f64, expected: f64) {
    assert!(
        actual.is_finite() && (actual - expected).abs() < 1e-8,
        "{actual} != {expected}"
    );
}

/// Require an independently expected finite vector within a tighter spatial tolerance.
fn close_vector(actual: Vec3, expected: Vec3) {
    assert!(
        actual.is_finite() && (actual - expected).length() < 1e-9,
        "{actual:?} != {expected:?}"
    );
}

/// Require a visible sampled position without assuming triangle iteration order.
fn contains(scene: &Scene, position: Vec3) {
    assert!(
        scene
            .positions()
            .any(|point| (point - position).length() < 1e-9),
        "missing sample {position:?}"
    );
}

/// Calculate total triangle area, signed contributions and sampled curve length.
fn metrics(scene: &Scene) -> [f64; 3] {
    let area = scene
        .triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.vertices;
            (b - a).cross(c - a).length() * 0.5
        })
        .sum();
    let signed = scene
        .triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.vertices;
            a.dot(b.cross(c)) / 6.
        })
        .sum();
    let length = scene
        .lines
        .iter()
        .map(|line| (line.vertices[1] - line.vertices[0]).length())
        .sum();
    [area, signed, length]
}

/// Calculate the visible geometry's minimum and maximum coordinates.
fn bounds(scene: &Scene) -> [Vec3; 2] {
    scene.positions().fold(
        [
            Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
        ],
        |[a, b], p| {
            [
                Vec3::new(a.x.min(p.x), a.y.min(p.y), a.z.min(p.z)),
                Vec3::new(b.x.max(p.x), b.y.max(p.y), b.z.max(p.z)),
            ]
        },
    )
}

/// Read authored ASCII nodes directly, independently of the scientific importer.
fn fixture_nodes(path: &Path) -> Vec<Vec3> {
    let source = fs::read_to_string(path).unwrap();
    let document = roxmltree::Document::parse(&source).unwrap();
    let array = document
        .descendants()
        .find(|node| {
            node.has_tag_name("DataArray")
                && node
                    .parent()
                    .is_some_and(|parent| parent.has_tag_name("Points"))
        })
        .unwrap();
    let values: Vec<f64> = array
        .text()
        .unwrap()
        .split_whitespace()
        .map(|value| value.parse().unwrap())
        .collect();
    assert!(values.len().is_multiple_of(3));
    values
        .as_chunks::<3>()
        .0
        .iter()
        .map(|point| Vec3::new(point[0], point[1], point[2]))
        .collect()
}

/// Evaluate one fixed independent witness from literal node-role weights.
fn probe(points: &[Vec3], roles: &[usize], weights: &[f64]) -> Vec3 {
    assert_eq!(roles.len(), weights.len());
    roles
        .iter()
        .zip(weights)
        .fold(Vec3::default(), |sum, (&role, &weight)| {
            sum + points[role] * weight
        })
}

/// Every authored curved family preserves all boundary nodes and primary face roles.
#[test]
fn every_authored_type_has_independent_curvature_and_geometry_witnesses() {
    assert_eq!(CASES.iter().map(|case| case.triangles).sum::<usize>(), 2784);
    for case in CASES {
        let path = fixture(&format!("{}.vtu", case.stem));
        let points = fixture_nodes(&path);
        let scene = formats::load(&path).unwrap_or_else(|error| panic!("{}: {error}", case.stem));
        assert_eq!(scene.triangles.len(), case.triangles, "{}", case.stem);
        assert_eq!(
            scene.lines.len(),
            if matches!(case.kind, 21 | 35) { 12 } else { 0 }
        );
        assert!(scene.points.is_empty());
        for (index, &point) in points.iter().enumerate() {
            if !matches!((case.kind, index), (29, 26) | (37, 18)) {
                contains(&scene, point);
            }
        }
        for (roles, weights) in faces(case.kind) {
            contains(&scene, probe(&points, roles, weights));
        }
        if case.kind == 21 {
            contains(
                &scene,
                probe(&points, &[0, 1, 2], &[55. / 72., -5. / 72., 11. / 36.]),
            );
        } else if case.kind == 35 {
            // Cardinal cubic basis at normalized t=1/12, with node order 0,1,2,3.
            contains(
                &scene,
                probe(
                    &points,
                    &[0, 1, 2, 3],
                    &[77. / 128., 7. / 128., 77. / 128., -33. / 128.],
                ),
            );
        }
        for (actual, expected) in metrics(&scene).into_iter().zip(case.metrics) {
            close(actual, expected);
        }
        for (actual, expected) in bounds(&scene).into_iter().zip(case.bounds) {
            close_vector(actual, vector(expected));
        }
        assert!(scene.triangles.iter().all(|triangle| {
            let [a, b, c] = triangle.vertices;
            (b - a).cross(c - a).length() > 1e-10
        }));
    }
}

/// Produce a small Float64 XML grid without using importer or fixture-generator helpers.
fn xml_grid(points: &[Vec3], cells: &[Vec<usize>], types: &[usize]) -> String {
    assert_eq!(cells.len(), types.len());
    let mut coordinates = String::new();
    for point in points {
        write!(coordinates, "{} {} {} ", point.x, point.y, point.z).unwrap();
    }
    let mut connectivity = String::new();
    let mut offsets = String::new();
    let mut total = 0;
    for cell in cells {
        for index in cell {
            write!(connectivity, "{index} ").unwrap();
        }
        total += cell.len();
        write!(offsets, "{total} ").unwrap();
    }
    let types = types
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        r#"<VTKFile type="UnstructuredGrid" byte_order="LittleEndian"><UnstructuredGrid><Piece NumberOfPoints="{}" NumberOfCells="{}"><Points><DataArray type="Float64" NumberOfComponents="3" format="ascii">{coordinates}</DataArray></Points><Cells><DataArray type="Int64" Name="connectivity" format="ascii">{connectivity}</DataArray><DataArray type="Int64" Name="offsets" format="ascii">{offsets}</DataArray><DataArray type="UInt8" Name="types" format="ascii">{types}</DataArray></Cells></Piece></UnstructuredGrid></VTKFile>"#,
        points.len(),
        cells.len()
    )
}

/// Construct affine unit cells with independently specified edge and center roles.
#[allow(
    clippy::too_many_lines,
    reason = "The explicit canonical-cell table independently checks the complete primary node ordering."
)]
fn unit_nodes(kind: usize) -> Vec<Vec3> {
    if kind == 21 {
        return vec![
            vector([0., 0., 0.]),
            vector([1., 0., 0.]),
            vector([0.5, 0., 0.]),
        ];
    }
    if kind == 35 {
        return vec![
            vector([0., 0., 0.]),
            vector([1., 0., 0.]),
            vector([1. / 3., 0., 0.]),
            vector([2. / 3., 0., 0.]),
        ];
    }
    let (corners, edges): (&[[f64; 3]], &[[usize; 2]]) = match kind {
        22 | 34 => (
            &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            &[[0, 1], [1, 2], [2, 0]],
        ),
        23 | 28 => (
            &[[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]],
            &[[0, 1], [1, 2], [2, 3], [3, 0]],
        ),
        30 => (
            &[[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]],
            &[[0, 1], [3, 2]],
        ),
        24 => (
            &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            &[[0, 1], [1, 2], [2, 0], [0, 3], [1, 3], [2, 3]],
        ),
        25 | 29 | 33 => (
            &[
                [0., 0., 0.],
                [1., 0., 0.],
                [1., 1., 0.],
                [0., 1., 0.],
                [0., 0., 1.],
                [1., 0., 1.],
                [1., 1., 1.],
                [0., 1., 1.],
            ],
            &[
                [0, 1],
                [1, 2],
                [2, 3],
                [3, 0],
                [4, 5],
                [5, 6],
                [6, 7],
                [7, 4],
                [0, 4],
                [1, 5],
                [2, 6],
                [3, 7],
            ],
        ),
        26 | 32 => (
            &[
                [0., 0., 0.],
                [1., 0., 0.],
                [0., 1., 0.],
                [0., 0., 1.],
                [1., 0., 1.],
                [0., 1., 1.],
            ],
            &[
                [0, 1],
                [1, 2],
                [2, 0],
                [3, 4],
                [4, 5],
                [5, 3],
                [0, 3],
                [1, 4],
                [2, 5],
            ],
        ),
        31 => (
            &[
                [0., 0., 0.],
                [1., 0., 0.],
                [0., 1., 0.],
                [0., 0., 1.],
                [1., 0., 1.],
                [0., 1., 1.],
            ],
            &[[0, 1], [1, 2], [2, 0], [3, 4], [4, 5], [5, 3]],
        ),
        27 | 37 => (
            &[
                [0., 0., 0.],
                [1., 0., 0.],
                [1., 1., 0.],
                [0., 1., 0.],
                [0.5, 0.5, 1.],
            ],
            &[
                [0, 1],
                [1, 2],
                [2, 3],
                [3, 0],
                [0, 4],
                [1, 4],
                [2, 4],
                [3, 4],
            ],
        ),
        _ => panic!("unexpected canonical VTK cell type {kind}"),
    };
    let mut points: Vec<_> = corners.iter().copied().map(vector).collect();
    for &[a, b] in edges {
        points.push((points[a] + points[b]) * 0.5);
    }
    let centers: &[[f64; 3]] = match kind {
        28 => &[[0.5, 0.5, 0.]],
        34 => &[[1. / 3., 1. / 3., 0.]],
        29 => &[
            [0., 0.5, 0.5],
            [1., 0.5, 0.5],
            [0.5, 0., 0.5],
            [0.5, 1., 0.5],
            [0.5, 0.5, 0.],
            [0.5, 0.5, 1.],
            [0.5, 0.5, 0.5],
        ],
        33 => &[
            [0., 0.5, 0.5],
            [1., 0.5, 0.5],
            [0.5, 0., 0.5],
            [0.5, 1., 0.5],
        ],
        32 => &[[0.5, 0., 0.5], [0.5, 0.5, 0.5], [0., 0.5, 0.5]],
        37 => &[
            [0.5, 0.5, 0.],
            [0.5, 1. / 6., 1. / 3.],
            [5. / 6., 0.5, 1. / 3.],
            [0.5, 5. / 6., 1. / 3.],
            [1. / 6., 0.5, 1. / 3.],
            [0.5, 0.5, 0.25],
        ],
        _ => &[],
    };
    points.extend(centers.iter().copied().map(vector));
    points
}

/// Affine nodes reproduce elementary areas, closed volumes and outward winding.
#[test]
fn affine_unit_cells_reproduce_bounds_area_volume_and_orientation() {
    let directory = Directory::new();
    for case in CASES {
        let points = unit_nodes(case.kind);
        let indices: Vec<_> = (0..points.len()).collect();
        let path = directory.write(
            &format!("unit-{}.vtu", case.kind),
            xml_grid(&points, &[indices], &[case.kind]),
        );
        let scene = formats::load(&path).unwrap();
        assert_eq!(scene.triangles.len(), case.triangles);
        let (area, volume, center) = match case.kind {
            21 | 35 => (0., 0., Vec3::default()),
            22 | 34 => (0.5, 0., Vec3::default()),
            23 | 28 | 30 => (1., 0., Vec3::default()),
            24 => (
                f64::midpoint(3., 3_f64.sqrt()),
                1. / 6.,
                vector([0.25, 0.25, 0.25]),
            ),
            25 | 29 | 33 => (6., 1., vector([0.5, 0.5, 0.5])),
            26 | 31 | 32 => (3. + 2_f64.sqrt(), 0.5, vector([1. / 3., 1. / 3., 0.5])),
            27 | 37 => (1. + 5_f64.sqrt(), 1. / 3., vector([0.5, 0.5, 0.2])),
            _ => unreachable!(),
        };
        let actual = metrics(&scene);
        close(actual[0], area);
        close(actual[1], volume);
        close(
            actual[2],
            if matches!(case.kind, 21 | 35) { 1. } else { 0. },
        );
        close_vector(bounds(&scene)[0], Vec3::default());
        close_vector(
            bounds(&scene)[1],
            vector([
                1.,
                if matches!(case.kind, 21 | 35) { 0. } else { 1. },
                if volume > 0. { 1. } else { 0. },
            ]),
        );
        for triangle in &scene.triangles {
            let [a, b, c] = triangle.vertices;
            let normal = (b - a).cross(c - a);
            if volume > 0. {
                assert!(normal.dot((a + b + c) / 3. - center) > 1e-10);
            } else {
                assert!(normal.z > 1e-10);
            }
        }
    }
}

/// Shared curved tetrahedral face controls with an independently rotated lower cell.
fn shared_tetrahedra() -> (Vec<Vec3>, Vec<Vec<usize>>) {
    let mut points = unit_nodes(24);
    points[4].z = 0.1;
    points[5].z = 0.07;
    points[6].z = -0.03;
    points.extend(
        [
            [0., 0., -1.],
            [0.5, 0., -0.5],
            [0., 0., -0.5],
            [0., 0.5, -0.5],
        ]
        .map(vector),
    );
    (
        points,
        vec![(0..10).collect(), vec![1, 0, 2, 10, 4, 6, 5, 11, 12, 13]],
    )
}

/// Role-preserving rotations and global relabeling cancel a genuinely curved interface.
#[test]
fn shared_curved_faces_cancel_after_rotation_and_global_relabeling() {
    let directory = Directory::new();
    let (points, cells) = shared_tetrahedra();
    let original =
        formats::load(&directory.write("shared.vtu", xml_grid(&points, &cells, &[24, 24])))
            .unwrap();
    assert_eq!(original.triangles.len(), 216);
    // The shared edge mids also bow the exposed sloping faces. Independent
    // barycentric n6 integration gives this exterior area; the closed volume
    // remains 1/3 after the two copies of the curved interface are removed.
    close(metrics(&original)[0], 3.735_732_565_298_488_5);
    close(metrics(&original)[1], 1. / 3.);
    close_vector(bounds(&original)[0], vector([0., 0., -1.]));
    close_vector(bounds(&original)[1], vector([1., 1., 1.]));
    let relabeled_points: Vec<_> = points.iter().copied().rev().collect();
    let relabeled_cells: Vec<Vec<_>> = cells
        .iter()
        .map(|cell| cell.iter().map(|&index| points.len() - 1 - index).collect())
        .collect();
    let relabeled = formats::load(&directory.write(
        "relabeled.vtu",
        xml_grid(&relabeled_points, &relabeled_cells, &[24, 24]),
    ))
    .unwrap();
    assert_eq!(relabeled.triangles.len(), 216);
    for (a, b) in metrics(&original).into_iter().zip(metrics(&relabeled)) {
        close(a, b);
    }
    for point in original.positions() {
        contains(&relabeled, point);
    }
}

/// Equal ID sets with different attached edge roles are distinct curved surfaces.
#[test]
fn mismatched_midside_roles_are_not_canceled_by_sorted_id_sets() {
    let directory = Directory::new();
    let (points, mut cells) = shared_tetrahedra();
    cells[1].swap(4, 5);
    let scene =
        formats::load(&directory.write("wrong-roles.vtu", xml_grid(&points, &cells, &[24, 24])))
            .unwrap();
    assert_eq!(scene.triangles.len(), 288);
}

/// Invalid cardinalities, references and nonmanifold sharing fail for every family.
#[test]
fn every_fixed_node_type_validates_arity_uniqueness_indices_and_finite_nodes() {
    let directory = Directory::new();
    for case in CASES {
        let points = unit_nodes(case.kind);
        let indices: Vec<_> = (0..points.len()).collect();
        let mut short = indices.clone();
        short.pop();
        let mut long = indices.clone();
        long.push(0);
        let mut invalid = indices.clone();
        *invalid.last_mut().unwrap() = points.len();
        let mut repeated = indices.clone();
        *repeated.last_mut().unwrap() = 0;
        for (name, cell) in [
            ("short", short),
            ("long", long),
            ("index", invalid),
            ("repeat", repeated),
        ] {
            let path = directory.write(
                &format!("{name}-{}.vtu", case.kind),
                xml_grid(&points, &[cell], &[case.kind]),
            );
            assert!(
                formats::load(&path).is_err(),
                "accepted {name} type{}",
                case.kind
            );
        }
        let mut nonfinite = points;
        nonfinite.last_mut().unwrap().z = f64::NAN;
        let path = directory.write(
            &format!("nan-{}.vtu", case.kind),
            xml_grid(&nonfinite, &[indices], &[case.kind]),
        );
        assert!(formats::load(&path).is_err());
    }
    let points = unit_nodes(24);
    let cells = vec![(0..10).collect(); 3];
    let path = directory.write("nonmanifold.vtu", xml_grid(&points, &cells, &[24, 24, 24]));
    assert!(
        formats::load(&path)
            .unwrap_err()
            .to_string()
            .contains("non-manifold")
    );
}

/// Validated interior nodes affect neither the exposed hexahedron nor pyramid boundary.
#[test]
fn unused_volume_interior_nodes_are_validated_but_not_rendered() {
    let directory = Directory::new();
    for (kind, interior) in [(29, 26), (37, 18)] {
        let mut points = unit_nodes(kind);
        let cells = vec![(0..points.len()).collect()];
        let reference = formats::load(&directory.write(
            &format!("interior-{kind}.vtu"),
            xml_grid(&points, &cells, &[kind]),
        ))
        .unwrap();
        points[interior] = vector([999., -777., 333.]);
        let moved = formats::load(&directory.write(
            &format!("moved-{kind}.vtu"),
            xml_grid(&points, &cells, &[kind]),
        ))
        .unwrap();
        assert_eq!(reference.triangles.len(), moved.triangles.len());
        for (a, b) in reference.triangles.iter().zip(&moved.triangles) {
            assert_eq!(a.vertices, b.vertices);
        }
        points[interior].x = f64::INFINITY;
        assert!(
            formats::load(&directory.write(
                &format!("bad-interior-{kind}.vtu"),
                xml_grid(&points, &cells, &[kind])
            ))
            .is_err()
        );
    }
}

/// Constant nonzero curves are omitted while tiny legitimate edges survive.
#[test]
fn collapsed_curves_and_tiny_nonzero_curves_preserve_actual_geometry() {
    let directory = Directory::new();
    for kind in [21, 35] {
        let count = if kind == 21 { 3 } else { 4 };
        let mut points = vec![vector([2., 3., 4.]); count];
        points.push(vector([7., 8., 9.]));
        let cells = vec![(0..count).collect(), vec![count]];
        let scene = formats::load(&directory.write(
            &format!("collapsed-{kind}.vtu"),
            xml_grid(&points, &cells, &[kind, 1]),
        ))
        .unwrap();
        assert!(scene.lines.is_empty());
        assert_eq!(scene.points.len(), 1);
        close_vector(scene.points[0].position, vector([7., 8., 9.]));
        let mut tiny = unit_nodes(kind);
        for point in &mut tiny {
            point.x *= 1e-150;
        }
        let scene = formats::load(&directory.write(
            &format!("tiny-{kind}.vtu"),
            xml_grid(&tiny, &[(0..tiny.len()).collect()], &[kind]),
        ))
        .unwrap();
        assert_eq!(scene.lines.len(), 12);
        assert!(
            scene
                .lines
                .iter()
                .all(|line| line.vertices[0] != line.vertices[1])
        );
    }
}

/// Coincident consecutive samples are omitted without discarding the rest of a curve.
#[test]
fn partially_stationary_quadratic_and_cubic_sample_intervals_are_omitted() {
    let directory = Directory::new();
    // x(t)=12t²-11t+2 has equal samples at t=5/12 and t=1/2.
    // x(t)=18t³-13.5t²+2.5t+1 equals 9/8 at both t=1/12 and t=1/6,
    // and equals 1 at both t=1/3 and t=5/12, omitting two intervals.
    for (kind, coordinates) in [(21, vec![2., 3., -0.5]), (35, vec![1., 8., 1., 2.])] {
        let points: Vec<_> = coordinates
            .into_iter()
            .map(|x| Vec3::new(x, 0., 0.))
            .collect();
        let input = directory.write(
            &format!("partial-{kind}.vtu"),
            xml_grid(&points, &[(0..points.len()).collect()], &[kind]),
        );
        let scene = formats::load(&input).unwrap();
        assert_eq!(scene.lines.len(), if kind == 21 { 11 } else { 10 });
        assert!(
            scene
                .lines
                .iter()
                .all(|line| line.vertices[0] != line.vertices[1])
        );
        contains(&scene, points[0]);
        contains(&scene, points[1]);
        for &point in &points[2..] {
            contains(&scene, point);
        }
    }
}

/// Float64 encoding variants preserve every independently qualified family primitive.
#[test]
fn whole_family_matches_across_legacy_xml_and_hdf_encodings() {
    let mut reference: Option<Scene> = None;
    for name in [
        "vtk-quadratic-family-ascii.vtk",
        "vtk-quadratic-family-binary.vtk",
        "vtu-quadratic-family-binary.vtu",
        "vtu-quadratic-family-appended-zlib.vtu",
        "vtkhdf-quadratic-family.vtkhdf",
    ] {
        let scene = formats::load(&fixture(name)).unwrap();
        assert_eq!(scene.triangles.len(), 2784);
        assert_eq!(scene.lines.len(), 24);
        assert!(scene.points.is_empty());
        if let Some(reference) = &reference {
            for (a, b) in reference.triangles.iter().zip(&scene.triangles) {
                assert_eq!(a.vertices, b.vertices, "{name}");
            }
            for (a, b) in reference.lines.iter().zip(&scene.lines) {
                assert_eq!(a.vertices, b.vertices, "{name}");
            }
        } else {
            reference = Some(scene);
        }
    }
}

/// The original retained Kitware input and license remain byte-for-byte unchanged.
#[test]
fn retained_kitware_input_and_license_keep_their_original_identity() {
    for (name, length, digest) in [
        (
            "quadraticTetra01.vtu",
            1471,
            "6bebfc84f0c342a867e6c50f849aa532f075d99b6ea980c0140966fb96d520ad",
        ),
        (
            "VTK-Copyright.txt",
            1761,
            "11232448be82e0ea2c2c66219c2e36389f42249894070ee10c549ff182fc08b6",
        ),
    ] {
        let bytes = fs::read(fixture(name)).unwrap();
        assert_eq!(bytes.len(), length, "{name}");
        assert_eq!(
            format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))),
            digest,
            "{name}"
        );
    }
}

/// Original Kitware decimal ASCII nodes qualify the fresh n6 oracle, not old n1 data.
#[test]
fn genuine_kitware_quadratic_tetra_uses_decimal_ascii_n6_curvature() {
    let scene = formats::load(&fixture("quadraticTetra01.vtu")).unwrap();
    assert_eq!(scene.triangles.len(), 216);
    close(metrics(&scene)[0], 3.296_781_687_635_255);
    close_vector(bounds(&scene)[0], vector([0., -0.5, -2.]));
    close_vector(bounds(&scene)[1], vector([3., 0.8, 1.]));
    contains(&scene, vector([1. / 6., 0., -1. / 9.]));
    contains(&scene, vector([0.5, 0., -0.2]));
    let tetra = Scene {
        triangles: scene
            .triangles
            .iter()
            .filter(|triangle| {
                triangle
                    .vertices
                    .iter()
                    .all(|point| point.x < 1.5 && point.z > -0.3)
            })
            .cloned()
            .collect(),
        ..Scene::default()
    };
    assert_eq!(tetra.triangles.len(), 144);
    close(metrics(&tetra)[0], 1.972_707_613_561_181_2);
    close(metrics(&tetra)[1], 0.128_349_794_238_683_15);
    // The unchanged source declares Float32, but the existing ASCII reader keeps
    // decimal f64 values. VTK-native binary32 area/volume would instead be
    // 1.97270768013521 / 0.1283498033118041; keep that reader difference explicit.
    assert!((metrics(&tetra)[0] - 1.972_707_680_135_21).abs() > 5e-8);
}

/// Encode a tiny independent static VTKHDF grid using the published writer API.
fn hdf_grid(points: &[Vec3], cells: &[Vec<usize>], types: &[usize]) -> Vec<u8> {
    let coordinates: Vec<_> = points
        .iter()
        .flat_map(|point| [point.x, point.y, point.z])
        .collect();
    let mut builder = Hdf5Builder::new()
        .group_attribute(
            "VTKHDF",
            AttributeBuilder::fixed_string("Type", "UnstructuredGrid"),
        )
        .group_attribute(
            "VTKHDF",
            AttributeBuilder::vector("Version", &[2i32, 0]).unwrap(),
        )
        .dataset(
            DatasetBuilder::typed_data(
                "VTKHDF/Points",
                vec![u64::try_from(points.len()).unwrap(), 3],
                &coordinates,
            )
            .unwrap(),
        );
    let mut offsets = vec![0_i64];
    let mut connectivity = Vec::new();
    for cell in cells {
        connectivity.extend(cell.iter().map(|&index| i64::try_from(index).unwrap()));
        offsets.push(i64::try_from(connectivity.len()).unwrap());
    }
    for (name, values) in [
        ("NumberOfPoints", vec![i64::try_from(points.len()).unwrap()]),
        ("NumberOfCells", vec![i64::try_from(cells.len()).unwrap()]),
        (
            "NumberOfConnectivityIds",
            vec![i64::try_from(connectivity.len()).unwrap()],
        ),
        ("Offsets", offsets),
        ("Connectivity", connectivity),
        (
            "Types",
            types
                .iter()
                .map(|&kind| i64::try_from(kind).unwrap())
                .collect(),
        ),
    ] {
        builder = builder.dataset(
            DatasetBuilder::typed_data(
                format!("VTKHDF/{name}"),
                vec![u64::try_from(values.len()).unwrap()],
                &values,
            )
            .unwrap(),
        );
    }
    builder
        .into_plan()
        .unwrap()
        .encode(WriteOptions::default())
        .unwrap()
}

/// Existing HDF lines and new nonlinear primitives coexist and render the same scene.
#[test]
fn mixed_hdf_linear_line_quadratic_edge_and_surface_preserve_geometry_and_png() {
    let directory = Directory::new();
    let points = [
        [-3., 0., 0.],
        [-2., 0., 0.],
        [-1., 0., 0.],
        [1., 0., 0.],
        [0., 1., 0.],
        [2., 0., 0.],
        [3., 0., 0.],
        [2., 1., 0.],
        [2.5, 0., 0.5],
        [2.5, 0.5, 0.5],
        [2., 0.5, 0.5],
    ]
    .map(vector);
    let cells = vec![vec![0, 1], vec![2, 3, 4], vec![5, 6, 7, 8, 9, 10]];
    let input = directory.write("mixed.vtkhdf", hdf_grid(&points, &cells, &[3, 21, 22]));
    let scene = formats::load(&input).unwrap();
    assert_eq!(scene.lines.len(), 13);
    assert_eq!(scene.triangles.len(), 36);
    assert_eq!(scene.lines[0].vertices, [points[0], points[1]]);
    contains(&scene, vector([0., 1., 0.]));
    contains(&scene, vector([2. + 1. / 6., 1. / 3., 11. / 18.]));
    let options = render::RenderOptions {
        width: 64,
        height: 64,
        transparent: true,
        supersampling: 1,
        ..render::RenderOptions::default()
    };
    let expected = render::render(&scene, &options).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_catwalk"))
        .arg(&input)
        .args([
            "--output",
            "-",
            "--size",
            "64",
            "--transparent",
            "--samples",
            "1",
        ])
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stderr, [] as [u8; 0]);
    assert_eq!(
        image::load_from_memory(&result.stdout).unwrap().to_rgba8(),
        expected
    );
}

/// Late malformed cells and oversized declared arrays leave prior CLI output intact.
#[test]
fn failed_higher_order_import_preserves_existing_output_and_empty_stdout() {
    let directory = Directory::new();
    let points = unit_nodes(22);
    let valid: Vec<_> = (0..points.len()).collect();
    let mut invalid = valid.clone();
    invalid.pop();
    let late = xml_grid(&points, &[valid, invalid], &[22, 22]);
    let oversized = xml_grid(&points, &[vec![0, 1, 2, 3, 4, 5]], &[22])
        .replace("NumberOfCells=\"1\"", "NumberOfCells=\"5000001\"");
    for (name, source) in [("late", late), ("oversized", oversized)] {
        let input = directory.write(&format!("{name}.vtu"), source);
        let output = directory.write(
            &format!("{name}.png"),
            b"retain the previous usable preview",
        );
        let result = Command::new(env!("CARGO_BIN_EXE_catwalk"))
            .arg(&input)
            .arg("--output")
            .arg(&output)
            .args(["--size", "64"])
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert_eq!(result.stdout, [] as [u8; 0]);
        assert_ne!(result.stderr, [] as [u8; 0]);
        assert_eq!(
            fs::read(&output).unwrap(),
            b"retain the previous usable preview"
        );
        let stdout = Command::new(env!("CARGO_BIN_EXE_catwalk"))
            .arg(&input)
            .args(["--output", "-", "--size", "64"])
            .output()
            .unwrap();
        assert!(!stdout.status.success());
        assert_eq!(stdout.stdout, [] as [u8; 0]);
    }
}
