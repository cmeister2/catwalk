//! Original multipage TIFF and authored scalar, placement and compression controls.

#[path = "../../tests/support/tiff_fixture.rs"]
/// Encode authored page chains independently of the production reader.
mod carrier;

use carrier::{Frame, Layout};
use catwalk3d::Result;
use std::{fs, io::Cursor, path::Path};
use tiff::encoder::{Compression, TiffEncoder, colortype, compression::DeflateLevel};

/// Corpus directory retaining the original file and authored volume controls.
const DIR: &str = "tiff_volumes";
/// Original source, external producer and redistribution notice.
const ORIGINAL: &str = "Unmodified image-rs/image-tiff tests/images/seq/seq-1c-8b-multipage-adeefdcc.tiff at commit 3302936b08e2260d4c66f40f51170efbb1fded4c; SHA-256 f9dd5eb92e193f121b2afcb3575d2be5566adb6198afcb5009516a77972d6428. MIT, Copyright (c) 2018 PistonDevelopers; original notice retained as tiff_volumes/IMAGE-TIFF-LICENSE. The repository's libtiff producer writes three actual 16x16 uint8 pages; every decoded sample and independent surface facts are qualified in tests/tiff_volumes.rs. https://github.com/image-rs/image-tiff/blob/3302936b08e2260d4c66f40f51170efbb1fded4c/tests/images/seq/seq-1c-8b-multipage-adeefdcc.tiff";
/// Authorship for explicit small scalar page chains and their shape controls.
const AUTHORED: &str = "Author-created deterministic Rust scalar TIFF page chain; MIT, matching the project license. Manual classic/BigTIFF carriers, an asymmetric voxel shape, orientations, scalar types and rational spacing are qualified by independent literal geometry in tests/tiff_volumes.rs; these controls do not claim a desktop exporter.";

/// Make a nonsymmetric solid from a base, a vertical post and a low projecting arm.
fn shape() -> Vec<Frame> {
    (0..5)
        .map(|z| {
            let data = (0..6)
                .flat_map(|y| {
                    (0..8).map(move |x| {
                        let post = (1..=2).contains(&x) && (1..=4).contains(&y);
                        let arm = z < 2 && (2..=6).contains(&x) && (1..=2).contains(&y);
                        let base = z == 0 && (1..=6).contains(&x) && (1..=4).contains(&y);
                        if post || arm || base { 200 } else { 0 }
                    })
                })
                .collect();
            Frame::gray8(8, 6, data)
        })
        .collect()
}

/// Preserve the lit voxels while storing the inverse unsigned 16-bit samples.
fn white_frames(frames: &[Frame]) -> Vec<Frame> {
    frames
        .iter()
        .cloned()
        .map(|mut frame| {
            frame.bits = 16;
            frame.white_is_zero = true;
            frame.data = frame
                .data
                .iter()
                .flat_map(|&v| (u16::MAX - u16::from(v) * 257).to_le_bytes())
                .collect();
            frame
        })
        .collect()
}

/// Store float32 samples with independent two/three millimetre column/row spacing.
fn physical_frames(frames: &[Frame]) -> Vec<Frame> {
    frames
        .iter()
        .cloned()
        .map(|mut frame| {
            frame.bits = 32;
            frame.sample_format = 3;
            frame.resolution_unit = 3;
            frame.x_resolution = [5, 1];
            frame.y_resolution = [10, 3];
            frame.data = frame
                .data
                .iter()
                .flat_map(|&v| f32::from(v).to_le_bytes())
                .collect();
            frame
        })
        .collect()
}

/// Write one authored case and register its automatic volume-thumbnail output.
///
/// # Errors
/// Returns an error if the authored page chain or output file cannot be written.
fn authored(
    root: &Path,
    id: &str,
    extension: &str,
    title: &str,
    bytes: &[u8],
) -> Result<super::Fixture> {
    let input = format!("{DIR}/{id}.{extension}");
    fs::write(root.join(&input), bytes)?;
    Ok(super::fixture(
        id, extension, title, &input, "image", AUTHORED,
    ))
}

/// Preserve the original multipage file and generate typed scalar volume cases.
///
/// # Errors
/// Returns an error for missing retained originals, codec errors or output I/O failures.
pub fn generate(root: &Path) -> Result<Vec<super::Fixture>> {
    fs::create_dir_all(root.join(DIR))?;
    let original = format!("{DIR}/seq-1c-8b-multipage-adeefdcc.tiff");
    if !root.join(&original).is_file() || !root.join(DIR).join("IMAGE-TIFF-LICENSE").is_file() {
        return Err("retained TIFF original and MIT notice are required".into());
    }
    let mut cases = vec![super::fixture(
        "tiff-original-three-page-volume",
        "tiff",
        "Original libtiff three-page scalar volume",
        &original,
        "image",
        ORIGINAL,
    )];
    let frames = shape();
    cases.push(authored(
        root,
        "tif-scalar-asymmetric-volume",
        "tif",
        "TIFF asymmetric scalar volume",
        &carrier::encode(&frames, Layout::default())?,
    )?);
    let mut rotated = frames.clone();
    for frame in &mut rotated {
        frame.orientation = 6;
    }
    cases.push(authored(
        root,
        "tiff-bigtiff-rotated-volume",
        "tiff",
        "BigTIFF rotated scalar volume",
        &carrier::encode(
            &rotated,
            Layout {
                big: true,
                big_endian: false,
            },
        )?,
    )?);
    cases.push(authored(
        root,
        "tiff-whiteiszero-gray16-volume",
        "tiff",
        "TIFF WhiteIsZero 16-bit volume",
        &carrier::encode(&white_frames(&frames), Layout::default())?,
    )?);
    cases.push(authored(
        root,
        "tiff-float32-physical-volume",
        "tiff",
        "TIFF float32 volume with physical spacing",
        &carrier::encode(&physical_frames(&frames), Layout::default())?,
    )?);
    let mut signed = frames.clone();
    for frame in &mut signed {
        frame.bits = 16;
        frame.sample_format = 2;
        frame.data = frame
            .data
            .iter()
            .flat_map(|&v| (if v == 0 { -40_i16 } else { 80 }).to_be_bytes())
            .collect();
    }
    cases.push(authored(
        root,
        "tif-big-endian-signed-volume",
        "tif",
        "Big-endian signed TIFF volume",
        &carrier::encode(
            &signed,
            Layout {
                big: false,
                big_endian: true,
            },
        )?,
    )?);
    let mut deflate = Cursor::new(Vec::new());
    {
        let mut encoder = TiffEncoder::new(&mut deflate)?
            .with_compression(Compression::Deflate(DeflateLevel::Balanced));
        for frame in &frames {
            encoder.write_image::<colortype::Gray8>(frame.width, frame.height, &frame.data)?;
        }
    }
    cases.push(authored(
        root,
        "tiff-deflate-scalar-volume",
        "tiff",
        "Deflate-compressed TIFF scalar volume",
        deflate.get_ref(),
    )?);
    Ok(cases)
}
