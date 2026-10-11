//! Licensed, unmodified saved-preview fixtures. Their images come from the
//! source application's saved carrier rather than Catwalk geometry rendering.
#[path = "inventor.rs"]
/// Writer for authored Inventor thumbnail carriers.
mod inventor;

use catwalk::Result;
use std::{
    fs,
    io::{Cursor, Write},
    path::Path,
};

/// Corpus subdirectory containing document thumbnail carriers and their licenses.
const DIR: &str = "saved_previews";

/// Generate authored Inventor part and assembly thumbnail carriers.
///
/// # Errors
/// Returns an error if the source model cannot be rendered, the preview cannot
/// be encoded, or the output directory and compound files cannot be written.
pub fn generate_inventor(root: &Path) -> Result<Vec<crate::Fixture>> {
    fs::create_dir_all(root.join(DIR))?;
    let options = catwalk::render::RenderOptions {
        width: 160,
        height: 128,
        transparent: true,
        ..catwalk::render::RenderOptions::default()
    };
    let part = catwalk::thumbnail(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/models/gear.obj"),
        &options,
    )?;
    let mut assembly = image::RgbaImage::new(280, 160);
    image::imageops::overlay(&mut assembly, &part, 0, 0);
    image::imageops::overlay(&mut assembly, &image::imageops::rotate180(&part), 120, 32);
    let mut cases = Vec::new();
    for (id, extension, title, version, image) in [
        (
            "inventor-v3-part-saved-thumbnail",
            "ipt",
            "Inventor CFBv3 part thumbnail carrier",
            cfb::Version::V3,
            part,
        ),
        (
            "inventor-v4-assembly-saved-thumbnail",
            "iam",
            "Inventor CFBv4 assembly thumbnail carrier",
            cfb::Version::V4,
            assembly,
        ),
    ] {
        let mut png = Vec::new();
        catwalk::render::write_png(&image, &mut png)?;
        let input = format!("{DIR}/{id}.{extension}");
        inventor::write(&root.join(&input), version, &png)?;
        cases.push(crate::fixture(id, extension, title, &input, "saved-preview", "Original MIT-licensed CFB/OLEPS thumbnail-carrier fixture. Native Inventor summary FMTID and PID17 VT_CF wrap a saved PNG of Catwalk's authored gear; no Inventor CAD geometry. Supported carriers and resource bounds are documented in docs/research/autodesk-previews.json."));
    }
    Ok(cases)
}

/// Copy the proven `LightBurn` legacy/current carriers into the portable corpus.
/// Expected snapshot images remain controlled by the separate review workflow.
///
/// # Errors
/// Returns an error if fixture or license files cannot be copied, provenance
/// cannot be written, or a copied carrier has no valid 256 by 256 saved preview.
pub fn generate_lightburn(root: &Path) -> Result<Vec<crate::Fixture>> {
    let directory = root.join(DIR);
    fs::create_dir_all(&directory)?;
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/saved-previews/lightburn");
    for (input, output) in [
        (
            "legacy-example1.lbrn",
            "lightburn-legacy-saved-thumbnail.lbrn",
        ),
        (
            "current-jig.lbrn2",
            "lightburn-current-saved-thumbnail.lbrn2",
        ),
        (
            "LICENSE-lightburn-tester.txt",
            "LIGHTBURN-TESTER-LICENSE.txt",
        ),
        ("LICENSE-lbrnts.txt", "LBRNTS-LICENSE.txt"),
    ] {
        fs::copy(source.join(input), directory.join(output))?;
    }
    fs::write(
        directory.join("LIGHTBURN-PROVENANCE.md"),
        "# LightBurn saved-thumbnail corpus\n\n\
         These project bytes are unmodified public files. Their saved PNGs are\n\
         extracted, not regenerated from geometry; saved images can be stale.\n\n\
         - `lightburn-legacy-saved-thumbnail.lbrn`: MarcinZukowski/lightburn-tester\n\
         examples/example1.lbrn, commit 3f51e497f0d899209f762c8d507a2a08322a6a54.\n\
         Apache-2.0; full license in LIGHTBURN-TESTER-LICENSE.txt.\n\
         Source SHA-256: 181072c4c881dbca5af7c510b63bf6d7dceb9c05bc8f1d2f4364d26eaa077552.\n\
         Recorded AppVersion 0.9.16 / FormatVersion 0.\n\n\
         - `lightburn-current-saved-thumbnail.lbrn2`: tscircuit/lbrnts\n\
         tests/assets/printable_area_jig.lbrn2, commit d21a7b29354cd782cabf533b1b8f0ade6a11c7dd.\n\
         MIT, Copyright (c) 2025 tscircuit Inc.; full license in LBRNTS-LICENSE.txt.\n\
         Source SHA-256: a13336c9c1af59c1e229cb4d9f2387e45a4ac84c06a067d079f14f1fad30f784.\n\
         Recorded AppVersion 1.4.03 / FormatVersion 1.\n\n\
         Original files and exact embedded expected PNGs are retained in\n\
         tests/fixtures/saved-previews/lightburn. Detailed qualification evidence:\n\
         docs/research/lightburn-preview.json. Reviewed thumbnail snapshots are\n\
         managed separately from fixture generation.\n",
    )?;
    let cases = vec![
        crate::fixture(
            "lightburn-legacy-saved-thumbnail",
            "lbrn",
            "LightBurn legacy 0.9.16 saved thumbnail",
            "saved_previews/lightburn-legacy-saved-thumbnail.lbrn",
            "saved-preview",
            "Unmodified MarcinZukowski/lightburn-tester examples/example1.lbrn, commit 3f51e497f0d899209f762c8d507a2a08322a6a54; Apache-2.0; license retained in saved_previews/LIGHTBURN-TESTER-LICENSE.txt. Extracted saved PNG, not regenerated geometry; the saved image can be stale. https://github.com/MarcinZukowski/lightburn-tester/blob/3f51e497f0d899209f762c8d507a2a08322a6a54/examples/example1.lbrn",
        ),
        crate::fixture(
            "lightburn-current-saved-thumbnail",
            "lbrn2",
            "LightBurn 1.4.03 saved jig thumbnail",
            "saved_previews/lightburn-current-saved-thumbnail.lbrn2",
            "saved-preview",
            "Unmodified tscircuit/lbrnts tests/assets/printable_area_jig.lbrn2, commit d21a7b29354cd782cabf533b1b8f0ade6a11c7dd; MIT, Copyright (c) 2025 tscircuit Inc.; license retained in saved_previews/LBRNTS-LICENSE.txt. Extracted saved PNG, not regenerated geometry. https://github.com/tscircuit/lbrnts/blob/d21a7b29354cd782cabf533b1b8f0ade6a11c7dd/tests/assets/printable_area_jig.lbrn2",
        ),
    ];
    for case in &cases {
        let preview = catwalk::saved_preview::lightburn::extract(&root.join(&case.input))?
            .ok_or_else(|| format!("LightBurn corpus fixture has no saved preview: {}", case.id))?;
        if preview.dimensions() != (256, 256) {
            return Err(format!(
                "LightBurn corpus preview has unexpected dimensions: {}",
                case.id
            )
            .into());
        }
    }
    Ok(cases)
}

