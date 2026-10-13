//! Register unchanged `SketchUp` producer documents with independently retained PNGs.

use catwalk3d::Result;
use std::path::Path;

/// Pinned upstream producer corpus revision, separate from Catwalk's MIT code.
const REVISION: &str = "a6788afa6386cfd4bc781998dca99c591b3fb069";

/// Register qualified saved thumbnails without regenerating or rewriting documents.
///
/// # Errors
/// Returns an error if an original source or independent PNG is missing, cannot
/// be decoded, or differs from the reader's exact saved pixels.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    let mut cases = Vec::new();
    for name in [
        "legacy-box-v2013",
        "legacy-box-v2014",
        "legacy-box-v2015",
        "legacy-box-v2016",
        "2017-box",
        "2017-box-two-materials",
        "2026-box",
        "2026-box-two-materials",
    ] {
        let input = format!("sketchup/{name}.skp");
        let actual = catwalk3d::saved_preview::sketchup::extract(&root.join(&input))?
            .ok_or("original SketchUp model thumbnail is absent")?;
        let expected = image::open(root.join(format!("sketchup/{name}.png")))?.into_rgba8();
        if actual.dimensions() != (256, 256) || actual != expected {
            return Err(
                "original SketchUp saved thumbnail differs from independent reference".into(),
            );
        }
        cases.push(crate::fixture(
            &format!("skp-{name}-saved-thumbnail"), "skp", &format!("SketchUp {name} original model thumbnail"),
            &input, "saved-preview",
            &format!("Unchanged hew3d/openskp retail SketchUp producer corpus at {REVISION}; GPL-3.0-only fixture geometry, original license and corpus notice retained in sketchup/. Bundled Trimble content retains its original ownership as described in that notice. Document model-thumbnail PNG extracted independently; source and PNG hashes, version/carrier qualification and limitations in sketchup/README.md. Saved image only; no model geometry rendering. https://github.com/hew3d/openskp/tree/{REVISION}/corpus"),
        ));
    }
    Ok(cases)
}
