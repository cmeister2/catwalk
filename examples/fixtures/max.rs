//! Register unchanged Autodesk 3ds Max producer scenes with exact saved-pixel checks.

use catwalk3d::Result;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Exact pinned revision of Autodesk's MIT-licensed MCG producer examples.
const REVISION: &str = "820d8a32badf916849282081ca42416a88d2f6b7";

/// Register retained source documents without regenerating or resaving them.
///
/// # Errors
/// Returns an error if retained files differ from their pinned source or license
/// hashes, or extraction fails to preserve their independently decoded pixels.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    let license = std::fs::read(root.join("max_saved/LICENSE-Autodesk.txt"))?;
    if hash(&license) != "55815a27053bafdefdb8b14fe79536f3af21f1b087fe3352f3aa6eeb6212ab74" {
        return Err("retained Autodesk MAX license differs from its original bytes".into());
    }
    let mut cases = Vec::new();
    for (name, id, source_hash, pixel_hash, height) in [
        (
            "Combine Test.max",
            "combine",
            "4c065fedde00ed23f884b959448bb9d69725b5e0aaf667746639de91cd74f68c",
            "c06792b6a85f99a7eaf3991cacd2a37fe75833678b67f02b874ffc4b3580ab91",
            256,
        ),
        (
            "Spherify Test.max",
            "spherify",
            "e2d8d94e20a9c182bb38ac03426bfbf6608aafa9b3f2dcbcc033a47d669ee823",
            "43f61b0fdefb6cfb3a5b682363ab032a6cb7b8c840d08d305563309483610023",
            268,
        ),
    ] {
        let input = format!("max_saved/{name}");
        if hash(&std::fs::read(root.join(&input))?) != source_hash {
            return Err(
                format!("retained Autodesk scene differs from its original bytes: {name}").into(),
            );
        }
        let actual = catwalk3d::saved_preview::max::extract(&root.join(&input))?
            .ok_or("original Autodesk MAX thumbnail is absent")?;
        if actual.dimensions() != (512, height) || hash(actual.as_raw()) != pixel_hash {
            return Err(format!("original Autodesk MAX saved pixels differ: {name}").into());
        }
        let source_path = format!("Scenes/Tests/{name}");
        let url = format!(
            "https://github.com/ADN-DevTech/3dsMax-MCG-Samples/blob/{REVISION}/Scenes/Tests/{}",
            name.replace(' ', "%20")
        );
        cases.push(crate::fixture(
            &format!("max-autodesk-{id}"), "max", &format!("3ds Max saved thumbnail: {}", name.trim_end_matches(".max")),
            &input, "saved-preview",
            &format!("Unmodified Autodesk ADN-DevTech/3dsMax-MCG-Samples {source_path} at commit {REVISION}; MIT, original license retained as max_saved/LICENSE-Autodesk.txt. Exact source URLs, Git blobs, SHA-256 hashes and independently decoded saved-pixel hashes are recorded in max_saved/provenance.json. Root SummaryInformation PID17 CF_METAFILEPICT thumbnail is extracted without reading scene geometry. {url}"),
        ));
    }
    Ok(cases)
}

/// Format an exact SHA-256 checksum using the existing corpus hexadecimal protocol.
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", crate::digest_hex::Hex(Sha256::digest(bytes)))
}
