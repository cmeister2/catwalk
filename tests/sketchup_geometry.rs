//! Native `SketchUp` surfaces, holes and placements checked against producer exports.

use catwalk3d::{
    formats,
    render::RenderOptions,
    scene::{Scene, Vec3},
};
use std::{
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Unique temporary file suffix.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Locate an unchanged `SketchUp` producer fixture or its independent COLLADA export.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/sketchup")
        .join(name)
}

/// Find triangle coordinate bounds without consulting cached native model bounds.
fn bounds(scene: &Scene) -> [Vec3; 2] {
    let mut min = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = -min;
    for v in scene.positions() {
        min = Vec3::new(min.x.min(v.x), min.y.min(v.y), min.z.min(v.z));
        max = Vec3::new(max.x.max(v.x), max.y.max(v.y), max.z.max(v.z));
    }
    [min, max]
}

/// Total surface area; `SketchUp`'s COLLADA export stores both sides of each face.
fn area(scene: &Scene) -> f64 {
    scene
        .triangles
        .iter()
        .map(|t| {
            (t.vertices[1] - t.vertices[0])
                .cross(t.vertices[2] - t.vertices[0])
                .length()
                / 2.
        })
        .sum()
}

#[test]
fn native_surfaces_holes_and_placements_match_sketchup_collada_exports() {
    let witnesses: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture("geometry-witnesses.json")).unwrap()).unwrap();
    for name in [
        "box",
        "box-two-materials",
        "box-component-two-instances",
        "nested-3-deep",
        "face-with-hole",
        "instance-scaled",
        "component-move",
        "component-rotate",
        "ngon-face",
    ] {
        let native = formats::load(&fixture(&format!("2026-{name}.skp"))).unwrap();
        let native_bounds = bounds(&native);
        let export_bounds: [[f64; 3]; 2] =
            serde_json::from_value(witnesses[name]["bounds_mm"].clone()).unwrap();
        for (actual, expected) in native_bounds.into_iter().zip(export_bounds) {
            let expected = Vec3::new(expected[0], expected[1], expected[2]);
            assert!(
                (actual - expected).length() < 0.01,
                "{name}: {actual:?} vs {expected:?}"
            );
        }
        let expected_area = witnesses[name]["one_sided_area_mm2"].as_f64().unwrap();
        assert!(
            (area(&native) - expected_area).abs() < expected_area * 1e-6,
            "{name}: native area {} vs export {expected_area}",
            area(&native)
        );
    }
}

#[test]
fn original_box_preserves_topology_and_outward_winding() {
    let scene = formats::load(&fixture("2026-box.skp")).unwrap();
    assert_eq!(scene.triangles.len(), 12);
    assert!(scene.lines.is_empty());
    let volume = scene
        .triangles
        .iter()
        .map(|t| t.vertices[0].dot(t.vertices[1].cross(t.vertices[2])) / 6.)
        .sum::<f64>();
    assert!((volume - 1_000_000_000.).abs() < 0.01);
    assert!((area(&scene) - 6_000_000.).abs() < 0.01);
    let mirrored = formats::load(&fixture("2026-instance-scaled.skp")).unwrap();
    let volume = mirrored
        .triangles
        .iter()
        .map(|t| t.vertices[0].dot(t.vertices[1].cross(t.vertices[2])) / 6.)
        .sum::<f64>();
    assert!(
        (volume - 4_000_000_000.).abs() < 0.01,
        "placed mirrored solids retain outward winding"
    );
}

/// Rebuild the addressed modern archive, preserving unrelated member bytes.
fn rewrite_member(source: &[u8], name: &str, replacement: Option<&[u8]>) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(Cursor::new(&source[0x41..])).unwrap();
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let entry_name = entry.name().unwrap().into_owned();
        if entry_name == name && replacement.is_none() {
            continue;
        }
        let data = if entry_name == name {
            replacement.unwrap().to_vec()
        } else {
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
            data
        };
        writer
            .start_file(
                &entry_name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(&data).unwrap();
    }
    let mut result = source[..0x41].to_vec();
    result.extend(writer.finish().unwrap().into_inner());
    result
}

