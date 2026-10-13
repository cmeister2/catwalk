//! Original `SketchUp` saved-pixel qualification and addressed-carrier failure tests.

use catwalk3d::saved_preview;
use std::{
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

/// Unique suffix for temporary mutated carriers and CLI outputs.
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

/// Resolve an unchanged producer file or its independently extracted reference PNG.
fn fixture(name: &str, extension: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/sketchup")
        .join(format!("{name}.{extension}"))
}

/// Automatically remove one test-owned temporary file.
struct Temporary(PathBuf);

impl Temporary {
    /// Write a unique input without depending on external desktop software.
    fn write(bytes: &[u8]) -> Self {
        let index = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "catwalk-sketchup-{}-{index}.skp",
            std::process::id()
        ));
        fs::write(&path, bytes).unwrap();
        Self(path)
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
fn original_producer_thumbnails_match_independent_pngs_exactly() {
    for name in [
        "legacy-box-v2013",
        "legacy-box-v2014",
        "legacy-box-v2015",
        "legacy-box-v2016",
        "2017-box",
        "2017-box-two-materials",
        "2026-box",
        "2026-box-two-materials",
    ] {
        let actual = saved_preview::extract(&fixture(name, "skp"))
            .unwrap()
            .unwrap();
        let expected = image::open(fixture(name, "png")).unwrap().into_rgba8();
        assert_eq!(actual.dimensions(), (256, 256));
        assert_eq!(actual, expected, "exact original saved pixels for {name}");
    }
}

#[test]
fn automatic_thumbnail_is_fitted_and_geometry_mode_remains_unsupported() {
    let input = fixture("2026-box-two-materials", "skp");
    let options = catwalk3d::render::RenderOptions {
        width: 64,
        height: 64,
        transparent: true,
        ..Default::default()
    };
    assert_eq!(
        catwalk3d::thumbnail(&input, &options).unwrap().dimensions(),
        (64, 64)
    );
    assert!(catwalk3d::formats::load(&input).is_err());
    let output = Command::new(env!("CARGO_BIN_EXE_catwalk"))
        .arg(&input)
        .args(["--render-geometry", "--output", "-"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stdout, [] as [u8; 0]);
}

#[test]
fn legacy_null_model_image_does_not_select_later_pngs() {
    let mut bytes = fs::read(fixture("2017-box", "skp")).unwrap();
    // Independently characterized model CDib starts at 0x7cf; preview and
    // component PNGs remain later in the original file as decoys.
    bytes[0x7cf..0x7d1].fill(0);
    let input = Temporary::write(&bytes);
    assert!(saved_preview::extract(&input.0).unwrap().is_none());
    let output = Command::new(env!("CARGO_BIN_EXE_catwalk"))
        .arg(&input.0)
        .args(["--extract-saved-preview", "--output", "-"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(output.stdout, [] as [u8; 0]);
}

#[test]
fn legacy_framing_class_size_and_image_limits_fail_explicitly() {
    let original = fs::read(fixture("2017-box", "skp")).unwrap();
    for (offset, replacement) in [
        (0x50, vec![0, 0]),                    // CVersionMap class tag.
        (0x7cf + 2, vec![2, 0]),               // CDib schema.
        (0x7d9, 1_u32.to_le_bytes().to_vec()), // Image subtype.
        (0x7dd, (8_u32 * 1024 * 1024 + 1).to_le_bytes().to_vec()), // Encoded image size.
        (0x7e1 + 16, 4097_u32.to_be_bytes().to_vec()), // Width, before codec allocation.
        (0x26, vec![b'2', 0, b'0', 0]),        // Unqualified major version 20.
    ] {
        let mut bytes = original.clone();
        bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
        let input = Temporary::write(&bytes);
        assert!(
            saved_preview::extract(&input.0).is_err(),
            "corruption at {offset:#x}"
        );
    }
    for length in [0, 31, 51, 79, 150, 0x7d0, 0x7e0, 0x7e1 + 100] {
        let input = Temporary::write(&original[..length]);
        assert!(
            saved_preview::extract(&input.0).is_err(),
            "truncation at {length:#x}"
        );
    }
}

/// Construct modern ZIP controls while retaining the independently observed VFF prefix.
fn modern(metadata: &[u8], images: &[(&str, &[u8])]) -> Vec<u8> {
    let original = fs::read(fixture("2026-box", "skp")).unwrap();
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in [
        ("meta/meta.dat", metadata),
        ("model.dat", b"opaque stored model".as_slice()),
    ]
    .into_iter()
    .chain(images.iter().copied())
    {
        archive.start_file(name, options).unwrap();
        archive.write_all(bytes).unwrap();
    }
    let mut bytes = original[..65].to_vec();
    bytes.extend(archive.finish().unwrap().into_inner());
    bytes
}

/// Independently access the original metadata using the general-purpose ZIP reader.
fn metadata() -> Vec<u8> {
    use std::io::Read;
    let bytes = fs::read(fixture("2026-box", "skp")).unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(&bytes[65..])).unwrap();
    let mut meta = Vec::new();
    zip.by_name("meta/meta.dat")
        .unwrap()
        .read_to_end(&mut meta)
        .unwrap();
    meta
}

#[test]
fn modern_preview_and_material_images_cannot_replace_model_thumbnail() {
    let mut meta = metadata();
    // The model descriptor is the 0x67 field at byte72, length46. Remove it
    // and adjust only the parent record length, preserving preview metadata.
    assert_eq!(&meta[72..74], &[0x67, 0]);
    meta.drain(72..124);
    meta[2..6].copy_from_slice(&355_u32.to_le_bytes());
    let png = fs::read(fixture("2026-box", "png")).unwrap();
    let input = Temporary::write(&modern(
        &meta,
        &[
            ("meta/preview_thumbnail.png", &png),
            ("materials/Default/texture.png", &png),
            ("meta/model_thumbnail.png", &png),
        ],
    ));
    assert!(saved_preview::extract(&input.0).unwrap().is_none());
    // The carrier declares a model thumbnail but its selected member is absent.
    let input = Temporary::write(&modern(
        &metadata(),
        &[("meta/preview_thumbnail.png", &png)],
    ));
    assert!(saved_preview::extract(&input.0).is_err());
}

#[test]
fn modern_metadata_identity_addressing_and_duplicate_fields_are_checked() {
    let original = metadata();
    let png = fs::read(fixture("2026-box", "png")).unwrap();
    for (offset, replacement) in [
        (0, vec![0x65, 0]),       // Root record type.
        (12, b"25.2.0".to_vec()), // Metadata disagrees with outer header.
        (24, vec![25, 0]),        // Metadata major version disagrees.
        (90, vec![1, 0, 0, 0]),   // Unsupported model image subtype.
        (105, b"other".to_vec()), // Different image address cannot be selected.
    ] {
        let mut meta = original.clone();
        meta[offset..offset + replacement.len()].copy_from_slice(&replacement);
        let input = Temporary::write(&modern(&meta, &[("meta/model_thumbnail.png", &png)]));
        assert!(
            saved_preview::extract(&input.0).is_err(),
            "metadata corruption at {offset}"
        );
    }
    let mut meta = original.clone();
    meta.extend_from_slice(&original[6..18]);
    meta[2..6].copy_from_slice(&419_u32.to_le_bytes());
    let input = Temporary::write(&modern(&meta, &[("meta/model_thumbnail.png", &png)]));
    assert!(saved_preview::extract(&input.0).is_err());
    let input = Temporary::write(&modern(
        &original,
        &[
            ("meta/model_thumbnail.png", b"not a PNG"),
            ("../texture.png", &png),
        ],
    ));
    assert!(saved_preview::extract(&input.0).is_err());
}

#[test]
fn modern_member_crc_is_checked_and_cli_preserves_original_pixels() {
    let png = fs::read(fixture("2026-box", "png")).unwrap();
    let mut bytes = modern(&metadata(), &[("meta/model_thumbnail.png", &png)]);
    let start = bytes.windows(png.len()).position(|b| b == png).unwrap();
    bytes[start + png.len() / 2] ^= 1;
    let input = Temporary::write(&bytes);
    assert!(saved_preview::extract(&input.0).is_err());
    let output = Command::new(env!("CARGO_BIN_EXE_catwalk"))
        .arg(fixture("2026-box-two-materials", "skp"))
        .args(["--extract-saved-preview", "--output", "-"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = image::load_from_memory(&output.stdout)
        .unwrap()
        .into_rgba8();
    assert_eq!(
        actual,
        image::open(fixture("2026-box-two-materials", "png"))
            .unwrap()
            .into_rgba8()
    );
}
