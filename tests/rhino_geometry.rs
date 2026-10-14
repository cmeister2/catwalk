//! Native Rhino producer geometry independently checked against the independent Rhino SDK.

#[path = "support/digest_hex.rs"]
mod digest_hex;

use catwalk3d::{
    Result, formats, saved_preview,
    scene::{Scene, Triangle, Vec3},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Resolve one unchanged native producer archive or independent geometry witness.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/rhino")
        .join(name)
}

/// Load the SDK geometry measurements, which never invoke Catwalk.
fn witnesses() -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(fixture(
        "geometry-witnesses.json",
    ))?)?)
}

/// Resolve one independently measured source from its retained filename.
fn witness<'a>(witnesses: &'a Value, name: &str) -> &'a Value {
    witnesses["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["file"] == name)
        .unwrap()
}

/// Convert an independently recorded native coordinate to scene millimetres.
fn position(value: &Value, scale: f64) -> Vec3 {
    let values = value.as_array().unwrap();
    Vec3::new(
        values[0].as_f64().unwrap() * scale,
        values[1].as_f64().unwrap() * scale,
        values[2].as_f64().unwrap() * scale,
    )
}

/// Measure the exact three-dimensional area of a rendered triangle.
fn area(triangle: &Triangle) -> f64 {
    let [first, second, third] = triangle.vertices;
    (second - first).cross(third - first).length() / 2.0
}

/// Measure point distance to an emitted curve chord without depending on sample ordinals.
fn chord_distance(point: Vec3, endpoints: [Vec3; 2]) -> f64 {
    let [first, second] = endpoints;
    let edge = second - first;
    let fraction = ((point - first).dot(edge) / edge.dot(edge)).clamp(0.0, 1.0);
    (point - first - edge * fraction).length()
}

/// Measure point distance to a triangle, including its finite edge boundaries.
fn triangle_distance(point: Vec3, triangle: &Triangle) -> f64 {
    let [first, second, third] = triangle.vertices;
    let normal = (second - first).cross(third - first).normalized();
    let signed_distance = (point - first).dot(normal);
    let projected = point - normal * signed_distance;
    let edges = [[first, second], [second, third], [third, first]];
    let inside = normal.length() > 0.0
        && edges
            .iter()
            .all(|&[a, b]| (b - a).cross(projected - a).dot(normal) >= -1e-12);
    if inside {
        signed_distance.abs()
    } else {
        edges
            .into_iter()
            .map(|edge| chord_distance(point, edge))
            .fold(f64::INFINITY, f64::min)
    }
}

/// Require every independent SDK curve witness to be covered by the sampled native curve.
fn check_curves(scene: &Scene, source: &Value, scale: f64, tolerance: f64) {
    for object in source["objects"].as_array().unwrap() {
        if let Some(samples) = object["samples"].as_array() {
            for sample in samples {
                let point = position(sample, scale);
                let distance = scene
                    .lines
                    .iter()
                    .map(|line| chord_distance(point, line.vertices))
                    .fold(f64::INFINITY, f64::min);
                assert!(
                    distance < tolerance,
                    "{}: SDK point {point:?} misses native chords by {distance}",
                    source["file"]
                );
            }
        }
    }
}

/// Remove one test's native source and output controls when assertions finish.
struct Temporary {
    /// Uniquely named directory owned by this test.
    path: PathBuf,
}

impl Temporary {
    /// Create an isolated directory for one test's source and output files.
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "catwalk-rhino-geometry-{}-{label}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn original_sources_and_independent_geometry_witnesses_remain_exact() -> Result<()> {
    let witnesses = witnesses()?;
    assert_eq!(witnesses["producer"], "McNeel rhino3dm");
    assert_eq!(witnesses["version"], "8.17.0");
    for source in witnesses["files"].as_array().unwrap() {
        let name = source["file"].as_str().unwrap();
        let path = fixture(name);
        let bytes = fs::read(&path)?;
        assert_eq!(
            format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))),
            source["sha256"].as_str().unwrap(),
            "native producer: {name}"
        );
        let scene = formats::load(&path)?;
        scene.validate()?;
        assert_eq!(fs::read(path)?, bytes, "geometry import preserves {name}");
    }
    Ok(())
}

