//! Genuine Inventor B-rep and assembly qualification against independent exports.

#[path = "support/digest_hex.rs"]
/// Compare retained native CAD bytes with independently pinned source digests.
mod digest_hex;

use catwalk3d::{
    formats, geometry_thumbnail,
    render::{RenderOptions, render},
    scene::{Scene, Vec3},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Distinguish independent malformed-project controls in parallel test runs.
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

/// Resolve an original Inventor document, companion or independent reference.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/native_geometry/inventor")
        .join(name)
}

/// Isolated source copies for missing references, cycles and saved-image controls.
struct TemporaryProject(PathBuf);

impl TemporaryProject {
    /// Copy the intact public conveyor project into an isolated directory.
    fn conveyor() -> Self {
        let path = std::env::temp_dir().join(format!(
            "catwalk-inventor-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        for name in [
            "box.ipt",
            "part.ipt",
            "rail.ipt",
            "support.ipt",
            "conveyor_model.iam",
        ] {
            fs::copy(fixture(&format!("conveyor/{name}")), path.join(name)).unwrap();
        }
        Self(path)
    }
}

impl Drop for TemporaryProject {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

/// Independent axis bounds over every emitted geometric position.
fn bounds(scene: &Scene) -> [Vec3; 2] {
    scene.positions().fold(
        [
            Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
        ],
        |[minimum, maximum], point| {
            [
                Vec3::new(
                    minimum.x.min(point.x),
                    minimum.y.min(point.y),
                    minimum.z.min(point.z),
                ),
                Vec3::new(
                    maximum.x.max(point.x),
                    maximum.y.max(point.y),
                    maximum.z.max(point.z),
                ),
            ]
        },
    )
}

/// Oriented enclosed volume computed from triangles without native-parser helpers.
fn volume(scene: &Scene) -> f64 {
    scene
        .triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.vertices;
            a.dot(b.cross(c)) / 6.0
        })
        .sum::<f64>()
}

/// Independently evaluate the surface area of every emitted triangle.
fn area(scene: &Scene) -> f64 {
    scene
        .triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.vertices;
            (b - a).cross(c - a).length() / 2.0
        })
        .sum()
}

/// Read analytic measurements of the producer's original assembly export.
fn conveyor_oracle() -> serde_json::Value {
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture("reference-metrics.json")).unwrap()).unwrap();
    manifest["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["file"] == "references/conveyor_model.step")
        .unwrap()
        .clone()
}

#[test]
fn inventor_fixtures_are_untouched_real_producer_files_with_retained_licenses() {
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture("provenance.json")).unwrap()).unwrap();
    for entry in manifest.as_array().unwrap() {
        let name = entry["file"].as_str().unwrap();
        let bytes = fs::read(fixture(name)).unwrap();
        assert_eq!(
            bytes.len() as u64,
            entry["bytes"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(
            format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))),
            entry["sha256"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(entry["license"], "MIT");
        assert!(fixture(entry["license_file"].as_str().unwrap()).is_file());
    }
}

#[test]
fn each_real_conveyor_part_renders_solved_native_geometry() {
    for name in ["box", "part", "rail", "support"] {
        let input = fixture(&format!("conveyor/{name}.ipt"));
        let scene = formats::load(&input).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(scene.triangles.len() >= 12, "{name}");
        assert!(volume(&scene) > 1.0, "{name}");
        let options = RenderOptions {
            width: 96,
            height: 96,
            transparent: true,
            supersampling: 1,
            ..RenderOptions::default()
        };
        let geometry = geometry_thumbnail(&input, &options).unwrap();
        assert_eq!(geometry, render(&scene, &options).unwrap(), "{name}");
        assert!(
            geometry.pixels().filter(|pixel| pixel[3] != 0).count() > 100,
            "{name}"
        );
    }
}

