//! Stored-model Lychee originals and neutral placement/preparation witnesses.

use catwalk3d::Result;
use rmpv::Value;
use serde_json::{Value as Json, json};
use std::{fs, path::Path};

/// Neutral generated-model directory; licensed scenes remain in their original directory.
const DIR: &str = "lychee_geometry";
/// Deliberately asymmetric local tetrahedron with an already centered bounding box.
const POINTS: [[f32; 3]; 4] = [
    [-3., -2., -2.],
    [3., -2., -2.],
    [-1., 2., -2.],
    [0., -1., 2.],
];
/// Outward-wound source triangle order, independent of scene transforms.
const FACES: [[u32; 3]; 4] = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [2, 0, 3]];
/// Constant protocol spelling represented with the public parser's source-hygiene mask.
/// Pinned MIT df-plugin-lys cc4e648; original notice is retained in DF-PLUGIN-LYS-LICENSE.
const PROTOCOL_BYTES: [u8; 36] = [
    0x25, 0x4a, 0x04, 0x02, 0x5e, 0x5f, 0x72, 0x44, 0x58, 0x51, 0x10, 0x76, 0x67, 0x7a, 0x70, 0x10,
    0x57, 0x5e, 0x42, 0x56, 0x27, 0x44, 0x42, 0x44, 0x41, 0x7f, 0x64, 0x67, 0x7d, 0x13, 0x52, 0x01,
    0x56, 0x0b, 0x23, 0x45,
];
/// Public interoperability mask from the same pinned MIT reference parser.
const PROTOCOL_MASK: &[u8] = b"DragonFruitFTW";
/// Original scene and STL redistribution comes from the same author's MIT-licensed repository.
const ORIGINAL: &str = "Unmodified JRiggles/DELPHI-kbd keyboard scene and independently retained original STL, commit a666598d5916b73f8a3fac796e569878de18bb79; MIT, Copyright (c) 2022-25 John Riggles, notice retained as lychee/DELPHI-LICENSE. Fresh render uses the original stored indexed model with scene placement; supports, raft, hollowing and holes preparation are intentionally excluded. All original triangle-corner scalars are checked against the author's STL with its recorded source center, then placed corners against independent literal references. https://github.com/JRiggles/DELPHI-kbd/tree/a666598d5916b73f8a3fac796e569878de18bb79/cases";
/// Authored controls qualify the stated model subset without claiming a desktop exporter.
const AUTHORED: &str = "Author-created deterministic Rust Lychee model-only control; MIT, matching the project license. Qualified flat Mango 3.1.0 / scene-state v2 / mesh-v2 layout; literal asymmetric geometry and independent placement expectations are in tests/lychee_geometry.rs. Scene byte interoperability constants follow the MIT Open-Resin-Alliance/df-plugin-lys parser at cc4e64805d2e31095b084e244ef884018c92d1ba; original notice retained as lychee_geometry/DF-PLUGIN-LYS-LICENSE. This authored subset does not establish compatibility with other Lychee save versions or prepared solids. https://github.com/Open-Resin-Alliance/df-plugin-lys/tree/cc4e64805d2e31095b084e244ef884018c92d1ba";

/// Store qualified indexed float32 geometry with its full twenty-byte v2 header.
fn mesh() -> Vec<u8> {
    let mut bytes = Vec::new();
    for word in [2_u32, 12, 12, 12, 0] {
        bytes.extend(word.to_le_bytes());
    }
    for index in FACES.into_iter().flatten() {
        bytes.extend(index.to_le_bytes());
    }
    for scalar in POINTS.into_iter().flatten() {
        bytes.extend(scalar.to_le_bytes());
    }
    bytes
}

/// Convert small, owned authored JSON values to the public `MessagePack` value API.
fn message_value(value: &Json) -> Value {
    match value {
        Json::Null => Value::Nil,
        Json::Bool(value) => Value::Boolean(*value),
        Json::Number(value) => {
            if let Some(integer) = value.as_u64() {
                Value::from(integer)
            } else if let Some(integer) = value.as_i64() {
                Value::from(integer)
            } else {
                Value::F64(value.as_f64().expect("authored finite scalar"))
            }
        }
        Json::String(value) => Value::from(value.as_str()),
        Json::Array(values) => Value::Array(values.iter().map(message_value).collect()),
        Json::Object(values) => Value::Map(
            values
                .iter()
                .map(|(key, value)| (Value::from(key.as_str()), message_value(value)))
                .collect(),
        ),
    }
}

