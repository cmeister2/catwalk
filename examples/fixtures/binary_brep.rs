//! Original F3D BREP pair and authored native table wire/point placement controls.

use catwalk3d::Result;
use std::{fs, path::Path};

/// Corpus directory containing standalone binary and ASCII BREP examples.
const DIR: &str = "binary_brep";
/// License and source revision for the unchanged F3D pair.
const ORIGINAL: &str = "Unmodified f3d-app/f3d testing/data asset at commit 89464637f12acaed870c46cf5cdeda3b8411841b; BSD-3-Clause under the original F3D license and DATA_LICENSES mapping retained in binary_brep/F3D-LICENSE.md and binary_brep/F3D-DATA_LICENSES.md. Exact source hashes and independent native topology expectations in binary_brep/README.md. https://github.com/f3d-app/f3d/tree/89464637f12acaed870c46cf5cdeda3b8411841b/testing/data";
/// Authored diagnostics use the native layout without invoking a CAD exporter.
const AUTHORED: &str = "Author-created deterministic Rust BinTools table fixture; MIT, matching the project license. Native V1–V4 layout follows the original OCCT BinTools serializer; these authored controls qualify their wire/point and placement subsets, not historical exporter compatibility. https://github.com/Open-Cascade-SAS/OCCT/tree/V7_9_0/src/BinTools";
/// Native table version headers emitted by the original `BinTools` writer.
const HEADERS: [&str; 4] = [
    "Open CASCADE Topology V1 (c)",
    "Open CASCADE Topology V2 (c)",
    "Open CASCADE Topology V3 (c)",
    "Open CASCADE Topology V4, (c) Open Cascade",
];

/// One row-major affine placement stored as twelve little-endian binary64 values.
type Placement = [[f64; 4]; 3];

/// Append a signed native table index without depending on host endianness.
fn integer(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend(value.to_le_bytes());
}

/// Append finite authored geometry scalars as little-endian binary64 values.
fn reals(bytes: &mut Vec<u8>, values: &[f64]) {
    for value in values {
        bytes.extend(value.to_le_bytes());
    }
}

/// Append an occurrence orientation, reverse shape index and location index.
fn reference(bytes: &mut Vec<u8>, orientation: u8, reverse_id: i32, location: i32) {
    bytes.push(orientation);
    integer(bytes, reverse_id);
    integer(bytes, location);
}

/// Complete a shape with explicit native flags, children and the null-shape sentinel.
fn finish(bytes: &mut Vec<u8>, children: &[(u8, i32, i32)]) {
    bytes.extend([0, 1, 0, 1, 0, 0, 0]);
    for &(orientation, id, location) in children {
        reference(bytes, orientation, id, location);
    }
    bytes.push(b'*');
}

/// Encode a vertex with no optional point-on-curve or point-on-surface representations.
fn vertex(bytes: &mut Vec<u8>, point: [f64; 3]) {
    bytes.push(7);
    reals(bytes, &[1e-7, point[0], point[1], point[2]]);
    bytes.push(0);
    finish(bytes, &[]);
}

/// Encode an analytic line edge and its two independently stored endpoint vertices.
fn edge(bytes: &mut Vec<u8>, curve: i32, carrier: i32, length: f64, endpoints: [i32; 2]) {
    bytes.push(6);
    reals(bytes, &[1e-7]);
    bytes.extend([1, 1, 0, 1]);
    integer(bytes, curve);
    integer(bytes, carrier);
    reals(bytes, &[0., length]);
    bytes.push(0);
    finish(bytes, &[(0, endpoints[0], 0), (1, endpoints[1], 0)]);
}

/// Encode elementary placements before the geometry and topology tables.
fn header(version: usize, placements: &[Placement]) -> Vec<u8> {
    let mut bytes =
        format!("\n{}\nLocations {}\n", HEADERS[version], placements.len()).into_bytes();
    for placement in placements {
        bytes.push(1);
        for row in placement {
            reals(&mut bytes, row);
        }
    }
    bytes
}

/// Build a translated three-point compound in native topology version one.
fn point_cloud() -> Vec<u8> {
    let mut bytes = header(
        0,
        &[[[1., 0., 0., 3.], [0., 1., 0., -2.], [0., 0., 1., 1.]]],
    );
    bytes.extend(b"Curve2ds 0\nCurves 0\nPolygon3D 0\nPolygonOnTriangulations 0\nSurfaces 0\nTriangulations 0\nTShapes 4\n");
    for point in [[0., 0., 0.], [2., 0., 0.], [0., 1., 0.]] {
        vertex(&mut bytes, point);
    }
    bytes.push(0);
    finish(&mut bytes, &[(0, 4, 0), (0, 3, 0), (0, 2, 0)]);
    reference(&mut bytes, 0, 1, 1);
    bytes
}

/// Encode two local line curves and empty unused cache/surface tables.
fn line_geometry(bytes: &mut Vec<u8>, shapes: usize) {
    bytes.extend(b"Curve2ds 0\nCurves 2\n");
    bytes.push(1);
    reals(bytes, &[0., 0., 0., 1., 0., 0.]);
    bytes.push(1);
    reals(bytes, &[2., 0., 0., 0., 1., 0.]);
    bytes.extend(format!("Polygon3D 0\nPolygonOnTriangulations 0\nSurfaces 0\nTriangulations 0\nTShapes {shapes}\n").as_bytes());
}

