//! Qualification tool for saved-thumbnail carriers. Research readers are kept
//! separate from the production format registry until their coverage is tested.
#[path = "saved_previews/autodesk.rs"]
mod autodesk;
#[path = "saved_previews/blend_dwg.rs"]
mod blend_dwg;
#[path = "saved_previews/lightburn.rs"]
mod lightburn;

use catwalk::{Result, render::write_png};
use std::path::Path;

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 2 {
        return Err("usage: cargo run --example inspect_saved_preview -- INPUT OUTPUT.png".into());
    }
    let path = Path::new(&arguments[0]);
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .ok_or("input has no extension")?
        .to_ascii_lowercase();
    let image = match extension.as_str() {
        "lbrn" | "lbrn2" => lightburn::extract(path)?,
        "blend" => blend_dwg::extract_blend(path)?,
        "dwg" => blend_dwg::extract_dwg(path)?,
        "f3d" => autodesk::extract_fusion(path)?,
        _ => return Err(format!("no qualified research reader for .{extension}").into()),
    }
    .ok_or("file contains no supported saved preview")?;
    let mut png = Vec::new();
    write_png(&image, &mut png)?;
    std::fs::write(&arguments[1], png)?;
    println!(
        "Extracted {} x {} saved preview",
        image.width(),
        image.height()
    );
    Ok(())
}