/// Encode only a neutral authored scene with the qualified additive byte protocol.
fn scene_bytes(scene: &Json) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    rmpv::encode::write_value(&mut bytes, &message_value(scene))?;
    for (index, byte) in bytes.iter_mut().enumerate() {
        let protocol_index = index % PROTOCOL_BYTES.len();
        let identifier =
            PROTOCOL_BYTES[protocol_index] ^ PROTOCOL_MASK[protocol_index % PROTOCOL_MASK.len()];
        *byte = byte.wrapping_add(identifier);
    }
    Ok(bytes)
}

/// Construct the complete flat manifest framing without using the production reader.
fn container(members: &[(&str, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut files = serde_json::Map::new();
    let mut data: Vec<u8> = Vec::new();
    for (name, bytes) in members {
        files.insert(
            (*name).into(),
            json!({"offset":data.len().to_string(), "size":bytes.len()}),
        );
        data.extend(bytes);
    }
    let manifest = serde_json::to_vec(&json!({"version":"3.1.0", "mangoFiles":files}))?;
    let padded = manifest
        .len()
        .checked_add(3)
        .ok_or("fixture manifest overflow")?
        & !3;
    let json_bytes = u32::try_from(manifest.len())?;
    let padded_bytes = u32::try_from(padded)?;
    let mut bytes = Vec::new();
    for word in [4, padded_bytes + 8, padded_bytes + 4, json_bytes] {
        bytes.extend(word.to_le_bytes());
    }
    bytes.extend(manifest);
    bytes.resize(16 + padded, 0);
    bytes.extend(data);
    Ok(bytes)
}

/// Construct an explicitly selected source-model occurrence with neutral metadata.
fn object(id: &str, position: [f64; 3], rotation: [f64; 3], scale: [f64; 3]) -> Json {
    json!({
        "id":id,"type":"file","loaded":true,"initialized":true,
        "visible":true,"isOnScene":true,"isHole":false,"isOperationObject":false,
        "properties":{"hash":"asymmetric-model"},
        "position":{"x":position[0],"y":position[1],"z":position[2]},
        "rotation":{"x":rotation[0],"y":rotation[1],"z":rotation[2]},
        "scale":{"x":scale[0],"y":scale[1],"z":scale[2]},
        "center":{"x":123,"y":-456,"z":789},
        "formerCenter":{"x":-1000,"y":2000,"z":-3000},
        "hollowing":{"enabled":false},"foundation":{"enabled":false},
        "holes":[],"supportsBase":[],"supportsTip":[]
    })
}

/// Make a version-two resin scene without printer, filesystem or user information.
fn scene(objects: &[Json]) -> Json {
    let mut by_id = serde_json::Map::new();
    let mut ids = Vec::new();
    for object in objects {
        let id = object["id"].as_str().expect("authored object id");
        by_id.insert(id.into(), object.clone());
        ids.push(id);
    }
    json!({
        "version":2,"appVersion":"5.4.3","kind":{"current":"MSLA"},
        "objects":{"present":{"byId":by_id,"allIds":ids}},
        "supports":{"present":{"byId":{},"allIds":[]}},
        "holes":{"present":{"byId":{},"allIds":[]}}
    })
}

/// Register one addressed source scene for a fresh model render in the gallery.
fn register(
    root: &Path,
    id: &str,
    title: &str,
    input: &str,
    provenance: &str,
) -> Result<crate::Fixture> {
    let loaded = catwalk3d::formats::load(&root.join(input))?;
    if loaded.triangles.is_empty() {
        return Err("Lychee stored-model fixture has no triangles".into());
    }
    Ok(crate::fixture(
        id,
        "lys",
        title,
        input,
        "native-geometry",
        provenance,
    ))
}

/// Place a shared source twice and retain two explicitly excluded scene occurrences.
fn shared_scene(posed: &Json) -> Json {
    let second = object("o2", [-9., 8., 2.], [0., 0., 90.], [1.; 3]);
    let mut hidden = object("o3", [1000.; 3], [0.; 3], [1.; 3]);
    hidden["visible"] = json!(false);
    let mut off_scene = object("o4", [-1000.; 3], [0.; 3], [1.; 3]);
    off_scene["isOnScene"] = json!(false);
    scene(&[posed.clone(), second, hidden, off_scene])
}

/// Add preparation metadata and tools without changing the original source model.
fn preparation_scene(posed: &Json) -> Json {
    let mut prepared = posed.clone();
    // Producer metadata and preparation hashes do not change the addressed source model.
    prepared["properties"]["lysHash"] = json!("asymmetric-model_hollowing");
    prepared["hollowing"] = json!({"enabled":true,"outer":1.25});
    prepared["foundation"] = json!({"enabled":true,"visible":true,"size":2});
    prepared["holes"] = json!(["h1"]);
    prepared["supportsBase"] = json!(["s1"]);
    prepared["supportsTip"] = json!(["s1"]);
    let mut hole = object("hole-tool", [3000.; 3], [0.; 3], [1.; 3]);
    hole["isHole"] = json!(true);
    let mut operation = object("operation-tool", [-3000.; 3], [0.; 3], [1.; 3]);
    operation["isOperationObject"] = json!(true);
    let mut preparation = scene(&[prepared, hole, operation]);
    preparation["appVersion"] = json!("7.6.6");
    preparation["supports"]["present"] = json!({
        "allIds":["s1"],"byId":{"s1":{
            "id":"s1","type":"straight","objectIdBase":"o1","objectIdTip":"o1",
            "base":{"x":4000,"y":5000,"z":0},"tip":{"x":4000,"y":5000,"z":6000}
        }}
    });
    preparation["holes"]["present"] = json!({
        "allIds":["h1"],"byId":{"h1":{"id":"h1","objectId":"o1","radius":100}}
    });
    preparation
}

/// Generate four neutral controls and register two untouched original keyboard scenes.
///
/// # Errors
///
/// Returns an error if encoding, fixture writes, original reads or native geometry loading fails.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    let directory = root.join(DIR);
    fs::create_dir_all(&directory)?;
    let identity = object("o1", [0.; 3], [0.; 3], [1.; 3]);
    let posed = object("o1", [11., -7., 9.], [30., 45., 60.], [2., 3., 0.5]);
    let saved_png =
        include_bytes!("../../tests/fixtures/formats/lychee_geometry/authored-preview.png");
    let mut cases = Vec::new();
    for (name, id, title, state, preview) in [
        (
            "missing-preview.lys",
            "lychee-stored-model-missing-preview",
            "Lychee stored model with no saved PNG",
            scene(&[identity]),
            false,
        ),
        (
            "asymmetric-placement.lys",
            "lychee-stored-model-asymmetric-placement",
            "Lychee asymmetric model with nonuniform scale and three-axis rotation",
            scene(std::slice::from_ref(&posed)),
            false,
        ),
        (
            "shared-visibility.lys",
            "lychee-stored-model-shared-visibility",
            "Lychee shared source mesh with two visible placements",
            shared_scene(&posed),
            false,
        ),
        (
            "preparation-ignored.lys",
            "lychee-stored-model-preparation-ignored",
            "Lychee original stored model with preparation metadata excluded",
            preparation_scene(&posed),
            true,
        ),
    ] {
        let mut members = vec![
            ("asymmetric-model.bin", mesh()),
            ("scene.bin", scene_bytes(&state)?),
        ];
        if preview {
            // This distracting derived mesh must not replace the exact addressed source model.
            let mut derived = mesh();
            derived[0..4].copy_from_slice(&99_u32.to_le_bytes());
            members.push(("asymmetric-model_hollowing.bin", derived));
            members.push(("preview.png", saved_png.to_vec()));
        }
        fs::write(directory.join(name), container(&members)?)?;
        cases.push(register(
            root,
            id,
            title,
            &format!("{DIR}/{name}"),
            AUTHORED,
        )?);
    }
    for (profile, title) in [("lp", "low-profile"), ("hp", "high-profile")] {
        cases.push(register(
            root,
            &format!("lychee-delphi-{profile}-stored-model"),
            &format!("Lychee 5.4.3 DELPHI {title} keyboard case stored model"),
            &format!("lychee/delphi-edge-case-{profile}.lys"),
            ORIGINAL,
        )?);
    }
    Ok(cases)
}
