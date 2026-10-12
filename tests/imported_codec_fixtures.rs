//! Mandatory upstream Draco and Esri LEPCC/I3S qualification through the public loaders.

pub use catwalk3d::{Error, Result, scene};

#[path = "../src/formats/lepcc.rs"]
/// Exercise the exact internal LEPCC decoder without changing its public visibility.
mod lepcc;

use base64::Engine;
use catwalk3d::{
    formats,
    render::{RenderOptions, render},
    scene::{Scene, Vec3},
};
use draco_core::{DecoderBuffer, GeometryAttributeType, Mesh, MeshDecoder};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Distinguish temporary inputs created by concurrently running fixture tests.
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
/// Original Esri compressed coordinate stream used by direct and package tests.
const XYZ: &[u8] = include_bytes!("fixtures/slpk/autzen.xyz");
/// Esri raw RGB stream containing 17 separately specified colors.
const RAW: &[u8] = include_bytes!("fixtures/slpk/rgb-raw.bin");
/// Esri constant RGB stream containing 1024 equal colors.
const CONSTANT: &[u8] = include_bytes!("fixtures/slpk/rgb-constant.bin");
/// Esri Huffman palette stream containing three independently specified colors.
const HUFFMAN: &[u8] = include_bytes!("fixtures/slpk/rgb-huffman.bin");

/// Temporary on-disk inputs allow malformed bytes to reach the real format dispatcher.
struct Inputs {
    /// Unique directory removed when this test's input owner is dropped.
    root: PathBuf,
}

impl Inputs {
    /// Create an empty per-test directory without changing retained upstream bytes.
    fn new(label: &str) -> Result<Self> {
        let serial = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "catwalk-codec-fixtures-{label}-{}-{serial}",
            std::process::id(),
        ));
        fs::create_dir(&root)?;
        Ok(Self { root })
    }

    /// Save bytes under their format extension and invoke the public loader.
    fn load(&self, name: &str, bytes: &[u8]) -> Result<Scene> {
        let path = self.root.join(name);
        fs::write(&path, bytes)?;
        formats::load(&path)
    }
}

