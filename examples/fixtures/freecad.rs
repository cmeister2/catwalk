//! Register original licensed FreeCAD documents for saved and fresh previews.

use catwalk3d::Result;
use std::path::Path;

/// Pinned original F3D `FreeCAD` fixture revision.
const REVISION: &str = "efb898d0943b4d73b8ef707fb333292493151ef8";

/// Original native documents with separately qualified fresh geometry previews.
const GEOMETRY_CASES: [(&str, &str); 17] = [
    (
        "colored_visibility",
        "FreeCAD original visible colored solid",
    ),
    ("legacy_visibility", "FreeCAD original legacy visibility"),
    ("part_container", "FreeCAD original rotated part container"),
    ("link", "FreeCAD original local link placement"),
    ("link_transform", "FreeCAD original composed link transform"),
    ("link_array", "FreeCAD original placed model array"),
    (
        "link_array_transform",
        "FreeCAD original rotated model array",
    ),
    ("headless", "FreeCAD original headless document geometry"),
    (
        "partdesign_body",
        "FreeCAD original body result and side solid",
    ),
    (
        "hidden_container",
        "FreeCAD original hidden container selection",
    ),
    (
        "arch_floor",
        "FreeCAD original nested floor without duplicate solids",
    ),
    ("face_colors", "FreeCAD original native face colors"),
    ("transparency", "FreeCAD original transparent materials"),
    (
        "bracket_1x",
        "FreeCAD 1.1 original bracket and tube materials",
    ),
    (
        "draft_wireframe",
        "FreeCAD original draft wireframe geometry",
    ),
    (
        "gui_without_view_providers",
        "FreeCAD original document without GUI providers",
    ),
    (
        "no_version",
        "FreeCAD original colors without producer version metadata",
    ),
];

/// Original saved PNG carriers and their independently decoded dimensions.
const SAVED_CASES: [(&str, &str, (u32, u32)); 2] = [
    (
        "colored_visibility",
        "FreeCAD original saved scene PNG",
        (128, 128),
    ),
    (
        "bracket_1x",
        "FreeCAD 1.1 original saved bracket PNG",
        (256, 256),
    ),
];

/// Register retained native geometry and saved PNGs without rewriting their bytes.
///
/// # Errors
/// Returns an error when a retained document is absent, its qualified geometry
/// cannot be loaded, or its addressed saved PNG is missing or has changed dimensions.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    let mut cases = Vec::new();
    for (name, title) in GEOMETRY_CASES {
        let input = format!("freecad/{name}.FCStd");
        let scene = catwalk3d::formats::load(&root.join(&input))?;
        if scene.positions().next().is_none() {
            return Err(format!("original FreeCAD fixture {name} has no visible geometry").into());
        }
        cases.push(crate::fixture(
            &format!("fcstd-{name}-geometry").replace('_', "-"),
            "fcstd",
            title,
            &input,
            "native-geometry",
            &provenance(name, "Fresh geometry uses the original addressed ASCII BREP shapes and native document placements; saved images are bypassed."),
        ));
    }
    for (name, title, dimensions) in SAVED_CASES {
        let input = format!("freecad/{name}.FCStd");
        let saved = catwalk3d::saved_preview::freecad::extract(&root.join(&input))?
            .ok_or("original FreeCAD corpus document has no addressed saved PNG")?;
        if saved.dimensions() != dimensions {
            return Err("original FreeCAD saved PNG dimensions changed".into());
        }
        cases.push(crate::fixture(
            &format!("fcstd-{name}-saved-preview").replace('_', "-"),
            "fcstd",
            title,
            &input,
            "saved-preview",
            &provenance(name, "The exact addressed thumbnails/Thumbnail.png is fitted to the preview canvas; a stored image may differ from current geometry."),
        ));
    }
    Ok(cases)
}

/// Describe the pinned original input and its separately retained BSD license.
fn provenance(name: &str, behavior: &str) -> String {
    format!(
        "Unmodified original f3d-app/f3d testing/data/{name}.FCStd at commit {REVISION}; BSD-3-Clause, original license and dataset mapping retained as freecad/LICENSE.md and freecad/DATA_LICENSES.md. Original FreeCAD document reference; upstream includes modified protocol controls as well as application exports. Exact source hashes and selection details are in freecad/README.md. {behavior} https://github.com/f3d-app/f3d/blob/{REVISION}/testing/data/{name}.FCStd"
    )
}
