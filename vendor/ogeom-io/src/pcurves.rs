//! Pcurves for imported faces, shared by the exchange readers.
//!
//! The machinery itself lives in `ogeom_algo::pcurve_fit` (fitting a trim
//! by projection is geometry, not exchange), and the readers reach it
//! through this shim.
//!
//! What is written here is the part neither reader can do edge by edge:
//! *where in the chart* an image goes. A periodic chart offers a branch per
//! turn, every one describing the same points, and only continuity with the
//! edge before chooses between them. Both readers derive each image alone,
//! so both need the same walk afterwards.

use ogeom_core::{OgeomResult, Tolerances};
use ogeom_geom::{PlanarCurve, SurfaceGeometry};

pub(crate) use ogeom_algo::pcurve_fit::fit_projected_pcurve;

/// An image slid by whole periods until its start meets `previous`.
///
/// The only freedom a periodic chart leaves: every branch describes the
/// same points, and only continuity with the edge before it chooses.
pub(crate) fn shifted_to_meet(
    image: &PlanarCurve,
    start: f64,
    previous: Option<ogeom_math::Point2>,
    surface: &SurfaceGeometry,
    tol: Tolerances,
) -> OgeomResult<PlanarCurve> {
    use ogeom_geom::Curve2d as _;
    use ogeom_geom::Surface as _;
    let Some(previous) = previous else {
        return Ok(image.clone());
    };
    let ((ua, ub), (va, vb)) = surface.domain();
    let u_period = surface.is_periodic_u().then_some(ub - ua);
    let v_period = surface.is_periodic_v().then_some(vb - va);
    if u_period.is_none() && v_period.is_none() {
        return Ok(image.clone());
    }
    // Whole turns only, and only where the gap is plainly some number of
    // them. A branch chosen wrongly leaves a gap within rounding of a whole
    // period; anything else is the file saying something this does not
    // understand (a cone's wire that already stood half a turn open, say),
    // and half a period rounds to one, which would open it further. Left
    // alone, such a wire is exactly as it was.
    let whole = |gap: f64, period: f64| -> f64 {
        let turns = (gap / period).round();
        let left = period.mul_add(-turns, gap);
        if gap.abs() > period * 0.75 && left.abs() < period * 0.25 {
            turns * period
        } else {
            0.0
        }
    };
    let at = image.point_at(start, tol)?;
    let shift = ogeom_math::Vector2::new(
        u_period.map_or(0.0, |p| whole(previous.x - at.x, p)),
        v_period.map_or(0.0, |p| whole(previous.y - at.y, p)),
    );
    if shift.x == 0.0 && shift.y == 0.0 {
        return Ok(image.clone());
    }
    image.transformed(&ogeom_math::Transform2::translation(shift), tol)
}

/// The points a surface's chart collapses a whole row of parameters to: a
/// sphere's poles, a cone's apex, a patch's side drawn together to a point.
///
/// A curve through one of them has no continuous image in the chart: at the
/// point every value of the other parameter is the same place, and the image
/// leaves along another column than the one it arrived on.
pub(crate) fn chart_poles(surface: &SurfaceGeometry, tol: Tolerances) -> Vec<ogeom_math::Point> {
    use ogeom_geom::Surface as _;
    match surface {
        SurfaceGeometry::Plane(_)
        | SurfaceGeometry::Cylinder(_)
        | SurfaceGeometry::Extrusion(_) => Vec::new(),
        SurfaceGeometry::Cone(cone) => vec![cone.cone().apex()],
        _ => {
            let ((ua, ub), (va, vb)) = surface.domain();
            if ![ua, ub, va, vb].iter().all(|x| x.is_finite()) {
                return Vec::new();
            }
            let sides: [(f64, f64, f64, f64); 4] = [
                (ua, va, ub, va),
                (ua, vb, ub, vb),
                (ua, va, ua, vb),
                (ub, va, ub, vb),
            ];
            let mut out: Vec<ogeom_math::Point> = Vec::new();
            for (u0, v0, u1, v1) in sides {
                let Ok(first) = surface.point_at(u0, v0, tol) else {
                    continue;
                };
                let collapsed = (1..=8).all(|k| {
                    let s = f64::from(k) / 8.0;
                    surface
                        .point_at((u1 - u0).mul_add(s, u0), (v1 - v0).mul_add(s, v0), tol)
                        .is_ok_and(|p| p.distance(first) <= tol.confusion())
                });
                if collapsed && out.iter().all(|p| p.distance(first) > tol.confusion()) {
                    out.push(first);
                }
            }
            out
        }
    }
}

