//! Original-source and CLI contracts for exact Lychee saved-image extraction.

#[path = "support/digest_hex.rs"]
/// Use the shared test-only lowercase SHA-256 formatting protocol.
mod digest_hex;

use catwalk3d::{render::RenderOptions, saved_preview, thumbnail};
use image::RgbaImage;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

/// Unique process-local directory suffix for temporary CLI controls.
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

/// Literal original-source facts established independently of the reader.
struct Original {
    /// Stable low/high-profile keyboard-case source identifier.
    name: &'static str,
    /// Stable gallery/snapshot identifier.
    case: &'static str,
    /// Entire original source byte count.
    bytes: usize,
    /// Literal source-file SHA-256.
    source_hash: &'static str,
    /// Independently verified absolute start of the addressed original PNG.
    preview_start: usize,
    /// Original encoded preview byte count.
    preview_bytes: usize,
    /// Literal encoded preview SHA-256.
    preview_hash: &'static str,
    /// Literal decoded 512 × 512 RGBA pixel SHA-256.
    pixel_hash: &'static str,
}

/// Original MIT-licensed Lychee 5.4.3 scenes pinned at source commit a666598d.
const ORIGINALS: [Original; 2] = [
    Original {
        name: "delphi-edge-case-lp",
        case: "lychee-delphi-lp-saved-thumbnail",
        bytes: 805_288,
        source_hash: "b1f145b784f1705582f86f980e7f93e0ef849746fe0f0f1e3bd395ea9535ce90",
        preview_start: 115_308,
        preview_bytes: 21_320,
        preview_hash: "1a2fe62db9dbc709cd1c6fb7e5a3ef7b3c484d6e996569d46803f3a061ef6c9a",
        pixel_hash: "627b5a44e15db1817513082c43eceb70e7e7129b4a21194d3ada588a1ea88d06",
    },
    Original {
        name: "delphi-edge-case-hp",
        case: "lychee-delphi-hp-saved-thumbnail",
        bytes: 1_015_246,
        source_hash: "efb540d3aa82c581dc7fbeb34db07a91a93e8230a652f591a35ef1e2cd2640ac",
        preview_start: 116_028,
        preview_bytes: 86_679,
        preview_hash: "b99e0763a77248b2094f50a00b0e83b407fbba1cb96ed22290351ba3727ba1d6",
        pixel_hash: "f071d4f2f3742c4bcdbb5b54f841a95519ba8d4389af9c085149487a90f7a247",
    },
];

/// Resolve original assets without depending on the current working directory.
fn fixture(name: &str, extension: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/lychee")
        .join(format!("{name}.{extension}"))
}

/// Calculate a literal SHA-256 comparison string for original bytes and pixels.
fn hash(data: &[u8]) -> String {
    format!("{:x}", digest_hex::Hex(Sha256::digest(data)))
}

/// Decode CLI PNG output and preserve all RGBA values for exact comparison.
fn decode(bytes: &[u8]) -> RgbaImage {
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .unwrap()
        .into_rgba8()
}

/// Invoke the CLI headlessly without forwarding output or opening desktop tools.
fn command(input: &Path, output: &Path, options: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_catwalk"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR")
        .arg(input)
        .arg("--output")
        .arg(output)
        .args(options)
        .output()
        .unwrap()
}

/// Require successful image output without diagnostics contaminating stdout.
fn successful(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stderr, [] as [u8; 0]);
}

#[test]
fn original_licensed_sources_and_selected_png_bytes_remain_exact() {
    for original in &ORIGINALS {
        let source = fs::read(fixture(original.name, "lys")).unwrap();
        let preview = fs::read(fixture(original.name, "png")).unwrap();
        assert_eq!(source.len(), original.bytes);
        assert_eq!(hash(&source), original.source_hash);
        assert_eq!(preview.len(), original.preview_bytes);
        assert_eq!(hash(&preview), original.preview_hash);
        assert_eq!(
            &source[original.preview_start..original.preview_start + original.preview_bytes],
            preview,
        );
        assert_eq!(
            &source[..16],
            &[4, 0, 0, 0, 56, 3, 0, 0, 52, 3, 0, 0, 48, 3, 0, 0]
        );
    }
    let license = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/formats/lychee/DELPHI-LICENSE"),
    )
    .unwrap();
    assert_eq!(license.len(), 1072);
    assert_eq!(
        hash(&license),
        "968f1cbaf9ddfe41ee913bac7fbec6bb8cc9de0cbd5f5e71d4e1de7d381cae40"
    );
    let license = std::str::from_utf8(&license).unwrap();
    assert!(license.contains("Copyright (c) 2022-25 John Riggles"));
    assert!(license.contains("The above copyright notice and this permission notice"));
}

