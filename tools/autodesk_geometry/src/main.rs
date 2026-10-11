//! Command-line interface for the isolated native `BRep` geometry experiment.
use catwalk::render::{RenderOptions, render};
use catwalk_autodesk_geometry::load_geometry;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// Resolve an existing destination or combine its canonical parent with a new filename.
fn destination_identity(path: &Path) -> catwalk::Result<PathBuf> {
    if path.exists() {
        return Ok(fs::canonicalize(path)?);
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok(fs::canonicalize(parent)?.join(path.file_name().ok_or("output path has no filename")?))
}

/// Detect path aliases and, on Unix, existing hard links that identify one destination.
fn same_destination(a: &Path, b: &Path) -> catwalk::Result<bool> {
    if destination_identity(a)? == destination_identity(b)? {
        return Ok(true);
    }
    #[cfg(unix)]
    if let (Ok(a), Ok(b)) = (fs::metadata(a), fs::metadata(b)) {
        use std::os::unix::fs::MetadataExt;
        if a.dev() == b.dev() && a.ino() == b.ino() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Require a PNG destination and prevent output/report aliases from overwriting the input.
fn verify_destinations(input: &Path, output: &Path, report: Option<&Path>) -> catwalk::Result<()> {
    if !output
        .extension()
        .is_some_and(|v| v.as_encoded_bytes().eq_ignore_ascii_case(b"png"))
    {
        return Err("output path must have a .png extension".into());
    }
    if same_destination(input, output)? {
        return Err("PNG output would overwrite the input file".into());
    }
    if let Some(report) = report {
        if same_destination(input, report)? {
            return Err("report output would overwrite the input file".into());
        }
        if same_destination(output, report)? {
            return Err("PNG and report outputs must be different files".into());
        }
    }
    Ok(())
}

/// Decode a requested part, render a fixed thumbnail and optionally write its geometry report.
fn main() -> catwalk::Result<()> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    let [input, output, rest @ ..] = args.as_slice() else {
        return Err("usage: autodesk-preview INPUT.{f3d,ipt} OUTPUT.png [REPORT.json]".into());
    };
    if rest.len() > 1 {
        return Err("too many arguments".into());
    }
    verify_destinations(
        Path::new(input),
        Path::new(output),
        rest.first().map(Path::new),
    )?;
    let (scene, report) = load_geometry(Path::new(input))?;
    let options = RenderOptions {
        width: 256,
        height: 256,
        transparent: true,
        supersampling: 2,
        ..RenderOptions::default()
    };
    render(&scene, &options)?.save(Path::new(output))?;
    if let Some(destination) = rest.first() {
        fs::write(destination, serde_json::to_vec_pretty(&report)?)?;
    }
    println!(
        "{} bodies, {} faces, {} rendered triangles",
        report["source_bodies"], report["source_faces"], report["render_triangles"]
    );
    Ok(())
}

#[cfg(test)]
/// Destination-identity checks that preserve the source document and separate output artifacts.
mod tests {
    use super::verify_destinations;
    use std::fs;

    #[test]
    fn destinations_preserve_input_and_keep_png_separate_from_report() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("part.ipt");
        let output = directory.path().join("preview.png");
        fs::write(&input, b"original geometry").unwrap();
        assert!(verify_destinations(&input, &output, Some(&input)).is_err());
        assert!(verify_destinations(&input, &output, Some(&output)).is_err());
        assert!(verify_destinations(&input, &directory.path().join("bad.jpg"), None).is_err());
        #[cfg(unix)]
        {
            fs::hard_link(&input, &output).unwrap();
            assert!(verify_destinations(&input, &output, None).is_err());
        }
        assert_eq!(fs::read(input).unwrap(), b"original geometry");
    }
}
