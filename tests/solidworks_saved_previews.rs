//! Original producer-document qualification of addressed `SolidWorks` thumbnails.

#[path = "support/digest_hex.rs"]
mod digest_hex;

use catwalk3d::{render::RenderOptions, saved_preview, thumbnail};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/solidworks")
        .join(name)
}

/// Start the public CLI without display-related environment services.
fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_catwalk"));
    command
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR");
    command
}

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "catwalk-solidworks-producer-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn original_parts_and_assemblies_match_independently_extracted_png_pixels() {
    for (name, expected_hash) in [
        (
            "solidworks-about.sldprt",
            "08a8ec2afc112a27b1b8f4fad92224080b5c0d8cda8fc095b423cf91ca062e34",
        ),
        (
            "solidworks-powerpackage.sldasm",
            "2e71fb43b889a5d9585eb7606266a1dd35ef685244df6ee65782a7f7b2f19788",
        ),
    ] {
        let path = fixture(name);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes))),
            expected_hash
        );
        let expected = image::open(fixture(&format!("{name}.preview.png")))
            .unwrap()
            .into_rgba8();
        let actual = saved_preview::extract(&path).unwrap().unwrap();
        assert_eq!(actual.dimensions(), (640, 480));
        assert_eq!(actual, expected);
        let options = RenderOptions {
            width: 96,
            height: 64,
            transparent: true,
            ..RenderOptions::default()
        };
        assert_eq!(thumbnail(&path, &options).unwrap().dimensions(), (96, 64));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "original producer file changed"
        );
    }
}

#[test]
fn original_rle8_preview_streams_work_when_the_png_is_absent() {
    for name in ["solidworks-about.sldprt", "solidworks-powerpackage.sldasm"] {
        let directory = Directory::new();
        let path = directory.0.join(name);
        std::fs::copy(fixture(name), &path).unwrap();
        {
            let mut compound = cfb::open_rw(&path).unwrap();
            compound.remove_stream("PreviewPNG").unwrap();
        }
        let actual = saved_preview::extract(&path).unwrap().unwrap();
        let expected = image::open(fixture(&format!("{name}.preview-dib.png")))
            .unwrap()
            .into_rgba8();
        assert_eq!(actual, expected);
    }
}

#[test]
fn cli_extraction_preserves_pixels_and_geometry_forcing_preserves_output() {
    let directory = Directory::new();
    let source = fixture("solidworks-powerpackage.sldasm");
    let extracted = cli()
        .arg(&source)
        .args(["--extract-saved-preview", "--output", "-"])
        .output()
        .unwrap();
    assert!(
        extracted.status.success(),
        "{}",
        String::from_utf8_lossy(&extracted.stderr)
    );
    let expected = saved_preview::extract(&source).unwrap().unwrap();
    assert_eq!(
        image::load_from_memory(&extracted.stdout)
            .unwrap()
            .into_rgba8(),
        expected
    );
    let previous = directory.0.join("previous.png");
    std::fs::write(&previous, b"previous thumbnail").unwrap();
    let result = cli()
        .arg(&source)
        .arg("--render-geometry")
        .arg("--output")
        .arg(&previous)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("unsupported model format .sldasm"));
    assert_eq!(std::fs::read(previous).unwrap(), b"previous thumbnail");
}

#[test]
fn missing_document_thumbnail_has_exit_three_and_keeps_old_output() {
    let directory = Directory::new();
    let source = directory.0.join("missing.sldprt");
    std::fs::copy(fixture("solidworks-about.sldprt"), &source).unwrap();
    {
        let mut compound = cfb::open_rw(&source).unwrap();
        compound.remove_stream("PreviewPNG").unwrap();
        compound.remove_stream("Preview").unwrap();
    }
    let destination = directory.0.join("previous.png");
    std::fs::write(&destination, b"previous thumbnail").unwrap();
    for extra in [vec![], vec!["--extract-saved-preview"]] {
        let result = cli()
            .arg(&source)
            .args(extra)
            .arg("--output")
            .arg(&destination)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(3));
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous thumbnail");
    }
}

#[test]
fn corrupt_addressed_thumbnail_preserves_existing_output() {
    use std::io::Write;

    let directory = Directory::new();
    let source = directory.0.join("corrupt.SLDASM");
    std::fs::copy(fixture("solidworks-powerpackage.sldasm"), &source).unwrap();
    {
        let mut compound = cfb::open_rw(&source).unwrap();
        compound.remove_stream("Preview").unwrap();
        compound
            .create_stream("PreviewPNG")
            .unwrap()
            .write_all(b"corrupt image")
            .unwrap();
    }
    let destination = directory.0.join("previous.png");
    std::fs::write(&destination, b"previous thumbnail").unwrap();
    let output = cli()
        .arg(&source)
        .args(["--extract-saved-preview", "--output"])
        .arg(&destination)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(std::fs::read(destination).unwrap(), b"previous thumbnail");
}
