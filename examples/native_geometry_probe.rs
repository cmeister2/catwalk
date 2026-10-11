//! Inspect fresh native geometry without consulting embedded previews.
use serde_json::json;
use std::path::Path;

/// Print native geometry counts and bounds, or a load error, for each requested document.
fn main() {
    for name in std::env::args().skip(1) {
        let path = Path::new(&name);
        match catwalk::formats::load(path) {
            Ok(scene) => {
                let mut min = [f64::INFINITY; 3];
                let mut max = [f64::NEG_INFINITY; 3];
                for p in scene.positions() {
                    for (i, value) in [p.x, p.y, p.z].into_iter().enumerate() {
                        min[i] = min[i].min(value);
                        max[i] = max[i].max(value);
                    }
                }
                println!(
                    "{}",
                    json!({"file":path.file_name().and_then(|s|s.to_str()),"triangles":scene.triangles.len(),"lines":scene.lines.len(),"points":scene.points.len(),"bounds":[min,max]})
                );
            }
            Err(error) => println!(
                "{}",
                json!({"file":path.file_name().and_then(|s|s.to_str()),"error":error.to_string()})
            ),
        }
    }
}
