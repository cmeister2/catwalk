use catwalk::formats;
use std::{fs, path::PathBuf, process::Command};
use truck_modeling::{Point3, Vector3, builder};
use truck_stepio::out::{CompleteStepDisplay, StepHeaderDescriptor, StepModel};

struct FixtureDirectory(PathBuf);

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn all_f3d_step_aliases_render_a_cad_solid_without_a_display() {
    let directory =
        FixtureDirectory(std::env::temp_dir().join(format!("catwalk-step-{}", std::process::id())));
    fs::create_dir(&directory.0).unwrap();
    let vertex = builder::vertex(Point3::new(0.0, 0.0, 0.0));
    let edge = builder::tsweep(&vertex, Vector3::new(2.0, 0.0, 0.0));
    let face = builder::tsweep(&edge, Vector3::new(0.0, 1.0, 0.0));
    let solid = builder::tsweep(&face, Vector3::new(0.0, 0.0, 3.0)).compress();
    let source = CompleteStepDisplay::new(StepModel::from(&solid), StepHeaderDescriptor::default())
        .to_string();
    let mut reference = None;
    for extension in ["step", "stp", "stpnc", "p21", "210"] {
        let input = directory.0.join(format!("solid.{extension}"));
        let output = directory.0.join(format!("{extension}.png"));
        fs::write(&input, &source).unwrap();
        let scene = formats::load(&input).unwrap();
        assert!(scene.triangles.len() >= 12);
        assert!(scene.points.is_empty());
        let result = Command::new(env!("CARGO_BIN_EXE_catwalk"))
            .arg(&input)
            .args(["--output"])
            .arg(&output)
            .args(["--size", "64"])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_RUNTIME_DIR")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{extension}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stderr.is_empty());
        let bytes = fs::read(output).unwrap();
        let thumbnail = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(thumbnail.dimensions(), (64, 64));
        assert!(thumbnail.pixels().filter(|pixel| pixel[3] > 0).count() > 100);
        if let Some(reference) = &reference {
            assert_eq!(&bytes, reference);
        } else {
            reference = Some(bytes);
        }
    }
}
