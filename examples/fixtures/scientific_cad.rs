//! Authored scientific/CAD fixtures and retained, licensed exporter examples.
use base64::{Engine, engine::general_purpose::STANDARD};
use catwalk::{Result, scene::Vec3};
use flate2::{
    Compression,
    write::{GzEncoder, ZlibEncoder},
};
use hdf5_writer::{AttributeBuilder, DatasetBuilder, Hdf5Builder, WriteOptions};
use netcdf_writer::{NcAttrValue, NcFileBuilder, NcWriteFormat, NcWriteOptions};
use ogeom_core::Tolerances;
use ogeom_math::{Frame, Point};
use ogeom_topo::{Model, Shape};
use std::{fmt::Write as _, fs, io::Write, path::Path};
use truck_modeling::{Point3, Vector3, builder};
use truck_stepio::out::{CompleteStepDisplay, StepHeaderDescriptor, StepModel};

/// License and authorship notice attached to generated fixture records.
const PROVENANCE: &str =
    "Author-created deterministic Rust fixture; MIT, matching the project license";
/// Corpus subdirectory for the scientific and CAD model inputs.
const DIR: &str = "scientific_cad";
/// Upstream license, exact source revision, and retained-asset checksum.
const CADQUERY: &str = "Unmodified CadQuery tests/testdata/red_cube_blue_cylinder.step; Apache-2.0; https://github.com/CadQuery/cadquery/blob/5f5bc62b255f62e3512105740025361b6c31eddd/tests/testdata/red_cube_blue_cylinder.step; SHA-256 1fd5c7d9325bcfb9863365faf84554f89e7e7a5e86760a0fc8d5db00619d9ef5; original license and provenance retained in scientific_cad/step-cadquery-LICENSE and step-cadquery-NOTICE.txt";
/// NIST source revision, unrestricted-reuse terms and unchanged STEP checksum.
const NIST: &str = "Unmodified NIST FTC 08 ASME1 nist_ftc_08_asme1_ap242-e4-tg.stp from usnistgov/SFA Release/NIST-PMI-STEP-Files.zip at commit 72375d17c09cf7072c30d9393b5c08b7aa272285; unrestricted reuse with NIST acknowledgement, no endorsement. SHA-256 40cd4e5e2e02b5742e0884ba5b132f2e37d2215c45f57afb48962a05a060c384; source and terms retained in scientific_cad/step-nist-NOTICE.txt. https://github.com/usnistgov/SFA/blob/72375d17c09cf7072c30d9393b5c08b7aa272285/Release/NIST-PMI-STEP-Files.zip";
/// Written corpus provenance explaining authored geometry and retained licensing.
const README: &str = r"# Scientific and CAD fixtures

Authored examples are generated deterministically by `examples/fixtures/scientific_cad.rs` and are MIT licensed with Catwalk. The STEP assembly examples use actual B-rep surfaces, repeated product occurrences and nested representation mappings, with nonzero source-frame origins and rotated/translated components. Geometry checks compare their placed vertices independently; PNG snapshots check the resulting thumbnails.

`step-cadquery-placed-bodies.step` is an unmodified upstream test asset, renamed only for the corpus, from [CadQuery at commit 5f5bc62b255f62e3512105740025361b6c31eddd](https://github.com/CadQuery/cadquery/blob/5f5bc62b255f62e3512105740025361b6c31eddd/tests/testdata/red_cube_blue_cylinder.step). It retains its Apache-2.0 license in `step-cadquery-LICENSE`; source provenance and the exact SHA-256 are recorded in `step-cadquery-NOTICE.txt`. The file contains two placed bodies and a conversion-based metre context. It checks curved surfaces and millimeter unit conversion, without claiming an independently exported assembly hierarchy. STEP material colors are not currently rendered.

`step-nist-ap242-tessellated.stp` is the unchanged NIST FTC 08 ASME1 AP242 export from [the pinned NIST archive](https://github.com/usnistgov/SFA/blob/72375d17c09cf7072c30d9393b5c08b7aa272285/Release/NIST-PMI-STEP-Files.zip), renamed only for the corpus. [Source and reuse terms](step-nist-NOTICE.txt) acknowledge NIST and retain its unrestricted-reuse terms. The snapshot renders the product-owned tessellated part with its authored normals. Geometry tests check triangle count, bounds and the producer's embedded surface area, while excluding construction geometry, validation copies and drafting views. This is one real export; fans, surface sets and linked B-rep alternatives have separate authored tests.

Regeneration uses Rust and retained fixture bytes; no CAD application, native writer or network is required. The separately licensed upstream asset is not relabeled as an authored fixture.
";

/// Writes corpus resources and records models after checking they load geometry.
struct Fixtures<'a> {
    /// Output corpus root containing the scientific/CAD subdirectory.
    root: &'a Path,
    /// Manifest entries for successfully validated model inputs.
    cases: Vec<crate::Fixture>,
}
impl Fixtures<'_> {
    /// Write a model, sidecar, or license resource into the corpus subdirectory.
    ///
    /// # Errors
    /// Returns an error if the resource cannot be written to the filesystem.
    fn resource(&self, name: &str, bytes: &[u8]) -> Result<()> {
        fs::write(self.root.join(DIR).join(name), bytes)?;
        Ok(())
    }
    /// Write an authored model and register its geometry and provenance.
    ///
    /// # Errors
    /// Returns an error if the model cannot be written, loaded, or produces no
    /// triangles, points, or lines.
    fn add(&mut self, id: &str, extension: &str, title: &str, bytes: &[u8]) -> Result<()> {
        let name = format!("{id}.{extension}");
        self.resource(&name, bytes)?;
        self.register(id, extension, title, PROVENANCE)
    }
    /// Validate a written model and append its gallery manifest entry.
    ///
    /// # Errors
    /// Returns an error if the model cannot be loaded or has no geometry.
    fn register(&mut self, id: &str, extension: &str, title: &str, provenance: &str) -> Result<()> {
        let input = format!("{DIR}/{id}.{extension}");
        let scene = catwalk::formats::load(&self.root.join(&input))?;
        if scene.triangles.is_empty() && scene.points.is_empty() && scene.lines.is_empty() {
            return Err(format!("generated {input} has no geometry").into());
        }
        self.cases.push(crate::fixture(
            id, extension, title, &input, "model", provenance,
        ));
        Ok(())
    }
}

