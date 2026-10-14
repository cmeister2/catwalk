//! Native MAX geometry qualified against original, independently exported producer meshes.

#[path = "support/digest_hex.rs"]
/// Existing corpus digest formatting for retained original source bytes.
mod digest_hex;

use catwalk3d::{
    formats, geometry_thumbnail,
    render::RenderOptions,
    scene::{Scene, Vec3},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

/// Unique malformed producer-copy identifiers under parallel test execution.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Resolve a licensed original scene or its independently exported producer oracle.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/native_geometry/max")
        .join(name)
}

/// Temporary producer copy removed even if an assertion unwinds.
struct Copy(PathBuf);

impl Copy {
    /// Retain the entire genuine compound document while changing addressed streams only.
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "catwalk-max-{}-{}.max",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::copy(fixture(name), &path).unwrap();
        Self(path)
    }

    /// Change the original addressed native Scene stream through the independent CFB API.
    fn scene(&self, edit: impl FnOnce(&mut Vec<u8>)) {
        let mut compound = cfb::open_rw(&self.0).unwrap();
        let mut scene = Vec::new();
        compound
            .open_stream("/Scene")
            .unwrap()
            .read_to_end(&mut scene)
            .unwrap();
        edit(&mut scene);
        compound
            .create_stream("/Scene")
            .unwrap()
            .write_all(&scene)
            .unwrap();
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        fs::remove_file(&self.0).unwrap();
    }
}

/// Independently compute geometric axis bounds for native and exported positions.
fn bounds(points: impl IntoIterator<Item = Vec3>) -> [[f64; 3]; 2] {
    let mut result = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
    for p in points {
        for (axis, v) in [p.x, p.y, p.z].into_iter().enumerate() {
            result[0][axis] = result[0][axis].min(v);
            result[1][axis] = result[1][axis].max(v);
        }
    }
    result
}

/// Tolerance covers the producer exporter's four-decimal formatting.
fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 0.015, "{actual} != {expected}");
}

/// Native and independent producer exports must have the same placed bounds.
fn compare_bounds(actual: &Scene, expected: Vec<Vec3>) {
    for (a, b) in bounds(actual.positions())
        .into_iter()
        .flatten()
        .zip(bounds(expected).into_iter().flatten())
    {
        near(a, b);
    }
}

/// Decode an independently saved Babylon mesh's placement, then convert its Y-up axes.
fn exported_points(name: &str) -> Vec<Vec3> {
    let export: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture(name)).unwrap()).unwrap();
    let mut points = Vec::new();
    for mesh in export["meshes"].as_array().unwrap() {
        let mut occurrences = vec![mesh];
        if let Some(instances) = mesh["instances"].as_array() {
            occurrences.extend(instances);
        }
        for occurrence in occurrences {
            let values = |field: &str| {
                occurrence[field]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap())
                    .collect::<Vec<_>>()
            };
            let position = values("position");
            let scale = values("scaling");
            let q = values("rotationQuaternion");
            let axis = Vec3::new(q[0], q[1], q[2]);
            for vertex in mesh["positions"].as_array().unwrap().as_chunks::<3>().0 {
                let p = Vec3::new(
                    vertex[0].as_f64().unwrap() * scale[0],
                    vertex[1].as_f64().unwrap() * scale[1],
                    vertex[2].as_f64().unwrap() * scale[2],
                );
                let rotated = p + axis.cross(p) * (2. * q[3]) + axis.cross(axis.cross(p)) * 2.;
                let p = rotated + Vec3::new(position[0], position[1], position[2]);
                points.push(Vec3::new(p.x, p.z, p.y));
            }
        }
    }
    points
}

#[test]
fn unchanged_native_sources_have_independent_producer_geometry() {
    let provenance: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture("provenance.json")).unwrap()).unwrap();
    for record in provenance["files"].as_array().unwrap() {
        let bytes = fs::read(fixture(record["file"].as_str().unwrap())).unwrap();
        assert_eq!(
            format!("{:x}", digest_hex::Hex(Sha256::digest(bytes))),
            record["sha256"].as_str().unwrap()
        );
    }
    for (name, count) in [
        ("SimpleTriangle", 1),
        ("Instances", 36),
        ("DoubleSidedMaterial", 2),
    ] {
        let scene = formats::load(&fixture(&format!("{name}.max"))).unwrap();
        assert_eq!(scene.triangles.len(), count, "{name}");
        compare_bounds(&scene, exported_points(&format!("{name}.babylon")));
    }
    let instances = formats::load(&fixture("Instances.max")).unwrap();
    for (occurrence, triangles) in instances.triangles.as_chunks::<12>().0.iter().enumerate() {
        let b = bounds(triangles.iter().flat_map(|t| t.vertices));
        near(
            b[0][0],
            f64::from(u32::try_from(occurrence).unwrap()) * 2. - 0.5,
        );
        near(
            b[1][0],
            f64::from(u32::try_from(occurrence).unwrap()) * 2. + 0.5,
        );
        near(b[0][2], 0.);
        near(b[1][2], 1.);
    }
}