#[test]
fn official_native_mesh_preserves_sdk_positions_winding_and_normals() -> Result<()> {
    let scene = formats::load(&fixture("my_mesh.3dm"))?;
    let witnesses = witnesses()?;
    let expected = &witness(&witnesses, "my_mesh.3dm")["objects"][0];
    let triangles = expected["triangles"].as_array().unwrap();
    assert_eq!(scene.triangles.len(), 6);
    assert_eq!(scene.triangles.len(), triangles.len());
    assert!(scene.points.is_empty() && scene.lines.is_empty());
    for (actual, expected) in scene.triangles.iter().zip(triangles) {
        let points: [Vec3; 3] = std::array::from_fn(|index| position(&expected[index], 1.0));
        assert_eq!(actual.vertices, points);
    }
    assert_eq!(
        scene.appearances.len(),
        6,
        "native producer shading normals survive"
    );
    for (index, face) in expected["triangle_indices"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let normals: [Vec3; 3] = std::array::from_fn(|corner| {
            position(
                &expected["normals"][usize::try_from(face[corner].as_u64().unwrap()).unwrap()],
                1.0,
            )
            .normalized()
        });
        assert_eq!(
            scene.appearances[&index].normals,
            Some(normals),
            "independent SDK native corner normals"
        );
    }
    assert!(saved_preview::extract(&fixture("my_mesh.3dm"))?.is_none());
    Ok(())
}

#[test]
fn native_versions_five_through_eight_mesh_exact_bodies_curves_units_and_visibility() -> Result<()>
{
    let witnesses = witnesses()?;
    for version in [5, 6, 7, 8] {
        let name = format!("producer_geometry_v{version}.3dm");
        let source = witness(&witnesses, &name);
        assert_eq!(source["unit_system"], "UnitSystem.Centimeters");
        assert_eq!(
            source["objects"][0]["cache_triangles"], 0,
            "exact B-rep has no stored display mesh"
        );
        let scene = formats::load(&fixture(&name))?;
        assert_eq!(scene.points.len(), 1, "hidden distant point is omitted");
        assert_eq!(scene.points[0].position, Vec3::new(-10.0, -20.0, -30.0));
        assert!(
            scene
                .positions()
                .all(|point| point.x <= 100.0 && point.y <= 40.0 && point.z <= 50.0)
        );
        let box_triangles: Vec<_> = scene
            .triangles
            .iter()
            .filter(|triangle| triangle.vertices.iter().all(|point| point.x <= 20.0 + 1e-9))
            .collect();
        assert!(box_triangles.len() >= 12);
        let surface_area: f64 = box_triangles.iter().map(|triangle| area(triangle)).sum();
        assert!(
            (surface_area - 6200.0).abs() < 1e-6,
            "exact box surface area in mm2: {surface_area}"
        );
        let volume: f64 = box_triangles
            .iter()
            .map(|triangle| {
                let [first, second, third] = triangle.vertices;
                first.dot(second.cross(third)) / 6.0
            })
            .sum();
        assert!(
            (volume.abs() - 30_000.0).abs() < 1e-6,
            "box winding encloses source volume: {volume}"
        );
        assert!(
            scene
                .triangles
                .iter()
                .all(|triangle| triangle.color == Some([220, 70, 30]))
        );
        check_curves(&scene, source, 10.0, 1e-7);
    }
    Ok(())
}

