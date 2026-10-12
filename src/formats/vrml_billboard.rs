//! VRML97 Billboard rotations frozen against the primary orthographic view.
//!
//! Viewer directions are pulled into each occurrence's parent frame. Constrained
//! axes retain their authored pivot; a zero axis builds a local orthonormal frame
//! from the viewing ray and camera up. Inherited nonuniform stretch or shear is
//! preserved, so local up alignment does not promise exact world-space up alignment.
//! Authored Viewpoint nodes, camera movement and perspective positions are not used.

use super::Transform;
use crate::Result;
use crate::render::PRIMARY_VIEW_DIRECTION;
use crate::scene::Vec3;

/// Unit-vector cross products below this threshold have no stable rotation plane.
const MIN_PLANE: f64 = 32. * f64::EPSILON;

/// Compose the occurrence's affine parent with its fixed-view Billboard rotation.
///
/// Axis/view coincidence has undefined VRML97 behavior and is rejected. An axis
/// parallel to local Z selects zero rotation because Z already lies in the required
/// plane. Free alignment projects pulled-back camera up perpendicular to the local
/// viewing ray before constructing a proper rotation, preserving parent stretch.
/// Translation is retained but does not affect the orthographic viewing directions.
///
/// # Errors
/// Returns an error for non-finite axes or matrices, a singular or non-affine parent,
/// numerically coincident directions, or an unrepresentable composed transform.
pub(super) fn placement(parent: Transform, axis: Vec3) -> Result<Transform> {
    if !axis.is_finite() {
        return Err("VRML Billboard axis is non-finite".into());
    }
    let (toward, up) = local_view(parent)?;
    let local = if axis.x == 0. && axis.y == 0. && axis.z == 0. {
        viewer_alignment(toward, up)?
    } else {
        constrained_rotation(unit(axis)?, toward)?
    };
    let world = parent.then(local);
    if world.0.iter().flatten().any(|value| !value.is_finite()) {
        return Err("VRML Billboard composed transform is non-finite".into());
    }
    Ok(world)
}

/// Normalize a finite nonzero vector without overflowing its original length.
///
/// # Errors
/// Returns an error when a direction is zero or has non-finite components.
fn unit(vector: Vec3) -> Result<Vec3> {
    let scale = vector.x.abs().max(vector.y.abs()).max(vector.z.abs());
    if !vector.is_finite() || scale == 0. {
        return Err("VRML Billboard direction is zero or non-finite".into());
    }
    let scaled = vector / scale;
    Ok(scaled / scaled.length())
}

/// Convert a renderer Z-up direction back into the VRML Y-up coordinate system.
fn vrml_direction(direction: Vec3) -> Vec3 {
    Vec3::new(direction.x, direction.z, -direction.y)
}

/// Resolve the true primary camera ray and up through a scaled affine inverse.
///
/// # Errors
/// Returns an error for invalid, singular or numerically unrepresentable parents.
#[allow(
    clippy::float_cmp,
    reason = "An affine homogeneous row has exact zero and one entries."
)]
fn local_view(parent: Transform) -> Result<(Vec3, Vec3)> {
    if parent.0.iter().flatten().any(|value| !value.is_finite()) {
        return Err("VRML Billboard parent transform is non-finite".into());
    }
    if parent.0[..3].iter().any(|column| column[3] != 0.) || parent.0[3][3] != 1. {
        return Err("VRML Billboard parent transform is not affine".into());
    }
    let columns = parent.0[..3]
        .iter()
        .map(|column| Vec3::new(column[0], column[1], column[2]));
    let mut normalized_columns = [Vec3::default(); 3];
    let mut scales = [0.; 3];
    for (index, column) in columns.enumerate() {
        let scale = column.x.abs().max(column.y.abs()).max(column.z.abs());
        if scale == 0. {
            return Err("VRML Billboard parent transform is singular".into());
        }
        normalized_columns[index] = column / scale;
        scales[index] = scale;
    }
    let duals = [
        normalized_columns[1].cross(normalized_columns[2]),
        normalized_columns[2].cross(normalized_columns[0]),
        normalized_columns[0].cross(normalized_columns[1]),
    ];
    let determinant = normalized_columns[0].dot(duals[0]);
    if determinant == 0. || !determinant.is_finite() {
        return Err("VRML Billboard parent transform is singular".into());
    }
    let viewer = unit(PRIMARY_VIEW_DIRECTION)?;
    let camera_up = unit(viewer.cross(Vec3::new(0., 0., 1.).cross(viewer)))?;
    let toward = inverse_direction(duals, scales, determinant, vrml_direction(viewer))?;
    let up = inverse_direction(duals, scales, determinant, vrml_direction(camera_up))?;
    Ok((toward, up))
}

