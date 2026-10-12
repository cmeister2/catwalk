//! Retained Maya-exported NURBS and authored rational-curve/container controls.

#[path = "../../tests/support/digest_hex.rs"]
/// Lowercase formatting shared with fixture-integrity tests.
mod digest_hex;

use catwalk3d::Result;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Write},
    path::Path,
};

/// Corpus directory containing NURBS centerline inputs and their notices.
const DIR: &str = "usd_nurbs";
/// Authorship and licensing for the reproducible curve controls.
const AUTHORED: &str = "Author-created deterministic Rust fixture; MIT, matching the project license. Curve widths are omitted from the centerline preview.";
/// Pinned exporter source, exact checksum and separate licensing for the original layer.
const ORIGINAL: &str = "Unmodified Autodesk Maya USD test/testSamples/basisCurves/nurbsCircleExport.usda at commit 48eae123cabec8e671f0779b03c22fa30631a781; SHA-256 17b9eab67f027d314e4de32cd01d1c383068768d8b9a49c7a72ea5abc0b6057b. Apache-2.0; original license and source attribution retained in usd_nurbs/MAYA-USD-LICENSE.md and usd_nurbs/MAYA-USD-NOTICE.txt. Two exported periodic cubic NURBS centerlines, including the second curve's authored placement; width geometry is not rendered. https://github.com/Autodesk/maya-usd/blob/48eae123cabec8e671f0779b03c22fa30631a781/test/testSamples/basisCurves/nurbsCircleExport.usda";
/// Stage metadata fixes the authored controls' units and up axis.
const HEADER: &str = "#usda 1.0\n(upAxis = \"Z\"; metersPerUnit = 1)\n";

/// Positive, zero and signed weights distinguish genuine rational interpolation.
const RATIONAL: &str = r#"
def NurbsCurves "PositiveQuarter" {
    int[] curveVertexCounts = [3]
    int[] order = [3]
    double[] knots = [0,0,0,1,1,1]
    double2[] ranges = [(0,1)]
    point3f[] points = [(2,0,0),(2,2,0),(0,2,0)]
    double[] pointWeights = [1,0.7071067811865476,1]
    color3f[] primvars:displayColor = [(0.95,0.25,0.12)]
}
def NurbsCurves "ZeroMiddle" {
    int[] curveVertexCounts = [3]
    int[] order = [3]
    double[] knots = [0,0,0,1,1,1]
    double2[] ranges = [(0,1)]
    point3f[] points = [(5,0,0),(5,2,0),(3,2,0)]
    double[] pointWeights = [1,0,1]
    color3f[] primvars:displayColor = [(0.2,0.8,0.3)]
}
def NurbsCurves "SignedMiddle" {
    int[] curveVertexCounts = [3]
    int[] order = [3]
    double[] knots = [0,0,0,1,1,1]
    double2[] ranges = [(0,1)]
    point3f[] points = [(8,0,0),(8,2,0),(6,2,0)]
    double[] pointWeights = [1,-0.25,1]
    color3f[] primvars:displayColor = [(0.18,0.4,0.95)]
}
"#;

/// Batched mixed orders and trimmed domains accompany two disconnected quadratic pieces.
const TOPOLOGY: &str = r#"
def NurbsCurves "VariableOrders" {
    int[] curveVertexCounts = [4,3,4]
    int[] order = [2,3,4]
    double[] knots = [0,0,1,3,4,4, 0,0,0,2,2,2, 0,0,0,0,1,1,1,1]
    double2[] ranges = [(0.5,3.5),(0.5,1.5),(0.25,0.75)]
    point3f[] points = [(0,0,0),(1,2,0),(3,1,0),(4,3,0),
                        (5,0,0),(7,4,1),(9,0,0),
                        (10,0,0),(12,4,2),(14,-2,1),(16,1,0)]
    color3f[] primvars:displayColor = [(0.95,0.35,0.12),(0.2,0.8,0.3),(0.18,0.4,0.95)] (
        interpolation = "uniform"
    )
}
def NurbsCurves "FullMultiplicityBreak" {
    int[] curveVertexCounts = [6]
    int[] order = [3]
    double[] knots = [0,0,0,1,1,1,2,2,2]
    double2[] ranges = [(0,2)]
    point3f[] points = [(0,0,2),(1,3,2),(2,0,2),(5,0,2),(6,-2,3),(7,1,2)]
    color3f[] primvars:displayColor = [(0.75,0.3,0.85)]
}
"#;

