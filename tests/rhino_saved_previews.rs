//! Unchanged Rhino producer sources, independently decoded pixels and headless CLI controls.

#[path = "support/digest_hex.rs"]
mod digest_hex;

use catwalk3d::{Result, saved_preview};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Resolve one retained official openNURBS source or independent saved-image reference.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/rhino")
        .join(name)
}

/// Remove each control directory after its source/output assertions finish.
struct Temporary {
    /// Process-specific directory owned only by this test.
    path: PathBuf,
}

impl Temporary {
    /// Create an isolated directory for one uniquely named test.
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("catwalk-rhino-{}-{label}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn original_sources_and_independent_preview_pixels_remain_exact() -> Result<()> {
    let manifest: Value = serde_json::from_slice(&fs::read(fixture("sources.json"))?)?;
    for source in manifest["files"].as_array().unwrap() {
        let name = source["file"].as_str().unwrap();
        let path = fixture(name);
        let bytes = fs::read(&path)?;
        let digest = format!("{:x}", digest_hex::Hex(Sha256::digest(&bytes)));
        assert_eq!(
            digest,
            source["sha256"].as_str().unwrap(),
            "original source: {name}"
        );
        let actual = saved_preview::extract(&path)?;
        if let Some(reference) = source["preview"].as_str() {
            let actual = actual.unwrap();
            let expected = image::open(fixture(reference))?.into_rgba8();
            assert_eq!(
                actual, expected,
                "independently extracted saved pixels: {name}"
            );
            assert_eq!(
                (u64::from(actual.width()), u64::from(actual.height())),
                (
                    source["width"].as_u64().unwrap(),
                    source["height"].as_u64().unwrap()
                )
            );
            assert_eq!(
                format!("{:x}", digest_hex::Hex(Sha256::digest(actual.as_raw()))),
                source["rgba_sha256"].as_str().unwrap()
            );
        } else {
            assert!(actual.is_none(), "absent preview: {name}");
        }
        assert_eq!(
            fs::read(&path)?,
            bytes,
            "extraction must preserve the design: {name}"
        );
    }
    Ok(())
}

#[test]
fn cli_extracts_original_pixels_and_fits_the_automatic_canvas_without_a_display() -> Result<()> {
    let temporary = Temporary::new("pixels");
    for name in ["v3_DeformSpheres.3dm", "v8_caps_and_joins.3dm"] {
        let output = Command::new(env!("CARGO_BIN_EXE_catwalk"))
            .args([
                fixture(name).as_os_str(),
                "--extract-saved-preview".as_ref(),
                "--output".as_ref(),
                "-".as_ref(),
            ])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_RUNTIME_DIR")
            .output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stderr, [] as [u8; 0]);
        let image = image::load_from_memory_with_format(&output.stdout, image::ImageFormat::Png)?
            .into_rgba8();
        assert_eq!(
            image,
            image::open(fixture(&name.replace(".3dm", ".png")))?.into_rgba8()
        );
        let destination = temporary.path.join(format!("{name}.png"));
        let output = Command::new(env!("CARGO_BIN_EXE_catwalk"))
            .arg(fixture(name))
            .args(["--size", "128", "--transparent", "--output"])
            .arg(&destination)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let image = image::open(destination)?.into_rgba8();
        assert_eq!(image.dimensions(), (128, 128));
        assert!(image.pixels().any(|pixel| pixel[3] != 0));
        assert!(image.pixels().any(|pixel| pixel[3] == 0));
    }
    Ok(())
}

#[test]
fn absence_corruption_and_geometry_forcing_preserve_existing_outputs() -> Result<()> {
    let temporary = Temporary::new("failures");
    let destination = temporary.path.join("existing.png");
    fs::write(&destination, b"existing thumbnail")?;
    for flag in ["--extract-saved-preview", "--render-geometry"] {
        let output = Command::new(env!("CARGO_BIN_EXE_catwalk"))
            .arg(fixture("my_mesh.3dm"))
            .args([flag, "--output"])
            .arg(&destination)
            .output()?;
        assert_eq!(
            output.status.code(),
            Some(if flag == "--extract-saved-preview" {
                3
            } else {
                1
            })
        );
        assert_eq!(fs::read(&destination)?, b"existing thumbnail");
    }
    let mut source = fs::read(fixture("v3_DeformSpheres.3dm"))?;
    // The original V3 fixture's addressed preview record starts at 1061,
    // has an eight-byte header and a 7124-byte payload including its CRC.
    source[1061 + 8 + 7124 - 1] ^= 1;
    let corrupt = temporary.path.join("corrupt.3DM");
    fs::write(&corrupt, &source)?;
    let output = Command::new(env!("CARGO_BIN_EXE_catwalk"))
        .arg(&corrupt)
        .args(["--extract-saved-preview", "--output"])
        .arg(&destination)
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("checksum"));
    assert_eq!(fs::read(&destination)?, b"existing thumbnail");
    assert_eq!(fs::read(corrupt)?, source);
    Ok(())
}

#[test]
fn oversized_sources_and_properties_fail_before_payload_allocation() -> Result<()> {
    let temporary = Temporary::new("limits");
    let source = temporary.path.join("oversized.3dm");
    let file = fs::File::create(&source)?;
    file.set_len(512 * 1024 * 1024 + 1)?;
    assert!(
        saved_preview::extract(&source)
            .unwrap_err()
            .to_string()
            .contains("512 MiB")
    );
    let mut bytes = b"3D Geometry File Format       80".to_vec();
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&0x1000_0014_u32.to_le_bytes());
    bytes.extend_from_slice(&(16 * 1024 * 1024 + 1_u64).to_le_bytes());
    fs::write(&source, bytes)?;
    assert!(
        saved_preview::extract(&source)
            .unwrap_err()
            .to_string()
            .contains("bounded")
    );
    Ok(())
}