/// Apply an inverse to a direction using cofactors without dividing by its determinant.
///
/// A positive common scale preserves direction and avoids overflowing inverse
/// entries for uniformly tiny parents. The determinant sign retains reflections.
///
/// # Errors
/// Returns an error when the inverse direction cannot be represented as a finite
/// nonzero vector at double precision.
fn inverse_direction(
    duals: [Vec3; 3],
    scales: [f64; 3],
    determinant: f64,
    direction: Vec3,
) -> Result<Vec3> {
    let values = duals.map(|dual| dual.dot(direction));
    let common = values
        .iter()
        .zip(scales)
        .filter_map(|(value, scale)| (*value != 0.).then_some(scale))
        .fold(f64::INFINITY, f64::min);
    let values: [f64; 3] = std::array::from_fn(|index| {
        if values[index] == 0. {
            0.
        } else {
            values[index] * (common / scales[index]) * determinant.signum()
        }
    });
    unit(Vec3::new(values[0], values[1], values[2]))
}

/// Build a proper local rotation with Z toward the viewer and projected Y upward.
///
/// # Errors
/// Returns an error when the pulled-back up and viewing directions are coincident.
fn viewer_alignment(toward: Vec3, up: Vec3) -> Result<Transform> {
    let right = up.cross(toward);
    if right.length() <= MIN_PLANE {
        return Err("VRML Billboard viewer-up and viewing directions coincide".into());
    }
    let right = unit(right)?;
    let up = unit(toward.cross(right))?;
    Ok(Transform([
        [right.x, right.y, right.z, 0.],
        [up.x, up.y, up.z, 0.],
        [toward.x, toward.y, toward.z, 0.],
        [0., 0., 0., 1.],
    ]))
}

/// Rotate local Z around the authored axis into the axis/view plane facing the viewer.
///
/// # Errors
/// Returns an error when the axis and viewing ray cannot define a stable plane.
fn constrained_rotation(axis: Vec3, toward: Vec3) -> Result<Transform> {
    let normal = axis.cross(toward);
    if normal.length() <= MIN_PLANE {
        return Err("VRML Billboard axis and viewing direction coincide".into());
    }
    let local_z = Vec3::new(0., 0., 1.);
    let start = local_z - axis * axis.dot(local_z);
    if start.length() <= MIN_PLANE {
        return Ok(Transform::IDENTITY);
    }
    let start = unit(start)?;
    let target = unit(normal.cross(axis))?;
    let sine = axis.dot(start.cross(target));
    let cosine = start.dot(target);
    super::axis_rotation(axis, sine.atan2(cosine))
}

/// Mathematical controls and end-to-end asymmetric Billboard examples.
#[cfg(test)]
mod tests {
    use super::*;

    /// Compare finite vector coordinates with a tight absolute tolerance.
    fn close(actual: Vec3, expected: Vec3) {
        assert!(
            (actual - expected).length() < 1e-12,
            "{actual:?} != {expected:?}"
        );
    }

    /// Apply only the linear portion of an affine transform to a direction.
    fn direction(transform: Transform, vector: Vec3) -> Vec3 {
        transform.apply(vector) - transform.apply(Vec3::default())
    }

    /// Default Y-constrained alignment rotates asymmetric local geometry by 45 degrees.
    #[test]
    fn default_axis_rotates_asymmetric_points() {
        let world = placement(Transform::IDENTITY, Vec3::new(0., 1., 0.)).unwrap();
        let diagonal = std::f64::consts::FRAC_1_SQRT_2;
        close(
            world.apply(Vec3::new(0., 0., 2.)),
            Vec3::new(2. * diagonal, 0., 2. * diagonal),
        );
        close(
            world.apply(Vec3::new(1., 0., 0.)),
            Vec3::new(diagonal, 0., -diagonal),
        );
        close(world.apply(Vec3::new(0., 3., 0.)), Vec3::new(0., 3., 0.));
    }

