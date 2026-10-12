//! Build the gallery from the exact images emitted by the format snapshot test.
#[path = "../tests/support/format_corpus.rs"]
/// Shared snapshot manifest validation, rendering and comparison helpers.
mod corpus;

use catwalk3d::Result;
use clap::Parser;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Parser)]
/// Gallery output destination, snapshot-report reuse and explicit local update options.
struct Args {
    /// Static site destination, suitable for a GitHub Pages artifact.
    #[arg(long, default_value = "target/gallery")]
    output: PathBuf,
    /// Use a completed snapshot report instead of rerendering the corpus.
    #[arg(long)]
    report: Option<PathBuf>,
    /// Explicitly regenerate the committed expected PNGs for local review.
    #[arg(long, conflicts_with = "report")]
    update_snapshots: bool,
    /// Render through this CLI executable; otherwise use the library.
    #[arg(long, conflicts_with = "report")]
    binary: Option<PathBuf>,
}

/// Collect fixture license, copyright and notice files for the published gallery.
fn collect_licenses(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect_licenses(&entry.path(), files)?;
        } else if entry.file_type()?.is_file() {
            let name = entry.file_name().to_string_lossy().to_ascii_uppercase();
            if name.contains("LICENSE") || name.contains("COPYRIGHT") || name.contains("NOTICE") {
                files.push(entry.path());
            }
        }
    }
    Ok(())
}

/// Copy original notice bytes and build a UTF-8 license index for the gallery.
fn copy_licenses(workspace: &Path, output: &Path) -> Result<()> {
    let mut files = vec![workspace.join("LICENSE")];
    collect_licenses(&workspace.join("tests/fixtures"), &mut files)?;
    files.sort();
    let mut text = String::from("Catwalk and thumbnail fixture licenses and notices\n\n");
    for source in files {
        let relative = source.strip_prefix(workspace)?;
        let destination = output.join("licenses").join(relative);
        std::fs::create_dir_all(destination.parent().ok_or("missing license parent")?)?;
        std::fs::copy(&source, destination)?;
        text.push_str(&relative.to_string_lossy().replace('\\', "/"));
        text.push_str("\n\n");
        let bytes = std::fs::read(source)?;
        let content = if let Ok(content) = std::str::from_utf8(&bytes) {
            content.to_owned()
        } else {
            // Esri's original NOTICE uses a Windows-1252 en dash. Keep
            // its original bytes above and normalize only this text index.
            if !bytes.iter().all(|&byte| byte.is_ascii() || byte == 0x96) {
                return Err("unsupported fixture license text encoding".into());
            }
            bytes
                .iter()
                .map(|&byte| {
                    if byte == 0x96 {
                        '–'
                    } else {
                        char::from(byte)
                    }
                })
                .collect()
        };
        text.push_str(&content);
        text.push_str("\n\n");
    }
    std::fs::write(output.join("licenses.txt"), text)?;
    Ok(())
}

