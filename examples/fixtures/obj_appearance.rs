//! Authored OBJ appearance controls and a retained independently exported model.

use catwalk3d::Result;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use std::{fs, io::Cursor, path::Path};

/// Corpus directory retaining the model and all of its local dependencies.
const DIR: &str = "obj_appearance";
/// Source and license notice attached to each authored control.
const AUTHORED: &str =
    "Author-created deterministic Rust fixture; MIT, matching the project license";
/// Exact independently exported source and original retained license location.
const ASSIMP: &str = "Unmodified Assimp test/models/OBJ/regr01.obj and regr01.mtl at commit f6ee58e2489f3cbaee328655599f50c5c386a08e; original headers identify Max2Obj/Max2Mtl Version 4.0 export. Original legacy material names are decoded without modifying source bytes. Separately BSD-3-Clause licensed test asset; complete original license retained as obj_appearance/ASSIMP-LICENSE. Source contains 2710 triangles, including four zero-area faces omitted from rendered geometry, and 12 diffuse materials with UVs and translucent windows; no texture maps or supplied normals. Exact hashes and independent geometry/appearance expectations in obj_appearance/README.md. https://github.com/assimp/assimp/tree/f6ee58e2489f3cbaee328655599f50c5c386a08e/test/models/OBJ";

/// Tilted concave L with separately ordered position, UV and normal streams.
const CONCAVE_OBJ: &str = "# Author-created tilted concave L; MIT\nmtllib concave.mtl\no ConcaveCornerStreams\nv 0 0 0 1 1 1\nv 2 0 .5 .75 1 1\nv 2 1 .625 1 .75 1\nv 1 1 .375 1 1 .75\nv 1 2 .5 .75 .75 1\nv 0 2 .25 1 .75 .75\nvt .5 .5\nvt 0 1\nvt 1 0\nvt .5 1\nvt 0 0\nvt 1 .5\nvn -.25 -.125 2\nvn .75 -.125 1\nvn -.25 .875 1\nusemtl textured\nf -5/-4/2 -4/-1/2 -3/-6/1 -2/-3/3 -1/-5/3 -6/-2/1\n";
/// Diffuse reflectivity multiplying the independent quadrant image and vertex RGB.
const CONCAVE_MTL: &str =
    "# Author-created diffuse material; MIT\nnewmtl textured\nKd .5 .8 1\nmap_Kd quadrants.png\n";
/// Four congruent cards comparing no map, white map, repeat and material border.
const MAPS_OBJ: &str = "# Author-created diffuse map controls; MIT\nmtllib maps.mtl\nv 0 0 0\nv 1 0 .25\nv 1 1 .375\nv 0 1 .125\nv 1.5 0 .375\nv 2.5 0 .625\nv 2.5 1 .75\nv 1.5 1 .5\nv 0 1.5 .1875\nv 1 1.5 .4375\nv 1 2.5 .5625\nv 0 2.5 .3125\nv 1.5 1.5 .5625\nv 2.5 1.5 .8125\nv 2.5 2.5 .9375\nv 1.5 2.5 .6875\nvt -.5 0\nvt .75 0\nvt .75 1\nvt -.5 1\nusemtl plain\nf 1 2 3 4\nusemtl white\nf 5/1 6/2 7/3 8/4\nusemtl repeat\nf 9/1 10/2 11/3 12/4\nusemtl border\nf 13/1 14/2 15/3 16/4\n";
/// Identical diffuse values expose gamma and outside-texture sampling mistakes.
const MAPS_MTL: &str = "# Author-created linear diffuse controls; MIT\nnewmtl plain\nKd .5 .4 .25\nnewmtl white\nKd .5 .4 .25\nmap_Kd white.png\nnewmtl repeat\nKd .5 .4 .25\nmap_Kd -s 2 .5 -o -.25 .125 -clamp off quadrants.png\nnewmtl border\nKd .5 .4 .25\nmap_Kd -s 2 .5 -o -.25 .125 -clamp on quadrants.png\n";
/// Opaque backing with two partly overlapping translucent front cards.
const ALPHA_OBJ: &str = "# Author-created material dissolve overlap; MIT\nmtllib alpha.mtl\nv 0 0 0\nv 2 0 .5\nv 2 2 .75\nv 0 2 .25\nv .1 .15 .24375\nv 1.4 .15 .56875\nv 1.4 1.8 .775\nv .1 1.8 .45\nv .6 .4 .5\nv 1.9 .4 .825\nv 1.9 1.9 1.0125\nv .6 1.9 .6875\nusemtl backing\nf 1 2 3 4\nusemtl dissolve\nf 5 6 7 8\nusemtl transparency\nf 9 10 11 12\n";
/// Original dissolve and the common inverse-transparency extension are separated.
const ALPHA_MTL: &str = "# Author-created opacity materials; MIT\nnewmtl backing\nKd .15 .2 .8\nd 1\nnewmtl dissolve\nKd .9 .1 .1\nd .5\nnewmtl transparency\nKd .1 .9 .15\nTr .75\n";

