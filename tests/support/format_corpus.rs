//! Shared corpus and decoded-pixel comparison used by CI and the Pages gallery.
use catwalk::{
    Result, formats,
    render::{RenderOptions, write_png},
};
use image::{Rgba, RgbaImage};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
};

/// Fixed corpus thumbnail width in pixels.
pub const WIDTH: u32 = 256;
/// Fixed corpus thumbnail height in pixels.
pub const HEIGHT: u32 = 256;
/// Maximum accepted premultiplied RGBA channel difference in snapshot comparisons.
pub const CHANNEL_TOLERANCE: u8 = 2;

/// Describe the deterministic render and comparison settings recorded in reports.
pub fn render_settings() -> Value {
    json!({"width":WIDTH,"height":HEIGHT,"transparent":true,"samples":2,"channelTolerance":CHANNEL_TOLERANCE})
}

/// Collect regular fingerprint inputs, recording missing paths and rejecting symlinks.
fn collect_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Absence is fingerprinted as well, including an entirely missing
            // baseline directory. Individual cases still report their errors.
            files.push(path.to_owned());
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        return Err(format!("snapshot input tree contains a symlink: {}", path.display()).into());
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            collect_files(&entry?.path(), files)?;
        }
    } else if metadata.is_file() {
        files.push(path.to_owned());
    } else {
        return Err(format!("snapshot input is not a regular file: {}", path.display()).into());
    }
    Ok(())
}

/// Convert an input path to a portable UTF-8 name relative to the fingerprint root.
fn relative_name(root: &Path, path: &Path) -> Result<String> {
    // Portable names make identical checked-out bytes hash identically on all
    // platforms; absolute workspace paths and directory timestamps are omitted.
    path.strip_prefix(root)?
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| "snapshot input name is not UTF-8".into())
        })
        .collect::<Result<Vec<_>>>()
        .map(|parts| parts.join("/"))
}