#[test]
fn whole_native_assembly_matches_independently_exported_step_geometry() {
    let native = formats::load(&fixture("conveyor/conveyor_model.iam")).unwrap();
    let oracle = conveyor_oracle();
    // The original native documents contain exactly the export's four bodies
    // and 137 faces. Tessellation is independent, so compare geometry rather
    // than demanding the same triangle subdivision.
    let oracle_bounds = &oracle["metrics"]["bbox_mm"];
    let expected = ["min", "max"].map(|name| {
        let coordinates = oracle_bounds[name].as_array().unwrap();
        Vec3::new(
            coordinates[0].as_f64().unwrap(),
            coordinates[1].as_f64().unwrap(),
            coordinates[2].as_f64().unwrap(),
        )
    });
    for (actual, expected) in bounds(&native).into_iter().zip(expected) {
        assert!(
            (actual - expected).length() < 0.05,
            "native {actual:?}, STEP {expected:?}"
        );
    }
    let expected_volume = oracle["metrics"]["volume_mm3"].as_f64().unwrap();
    assert!((volume(&native) / expected_volume - 1.0).abs() < 0.02);
    let expected_area = oracle["metrics"]["area_mm2"].as_f64().unwrap();
    assert!((area(&native) / expected_area - 1.0).abs() < 0.02);
    // Measure against each independent source solid too: errors in separate
    // parts must not cancel in the assembly's total mass or surface area.
    for component in oracle["components"].as_array().unwrap() {
        let name = component["native_file"].as_str().unwrap();
        let part = formats::load(&fixture(name)).unwrap();
        let expected_volume = component["metrics"]["volume_mm3"].as_f64().unwrap();
        let expected_area = component["metrics"]["area_mm2"].as_f64().unwrap();
        assert!(
            (volume(&part) / expected_volume - 1.0).abs() < 0.02,
            "{name} volume"
        );
        assert!(
            (area(&part) / expected_area - 1.0).abs() < 0.02,
            "{name} surface area"
        );
    }
    let parts: usize = ["box", "part", "rail", "support"]
        .into_iter()
        .map(|name| {
            formats::load(&fixture(&format!("conveyor/{name}.ipt")))
                .unwrap()
                .triangles
                .len()
        })
        .sum();
    assert_eq!(native.triangles.len(), parts);
}

#[test]
fn original_motor_mount_matches_the_authors_independent_stl() {
    let native = formats::load(&fixture("mount/Motor Mount v2.ipt")).unwrap();
    let reference = formats::load(&fixture("references/Motor Mount v2.stl")).unwrap();
    for (actual, expected) in bounds(&native).into_iter().zip(bounds(&reference)) {
        assert!(
            (actual - expected).length() < 0.03,
            "native {actual:?}, STL {expected:?}"
        );
    }
    assert!((volume(&native) / volume(&reference) - 1.0).abs() < 0.02);
}

#[test]
fn original_motor_bottom_preserves_native_torus_trim_and_outward_orientation() {
    let oracle: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture("motor-bottom-torus-oracle.json")).unwrap())
            .unwrap();
    let bytes = fs::read(fixture("motor/Motor Bot.ipt")).unwrap();
    assert_eq!(
        format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))),
        oracle["source_sha256"].as_str().unwrap()
    );
    let scene = formats::load(&fixture("motor/Motor Bot.ipt")).unwrap();
    let carrier = &oracle["carrier_mm"];
    let major = carrier["major_radius"].as_f64().unwrap();
    let minor = carrier["minor_radius"].as_f64().unwrap().abs();
    let center_z = carrier["center_z"].as_f64().unwrap();
    let mut torus_area = 0.0;
    let mut torus_triangles = 0;
    for triangle in &scene.triangles {
        // The exact native circular carrier identifies this face independently
        // of its parser ID or mesh subdivision. An interior vertex excludes
        // neighbouring planar/cylindrical triangles on the two joint rings.
        let on_carrier = triangle.vertices.iter().all(|point| {
            ((point.x.hypot(point.y) - major).powi(2) + (point.z - center_z).powi(2)
                - minor.powi(2))
            .abs()
                < 1e-5
        });
        let off_joint_rings = triangle.vertices.iter().any(|point| {
            (point.z - center_z).abs() > 1e-5 && (point.z - (center_z - minor)).abs() > 1e-5
        });
        if !on_carrier || !off_joint_rings {
            continue;
        }
        torus_triangles += 1;
        for point in triangle.vertices {
            // Original PmGraphicsFace key3673 stores these geometric bounds,
            // in centimetres. The complementary torus patch violates both.
            assert!(point.x.hypot(point.y) <= major + 1e-5, "{point:?}");
            assert!(
                point.z >= center_z - minor - 1e-5 && point.z <= center_z + 1e-5,
                "{point:?}"
            );
        }
        let [a, b, c] = triangle.vertices;
        let normal = (b - a).cross(c - a);
        let center = (a + b + c) * (1.0 / 3.0);
        // The original adjoining solid shaft points outward radially; the
        // adjoining floor points upward. The torus must meet both smoothly.
        assert!(normal.x * center.x + normal.y * center.y > 0.0);
        assert!(normal.z > 0.0);
        torus_area += normal.length() / 2.0;
    }
    assert!(torus_triangles > 100);
    let expected_area = oracle["quarter_area_mm2"].as_f64().unwrap();
    assert!((torus_area / expected_area - 1.0).abs() < 0.01);
}

