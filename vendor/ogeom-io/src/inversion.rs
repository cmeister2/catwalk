//! A point's parameter on one of the curves the readers build edges on,
//! shared by the exchange readers.

use ogeom_geom::Curve;
use ogeom_math::Point;

/// The parameter of the curve's point nearest `p`, for the curves that
/// have a closed form; `None` for the rest.
///
/// A vertex a file writes sits on its curve only to the file's own slop,
/// and the closed-form inversions are exact only for a point on the curve.
/// A line's and a circle's stay good off it; an ellipse's does not. The
/// eccentric anomaly read off a point beside the curve is off by the
/// point's miss over the minor radius, and it moves along the curve by the
/// major radius: on a very eccentric ellipse (a plane's section of a drum
/// cut almost along its axis), a vertex microns off the curve takes a
/// parameter millimetres away from it, and the edge runs the long way
/// round the ellipse. The closed form seeds Newton on the nearest-point
/// condition instead, which is well posed however eccentric the ellipse.
pub(crate) fn parameter_on(curve: &Curve, p: Point) -> Option<f64> {
    let tau = core::f64::consts::TAU;
    match curve {
        Curve::Line(line) => {
            let axis = line.axis();
            Some((p - axis.location).dot(axis.direction.vector()))
        }
        Curve::Circle(c) => {
            let local = c.circle().frame().to_local(p);
            Some(local.y.atan2(local.x).rem_euclid(tau))
        }
        Curve::Ellipse(e) => {
            let ellipse = e.ellipse();
            let local = ellipse.frame().to_local(p);
            let (a, b) = (ellipse.major_radius(), ellipse.minor_radius());
            let (x, y) = (local.x, local.y);
            let mut t = (y / b).atan2(x / a);
            // Newton on f(t) = (E(t) − p) · E′(t) = 0, in the plane of the
            // ellipse, from the eccentric anomaly; each step held to a
            // fraction of a turn so it cannot leap to the far side.
            for _ in 0..50 {
                let (s, c) = t.sin_cos();
                let f = (a * c - x) * (-a * s) + (b * s - y) * (b * c);
                let df = a * a * s * s + b * b * c * c - (a * c - x) * a * c - (b * s - y) * b * s;
                if df.abs() <= f64::MIN_POSITIVE {
                    break;
                }
                let step = (f / df).clamp(-0.25, 0.25);
                t -= step;
                if step.abs() <= 1e-15 {
                    break;
                }
            }
            Some(t.rem_euclid(tau))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::parameter_on;
    use ogeom_geom::{Curve, Curve3d, EllipseCurve};
    use ogeom_math::{Ellipse, Frame, Vector};

    const T: ogeom_core::Tolerances = ogeom_core::Tolerances::millimetres();

    /// A point two microns beside a very eccentric ellipse takes the
    /// parameter of its nearest point, not one metres along the curve.
    #[test]
    fn a_point_beside_an_eccentric_ellipse_takes_its_nearest_parameter() {
        let ellipse = Ellipse::new(Frame::WORLD, 6542.58, 1.8, T).unwrap();
        let curve: Curve = EllipseCurve::new(ellipse).into();
        for t in [1.2_f64, 2.9, 4.4, 5.9] {
            let on = curve.point_at(t, T).unwrap();
            let normal = {
                let d = curve.d1_at(t, T).unwrap();
                Vector::new(-d.y, d.x, 0.0) / d.magnitude()
            };
            let beside = on + normal * 2e-3;
            let found = parameter_on(&curve, beside).unwrap();
            let back = curve.point_at(found, T).unwrap();
            assert!(
                back.distance(on) < 1e-6,
                "t {t}: landed {} away",
                back.distance(on)
            );
        }
    }
}
