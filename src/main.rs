//! Command-line preview rendering, saved-image extraction and PNG output.

use catwalk::{
    Result, formats,
    render::{RenderOptions, write_png},
    thumbnail,
};
use clap::Parser;
use std::{io::Write, path::PathBuf};

/// CLI arguments shared by thumbnail rendering and original-size extraction.
#[derive(Parser)]
#[command(
    version,
    about = "Render model preview thumbnails entirely in Rust, without a GPU or display server"
)]
struct Args {
    /// Model or image to preview. See --list-formats for importer coverage.
    input: Option<PathBuf>,
    /// PNG destination; use - to write PNG bytes to stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Square thumbnail size in pixels.
    #[arg(short, long, default_value_t = 512, value_parser = clap::value_parser!(u32).range(1..=4096))]
    size: u32,
    /// Override size with WIDTH,HEIGHT (F3D-style) or `WIDTHxHEIGHT`.
    #[arg(long)]
    resolution: Option<String>,
    /// Save with a transparent background.
    #[arg(long, alias = "no-background")]
    transparent: bool,
    /// Background color as RRGGBB or #RRGGBB.
    #[arg(long, default_value = "f5f7fa", value_parser = parse_color)]
    background: [u8; 3],
    /// Color for geometry without material/vertex colors.
    #[arg(long, default_value = "64a0dc", value_parser = parse_color)]
    color: [u8; 3],
    /// Supersampling factor for smoothing triangle edges.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(1..=4))]
    samples: u32,
    /// Extract a saved document preview at its original size, without canvas fitting.
    #[arg(long, conflicts_with_all = ["size", "resolution", "transparent", "background", "color", "samples"])]
    extract_saved_preview: bool,
    /// Print implemented readers and their limitations.
    #[arg(long, alias = "list-readers")]
    list_formats: bool,
}

/// Parse six ASCII hexadecimal color digits, with an optional leading hash.
fn parse_color(value: &str) -> std::result::Result<[u8; 3], String> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 || !hex.is_ascii() {
        return Err("color must have six hexadecimal digits".into());
    }
    let parse = |offset| {
        u8::from_str_radix(&hex[offset..offset + 2], 16)
            .map_err(|_| "color must have six hexadecimal digits".to_owned())
    };
    Ok([parse(0)?, parse(2)?, parse(4)?])
}

/// Resolve square size or explicit resolution and enforce the output axis limits.
fn dimensions(args: &Args) -> Result<(u32, u32)> {
    let Some(value) = &args.resolution else {
        return Ok((args.size, args.size));
    };
    let (width, height) = value
        .split_once([',', 'x'])
        .ok_or("resolution must be WIDTH,HEIGHT or WIDTHxHEIGHT")?;
    let pair: (u32, u32) = (width.parse()?, height.parse()?);
    if pair.0 == 0 || pair.1 == 0 || pair.0 > 4096 || pair.1 > 4096 {
        return Err("resolution dimensions must be between 1 and 4096".into());
    }
    Ok(pair)
}

/// Complete preview decoding and PNG encoding before opening the destination.
fn run(args: &Args) -> Result<()> {
    if args.list_formats {
        println!(
            "Implemented model readers: {}",
            formats::MODEL_EXTENSIONS.join(", ")
        );
        println!(
            "Implemented image readers: {}",
            formats::IMAGE_EXTENSIONS.join(", ")
        );
        println!(
            "Saved-preview readers: {}",
            catwalk::saved_preview::EXTENSIONS.join(", ")
        );
        println!(
            "Fixed-view surface/point/line and image previews. Readers support documented subsets."
        );
        println!("Full importer coverage and limitations: docs/format-coverage.md");
        return Ok(());
    }
    let input = args
        .input
        .as_ref()
        .ok_or("provide an input model or use --list-formats")?;
    let output = args
        .output
        .as_ref()
        .ok_or("provide --output thumbnail.png (or --output -)")?;
    if output != &PathBuf::from("-")
        && input.exists()
        && output.exists()
        && input.canonicalize()? == output.canonicalize()?
    {
        return Err("input and output must be different files".into());
    }
    // Finish extraction and encoding before opening an output file or writing
    // stdout, so absent or malformed carriers preserve previous previews.
    let mut bytes = Vec::new();
    if args.extract_saved_preview {
        if matches!(formats::extension(input)?.as_str(), "ipt" | "iam") {
            bytes = catwalk::saved_preview::inventor::extract_png(input)?
                .ok_or(catwalk::saved_preview::NoSavedPreview)?;
        } else {
            let image = catwalk::saved_preview::extract(input)?
                .ok_or(catwalk::saved_preview::NoSavedPreview)?;
            write_png(&image, &mut bytes)?;
        }
    } else {
        let (width, height) = dimensions(args)?;
        let options = RenderOptions {
            width,
            height,
            transparent: args.transparent,
            background: args.background,
            color: args.color,
            supersampling: args.samples,
        };
        write_png(&thumbnail(input, &options)?, &mut bytes)?;
    }
    if output == &PathBuf::from("-") {
        let stdout = std::io::stdout();
        let mut writer = std::io::BufWriter::new(stdout.lock());
        writer.write_all(&bytes)?;
        writer.flush()?;
    } else {
        std::fs::write(output, bytes)?;
    }
    Ok(())
}

/// Map CLI failures to diagnostics and a distinct absent-saved-preview exit code.
fn main() -> std::process::ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("catwalk: {error}");
            if error.is::<catwalk::saved_preview::NoSavedPreview>() {
                std::process::ExitCode::from(3)
            } else {
                std::process::ExitCode::FAILURE
            }
        }
    }
}