#[test]
fn public_extraction_preserves_original_pixels_and_thumbnail_dispatch() {
    assert!(saved_preview::EXTENSIONS.contains(&"lys"));
    assert!(!catwalk3d::formats::NATIVE_GEOMETRY_EXTENSIONS.contains(&"lys"));
    for original in &ORIGINALS {
        let input = fixture(original.name, "lys");
        let actual = saved_preview::lychee::extract(&input).unwrap().unwrap();
        let expected = decode(&fs::read(fixture(original.name, "png")).unwrap());
        assert_eq!(actual.dimensions(), (512, 512));
        assert_eq!(hash(actual.as_raw()), original.pixel_hash);
        assert_eq!(actual, expected);
        assert_eq!(saved_preview::extract(&input).unwrap().unwrap(), expected);
        let options = RenderOptions {
            width: 512,
            height: 512,
            transparent: true,
            ..Default::default()
        };
        let fitted = thumbnail(&input, &options).unwrap();
        assert_eq!(fitted.dimensions(), (512, 512));
        for (index, (actual, expected)) in fitted.pixels().zip(expected.pixels()).enumerate() {
            assert!(
                actual[3].abs_diff(expected[3]) <= 1,
                "alpha at pixel {index}"
            );
            if expected[3] != 0 {
                for channel in 0..3 {
                    assert!(
                        actual[channel].abs_diff(expected[channel]) <= 1,
                        "visible color at pixel {index}"
                    );
                }
            }
        }
        assert!(catwalk3d::geometry_thumbnail(&input, &options).is_err());
    }
}

#[test]
fn cli_original_extraction_and_automatic_thumbnails_match_reviewed_images() {
    for original in &ORIGINALS {
        let input = fixture(original.name, "lys");
        let before = fs::read(&input).unwrap();
        let extracted = command(&input, Path::new("-"), &["--extract-saved-preview"]);
        successful(&extracted);
        let expected = decode(&fs::read(fixture(original.name, "png")).unwrap());
        assert_eq!(decode(&extracted.stdout), expected);
        let automatic = command(&input, Path::new("-"), &["--size", "256", "--transparent"]);
        successful(&automatic);
        let snapshot = image::open(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/snapshots/formats")
                .join(format!("{}.png", original.case)),
        )
        .unwrap()
        .into_rgba8();
        assert_eq!(decode(&automatic.stdout), snapshot);
        assert_eq!(fs::read(&input).unwrap(), before);
    }
}

/// Own a collision-resistant temporary folder for neutral malformed/absent controls.
struct Controls(PathBuf);

impl Controls {
    /// Create a unique temporary directory without deleting any preexisting folder.
    fn new() -> Self {
        loop {
            let suffix = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("catwalk lychee {} {suffix}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("cannot create Lychee control directory: {error}"),
            }
        }
    }

    /// Write a neutral control without modifying the licensed source scenes.
    fn write(&self, name: &str, data: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, data).unwrap();
        path
    }
}

impl Drop for Controls {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Frame a neutral no-preview manifest independently of the production container reader.
fn absent_container() -> Vec<u8> {
    let json = br#"{"version":"3.1.0","mangoFiles":{}}"#;
    let mut bytes = vec![4, 0, 0, 0, 44, 0, 0, 0, 40, 0, 0, 0, 35, 0, 0, 0];
    assert_eq!(json.len(), 35);
    bytes.extend_from_slice(json);
    bytes.push(0);
    bytes
}

#[test]
fn absence_and_corruption_have_distinct_statuses_and_preserve_previous_output() {
    let controls = Controls::new();
    let absent = controls.write("absent.LYS", &absent_container());
    let corrupt = controls.write("corrupt.lys", b"neutral corrupt document");
    let output = controls.write("existing.png", b"retain previous usable image");
    let options = RenderOptions::default();
    assert!(saved_preview::extract(&absent).unwrap().is_none());
    assert!(
        thumbnail(&absent, &options)
            .unwrap_err()
            .is::<saved_preview::NoSavedPreview>()
    );
    for mode in [vec![], vec!["--extract-saved-preview"]] {
        let missing = command(&absent, &output, &mode);
        assert_eq!(missing.status.code(), Some(3));
        assert_eq!(missing.stdout, [] as [u8; 0]);
        assert!(String::from_utf8_lossy(&missing.stderr).contains("no supported saved preview"));
        assert_eq!(fs::read(&output).unwrap(), b"retain previous usable image");
        let invalid = command(&corrupt, &output, &mode);
        assert_eq!(invalid.status.code(), Some(1));
        assert_eq!(invalid.stdout, [] as [u8; 0]);
        assert_eq!(fs::read(&output).unwrap(), b"retain previous usable image");
    }
    let forced = command(
        &fixture(ORIGINALS[0].name, "lys"),
        &output,
        &["--render-geometry"],
    );
    assert_eq!(forced.status.code(), Some(1));
    assert_eq!(forced.stdout, [] as [u8; 0]);
    assert_eq!(fs::read(output).unwrap(), b"retain previous usable image");
}
