//! Reproduce original, MIT-licensed thumbnail-carrier fixtures for snapshot CI.
//! These CFB files contain saved images, not full Inventor CAD designs.
#[path = "fixtures/inventor.rs"]
mod inventor;

fn png(image: &image::RgbaImage) -> catwalk::Result<Vec<u8>> {
    let mut output = Vec::new();
    catwalk::render::write_png(image, &mut output)?;
    Ok(output)
}

fn main() -> catwalk::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let destination = root.join("tests/fixtures/saved-previews/autodesk");
    std::fs::create_dir_all(&destination)?;
    let scene = catwalk::formats::load(&root.join("examples/models/gear.obj"))?;
    let options = catwalk::render::RenderOptions {
        width: 160,
        height: 128,
        transparent: true,
        ..Default::default()
    };
    let part = catwalk::render::render(&scene, &options)?;
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
