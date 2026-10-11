//! Keep the separately qualified native documents in the shared image corpus.
use catwalk3d::Result;
use std::path::Path;

/// Copy retained source documents and their licenses, excluding reference image outputs.
fn copy_sources(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if entry.file_name() != "snapshots" {
                copy_sources(&path, &destination.join(entry.file_name()))?;
            }
        } else if path.extension().and_then(|s| s.to_str()) != Some("png") {
            std::fs::copy(&path, destination.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// Copy qualified native input documents and register fresh-render corpus entries.
///
/// # Errors
/// Returns an error when source documents cannot be enumerated or copied, output directories
/// cannot be created, or a registered document has no UTF-8 filename or extension.
#[allow(
    clippy::too_many_lines,
    reason = "The fixed document list keeps each licensed native input beside its gallery description"
)]
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    let sources = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native_geometry");
    let output = root.join("native_geometry");
    let mut cases = Vec::new();
    for family in ["blend", "lightburn", "dwg"] {
        copy_sources(&sources.join(family), &output.join(family))?;
    }
    let documents = [
        (
            "blend",
            "default-252-no-thumbnail.blend",
            "Blender 2.52 default mesh, originally saved without an image",
        ),
        (
            "blend",
            "cube-278-no-thumbnail.blend",
            "Blender 2.78 mesh, thumbnail block removed",
        ),
        (
            "blend",
            "shared-278-no-thumbnail.blend",
            "Blender 2.78 shared meshes and placements, thumbnail block removed",
        ),
        (
            "blend",
            "mesh-280-no-thumbnail.blend",
            "Blender 2.80 mesh attributes, thumbnail block removed",
        ),
        (
            "blend",
            "loose-291-no-thumbnail.blend",
            "Blender 2.91 loose edges and points, thumbnail block removed",
        ),
        (
            "blend",
            "hierarchy-502-no-thumbnail.blend",
            "Blender 5.2 hierarchy and collection instances, thumbnail block removed",
        ),
        (
            "lightburn",
            "classic-geometry.lbrn",
            "LightBurn classic Beziers and transformed groups, no thumbnail",
        ),
        (
            "lightburn",
            "line.lbrn2",
            "LightBurn native affine line, no thumbnail",
        ),
        (
            "lightburn",
            "square.lbrn2",
            "LightBurn native rectangle, no thumbnail",
        ),
        (
            "lightburn",
            "circle.lbrn2",
            "LightBurn native ellipse, no thumbnail",
        ),
        (
            "lightburn",
            "rings.lbrn2",
            "LightBurn shared curved paths and groups, no thumbnail",
        ),
        (
            "lightburn",
            "crucifix.lbrn2",
            "LightBurn four shared curved instances, no thumbnail",
        ),
        (
            "lightburn",
            "butterfly_vectorized.lbrn2",
            "LightBurn vectorized butterfly, no thumbnail",
        ),
        (
            "lightburn",
            "board-with-soldermask-only.lbrn2",
            "LightBurn generated PCB outlines, no thumbnail",
        ),
        (
            "lightburn",
            "repro04-motor-controller.lbrn2",
            "LightBurn 662-path PCB, no thumbnail",
        ),
        ("dwg", "line-2000.dwg", "AutoCAD 2000 native line records"),
        ("dwg", "line-2004.dwg", "AutoCAD 2004 native line records"),
        ("dwg", "line-2007.dwg", "AutoCAD 2007 native line records"),
        ("dwg", "line-2013.dwg", "AutoCAD 2013 native line records"),
        (
            "dwg",
            "line-2018.dwg",
            "AutoCAD 2018 native line, originally saved without an image",
        ),
    ];
    for (family, file, title) in documents {
        let input = format!("native_geometry/{family}/{file}");
        let path = root.join(&input);
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .ok_or("missing native extension")?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("missing native fixture name")?;
        let id = format!("{family}-geometry-{}", stem.replace('_', "-"));
        let metadata = if family == "blend" {
            "manifest.json"
        } else {
            "provenance.json"
        };
        cases.push(crate::fixture(
            &id,
            extension,
            title,
            &input,
            "native-geometry",
            &format!("Fresh geometry render; no saved image is consulted. Licensed fixture provenance and any thumbnail-removal derivation: tests/fixtures/native_geometry/{family}/README.md and {metadata}. Original license texts are retained alongside the inputs."),
        ));
    }
    Ok(cases)
}