#[test]
fn official_curves_points_trimmed_surface_and_extrusions_transfer_geometry() -> Result<()> {
    let witnesses = witnesses()?;
    let curves = formats::load(&fixture("my_curves.3dm"))?;
    assert!(curves.triangles.is_empty() && curves.points.is_empty());
    check_curves(&curves, witness(&witnesses, "my_curves.3dm"), 25.4, 0.03);
    let points = formats::load(&fixture("my_points.3dm"))?;
    assert_eq!(points.points.len(), 5);
    for object in witness(&witnesses, "my_points.3dm")["objects"]
        .as_array()
        .unwrap()
    {
        let expected: Vec<&Value> = object["point"].as_array().map_or_else(
            || object["points"].as_array().unwrap().iter().collect(),
            |_| vec![&object["point"]],
        );
        for point in expected {
            assert!(
                points
                    .points
                    .iter()
                    .any(|actual| actual.position == position(point, 1000.0))
            );
        }
    }
    let trimmed = formats::load(&fixture("my_trimmed_surface.3dm"))?;
    assert!(trimmed.points.is_empty() && trimmed.lines.is_empty());
    let vertices = &witness(&witnesses, "my_trimmed_surface.3dm")["objects"][0]["vertices"];
    let expected = Triangle {
        vertices: std::array::from_fn(|index| position(&vertices[index], 1.0)),
        color: None,
    };
    assert!(
        (trimmed.triangles.iter().map(area).sum::<f64>() - area(&expected)).abs() < 1e-9,
        "native trim must constrain its supporting plane"
    );
    let mechanical = formats::load(&fixture("v4_MechPartB.3dm"))?;
    let expected_triangles = usize::try_from(
        witness(&witnesses, "v4_MechPartB.3dm")["objects"][0]["cache_triangles"]
            .as_u64()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(mechanical.triangles.len(), expected_triangles);
    assert!(mechanical.points.is_empty() && mechanical.lines.is_empty());
    let extrusions = formats::load(&fixture("v5_extrusions.3dm"))?;
    assert!(extrusions.triangles.len() > 1000);
    assert!(extrusions.points.is_empty());
    check_curves(
        &extrusions,
        witness(&witnesses, "v5_extrusions.3dm"),
        1.0,
        0.02,
    );
    let mut count = 0;
    for object in witness(&witnesses, "v5_extrusions.3dm")["objects"]
        .as_array()
        .unwrap()
    {
        if let Some(samples) = object["profile_samples"].as_array() {
            count += 1;
            for sample in samples {
                let point = position(sample, 1.0);
                let distance = extrusions
                    .triangles
                    .iter()
                    .map(|triangle| triangle_distance(point, triangle))
                    .fold(f64::INFINITY, f64::min);
                assert!(
                    distance < 0.03,
                    "native extrusion SDK profile point {point:?} misses its surface by {distance}"
                );
            }
        }
    }
    assert_eq!(
        count, 7,
        "every native extrusion has independent profile coverage"
    );
    Ok(())
}

#[test]
fn native_blocks_place_members_and_free_nurbs_surfaces_use_sdk_domains() -> Result<()> {
    let blocks = formats::load(&fixture("producer_instances.3dm"))?;
    assert_eq!(
        blocks.triangles.len(),
        2,
        "definition geometry is emitted only at its two occurrences"
    );
    for translation in [Vec3::new(10.0, 20.0, 30.0), Vec3::new(-10.0, -20.0, -30.0)] {
        let expected = [
            translation,
            translation + Vec3::new(2.0, 0.0, 0.0),
            translation + Vec3::new(0.0, 3.0, 0.0),
        ];
        assert!(blocks.triangles.iter().any(
            |triangle| triangle.vertices == expected && triangle.color == Some([20, 100, 200])
        ));
    }
    let surface = formats::load(&fixture("producer_surface.3dm"))?;
    assert_eq!(surface.triangles.len(), 64);
    assert!(surface.lines.is_empty() && surface.points.is_empty());
    let witnesses = witnesses()?;
    for expected in witness(&witnesses, "producer_surface.3dm")["objects"][0]["samples"]
        .as_array()
        .unwrap()
    {
        let expected = position(expected, 1.0);
        assert!(
            surface
                .positions()
                .any(|actual| (actual - expected).length() < 1e-9),
            "independent SDK surface sample {expected:?}"
        );
    }
    let point = formats::load(&fixture("producer_point.3dm"))?;
    assert_eq!(point.points.len(), 1);
    assert_eq!(point.points[0].position, Vec3::new(1.25, -2.5, 7.75));
    Ok(())
}

#[test]
fn headless_geometry_rendering_and_failures_preserve_native_sources_and_outputs() -> Result<()> {
    let temporary = Temporary::new("cli");
    let destination = temporary.path.join("render.png");
    let output = Command::new(env!("CARGO_BIN_EXE_catwalk"))
        .arg(fixture("my_mesh.3dm"))
        .args([
            "--render-geometry",
            "--size",
            "128",
            "--transparent",
            "--output",
        ])
        .arg(&destination)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR")
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let image = image::open(&destination)?.into_rgba8();
    assert_eq!(image.dimensions(), (128, 128));
    assert!(image.pixels().any(|pixel| pixel[3] == 0));
    assert!(image.pixels().any(|pixel| pixel[3] != 0));
    fs::write(&destination, b"existing thumbnail")?;
    let mut source = fs::read(fixture("my_mesh.3dm"))?;
    source.truncate(source.len() - 24);
    let truncated = temporary.path.join("truncated.3DM");
    fs::write(&truncated, &source)?;
    for path in [truncated.clone(), fixture("v3_DeformSpheres.3dm")] {
        let output = Command::new(env!("CARGO_BIN_EXE_catwalk"))
            .arg(path)
            .args(["--render-geometry", "--output"])
            .arg(&destination)
            .output()?;
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(fs::read(&destination)?, b"existing thumbnail");
    }
    assert_eq!(fs::read(truncated)?, source);
    let oversized = temporary.path.join("oversized.3dm");
    fs::File::create(&oversized)?.set_len(512 * 1024 * 1024 + 1)?;
    assert!(
        formats::load(&oversized)
            .unwrap_err()
            .to_string()
            .contains("512 MiB")
    );
    Ok(())
}
