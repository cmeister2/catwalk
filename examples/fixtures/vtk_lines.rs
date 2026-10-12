//! Original scientific line cells and equivalent authored unstructured-grid encodings.

use catwalk3d::Result;
use hdf5_writer::{AttributeBuilder, DatasetBuilder, Hdf5Builder, WriteOptions};
use std::{fmt::Write as _, fs, path::Path};

/// Corpus directory containing these unstructured line-cell examples.
const DIR: &str = "vtk_lines";
/// Source revision, asset hash and separate licensing for the unchanged F3D dataset.
const ORIGINAL: &str = "Unmodified f3d-app/f3d testing/data/pdiag.vtu at commit 09ba978ad03eb40466548f340c0db7c18843a354; SHA-256 61152cf6369593d3cb4d7213496cd0fc0da298e37de156f93f965e8585b2e342. BSD-3-Clause under the original F3D license and DATA_LICENSES mapping retained in vtk_lines/F3D-LICENSE.md and vtk_lines/F3D-DATA_LICENSES.md. Original base64-appended zlib VTU contains 6992 points and 3497 line cells. https://github.com/f3d-app/f3d/blob/09ba978ad03eb40466548f340c0db7c18843a354/testing/data/pdiag.vtu";
/// License and authorship for the four reproducible mixed-cell controls.
const AUTHORED: &str =
    "Author-created deterministic Rust fixture; MIT, matching the project license";
/// Literal asymmetric coordinates shared by the legacy, XML and HDF encodings.
const POINTS: [[f64; 3]; 9] = [
    [-2., -1., 0.5],
    [-1., 2., 1.],
    [2., -2., 3.],
    [0., 0., 0.],
    [3., 1., 2.],
    [4., 3., -1.],
    [1., 5., 1.],
    [-1., 1., 2.],
    [-2., 3., 2.],
];
/// Flattened vertex, polyvertex, line, polyline and triangle connectivity.
const CONNECTIVITY: [i64; 11] = [0, 1, 2, 3, 4, 4, 5, 6, 3, 7, 8];
/// Native split-cell offsets include the initial zero used by VTK 5 and VTKHDF.
const OFFSETS: [i64; 6] = [0, 1, 3, 5, 8, 11];
/// Cell kinds in the same order as their connectivity ranges.
const TYPES: [u8; 5] = [1, 2, 3, 4, 5];

/// Encode the literal positions as deterministic, finite decimal coordinate text.
fn point_text() -> Result<String> {
    let mut text = String::new();
    for [x, y, z] in POINTS {
        writeln!(text, "{x} {y} {z}")?;
    }
    Ok(text)
}

/// Write classic ASCII count-prefixed cell lists with one surface sibling.
fn legacy_ascii() -> Result<Vec<u8>> {
    Ok(format!(
        "# vtk DataFile Version 3.0\nAuthored mixed line cells; MIT\nASCII\nDATASET UNSTRUCTURED_GRID\nPOINTS 9 double\n{}CELLS 5 16\n1 0\n2 1 2\n2 3 4\n3 4 5 6\n3 3 7 8\nCELL_TYPES 5\n1 2 3 4 5\n",
        point_text()?,
    )
    .into_bytes())
}

/// Write standard big-endian coordinates and VTK 5 split offsets/connectivity.
fn legacy_binary() -> Vec<u8> {
    let mut bytes = b"# vtk DataFile Version 5.1\nAuthored mixed line cells; MIT\nBINARY\nDATASET UNSTRUCTURED_GRID\nPOINTS 9 double\n".to_vec();
    for value in POINTS.into_iter().flatten() {
        bytes.extend(value.to_be_bytes());
    }
    bytes.extend(b"\nCELLS 6 11\nOFFSETS vtktypeint64\n");
    for offset in OFFSETS {
        bytes.extend(offset.to_be_bytes());
    }
    bytes.extend(b"\nCONNECTIVITY vtktypeint64\n");
    for index in CONNECTIVITY {
        bytes.extend(index.to_be_bytes());
    }
    bytes.extend(b"\nCELL_TYPES 5\n");
    for kind in TYPES {
        bytes.extend(i32::from(kind).to_be_bytes());
    }
    bytes.push(b'\n');
    bytes
}

/// Write an ASCII XML `UnstructuredGrid` with the same independently indexed cells.
fn xml() -> Result<Vec<u8>> {
    Ok(format!(
        "<?xml version=\"1.0\"?>\n<VTKFile type=\"UnstructuredGrid\" version=\"0.1\" byte_order=\"LittleEndian\"><UnstructuredGrid><Piece NumberOfPoints=\"9\" NumberOfCells=\"5\"><Points><DataArray type=\"Float64\" NumberOfComponents=\"3\" format=\"ascii\">{}</DataArray></Points><Cells><DataArray type=\"Int64\" Name=\"connectivity\" format=\"ascii\">0 1 2 3 4 4 5 6 3 7 8</DataArray><DataArray type=\"Int64\" Name=\"offsets\" format=\"ascii\">1 3 5 8 11</DataArray><DataArray type=\"UInt8\" Name=\"types\" format=\"ascii\">1 2 3 4 5</DataArray></Cells></Piece></UnstructuredGrid></VTKFile>\n",
        point_text()?,
    )
    .into_bytes())
}

