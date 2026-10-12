//! Register unchanged, MIT-licensed Lychee keyboard scenes in the thumbnail corpus.

use catwalk3d::Result;
use std::path::Path;

/// Register original Lychee 5.4.3 scenes after qualifying their addressed saved images.
/// Fixture bytes and reviewed PNG baselines remain unchanged by generation.
///
/// # Errors
///
/// Returns an error if an original fixture or its qualified saved PNG is missing,
/// malformed, or has unexpected original dimensions.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    let mut cases = Vec::new();
    for (id, title, source_name, input) in [
        (
            "lychee-delphi-lp-saved-thumbnail",
            "Lychee 5.4.3 DELPHI low-profile keyboard case saved PNG",
            "DELPHI%20-%20Edge%20Case%20LP.lys",
            "lychee/delphi-edge-case-lp.lys",
        ),
        (
            "lychee-delphi-hp-saved-thumbnail",
            "Lychee 5.4.3 DELPHI high-profile keyboard case saved PNG",
            "DELPHI%20Edge%20Case%20HP.lys",
            "lychee/delphi-edge-case-hp.lys",
        ),
    ] {
        let saved = catwalk3d::saved_preview::lychee::extract(&root.join(input))?
            .ok_or("original Lychee corpus scene has no addressed saved preview")?;
        if saved.dimensions() != (512, 512) {
            return Err("original Lychee corpus PNG dimensions changed".into());
        }
        cases.push(crate::fixture(
            id,
            "lys",
            title,
            input,
            "saved-preview",
            &format!("Unmodified JRiggles/DELPHI-kbd cases/{source_name}, commit a666598d5916b73f8a3fac796e569878de18bb79; MIT, Copyright (c) 2022-25 John Riggles; original license retained as lychee/DELPHI-LICENSE. Actual Lychee 5.4.3 scene, flat Mango manifest 3.1.0; exact saved PNG extraction preserves the prepared image and does not regenerate geometry or supports. https://github.com/JRiggles/DELPHI-kbd/blob/a666598d5916b73f8a3fac796e569878de18bb79/cases/{source_name}"),
        ));
    }
    Ok(cases)
}
