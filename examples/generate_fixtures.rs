//! Regenerate the authored, portable format corpus. PNG baselines are updated
//! separately so fixture regeneration can never silently approve a new image.
#[path = "fixtures/binary_brep.rs"]
/// Register original BREP counterparts and generate native table placement controls.
mod binary_brep;
#[path = "fixtures/collada_primitives.rs"]
/// Generate COLLADA static primitive controls and register licensed conformance models.
mod collada_primitives;
#[path = "../tests/support/digest_hex.rs"]
/// Lowercase checksum formatting shared by retained-source fixture generators.
mod digest_hex;
#[path = "fixtures/freecad.rs"]
/// Register original FreeCAD saved images and native placed BREP geometry.
mod freecad;
#[path = "fixtures/images_points.rs"]
/// Generate portable raster and point-cloud fixture encodings.
mod images_points;
#[path = "fixtures/lychee.rs"]
/// Register original Lychee scenes and their independently qualified saved images.
mod lychee;
#[path = "fixtures/lychee_geometry.rs"]
/// Register original stored Lychee meshes and generate independent placement controls.
mod lychee_geometry;
#[path = "fixtures/mesh_scene.rs"]
/// Generate authored mesh and scene fixtures and register retained reference assets.
mod mesh_scene;
#[path = "fixtures/native_autodesk.rs"]
/// Register unchanged Inventor and Fusion documents for fresh geometry thumbnails.
mod native_autodesk;
#[path = "fixtures/native_geometry.rs"]
/// Register licensed native documents with fresh geometry render qualification.
mod native_geometry;
#[path = "fixtures/obj_appearance.rs"]
/// Generate OBJ appearance controls and register a retained textured exporter model.
mod obj_appearance;
#[path = "fixtures/saved_previews.rs"]
/// Generate qualified saved-thumbnail carrier fixtures.
mod saved_previews;
#[path = "fixtures/scientific_cad.rs"]
/// Generate scientific datasets and CAD boundary representations.
mod scientific_cad;
#[path = "fixtures/vrml_prototypes.rs"]
/// Register original VRML prototypes and generate typed static expansion controls.
mod vrml_prototypes;
#[path = "fixtures/vtk_lines.rs"]
/// Register scientific line cells and generate equivalent mixed unstructured grids.
mod vtk_lines;

#[path = "fixtures/amf.rs"]
/// Register original plain and compressed AMF exporter references.
mod amf;

#[path = "fixtures/tiff_volumes.rs"]
/// Register an original TIFF stack and generate typed scalar volume controls.
mod tiff_volumes;
#[path = "fixtures/usd_nurbs.rs"]
/// Generate authored NURBS layers and register a licensed Maya-exported reference.
mod usd_nurbs;
#[path = "fixtures/vtk_quadratic.rs"]
/// Register retained VTK geometry and generate curved fixed-node cell representations.
mod vtk_quadratic;

use catwalk3d::{Result, formats};
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
    let mut cases = amf::generate(&root)?;
    cases.extend(images_points::generate(&root)?);
    cases.extend(mesh_scene::generate(&root)?);
    cases.extend(scientific_cad::generate(&root)?);
    cases.extend(saved_previews::generate_lightburn(&root)?);
    cases.extend(saved_previews::generate_blend_dwg(&root)?);
    cases.extend(saved_previews::generate_fusion(&root)?);
    cases.extend(saved_previews::generate_inventor(&root)?);
    cases.extend(lychee::generate(&root)?);
    cases.extend(lychee_geometry::generate(&root)?);
    cases.extend(native_geometry::generate(&root)?);
    cases.extend(native_autodesk::generate(&root)?);
    cases.extend(vrml_prototypes::generate(&root)?);
    cases.extend(vtk_lines::generate(&root)?);
    cases.extend(binary_brep::generate(&root)?);
    cases.extend(obj_appearance::generate(&root)?);
    cases.extend(collada_primitives::generate(&root)?);
    cases.extend(vtk_quadratic::generate(&root)?);
    cases.extend(usd_nurbs::generate(&root)?);
    cases.extend(tiff_volumes::generate(&root)?);
    cases.extend(freecad::generate(&root)?);
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
        .chain(catwalk3d::saved_preview::EXTENSIONS)
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
    let native_advertised: BTreeSet<_> = formats::NATIVE_GEOMETRY_EXTENSIONS
        .iter()
        .copied()
        .collect();
    let native_generated: BTreeSet<_> = cases
        .iter()
        .filter(|case| case.kind == "native-geometry")
        .map(|case| case.extension.as_str())
        .collect();
    if native_advertised != native_generated {
        return Err("every native geometry reader needs a fresh-render corpus case".into());
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