#[test]
fn native_triangles_and_instances_have_independent_surface_area() {
    for (name, expected) in [("SimpleTriangle.max", 5000.), ("Instances.max", 18.)] {
        let scene = formats::load(&fixture(name)).unwrap();
        let area: f64 = scene
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    / 2.
            })
            .sum();
        near(area, expected);
    }
}

#[test]
fn geometry_survives_thumbnail_removal_and_cli_renders_it() {
    let input = Copy::new("SimpleTriangle.max");
    let mut compound = cfb::open_rw(&input.0).unwrap();
    compound.remove_stream("/\u{5}SummaryInformation").unwrap();
    drop(compound);
    let options = RenderOptions {
        width: 96,
        height: 64,
        transparent: true,
        ..RenderOptions::default()
    };
    let image = geometry_thumbnail(&input.0, &options).unwrap();
    assert_eq!(image.dimensions(), (96, 64));
    assert!(image.pixels().any(|p| p[3] != 0));
    let output = input.0.with_extension("png");
    let command = Command::new(env!("CARGO_BIN_EXE_catwalk"))
        .arg(&input.0)
        .arg("--render-geometry")
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        command.status.success(),
        "{}",
        String::from_utf8_lossy(&command.stderr)
    );
    assert!(
        image::open(&output)
            .unwrap()
            .to_rgba8()
            .pixels()
            .any(|p| p[3] != 0)
    );
    fs::remove_file(output).unwrap();
}

/// Locate an exact retained producer field rather than scanning arbitrary float patterns.
fn field(data: &[u8], marker: &[u8]) -> usize {
    let matches: Vec<_> = data
        .windows(marker.len())
        .enumerate()
        .filter(|(_, v)| *v == marker)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(matches.len(), 1, "native fixture field must be unique");
    matches[0]
}

#[test]
fn addressed_native_buffers_reject_bad_indices_counts_and_nonfinite_positions() {
    for kind in 0..3 {
        let input = Copy::new("SimpleTriangle.max");
        input.scene(|scene| {
            let vertices = field(scene, &[0x14, 0x09, 46, 0, 0, 0, 3, 0, 0, 0]);
            let faces = field(
                scene,
                &[0x12, 0x09, 30, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
            );
            match kind {
                0 => scene[faces + 10..faces + 14].copy_from_slice(&99_u32.to_le_bytes()),
                1 => scene[vertices + 6..vertices + 10].copy_from_slice(&u32::MAX.to_le_bytes()),
                _ => scene[vertices + 10..vertices + 14].copy_from_slice(&f32::NAN.to_le_bytes()),
            }
        });
        assert!(formats::load(&input.0).is_err());
    }
}

#[test]
fn native_scene_references_and_chunk_lengths_are_checked() {
    let input = Copy::new("SimpleTriangle.max");
    input.scene(|scene| {
        let node = field(
            scene,
            &[
                0x35, 0x20, 42, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 0xa1, 1, 0, 0, 1, 0, 0, 0, 0xa3,
                1, 0, 0,
            ],
        );
        scene[node + 22..node + 26].copy_from_slice(&99999_u32.to_le_bytes());
    });
    assert!(
        formats::load(&input.0)
            .unwrap_err()
            .to_string()
            .contains("reference")
    );
    let input = Copy::new("SimpleTriangle.max");
    input.scene(|scene| {
        scene[2..6].copy_from_slice(&0x8000_0005_u32.to_le_bytes());
    });
    assert!(formats::load(&input.0).is_err());
}

#[test]
fn unsupported_procedural_producer_scenes_do_not_expose_base_geometry() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/max_saved/Combine Test.max");
    assert!(formats::load(&path).is_err());
}

/// Find the addressed producer node's nearby parent field using its unique UTF-16 name.
fn parent_field(scene: &[u8], name: &str) -> usize {
    let marker: Vec<_> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let name = field(scene, &marker);
    scene[..name]
        .windows(6)
        .rposition(|v| v == [0x60, 0x09, 14, 0, 0, 0])
        .unwrap()
        + 6
}

#[test]
fn native_parent_occurrences_compose_and_cycles_fail() {
    let input = Copy::new("Instances.max");
    input.scene(|scene| {
        let parent = parent_field(scene, "InstanceBox2");
        scene[parent..parent + 4].copy_from_slice(&374_u32.to_le_bytes());
    });
    let scene = formats::load(&input.0).unwrap();
    let last = bounds(
        scene.triangles[24..]
            .iter()
            .flat_map(|triangle| triangle.vertices),
    );
    near(last[0][0], 5.5);
    near(last[1][0], 6.5);
    let input = Copy::new("Instances.max");
    input.scene(|scene| {
        let parent = parent_field(scene, "MasterBox");
        scene[parent..parent + 4].copy_from_slice(&363_u32.to_le_bytes());
    });
    assert!(
        formats::load(&input.0)
            .unwrap_err()
            .to_string()
            .contains("cyclic")
    );
}