/// Include actual licensed Blender scenes and authored document-only carriers.
///
/// # Errors
/// Returns an error if the corpus directory cannot be created or a source
/// carrier or license file cannot be copied into it.
pub fn generate_blend_dwg(root: &Path) -> Result<Vec<crate::Fixture>> {
    let directory = root.join(DIR);
    fs::create_dir_all(&directory)?;
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/saved-previews/blend_dwg");
    for license in ["LICENSE-Khronos.txt", "LICENSE-Assimp.txt"] {
        fs::copy(source.join(license), directory.join(license))?;
    }
    let mut cases = Vec::new();
    for (id, extension, title, source_name, provenance) in [
        (
            "blend-252-gzip-saved-thumbnail",
            "blend",
            "Blender 2.52 saved cube thumbnail (gzip)",
            "blend-gzip-252.blend",
            "Unmodified Assimp test/models/BLEND/MirroredCube_252.blend; BSD-3-Clause, license retained as saved_previews/LICENSE-Assimp.txt. Actual scene with its saved thumbnail.",
        ),
        (
            "blend-278-saved-thumbnail",
            "blend",
            "Blender 2.78 saved cube thumbnail",
            "blend-cube.blend",
            "Unmodified Khronos glTF-Blender-IO tests/scenes/cube.blend, commit ccc2cc6ea93f7618df42b56f6001538a8366f41c; Apache-2.0, license retained as saved_previews/LICENSE-Khronos.txt. Actual scene with its saved thumbnail.",
        ),
        (
            "blend-280-saved-thumbnail",
            "blend",
            "Blender 2.80 saved scene thumbnail",
            "blend-280.blend",
            "Unmodified Khronos glTF-Blender-IO fixture, commit ccc2cc6ea93f7618df42b56f6001538a8366f41c; Apache-2.0, license retained as saved_previews/LICENSE-Khronos.txt. Actual scene with its saved thumbnail; provenance in docs/research/blend-dwg-previews.json.",
        ),
        (
            "blend-502-zstd-saved-thumbnail",
            "blend",
            "Blender 5.2 saved thumbnail (seekable Zstandard)",
            "blend-extras.blend",
            "Unmodified Khronos glTF-Blender-IO fixture, commit ccc2cc6ea93f7618df42b56f6001538a8366f41c; Apache-2.0, license retained as saved_previews/LICENSE-Khronos.txt. Actual format-1 scene with saved TEST thumbnail; provenance in docs/research/blend-dwg-previews.json.",
        ),
        (
            "blend-authored-32-saved-thumbnail",
            "blend",
            "Blender 32-bit thumbnail carrier",
            "authored-legacy-32.blend",
            "Original MIT-licensed carrier fixture. Minimal Blender header and TEST RGBA block, without model geometry; exercises 32-bit block layout, pixel orientation and transparency.",
        ),
        (
            "blend-authored-64-saved-thumbnail",
            "blend",
            "Blender 64-bit thumbnail carrier",
            "authored-legacy-64.blend",
            "Original MIT-licensed carrier fixture. Minimal Blender header and TEST RGBA block, without model geometry; exercises 64-bit block layout, pixel orientation and transparency.",
        ),
        (
            "blend-authored-format1-saved-thumbnail",
            "blend",
            "Blender format-1 thumbnail carrier",
            "authored-format1.blend",
            "Original MIT-licensed carrier fixture. Minimal 17-byte Blender header and TEST RGBA block, without model geometry; exercises new 32-byte block layout.",
        ),
        (
            "dwg-authored-dib-saved-thumbnail",
            "dwg",
            "DWG palette DIB saved-thumbnail carrier",
            "authored-dib.dwg",
            "Original MIT-licensed carrier fixture. Minimal AC1032 header with addressed, sentinel-delimited preview table and type-2 palette DIB. Contains no drawing geometry; genuine R13–2018 DWG qualification remains recorded separately in docs/research/blend-dwg-previews.json.",
        ),
        (
            "dwg-authored-png-saved-thumbnail",
            "dwg",
            "DWG PNG saved-thumbnail carrier",
            "authored-png.dwg",
            "Original MIT-licensed carrier fixture. Minimal AC1032 header with addressed, sentinel-delimited preview table and type-6 RGBA PNG. Contains no drawing geometry; genuine R13–2018 DWG qualification remains recorded separately in docs/research/blend-dwg-previews.json.",
        ),
    ] {
        let input = format!("{DIR}/{id}.{extension}");
        fs::copy(source.join(source_name), root.join(&input))?;
        cases.push(crate::fixture(
            id,
            extension,
            title,
            &input,
            "saved-preview",
            provenance,
        ));
    }
    Ok(cases)
}