/// Build a version-two free wire with a rotated and translated rigid curve carrier.
fn rotated_wire() -> Vec<u8> {
    let mut bytes = header(
        1,
        &[[[0., -1., 0., 1.], [1., 0., 0., 2.], [0., 0., 1., 3.]]],
    );
    line_geometry(&mut bytes, 6);
    for point in [[1., 2., 3.], [1., 4., 3.], [0., 4., 3.]] {
        vertex(&mut bytes, point);
    }
    edge(&mut bytes, 1, 1, 2., [6, 5]);
    edge(&mut bytes, 2, 1, 1., [5, 4]);
    bytes.push(5);
    finish(&mut bytes, &[(0, 3, 0), (0, 2, 0)]);
    reference(&mut bytes, 0, 1, 0);
    bytes
}

/// Build a version-three compound with a reflected carrier, rotated root and isolated point.
fn placed_wire_point() -> Vec<u8> {
    let placements = [
        [[-1., 0., 0., 1.], [0., 1., 0., 2.], [0., 0., 1., 3.]],
        [[0., -1., 0., 4.], [1., 0., 0., -2.], [0., 0., 1., 1.]],
    ];
    let mut bytes = header(2, &placements);
    line_geometry(&mut bytes, 8);
    for point in [[1., 2., 3.], [-1., 2., 3.], [-1., 3., 3.], [7., 0., 2.]] {
        vertex(&mut bytes, point);
    }
    edge(&mut bytes, 1, 1, 2., [8, 7]);
    edge(&mut bytes, 2, 1, 1., [7, 6]);
    bytes.push(5);
    finish(&mut bytes, &[(0, 4, 0), (0, 3, 0)]);
    bytes.push(0);
    finish(&mut bytes, &[(0, 2, 0), (0, 5, 0)]);
    reference(&mut bytes, 0, 1, 2);
    bytes
}

/// Build a version-four repeated wire with separate child placement and uniform root scaling.
fn repeated_wire() -> Vec<u8> {
    let placements = [
        [[1., 0., 0., 1.], [0., 1., 0., 2.], [0., 0., 1., 3.]],
        [[1., 0., 0., 4.], [0., 1., 0., 0.], [0., 0., 1., 1.]],
        [[2., 0., 0., 10.], [0., 2., 0., 0.], [0., 0., 2., -1.]],
    ];
    let mut bytes = header(3, &placements);
    line_geometry(&mut bytes, 7);
    for point in [[1., 2., 3.], [3., 2., 3.], [3., 3., 3.]] {
        vertex(&mut bytes, point);
    }
    edge(&mut bytes, 1, 1, 2., [7, 6]);
    edge(&mut bytes, 2, 1, 1., [6, 5]);
    bytes.push(5);
    finish(&mut bytes, &[(0, 4, 0), (0, 3, 0)]);
    bytes.push(0);
    finish(&mut bytes, &[(0, 2, 0), (0, 2, 2)]);
    reference(&mut bytes, 0, 1, 3);
    bytes
}

/// Load one written fixture and record its nonempty geometry for the example gallery.
fn register(
    root: &Path,
    name: &str,
    id: &str,
    title: &str,
    provenance: &str,
) -> Result<crate::Fixture> {
    let input = format!("{DIR}/{name}");
    let scene = catwalk3d::formats::load(&root.join(&input))?;
    if scene.triangles.is_empty() && scene.lines.is_empty() && scene.points.is_empty() {
        return Err(format!("BREP fixture {input} has no geometry").into());
    }
    Ok(crate::fixture(
        id, "brep", title, &input, "model", provenance,
    ))
}

/// Copy unchanged licensed BREP files and write independently measurable authored controls.
///
/// # Errors
///
/// Returns an error for failed filesystem writes or a registered model that cannot load geometry.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    let directory = root.join(DIR);
    fs::create_dir_all(&directory)?;
    for (name, bytes) in [
        (
            "f3d.bin.brep",
            include_bytes!("../../tests/fixtures/formats/binary_brep/f3d.bin.brep").as_slice(),
        ),
        (
            "f3d.brep",
            include_bytes!("../../tests/fixtures/formats/binary_brep/f3d.brep").as_slice(),
        ),
        (
            "F3D-LICENSE.md",
            include_bytes!("../../tests/fixtures/formats/binary_brep/F3D-LICENSE.md").as_slice(),
        ),
        (
            "F3D-DATA_LICENSES.md",
            include_bytes!("../../tests/fixtures/formats/binary_brep/F3D-DATA_LICENSES.md")
                .as_slice(),
        ),
    ] {
        fs::write(directory.join(name), bytes)?;
    }
    let mut cases = Vec::new();
    for (name, id, title) in [
        (
            "f3d.bin.brep",
            "brep-f3d-original-binary",
            "F3D original binary BREP compound",
        ),
        (
            "f3d.brep",
            "brep-f3d-original-ascii",
            "F3D original ASCII BREP counterpart",
        ),
    ] {
        cases.push(register(root, name, id, title, ORIGINAL)?);
    }
    for (id, title, bytes) in [
        (
            "brep-binary-v1-points",
            "Binary BREP V1 isolated points with root translation",
            point_cloud(),
        ),
        (
            "brep-binary-v2-wire",
            "Binary BREP V2 wire with rotated curve carrier",
            rotated_wire(),
        ),
        (
            "brep-binary-v3-placed-wire-points",
            "Binary BREP V3 reflected carrier and rotated wire/point compound",
            placed_wire_point(),
        ),
        (
            "brep-binary-v4-repeated-wire",
            "Binary BREP V4 repeated wires and uniformly scaled root",
            repeated_wire(),
        ),
    ] {
        let name = format!("{id}.brep");
        fs::write(directory.join(&name), bytes)?;
        cases.push(register(root, &name, id, title, AUTHORED)?);
    }
    Ok(cases)
}
