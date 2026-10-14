//! Native default/current tag visibility and selected annotation controls.

use catwalk3d::{formats, scene::Scene};
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Unique native control-file suffix.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Borrowed native TLV record preserving its original order and payload.
#[derive(Clone)]
struct Field {
    /// Native record tag.
    tag: u16,
    /// Exact leaf payload or independently reframed child records.
    bytes: Vec<u8>,
}

/// Decode only the explicit native record level used by a control.
fn fields(bytes: &[u8]) -> Vec<Field> {
    let mut result = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let tag = u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap());
        let len = usize::try_from(u32::from_le_bytes(
            bytes[offset + 2..offset + 6].try_into().unwrap(),
        ))
        .unwrap();
        result.push(Field {
            tag,
            bytes: bytes[offset + 6..offset + 6 + len].to_vec(),
        });
        offset += 6 + len;
    }
    assert_eq!(offset, bytes.len());
    result
}

/// Preserve every sibling while serializing addressed native child modifications.
fn encode(fields: &[Field]) -> Vec<u8> {
    let mut result = Vec::new();
    for field in fields {
        result.extend(field.tag.to_le_bytes());
        result.extend(u32::try_from(field.bytes.len()).unwrap().to_le_bytes());
        result.extend(&field.bytes);
    }
    result
}

/// Read one unique addressed child from its typed parent.
fn child(bytes: &[u8], tag: u16) -> Vec<u8> {
    let fields = fields(bytes);
    let selected: Vec<_> = fields.iter().filter(|field| field.tag == tag).collect();
    assert_eq!(selected.len(), 1);
    selected[0].bytes.clone()
}

/// Mutate one explicitly selected nested record without scanning leaf bytes.
fn edit_path(bytes: &[u8], path: &[u16], edit: &mut impl FnMut(&[u8]) -> Vec<u8>) -> Vec<u8> {
    if path.is_empty() {
        return edit(bytes);
    }
    let mut fields = fields(bytes);
    let mut count = 0;
    for field in &mut fields {
        if field.tag == path[0] {
            field.bytes = edit_path(&field.bytes, &path[1..], edit);
            count += 1;
        }
    }
    assert_eq!(count, 1, "addressed producer field {path:?}");
    encode(&fields)
}

/// Replace, append or remove one field from an addressed parent.
fn set(bytes: &[u8], tag: u16, value: Option<&[u8]>) -> Vec<u8> {
    let mut fields = fields(bytes);
    fields.retain(|field| field.tag != tag);
    if let Some(value) = value {
        fields.push(Field {
            tag,
            bytes: value.to_vec(),
        });
    }
    encode(&fields)
}

/// Read an unchanged native producer archive and its addressed model member.
fn source(name: &str) -> (Vec<u8>, Vec<u8>) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/sketchup")
        .join(name);
    let bytes = fs::read(path).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes[0x41..])).unwrap();
    let mut model = Vec::new();
    archive
        .by_name("model.dat")
        .unwrap()
        .read_to_end(&mut model)
        .unwrap();
    (bytes, model)
}

