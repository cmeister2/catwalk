//! Authored static COLLADA controls and unchanged Khronos conformance resources.

use catwalk3d::Result;
use std::{fs, path::Path};

/// Corpus subdirectory containing these static geometry fixtures.
const DIR: &str = "collada_primitives";
/// Authorship and license notice for generated geometry controls.
const AUTHORED: &str =
    "Author-created deterministic Rust fixture; MIT, matching the project license";
/// Pinned upstream conformance provenance, distinct from a newly created exporter asset.
const KHRONOS: &str = "Unmodified KhronosGroup/COLLADA-CTS XML conformance asset at commit 61f2a560cbb2a06ee62da8025241f6b08d06bfd9. Original Maya/ColladaMaya/FCollada metadata and Khronos 2012 permission notice are preserved; full original license retained as collada_primitives/KHRONOS-CTS-LICENSE. Independent source geometry expectations and exact hashes in collada_primitives/README.md. https://github.com/KhronosGroup/COLLADA-CTS/tree/61f2a560cbb2a06ee62da8025241f6b08d06bfd9/StandardDataSets/collada/library_visual_scenes/visual_scene/node";

/// Closed tetrahedron topology shared by the two authored affine-transform controls.
const TETRA_FACES: &str = "<triangles count=\"4\"><input semantic=\"VERTEX\" source=\"#vertices\" offset=\"0\"/><p>0 2 1 0 1 3 1 2 3 2 0 3</p></triangles>";
/// Conventional ordered position parameters for the plain geometry controls.
const XYZ: &str = "<param name=\"X\" type=\"float\"/><param name=\"Y\" type=\"float\"/><param name=\"Z\" type=\"float\"/>";

/// Assemble a complete static document with deterministic asset timestamps.
fn document(mesh: &str, nodes: &str, unit: &str, axis: &str) -> String {
    format!(
        "<COLLADA xmlns=\"http://www.collada.org/2005/11/COLLADASchema\" version=\"1.4.1\"><asset><contributor><author>Catwalk authored control; MIT</author></contributor><created>2000-01-01T00:00:00Z</created><modified>2000-01-01T00:00:00Z</modified><unit meter=\"{unit}\"/><up_axis>{axis}</up_axis></asset><library_geometries><geometry id=\"shape\"><mesh>{mesh}</mesh></geometry></library_geometries><library_visual_scenes><visual_scene id=\"scene\">{nodes}</visual_scene></library_visual_scenes><scene><instance_visual_scene url=\"#scene\"/></scene></COLLADA>\n"
    )
}

/// Encode the named-slot accessor separately from its scalar array and vertex binding.
fn positions(values: &str, count: usize, stride: usize, offset: usize, parameters: &str) -> String {
    let scalars = values.split_whitespace().count();
    format!(
        "<source id=\"positions\"><float_array id=\"position-array\" count=\"{scalars}\">{values}</float_array><technique_common><accessor source=\"#position-array\" count=\"{count}\" stride=\"{stride}\" offset=\"{offset}\">{parameters}</accessor></technique_common></source><vertices id=\"vertices\"><input semantic=\"POSITION\" source=\"#positions\"/></vertices>"
    )
}

/// Build a tilted rectangle with two disjoint clockwise hole rings.
fn holed_plane() -> String {
    let source = positions(
        "0 0 0 4 0 1 4 3 1.375 0 3 .375 .5 .5 .1875 .5 1.5 .3125 1.5 1.5 .5625 1.5 .5 .4375 2.5 1 .75 2.5 2 .875 3 2 1 3 1 .875",
        12,
        3,
        0,
        XYZ,
    );
    let polygon = "<polygons count=\"1\"><input semantic=\"VERTEX\" source=\"#vertices\" offset=\"0\"/><ph><p>0 1 2 3</p><h>4 5 6 7</h><h>8 9 10 11</h></ph></polygons>";
    document(
        &format!("{source}{polygon}"),
        "<node><instance_geometry url=\"#shape\"/></node>",
        "1",
        "Z_UP",
    )
}