/// Remove a test-owned mutated model on scope exit.
struct Temporary(PathBuf);

impl Temporary {
    /// Save mutated bytes under a unique `SketchUp` input path.
    fn new(bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "catwalk-skp-geometry-{}-{}.skp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, bytes).unwrap();
        Self(path)
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
fn geometry_ignores_saved_images_and_automatic_render_falls_back() {
    let original = fixture("2026-box.skp");
    let source = fs::read(&original).unwrap();
    let options = RenderOptions {
        width: 64,
        height: 64,
        transparent: true,
        ..RenderOptions::default()
    };
    let expected = catwalk3d::geometry_thumbnail(&original, &options).unwrap();
    for replacement in [None, Some(b"corrupt saved image".as_slice())] {
        let input = Temporary::new(&rewrite_member(
            &source,
            "meta/model_thumbnail.png",
            replacement,
        ));
        let actual = catwalk3d::geometry_thumbnail(&input.0, &options).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(catwalk3d::thumbnail(&input.0, &options).unwrap(), expected);
    }
}

#[test]
fn saved_preview_and_unrelated_images_cannot_replace_missing_model_geometry() {
    let source = fs::read(fixture("2026-box.skp")).unwrap();
    for data in [None, Some(&b"invalid model records"[..])] {
        let input = Temporary::new(&rewrite_member(&source, "model.dat", data));
        assert!(formats::load(&input.0).is_err());
    }
}

/// Find a known record's payload through its explicitly typed parent path.
fn field_offset(bytes: &[u8], path: &[u16]) -> usize {
    let mut offset = 0;
    while offset < bytes.len() {
        let tag = u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap());
        let length = usize::try_from(u32::from_le_bytes(
            bytes[offset + 2..offset + 6].try_into().unwrap(),
        ))
        .unwrap();
        if tag == path[0] {
            return offset
                + 6
                + if path.len() == 1 {
                    0
                } else {
                    field_offset(&bytes[offset + 6..offset + 6 + length], &path[1..])
                };
        }
        offset += length + 6;
    }
    panic!("producer fixture lacks addressed field {path:?}");
}

#[test]
fn native_topology_numeric_records_and_crc_fail_explicitly() {
    let source = fs::read(fixture("2026-box.skp")).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(&source[0x41..])).unwrap();
    let mut original = Vec::new();
    std::io::Read::read_to_end(&mut archive.by_name("model.dat").unwrap(), &mut original).unwrap();
    for (path, bad) in [
        (
            vec![0x1f4, 0x1f6, 0x1388, 0x138b, 0x0dac, 0x0dad],
            [0_f64, 0., -1., 1000.]
                .into_iter()
                .flat_map(f64::to_le_bytes)
                .collect::<Vec<_>>(),
        ),
        (
            vec![0x1f4, 0x1f6, 0x1388, 0x1389, 0x09c4, 0x09c5],
            f64::NAN.to_le_bytes().to_vec(),
        ),
        (
            vec![0x1f4, 0x1f6, 0x1388, 0x138a, 0x0bb8, 0x0bb9],
            vec![0xff, 0xff],
        ),
        (
            vec![
                0x1f4, 0x1f6, 0x1388, 0x138b, 0x0dac, 0x0dae, 0x1194, 0x1195, 0x0fa0, 0x0fa2,
            ],
            vec![2],
        ),
    ] {
        let mut model = original.clone();
        let offset = field_offset(&model, &path);
        model[offset..offset + bad.len()].copy_from_slice(&bad);
        let input = Temporary::new(&rewrite_member(&source, "model.dat", Some(&model)));
        assert!(formats::load(&input.0).is_err(), "native mutation {path:?}");
    }
    let mut corrupt = rewrite_member(&source, "model.dat", Some(&original));
    let offset = corrupt
        .windows(original.len())
        .position(|bytes| bytes == original)
        .unwrap();
    corrupt[offset + 7] ^= 1;
    let input = Temporary::new(&corrupt);
    assert!(formats::load(&input.0).is_err());
}