/// An authored archive matching the qualified Fusion carrier, not CAD geometry.
///
/// # Errors
/// Returns an error if the source model cannot be rendered, a PNG or ZIP cannot
/// be encoded, or the output directory and archives cannot be written.
pub fn generate_fusion(root: &Path) -> Result<Vec<crate::Fixture>> {
    let directory = root.join(DIR);
    fs::create_dir_all(&directory)?;
    let options = catwalk::render::RenderOptions {
        width: 192,
        height: 128,
        supersampling: 2,
        ..catwalk::render::RenderOptions::default()
    };
    let image = catwalk::thumbnail(&root.join("mesh_scene/obj-material-palette.obj"), &options)?;
    let mut png = Vec::new();
    catwalk::render::write_png(&image, &mut png)?;
    let mut manifest = Vec::new();
    for value in ["3-2-0-0", "FusionDocType"] {
        manifest.extend_from_slice(&u32::try_from(value.len())?.to_le_bytes());
        manifest.extend_from_slice(value.as_bytes());
    }
    manifest.extend_from_slice(&4_u32.to_le_bytes());
    manifest.extend_from_slice(b".\x00f\x003\x00d\x00");
    let mut active = 15_u32.to_le_bytes().to_vec();
    active.extend("FusionAssetName".encode_utf16().flat_map(u16::to_le_bytes));
    let simulation = image::RgbaImage::from_pixel(192, 128, image::Rgba([255, 0, 255, 255]));
    let mut wrong_png = Vec::new();
    catwalk::render::write_png(&simulation, &mut wrong_png)?;
    let mut cases = Vec::new();
    for (id, compression) in [
        (
            "fusion-authored-deflate-preview",
            zip::CompressionMethod::Deflated,
        ),
        (
            "fusion-authored-stored-preview",
            zip::CompressionMethod::Stored,
        ),
    ] {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in [
            ("Manifest.dat", &manifest),
            ("FusionAssetName[Active]/Manifest.dat", &active),
            ("FusionAssetName[Active]/Previews/small.png", &png),
            (
                "SimulationModelAssetSubtype[Active]/Previews/small.png",
                &wrong_png,
            ),
        ] {
            archive.start_file(
                name,
                zip::write::SimpleFileOptions::default().compression_method(compression),
            )?;
            archive.write_all(data)?;
        }
        let input = format!("{DIR}/{id}.f3d");
        fs::write(root.join(&input), archive.finish()?.into_inner())?;
        cases.push(crate::fixture(id, "f3d", "Fusion active-design saved-thumbnail carrier", &input, "saved-preview", "Original MIT-licensed carrier fixture. ZIP32, qualified 3-2-0-0 manifest and exact active-design PNG; deliberately different simulation image tests selection. Contains no Fusion geometry. Real design/archive qualification, including ZIP method93 Zstandard, is recorded separately in docs/research/autodesk-previews.json."));
    }
    Ok(cases)
}
