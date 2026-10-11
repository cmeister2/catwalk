//! Generate a reproducible OBJ model and render its preview.
use catwalk3d::{
    Result,
    render::{RenderOptions, write_png},
    thumbnail,
};
use std::{f64::consts::TAU, fmt::Write as _, path::Path};

/// Write a procedural OBJ gear and render its thumbnail with the public API.
fn main() -> Result<()> {
    let segments = 64;
    let mut obj = String::from("# Original procedural gear example, MIT licensed with Catwalk\n");
    for i in 0..segments {
        let angle = TAU * f64::from(i) / f64::from(segments);
        let outer = if i % 4 < 2 { 1.15 } else { 1.0 };
        for (radius, z) in [(outer, 0.), (0.43, 0.), (0.43, 0.4), (outer, 0.4)] {
            writeln!(
                obj,
                "v {:.9} {:.9} {z:.9}",
                radius * angle.cos(),
                radius * angle.sin()
            )?;
        }
    }
    for i in 0..segments {
        let a = i * 4 + 1;
        let b = ((i + 1) % segments) * 4 + 1;
        for face in [
            [a, b, b + 1, a + 1],
            [a + 1, b + 1, b + 2, a + 2],
            [a + 2, b + 2, b + 3, a + 3],
            [a + 3, b + 3, b, a],
        ] {
            writeln!(obj, "f {} {} {} {}", face[0], face[1], face[2], face[3])?;
        }
    }
    std::fs::create_dir_all("examples/models")?;
    std::fs::write("examples/models/gear.obj", obj)?;
    let options = RenderOptions {
        color: [226, 148, 64],
        ..RenderOptions::default()
    };
    let image = thumbnail(Path::new("examples/models/gear.obj"), &options)?;
    write_png(&image, std::fs::File::create("examples/gear.png")?)?;
    println!("Wrote examples/models/gear.obj and examples/gear.png");
    Ok(())
}