/// A curve's passage through a chart pole: its parameter, the pole, and
/// how far from the pole the curve passes.
pub(crate) type Crossing = (f64, ogeom_math::Point, f64);

/// Where `curve` runs through one of `poles` strictly inside `range`, clear
/// of both its ends, ascending by parameter.
///
/// A crossing is a local minimum of the distance to the pole, found on a
/// scan and refined by golden section, that comes within a hundred times
/// the confusion distance.
pub(crate) fn pole_crossings(
    curve: &ogeom_geom::Curve,
    range: (f64, f64),
    poles: &[ogeom_math::Point],
    tol: Tolerances,
) -> Vec<Crossing> {
    use ogeom_geom::Curve3d as _;
    if poles.is_empty() || range.1 <= range.0 {
        return Vec::new();
    }
    let reach = tol.confusion() * 100.0;
    let samples: u32 = match curve {
        ogeom_geom::Curve::BSpline(spline) => u32::try_from(spline.control_points().len())
            .unwrap_or(u32::MAX / 8)
            .saturating_mul(8)
            .max(64),
        _ => 64,
    };
    let step = (range.1 - range.0) / f64::from(samples);
    let (Ok(head), Ok(tail)) = (curve.point_at(range.0, tol), curve.point_at(range.1, tol)) else {
        return Vec::new();
    };
    let clear = (range.1 - range.0) * 1e-6;
    let mut out: Vec<Crossing> = Vec::new();
    for &pole in poles {
        let distance = |t: f64| {
            curve
                .point_at(t, tol)
                .map_or(f64::INFINITY, |p| p.distance(pole))
        };
        let scan: Vec<(f64, f64)> = (0..=samples)
            .map(|k| {
                let t = step.mul_add(f64::from(k), range.0);
                (t, distance(t))
            })
            .collect();
        for window in scan.windows(3) {
            let [before, here, after] = [window[0], window[1], window[2]];
            if here.1 > before.1 || here.1 > after.1 {
                continue;
            }
            let (mut a, mut b) = (before.0, after.0);
            for _ in 0..80 {
                let (c, d) = ((b - a).mul_add(-0.618, b), (b - a).mul_add(0.618, a));
                if distance(c) < distance(d) {
                    b = d;
                } else {
                    a = c;
                }
            }
            let t = f64::midpoint(a, b);
            let miss = distance(t);
            let Ok(at) = curve.point_at(t, tol) else {
                continue;
            };
            if miss <= reach
                && t - range.0 > clear
                && range.1 - t > clear
                && at.distance(head) > reach * 10.0
                && at.distance(tail) > reach * 10.0
                && out.iter().all(|(s, ..)| (s - t).abs() > clear)
            {
                out.push((t, pole, miss));
            }
        }
    }
    out.sort_by(|x, y| x.0.total_cmp(&y.0));
    out
}

/// `edge`, built along `curve` over `range`, cut at `crossings` into pieces
/// that each run along the curve between consecutive vertices: the edge's
/// own at its ends, and at each crossing the vertex already standing at
/// that pole in `pole_vertices`, or a new one added there.
///
/// Each piece keeps the edge's tolerance; a pole vertex widens to how far
/// the curve passes from it.
pub(crate) fn split_at_poles(
    model: &mut ogeom_topo::Model,
    edge: &ogeom_topo::Shape,
    curve: &ogeom_geom::Curve,
    range: (f64, f64),
    crossings: &[Crossing],
    pole_vertices: &mut Vec<ogeom_topo::Shape>,
    tol: Tolerances,
) -> OgeomResult<Vec<(ogeom_topo::Shape, (f64, f64))>> {
    let Some((first, last)) = ogeom_algo::edge_vertices(model, edge)? else {
        return Ok(vec![(edge.clone(), range)]);
    };
    let tolerance = model
        .node(edge)
        .and_then(|n| n.data().as_edge())
        .map(|d| d.tolerance);
    let mut stops = vec![(range.0, first)];
    for &(t, pole, miss) in crossings {
        let found = pole_vertices.iter().find(|v| {
            model
                .node(v)
                .and_then(|n| n.data().as_vertex())
                .is_some_and(|d| d.point.distance(pole) <= tol.confusion())
        });
        let vertex = if let Some(found) = found {
            found.clone()
        } else {
            let made = ogeom_algo::make_vertex(model, pole).shape;
            pole_vertices.push(made.clone());
            made
        };
        if miss > tol.confusion() {
            model.widen(&vertex, ogeom_core::Tolerance::new(miss + tol.confusion())?)?;
        }
        stops.push((t, vertex));
    }
    stops.push((range.1, last));
    let mut pieces = Vec::with_capacity(stops.len() - 1);
    for pair in stops.windows(2) {
        let window = (pair[0].0, pair[1].0);
        let piece = ogeom_algo::make_edge_between(
            model,
            curve.clone(),
            window,
            &pair[0].1,
            &pair[1].1,
            tol,
        )?
        .shape;
        if let Some(stated) = tolerance {
            model.widen(&piece, stated)?;
        }
        pieces.push((piece, window));
    }
    Ok(pieces)
}

