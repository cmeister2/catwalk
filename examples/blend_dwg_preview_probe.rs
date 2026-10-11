//! Temporary research probe; no production format dispatch is changed.
#[path = "saved_previews/blend_dwg.rs"]
mod blend_dwg;

fn main() -> catwalk::Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let input = std::path::PathBuf::from(arguments.next().ok_or("expected input file")?);
    let output = std::path::PathBuf::from(arguments.next().ok_or("expected output PNG")?);
    let preview = match input.extension().and_then(|value| value.to_str()) {
        Some("blend") => blend_dwg::extract_blend(&input)?,
        Some("dwg") => blend_dwg::extract_dwg(&input)?,
        _ => return Err("expected .blend or .dwg input".into()),
    };
    match preview {
        Some(image) => {
            image.save(&output)?;
            println!("{}x{} {}", image.width(), image.height(), output.display());
        }
        None => println!("no saved preview"),
    }
    Ok(())
}