/// Encode one typed dataset under the VTKHDF group through the published Rust writer.
fn dataset<T: hdf5_writer::H5WriteElement>(
    name: &str,
    shape: &[u64],
    values: &[T],
) -> Result<DatasetBuilder> {
    Ok(DatasetBuilder::typed_data(name, shape.to_vec(), values)?)
}

/// Write the same five cells in a static single-partition VTKHDF unstructured grid.
fn hdf() -> Result<Vec<u8>> {
    let points: Vec<_> = POINTS.into_iter().flatten().collect();
    let builder = Hdf5Builder::new()
        .group_attribute(
            "VTKHDF",
            AttributeBuilder::fixed_string("Type", "UnstructuredGrid"),
        )
        .group_attribute("VTKHDF", AttributeBuilder::vector("Version", &[2i32, 0])?)
        .dataset(dataset("VTKHDF/Points", &[9, 3], &points)?)
        .dataset(dataset("VTKHDF/NumberOfPoints", &[1], &[9i64])?)
        .dataset(dataset("VTKHDF/NumberOfCells", &[1], &[5i64])?)
        .dataset(dataset("VTKHDF/NumberOfConnectivityIds", &[1], &[11i64])?)
        .dataset(dataset("VTKHDF/Offsets", &[6], &OFFSETS)?)
        .dataset(dataset("VTKHDF/Connectivity", &[11], &CONNECTIVITY)?)
        .dataset(dataset("VTKHDF/Types", &[5], &TYPES)?);
    Ok(builder.into_plan()?.encode(WriteOptions::default())?)
}

/// Require loaded geometry before adding a model to the snapshot corpus and gallery.
fn register(
    root: &Path,
    name: &str,
    id: &str,
    extension: &str,
    title: &str,
    provenance: &str,
) -> Result<crate::Fixture> {
    let input = format!("{DIR}/{name}");
    let scene = catwalk3d::formats::load(&root.join(&input))?;
    if scene.triangles.is_empty() && scene.points.is_empty() && scene.lines.is_empty() {
        return Err(format!("VTK line fixture {input} has no geometry").into());
    }
    Ok(crate::fixture(
        id, extension, title, &input, "model", provenance,
    ))
}

/// Retain the original licensed VTU and generate four equivalent mixed-cell encodings.
///
/// # Errors
///
/// Filesystem writes, typed HDF serialization or required model loading can fail.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    let directory = root.join(DIR);
    fs::create_dir_all(&directory)?;
    for (name, bytes) in [
        (
            "pdiag.vtu",
            include_bytes!("../../tests/fixtures/formats/vtk_lines/pdiag.vtu").as_slice(),
        ),
        (
            "F3D-LICENSE.md",
            include_bytes!("../../tests/fixtures/formats/vtk_lines/F3D-LICENSE.md").as_slice(),
        ),
        (
            "F3D-DATA_LICENSES.md",
            include_bytes!("../../tests/fixtures/formats/vtk_lines/F3D-DATA_LICENSES.md")
                .as_slice(),
        ),
    ] {
        fs::write(directory.join(name), bytes)?;
    }
    let mut cases = vec![register(
        root,
        "pdiag.vtu",
        "vtu-f3d-persistence-diagram-lines",
        "vtu",
        "F3D original persistence diagram with 3497 line cells",
        ORIGINAL,
    )?];
    for (id, extension, title, bytes) in [
        (
            "vtk-unstructured-lines-ascii",
            "vtk",
            "Legacy VTK ASCII mixed line, point and surface cells",
            legacy_ascii()?,
        ),
        (
            "vtk-unstructured-lines-binary-v5",
            "vtk",
            "Legacy VTK big-endian V5 mixed line cells",
            legacy_binary(),
        ),
        (
            "vtu-unstructured-lines-ascii",
            "vtu",
            "VTU ASCII mixed line, point and surface cells",
            xml()?,
        ),
        (
            "vtkhdf-unstructured-lines",
            "vtkhdf",
            "VTKHDF mixed line, point and surface cells",
            hdf()?,
        ),
    ] {
        let name = format!("{id}.{extension}");
        fs::write(directory.join(&name), bytes)?;
        cases.push(register(root, &name, id, extension, title, AUTHORED)?);
    }
    Ok(cases)
}
