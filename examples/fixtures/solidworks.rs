//! Register retained `SolidWorks` producer documents without rewriting their contents.

use crate::digest_hex;
use catwalk3d::{Result, saved_preview};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Original documents and exact source/independent PNG stream identities.
const INPUTS: [(&str, &str, usize, &str, &str); 2] = [
    (
        "solidworks-about.sldprt",
        "about.SLDPRT",
        138_752,
        "08a8ec2afc112a27b1b8f4fad92224080b5c0d8cda8fc095b423cf91ca062e34",
        "cfa22d7410a5b2467aaa4b6beefb1b9eff25b405f9d31c1e78616762b2dc68ec",
    ),
    (
        "solidworks-powerpackage.sldasm",
        "powerpackage.SLDASM",
        323_072,
        "2e71fb43b889a5d9585eb7606266a1dd35ef685244df6ee65782a7f7b2f19788",
        "df67217ac2d1968a2956834f51e8c54527897f5562fa267e2365c4dd0b084f46",
    ),
];

/// Require an unchanged retained asset by SHA-256.
fn verify(path: &Path, expected: &str) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path)?;
    if format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))) != expected {
        return Err(format!(
            "SolidWorks retained source differs from its checksum: {}",
            path.display()
        )
        .into());
    }
    Ok(bytes)
}

/// Validate original documents against independent preview streams before registration.
///
/// # Errors
/// Returns an error for changed or missing producer files, licensing, independent
/// PNG references or mismatched public-extraction pixels.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    let directory = root.join("solidworks");
    let license = std::fs::read_to_string(directory.join("POWER-PACKAGE-LICENSE.txt"))?;
    if !license.contains("Copyright (c) 2022 GuoXiong") || !license.contains("MIT License") {
        return Err("SolidWorks producer source license is missing or changed".into());
    }
    let mut cases = Vec::new();
    for (name, original, length, source_hash, png_hash) in INPUTS {
        let path = directory.join(name);
        if verify(&path, source_hash)?.len() != length {
            return Err(format!("SolidWorks source length differs: {name}").into());
        }
        let reference = verify(&directory.join(format!("{name}.preview.png")), png_hash)?;
        let expected =
            image::load_from_memory_with_format(&reference, image::ImageFormat::Png)?.into_rgba8();
        if saved_preview::solidworks::extract(&path)?.as_ref() != Some(&expected) {
            return Err(format!("SolidWorks independent preview pixels differ: {name}").into());
        }
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .ok_or("SolidWorks fixture has no extension")?;
        let kind = if extension == "sldprt" {
            "part"
        } else {
            "assembly"
        };
        let id = name
            .strip_suffix(&format!(".{extension}"))
            .ok_or("SolidWorks fixture extension mismatch")?;
        cases.push(crate::fixture(
            id,
            extension,
            &format!("SolidWorks native {kind} saved PNG preview"),
            &format!("solidworks/{name}"),
            "saved-preview",
            &format!("Unmodified guoxxiong/Power-Package/{original} at commit bc6b8d0ca0fe472a4f0ed91c9e64f986b3ddc7e6; separately MIT-licensed native SolidWorks {kind} document. Extracts root PreviewPNG, without geometry or assembly companion reads. Original license, exact hashes and reference PNG streams retained in solidworks/."),
        ));
    }
    Ok(cases)
}
