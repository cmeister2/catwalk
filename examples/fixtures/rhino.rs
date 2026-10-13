//! Register unchanged openNURBS documents and independently qualified saved viewport pixels.

use catwalk3d::{Result, saved_preview};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

/// Exact upstream SDK revision used for every retained producer archive.
const REVISION: &str = "eb92af3ba1806b0a34a99aba0d3bda83e3d46083";
/// Stable saved-image corpus identities, original filenames and gallery titles.
const CASES: [(&str, &str, &str); 6] = [
    (
        "rhino-v3-deform-spheres-saved-thumbnail",
        "v3_DeformSpheres.3dm",
        "Rhino 3 deformable spheres saved viewport",
    ),
    (
        "rhino-v4-mechanical-part-saved-thumbnail",
        "v4_MechPartB.3dm",
        "Rhino 4 mechanical part saved viewport",
    ),
    (
        "rhino-v5-extrusions-saved-thumbnail",
        "v5_extrusions.3dm",
        "Rhino 5 extrusions saved viewport",
    ),
    (
        "rhino-v6-annotations-saved-thumbnail",
        "V6_annotation.3dm",
        "Rhino 6 annotations saved viewport",
    ),
    (
        "rhino-v7-logo-annotations-saved-thumbnail",
        "v7_rhino_logo_annotations.3dm",
        "Rhino 7 logo and annotations saved viewport",
    ),
    (
        "rhino-v8-caps-joins-saved-thumbnail",
        "v8_caps_and_joins.3dm",
        "Rhino 8 capped and joined surfaces saved viewport",
    ),
];

/// Validate every producer hash and saved-pixel reference before registering corpus cases.
///
/// # Errors
/// Returns an error for changed or missing originals, mismatching saved pixels,
/// incomplete provenance or loss of the independently qualified absence controls.
pub fn generate(root: &Path) -> Result<Vec<super::Fixture>> {
    let directory = root.join("rhino");
    let manifest: Value = serde_json::from_slice(&fs::read(directory.join("sources.json"))?)?;
    if manifest["commit"] != REVISION {
        return Err("Rhino fixture source revision differs from its pinned provenance".into());
    }
    let sources = manifest["files"]
        .as_array()
        .ok_or("missing Rhino fixture sources")?;
    let mut cases = Vec::new();
    for source in sources {
        let name = source["file"]
            .as_str()
            .ok_or("missing Rhino source filename")?;
        let path = directory.join(name);
        let bytes = fs::read(&path)?;
        let expected_hash = source["sha256"]
            .as_str()
            .ok_or("missing Rhino source checksum")?;
        if format!("{:x}", super::digest_hex::Hex(Sha256::digest(&bytes))) != expected_hash {
            return Err(format!("retained Rhino source changed: {name}").into());
        }
        let actual = saved_preview::rhino::extract(&path)?;
        if let Some(reference) = source["preview"].as_str() {
            let actual = actual.ok_or("qualified Rhino document lost its saved preview")?;
            let expected = image::open(directory.join(reference))?.into_rgba8();
            if actual != expected {
                return Err(format!(
                    "Rhino saved pixels differ from their independent reference: {name}"
                )
                .into());
            }
        } else if actual.is_some() {
            return Err(format!("Rhino absence control acquired a preview: {name}").into());
        }
        if let Some(&(id, _, title)) = CASES.iter().find(|&&(_, file, _)| file == name) {
            let upstream = source["upstream_path"]
                .as_str()
                .ok_or("missing Rhino upstream source path")?;
            let provenance = format!(
                "Unmodified McNeel openNURBS SDK sample {upstream} at commit {REVISION}; permissive openNURBS license and official redistribution grant retained as rhino/LICENSE-openNURBS*.txt. Exact source/pixel hashes and independent preview extraction in rhino/sources.json and README.md. Saved viewport pixels only; no geometry decoding."
            );
            cases.push(super::fixture(
                id,
                "3dm",
                title,
                &format!("rhino/{name}"),
                "saved-preview",
                &provenance,
            ));
        }
    }
    if cases.len() != CASES.len() {
        return Err("Rhino corpus registration is missing a retained producer source".into());
    }
    Ok(cases)
}
