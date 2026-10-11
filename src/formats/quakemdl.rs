//! Quake MDL v6 geometry; preview the first frame, including grouped frames.
//! Layout: <https://github.com/id-Software/Quake/blob/master/WinQuake/modelgen.h>
use crate::{
    Result,
    scene::{Scene, Triangle, Vec3},
};
use std::path::Path;

/// Bounds-checked cursor over a Quake MDL byte stream.
struct Input<'a> {
    /// Borrowed complete MDL input bytes.
    bytes: &'a [u8],
    /// Absolute position of the next unread record.
    position: usize,
}
impl<'a> Input<'a> {
    /// Consume the requested bytes, rejecting offset overflow and truncated records.
    fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        let end = self.position.checked_add(size).ok_or("MDL size overflow")?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or("truncated MDL record")?;
        self.position = end;
        Ok(value)
    }
    /// Read a little-endian signed header or triangle-index word.
    fn integer(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }
    /// Read a positive count bounded to five million records.
    #[allow(
        clippy::cast_sign_loss,
        reason = "The signed MDL count is validated as positive and at most five million before native conversion."
    )]
    fn count(&mut self) -> Result<usize> {
        let value = self.integer()?;
        if !(1..=5_000_000).contains(&value) {
            return Err("MDL count is outside the importer limit".into());
        }
        Ok(value as usize)
    }
    /// Read a finite little-endian MDL float and widen it to double precision.
    fn float(&mut self) -> Result<f64> {
        let value = f64::from(f32::from_le_bytes(self.take(4)?.try_into()?));
        if !value.is_finite() {
            return Err("non-finite MDL numeric value".into());
        }
        Ok(value)
    }
    /// Read three consecutive finite components into a geometry vector.
    fn vector(&mut self) -> Result<Vec3> {
        Ok(Vec3::new(self.float()?, self.float()?, self.float()?))
    }
    /// Validate that grouped skin or frame intervals are positive and strictly increasing.
    fn intervals(&mut self, count: usize) -> Result<()> {
        let mut previous = 0.;
        for _ in 0..count {
            let value = self.float()?;
            if value <= previous {
                return Err("MDL group intervals must increase and be positive".into());
            }
            previous = value;
        }
        Ok(())
    }
}

/// Read a bounded Quake MDL version-six first-frame geometry and decode its preview geometry.
///
/// # Errors
/// Returns an error for unreadable input, invalid version/counts, truncated records, invalid
/// indices or nonfinite geometry.
pub(super) fn load(path: &Path) -> Result<Scene> {
    parse(&super::read_file(path)?)
}

