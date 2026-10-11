//! Reproducible `LightBurn` saved-preview qualification runner.
#[path = "saved_previews/lightburn.rs"]
/// `LightBurn` saved-preview qualification reader.
mod lightburn;

/// Emit JSON qualification outcomes and save extracted `LightBurn` thumbnails.
fn main() -> catwalk::Result<()> {
    for input in std::env::args_os().skip(1) {
        let path = std::path::Path::new(&input);
        let start = std::time::Instant::now();
        let result = match lightburn::extract(path) {
            Ok(Some(image)) => {
                let output = path.with_extension("preview.png");
                image.save(&output)?;
                serde_json::json!({"input": path, "outcome": "extracted", "width": image.width(), "height": image.height(), "output": output, "elapsed_us": start.elapsed().as_micros()})
            }
            Ok(None) => {
                serde_json::json!({"input": path, "outcome": "absent", "elapsed_us": start.elapsed().as_micros()})
            }
            Err(error) => {
                serde_json::json!({"input": path, "outcome": "error", "error": error.to_string(), "elapsed_us": start.elapsed().as_micros()})
            }
        };
        println!("{result}");
    }
    Ok(())
}
