//! Genuine Autodesk 3ds Max saved-image qualification through library and CLI APIs.

#[path = "support/digest_hex.rs"]
/// Shared lowercase SHA-256 formatting used by the corpus qualification tests.
mod digest_hex;

use catwalk3d::{render::RenderOptions, saved_preview, thumbnail};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Resolve an unchanged, licensed producer scene in the mandatory format corpus.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/max_saved")
        .join(name)
}

/// Produce the lowercase SHA-256 digest used in the retained provenance.
fn digest(data: &[u8]) -> String {
    format!("{:x}", digest_hex::Hex(Sha256::digest(data)))
}

#[test]
fn unchanged_producer_scenes_preserve_exact_saved_pixels() {
    for (name, source_hash, pixel_hash, dimensions) in [
        (
            "Combine Test.max",
            "4c065fedde00ed23f884b959448bb9d69725b5e0aaf667746639de91cd74f68c",
            "c06792b6a85f99a7eaf3991cacd2a37fe75833678b67f02b874ffc4b3580ab91",
            (512, 256),
        ),
        (
            "Spherify Test.max",
            "e2d8d94e20a9c182bb38ac03426bfbf6608aafa9b3f2dcbcc033a47d669ee823",
            "43f61b0fdefb6cfb3a5b682363ab032a6cb7b8c840d08d305563309483610023",
            (512, 268),
        ),
    ] {
        let path = fixture(name);
        let source = std::fs::read(&path).unwrap();
        assert_eq!(digest(&source), source_hash, "{name}");
        let image = saved_preview::extract(&path).unwrap().unwrap();
        assert_eq!(image.dimensions(), dimensions, "{name}");
        assert_eq!(digest(image.as_raw()), pixel_hash, "{name}");
        let options = RenderOptions {
            width: 96,
            height: 64,
            transparent: true,
            ..RenderOptions::default()
        };
        let fitted = thumbnail(&path, &options).unwrap();
        assert_eq!(fitted.dimensions(), (96, 64));
        assert!(fitted.pixels().any(|p| p[3] != 0));
        assert_eq!(std::fs::read(&path).unwrap(), source);
        assert!(
            catwalk3d::formats::load(&path).is_err(),
            "saved pixels must not become geometry"
        );
    }
}

#[test]
fn cli_extraction_preserves_saved_dimensions_and_refuses_geometry() {
    let input = fixture("Combine Test.max");
    let output = std::env::temp_dir().join(format!("catwalk-max-cli-{}.png", std::process::id()));
    let command = Command::new(env!("CARGO_BIN_EXE_catwalk"))
        .arg(&input)
        .arg("--extract-saved-preview")
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        command.status.success(),
        "{}",
        String::from_utf8_lossy(&command.stderr)
    );
    let image = image::open(&output).unwrap().to_rgba8();
    assert_eq!(image.dimensions(), (512, 256));
    assert_eq!(
        digest(image.as_raw()),
        "c06792b6a85f99a7eaf3991cacd2a37fe75833678b67f02b874ffc4b3580ab91"
    );
    let previous = std::fs::read(&output).unwrap();
    let failed = Command::new(env!("CARGO_BIN_EXE_catwalk"))
        .arg(&input)
        .arg("--render-geometry")
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert_eq!(std::fs::read(&output).unwrap(), previous);
    std::fs::remove_file(output).unwrap();
}

#[test]
fn absent_thumbnail_extracts_as_absent_and_unsupported_geometry_preserves_output() {
    let input = std::env::temp_dir().join(format!("catwalk-max-absent-{}.max", std::process::id()));
    std::fs::copy(fixture("Combine Test.max"), &input).unwrap();
    let mut compound = cfb::open_rw(&input).unwrap();
    compound.remove_stream("/\u{5}SummaryInformation").unwrap();
    drop(compound);
    assert!(saved_preview::extract(&input).unwrap().is_none());
    let error = thumbnail(&input, &RenderOptions::default()).unwrap_err();
    assert!(error.to_string().contains("3ds Max"));
    let output = input.with_extension("png");
    std::fs::write(&output, b"keep previous output").unwrap();
    for flags in [vec![], vec!["--extract-saved-preview"]] {
        let command = Command::new(env!("CARGO_BIN_EXE_catwalk"))
            .arg(&input)
            .args(&flags)
            .arg("--output")
            .arg(&output)
            .output()
            .unwrap();
        assert_eq!(
            command.status.code(),
            Some(if flags.is_empty() { 1 } else { 3 }),
            "{}",
            String::from_utf8_lossy(&command.stderr)
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"keep previous output");
    }
    std::fs::remove_file(input).unwrap();
    std::fs::remove_file(output).unwrap();
}

#[test]
fn original_fat_chain_integrity_is_checked_before_summary_read() {
    let input = std::env::temp_dir().join(format!("catwalk-max-cycle-{}.max", std::process::id()));
    let mut data = std::fs::read(fixture("Combine Test.max")).unwrap();
    // Locate the genuine summary's first sector through its directory entry, then make it cycle.
    let directory = cfb::CompoundFile::open(std::io::Cursor::new(&data)).unwrap();
    assert!(directory.is_stream("/\u{5}SummaryInformation"));
    drop(directory);
    let marker: Vec<u8> = "\u{5}SummaryInformation"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let entry = data
        .windows(marker.len())
        .position(|bytes| bytes == marker)
        .unwrap();
    let start = u32::from_le_bytes(data[entry + 116..entry + 120].try_into().unwrap());
    let fat = u32::from_le_bytes(data[76..80].try_into().unwrap());
    let shift = u16::from_le_bytes(data[30..32].try_into().unwrap());
    let sector_size = 1_usize << shift;
    let sector_index = usize::try_from(start).unwrap();
    assert!(
        sector_index < sector_size / 4,
        "fixture selected chain starts in first FAT sector"
    );
    let offset = (usize::try_from(fat).unwrap() + 1) * sector_size + sector_index * 4;
    data[offset..offset + 4].copy_from_slice(&start.to_le_bytes());
    std::fs::write(&input, &data).unwrap();
    let error = saved_preview::extract(&input).unwrap_err().to_string();
    assert!(error.contains("cyclic"), "{error}");
    assert_eq!(std::fs::read(&input).unwrap(), data);
    std::fs::remove_file(input).unwrap();
}
