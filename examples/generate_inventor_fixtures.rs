//! Reproduce original, MIT-licensed thumbnail-carrier fixtures for snapshot CI.
//! These CFB files contain saved images, not full Inventor CAD designs.
#[path = "fixtures/inventor.rs"]
/// Writer for the authored Inventor thumbnail property-set carriers.
mod inventor;

/// Encode a rendered fixture image as a PNG byte vector.
///
/// # Errors
/// Returns an error if the image cannot be encoded as PNG.
fn png(image: &image::RgbaImage) -> catwalk3d::Result<Vec<u8>> {
    let mut output = Vec::new();
    catwalk3d::render::write_png(image, &mut output)?;
    Ok(output)
}

/// Regenerate the authored part and assembly carrier files used by parser tests.
///
/// # Errors
/// Returns an error if the model cannot be loaded or rendered, an image cannot
/// be encoded, or the fixture directory and compound files cannot be written.
fn main() -> catwalk3d::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let destination = root.join("tests/fixtures/saved-previews/autodesk");
    std::fs::create_dir_all(&destination)?;
    let scene = catwalk3d::formats::load(&root.join("examples/models/gear.obj"))?;
    let options = catwalk3d::render::RenderOptions {
        width: 160,
        height: 128,
        transparent: true,
        ..catwalk3d::render::RenderOptions::default()
    };
    let part = catwalk3d::render::render(&scene, &options)?;
    inventor::write(
        &destination.join("part-carrier.ipt"),
        cfb::Version::V3,
        &png(&part)?,
    )?;
    let mut assembly = image::RgbaImage::new(280, 160);
    image::imageops::overlay(&mut assembly, &part, 0, 0);
    let second = image::imageops::rotate180(&part);
    image::imageops::overlay(&mut assembly, &second, 120, 32);
    inventor::write(
        &destination.join("assembly-carrier.iam"),
        cfb::Version::V4,
        &png(&assembly)?,
    )?;
    println!(
        "Wrote original Inventor thumbnail-carrier fixtures to {}",
        destination.display()
    );
    Ok(())
}