/// Hash sorted portable names, existence, lengths and complete contents of input trees.
fn fingerprint(root: &Path, paths: &[&str]) -> Result<String> {
    let mut files = Vec::new();
    for path in paths {
        collect_files(&root.join(path), &mut files)?;
    }
    let mut names = files
        .into_iter()
        .map(|path| Ok((relative_name(root, &path)?, path)))
        .collect::<Result<Vec<_>>>()?;
    names.sort_by(|a, b| a.0.cmp(&b.0));
    let mut digest = Sha256::new();
    digest.update(b"catwalk-snapshot-files-v1\0");
    let mut buffer = vec![0_u8; 64 * 1024];
    for (name, path) in names {
        digest.update((name.len() as u64).to_be_bytes());
        digest.update(name.as_bytes());
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                digest.update(b"missing\0");
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        digest.update(b"file\0");
        digest.update(file.metadata()?.len().to_be_bytes());
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Include every corpus file, so detached buffers, materials and scalar arrays
/// cannot change unnoticed merely because the top-level input stayed the same.
///
/// # Errors
/// Returns an error if an input tree cannot be read or contains a symlink, special file
/// or non-UTF-8 path that cannot be fingerprinted portably.
pub fn verification_context(workspace: &Path) -> Result<Value> {
    Ok(json!({
        "schema": 1,
        "corpus_sha256": fingerprint(workspace, &["tests/fixtures/formats"] )?,
        "baselines_sha256": fingerprint(workspace, &["tests/snapshots/formats"] )?,
        "renderer_sha256": fingerprint(workspace, &["src", "Cargo.toml", "Cargo.lock", "tests/support/format_corpus.rs"] )?,
    }))
}

/// Read the optional CI commit identifier and reject non-hexadecimal or incorrectly sized values.
fn tested_commit() -> Result<Option<String>> {
    match std::env::var("GITHUB_SHA") {
        Ok(commit)
            if matches!(commit.len(), 40 | 64)
                && commit.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            Ok(Some(commit))
        }
        Ok(_) => Err("GITHUB_SHA must be a hexadecimal commit ID".into()),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// A reused report must describe these inputs, baselines, implementation and
/// settings, and the same image artifacts that the snapshot runner emitted.
///
/// # Errors
/// Returns an error for unreadable fingerprint inputs, changed settings, stale corpus or renderer
/// bytes, changed image artifacts, or a report from a different CI commit.
pub fn validate_report(workspace: &Path, artifacts: &Path, report: &Value) -> Result<()> {
    if report["settings"] != render_settings() {
        return Err("snapshot report uses different render/comparison settings".into());
    }
    if report["verification"] != verification_context(workspace)? {
        return Err("snapshot report is stale: corpus, baselines or renderer changed".into());
    }
    if report["artifact_images_sha256"] != fingerprint(artifacts, &["images"])? {
        return Err("snapshot report image artifacts changed after comparison".into());
    }
    if let Some(commit) = tested_commit()?
        && report["commit"].as_str() != Some(&commit)
    {
        return Err("snapshot report belongs to a different GitHub commit".into());
    }
    Ok(())
}

#[derive(Clone, Debug)]
/// Validated corpus entry identifying one input and its committed image baseline.
pub struct Case {
    /// Stable, validated identifier used for artifact and baseline filenames.
    pub id: String,
    /// Lowercase advertised input extension.
    pub extension: String,
    /// Human-readable gallery description.
    pub title: String,
    /// Validated relative input path inside the corpus.
    pub input: String,
    /// Manifest category: model, image, saved-preview or native-geometry.
    pub kind: String,
    /// Authorship, source and licensing description.
    pub provenance: String,
}

/// Decoded, premultiplied RGBA difference statistics for one image pair.
pub struct Comparison {
    /// Largest absolute difference among all premultiplied RGBA channels.
    pub max_delta: u8,
    /// Number of pixels with at least one differing premultiplied channel.
    pub changed_pixels: usize,
    /// Mean absolute channel difference across the complete image.
    pub mean_delta: f64,
}

impl Comparison {
    /// Whether the maximum channel difference stays within the committed corpus tolerance.
    #[must_use]
    pub fn matches(&self) -> bool {
        self.max_delta <= CHANNEL_TOLERANCE
    }
    /// Serialize the comparison metrics for the snapshot report and gallery.
    fn json(&self) -> Value {
        json!({"max_delta":self.max_delta,"changed_pixels":self.changed_pixels,"mean_delta":self.mean_delta})
    }
}

/// Read a required nonempty string field from a corpus manifest entry.
fn string(value: &Value, key: &str) -> Result<String> {
    let value = value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("corpus field {key} must be a nonempty string"))?;
    Ok(value.to_owned())
}

/// Read the manifest and validate input paths, fixture identities and complete extension coverage.
///
/// # Errors
/// Returns an error for I/O or JSON failures, unsupported schema or settings, invalid or duplicate
/// identifiers, missing or escaping inputs, mismatched kinds, or incomplete extension coverage.
pub fn read_cases(workspace: &Path) -> Result<Vec<Case>> {
    let root = workspace.join("tests/fixtures/formats");
    let manifest: Value = serde_json::from_slice(&std::fs::read(root.join("manifest.json"))?)?;
    if manifest["schema"] != 1
        || manifest["render"]
            != json!({"width":WIDTH,"height":HEIGHT,"transparent":true,"samples":2})
    {
        return Err("unsupported format corpus schema or render settings".into());
    }
    let entries = manifest["cases"]
        .as_array()
        .ok_or("corpus cases must be an array")?;
    let mut ids = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut native_seen = BTreeSet::new();
    let mut cases = Vec::new();
    for entry in entries {
        let case = Case {
            id: string(entry, "id")?,
            extension: string(entry, "extension")?,
            title: string(entry, "title")?,
            input: string(entry, "input")?,
            kind: string(entry, "kind")?,
            provenance: string(entry, "provenance")?,
        };
        if !case
            .id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_')
            || !ids.insert(case.id.clone())
        {
            return Err(format!("invalid or duplicate fixture ID: {}", case.id).into());
        }
        let path = Path::new(&case.input);
        if path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(format!(
                "fixture path must be relative and remain inside the corpus: {}",
                case.input
            )
            .into());
        }
        let input = root.join(path);
        if !input.is_file()
            || !input.canonicalize()?.starts_with(root.canonicalize()?)
            || formats::extension(&input)? != case.extension
        {
            return Err(format!("missing fixture or extension/path mismatch: {}", case.id).into());
        }
        let kind = if formats::IMAGE_EXTENSIONS.contains(&case.extension.as_str()) {
            "image"
        } else if catwalk::saved_preview::EXTENSIONS.contains(&case.extension.as_str()) {
            "saved-preview"
        } else {
            "model"
        };
        let native = case.kind == "native-geometry"
            && formats::NATIVE_GEOMETRY_EXTENSIONS.contains(&case.extension.as_str());
        if case.kind != kind && !native {
            return Err(format!("wrong fixture kind: {}", case.id).into());
        }
        if native {
            native_seen.insert(case.extension.as_str().to_owned());
        }
        seen.insert(case.extension.clone());
        cases.push(case);
    }
    let advertised: BTreeSet<String> = formats::MODEL_EXTENSIONS
        .iter()
        .chain(formats::IMAGE_EXTENSIONS)
        .chain(catwalk::saved_preview::EXTENSIONS)
        .map(|s| (*s).to_owned())
        .collect();
    if seen != advertised {
        return Err(format!(
            "fixture coverage mismatch; missing {:?}, unexpected {:?}",
            advertised.difference(&seen).collect::<Vec<_>>(),
            seen.difference(&advertised).collect::<Vec<_>>()
        )
        .into());
    }
    let native_advertised: BTreeSet<String> = formats::NATIVE_GEOMETRY_EXTENSIONS
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    if native_seen != native_advertised {
        return Err("every native geometry reader needs a fresh-render snapshot case".into());
    }
    if cases.len() > 512 {
        return Err("format corpus exceeds the fixture limit".into());
    }
    Ok(cases)
}

/// Convert straight-alpha byte colors to rounded premultiplied channels for stable comparisons.
#[allow(
    clippy::cast_possible_truncation,
    reason = "Rounded premultiplication of two byte channels always produces a value from zero to 255"
)]
fn premultiplied(pixel: Rgba<u8>) -> [u8; 4] {
    std::array::from_fn(|i| {
        if i == 3 {
            pixel[3]
        } else {
            ((u16::from(pixel[i]) * u16::from(pixel[3]) + 127) / 255) as u8
        }
    })
}

