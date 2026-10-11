//! Regenerate the authored, portable format corpus. PNG baselines are updated
//! separately so fixture regeneration can never silently approve a new image.
#[path = "fixtures/images_points.rs"]
mod images_points;
#[path = "fixtures/mesh_scene.rs"]
mod mesh_scene;
#[path = "fixtures/saved_previews.rs"]
mod saved_previews;
#[path = "fixtures/scientific_cad.rs"]
mod scientific_cad;

use catwalk::{Result, formats};
use serde_json::json;
use std::{collections::BTreeSet, path::Path};

pub struct Fixture {
    pub id: String,
    pub extension: String,
    pub title: String,
    pub input: String,
    pub kind: String,
    pub provenance: String,
}

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