/// The other column of a seam the wire walked only one way: a period over,
/// toward the middle of the chart.
///
/// Which axis to step along is the surface's business, not the curve's. A
/// seam lies *on* the join, so it runs along the direction the surface does
/// not close and stands still in the one it does, and closure, not
/// periodicity, is the test, the same distinction a skinned wall forces
/// everywhere else in this module. On a patch closed in `v` over `(-π, π)`
/// whose `u` is a knot range that closes on nothing, stepping the second
/// column a `u` span sideways puts it off the chart entirely, and the face
/// draws as one flat triangle across itself.
///
/// Where the surface closes on neither axis the edge is a slit rather than
/// a seam (one curve, walked twice), and the same column serves both ways.
pub(crate) fn seam_other_side(
    image: &PlanarCurve,
    range: (f64, f64),
    surface: &SurfaceGeometry,
    tol: Tolerances,
) -> OgeomResult<PlanarCurve> {
    use ogeom_geom::Curve2d as _;
    use ogeom_geom::Surface as _;
    let ((ua, ub), (va, vb)) = surface.domain();
    let closed_u = surface.is_periodic_u() || surface.is_closed_u(tol);
    let closed_v = surface.is_periodic_v() || surface.is_closed_v(tol);
    let a = image.point_at(range.0, tol)?;
    let b = image.point_at(range.1, tol)?;
    let runs_in_u = (b.x - a.x).abs() >= (b.y - a.y).abs();
    let over_v = match (closed_u, closed_v) {
        (false, false) => return Ok(image.clone()),
        (true, false) => false,
        (false, true) => true,
        // Closed both ways: a torus. The seam stands still in the axis it
        // is a seam of, which is the one it does not run along.
        (true, true) => runs_in_u,
    };
    let mid = image.point_at(f64::midpoint(range.0, range.1), tol)?;
    let shift = if over_v {
        let span = vb - va;
        let d = if mid.y - va < span * 0.5 { span } else { -span };
        ogeom_math::Vector2::new(0.0, d)
    } else {
        let span = ub - ua;
        let d = if mid.x - ua < span * 0.5 { span } else { -span };
        ogeom_math::Vector2::new(d, 0.0)
    };
    image.transformed(&ogeom_math::Transform2::translation(shift), tol)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::*;
    use ogeom_geom::Curve2d as _;
    use ogeom_geom::{BSplineSurface, CylinderSurface, Line2d, PlaneSurface};
    use ogeom_math::{ControlGrid, Cylinder, Frame, KnotVector, Plane, Point, Point2};

    const T: Tolerances = Tolerances::millimetres();

    /// A cone's apex is a pole of its chart, and a ruling through it is
    /// found crossing there; one that only ends at the apex, or passes it
    /// by a tenth of a millimetre, is not.
    #[test]
    fn a_line_through_a_cone_apex_crosses_a_pole() {
        use ogeom_geom::{ConeSurface, Curve, LineCurve};
        use ogeom_math::{Axis, Cone, Direction};
        let cone: SurfaceGeometry = ConeSurface::new(
            Cone::new(Frame::WORLD, 2.0, core::f64::consts::FRAC_PI_4, T).unwrap(),
            (-10.0, 10.0),
        )
        .unwrap()
        .into();
        let poles = chart_poles(&cone, T);
        assert_eq!(poles.len(), 1);
        assert!(poles[0].distance(Point::new(0.0, 0.0, -2.0)) < 1e-12);
        let ruling = |through: Point| {
            let towards = Direction::new(Point::new(4.0, 0.0, 2.0) - through, T).unwrap();
            Curve::Line(LineCurve::new(Axis {
                location: through,
                direction: towards,
            }))
        };
        let line = ruling(Point::new(0.0, 0.0, -2.0));
        let found = pole_crossings(&line, (-3.0, 5.0), &poles, T);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].0.abs() < 1e-9 && found[0].2 < 1e-9, "{found:?}");
        assert!(pole_crossings(&line, (0.0, 5.0), &poles, T).is_empty());
        let beside = ruling(Point::new(0.0, 0.1, -2.0));
        assert!(pole_crossings(&beside, (-3.0, 5.0), &poles, T).is_empty());
    }

    /// A seam steps across the join the surface actually has.
    ///
    /// A boss around a bolt hole is a patch closed in `v` whose `u` is a
    /// knot range closing on nothing. Stepping its second column a `u` span
    /// sideways puts it off the chart, the wire never closes, and the face
    /// collapses to a single triangle laid across whatever it was a boss
    /// for. Closure decides the axis, not periodicity and not `u` by
    /// default: the same distinction a skinned wall forces everywhere.
    #[test]
    fn a_seam_steps_across_the_axis_its_surface_closes_on() {
        use ogeom_geom::Surface as _;
        let pi = core::f64::consts::PI;

        // A tube: one straight run in `u`, a closed square ring in `v`.
        let ring = [
            Point::new(0.0, 1.0, 0.0),
            Point::new(0.0, 0.0, 1.0),
            Point::new(0.0, -1.0, 0.0),
            Point::new(0.0, 0.0, -1.0),
            Point::new(0.0, 1.0, 0.0),
        ];
        let mut control = Vec::new();
        for i in 0..2 {
            for p in &ring {
                control.push(Point::new(f64::from(i), p.y, p.z));
            }
        }
        let tube: SurfaceGeometry = BSplineSurface::new(
            KnotVector::clamped_uniform(1, 2).unwrap(),
            KnotVector::new(vec![-pi, -pi, -pi / 2.0, 0.0, pi / 2.0, pi, pi], 1).unwrap(),
            &ControlGrid::new(control, 2, 5).unwrap(),
            T,
        )
        .unwrap()
        .into();
        assert!(!tube.is_closed_u(T) && tube.is_closed_v(T), "closed in v");

        // The seam runs along `v = -pi`, the whole way across `u`.
        let along: PlanarCurve = Line2d::segment(Point2::new(0.0, -pi), Point2::new(1.0, -pi), T)
            .unwrap()
            .into();
        let other = seam_other_side(&along, (0.0, 1.0), &tube, T).unwrap();
        let ends = (
            other.point_at(0.0, T).unwrap(),
            other.point_at(1.0, T).unwrap(),
        );
        assert!(
            (ends.0.y - pi).abs() < 1e-12 && (ends.1.y - pi).abs() < 1e-12,
            "stepped to the other v: {ends:?}"
        );
        assert!(
            (ends.0.x - 0.0).abs() < 1e-12 && (ends.1.x - 1.0).abs() < 1e-12,
            "and stayed where it was in u: {ends:?}"
        );

        // A cylinder closes the other way round, and the seam runs along
        // `u = 0` up its height; the step is a turn in `u`.
        let drum: SurfaceGeometry =
            CylinderSurface::new(Cylinder::new(Frame::WORLD, 3.0, T).unwrap(), (0.0, 10.0))
                .unwrap()
                .into();
        let up: PlanarCurve = Line2d::segment(Point2::new(0.0, 0.0), Point2::new(0.0, 10.0), T)
            .unwrap()
            .into();
        let over = seam_other_side(&up, (0.0, 1.0), &drum, T).unwrap();
        let at = over.point_at(0.0, T).unwrap();
        assert!(
            (at.x - core::f64::consts::TAU).abs() < 1e-12 && at.y.abs() < 1e-12,
            "a turn round in u: {at:?}"
        );

        // A surface that closes on neither axis has no other side: the
        // edge is a slit, one curve walked twice, and the column stands.
        let flat: SurfaceGeometry = PlaneSurface::over(
            Plane::through(Point::ORIGIN, ogeom_math::Direction::Z),
            (-5.0, 5.0),
            (-5.0, 5.0),
        )
        .unwrap()
        .into();
        let slit = seam_other_side(&along, (0.0, 1.0), &flat, T).unwrap();
        assert_eq!(
            slit.point_at(0.0, T).unwrap(),
            along.point_at(0.0, T).unwrap()
        );
    }
}