/// Compare decoded images in premultiplied RGBA and create an amplified opaque difference image.
///
/// # Errors
/// Returns an error when the expected and actual image dimensions differ.
#[allow(
    clippy::cast_precision_loss,
    reason = "The reported mean channel delta is an approximate floating-point statistic"
)]
pub fn compare(expected: &RgbaImage, actual: &RgbaImage) -> Result<(Comparison, RgbaImage)> {
    if expected.dimensions() != actual.dimensions() {
        return Err(format!(
            "snapshot dimensions differ: {:?} versus {:?}",
            expected.dimensions(),
            actual.dimensions()
        )
        .into());
    }
    let mut max_delta = 0;
    let mut changed_pixels = 0;
    let mut total_delta = 0u64;
    let mut difference = RgbaImage::new(actual.width(), actual.height());
    for ((expected, actual), diff) in expected
        .pixels()
        .zip(actual.pixels())
        .zip(difference.pixels_mut())
    {
        let expected = premultiplied(*expected);
        let actual = premultiplied(*actual);
        let deltas: [u8; 4] = std::array::from_fn(|i| expected[i].abs_diff(actual[i]));
        let maximum = deltas.into_iter().fold(0, u8::max);
        max_delta = max_delta.max(maximum);
        changed_pixels += usize::from(maximum != 0);
        total_delta += deltas.iter().map(|&v| u64::from(v)).sum::<u64>();
        *diff = Rgba([
            maximum.saturating_mul(8),
            deltas[3].saturating_mul(4),
            0,
            255,
        ]);
    }
    Ok((
        Comparison {
            max_delta,
            changed_pixels,
            mean_delta: total_delta as f64
                / (f64::from(actual.width()) * f64::from(actual.height()) * 4.),
        },
        difference,
    ))
}

/// Decode a PNG artifact as RGBA8 for snapshot comparison.
fn load_png(path: &Path) -> Result<RgbaImage> {
    Ok(
        image::load_from_memory_with_format(&std::fs::read(path)?, image::ImageFormat::Png)?
            .to_rgba8(),
    )
}

/// Create parent directories and write an RGBA snapshot artifact as PNG.
fn save_png(path: &Path, image: &RgbaImage) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_png(image, std::fs::File::create(path)?)
}

/// Render a corpus input through the library or a silent headless CLI and return its decoded PNG.
fn render(
    case: &Case,
    workspace: &Path,
    output: &Path,
    binary: Option<&Path>,
) -> Result<RgbaImage> {
    let input = workspace.join("tests/fixtures/formats").join(&case.input);
    if let Some(binary) = binary {
        let mut command = Command::new(binary);
        if case.kind == "native-geometry" {
            command.arg("--render-geometry");
        }
        let result = command
            .arg(&input)
            .arg("--output")
            .arg(output)
            .args(["--resolution", "256,256", "--transparent", "--samples", "2"])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_RUNTIME_DIR")
            .output()?;
        if !result.status.success() || !result.stderr.is_empty() || !result.stdout.is_empty() {
            return Err(format!(
                "thumbnail CLI failed: status {}; stdout {}; stderr {}",
                result.status,
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            )
            .into());
        }
        load_png(output)
    } else {
        let options = RenderOptions {
            width: WIDTH,
            height: HEIGHT,
            transparent: true,
            supersampling: 2,
            ..RenderOptions::default()
        };
        let image = if case.kind == "native-geometry" {
            catwalk::geometry_thumbnail(&input, &options)?
        } else {
            catwalk::thumbnail(&input, &options)?
        };
        save_png(output, &image)?;
        Ok(image)
    }
}