#[test]
fn original_assembly_uses_real_non_identity_rotations_and_translations() {
    let assembly = formats::load(&fixture("motor/AXI 2204 Multicopter.iam")).unwrap();
    let bottom = formats::load(&fixture("motor/Motor Bot.ipt")).unwrap();
    let top = formats::load(&fixture("motor/Motor Top.ipt")).unwrap();
    assert_eq!(
        assembly.triangles.len(),
        bottom.triangles.len() + top.triangles.len()
    );
    let translated = bottom.triangles[0].vertices[0]
        + Vec3::new(
            -3.227_713_359_328_246_3,
            0.212_816_279_191_622_55,
            -18.983_074_920_982_354,
        );
    assert!((assembly.triangles[0].vertices[0] - translated).length() < 1e-10);
    let point = top.triangles[0].vertices[0];
    // Literal native AmGraphics coefficients, independently recorded from the
    // original bytes; source translation is converted from centimetres.
    let placed = Vec3::new(
        0.972_391_173_463_430_5 * point.x
            - 0.233_356_820_706_861_84 * point.z
            - 3.105_577_667_045_102,
        -0.233_356_820_706_861_73 * point.x - 0.972_391_173_463_431 * point.z
            + 0.183_505_855_954_735_37,
        point.y - 12.513_074_920_982_35,
    );
    assert!((assembly.triangles[bottom.triangles.len()].vertices[0] - placed).length() < 1e-10);
}

#[test]
fn missing_companions_fail_instead_of_rendering_a_partial_assembly() {
    let project = TemporaryProject::conveyor();
    fs::remove_file(project.0.join("support.ipt")).unwrap();
    let error = formats::load(&project.0.join("conveyor_model.iam"))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("missing Inventor companion support.ipt"),
        "{error}"
    );
}

#[test]
fn native_saved_image_carriers_without_geometry_do_not_fake_a_model() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for name in ["part-carrier.ipt", "assembly-carrier.iam"] {
        let input = root
            .join("tests/fixtures/saved-previews/autodesk")
            .join(name);
        assert!(
            geometry_thumbnail(&input, &RenderOptions::default()).is_err(),
            "{name}"
        );
    }
}

#[test]
fn cyclic_native_references_are_rejected_before_geometry_expansion() {
    let project = TemporaryProject::conveyor();
    let path = project.0.join("loop.iam");
    fs::copy(project.0.join("conveyor_model.iam"), &path).unwrap();
    let mut file = cfb::open_rw(&path).unwrap();
    let mut bytes = Vec::new();
    file.open_stream("/UFRxDoc")
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    let before: Vec<u8> = "rail.ipt"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let after: Vec<u8> = "loop.iam"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let offsets = bytes
        .windows(before.len())
        .enumerate()
        .filter_map(|(offset, value)| (value == before).then_some(offset))
        .collect::<Vec<_>>();
    assert_ne!(offsets, [] as [usize; 0]);
    for offset in offsets {
        bytes[offset..offset + after.len()].copy_from_slice(&after);
    }
    file.open_stream("/UFRxDoc")
        .unwrap()
        .write_all(&bytes)
        .unwrap();
    drop(file);
    let error = formats::load(&path).unwrap_err().to_string();
    assert!(
        error.contains("cyclic Inventor assembly reference"),
        "{error}"
    );
}

#[test]
fn nonempty_local_assembly_results_cannot_be_ignored_when_composing_parts() {
    let project = TemporaryProject::conveyor();
    let path = project.0.join("conveyor_model.iam");
    let mut file = cfb::open_rw(&path).unwrap();
    // This original's exact AmBRep stream and typed kernel record were
    // independently inventoried in the producer bytes. The negative control
    // changes only its declared entity count; it supplies no invented model.
    let stream_name = "/RSeStorage/B62d2dgbozboawhple1g53m4lnh";
    let mut stored = Vec::new();
    file.open_stream(stream_name)
        .unwrap()
        .read_to_end(&mut stored)
        .unwrap();
    let mut expanded = Vec::new();
    flate2::read::ZlibDecoder::new(&stored[18..])
        .read_to_end(&mut expanded)
        .unwrap();
    assert_eq!(&expanded[41..56], b"ASM BinaryFile4");
    assert_eq!(u32::from_le_bytes(expanded[64..68].try_into().unwrap()), 1);
    expanded[64..68].copy_from_slice(&2u32.to_le_bytes());
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&expanded).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut replacement = stored[..18].to_vec();
    replacement.extend(compressed);
    file.create_stream(stream_name)
        .unwrap()
        .write_all(&replacement)
        .unwrap();
    drop(file);
    let error = formats::load(&path).unwrap_err().to_string();
    assert!(
        error.contains("unsupported local result geometry or assembly features"),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn native_references_cannot_follow_symlinks_outside_the_project() {
    let project = TemporaryProject::conveyor();
    let part = project.0.join("support.ipt");
    fs::remove_file(&part).unwrap();
    std::os::unix::fs::symlink(fixture("conveyor/support.ipt"), part).unwrap();
    let error = formats::load(&project.0.join("conveyor_model.iam"))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("escapes the supplied project directory"),
        "{error}"
    );
}
