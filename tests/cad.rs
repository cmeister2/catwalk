//! Qualification tests for supported CAD solids, triangulation and error handling.

use catwalk::formats;
use ogeom_core::Tolerances;
use ogeom_math::{Frame, Point};
use ogeom_topo::Model;
use std::{fs, path::PathBuf, process::Command};

struct FixtureDirectory(PathBuf);
impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn all_f3d_iges_and_brep_aliases_render_real_cad_geometry() {
    let tolerances = Tolerances::millimetres();
    let mut model = Model::new();
    let root = ogeom_algo::make_box(
        &mut model,
        Frame::WORLD.with_origin(Point::new(7., -2., 5.)),
        (2., 3., 4.),
        tolerances,
    )
    .unwrap()
    .shape;
    let brep = ogeom_io::brep::write(&model, &root, tolerances).unwrap();
    let mut document = ogeom_doc::Document::over(model);
    document.add_part("translated box", root);
    let iges = ogeom_io::write_iges(&document, tolerances).unwrap();
    let directory =
        FixtureDirectory(std::env::temp_dir().join(format!("catwalk-cad-{}", std::process::id())));
    fs::create_dir(&directory.0).unwrap();
    let mut reference = None;
    for extension in ["iges", "igs", "brep"] {
        let input = directory.0.join(format!("box.{extension}"));
        let output = directory.0.join(format!("{extension}.png"));
        fs::write(&input, if extension == "brep" { &brep } else { &iges }).unwrap();
        let scene = formats::load(&input).unwrap();
        assert_eq!(scene.triangles.len(), 12);
        assert!(scene.points.is_empty());
        assert!(scene.lines.is_empty());
        let area: f64 = scene
            .triangles
            .iter()
            .map(|triangle| {
                let [a, b, c] = triangle.vertices;
                (b - a).cross(c - a).length() * 0.5
            })
            .sum();
        assert!((area - 52.).abs() < 1e-7);
        let result = Command::new(env!("CARGO_BIN_EXE_catwalk"))
            .arg(&input)
            .arg("--output")
            .arg(&output)
            .args(["--size", "64", "--transparent"])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{extension}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stderr, [] as [u8; 0]);
        let bytes = fs::read(&output).unwrap();
        let thumbnail = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(thumbnail.dimensions(), (64, 64));
        let opaque = thumbnail.pixels().filter(|pixel| pixel[3] > 0).count();
        assert!((300..3500).contains(&opaque));
        if let Some(reference) = &reference {
            assert_eq!(&bytes, reference);
        } else {
            reference = Some(bytes);
        }
    }
}