/// One placed curve and two point instances exercise prototype-local and parent frames.
const INSTANCES: &str = r#"
def NurbsCurves "DirectPlaced" {
    int[] curveVertexCounts = [3]
    int[] order = [3]
    double[] knots = [0,0,0,1,1,1]
    double2[] ranges = [(0,1)]
    point3f[] points = [(2,0,0),(2,2,0),(0,2,0)]
    double[] pointWeights = [1,0.7071067811865476,1]
    color3f[] primvars:displayColor = [(0.2,0.4,0.95)]
    double3 xformOp:translate = (-5,3,0)
    double xformOp:rotateX = 90
    double3 xformOp:scale = (1,0.75,2)
    uniform token[] xformOpOrder = ["xformOp:translate","xformOp:rotateX","xformOp:scale"]
}
def PointInstancer "Copies" {
    rel prototypes = [<__PATH__/Copies/Curve>]
    int[] protoIndices = [0,0]
    point3f[] positions = [(-3,0,0),(3,0,2)]
    float3[] scales = [(1,1,1),(1.5,0.5,2)]
    color3f[] primvars:displayColor = [(0.95,0.25,0.12),(0.2,0.8,0.3)] (
        interpolation = "uniform"
    )
    double3 xformOp:translate = (1,-2,0)
    double xformOp:rotateZ = 90
    double3 xformOp:scale = (2,1,1)
    uniform token[] xformOpOrder = ["xformOp:translate","xformOp:rotateZ","xformOp:scale"]
    def NurbsCurves "Curve" {
        int[] curveVertexCounts = [3]
        int[] order = [3]
        double[] knots = [0,0,0,1,1,1]
        double2[] ranges = [(0,1)]
        point3f[] points = [(2,0,0),(2,2,0),(0,2,0)]
        double[] pointWeights = [1,0.7071067811865476,1]
        double3 xformOp:translate = (0,0,1)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }
}
"#;

/// Write one authored layer or package and describe its deterministic source.
fn add(
    root: &Path,
    id: &str,
    extension: &str,
    title: &str,
    bytes: &[u8],
) -> Result<crate::Fixture> {
    let input = format!("{DIR}/{id}.{extension}");
    fs::write(root.join(&input), bytes)?;
    Ok(crate::fixture(
        id, extension, title, &input, "model", AUTHORED,
    ))
}

/// Validate retained original bytes before registering their gallery case.
fn original(root: &Path) -> Result<crate::Fixture> {
    let input = format!("{DIR}/usda-maya-nurbs-circles.usda");
    for (name, length, hash) in [
        (
            "usda-maya-nurbs-circles.usda",
            1638,
            "17b9eab67f027d314e4de32cd01d1c383068768d8b9a49c7a72ea5abc0b6057b",
        ),
        (
            "MAYA-USD-LICENSE.md",
            10_850,
            "8173d5c29b4f956d532781d2b86e4e30f83e6b7878dce18c919451d6ba707c90",
        ),
    ] {
        let bytes = fs::read(root.join(DIR).join(name))?;
        if bytes.len() != length || format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))) != hash
        {
            return Err(format!(
                "retained Maya USD resource differs from its pinned source: {name}"
            )
            .into());
        }
    }
    Ok(crate::fixture(
        "usda-maya-nurbs-circles",
        "usda",
        "Maya-exported periodic NURBS circle pair",
        &input,
        "model",
        ORIGINAL,
    ))
}