/// Run every registered fixture, recording all failures rather than stopping at
/// the first. Baselines are immutable unless an explicit local update is asked.
///
/// # Errors
/// Returns an error for an invalid manifest, artifact or baseline I/O failures, an invalid CI commit
/// identifier, or input changes during rendering. Per-case render and comparison failures are
/// recorded in the report and returned in the failure list.
pub fn run(
    workspace: &Path,
    artifacts: &Path,
    binary: Option<&Path>,
    update: bool,
) -> Result<Vec<String>> {
    let cases = read_cases(workspace)?;
    let images = artifacts.join("images");
    std::fs::create_dir_all(&images)?;
    let snapshots = workspace.join("tests/snapshots/formats");
    if update {
        std::fs::create_dir_all(&snapshots)?;
    }
    let initial = verification_context(workspace)?;
    let commit = tested_commit()?;
    let source_ref = commit.as_deref().unwrap_or("main");
    let mut entries = Vec::new();
    let mut failures = Vec::new();
    for case in &cases {
        let actual_path = images.join(format!("{}.actual.png", case.id));
        let expected_path = images.join(format!("{}.expected.png", case.id));
        let diff_path = images.join(format!("{}.diff.png", case.id));
        // Remove only this case's old artifacts so stale images cannot masquerade
        // as a successful current render after a decoding failure.
        for path in [&actual_path, &expected_path, &diff_path] {
            if path.exists() {
                std::fs::remove_file(path)?;
            }
        }
        let mut entry = json!({"id":case.id,"extension":case.extension,"title":case.title,"kind":case.kind,"provenance":case.provenance,"input":case.input,"input_url":format!("https://github.com/cmeister2/catwalk/blob/{source_ref}/tests/fixtures/formats/{}",case.input),"actual_url":format!("images/{}.actual.png",case.id),"expected_url":format!("images/{}.expected.png",case.id),"diff_url":format!("images/{}.diff.png",case.id)});
        let result: Result<Comparison> = (|| {
            let actual = render(case, workspace, &actual_path, binary)?;
            if actual.dimensions() != (WIDTH, HEIGHT) || !actual.pixels().any(|p| p[3] != 0) {
                return Err("thumbnail has wrong dimensions or no visible content".into());
            }
            let baseline = snapshots.join(format!("{}.png", case.id));
            if update {
                save_png(&baseline, &actual)?;
            }
            if !baseline.is_file() {
                return Err(
                    format!("missing committed PNG baseline: {}", baseline.display()).into(),
                );
            }
            let expected = load_png(&baseline)?;
            save_png(&expected_path, &expected)?;
            let (comparison, difference) = compare(&expected, &actual)?;
            save_png(&diff_path, &difference)?;
            Ok(comparison)
        })();
        match result {
            Ok(comparison) => {
                entry["metrics"] = comparison.json();
                if comparison.matches() {
                    entry["status"] = json!("passed");
                } else {
                    entry["status"] = json!("changed");
                    let error = format!(
                        "{} (.{}) changed: maximum premultiplied RGBA delta {}, {} differing pixels; tolerance {}",
                        case.id,
                        case.extension,
                        comparison.max_delta,
                        comparison.changed_pixels,
                        CHANNEL_TOLERANCE
                    );
                    entry["error"] = json!(error);
                    failures.push(error);
                }
            }
            Err(error) => {
                let error = format!("{} (.{}): {error}", case.id, case.extension);
                entry["status"] = json!("error");
                entry["error"] = json!(error);
                failures.push(error);
            }
        }
        entries.push(entry);
    }
    let extension_count = cases
        .iter()
        .map(|case| &case.extension)
        .collect::<BTreeSet<_>>()
        .len();
    let verification = verification_context(workspace)?;
    if initial["corpus_sha256"] != verification["corpus_sha256"]
        || initial["renderer_sha256"] != verification["renderer_sha256"]
        || (!update && initial["baselines_sha256"] != verification["baselines_sha256"])
    {
        return Err("snapshot inputs changed while the corpus was rendering".into());
    }
    let report = json!({"schema":1,"extensionCount":extension_count,"caseCount":cases.len(),"settings":render_settings(),"verification":verification,"artifact_images_sha256":fingerprint(artifacts, &["images"] )?,"commit":commit,"cases":entries});
    let mut bytes = serde_json::to_vec_pretty(&report)?;
    bytes.push(b'\n');
    std::fs::write(artifacts.join("report.json"), bytes)?;
    Ok(failures)
}

