//! Headless scalar-volume rendering checks across supported volume encodings.

use flate2::{
    Compression,
    write::{GzEncoder, ZlibEncoder},
};
use std::{
    io::Write,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

#[test]
fn volume_formats_render_identical_nonuniform_density_without_a_display() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "catwalk-volume-cli-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let mut density = Vec::new();
    for z in 0..32i32 {
        for y in 0..32i32 {
            for x in 0..32i32 {
                let squared = (x - 16).pow(2) + (y - 16).pow(2) + (z - 16).pow(2);
                density.push(if (25..=81).contains(&squared) {
                    100u8
                } else {
                    0
                });
            }
        }
    }
    let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
    gzip.write_all(&density).unwrap();
    let gzip = gzip.finish().unwrap();
    let mut zlib = ZlibEncoder::new(Vec::new(), Compression::fast());
    zlib.write_all(&density).unwrap();
    let zlib = zlib.finish().unwrap();
    let nrrd = "NRRD0005\ntype: uint8\ndimension: 3\nsizes: 32 32 32\nencoding: gzip\nspace: right-anterior-superior\nspace directions: (1,0,0) (0,1,0) (0,0,1)\nspace origin: (0,0,0)\n";
    std::fs::write(
        directory.join("shell.nrrd"),
        [nrrd.as_bytes(), b"\n", &gzip].concat(),
    )
    .unwrap();
    std::fs::write(directory.join("shell.raw.gz"), &gzip).unwrap();
    std::fs::write(
        directory.join("shell.nhdr"),
        format!("{nrrd}data file: shell.raw.gz\n"),
    )
    .unwrap();
    let meta = "ObjectType = Image\nNDims = 3\nDimSize = 32 32 32\nElementType = MET_UCHAR\nElementSpacing = 1 1 1\nCompressedData = True\n";
    std::fs::write(
        directory.join("shell.mha"),
        [format!("{meta}ElementDataFile = LOCAL\n").as_bytes(), &zlib].concat(),
    )
    .unwrap();
    std::fs::write(directory.join("shell.zraw"), &zlib).unwrap();
    std::fs::write(
        directory.join("shell.mhd"),
        format!("{meta}ElementDataFile = shell.zraw\n"),
    )
    .unwrap();
    let scalars = density
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    std::fs::write(directory.join("shell.vti"),format!(r#"<VTKFile type="ImageData"><ImageData WholeExtent="0 31 0 31 0 31" Origin="0 0 0" Spacing="1 1 1"><Piece Extent="0 31 0 31 0 31"><PointData Scalars="density"><DataArray type="UInt8" Name="density" format="ascii">{scalars}</DataArray></PointData></Piece></ImageData></VTKFile>"#)).unwrap();
    let mut reference = None;
    for extension in ["vti", "nrrd", "nhdr", "mhd", "mha"] {
        let input = directory.join(format!("shell.{extension}"));
        let scene = catwalk::formats::load(&input).unwrap();
        assert!(scene.triangles.len() > 1000);
        let output = directory.join(format!("{extension}.png"));
        let status = Command::new(env!("CARGO_BIN_EXE_catwalk"))
            .env_remove("DISPLAY")
            .arg(&input)
            .arg("--output")
            .arg(&output)
            .arg("--size")
            .arg("128")
            .status()
            .unwrap();
        assert!(status.success(), "{extension} thumbnail failed");
        let image = image::ImageReader::open(&output)
            .unwrap()
            .decode()
            .unwrap()
            .to_rgba8();
        assert_eq!(image.dimensions(), (128, 128));
        assert!(image.pixels().filter(|p| p[3] > 0).count() > 1000);
        let bytes = std::fs::read(output).unwrap();
        if let Some(expected) = &reference {
            assert_eq!(&bytes, expected, "{extension} thumbnail differs");
        } else {
            reference = Some(bytes);
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
