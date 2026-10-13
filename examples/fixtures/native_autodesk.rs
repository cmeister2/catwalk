//! Register unchanged Autodesk documents for qualified fresh geometry renders.

use crate::digest_hex;
use catwalk3d::Result;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Gallery directory retaining the native Autodesk documents and their notices.
const DIR: &str = "autodesk_geometry";
/// Original conveyor documents, exact lengths and pinned source checksums.
const INVENTOR_INPUTS: [(&str, usize, &str); 5] = [
    (
        "box.ipt",
        75_264,
        "59829490a42a4391027f42ef8b0ba321ba8fa023a011b06c646fdedee41f1965",
    ),
    (
        "part.ipt",
        92_160,
        "72aee9aa6f6f40f926ef4b043ffabd38767a5f14e48cb20fea57f2a20d4aa427",
    ),
    (
        "rail.ipt",
        238_592,
        "83cc80e7be6475e0ea25dead62e184234e03e1b1793c02400deb1c6b033b2a46",
    ),
    (
        "support.ipt",
        144_384,
        "586f196475c215347b4002106c11a790bfd8ebc352f7525746217aadb9b3cdf7",
    ),
    (
        "conveyor_model.iam",
        66_048,
        "a2db0b7a6cda924b1e01ec182b663071242003576cdd3d2085779f8ab768f2a3",
    ),
];
/// Qualified native Fusion documents from Samuel Oladosu's MIT-licensed designs.
const FUSION_INPUTS: [(&str, usize, &str, &str); 2] = [
    (
        "toy-block",
        104_086,
        "7fb14c89f0f2b04657fbdf178f6b2cf9fd902f905fd1687263280ca629e1502f",
        "Fusion original eight-stud toy block geometry",
    ),
    (
        "hex-nut",
        96_965,
        "75a1b10b39bc635e3feb61cf571651a7e98ba0f54f664e2d7e4f26be7e4b371b",
        "Fusion original hex nut with cylindrical through-bore",
    ),
];
/// Describe the exact conveyor source, licensing and geometry selection.
const INVENTOR_PROVENANCE: &str = "Unmodified roshbeng/opcua_fusion_test_servers_and_hardware native Inventor documents at commit d9ab21ced0a26e09a213a218249ac1938340bae6; MIT, original license retained as autodesk_geometry/inventor/LICENSE-roshbeng.txt. Byte-identical corpus copies of tests/fixtures/native_geometry/inventor/conveyor originals; exact source URLs, Git blobs and SHA-256 hashes are retained in autodesk_geometry/inventor/provenance.json. Fresh native solved-body geometry and saved occurrence placements are rendered; stored thumbnails are bypassed. The assembly retains its four original sibling IPT files. Independent author-exported STEP bounds and volume are qualified in tests/inventor_geometry.rs. https://github.com/roshbeng/opcua_fusion_test_servers_and_hardware/tree/d9ab21ced0a26e09a213a218249ac1938340bae6/CAD%20models/conveyor_model/conveyor_model_v1";

/// Read a retained source and require its exact pinned length and SHA-256.
fn verified_bytes(path: &Path, length: usize, expected: &str) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path)?;
    if bytes.len() != length || format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))) != expected
    {
        return Err(format!(
            "native Autodesk fixture differs from its source: {}",
            path.display()
        )
        .into());
    }
    Ok(bytes)
}

/// Copy the exact conveyor project and preserve its complete per-file attribution.
fn retain_inventor(root: &Path) -> Result<()> {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native_geometry/inventor");
    let destination = root.join(DIR).join("inventor");
    std::fs::create_dir_all(&destination)?;
    for (name, length, hash) in INVENTOR_INPUTS {
        let bytes = verified_bytes(&source.join("conveyor").join(name), length, hash)?;
        std::fs::write(destination.join(name), bytes)?;
    }
    let license = verified_bytes(
        &source.join("LICENSE-roshbeng.txt"),
        1_065,
        "6144fd8afc4d2c905690a82f0ab5d0e9a98104f5b357c07fd7907865ed224e65",
    )?;
    std::fs::write(destination.join("LICENSE-roshbeng.txt"), license)?;
    let source_metadata: Vec<Value> =
        serde_json::from_slice(&std::fs::read(source.join("provenance.json"))?)?;
    let mut metadata = Vec::new();
    for (name, length, hash) in INVENTOR_INPUTS {
        let original = format!("conveyor/{name}");
        let entries: Vec<_> = source_metadata
            .iter()
            .filter(|entry| entry["file"] == original)
            .collect();
        let [entry] = entries.as_slice() else {
            return Err(format!("missing or duplicate Inventor provenance: {name}").into());
        };
        if entry["bytes"] != length || entry["sha256"] != hash {
            return Err(
                format!("Inventor provenance differs from its pinned source: {name}").into(),
            );
        }
        let mut entry = (**entry).clone();
        entry["file"] = Value::String(name.into());
        metadata.push(entry);
    }
    let mut bytes = serde_json::to_vec_pretty(&metadata)?;
    bytes.push(b'\n');
    std::fs::write(destination.join("provenance.json"), bytes)?;
    Ok(())
}

/// Register retained originals, with native geometry instead of their saved images.
///
/// # Errors
/// Returns an error when an original or its source attribution is missing, has
/// changed bytes, or cannot be copied into the portable assembly project.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    retain_inventor(root)?;
    let mut cases = Vec::new();
    for (id, extension, title, name) in [
        (
            "inventor-geometry-box",
            "ipt",
            "Inventor original open box part geometry",
            "box.ipt",
        ),
        (
            "inventor-geometry-conveyor",
            "iam",
            "Inventor original four-part conveyor assembly geometry",
            "conveyor_model.iam",
        ),
    ] {
        cases.push(crate::fixture(
            id,
            extension,
            title,
            &format!("{DIR}/inventor/{name}"),
            "native-geometry",
            INVENTOR_PROVENANCE,
        ));
    }
    verified_bytes(
        &root.join(DIR).join("FUSION-SAMPLES-README.md"),
        2_680,
        "d106743a92bed1ecd6d2d07f3d1610883cdedff000ecbbf0d6522c4f07cf1d45",
    )?;
    for (name, length, hash, title) in FUSION_INPUTS {
        let input = format!("{DIR}/fusion-{name}.f3d");
        verified_bytes(&root.join(&input), length, hash)?;
        let notice =
            std::fs::read_to_string(root.join(DIR).join(format!("fusion-{name}-NOTICE.txt")))?;
        if !notice.contains(hash) {
            return Err(format!("Fusion source notice omits its pinned checksum: {name}").into());
        }
        let details = if name == "hex-nut" {
            " Cosmetic thread marks are not reconstructed as helical surfaces."
        } else {
            ""
        };
        cases.push(crate::fixture(
            &format!("fusion-geometry-{name}"),
            "f3d",
            title,
            &input,
            "native-geometry",
            &format!("Unmodified Samuel Oladosu fusion360-sample-modeling native Fusion design at commit 2d19f73689ac133e1f9abc5d6780681268d21808; SHA-256 {hash}. MIT licensing and original source attribution are retained in autodesk_geometry/FUSION-SAMPLES-README.md and fusion-{name}-NOTICE.txt. Fresh native solved B-rep geometry is rendered; saved PNGs are bypassed.{details} https://github.com/samueloladosu37/fusion360-sample-modeling/tree/2d19f73689ac133e1f9abc5d6780681268d21808"),
        ));
    }
    Ok(cases)
}