/// Save one validly reframed native TLV control, recomputing its addressed ZIP CRC.
struct Temporary(PathBuf);
impl Temporary {
    /// Preserve every original archive member while replacing selected model records.
    fn new(source: &[u8], model: &[u8]) -> Self {
        let mut archive = zip::ZipArchive::new(Cursor::new(&source[0x41..])).unwrap();
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let name = entry.name().unwrap().into_owned();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            writer
                .start_file(
                    &name,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer
                .write_all(if name == "model.dat" { model } else { &data })
                .unwrap();
        }
        let mut bytes = source[..0x41].to_vec();
        bytes.extend(writer.finish().unwrap().into_inner());
        let path = std::env::temp_dir().join(format!(
            "catwalk-skp-tag-{}-{}.skp",
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

/// Measure independent physical surface area from emitted triangle positions.
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

/// Change layer flags/current selection through the native layer manager only.
fn layer_state(model: &[u8], hide_default: bool, current: Option<u64>) -> Vec<u8> {
    edit_path(model, &[0x1f4, 0x1f8, 0x3a98], &mut |manager| {
        let pool = child(manager, 0x3a99);
        let mut records = fields(&pool);
        let first = records
            .iter_mut()
            .find(|field| field.tag == 0x3c8c)
            .unwrap();
        if hide_default {
            first.bytes = set(&first.bytes, 0x3c8e, Some(&[1]));
        }
        let manager = set(manager, 0x3a99, Some(&encode(&records)));
        current.map_or(manager.clone(), |id| {
            set(&manager, 0x3a9a, Some(&id.to_le_bytes()))
        })
    })
}

/// Bind selected actual drawing entities to an explicit or implicit default layer.
fn bind_container(container: &[u8], selection: Option<u64>, layer: Option<u64>) -> Vec<u8> {
    let mut result = fields(container);
    for pool in &mut result {
        let wrapper = match pool.tag {
            0x138a => 0x0bb8,
            0x138b => 0x0dac,
            0x138c => 0x1964,
            _ => continue,
        };
        let mut items = fields(&pool.bytes);
        for item in &mut items {
            if item.tag != wrapper {
                continue;
            }
            item.bytes = edit_path(&item.bytes, &[0x7d0], &mut |drawing| {
                let original = fields(drawing)
                    .into_iter()
                    .find(|field| field.tag == 0x7d2)
                    .map(|field| {
                        u64::from_le_bytes({
                            let mut bytes = [0; 8];
                            bytes[..field.bytes.len()].copy_from_slice(&field.bytes);
                            bytes
                        })
                    });
                if selection.is_none() || selection == original {
                    set(
                        drawing,
                        0x7d2,
                        layer
                            .as_ref()
                            .map(|id| id.to_le_bytes())
                            .as_ref()
                            .map(<[u8; 8]>::as_slice),
                    )
                } else {
                    drawing.to_vec()
                }
            });
        }
        pool.bytes = encode(&items);
    }
    encode(&result)
}

#[test]
fn native_current_layer_remains_visible_while_default_entity_tags_obey_visibility() {
    let (bytes, model) = source("2026-layers.skp");
    // The unchanged producer has 1 m, 0.5 m and 0.25 m cubes tagged Big,
    // Medium and Little. Medium is hidden. The active native default tag
    // is id5066; Big/Medium/Little are ids5045/5047/5049 respectively.
    let original = formats::load(&Temporary::new(&bytes, &model).0).unwrap();
    assert_eq!(original.triangles.len(), 24);
    assert!((area(&original) - 6_375_000.0).abs() < 1e-6);
    let active_medium = layer_state(&model, false, Some(5047));
    let scene = formats::load(&Temporary::new(&bytes, &active_medium).0).unwrap();
    assert_eq!(scene.triangles.len(), 36);
    assert!(
        (area(&scene) - 7_875_000.0).abs() < 1e-6,
        "hidden current tag is still drawn"
    );
    let default_hidden = layer_state(&model, true, Some(5045));
    let default_hidden = edit_path(&default_hidden, &[0x1f4, 0x1f6, 0x1388], &mut |container| {
        bind_container(container, Some(5045), None)
    });
    let scene = formats::load(&Temporary::new(&bytes, &default_hidden).0).unwrap();
    assert_eq!(scene.triangles.len(), 12);
    assert!(
        (area(&scene) - 375_000.0).abs() < 1e-6,
        "implicit default-tag Big faces/edges disappear while explicit Little faces remain"
    );
    let malformed = layer_state(&model, false, Some(u64::MAX));
    assert!(
        formats::load(&Temporary::new(&bytes, &malformed).0)
            .unwrap_err()
            .to_string()
            .contains("current layer")
    );
    let (bytes, model) = source("2026-box.skp");
    let hidden_current_default = layer_state(&model, true, None);
    let scene = formats::load(&Temporary::new(&bytes, &hidden_current_default).0).unwrap();
    assert_eq!(scene.triangles.len(), 12);
    assert!((area(&scene) - 6_000_000.0).abs() < 1e-6);
}

#[test]
fn definition_storage_does_not_hide_explicitly_tagged_placed_geometry() {
    let (layers_source, layers_model) = source("2026-layers.skp");
    let (_, component_model) = source("2026-box-component-two-instances.skp");
    let component_root = child(&component_model, 0x1f4);
    let definitions = child(&component_root, 0x1f9);
    let definitions = edit_path(&definitions, &[0x1770, 0x1771], &mut |pool| {
        let mut records = fields(pool);
        for field in &mut records {
            if field.tag == 0x157c {
                field.bytes = edit_path(&field.bytes, &[0x1388], &mut |container| {
                    bind_container(container, None, Some(5045))
                });
            }
        }
        encode(&records)
    });
    let own = edit_path(
        &child(&component_root, 0x1f6),
        &[0x1388],
        &mut |container| bind_container(container, None, Some(5045)),
    );
    let model = edit_path(&layers_model, &[0x1f4], &mut |root| {
        let root = set(root, 0x1f9, Some(&definitions));
        set(&root, 0x1f6, Some(&own))
    });
    let model = layer_state(&model, true, Some(5045));
    let scene = formats::load(&Temporary::new(&layers_source, &model).0).unwrap();
    assert_eq!(
        scene.triangles.len(),
        24,
        "two placed cube instances survive hidden default storage tag"
    );
    let witnesses: serde_json::Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/formats/sketchup/geometry-witnesses.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let expected = witnesses["box-component-two-instances"]["one_sided_area_mm2"]
        .as_f64()
        .unwrap();
    assert!(
        (area(&scene) - expected).abs() < expected * 1e-6,
        "tag rebinding retains independent producer placement geometry"
    );
}

#[test]
fn selected_visible_annotations_fail_while_hidden_and_unplaced_annotations_do_not() {
    let (bytes, model) = source("2026-dimension.skp");
    assert!(
        formats::load(&Temporary::new(&bytes, &model).0)
            .unwrap_err()
            .to_string()
            .contains("text/dimension")
    );
    let hidden = edit_path(&model, &[0x1f4, 0x1f6, 0x1388, 0x1399], &mut |pool| {
        let mut records = fields(pool);
        for field in &mut records {
            if field.tag == 0x5bcc {
                field.bytes = edit_path(&field.bytes, &[0x59d8, 0x7d0], &mut |drawing| {
                    set(drawing, 0x7d3, Some(&[1]))
                });
            }
        }
        encode(&records)
    });
    let scene = formats::load(&Temporary::new(&bytes, &hidden).0).unwrap();
    assert_eq!(scene.triangles.len(), 12);
    let (source, root_model) = source("2026-box.skp");
    let dimension_root = child(&model, 0x1f4);
    let definitions = child(&dimension_root, 0x1f9);
    let annotation_container = child(&child(&dimension_root, 0x1f6), 0x1388);
    let annotation_container = edit_path(&annotation_container, &[0x7d0, 0x5dc], &mut |base| {
        set(base, 0x5de, Some(&u64::MAX.to_le_bytes()))
    });
    let definition = encode(&[Field {
        tag: 0x157c,
        bytes: encode(&[Field {
            tag: 0x1388,
            bytes: annotation_container,
        }]),
    }]);
    let definitions = edit_path(&definitions, &[0x1770, 0x1771], &mut |pool| {
        let mut pool = fields(pool);
        pool.extend(fields(&definition));
        encode(&pool)
    });
    let model = edit_path(&root_model, &[0x1f4], &mut |root| {
        set(root, 0x1f9, Some(&definitions))
    });
    let scene = formats::load(&Temporary::new(&source, &model).0).unwrap();
    assert_eq!(
        scene.triangles.len(),
        12,
        "unplaced annotation-bearing definition is not selected geometry"
    );
}
