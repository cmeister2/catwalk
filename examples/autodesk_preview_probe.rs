//! Small qualification runner; saved-preview research remains outside readers.
#[path = "saved_previews/autodesk.rs"]
/// Fusion saved-preview qualification reader.
mod autodesk;

/// Report saved-preview outcomes for Inventor and Fusion inputs and save successful images.
fn main() -> catwalk::Result<()> {
    for input in std::env::args_os().skip(1) {
        let path = std::path::Path::new(&input);
        if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("ipt" | "iam")
        ) {
            match catwalk::saved_preview::inventor::extract_png(path) {
                Ok(Some(png)) => {
                    let output = path.with_extension("rust-preview.png");
                    std::fs::write(&output, png)?;
                    println!("{}\t{}", path.display(), output.display());
                }
                Ok(None) => println!("{}\tabsent", path.display()),
                Err(error) => println!("{}\terror: {error}", path.display()),
            }
            continue;
        }
        match autodesk::extract_fusion(path) {
            Ok(Some(image)) => {
                let output = path.with_extension("preview.png");
                image.save(&output)?;
                println!(
                    "{}\t{}x{}\t{}",
                    path.display(),
                    image.width(),
                    image.height(),
                    output.display()
                );
            }
            Ok(None) => println!("{}\tabsent", path.display()),
            Err(error) => println!("{}\terror: {error}", path.display()),
        }
    }
    Ok(())
}