/// Resolve the artifact-directory override or use the workspace snapshot target directory.
pub fn artifact_directory(workspace: &Path) -> PathBuf {
    std::env::var_os("CATWALK_SNAPSHOT_ARTIFACTS")
        .map_or_else(|| workspace.join("target/format-snapshots"), PathBuf::from)
}

#[cfg(test)]
mod report_reuse_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Workspace(PathBuf);

    impl Workspace {
        fn new() -> Result<Self> {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let directory = loop {
                let candidate = std::env::temp_dir().join(format!(
                    "catwalk-report-reuse-{}-{}",
                    std::process::id(),
                    NEXT_ID.fetch_add(1, Ordering::Relaxed)
                ));
                match std::fs::create_dir(&candidate) {
                    Ok(()) => break candidate,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
            };
            let workspace = Self(directory);
            for directory in [
                "tests/fixtures/formats",
                "tests/snapshots/formats",
                "src",
                "report/images",
            ] {
                std::fs::create_dir_all(workspace.0.join(directory))?;
            }
            std::fs::write(
                workspace.0.join("tests/fixtures/formats/mesh.gltf"),
                r#"{"asset":{"version":"2.0"},"buffers":[{"uri":"mesh.bin","byteLength":36}],"bufferViews":[{"buffer":0,"byteLength":36}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],"nodes":[{"mesh":0}]}"#,
            )?;
            let vertices: Vec<_> = [0., 0., 0., 1., 0., 0., 0., 1., 0.]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect();
            std::fs::write(
                workspace.0.join("tests/fixtures/formats/mesh.bin"),
                vertices,
            )?;
            std::fs::write(workspace.0.join("src/lib.rs"), "// renderer version A\n")?;
            let image = RgbaImage::from_pixel(2, 2, Rgba([100, 50, 25, 255]));
            save_png(
                &workspace.0.join("tests/snapshots/formats/mesh.png"),
                &image,
            )?;
            save_png(&workspace.0.join("report/images/mesh.actual.png"), &image)?;
            Ok(workspace)
        }

        fn report(&self) -> Result<Value> {
            Ok(json!({
                "settings": render_settings(),
                "verification": verification_context(&self.0)?,
                "artifact_images_sha256": fingerprint(&self.0.join("report"), &["images"] )?,
                "commit": tested_commit()?,
            }))
        }

        fn validate(&self, report: &Value) -> Result<()> {
            validate_report(&self.0, &self.0.join("report"), report)
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reused_report_rejects_changed_external_buffer_baseline_and_renderer() -> Result<()> {
        let workspace = Workspace::new()?;
        let report = workspace.report()?;
        workspace.validate(&report)?;
        // The top-level glTF and manifest identity are unchanged; a referenced
        // buffer alone changes the geometry that a new render would consume.
        for path in [
            "tests/fixtures/formats/mesh.bin",
            "tests/snapshots/formats/mesh.png",
            "src/lib.rs",
        ] {
            let path = workspace.0.join(path);
            let original = std::fs::read(&path)?;
            let mut changed = original.clone();
            changed[0] ^= 1;
            std::fs::write(&path, changed)?;
            assert!(
                workspace
                    .validate(&report)
                    .unwrap_err()
                    .to_string()
                    .contains("stale")
            );
            std::fs::write(&path, original)?;
            workspace.validate(&report)?;
        }
        Ok(())
    }

    #[test]
    fn reused_report_rejects_changed_settings_and_image_artifacts() -> Result<()> {
        let workspace = Workspace::new()?;
        let report = workspace.report()?;
        let mut different_settings = report.clone();
        different_settings["settings"]["channelTolerance"] = json!(255);
        assert!(
            workspace
                .validate(&different_settings)
                .unwrap_err()
                .to_string()
                .contains("settings")
        );
        let changed = RgbaImage::from_pixel(2, 2, Rgba([0, 0, 0, 0]));
        save_png(&workspace.0.join("report/images/mesh.actual.png"), &changed)?;
        assert!(
            workspace
                .validate(&report)
                .unwrap_err()
                .to_string()
                .contains("artifacts")
        );
        Ok(())
    }
}