/// Validate snapshot identities and fingerprints before copying their artifacts into the static gallery.
fn build(workspace: &Path, report_root: &Path, output: &Path) -> Result<()> {
    let cases = corpus::read_cases(workspace)?;
    let report_bytes = std::fs::read(report_root.join("report.json"))?;
    let report: Value = serde_json::from_slice(&report_bytes)?;
    let entries = report["cases"]
        .as_array()
        .ok_or("snapshot report cases are missing")?;
    if report["schema"] != 1
        || report["caseCount"].as_u64() != Some(cases.len() as u64)
        || entries.len() != cases.len()
    {
        return Err("snapshot report schema/count differs from the current corpus".into());
    }
    corpus::validate_report(workspace, report_root, &report)?;
    // Check both order and input identity so a stale report cannot represent
    // newly registered fixtures in a successful gallery deployment.
    for (case, entry) in cases.iter().zip(entries) {
        if entry["id"] != case.id
            || entry["input"] != case.input
            || entry["extension"] != case.extension
            || entry["kind"] != case.kind
            || entry["title"] != case.title
            || entry["provenance"] != case.provenance
        {
            return Err("snapshot report fixture identity differs from current corpus".into());
        }
    }
    std::fs::create_dir_all(output.join("images"))?;
    for entry in entries {
        for key in ["actual_url", "expected_url", "diff_url"] {
            let path = entry[key]
                .as_str()
                .ok_or("snapshot report image path is missing")?;
            // Report image names are derived from validated IDs. Enforce the
            // same exact form before copying artifacts into a public site.
            let id = entry["id"]
                .as_str()
                .ok_or("snapshot report ID is missing")?;
            let suffix = match key {
                "actual_url" => "actual",
                "expected_url" => "expected",
                _ => "diff",
            };
            if path != format!("images/{id}.{suffix}.png") {
                return Err("unexpected snapshot report image path".into());
            }
            let destination = output.join(path);
            if destination.is_file() {
                std::fs::remove_file(&destination)?;
            }
            let input = report_root.join(path);
            if input.is_file() {
                std::fs::copy(input, destination)?;
            } else if entry["status"] == "passed" {
                return Err(format!("passed report has missing image {path}").into());
            }
        }
    }
    std::fs::write(output.join("data.json"), report_bytes)?;
    std::fs::write(
        output.join("index.html"),
        include_str!("gallery/index.html"),
    )?;
    std::fs::write(output.join("style.css"), include_str!("gallery/style.css"))?;
    std::fs::write(output.join("app.js"), include_str!("gallery/app.js"))?;
    std::fs::write(output.join(".nojekyll"), b"")?;
    copy_licenses(workspace, output)?;
    println!(
        "Gallery: {} ({} examples)",
        output.join("index.html").display(),
        cases.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilding_a_failed_report_removes_old_successful_images() -> Result<()> {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
        let cases = corpus::read_cases(workspace)?;
        let directory = std::env::temp_dir().join(format!(
            "catwalk-gallery-stale-output-{}",
            std::process::id()
        ));
        if directory.exists() {
            std::fs::remove_dir_all(&directory)?;
        }
        let report_root = directory.join("report");
        let output = directory.join("site");
        std::fs::create_dir_all(&report_root)?;
        std::fs::create_dir_all(output.join("images"))?;
        // Exercise a real failed render report, including its settings and
        // input/artifact fingerprints, without needing to decode every model.
        let missing_binary = directory.join("missing-catwalk");
        let failures = corpus::run(workspace, &report_root, Some(&missing_binary), false)?;
        assert_eq!(failures.len(), cases.len());
        for suffix in ["actual", "expected", "diff"] {
            std::fs::write(
                output.join(format!("images/{}.{suffix}.png", cases[0].id)),
                b"old successful image",
            )?;
        }
        let result = build(workspace, &report_root, &output);
        let image_count = std::fs::read_dir(output.join("images"))?.count();
        std::fs::remove_dir_all(directory)?;
        result?;
        assert_eq!(image_count, 0, "failed cases must not retain stale images");
        Ok(())
    }
}

/// Run or reuse a verified snapshot report and build the gallery at the requested destination.
fn main() -> Result<()> {
    let args = Args::parse();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
    let artifacts = args
        .report
        .clone()
        .unwrap_or_else(|| corpus::artifact_directory(workspace));
    let failures = if args.report.is_some() {
        Vec::new()
    } else {
        corpus::run(
            workspace,
            &artifacts,
            args.binary.as_deref(),
            args.update_snapshots,
        )?
    };
    build(workspace, &artifacts, &args.output)?;
    let report: Value = serde_json::from_slice(&std::fs::read(artifacts.join("report.json"))?)?;
    let count = report["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["status"] != "passed")
        .count();
    if count != 0 {
        return Err(format!(
            "{count} snapshot failures; gallery contains the comparisons. {}",
            failures.join("\n")
        )
        .into());
    }
    Ok(())
}