impl Drop for Inputs {
    /// Remove temporary mutations while leaving repository fixtures untouched.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Resolve one retained upstream fixture; missing inputs remain test failures.
fn fixture(family: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(family)
        .join(name)
}

/// Preserve the raw upstream face-count check separately from scene conversion.
fn raw_face_count(bytes: &[u8]) -> Result<usize> {
    let mut buffer = DecoderBuffer::new(bytes);
    let mut mesh = Mesh::new();
    MeshDecoder::new().decode(&mut buffer, &mut mesh)?;
    Ok(mesh.num_faces())
}

#[test]
fn independent_google_mesh_encodings() -> Result<()> {
    let inputs = Inputs::new("draco-mesh")?;
    for name in ["cube.drc", "cube-valence.drc", "cube-sequential.drc"] {
        let path = fixture("draco", name);
        let bytes = fs::read(&path)?;
        assert_eq!(raw_face_count(&bytes)?, 12);
        let scene = formats::load(&path)?;
        assert_eq!(scene.triangles.len(), 12);
        assert!(scene.positions().all(|point| {
            point.x.abs() <= 1.001 && point.y.abs() <= 1.001 && point.z.abs() <= 1.001
        }));
        assert!(
            inputs
                .load("truncated.drc", &bytes[..bytes.len() - 1])
                .is_err()
        );
        let mut trailing = bytes;
        trailing.push(42);
        assert!(inputs.load("trailing.drc", &trailing).is_err());
    }
    Ok(())
}

#[test]
fn independent_google_point_clouds_and_color() -> Result<()> {
    let scene = formats::load(&fixture("draco", "cube-points.drc"))?;
    assert!(!scene.points.is_empty());
    assert!(scene.triangles.is_empty());
    let scene = formats::load(&fixture("draco", "color-points.drc"))?;
    assert!(scene.points.len() > 100);
    assert!(scene.points.iter().all(|point| point.color.is_some()));
    assert!(
        scene
            .points
            .iter()
            .any(|point| point.color != scene.points[0].color)
    );
    Ok(())
}

/// Encode the authored compressed document as a properly padded GLB container.
fn compressed_glb(document: &Value, bytes: &[u8]) -> Result<Vec<u8>> {
    let mut document = document.clone();
    document["buffers"][0]
        .as_object_mut()
        .ok_or("missing buffer")?
        .remove("uri");
    let mut json = serde_json::to_vec(&document)?;
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut binary = bytes.to_vec();
    while !binary.len().is_multiple_of(4) {
        binary.push(0);
    }
    let mut glb = Vec::new();
    for word in [
        0x4654_6c67_u32,
        2,
        u32::try_from(28 + json.len() + binary.len())?,
        u32::try_from(json.len())?,
        0x4e4f_534a,
    ] {
        glb.extend(word.to_le_bytes());
    }
    glb.extend(json);
    glb.extend(u32::try_from(binary.len())?.to_le_bytes());
    glb.extend(0x004e_4942_u32.to_le_bytes());
    glb.extend(binary);
    Ok(glb)
}

#[test]
fn compressed_gltf_and_glb_keep_transforms_and_core_validation() -> Result<()> {
    let bytes = fs::read(fixture("draco", "cube-valence.drc"))?;
    let mut buffer = DecoderBuffer::new(&bytes);
    let mut mesh = Mesh::new();
    MeshDecoder::new().decode(&mut buffer, &mut mesh)?;
    let point_count = mesh.num_points();
    let face_count = mesh.num_faces();
    let attributes = mesh.into_point_cloud();
    let position_id = attributes
        .named_attribute(GeometryAttributeType::Position)
        .ok_or("fixture has no position attribute")?
        .unique_id();
    // The producer has eight distinct geometric corners and twelve faces.
    // Its encoded points may split those corners for normals/UVs, so accessor
    // metadata uses the public upstream decoder's encoded point count.
    // Expected placed positions below remain independent literal cube corners.
    let mut document = json!({
        "asset":{"version":"2.0"},"extensionsRequired":["KHR_draco_mesh_compression"],
        "buffers":[{"byteLength":bytes.len(),"uri":format!("data:application/octet-stream;base64,{}",base64::engine::general_purpose::STANDARD.encode(&bytes))}],
        "bufferViews":[{"buffer":0,"byteLength":bytes.len()}],
        "accessors":[{"componentType":5126,"count":point_count,"type":"VEC3","min":[-1,-1,-1],"max":[1,1,1]},
            {"componentType":5125,"count":face_count * 3,"type":"SCALAR"}],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1,"extensions":{"KHR_draco_mesh_compression":{"bufferView":0,"attributes":{"POSITION":position_id}}}}]}],
        "nodes":[{"mesh":0,"translation":[10,20,30]}],"scenes":[{"nodes":[0]}],"scene":0
    });
    let inputs = Inputs::new("gltf-draco")?;
    let scene = inputs.load("cube.gltf", &serde_json::to_vec(&document)?)?;
    assert_eq!(scene.triangles.len(), raw_face_count(&bytes)?);
    assert_eq!(scene.triangles.len(), 12);
    let mut expected = Vec::new();
    for x in [0., 1.] {
        for y in [0., 1.] {
            for z in [0., 1.] {
                expected.push(Vec3::new(x + 10., -z - 30., y + 20.));
            }
        }
    }
    assert!(
        scene
            .triangles
            .iter()
            .flat_map(|triangle| triangle.vertices)
            .all(|vertex| expected.contains(&vertex))
    );
    let other = inputs.load("cube.glb", &compressed_glb(&document, &bytes)?)?;
    assert_eq!(
        render(&scene, &RenderOptions::default())?,
        render(&other, &RenderOptions::default())?
    );
    document["meshes"][0]["primitives"][0]["extensions"]["KHR_draco_mesh_compression"]["attributes"]
        ["POSITION"] = json!(u32::MAX);
    assert!(
        inputs
            .load("invalid-id.gltf", &serde_json::to_vec(&document)?)
            .is_err()
    );
    document["meshes"][0]["primitives"][0]
        .as_object_mut()
        .unwrap()
        .remove("extensions");
    document
        .as_object_mut()
        .unwrap()
        .remove("extensionsRequired");
    // Ordinary accessors still require their buffer views after compression is removed.
    assert!(
        inputs
            .load(
                "missing-accessor-view.gltf",
                &serde_json::to_vec(&document)?
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn esri_xyz_reference_matches_error_bounds() {
    let points = lepcc::xyz(XYZ).unwrap();
    let reference = include_bytes!("fixtures/slpk/autzen-reference.bin");
    let count = usize::try_from(u32::from_le_bytes(reference[..4].try_into().unwrap())).unwrap();
    assert_eq!(points.len(), count);
    // The reference encoder sorts by row then X; compare to original unsorted
    // double-precision points with the published quantization error bounds.
    let expected: Vec<Vec3> = reference[4..4 + count * 24]
        .as_chunks::<24>()
        .0
        .iter()
        .map(|point| {
            Vec3::new(
                f64::from_le_bytes(point[..8].try_into().unwrap()),
                f64::from_le_bytes(point[8..16].try_into().unwrap()),
                f64::from_le_bytes(point[16..24].try_into().unwrap()),
            )
        })
        .collect();
    let bounds = Vec3::new(
        f64::from_le_bytes(XYZ[72..80].try_into().unwrap()),
        f64::from_le_bytes(XYZ[80..88].try_into().unwrap()),
        f64::from_le_bytes(XYZ[88..96].try_into().unwrap()),
    );
    for point in points {
        assert!(
            expected.iter().any(|reference| {
                (point.x - reference.x).abs() <= bounds.x * 1.000_01
                    && (point.y - reference.y).abs() <= bounds.y * 1.000_01
                    && (point.z - reference.z).abs() <= bounds.z * 1.000_01
            }),
            "no reference coordinate matches {point:?}"
        );
    }
}

#[test]
fn esri_raw_constant_and_huffman_color_encodings() {
    let expected: Vec<_> = (0..17_u8)
        .map(|index| [index * 13, index * 7, index * 9])
        .collect();
    assert_eq!(lepcc::rgb(RAW, 17).unwrap(), expected);
    assert_eq!(
        lepcc::rgb(CONSTANT, 1024).unwrap(),
        vec![[30, 100, 200]; 1024]
    );
    let expected: Vec<_> = (0..1024)
        .map(|index| {
            if index % 17 == 0 {
                [255, 0, 0]
            } else if index % 29 == 0 {
                [0, 255, 0]
            } else {
                [0, 0, 255]
            }
        })
        .collect();
    assert_eq!(lepcc::rgb(HUFFMAN, 1024).unwrap(), expected);
}

#[test]
fn corrupt_and_truncated_streams_fail() {
    for stream in [XYZ, RAW, CONSTANT, HUFFMAN] {
        for length in [0, 10, 20, stream.len() - 1] {
            assert!(lepcc::xyz(&stream[..length]).is_err());
            assert!(lepcc::rgb(&stream[..length], 17).is_err());
        }
        let mut bad = stream.to_vec();
        *bad.last_mut().unwrap() ^= 1;
        assert!(lepcc::xyz(&bad).is_err());
        assert!(lepcc::rgb(&bad, 17).is_err());
    }
    assert!(lepcc::rgb(RAW, 16).is_err());
}

/// Store authored I3S metadata and an unchanged upstream geometry member in a ZIP.
fn zip(files: &[(&str, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in files {
        writer.start_file(*name, zip::write::SimpleFileOptions::default())?;
        writer.write_all(bytes)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// Build the unchanged test metadata for both supported page-name conventions.
fn layer(version: &str) -> Value {
    json!({"layerType":"PointCloud", "spatialReference":{"wkid":4326},
        "store":{"version":version,"index":{"nodesPerPage":2},
        "defaultGeometrySchema":{"encoding":"lepcc-xyz"}}})
}

#[test]
fn esri_original_package_renders_geographic_points_in_meters() -> Result<()> {
    let scene = formats::load(&fixture("slpk", "autzen.slpk"))?;
    assert_eq!(scene.points.len(), 106);
    let (min, max) = scene.points.iter().fold(
        (
            Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
        ),
        |(low, high), point| {
            (
                Vec3::new(
                    low.x.min(point.position.x),
                    low.y.min(point.position.y),
                    low.z.min(point.position.z),
                ),
                Vec3::new(
                    high.x.max(point.position.x),
                    high.y.max(point.position.y),
                    high.z.max(point.position.z),
                ),
            )
        },
    );
    assert!(max.x - min.x > 800. && max.y - min.y > 1200.);
    assert!(max.z - min.z > 30. && max.z - min.z < 50.);
    assert!(render(&scene, &RenderOptions::default()).is_ok());
    Ok(())
}

#[test]
fn both_page_naming_schemes_select_only_leaves_and_resource_ids() -> Result<()> {
    let inputs = Inputs::new("slpk-nodepages")?;
    for version in ["1.6", "2.0"] {
        let count_key = if version == "2.0" {
            "vertexCount"
        } else {
            "pointCount"
        };
        let mut first = json!({"nodes":[
            {"resourceId":99,"firstChild":2,"childCount":1},
            {"resourceId":999,"firstChild":0,"childCount":0}]});
        first["nodes"][0][count_key] = 1.into();
        let mut leaf = json!({"nodes":[{"resourceId":"17","childCount":0}]});
        leaf["nodes"][0][count_key] = 106.into();
        let filename = if version == "2.0" {
            "nodepages/1.json"
        } else {
            "nodepages/2.json"
        };
        let package = zip(&[
            ("3dSceneLayer.json", serde_json::to_vec(&layer(version))?),
            ("nodepages/0.json", serde_json::to_vec(&first)?),
            (filename, serde_json::to_vec(&leaf)?),
            ("nodes/17/geometries/0.bin.pccxyz", XYZ.to_vec()),
        ])?;
        assert_eq!(inputs.load("pages.slpk", &package)?.points.len(), 106);
    }
    Ok(())
}