/// Encode a deterministic opaque white or four-quadrant image in Rust.
fn png(white: bool) -> Result<Vec<u8>> {
    let image = RgbaImage::from_fn(8, 8, |x, y| {
        Rgba(if white {
            [255; 4]
        } else {
            match (x < 4, y < 4) {
                (true, true) => [240, 55, 40, 255],
                (false, true) => [40, 220, 65, 255],
                (true, false) => [45, 65, 230, 255],
                (false, false) => [245, 215, 40, 255],
            }
        })
    });
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image).write_to(&mut output, ImageFormat::Png)?;
    Ok(output.into_inner())
}

/// Require nonempty decoded geometry before registering an OBJ gallery input.
fn register(
    root: &Path,
    name: &str,
    id: &str,
    title: &str,
    provenance: &str,
) -> Result<crate::Fixture> {
    let input = format!("{DIR}/{name}");
    let scene = catwalk3d::formats::load(&root.join(&input))?;
    if scene.triangles.is_empty() {
        return Err(format!("OBJ appearance fixture {input} has no triangles").into());
    }
    Ok(crate::fixture(
        id, "obj", title, &input, "model", provenance,
    ))
}

/// Borrow unchanged exporter files and their complete checked-in source notice.
fn retained_resources() -> [(&'static str, &'static [u8]); 4] {
    [
        (
            "regr01.obj",
            include_bytes!("../../tests/fixtures/formats/obj_appearance/regr01.obj"),
        ),
        (
            "regr01.mtl",
            include_bytes!("../../tests/fixtures/formats/obj_appearance/regr01.mtl"),
        ),
        (
            "ASSIMP-LICENSE",
            include_bytes!("../../tests/fixtures/formats/obj_appearance/ASSIMP-LICENSE"),
        ),
        (
            "README.md",
            include_bytes!("../../tests/fixtures/formats/obj_appearance/README.md"),
        ),
    ]
}

/// Write authored controls and copy the pinned exporter resources into the corpus.
///
/// # Errors
/// Returns an error for filesystem or PNG encoding failures, malformed decoded
/// geometry, or a fixture that contains no rendered triangles.
pub fn generate(root: &Path) -> Result<Vec<crate::Fixture>> {
    let directory = root.join(DIR);
    fs::create_dir_all(&directory)?;
    for (name, bytes) in retained_resources() {
        fs::write(directory.join(name), bytes)?;
    }
    for (name, bytes) in [
        ("obj-textured-concave.obj", CONCAVE_OBJ),
        ("concave.mtl", CONCAVE_MTL),
        ("obj-texture-transform-border.obj", MAPS_OBJ),
        ("maps.mtl", MAPS_MTL),
        ("obj-alpha-overlap.obj", ALPHA_OBJ),
        ("alpha.mtl", ALPHA_MTL),
    ] {
        fs::write(directory.join(name), bytes)?;
    }
    fs::write(directory.join("quadrants.png"), png(false)?)?;
    fs::write(directory.join("white.png"), png(true)?)?;
    [
        (
            "regr01.obj",
            "obj-assimp-building",
            "Max2Obj exported building with original diffuse materials and UVs",
            ASSIMP,
        ),
        (
            "obj-textured-concave.obj",
            "obj-textured-concave",
            "OBJ concave polygon with independent UV, normal and color corners",
            AUTHORED,
        ),
        (
            "obj-texture-transform-border.obj",
            "obj-texture-transform-border",
            "OBJ linear diffuse colors, map transforms, repeat and border",
            AUTHORED,
        ),
        (
            "obj-alpha-overlap.obj",
            "obj-alpha-overlap",
            "OBJ dissolved materials with translucent depth overlaps",
            AUTHORED,
        ),
    ]
    .into_iter()
    .map(|(name, id, title, provenance)| register(root, name, id, title, provenance))
    .collect()
}
