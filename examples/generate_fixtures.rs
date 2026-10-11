//! Regenerate the authored, portable format corpus. PNG baselines are updated
//! separately so fixture regeneration can never silently approve a new image.
#[path = "fixtures/images_points.rs"]
/// Generate portable raster and point-cloud fixture encodings.
mod images_points;
#[path = "fixtures/mesh_scene.rs"]
/// Generate authored mesh and scene fixtures and register retained reference assets.
mod mesh_scene;
#[path = "fixtures/native_geometry.rs"]
/// Register licensed native documents with fresh geometry render qualification.
mod native_geometry;
#[path = "fixtures/saved_previews.rs"]
/// Generate qualified saved-thumbnail carrier fixtures.
mod saved_previews;
#[path = "fixtures/scientific_cad.rs"]
/// Generate scientific datasets and CAD boundary representations.
mod scientific_cad;

use catwalk::{Result, formats};
use serde_json::json;
use std::{collections::BTreeSet, path::Path};

/// One corpus manifest entry describing an input and its provenance.
pub struct Fixture {
    /// Stable identifier used for image filenames and manifest lookup.
    pub id: String,
    /// Lowercase input extension covered by this fixture.
    pub extension: String,
    /// Human-readable description shown in the example gallery.
    pub title: String,
    /// Input path relative to the format corpus directory.
    pub input: String,
    /// Manifest category: model, image, saved-preview or native-geometry.
    pub kind: String,
    /// Source, authorship and licensing description for the input.
    pub provenance: String,
}

/// Construct a manifest entry from its identifier, format and provenance strings.
#[must_use]
pub fn fixture(
    id: &str,
    extension: &str,
    title: &str,
    input: &str,
    kind: &str,
    provenance: &str,
) -> Fixture {
    Fixture {
        id: id.into(),
        extension: extension.into(),
        title: title.into(),
        input: input.into(),
        kind: kind.into(),
        provenance: provenance.into(),
    }
}

/// Generate authored inputs and write a manifest after validating complete advertised extension coverage.
fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formats");
    std::fs::create_dir_all(&root)?;
    let mut cases = images_points::generate(&root)?;
    cases.extend(mesh_scene::generate(&root)?);
    cases.extend(scientific_cad::generate(&root)?);
    cases.extend(saved_previews::generate_lightburn(&root)?);
    cases.extend(saved_previews::generate_blend_dwg(&root)?);
    cases.extend(saved_previews::generate_fusion(&root)?);
    cases.extend(saved_previews::generate_inventor(&root)?);
    cases.extend(native_geometry::generate(&root)?);
    cases.sort_by(|a, b| a.extension.cmp(&b.extension).then(a.id.cmp(&b.id)));
    let mut ids = BTreeSet::new();
    for case in &cases {
        if !ids.insert(&case.id) {
            return Err(format!("duplicate fixture ID: {}", case.id).into());
        }
        if !root.join(&case.input).is_file() {
            return Err(format!("missing generated fixture: {}", case.input).into());
        }
        if formats::extension(&root.join(&case.input))? != case.extension {
            return Err(format!("fixture extension mismatch: {}", case.id).into());
        }
    }
    let advertised: BTreeSet<_> = formats::MODEL_EXTENSIONS
        .iter()
        .chain(formats::IMAGE_EXTENSIONS)
        .chain(catwalk::saved_preview::EXTENSIONS)
        .copied()
        .collect();
    let generated: BTreeSet<_> = cases.iter().map(|case| case.extension.as_str()).collect();
    if advertised != generated {
        return Err(format!(
            "fixture coverage mismatch; missing {:?}, unexpected {:?}",
            advertised.difference(&generated).collect::<Vec<_>>(),
            generated.difference(&advertised).collect::<Vec<_>>()
        )
        .into());
    }
    let count = cases.len();
    let cases: Vec<_> = cases.into_iter().map(|case| json!({"id":case.id,"extension":case.extension,"title":case.title,"input":case.input,"kind":case.kind,"provenance":case.provenance})).collect();
    let manifest = json!({"schema":1,"render":{"width":256,"height":256,"transparent":true,"samples":2},"cases":cases});
    let mut bytes = serde_json::to_vec_pretty(&manifest)?;
    bytes.push(b'\n');
    std::fs::write(root.join("manifest.json"), bytes)?;
    println!(
        "Generated {count} fixtures covering all {} extensions",
        advertised.len()
    );
    Ok(())
}