/// Combine the diagnostic groups into one stage with spatially separated controls.
fn family() -> String {
    format!(
        "#usda 1.0\n(upAxis = \"Z\"; metersPerUnit = 1; defaultPrim = \"Family\")\n\
         def Xform \"Family\" {{\n\
         def Xform \"Rational\" {{\n{RATIONAL}\n}}\n\
         def Xform \"Topology\" {{\n\
         double3 xformOp:translate = (0,7,0)\n\
         uniform token[] xformOpOrder = [\"xformOp:translate\"]\n{TOPOLOGY}\n}}\n\
         def Xform \"Placed\" {{\n\
         double3 xformOp:translate = (0,17,0)\n\
         uniform token[] xformOpOrder = [\"xformOp:translate\"]\n{}\n}}\n}}\n",
        INSTANCES.replace("__PATH__", "/Family/Placed"),
    )
}

/// Store a genuine binary root layer with USDZ's required 64-byte payload alignment.
fn package(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .with_alignment(64);
    archive.start_file("root.usdc", options)?;
    archive.write_all(bytes)?;
    Ok(archive.finish()?.into_inner())
}

/// Register equivalent ASCII, binary, generic-extension and aligned-package stages.
fn binary_witnesses(root: &Path) -> Result<Vec<crate::Fixture>> {
    let ascii = family();
    let mut cases = vec![add(
        root,
        "usda-nurbs-family",
        "usda",
        "USD ASCII rational, trimmed, disconnected and instanced NURBS",
        ascii.as_bytes(),
    )?];
    let source = root.join(DIR).join("usda-nurbs-family.usda");
    let binary = root.join(DIR).join("usdc-nurbs-family.usdc");
    let stage = openusd::usd::Stage::open(source.to_str().ok_or("USD fixture path is not UTF-8")?)?;
    stage
        .root_layer()
        .export(binary.to_str().ok_or("USD fixture path is not UTF-8")?)?;
    let bytes = fs::read(binary)?;
    if !bytes.starts_with(b"PXR-USDC") {
        return Err("USD fixture writer did not emit a genuine binary crate".into());
    }
    cases.push(crate::fixture(
        "usdc-nurbs-family",
        "usdc",
        "USD binary rational, trimmed, disconnected and instanced NURBS",
        &format!("{DIR}/usdc-nurbs-family.usdc"),
        "model",
        AUTHORED,
    ));
    cases.push(add(
        root,
        "usd-nurbs-family",
        "usd",
        "USD generic binary NURBS family",
        &bytes,
    )?);
    cases.push(add(
        root,
        "usdz-nurbs-family",
        "usdz",
        "USDZ aligned binary NURBS family",
        &package(&bytes)?,
    )?);
    Ok(cases)
}

/// Generate authored NURBS controls and register the unchanged original Maya layer.
///
/// # Errors
///
/// Returns an error when retained resources differ from their pinned checksums,
/// filesystem operations fail, or the Rust USD/ZIP writer cannot encode the stages.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    fs::create_dir_all(root.join(DIR))?;
    let mut cases = vec![original(root)?];
    for (id, title, body) in [
        (
            "usda-nurbs-rational-weights",
            "Rational NURBS with positive, zero and signed weights",
            RATIONAL.to_owned(),
        ),
        (
            "usda-nurbs-trimmed-discontinuous",
            "Variable-order trimmed NURBS and a disconnected knot boundary",
            TOPOLOGY.to_owned(),
        ),
        (
            "usda-nurbs-placed-instances",
            "Placed rational NURBS and transformed point instances",
            INSTANCES.replace("__PATH__", ""),
        ),
    ] {
        cases.push(add(
            root,
            id,
            "usda",
            title,
            format!("{HEADER}{body}").as_bytes(),
        )?);
    }
    cases.extend(binary_witnesses(root)?);
    Ok(cases)
}