/// Write and validate the scientific, volume, and CAD fixture families.
///
/// # Errors
/// Returns an error if model construction or serialization fails, a corpus file
/// cannot be written, or a generated or retained input cannot load geometry.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    fs::create_dir_all(root.join(DIR))?;
    let mut f = Fixtures {
        root,
        cases: Vec::new(),
    };
    scientific(&mut f)?;
    containers(&mut f)?;
    volumes(&mut f)?;
    cad(&mut f)?;
    f.add(
        "ifc-hollow-extrusion",
        "ifc",
        "IFC hollow rectangular extrusion",
        ifc().as_bytes(),
    )?;
    f.add(
        "vdb-hollow-float",
        "vdb",
        "OpenVDB compressed sparse hollow shell",
        &vdb(false)?,
    )?;
    f.add(
        "vdb-hollow-half",
        "vdb",
        "OpenVDB half-float shell with reflected placement",
        &vdb(true)?,
    )?;
    f.add(
        "xbf-colored-carrier",
        "xbf",
        "XBF colored face with genuine carrier and instance placement",
        &xbf(),
    )?;
    f.resource("README.md", README.as_bytes())?;
    Ok(f.cases)
}

/// Emit legacy and XML VTK meshes, structured grids, and multiblock references.
///
/// # Errors
/// Returns an error if compression, writing, or geometry validation fails.
fn scientific(f: &mut Fixtures<'_>) -> Result<()> {
    let vtk = b"# vtk DataFile Version 3.0\nTwo tetrahedra sharing a face\nASCII\nDATASET UNSTRUCTURED_GRID\nPOINTS 5 float\n0 0 0 2 0 0 0 3 0 0 0 4 0 0 -2\nCELLS 2 10\n4 0 1 2 3\n4 0 2 1 4\nCELL_TYPES 2\n10 10\n";
    f.add(
        "vtk-shared-tetrahedra",
        "vtk",
        "VTK legacy ASCII shared tetrahedral surface",
        vtk,
    )?;
    let mut binary = b"# vtk DataFile Version 3.0\nBinary tetrahedron\nBINARY\nDATASET UNSTRUCTURED_GRID\nPOINTS 4 double\n".to_vec();
    for v in [0f64, 0., 0., 2., 0., 0., 0., 3., 0., 0., 0., 4.] {
        binary.extend(v.to_be_bytes());
    }
    binary.extend(b"\nCELLS 1 5\n");
    for v in [4i32, 0, 1, 2, 3] {
        binary.extend(v.to_be_bytes());
    }
    binary.extend(b"\nCELL_TYPES 1\n");
    binary.extend(10i32.to_be_bytes());
    binary.push(b'\n');
    f.add(
        "vtk-big-endian-binary",
        "vtk",
        "VTK big-endian binary tetrahedron",
        &binary,
    )?;
    f.add("vtp-concave-polygon", "vtp", "VTK XML concave polygon and raised points", br#"<VTKFile type="PolyData"><PolyData><Piece NumberOfPoints="8" NumberOfVerts="1" NumberOfLines="0" NumberOfStrips="0" NumberOfPolys="1"><Points><DataArray type="Float64" NumberOfComponents="3">0 0 0 3 0 0 3 1 0 1 1 0 1 3 0 0 3 0 0 0 1 3 0 2</DataArray></Points><Verts><DataArray type="Int32" Name="connectivity">6 7</DataArray><DataArray type="Int32" Name="offsets">2</DataArray></Verts><Polys><DataArray type="Int32" Name="connectivity">0 1 2 3 4 5</DataArray><DataArray type="Int32" Name="offsets">6</DataArray></Polys></Piece></PolyData></VTKFile>"#)?;
    let vtu = br#"<VTKFile type="UnstructuredGrid"><UnstructuredGrid><Piece NumberOfPoints="4" NumberOfCells="1"><Points><DataArray type="Float64" NumberOfComponents="3">0 0 0 2 0 0 0 3 0 0 0 4</DataArray></Points><Cells><DataArray type="Int64" Name="connectivity">0 1 2 3</DataArray><DataArray type="Int64" Name="offsets">4</DataArray><DataArray type="UInt8" Name="types">10</DataArray></Cells></Piece></UnstructuredGrid></VTKFile>"#;
    f.add(
        "vtu-ascii-tetrahedron",
        "vtu",
        "VTK XML indexed tetrahedron",
        vtu,
    )?;
    f.add(
        "vtu-zlib-binary",
        "vtu",
        "VTK XML inline compressed binary arrays",
        &compressed_vtu()?,
    )?;
    f.add("vtr-nonuniform-grid", "vtr", "Rectilinear grid with nonuniform coordinates", br#"<VTKFile type="RectilinearGrid"><RectilinearGrid WholeExtent="-1 1 4 5 0 1"><Piece Extent="-1 1 4 5 0 1"><Coordinates><DataArray type="Float64">1 2 4</DataArray><DataArray type="Float64">2 5</DataArray><DataArray type="Float64">3 7</DataArray></Coordinates></Piece></RectilinearGrid></VTKFile>"#)?;
    let mut points = String::new();
    for z in [0., 4.] {
        for y in [0., 3.] {
            for x in [0., 2.] {
                let _ = write!(points, "{} {y} {z} ", x + z * 0.25);
            }
        }
    }
    let vts = format!(
        r#"<VTKFile type="StructuredGrid"><StructuredGrid WholeExtent="0 1 0 1 0 1"><Piece Extent="0 1 0 1 0 1"><Points><DataArray type="Float64" NumberOfComponents="3">{points}</DataArray></Points></Piece></StructuredGrid></VTKFile>"#
    );
    f.add(
        "vts-sheared-grid",
        "vts",
        "Structured grid preserving sheared authored coordinates",
        vts.as_bytes(),
    )?;
    f.add("vtm-mixed-blocks", "vtm", "Nested multiblock tetrahedron and concave sheet", br#"<VTKFile type="vtkMultiBlockDataSet"><vtkMultiBlockDataSet><Block index="0"><DataSet index="0" file="vtu-ascii-tetrahedron.vtu"/></Block><DataSet index="1" file="vtp-concave-polygon.vtp"/></vtkMultiBlockDataSet></VTKFile>"#)?;
    Ok(())
}

/// Compress bytes with a gzip wrapper or a zlib wrapper at the fixture level.
///
/// # Errors
/// Returns an error if the selected compressor cannot write or finish its data.
fn zip(raw: &[u8], gzip: bool) -> Result<Vec<u8>> {
    if gzip {
        let mut z = GzEncoder::new(Vec::new(), Compression::fast());
        z.write_all(raw)?;
        Ok(z.finish()?)
    } else {
        let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
        z.write_all(raw)?;
        Ok(z.finish()?)
    }
}
/// Encode one tetrahedron with separately base64-encoded zlib VTK arrays.
///
/// # Errors
/// Returns an error if one of the four authored arrays cannot be compressed.
#[allow(
    clippy::cast_possible_truncation,
    reason = "The four fixed tetrahedron arrays and their compressed payloads are much smaller than the VTK UInt32 length fields."
)]
fn compressed_vtu() -> Result<Vec<u8>> {
    let arrays = [
        (
            "Points",
            "Float64",
            3,
            [0f64, 0., 0., 2., 0., 0., 0., 3., 0., 0., 0., 4.]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        ),
        (
            "connectivity",
            "Int64",
            1,
            [0i64, 1, 2, 3]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect(),
        ),
        ("offsets", "Int64", 1, 4i64.to_le_bytes().to_vec()),
        ("types", "UInt8", 1, vec![10]),
    ];
    let mut tags = Vec::new();
    for (name, ty, n, raw) in arrays {
        let zipped = zip(&raw, false)?;
        let header = [
            1u32,
            raw.len() as u32,
            raw.len() as u32,
            zipped.len() as u32,
        ]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect::<Vec<_>>();
        let encoded = format!("{}{}", STANDARD.encode(header), STANDARD.encode(zipped));
        tags.push(format!(r#"<DataArray Name="{name}" type="{ty}" NumberOfComponents="{n}" format="binary">{encoded}</DataArray>"#));
    }
    Ok(format!(r#"<VTKFile type="UnstructuredGrid" byte_order="LittleEndian" header_type="UInt32" compressor="vtkZLibDataCompressor"><UnstructuredGrid><Piece NumberOfPoints="4" NumberOfCells="1"><Points>{}</Points><Cells>{}{}{}</Cells></Piece></UnstructuredGrid></VTKFile>"#,tags[0],tags[1],tags[2],tags[3]).into_bytes())
}

/// Construct a typed HDF5 dataset for one authored topology or coordinate array.
///
/// # Errors
/// Returns an error if the dataset shape and typed values cannot be serialized.
fn hdf<T: hdf5_writer::H5WriteElement>(
    name: &str,
    shape: &[u64],
    data: &[T],
) -> Result<DatasetBuilder> {
    Ok(DatasetBuilder::typed_data(name, shape.to_vec(), data)?)
}
/// Emit filtered VTKHDF and the supported Exodus and scalar `NetCDF` variants.
///
/// # Errors
/// Returns an error if container construction, serialization, writing, or model
/// geometry validation fails.
fn containers(f: &mut Fixtures<'_>) -> Result<()> {
    let points = [
        0f32, 0., 0., 2., 0., 0., 0., 3., 0., 0., 0., 4., 0., 0., -2.,
    ];
    let compressed = DatasetBuilder::typed::<f32>("VTKHDF/Points", vec![5, 3])
        .data(&points)?
        .chunked(vec![5, 3])
        .filter(hdf5_writer::FilterDescription {
            id: 2,
            name: None,
            client_data: vec![4],
        })
        .filter(hdf5_writer::FilterDescription {
            id: 1,
            name: None,
            client_data: vec![6],
        });
    let b = Hdf5Builder::new()
        .group_attribute(
            "VTKHDF",
            AttributeBuilder::fixed_string("Type", "UnstructuredGrid"),
        )
        .group_attribute("VTKHDF", AttributeBuilder::vector("Version", &[2i32, 0])?)
        .dataset(compressed)
        .dataset(hdf("VTKHDF/NumberOfPoints", &[1], &[5i64])?)
        .dataset(hdf("VTKHDF/NumberOfCells", &[1], &[2i32])?)
        .dataset(hdf("VTKHDF/NumberOfConnectivityIds", &[1], &[8i64])?)
        .dataset(hdf("VTKHDF/Offsets", &[3], &[0i64, 4, 8])?)
        .dataset(hdf(
            "VTKHDF/Connectivity",
            &[8],
            &[0i64, 1, 2, 3, 0, 2, 1, 4],
        )?)
        .dataset(hdf("VTKHDF/Types", &[2], &[10u8, 10])?);
    f.add(
        "vtkhdf-filtered-tetrahedra",
        "vtkhdf",
        "VTKHDF shuffle/deflate shared-cell boundary",
        &b.into_plan()?.encode(WriteOptions::default())?,
    )?;
    for (ext, format, title) in [
        (
            "e",
            NcWriteFormat::Classic,
            "Exodus CDF1 two element blocks",
        ),
        ("ex2", NcWriteFormat::Offset64, "Exodus CDF2 64-bit offsets"),
        (
            "exo",
            NcWriteFormat::Cdf5,
            "Exodus CDF5 shared tetrahedral boundary",
        ),
        ("g", NcWriteFormat::Nc4, "Exodus NetCDF4/HDF5 geometry"),
    ] {
        f.add(
            &format!("exodus-{ext}-blocks"),
            ext,
            title,
            &exodus(format)?,
        )?;
    }
    for (ext, format, title) in [
        (
            "nc",
            NcWriteFormat::Nc4,
            "NetCDF4 packed compressed first-time scalar volume",
        ),
        (
            "cdf",
            NcWriteFormat::Classic,
            "NetCDF classic nonuniform descending axes",
        ),
        (
            "ncdf",
            NcWriteFormat::Cdf5,
            "NetCDF CDF5 density and missing samples",
        ),
    ] {
        f.add(
            &format!("netcdf-{ext}-density"),
            ext,
            title,
            &netcdf(format)?,
        )?;
    }
    Ok(())
}
/// Serialize two shared-face tetrahedron blocks using a selected `NetCDF` format.
///
/// # Errors
/// Returns an error if dimensions, attributes, variables, or the final Exodus
/// container cannot be written by the `NetCDF` builder.
fn exodus(format: NcWriteFormat) -> Result<Vec<u8>> {
    let mut b = NcFileBuilder::new();
    b.add_dimension("num_dim", 3)?;
    let nodes = b.add_dimension("num_nodes", 5)?;
    b.add_dimension("num_elem", 2)?;
    let blocks = b.add_dimension("num_el_blk", 2)?;
    let status = b.add_variable::<i32>("eb_status", &[blocks])?;
    b.write_variable(status, &[1, 1])?;
    for (name, values) in [
        ("coordx", [0f64, 2., 0., 0., 0.]),
        ("coordy", [0., 0., 3., 0., 0.]),
        ("coordz", [0., 0., 0., 4., -2.]),
    ] {
        let v = b.add_variable::<f64>(name, &[nodes])?;
        b.write_variable(v, &values)?;
    }
    for (i, conn) in [[1i32, 2, 3, 4], [1, 3, 2, 5]].iter().enumerate() {
        let elems = b.add_dimension(format!("num_el_in_blk{}", i + 1), 1)?;
        let corners = b.add_dimension(format!("num_nod_per_el{}", i + 1), 4)?;
        let v = b.add_variable::<i32>(format!("connect{}", i + 1), &[elems, corners])?;
        b.add_variable_attribute(v, "elem_type", NcAttrValue::Chars("TETRA4".into()))?;
        b.write_variable(v, conn)?;
    }
    Ok(b.to_vec(NcWriteOptions { format })?.1)
}
/// Serialize a packed two-time density field with descending, nonuniform axes.
///
/// # Errors
/// Returns an error if the `NetCDF` builder cannot define or write the dataset,
/// configure compression, or serialize the selected container format.
fn netcdf(format: NcWriteFormat) -> Result<Vec<u8>> {
    let mut builder = NcFileBuilder::new();
    let time = builder.add_dimension("time", 2)?;
    let z_axis = builder.add_dimension("z", 3)?;
    let y_axis = builder.add_dimension("y", 3)?;
    let x_axis = builder.add_dimension("x", 4)?;
    for (name, dim, values) in [
        ("x", x_axis, vec![0f64, 1., 3., 6.]),
        ("y", y_axis, vec![0., 2., 4.]),
        ("z", z_axis, vec![5., 3., 0.]),
    ] {
        let variable = builder.add_variable::<f64>(name, &[dim])?;
        builder.write_variable(variable, &values)?;
    }
    let variable = builder.add_variable::<i16>("density", &[time, z_axis, y_axis, x_axis])?;
    builder.add_variable_attribute(variable, "scale_factor", NcAttrValue::Doubles(vec![2.]))?;
    builder.set_variable_fill_value(variable, -32767i16)?;
    let mut data = vec![0i16; 72];
    for z in 0..3 {
        for y in 0..3 {
            for x in 0..4 {
                if (x == 1 && y <= 1) || (x == 2 && y == 1 && z == 1) {
                    data[z * 12 + y * 4 + x] = 5;
                }
            }
        }
    }
    data[0] = -32767;
    data[36..].fill(5);
    builder.write_variable(variable, &data)?;
    if matches!(format, NcWriteFormat::Nc4) {
        builder.set_variable_chunking(variable, vec![1, 3, 3, 4])?;
        builder.set_variable_deflate(variable, Some(6), true)?;
    }
    Ok(builder.to_vec(NcWriteOptions { format })?.1)
}

/// Produce a five-cubed scalar grid containing three intersecting occupied planes.
fn density() -> Vec<u8> {
    let mut values = vec![0; 125];
    for z in 0..5 {
        for y in 0..5 {
            for x in 0..5 {
                if (x == 1 && y >= 1 && z >= 1)
                    || (y == 1 && x >= 1 && z >= 1)
                    || (z == 1 && x >= 1 && y >= 1)
                {
                    values[z * 25 + y * 5 + x] = 10;
                }
            }
        }
    }
    values
}
/// Emit the same oriented density grid in VTI, NRRD, and `MetaImage` containers.
///
/// # Errors
/// Returns an error if density compression, resource writing, or loaded geometry
/// validation fails.
fn volumes(f: &mut Fixtures<'_>) -> Result<()> {
    let values = density();
    let ascii = values
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    let vti = format!(
        r#"<VTKFile type="ImageData"><ImageData WholeExtent="0 4 0 4 0 4" Origin="10 20 30" Spacing="1 1.5 2" Direction="0 -1 0 1 0 0 0 0 -1"><Piece Extent="0 4 0 4 0 4"><PointData Scalars="density"><DataArray type="UInt8" Name="density">{ascii}</DataArray></PointData></Piece></ImageData></VTKFile>"#
    );
    f.add(
        "vti-oriented-density",
        "vti",
        "VTI sparse trihedral density with reflected directions",
        vti.as_bytes(),
    )?;
    let raw = values
        .iter()
        .flat_map(|&v| f32::from(v).to_le_bytes())
        .collect::<Vec<_>>();
    let header=b"NRRD0005\ntype: float\ndimension: 3\nsizes: 5 5 5\nencoding: gzip\nendian: little\nspace directions: (0,1,0) (-1.5,0,0) (0,0,-2)\nspace origin: (10,20,30)\n\n";
    f.add(
        "nrrd-gzip-density",
        "nrrd",
        "NRRD gzip float density and authored directions",
        &[header.as_slice(), &zip(&raw, true)?].concat(),
    )?;
    let raw = values
        .iter()
        .flat_map(|&v| u16::from(v).to_be_bytes())
        .collect::<Vec<_>>();
    f.resource("nhdr-density.raw", &raw)?;
    f.add("nhdr-detached-big-endian","nhdr","NRRD detached big-endian unsigned short volume",b"NRRD0005\ntype: ushort\ndimension: 3\nsizes: 5 5 5\nencoding: raw\nendian: big\nspace directions: (0,1,0) (-1.5,0,0) (0,0,-2)\nspace origin: (10,20,30)\ndata file: nhdr-density.raw\n\n")?;
    let zipped = zip(&raw, false)?;
    f.resource("mhd-density.zraw", &zipped)?;
    let common = "ObjectType = Image\nNDims = 3\nDimSize = 5 5 5\nElementType = MET_USHORT\nElementSpacing = 1 1.5 2\nTransformMatrix = 0 1 0 -1 0 0 0 0 -1\nOffset = 10 20 30\nBinaryDataByteOrderMSB = True\nCompressedData = True\n";
    f.add(
        "mhd-detached-zlib",
        "mhd",
        "MetaImage detached zlib volume with orientation",
        format!("{common}ElementDataFile = mhd-density.zraw\n").as_bytes(),
    )?;
    f.add(
        "mha-local-zlib",
        "mha",
        "MetaImage embedded big-endian zlib density",
        &[
            format!("{common}ElementDataFile = LOCAL\n").as_bytes(),
            &zipped,
        ]
        .concat(),
    )?;
    Ok(())
}

/// Serialize a swept STEP box at a nonzero source origin with a fixed header.
fn step_box(size: [f64; 3]) -> String {
    let vertex = builder::vertex(Point3::new(2., 3., 4.));
    let edge = builder::tsweep(&vertex, Vector3::new(size[0], 0., 0.));
    let face = builder::tsweep(&edge, Vector3::new(0., size[1], 0.));
    let solid = builder::tsweep(&face, Vector3::new(0., 0., size[2])).compress();
    CompleteStepDisplay::new(
        StepModel::from(&solid),
        StepHeaderDescriptor {
            file_name: "author-created-box".into(),
            time_stamp: "2026-01-01T00:00:00".into(),
            ..StepHeaderDescriptor::default()
        },
    )
    .to_string()
}
/// Serialize a swept circular disk as a STEP cylinder with a fixed header.
///
/// # Errors
/// Returns an error if the circular wire cannot be attached to a planar disk.
fn step_cylinder() -> Result<String> {
    let vertex = builder::vertex(Point3::new(1., 0., 0.));
    let circle = builder::rsweep(
        &vertex,
        Point3::new(0., 0., 0.),
        Vector3::unit_z(),
        truck_modeling::Rad(std::f64::consts::TAU),
    );
    let disk = builder::try_attach_plane(&[circle])?;
    let cylinder = builder::tsweep(&disk, Vector3::new(0., 0., 2.)).compress();
    Ok(CompleteStepDisplay::new(
        StepModel::from(&cylinder),
        StepHeaderDescriptor {
            file_name: "author-created-cylinder".into(),
            time_stamp: "2026-01-01T00:00:00".into(),
            ..StepHeaderDescriptor::default()
        },
    )
    .to_string())
}

/// Add placed product occurrences or nested representation mappings to a STEP box.
///
/// # Errors
/// Returns an error if the underlying STEP writer changes the expected part
/// representation layout or emits no terminating DATA section marker.
fn step_assembly(occurrences: bool) -> Result<String> {
    let mut source = step_box([2., 3., 4.]);
    let entities = if occurrences {
        // Include the source frame in the part representation, matching the
        // item-defined transformation references used by assembly exporters.
        let items = "(#16), #11";
        if !source.contains(items) {
            return Err("STEP fixture writer changed its representation layout".into());
        }
        source = source.replace(items, "(#16, #1003), #11");
        r"
#1000=CARTESIAN_POINT('',(2.,3.,4.));
#1001=DIRECTION('',(0.,0.,1.));
#1002=DIRECTION('',(1.,0.,0.));
#1003=AXIS2_PLACEMENT_3D('',#1000,#1001,#1002);
#1010=CARTESIAN_POINT('',(10.,20.,30.));
#1011=DIRECTION('',(0.,1.,0.));
#1012=AXIS2_PLACEMENT_3D('',#1010,#1001,#1011);
#1020=CARTESIAN_POINT('',(-10.,-20.,-30.));
#1022=AXIS2_PLACEMENT_3D('',#1020,#1001,#1002);
#1100=SHAPE_REPRESENTATION('parent',(#1012,#1022),#11);
#1101=SHAPE_DEFINITION_REPRESENTATION(#1102,#1100);
#1102=PRODUCT_DEFINITION_SHAPE('','',#1103);
#1103=PRODUCT_DEFINITION('assembly','',#1104,#9);
#1104=PRODUCT_DEFINITION_FORMATION('','',#1105);
#1105=PRODUCT('assembly','','',(#8));
#1110=(REPRESENTATION_RELATIONSHIP('','',#10,#1100) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#1111) SHAPE_REPRESENTATION_RELATIONSHIP());
#1111=ITEM_DEFINED_TRANSFORMATION('','',#1003,#1012);
#1112=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#1110,#1113);
#1113=PRODUCT_DEFINITION_SHAPE('','',#1114);
#1114=NEXT_ASSEMBLY_USAGE_OCCURRENCE('first','','',#1103,#5,$);
#1120=(REPRESENTATION_RELATIONSHIP('','',#10,#1100) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#1121) SHAPE_REPRESENTATION_RELATIONSHIP());
#1121=ITEM_DEFINED_TRANSFORMATION('','',#1003,#1022);
#1122=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#1120,#1123);
#1123=PRODUCT_DEFINITION_SHAPE('','',#1124);
#1124=NEXT_ASSEMBLY_USAGE_OCCURRENCE('second','','',#1103,#5,$);
"
    } else {
        // Four instances reuse one asymmetric solid through two mapping
        // levels. Nonzero map origins must be subtracted before placement.
        r"
#1000=CARTESIAN_POINT('',(2.,3.,4.));
#1001=DIRECTION('',(0.,0.,1.));
#1002=DIRECTION('',(1.,0.,0.));
#1003=AXIS2_PLACEMENT_3D('',#1000,#1001,#1002);
#1004=REPRESENTATION_MAP(#1003,#10);
#1010=CARTESIAN_POINT('',(10.,0.,0.));
#1011=DIRECTION('',(0.,1.,0.));
#1012=AXIS2_PLACEMENT_3D('',#1010,#1001,#1011);
#1013=MAPPED_ITEM('',#1004,#1012);
#1020=CARTESIAN_POINT('',(0.,8.,1.));
#1021=DIRECTION('',(0.,0.,-1.));
#1022=AXIS2_PLACEMENT_3D('',#1020,#1021,#1002);
#1023=MAPPED_ITEM('',#1004,#1022);
#1030=SHAPE_REPRESENTATION('repeated asymmetric parts',(#1013,#1023),#11);
#1031=CARTESIAN_POINT('',(5.,2.,1.));
#1032=AXIS2_PLACEMENT_3D('',#1031,#1001,#1002);
#1033=REPRESENTATION_MAP(#1032,#1030);
#1040=CARTESIAN_POINT('',(0.,0.,0.));
#1041=AXIS2_PLACEMENT_3D('',#1040,#1001,#1002);
#1042=MAPPED_ITEM('',#1033,#1041);
#1050=CARTESIAN_POINT('',(0.,20.,5.));
#1051=AXIS2_PLACEMENT_3D('',#1050,#1001,#1011);
#1052=MAPPED_ITEM('',#1033,#1051);
#1060=SHAPE_REPRESENTATION('nested repeated assembly',(#1042,#1052),#11);
#1070=SHAPE_DEFINITION_REPRESENTATION(#1071,#1060);
#1071=PRODUCT_DEFINITION_SHAPE('','',#1072);
#1072=PRODUCT_DEFINITION('assembly','',#1073,#9);
#1073=PRODUCT_DEFINITION_FORMATION('','',#1074);
#1074=PRODUCT('nested mapped assembly','','',(#8));
"
    };
    let end = source
        .rfind("ENDSEC;")
        .ok_or("STEP fixture has no DATA end")?;
    Ok(format!("{}{entities}\n{}", &source[..end], &source[end..]))
}

/// Build a translated analytic box, cylinder, or sphere in the CAD kernel.
///
/// # Errors
/// Returns an error if the requested body is unknown or the kernel cannot
/// construct its bounded analytic geometry.
fn cad_body(kind: &str) -> Result<(Model, Shape)> {
    let mut model = Model::new();
    let tol = Tolerances::millimetres();
    let frame = Frame::WORLD.with_origin(Point::new(7., -2., 5.));
    let root = match kind {
        "box" => ogeom_algo::make_box(&mut model, frame, (2., 3., 4.), tol)?,
        "cylinder" => ogeom_algo::make_cylinder(&mut model, frame, 2., 3., tol)?,
        "sphere" => ogeom_algo::make_sphere(&mut model, frame, 2., tol)?,
        _ => return Err("unknown fixture CAD body".into()),
    };
    Ok((model, root.shape))
}
/// Emit authored STEP assemblies, IGES bodies, BREP solids, and retained STEP data.
///
/// # Errors
/// Returns an error if CAD construction or serialization, resource writing, or
/// imported geometry validation fails.
#[allow(
    clippy::cast_precision_loss,
    reason = "The STEP extension list has four entries, so its enumeration indices convert exactly to f64 dimensions."
)]
fn cad(f: &mut Fixtures<'_>) -> Result<()> {
    for (i, ext) in ["step", "stpnc", "p21", "210"].into_iter().enumerate() {
        f.add(
            &format!("step-{ext}-translated-box"),
            ext,
            &format!("STEP translated box through .{ext} alias"),
            step_box([2. + i as f64 * 0.25, 3., 4.]).as_bytes(),
        )?;
    }
    f.add(
        "step-stp-cylinder",
        "stp",
        "STEP exact cylindrical solid tessellation",
        step_cylinder()?.as_bytes(),
    )?;
    f.add(
        "step-nested-mapped-assembly",
        "step",
        "STEP nested mappings with four placed part instances",
        step_assembly(false)?.as_bytes(),
    )?;
    f.add(
        "step-repeated-product-assembly",
        "step",
        "STEP repeated product occurrences with rotation and translation",
        step_assembly(true)?.as_bytes(),
    )?;
    f.resource(
        "step-cadquery-placed-bodies.step",
        include_bytes!(
            "../../tests/fixtures/formats/scientific_cad/step-cadquery-placed-bodies.step"
        ),
    )?;
    f.register(
        "step-cadquery-placed-bodies",
        "step",
        "STEP independently exported cube and cylinder with metre units",
        CADQUERY,
    )?;
    f.resource(
        "step-nist-ap242-tessellated.stp",
        include_bytes!(
            "../../tests/fixtures/formats/scientific_cad/step-nist-ap242-tessellated.stp"
        ),
    )?;
    f.register(
        "step-nist-ap242-tessellated",
        "stp",
        "NIST AP242 tessellated part with authored surface normals",
        NIST,
    )?;
    for (ext, kind) in [("iges", "box"), ("igs", "cylinder")] {
        let (model, root) = cad_body(kind)?;
        let mut document = ogeom_doc::Document::over(model);
        document.add_part("authored translated body", root);
        let bytes = ogeom_io::write_iges(&document, Tolerances::millimetres())?;
        f.add(
            &format!("iges-{ext}-{kind}"),
            ext,
            &format!("IGES translated {kind} geometry"),
            bytes.as_bytes(),
        )?;
    }
    let (model, root) = cad_body("cylinder")?;
    let brep = ogeom_io::brep::write(&model, &root, Tolerances::millimetres())?;
    f.add(
        "brep-analytic-cylinder",
        "brep",
        "Open CASCADE analytic cylindrical BREP",
        brep.as_bytes(),
    )?;
    let (model, root) = cad_body("box")?;
    let brep = ogeom_io::brep::write(&model, &root, Tolerances::millimetres())?;
    f.add(
        "brep-planar-box",
        "brep",
        "Open CASCADE translated planar topology",
        brep.as_bytes(),
    )?;
    Ok(())
}

/// Return an IFC4 hollow rectangular extrusion with explicit product placement.
fn ifc() -> String {
    "ISO-10303-21;HEADER;FILE_SCHEMA(('IFC4'));ENDSEC;DATA;\n#1=IFCCARTESIANPOINT((0.,0.,0.));#2=IFCAXIS2PLACEMENT3D(#1,$,$);#3=IFCLOCALPLACEMENT($,#2);\n#4=IFCBUILDINGELEMENTPROXY('id',$,'shape',$,$,#3,#5,$,$);#5=IFCPRODUCTDEFINITIONSHAPE($,$,(#6));#6=IFCSHAPEREPRESENTATION($,'Body','SolidModel',(#20));\n#20=IFCEXTRUDEDAREASOLID(#21,$,#22,2.);#21=IFCRECTANGLEHOLLOWPROFILEDEF(.AREA.,$,$,4.,4.,1.,$,$);#22=IFCDIRECTION((0.,0.,1.));\nENDSEC;END-ISO-10303-21;".into()
}

/// Encode a compressed hollow `OpenVDB` leaf as float or reflected half-float data.
///
/// # Errors
/// Returns an error if the fixed leaf values cannot be zlib-compressed.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "Fixed grid names and one 512-voxel compressed leaf fit their native string and signed payload-length fields."
)]
fn vdb(half: bool) -> Result<Vec<u8>> {
    let name = |bytes: &mut Vec<u8>, value: &str| {
        bytes.extend((value.len() as u32).to_le_bytes());
        bytes.extend(value.as_bytes());
    };
    let mut topology = Vec::new();
    topology.extend(1u32.to_le_bytes());
    topology.extend(0f32.to_le_bytes());
    topology.extend(0u32.to_le_bytes());
    topology.extend(1u32.to_le_bytes());
    topology.extend([0u8; 12]);
    for dimension in [32768, 4096] {
        let mut children = vec![0u8; dimension / 8];
        children[0] = 1;
        topology.extend(children);
        topology.extend(vec![0; dimension / 8]);
        topology.push(0);
        if !half {
            topology.extend(0i64.to_le_bytes());
        }
    }
    topology.extend([255u8; 64]);
    let mut payload = vec![255u8; 64];
    payload.push(6);
    let mut values = Vec::new();
    for x in 0..8 {
        for y in 0..8 {
            for z in 0..8 {
                let occupied = [x, y, z].iter().any(|&i| i == 0 || i == 7);
                if half {
                    values.extend(if occupied { 0x3c00u16 } else { 0 }.to_le_bytes());
                } else {
                    values.extend(if occupied { 1f32 } else { 0. }.to_le_bytes());
                }
            }
        }
    }
    let zipped = zip(&values, false)?;
    payload.extend((zipped.len() as i64).to_le_bytes());
    payload.extend(zipped);
    let mut grid = Vec::new();
    grid.extend(3u32.to_le_bytes());
    grid.extend(0u32.to_le_bytes());
    name(&mut grid, "ScaleTranslateMap");
    for value in [
        12.,
        -3.,
        7.,
        2.,
        -3.,
        5.,
        2.,
        3.,
        5.,
        0.5,
        -1. / 3.,
        0.2,
        0.25,
        1. / 9.,
        0.04,
        0.25,
        -1. / 6.,
        0.1,
    ] {
        grid.extend(f64::to_le_bytes(value));
    }
    let mut file = Vec::new();
    file.extend(0x5644_4220_u64.to_le_bytes());
    for n in [223u32, 12, 1] {
        file.extend(n.to_le_bytes());
    }
    file.push(1);
    file.extend([b'0'; 36]);
    file.extend(0u32.to_le_bytes());
    file.extend(1u32.to_le_bytes());
    name(&mut file, "density");
    name(
        &mut file,
        if half {
            "Tree_float_5_4_3_HalfFloat"
        } else {
            "Tree_float_5_4_3"
        },
    );
    name(&mut file, "");
    let start = file.len() + 24;
    let block = start + grid.len() + topology.len();
    let end = block + payload.len();
    for p in [start, block, end] {
        file.extend((p as u64).to_le_bytes());
    }
    file.extend(grid);
    file.extend(topology);
    file.extend(payload);
    Ok(file)
}

/// Append a signed little-endian XBF integer field.
fn integer(out: &mut Vec<u8>, value: i32) {
    out.extend(value.to_le_bytes());
}
/// Append one little-endian XBF geometric real value.
fn real(out: &mut Vec<u8>, value: f64) {
    out.extend(value.to_le_bytes());
}
/// Append a sequence of little-endian XBF geometric real values.
fn reals(out: &mut Vec<u8>, values: &[f64]) {
    for &v in values {
        real(out, v);
    }
}
/// Append a length-prefixed authored ASCII string to an XBF record.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "All authored XBF strings are short fixed names that fit a signed 32-bit byte count."
)]
fn string(out: &mut Vec<u8>, value: &str) {
    integer(out, value.len() as i32);
    out.extend(value.as_bytes());
}
/// Append an inline uniform-scale and translation XBF location definition.
fn xbf_transform(out: &mut Vec<u8>, scale: f64, offset: Vec3) {
    out.push(6);
    reals(
        out,
        &[
            scale, 0., 0., offset.x, 0., scale, 0., offset.y, 0., 0., scale, offset.z,
        ],
    );
}
/// Append a backward XBF reference to an earlier record in the same small buffer.
#[allow(
    clippy::cast_possible_truncation,
    reason = "All referenced records belong to the small authored face buffer, whose backward offsets fit u32."
)]
fn reference(out: &mut Vec<u8>, offset: usize) {
    let delta = out.len() - offset;
    out.push(3);
    out.extend((delta as u32).to_le_bytes());
}
/// Append an oriented XBF vertex with the authored geometric tolerance.
fn xbf_vertex(out: &mut Vec<u8>, point: Vec3, orientation: u8) {
    out.extend([228 + orientation, 7]);
    reals(out, &[1e-7, point.x, point.y, point.z]);
    out.extend([0, 8, 199]);
}
/// Serialize an XBF attribute and an optional length-prefixed direct shape payload.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "Authored attributes contain one small face or fixed metadata, fitting their signed 32-bit data lengths."
)]
fn xbf_attribute(kind: i32, id: i32, data: Vec<u8>, direct: Option<Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::new();
    integer(&mut out, kind);
    integer(&mut out, id);
    integer(&mut out, data.len() as i32);
    out.extend(data);
    if let Some(data) = direct {
        out.extend(((data.len() + 8) as u64).to_le_bytes());
        out.extend(data);
    }
    out
}
/// Serialize an XBF color-tree link using the native tree-node identifier.
fn xbf_tree(id: i32, father: i32, first: i32) -> Vec<u8> {
    let mut data = Vec::new();
    for value in [father, -1, -1, first] {
        integer(&mut data, value);
    }
    data.extend(0xefd2_12e4_u32.to_le_bytes());
    data.extend([
        0xfd, 0x6d, 0xd4, 0x11, 0xb9, 0xc8, 0, 0x60, 0xb0, 0xee, 0x28, 0x1b,
    ]);
    xbf_attribute(3, id, data, None)
}
/// Serialize one sized XBF label with attributes, tagged children, and sentinels.
fn xbf_label(attributes: Vec<Vec<u8>>, children: Vec<(i32, Vec<u8>)>) -> Vec<u8> {
    let mut out = vec![0; 8];
    for attr in attributes {
        out.extend(attr);
    }
    integer(&mut out, -1);
    for (tag, child) in children {
        integer(&mut out, tag);
        out.extend(child);
    }
    integer(&mut out, -2);
    let size = out.len() as u64;
    out[..8].copy_from_slice(&size.to_le_bytes());
    out
}
/// Encode a BINFILE/`BinXCAF` v12 analytic face with carrier placement and color.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "The fixed seven-name table and single-face document fit signed 32-bit count and header-offset fields."
)]
#[allow(
    clippy::too_many_lines,
    reason = "Keep the native inline face, label hierarchy, and binary header assembly together so their offset relationships remain reviewable."
)]
fn xbf() -> Vec<u8> {
    let carrier = Vec3::new(1., -2., 0.5);
    let root = Vec3::new(4., 1., 3.);
    let mut geometry = vec![200];
    xbf_transform(&mut geometry, 1.5, root);
    geometry.push(8);
    geometry.extend([216, 7, 0]);
    real(&mut geometry, 1e-7);
    let surface = geometry.len();
    geometry.extend([13, 1]);
    reals(
        &mut geometry,
        &[0., 0., 0., 0., 0., 1., 1., 0., 0., 0., 1., 0.],
    );
    let placement = geometry.len();
    xbf_transform(&mut geometry, 1., carrier);
    geometry.extend([0, 8, 220, 7, 8]);
    let points = [
        Vec3::new(0., 0., 0.),
        Vec3::new(2., 0., 0.),
        Vec3::new(0., 1., 0.),
    ];
    for edge in 0..3 {
        let start = points[edge];
        let end = points[(edge + 1) % 3];
        let delta = end - start;
        let length = delta.length();
        let direction = delta / length;
        geometry.extend([224, 7]);
        real(&mut geometry, 1e-7);
        geometry.extend([3, 1, 9, 1]);
        reals(
            &mut geometry,
            &[start.x, start.y, 0., direction.x, direction.y, 0.],
        );
        reference(&mut geometry, placement);
        reals(&mut geometry, &[0., length]);
        geometry.extend([2, 11, 1]);
        reals(&mut geometry, &[start.x, start.y, direction.x, direction.y]);
        reference(&mut geometry, surface);
        reference(&mut geometry, placement);
        reals(&mut geometry, &[0., length]);
        geometry.extend([0, 8]);
        xbf_vertex(&mut geometry, start + carrier, 0);
        xbf_vertex(&mut geometry, end + carrier, 1);
        geometry.push(199);
    }
    geometry.extend([199, 199, 199]);
    let mut named = Vec::new();
    integer(&mut named, 1);
    integer(&mut named, 1);
    named.push(b'P');
    let shape = xbf_label(
        vec![
            xbf_attribute(1, -2, named, Some(geometry)),
            xbf_tree(3, 4, -1),
        ],
        vec![],
    );
    let mut color = Vec::new();
    reals(&mut color, &[0.25, 0.5, 1.]);
    let color = xbf_label(
        vec![xbf_tree(4, -1, 3), xbf_attribute(4, 5, color, None)],
        vec![],
    );
    let body = xbf_label(
        vec![],
        vec![(
            1,
            xbf_label(
                vec![],
                vec![
                    (
                        1,
                        xbf_label(vec![xbf_attribute(2, 1, vec![], None)], vec![(1, shape)]),
                    ),
                    (2, xbf_label(vec![], vec![(1, color)])),
                ],
            ),
        )],
    );
    let mut out = b"BINFILE".to_vec();
    out.resize(59, 0);
    integer(&mut out, 1);
    string(&mut out, "12");
    for _ in 0..3 {
        string(&mut out, "");
    }
    integer(&mut out, 0);
    string(&mut out, "");
    integer(&mut out, 0);
    let info = [
        "FILE_FORMAT: BinXCAF",
        "START_TYPES",
        "TNaming_NamedShape",
        "XCAFDoc_ShapeTool",
        "TDataStd_TreeNode",
        "XCAFDoc_Color",
        "END_TYPES",
    ];
    integer(&mut out, info.len() as i32);
    for item in info {
        string(&mut out, item);
    }
    let end_info = out.len() as i32;
    integer(&mut out, 0);
    let start_doc = out.len() as i32;
    let header = [
        0x0403_0201,
        7,
        end_info,
        end_info,
        start_doc,
        -1,
        -1,
        -1,
        -1,
        -1,
        -1,
        start_doc,
        start_doc,
    ];
    for (index, value) in header.iter().enumerate() {
        out[7 + index * 4..11 + index * 4].copy_from_slice(&value.to_le_bytes());
    }
    integer(&mut out, 4);
    out.extend(b":\0\0\0");
    out.extend([0; 24]);
    integer(&mut out, 0);
    out.extend(body);
    out
}