    /// Free alignment uses projected camera up rather than the unprojected world axis.
    #[test]
    fn free_axis_uses_camera_up_for_an_asymmetric_plane_and_point() {
        let scene = super::super::vrml(r"#VRML V2.0 utf8
          Billboard { axisOfRotation 0 0 0 children [
            Shape { geometry IndexedFaceSet { coord Coordinate { point [0 0 0, 2 0 0, 0 3 0] } coordIndex [0 1 2 -1] } }
            Shape { geometry PointSet { coord Coordinate { point [0 0 2] } } }
          ] }").unwrap();
        assert_eq!(scene.triangles.len(), 1);
        assert_eq!(scene.points.len(), 1);
        let toward = unit(PRIMARY_VIEW_DIRECTION).unwrap();
        let right = Vec3::new(1., 1., 0.) / 2_f64.sqrt();
        let up = Vec3::new(-1., 1., 2.) / 6_f64.sqrt();
        close(scene.triangles[0].vertices[0], Vec3::default());
        close(scene.triangles[0].vertices[1], right * 2.);
        close(scene.triangles[0].vertices[2], up * 3.);
        close(scene.points[0].position, toward * 2.);
        let normal = (scene.triangles[0].vertices[1] - scene.triangles[0].vertices[0])
            .cross(scene.triangles[0].vertices[2] - scene.triangles[0].vertices[0]);
        close(unit(normal).unwrap(), toward);
    }

    /// Repeated instances recalculate their local rotation beneath different parents.
    #[test]
    fn repeated_default_instances_pull_back_rotated_and_scaled_parents() {
        let scene = super::super::vrml(r"#VRML V2.0 utf8
          Transform { translation 2 3 4 scale 2 3 4 children [
            DEF Facing Billboard { children [Shape { geometry PointSet { coord Coordinate { point [0 0 2] } } }] }
          ] }
          Transform { translation -2 0 1 rotation 0 1 0 1.5707963267948966 scale 3 2 1 children [USE Facing] }").unwrap();
        assert_eq!(scene.points.len(), 2);
        let first = 8. / 5_f64.sqrt();
        let second = 6. / 10_f64.sqrt();
        close(
            scene.points[0].position,
            Vec3::new(2. + first, -4. - first, 3.),
        );
        close(
            scene.points[1].position,
            Vec3::new(-2. + second, -1. - second, 0.),
        );
    }

    /// Free-axis alignment retains translation, nonuniform stretch and reflections.
    #[test]
    fn affine_parent_stretch_and_reflection_are_preserved() {
        for x_scale in [2., -2.] {
            let parent = Transform([
                [x_scale, 0., 0., 0.],
                [0., 3., 0., 0.],
                [1., 0., 4., 0.],
                [7., 8., 9., 1.],
            ]);
            let world = placement(parent, Vec3::default()).unwrap();
            close(world.apply(Vec3::default()), Vec3::new(7., 8., 9.));
            let z = direction(world, Vec3::new(0., 0., 1.));
            close(unit(z).unwrap(), unit(Vec3::new(1., 1., 1.)).unwrap());
            let x = direction(world, Vec3::new(1., 0., 0.));
            let y = direction(world, Vec3::new(0., 1., 0.));
            assert!((x.dot(y.cross(z)) - x_scale * 12.).abs() < 1e-10);
        }
    }

    /// Column scaling keeps uniformly tiny and huge invertible parents representable.
    #[test]
    fn extreme_uniform_parents_and_axis_magnitudes_remain_finite() {
        for scale in [1e-300, 1e300] {
            let parent = Transform([
                [scale, 0., 0., 0.],
                [0., scale, 0., 0.],
                [0., 0., scale, 0.],
                [0., 0., 0., 1.],
            ]);
            let world = placement(parent, Vec3::new(0., 1e308, 0.)).unwrap();
            close(
                unit(direction(world, Vec3::new(0., 0., 1.))).unwrap(),
                unit(Vec3::new(1., 0., 1.)).unwrap(),
            );
            assert!(world.0.iter().flatten().all(|value| value.is_finite()));
        }
    }

    /// A local-Z pivot has no ability or need to move local Z into the view plane.
    #[test]
    fn z_parallel_axis_uses_the_documented_zero_rotation() {
        let world = placement(Transform::IDENTITY, Vec3::new(0., 0., 2.)).unwrap();
        close(world.apply(Vec3::new(1., 2., 3.)), Vec3::new(1., 2., 3.));
    }

    /// Invalid parents and undefined rotation planes fail before visiting any children.
    #[test]
    fn nonfinite_singular_and_coincident_inputs_are_rejected() {
        for axis in [
            Vec3::new(f64::NAN, 1., 0.),
            Vec3::new(0., f64::INFINITY, 0.),
        ] {
            assert!(placement(Transform::IDENTITY, axis).is_err());
        }
        assert!(placement(Transform::IDENTITY, Vec3::new(1., 1., 1.)).is_err());
        let mut singular = Transform::IDENTITY;
        singular.0[2] = singular.0[0];
        assert!(placement(singular, Vec3::default()).is_err());
        let mut nonfinite = Transform::IDENTITY;
        nonfinite.0[3][0] = f64::NAN;
        assert!(placement(nonfinite, Vec3::new(0., 1., 0.)).is_err());
        let mut projective = Transform::IDENTITY;
        projective.0[0][3] = 0.5;
        assert!(placement(projective, Vec3::default()).is_err());
    }
}
