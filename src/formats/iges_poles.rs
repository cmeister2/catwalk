//! Split genuine trim ranges at singular surface-chart points through public APIs.
use super::{Geometry, Segment};
use crate::Result;
use ogeom_core::{Tolerance, Tolerances};
use ogeom_geom::{Curve, Curve3d, Surface, SurfaceGeometry, TrimmedCurve};
use ogeom_math::Point;
use ogeom_topo::{Model, Shape};

/// Chart points that collapse a surface side to one 3D location.
///
/// # Errors
///
/// A surface cannot be evaluated over its declared finite domain.
pub(super) fn chart_points(surface: &SurfaceGeometry, tol: Tolerances) -> Result<Vec<Point>> {
    match surface {
        SurfaceGeometry::Plane(_)
        | SurfaceGeometry::Cylinder(_)
        | SurfaceGeometry::Extrusion(_) => return Ok(Vec::new()),
        SurfaceGeometry::Cone(cone) => return Ok(vec![cone.cone().apex()]),
        _ => {}
    }
    let ((u0, u1), (v0, v1)) = surface.domain();
    if ![u0, u1, v0, v1].iter().all(|v| v.is_finite()) {
        return Err("IGES pole chart has non-finite bounds".into());
    }
    let mut poles: Vec<Point> = Vec::new();
    for (a, b, c, d) in [
        (u0, v0, u1, v0),
        (u0, v1, u1, v1),
        (u0, v0, u0, v1),
        (u1, v0, u1, v1),
    ] {
        let first = surface.point_at(a, b, tol)?;
        let mut collapsed = true;
        for station in 1..=8 {
            let fraction = f64::from(station) / 8.;
            if surface
                .point_at(
                    (c - a).mul_add(fraction, a),
                    (d - b).mul_add(fraction, b),
                    tol,
                )?
                .distance(first)
                > tol.confusion()
            {
                collapsed = false;
                break;
            }
        }
        if collapsed && poles.iter().all(|p| p.distance(first) > tol.confusion()) {
            poles.push(first);
        }
    }
    Ok(poles)
}

/// Copy cost for actual source curve controls, with bounded adaptor nesting.
///
/// # Errors
///
/// Adaptors are unbounded or a curve kind cannot arise from supported IGES trims.
fn curve_fields(curve: &Curve, depth: usize) -> Result<usize> {
    if depth > 64 {
        return Err("IGES pole curve nesting exceeds64".into());
    }
    Ok(match curve {
        Curve::BSpline(s) => 4 * s.control_points().len() + s.knots().knots().len(),
        Curve::Offset(s) => 16 + curve_fields(s.basis(), depth + 1)?,
        Curve::Trimmed(s) => 4 + curve_fields(s.basis(), depth + 1)?,
        Curve::OnSurface(_) => {
            return Err("IGES unsimplified lifted pole curve is unsupported".into());
        }
        _ => 16,
    })
}

/// Retain every original curve over consecutive ranges separated by actual poles.
///
/// # Errors
///
/// Pole passages cannot be refined or expansion exceeds existing preview budgets.
pub(super) fn split(
    g: &mut Geometry<'_>,
    segments: Vec<Segment>,
    poles: &[Point],
) -> Result<Vec<Segment>> {
    if poles.is_empty() {
        return Ok(segments);
    }
    let tol = g.tol;
    let reach = tol.confusion() * 100.;
    let mut output = Vec::new();
    for (curve, range) in segments {
        let stations = ogeom_algo::traced::stations(&curve, range, 64);
        if stations.len() > 4097 {
            return Err("IGES pole curve exceeds4096 scan intervals".into());
        }
        let samples = stations
            .iter()
            .map(|&u| Ok((u, curve.point_at(u, tol)?)))
            .collect::<Result<Vec<_>>>()?;
        let head = curve.point_at(range.0, tol)?;
        let tail = curve.point_at(range.1, tol)?;
        let clear = (range.1 - range.0) * 1e-6;
        let mut cuts = Vec::new();
        for &pole in poles {
            for window in samples.windows(3) {
                if window[1].1.distance(pole) > window[0].1.distance(pole)
                    || window[1].1.distance(pole) > window[2].1.distance(pole)
                {
                    continue;
                }
                // The upstream public projector refines all local minima in this
                // genuine parameter window. No fitted/replacement curve is used.
                g.reserve_parameters(curve_fields(&curve, 0)?)?;
                let local: Curve =
                    TrimmedCurve::new(curve.clone(), window[0].0, window[2].0, tol)?.into();
                let projected = ogeom_algo::project_on_curve(&local, pole, 8, tol)?;
                let u = projected.parameter;
                if projected.distance <= reach
                    && u - range.0 > clear
                    && range.1 - u > clear
                    && projected.point.distance(head) > reach * 10.
                    && projected.point.distance(tail) > reach * 10.
                    && cuts
                        .iter()
                        .all(|&existing: &f64| (existing - u).abs() > clear)
                {
                    cuts.push(u);
                }
            }
        }
        cuts.sort_by(f64::total_cmp);
        g.reserve_parameters(
            curve_fields(&curve, 0)?
                .checked_mul(cuts.len())
                .ok_or("IGES pole expansion overflow")?,
        )?;
        let mut from = range.0;
        for cut in cuts {
            output.push((curve.clone(), (from, cut)));
            from = cut;
        }
        output.push((curve, (from, range.1)));
        if output.len() > super::super::MAX_TOPOLOGY {
            return Err("IGES pole expansion exceeds100000 edges".into());
        }
    }
    Ok(output)
}

/// Preserve the authored coincidence policy for vertices beside a chart pole.
///
/// # Errors
///
/// The original vertex or its tolerance cannot be read or widened by the kernel.
pub(super) fn cover_vertices(
    model: &mut Model,
    edges: &[Shape],
    poles: &[Point],
    tol: Tolerances,
) -> Result<()> {
    for edge in edges {
        if let Some((first, last)) = ogeom_algo::edge_vertices(model, edge)? {
            for vertex in [first, last] {
                let data = model
                    .node(&vertex)
                    .and_then(|n| n.data().as_vertex())
                    .ok_or("IGES pole vertex is missing")?;
                let miss = poles
                    .iter()
                    .map(|p| p.distance(data.point))
                    .fold(f64::INFINITY, f64::min);
                if miss > tol.confusion() && miss <= tol.confusion() * 100. {
                    model.widen(&vertex, Tolerance::new(miss + tol.confusion())?)?;
                }
            }
        }
    }
    Ok(())
}
