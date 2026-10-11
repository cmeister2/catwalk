//! Register byte-identical, separately licensed AMF exporter references.
use catwalk3d::Result;
use std::path::Path;

/// Register plain and compressed original Slic3r AMF reference assets.
///
/// # Errors
/// Returns an error when a retained reference is missing or cannot load into
/// its expected nonempty triangle scene.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    let mut cases = Vec::new();
    for (id, title, source) in [
        (
            "amf-slic3r-plain",
            "Slic3r original AMF XML export",
            "20mmbox.amf",
        ),
        (
            "amf-slic3r-deflated",
            "Slic3r compressed AMF reference",
            "20mmbox_deflated.amf",
        ),
    ] {
        let input = format!("amf/{id}.amf");
        let scene = catwalk3d::formats::load(&root.join(&input))?;
        if scene.triangles.len() != 12 {
            return Err("retained AMF reference must contain twelve placed triangles".into());
        }
        cases.push(crate::fixture(id, "amf", title, &input, "model", &format!("Unchanged official Slic3r reference {source} at commit 026c1380e0ad12176dd2fc8cdf8f6f181deeb071; XML metadata Slic3r 1.3.1-dev; AGPL-3.0, full original license and source notice in amf/SLIC3R-LICENSE and amf/SLIC3R-NOTICE.txt. See amf/README.md for exact source hashes and independent geometry qualification. The ZIP reference wraps the same exported XML; no slicer or converter is required at runtime.")));
    }
    Ok(cases)
}