/// Build independent indexed colored line pairs and a closed strip with two placements.
fn wire_instances() -> String {
    let source = positions("0 0 0 2 0 0 2 1 0 0 1 0 1 .5 2", 5, 3, 0, XYZ);
    let colors = "<source id=\"colors\"><float_array id=\"color-array\" count=\"15\">1 0 0 0 1 0 0 0 1 1 1 0 1 0 1</float_array><technique_common><accessor source=\"#color-array\" count=\"5\" stride=\"3\"><param name=\"first\" type=\"float\"/><param name=\"second\" type=\"float\"/><param name=\"third\" type=\"float\"/></accessor></technique_common></source>";
    let inputs = "<input semantic=\"VERTEX\" source=\"#vertices\" offset=\"0\"/><input semantic=\"COLOR\" source=\"#colors\" offset=\"1\" set=\"4\"/>";
    let lines = format!(
        "<lines count=\"8\">{inputs}<p>0 0 1 1 1 1 2 2 2 2 3 3 3 3 0 0 0 0 4 4 1 1 4 4 2 2 4 4 3 3 4 4</p></lines>"
    );
    let strip = format!("<linestrips count=\"1\">{inputs}<p>0 0 1 1 2 2 3 3 0 0</p></linestrips>");
    document(
        &format!("{colors}{source}{lines}{strip}"),
        "<node id=\"first\"><instance_geometry url=\"#shape\"/></node><node id=\"second\"><translate>4 1 .5</translate><scale>-1 2 1</scale><instance_geometry url=\"#shape\"/></node>",
        ".1",
        "Z_UP",
    )
}

/// Build a placed look-at tetrahedron using offset, unnamed and arbitrarily named slots.
fn lookat_placement() -> String {
    let source = positions(
        "77 -88 9 0 0 0 9 2 0 0 9 0 3 0 9 0 0 1",
        4,
        4,
        2,
        "<param type=\"float\"/><param name=\"north\" type=\"float\"/><param name=\"height\" type=\"float\"/><param name=\"east\" type=\"float\"/>",
    );
    document(
        &format!("{source}{TETRA_FACES}"),
        "<node><translate>4 -2 1</translate><node><lookat>1 2 3 2 2 3 1 1 0</lookat><scale>2 3 4</scale><instance_geometry url=\"#shape\"/></node></node>",
        ".001",
        "Y_UP",
    )
}

/// Build a nonorthogonal angular shear whose independently known result is x'=x+y.
fn nonorthogonal_skew() -> String {
    let source = positions("0 0 0 2 0 0 0 3 0 0 0 1", 4, 3, 0, XYZ);
    document(
        &format!("{source}{TETRA_FACES}"),
        "<node><skew>18.43494882292201 1 1 0 1 0 0</skew><instance_geometry url=\"#shape\"/></node>",
        "1",
        "Z_UP",
    )
}

/// Validate nonempty surface or line geometry and construct its gallery entry.
fn register(
    root: &Path,
    name: &str,
    id: &str,
    title: &str,
    provenance: &str,
) -> Result<crate::Fixture> {
    let input = format!("{DIR}/{name}");
    let scene = catwalk3d::formats::load(&root.join(&input))?;
    if scene.triangles.is_empty() && scene.lines.is_empty() {
        return Err(format!("COLLADA primitive fixture {input} has no geometry").into());
    }
    Ok(crate::fixture(
        id, "dae", title, &input, "model", provenance,
    ))
}

/// Write static primitive controls and copy the two unchanged licensed CTS models.
///
/// # Errors
/// Returns errors for failed filesystem writes, malformed model geometry,
/// unsupported reader behavior or any registered model with no surfaces or lines.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    let directory = root.join(DIR);
    fs::create_dir_all(&directory)?;
    for (name, bytes) in [
        (
            "node_lookat4.dae",
            include_bytes!("../../tests/fixtures/formats/collada_primitives/node_lookat4.dae")
                .as_slice(),
        ),
        (
            "node_skew_cube.dae",
            include_bytes!("../../tests/fixtures/formats/collada_primitives/node_skew_cube.dae")
                .as_slice(),
        ),
        (
            "KHRONOS-CTS-LICENSE",
            include_bytes!("../../tests/fixtures/formats/collada_primitives/KHRONOS-CTS-LICENSE")
                .as_slice(),
        ),
    ] {
        fs::write(directory.join(name), bytes)?;
    }
    let mut cases = Vec::new();
    for (name, id, title) in [
        (
            "node_lookat4.dae",
            "dae-cts-lookat4",
            "Khronos original look-at pyramid conformance model",
        ),
        (
            "node_skew_cube.dae",
            "dae-cts-skew-cube",
            "Khronos original angular-skew cube conformance model",
        ),
    ] {
        cases.push(register(root, name, id, title, KHRONOS)?);
    }
    for (id, title, source) in [
        (
            "dae-holed-plane",
            "COLLADA tilted planar polygon with two openings",
            holed_plane(),
        ),
        (
            "dae-wire-instances",
            "COLLADA colored lines and closed strips with repeated placements",
            wire_instances(),
        ),
        (
            "dae-lookat-placement",
            "COLLADA nested look-at placement and ordered accessor slots",
            lookat_placement(),
        ),
        (
            "dae-nonorthogonal-skew",
            "COLLADA nonorthogonal angular shear with preserved volume",
            nonorthogonal_skew(),
        ),
    ] {
        let name = format!("{id}.dae");
        fs::write(directory.join(&name), source)?;
        cases.push(register(root, &name, id, title, AUTHORED)?);
    }
    Ok(cases)
}