/// Validate MDL v6 records and expand the first compressed-vertex frame using its scale and origin.
#[allow(
    clippy::cast_sign_loss,
    reason = "Every signed MDL vertex index is checked against the bounded vertex array before native conversion."
)]
fn parse(bytes: &[u8]) -> Result<Scene> {
    let mut input = Input { bytes, position: 0 };
    if input.take(4)? != b"IDPO" || input.integer()? != 6 {
        return Err("expected Quake MDL version 6".into());
    }
    let scale = input.vector()?;
    let origin = input.vector()?;
    input.float()?; // Radius.
    let _ = input.vector()?; // Eye position.
    let skins = input.count()?;
    let skin_width = input.count()?;
    let skin_height = input.count()?;
    let vertices = input.count()?;
    let triangles = input.count()?;
    let frames = input.count()?;
    input.integer()?; // Synchronization type.
    input.integer()?; // Flags.
    input.float()?; // Model size.
    let skin_bytes = skin_width
        .checked_mul(skin_height)
        .ok_or("MDL skin size overflow")?;
    for _ in 0..skins {
        match input.integer()? {
            0 => {
                input.take(skin_bytes)?;
            }
            1 => {
                let count = input.count()?;
                input.intervals(count)?;
                input.take(
                    skin_bytes
                        .checked_mul(count)
                        .ok_or("MDL skin group overflow")?,
                )?;
            }
            _ => return Err("unknown MDL skin type".into()),
        }
    }
    input.take(vertices.checked_mul(12).ok_or("MDL UV size overflow")?)?;
    let mut faces = Vec::new();
    for _ in 0..triangles {
        input.integer()?; // Texture seam selector.
        let indices = [input.integer()?, input.integer()?, input.integer()?];
        if indices.iter().any(|&i| i < 0 || i as usize >= vertices) {
            return Err("MDL vertex index out of range".into());
        }
        faces.push(indices.map(|i| i as usize));
    }
    let frame_bytes = 24usize
        .checked_add(vertices.checked_mul(4).ok_or("MDL vertex size overflow")?)
        .ok_or("MDL frame size overflow")?;
    let mut first = None;
    for _ in 0..frames {
        let group_frames = match input.integer()? {
            0 => 1,
            1 => {
                let count = input.count()?;
                input.take(8)?;
                input.intervals(count)?;
                count
            }
            _ => return Err("unknown MDL frame type".into()),
        };
        for _ in 0..group_frames {
            let frame = input.take(frame_bytes)?;
            if first.is_none() {
                let positions: Vec<_> = frame[24..]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|v| {
                        Vec3::new(
                            f64::from(v[0]) * scale.x + origin.x,
                            f64::from(v[1]) * scale.y + origin.y,
                            f64::from(v[2]) * scale.z + origin.z,
                        )
                    })
                    .collect();
                first = Some(positions);
            }
        }
    }
    if input.position != bytes.len() {
        return Err("unexpected bytes after MDL frames".into());
    }
    let positions = first.ok_or("MDL contains no frames")?;
    let scene = Scene {
        triangles: faces
            .into_iter()
            .map(|indices| Triangle {
                vertices: indices.map(|i| positions[i]),
                color: None,
            })
            .collect(),
        points: Vec::new(),
        lines: Vec::new(),
        ..Scene::default()
    };
    scene.validate()?;
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mdl(grouped: bool) -> Vec<u8> {
        let mut bytes = b"IDPO".to_vec();
        bytes.extend(6i32.to_le_bytes());
        for value in [2f32, 3., 4., 10., 20., 30., 1., 0., 0., 0.] {
            bytes.extend(value.to_le_bytes());
        }
        for value in [1i32, 1, 1, 3, 1, 1, 0, 0] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(1f32.to_le_bytes());
        bytes.extend(0i32.to_le_bytes());
        bytes.push(42);
        bytes.extend([0; 36]);
        for value in [1i32, 0, 1, 2] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(i32::from(grouped).to_le_bytes());
        if grouped {
            bytes.extend(2i32.to_le_bytes());
            bytes.extend([0; 8]);
            bytes.extend(0.1f32.to_le_bytes());
            bytes.extend(0.2f32.to_le_bytes());
        }
        for frame in 0..if grouped { 2 } else { 1 } {
            bytes.extend([0; 24]);
            bytes.extend([0, 0, 0, 0, 1 + frame, 0, 0, 0, 0, 1, 0, 0]);
        }
        bytes
    }
    #[test]
    fn single_and_grouped_frames_preserve_scale_and_origin() {
        for grouped in [false, true] {
            let scene = parse(&mdl(grouped)).unwrap();
            assert_eq!(
                scene.triangles[0].vertices,
                [
                    Vec3::new(10., 20., 30.),
                    Vec3::new(12., 20., 30.),
                    Vec3::new(10., 23., 30.)
                ]
            );
            assert!(
                crate::render::render(&scene, &crate::render::RenderOptions::default()).is_ok()
            );
        }
    }
    #[test]
    fn invalid_and_truncated_models_fail() {
        let bytes = mdl(true);
        assert!(parse(&bytes[..bytes.len() - 1]).is_err());
        assert!(parse(b"not an MDL").is_err());
        let mut bytes = mdl(false);
        bytes[133..137].copy_from_slice(&10i32.to_le_bytes());
        assert!(parse(&bytes).is_err());
    }
}
