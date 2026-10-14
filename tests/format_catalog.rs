//! CLI discovery contract for downstream format checks and thumbnail settings.

use catwalk3d::{formats, saved_preview};
use serde_json::Value;
use std::{collections::BTreeSet, process::Command};

/// Launch the built executable without a display server or any input model.
fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_catwalk"));
    command
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR");
    command
}

/// Decode the entire stdout document and require a clean, successful exit.
fn catalog(flag: &str) -> Value {
    let output = command().args([flag, "--json"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stderr, [] as [u8; 0]);
    serde_json::from_slice(&output.stdout).expect("stdout must contain only one JSON document")
}

/// Read a JSON extension array while checking its normalization and ordering.
fn extension_list(value: &Value) -> Vec<&str> {
    let result: Vec<_> = value
        .as_array()
        .expect("extension array")
        .iter()
        .map(|entry| entry.as_str().expect("extension string"))
        .collect();
    assert!(result.iter().all(|extension| {
        !extension.is_empty()
            && extension
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    }));
    assert!(result.windows(2).all(|pair| pair[0] < pair[1]));
    result
}

/// Independently collect the advertised aliases from the library's reader registries.
fn registered(registries: &[&[&'static str]]) -> Vec<&'static str> {
    registries
        .iter()
        .flat_map(|registry| registry.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[test]
fn json_contains_every_registered_alias_and_the_executable_version() {
    let document = catalog("--list-formats");
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["catwalk_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        extension_list(&document["extensions"]),
        registered(&[
            formats::MODEL_EXTENSIONS,
            formats::IMAGE_EXTENSIONS,
            saved_preview::EXTENSIONS,
            formats::NATIVE_GEOMETRY_EXTENSIONS,
        ])
    );
    assert_eq!(document, catalog("--list-readers"));

    let version = command().arg("--version").output().unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        format!("catwalk {}", document["catwalk_version"].as_str().unwrap())
    );
}

#[test]
fn every_alias_has_a_readable_description_for_thumbnail_settings() {
    let document = catalog("--list-formats");
    let entries = document["formats"].as_array().unwrap();
    let described: Vec<_> = entries
        .iter()
        .map(|entry| {
            let description = entry["description"].as_str().expect("display description");
            assert_eq!(description, description.trim());
            assert_ne!(description, "");
            entry["extension"].as_str().unwrap()
        })
        .collect();
    assert_eq!(described, extension_list(&document["extensions"]));

    let descriptions: std::collections::BTreeMap<_, _> = entries
        .iter()
        .map(|entry| {
            (
                entry["extension"].as_str().unwrap(),
                entry["description"].as_str().unwrap(),
            )
        })
        .collect();
    for (extension, description) in [
        ("ipt", "Autodesk Inventor part"),
        ("iam", "Autodesk Inventor assembly"),
        ("sldprt", "SolidWorks part"),
        ("sldasm", "SolidWorks assembly"),
        ("skp", "SketchUp model"),
        ("3dm", "Rhino model"),
        ("max", "Autodesk 3ds Max scene"),
        ("bin", "TerraScan BIN point cloud"),
    ] {
        assert_eq!(descriptions[extension], description);
    }
    assert_eq!(descriptions["stp"], descriptions["step"]);
    assert_eq!(descriptions["jpeg"], descriptions["jpg"]);
    assert_ne!(descriptions["usd"], descriptions["usda"]);
    assert_ne!(descriptions["usda"], descriptions["usdc"]);
}

#[test]
fn category_sets_match_the_readers_and_preserve_overlap() {
    let document = catalog("--list-formats");
    for (category, registries) in [
        (
            "geometry",
            vec![
                formats::MODEL_EXTENSIONS,
                formats::NATIVE_GEOMETRY_EXTENSIONS,
            ],
        ),
        ("image", vec![formats::IMAGE_EXTENSIONS]),
        ("saved_preview", vec![saved_preview::EXTENSIONS]),
        ("native_geometry", vec![formats::NATIVE_GEOMETRY_EXTENSIONS]),
    ] {
        assert_eq!(
            extension_list(&document["categories"][category]),
            registered(&registries),
            "incorrect {category} capabilities"
        );
    }
    let geometry = extension_list(&document["categories"]["geometry"]);
    let images = extension_list(&document["categories"]["image"]);
    for extension in ["tif", "tiff"] {
        assert!(geometry.contains(&extension));
        assert!(images.contains(&extension));
    }
    let previews = extension_list(&document["categories"]["saved_preview"]);
    assert!(geometry.contains(&"blend"));
    assert!(previews.contains(&"blend"));
    assert!(images.contains(&"png"));
    assert!(!geometry.contains(&"png"));
}

#[test]
fn new_native_geometry_readers_keep_saved_preview_catalog_support() {
    let document = catalog("--list-formats");
    let extensions = extension_list(&document["extensions"]);
    let images = extension_list(&document["categories"]["image"]);
    for extension in ["skp", "3dm", "max", "sldprt", "sldasm"] {
        assert!(extensions.contains(&extension));
        for category in ["geometry", "native_geometry", "saved_preview"] {
            assert!(
                extension_list(&document["categories"][category]).contains(&extension),
                ".{extension} must advertise {category} support"
            );
        }
        assert!(!images.contains(&extension));
    }
}

#[test]
fn default_human_readable_output_is_unchanged() {
    let expected = format!(
        "Implemented model readers: {}\n\
         Implemented image readers: {}\n\
         Saved-preview readers: {}\n\
         Native document geometry readers (qualified subsets): {}\n\
         Fixed-view surface/point/line and image previews. Readers support documented subsets.\n\
         Full importer coverage and limitations: docs/format-coverage.md\n",
        formats::MODEL_EXTENSIONS.join(", "),
        formats::IMAGE_EXTENSIONS.join(", "),
        saved_preview::EXTENSIONS.join(", "),
        formats::NATIVE_GEOMETRY_EXTENSIONS.join(", "),
    );
    for flag in ["--list-formats", "--list-readers"] {
        let output = command().arg(flag).output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stderr, [] as [u8; 0]);
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    }
}

#[test]
fn json_requires_format_discovery_and_reports_errors_only_to_stderr() {
    for args in [
        vec!["--json"],
        vec!["missing.stl", "--json", "--output", "-"],
    ] {
        let output = command().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.stdout, [] as [u8; 0]);
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        assert!(diagnostic.contains("--list-formats"));
    }
}

#[test]
fn discovery_does_not_read_models_or_open_output_destinations() {
    let output = command()
        .args([
            "--list-formats",
            "--json",
            "nonexistent-input.stl",
            "--output",
        ])
        // Opening a directory as an image output would fail; discovery must
        // return its JSON on stdout without attempting either file operation.
        .arg(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stderr, [] as [u8; 0]);
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document, catalog("--list-formats"));
}

#[cfg(target_os = "linux")]
#[test]
fn json_write_failures_exit_unsuccessfully_with_a_stderr_diagnostic() {
    let destination = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let output = command()
        .args(["--list-formats", "--json"])
        .stdout(destination)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .starts_with("catwalk: ")
    );
}
