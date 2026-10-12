//! Preview-specific IGES loading through published ogeom APIs.
//!
//! Ordinary manifold/CSG/subfigure imports retain the upstream reader. Independent
//! trimmed surfaces are constructed individually, retaining every authored face
//! rather than attempting to infer a sewn shell. No dependency source is patched.
#![allow(
    clippy::many_single_char_names,
    reason = "Parameter-chart mappings use conventional u/v coordinates and curve ranges."
)]
use crate::Result;
use ogeom_core::{Tolerance, Tolerances};
use ogeom_geom::{Curve2d, Curve3d, Surface, SurfaceGeometry};
use ogeom_io::IgesReport;
use ogeom_math::{Point, Point2, Weighted};
use ogeom_topo::{EdgeRepr, Location, Model, Shape};
use std::collections::BTreeSet;

#[path = "iges_deck.rs"]
mod deck;
#[path = "iges_geometry.rs"]
mod geometry;
#[path = "iges_poles.rs"]
mod poles;
/// Self-contained authored inputs for library unit tests.
#[cfg(test)]
#[path = "iges_test_input.rs"]
mod test_input;
use deck::{Deck, Entity};
use geometry::{Geometry, Segment};

/// Geometry and strict ownership/compromise report for a thumbnail import.
pub(super) struct PreviewImport {
    /// Shared geometry/topology arenas.
    pub model: Model,
    /// Upstream-imported genuine manifold or constructive solid occurrences.
    pub solids: Vec<Shape>,
    /// Native sheets and a compound retaining every independent authored face.
    pub sheets: Vec<Shape>,
    /// Original native compromise report plus precise owned-definition accounting.
    pub report: IgesReport,
}

/// Root entity families delegated to the unchanged published topology reader.
const NATIVE_ROOTS: [i64; 14] = [
    150, 152, 154, 156, 158, 160, 162, 164, 168, 180, 184, 186, 408, 430,
];
/// True when directory status declares top-level geometry.
fn independent(e: &Entity) -> bool {
    (e.status / 10_000) % 100 == 0
}

/// Import native topology unchanged and build every independent trimmed face.
/// Import native topology unchanged and build every independent trimmed face.
///
/// # Errors
///
/// Invalid, unsupported, unclosed or unbounded geometry is rejected.
pub(super) fn read(text: &str, tol: Tolerances) -> Result<PreviewImport> {
    let deck = Deck::read(text)?;
    let native: Vec<_> = deck
        .entities
        .iter()
        .filter(|(_, e)| e.kind == 186 || (independent(e) && NATIVE_ROOTS.contains(&e.kind)))
        .map(|(&id, _)| id)
        .collect();
    let mut strong = BTreeSet::new();
    for &id in &native {
        closure(&deck, id, &mut strong, 0)?;
    }
    let roots: Vec<_> = deck
        .entities
        .iter()
        .filter(|(id, e)| matches!(e.kind, 143 | 144) && independent(e) && !strong.contains(id))
        .map(|(&id, _)| id)
        .collect();
    if roots.len() > super::MAX_FACES {
        return Err("IGES preview exceeds 4096 independent faces".into());
    }
    let mut g = Geometry::new(&deck, tol);
    let mut owned_model = Model::new();
    let mut faces = Vec::with_capacity(roots.len());
    for &id in &roots {
        faces.push(face(&mut g, &mut owned_model, id)?);
    }
    // A successfully consumed 142's alternative parameter curve is a validated
    // definition. Independent/unowned curves never acquire this exemption.
    let boundary_ids: Vec<_> = strong.union(&g.visited).copied().collect();
    for id in boundary_ids {
        let e = deck.entity(id)?;
        if e.kind == 142 && e.integer(3)? != 0 && e.integer(2)? != 0 {
            g.segments(e.integer(2)?)?;
        }
    }
    let owned = g.visited;
    let mask: BTreeSet<_> = owned.difference(&strong).copied().collect();
    let mut imported = if native.is_empty() {
        PreviewImport {
            model: owned_model,
            solids: Vec::new(),
            sheets: Vec::new(),
            report: IgesReport {
                scale_mm: deck.scale,
                ..IgesReport::default()
            },
        }
    } else {
        let delegated = deck.delegation(&mask, &roots)?;
        let ordinary = ogeom_io::read_iges(&delegated, tol)?;
        let mut result = PreviewImport {
            model: ordinary.document.model().clone(),
            solids: ordinary.solids,
            sheets: ordinary.sheets,
            report: ordinary.report,
        };
        // Shape IDs are arena-local. Build these exact original faces into the
        // delegated model rather than copying topology IDs between models.
        if !roots.is_empty() {
            let mut geometry = Geometry::new(&deck, tol);
            faces.clear();
            for &id in &roots {
                faces.push(face(&mut geometry, &mut result.model, id)?);
            }
        }
        result
    };
    if !faces.is_empty() {
        imported
            .sheets
            .push(ogeom_algo::make_compound(&mut imported.model, &faces)?.shape);
    }
    if imported.solids.is_empty() && imported.sheets.is_empty() {
        return Err("IGES contains no supported preview geometry".into());
    }
    // The native reader's skipped table is still authoritative for all genuine
    // native definitions. Replaced records are accounted for using ORIGINAL
    // entity IDs, never by subtracting an aggregated type count.
    if native.is_empty() {
        for (&id, e) in &deck.entities {
            if !owned.contains(&id) {
                let name = if e.form == 0 {
                    format!("type {}", e.kind)
                } else {
                    format!("type {} form {}", e.kind, e.form)
                };
                *imported.report.skipped.entry(name).or_default() += 1;
            }
        }
    }
    Ok(imported)
}

/// Strong entity references by declared IGES schema; coordinates and arbitrary
/// integers are never interpreted as pointers. Alternate142 charts are advisory.
/// Collect strong entity references by their declared IGES parameter schema.
///
/// # Errors
///
/// Invalid, unsupported, unclosed or unbounded geometry is rejected.
#[allow(
    clippy::too_many_lines,
    reason = "An explicit schema arm per entity prevents mistaking coordinates for references."
)]
fn references(e: &Entity) -> Result<Vec<i64>> {
    let mut refs = Vec::new();
    let mut add = |i| -> Result<()> {
        let id = e.integer(i)?;
        if id != 0 {
            refs.push(id);
        }
        Ok(())
    };
    match e.kind {
        102 => {
            for i in 0..e.count(0)? {
                add(1 + i)?;
            }
        }
        118 | 120 => {
            add(0)?;
            add(1)?;
        }
        122 | 130 | 162 | 164 | 408 | 430 => {
            add(0)?;
            if e.kind == 130 && e.integer(1)? == 3 {
                add(2)?;
            }
        }
        140 => add(4)?,
        141 => {
            add(2)?;
            let mut i = 4;
            for _ in 0..e.count(3)? {
                add(i)?;
                i += 3 + e.count(i + 2)?;
            }
        }
        142 => {
            add(1)?;
            if e.integer(3)? != 0 {
                add(3)?;
            } else {
                add(2)?;
            }
        }
        143 => {
            add(1)?;
            for i in 0..e.count(2)? {
                add(3 + i)?;
            }
        }
        144 => {
            add(0)?;
            if e.integer(1)? == 1 {
                add(3)?;
            }
            for i in 0..e.count(2)? {
                add(4 + i)?;
            }
        }
        180 => {
            for i in 0..e.count(0)? {
                if e.integer(1 + i)? < 0 {
                    add(1 + i)?;
                }
            }
        }
        184 => {
            let n = e.count(0)?;
            for i in 0..2 * n {
                add(1 + i)?;
            }
        }
        186 => {
            add(0)?;
            for i in 0..e.count(2)? {
                add(3 + 2 * i)?;
            }
        }
        190 => {
            add(0)?;
            add(1)?;
            add(2)?;
        }
        192 => {
            add(0)?;
            add(1)?;
            add(3)?;
        }
        194 | 198 => {
            add(0)?;
            add(1)?;
            add(4)?;
        }
        196 => {
            add(0)?;
            add(2)?;
            add(3)?;
        }
        308 => {
            for i in 0..e.count(2)? {
                add(3 + i)?;
            }
        }
        504 => {
            for i in 0..e.count(0)? {
                add(1 + 5 * i)?;
                add(2 + 5 * i)?;
                add(4 + 5 * i)?;
            }
        }
        508 => {
            let mut i = 1;
            for _ in 0..e.count(0)? {
                add(i + 1)?;
                let n = e.count(i + 4)?;
                for j in 0..n {
                    add(i + 6 + 2 * j)?;
                }
                i += 5 + 2 * n;
            }
        }
        510 => {
            add(0)?;
            for i in 0..e.count(1)? {
                add(3 + i)?;
            }
        }
        514 => {
            for i in 0..e.count(0)? {
                add(1 + 2 * i)?;
            }
        }
        _ => {}
    }
    if e.transform != 0 {
        refs.push(e.transform);
    }
    Ok(refs)
}

/// Walk only declared references with bounded depth and deduplicated entity IDs.
///
/// # Errors
///
/// Invalid, unsupported, unclosed or unbounded geometry is rejected.
fn closure(deck: &Deck, id: i64, seen: &mut BTreeSet<i64>, depth: usize) -> Result<()> {
    if depth > 64 {
        return Err("IGES topology reference depth exceeds64".into());
    }
    let id = id.checked_abs().ok_or("IGES pointer overflow")?;
    if !seen.insert(id) {
        return Ok(());
    }
    for child in references(deck.entity(id)?)? {
        closure(deck, child, seen, depth + 1)?;
    }
    Ok(())
}

/// Construct a complete independent face from its original surface and boundaries.
///
/// # Errors
///
/// Invalid, unsupported, unclosed or unbounded geometry is rejected.
fn face(g: &mut Geometry<'_>, model: &mut Model, id: i64) -> Result<Shape> {
    if model.node_count() >= super::MAX_TOPOLOGY {
        return Err("IGES expanded topology exceeds 100000 nodes".into());
    }
    let e = g.entity(id)?;
    let (surface_id, bounds) = match e.kind {
        144 => {
            let mut b = Vec::new();
            if e.integer(1)? == 1 && e.integer(3)? != 0 {
                b.push(e.integer(3)?);
            }
            for i in 0..e.count(2)? {
                b.push(e.integer(4 + i)?);
            }
            (e.integer(0)?, b)
        }
        143 => (
            e.integer(1)?,
            (0..e.count(2)?)
                .map(|i| e.integer(3 + i))
                .collect::<Result<Vec<_>>>()?,
        ),
        _ => return Err("IGES preview face is not143/144".into()),
    };
    let surface = g.surface(surface_id)?;
    if bounds.is_empty() {
        return Ok(ogeom_algo::make_natural_face(model, surface)?.shape);
    }
    let pole_points = poles::chart_points(&surface, g.tol)?;
    let sid = model.geometry_mut().add_surface(surface.clone());
    let mut wires = Vec::new();
    for boundary in bounds {
        let segments = boundary_segments(g, boundary)?;
        let segments = poles::split(g, segments, &pole_points)?;
        let edges = wire(model, segments, g.tol)?;
        poles::cover_vertices(model, &edges, &pole_points, g.tol)?;
        for edge in &edges {
            let (curve, range) = {
                let data = model
                    .node(edge)
                    .and_then(|n| n.data().as_edge())
                    .ok_or("missing IGES preview edge")?;
                let Some(EdgeRepr::Curve3d { curve, range, .. }) = data.curve3d() else {
                    return Err("missing IGES preview curve".into());
                };
                (
                    model
                        .geometry()
                        .curve(*curve)
                        .ok_or("missing IGES preview curve geometry")?
                        .clone(),
                    *range,
                )
            };
            let (pcurve, gap) = edge_image(&curve, range, &surface, g.tol)?;
            if gap > g.tol.confusion() {
                model.widen(edge, Tolerance::new(gap + g.tol.confusion())?)?;
            }
            ogeom_algo::attach_pcurve(model, edge, pcurve, sid, Location::identity(), range)?;
        }
        wires.push(ogeom_algo::make_wire(model, &edges, g.tol)?.shape);
    }
    ogeom_algo::chain_wire_branches(model, sid, &wires, g.tol)?;
    for w in &mut wires {
        let edges = model.ordered_children_of(w)?;
        let closed = ogeom_algo::closed_at_poles(model, sid, &edges, g.tol)?;
        if closed.len() != edges.len() {
            *w = ogeom_algo::make_wire(model, &closed, g.tol)?.shape;
        }
    }
    Ok(ogeom_algo::make_face_on(model, sid, &wires, g.tol)?.shape)
}

/// Derive a verified chart image through the public exact-projection/fitting APIs.
///
/// # Errors
///
/// Missing or inexact projections and any native compromise warning are rejected.
fn edge_image(
    curve: &ogeom_geom::Curve,
    range: (f64, f64),
    surface: &SurfaceGeometry,
    tol: Tolerances,
) -> Result<(ogeom_geom::PlanarCurve, f64)> {
    let pcurve = if let Some(exact) = ogeom_intersect::exact_pcurve_over(curve, range, surface, tol)
    {
        if let ogeom_geom::PlanarCurve::Line(line) = &exact {
            ogeom_geom::Line2d::over(
                line.axis(),
                line.domain().0.min(range.0),
                line.domain().1.max(range.1),
            )?
            .into()
        } else {
            exact
        }
    } else {
        let (p, error, met, _, slop) =
            ogeom_algo::pcurve_fit::fit_projected_pcurve(curve, range, surface, tol)?;
        if let Some(warning) = slop {
            return Err(format!("IGES import would compromise geometry: {warning}").into());
        }
        if !met {
            return Err(format!("IGES projected pcurve exceeds fit tolerance: {error}").into());
        }
        p
    };
    let gap =
        ogeom_algo::pcurve_fit::lifted_gap((curve, range), (&pcurve, range), surface, false, tol)?;
    Ok((pcurve, gap))
}

/// Decode original model-space boundary curves or lift an authored parameter chart.
///
/// # Errors
///
/// Invalid, unsupported, unclosed or unbounded geometry is rejected.
fn boundary_segments(g: &mut Geometry<'_>, id: i64) -> Result<Vec<Segment>> {
    let e = g.entity(id)?;
    match e.kind {
        142 => {
            if e.integer(3)? != 0 {
                g.segments(e.integer(3)?)
            } else {
                lifted_segments(g, e.integer(1)?, e.integer(2)?)
            }
        }
        141 => {
            let mut out = Vec::new();
            let mut i = 4;
            for _ in 0..e.count(3)? {
                let child = g.segments(e.integer(i)?)?;
                if out
                    .len()
                    .checked_add(child.len())
                    .is_none_or(|n| n > super::MAX_TOPOLOGY)
                {
                    return Err("IGES boundary exceeds preview edge limit".into());
                }
                out.extend(child);
                i += 3 + e.count(i + 2)?;
            }
            Ok(out)
        }
        _ => g.segments(id),
    }
}

/// Map a parameter-only boundary through the authored surface with checked fit error.
///
/// # Errors
///
/// Invalid, unsupported, unclosed or unbounded geometry is rejected.
#[allow(
    clippy::too_many_lines,
    reason = "Chart mapping, exact controls and verified piecewise lift form one bounded conversion."
)]
fn lifted_segments(
    g: &mut Geometry<'_>,
    surface_id: i64,
    boundary_id: i64,
) -> Result<Vec<Segment>> {
    let e = g.entity(surface_id)?;
    let kind = e.kind;
    let surface = g.surface(surface_id)?;
    let scale = g.deck.scale;
    let t = g.tol;
    let half_angle = match &surface {
        SurfaceGeometry::Cone(c) => c.cone().half_angle(),
        _ => 0.,
    };
    let swept = match (kind, &surface) {
        (122, SurfaceGeometry::Extrusion(s)) => {
            let (a, b) = s.curve().domain();
            let (c, d) = surface.domain().1;
            Some(((a, b - a), (c, d - c)))
        }
        (120, SurfaceGeometry::Revolution(s)) => match g.entity(e.integer(1)?)?.kind {
            110 => {
                let (a, b) = s.curve().domain();
                Some(((a, b - a), (0., 1.)))
            }
            126 => Some(((0., 1.), (0., 1.))),
            _ => None,
        },
        _ => None,
    };
    let start = e.number(2);
    let map = |p: Point| -> Option<Point2> {
        let (u, v) = (p.x / scale, p.y / scale);
        match kind {
            128 => Some(Point2::new(u, v)),
            190 => Some(Point2::new(u * scale, v * scale)),
            192 => Some(Point2::new(u.to_radians(), v * scale)),
            194 => Some(Point2::new(u.to_radians(), v * scale * half_angle.cos())),
            196 => Some(Point2::new(u.to_radians(), v.to_radians())),
            198 => Some(Point2::new((360. - v).to_radians(), u.to_radians())),
            122 => swept.map(|((a, b), (c, d))| Point2::new(a + b * u, c + d * v)),
            120 => swept.map(|((a, b), _)| Point2::new(v - start, a + b * u)),
            _ => None,
        }
    };
    if map(Point::ORIGIN).is_none() {
        return Err("IGES parameter-only boundary has unsupported surface chart".into());
    }
    let mut out = Vec::new();
    for (curve, range) in g.segments(boundary_id)? {
        let chart = if let ogeom_geom::Curve::BSpline(b) = &curve {
            let pts = b
                .control_points()
                .iter()
                .map(|p| {
                    let q = map(p.point()).ok_or("invalid IGES parameter chart")?;
                    Ok(Weighted::new(q, p.weight, t)?)
                })
                .collect::<Result<Vec<_>>>()?;
            ogeom_geom::BSpline2d::rational(b.knots().clone(), pts)?
        } else {
            let stations = ogeom_algo::traced::stations(&curve, range, 200);
            let fitted = ogeom_algo::traced::fit_traced_2d(
                |u| {
                    map(curve.point_at(u, t)?).ok_or_else(|| {
                        ogeom_core::ogeom_err!(Construction, "invalid IGES parameter chart")
                    })
                },
                &stations,
                3,
                1e-10,
                t,
            )?;
            if !fitted.met {
                return Err("IGES parameter chart exceeds fit tolerance".into());
            }
            fitted.curve
        };
        let corners = ogeom_algo::traced::corners_2d(&chart, range);
        let lifted = ogeom_geom::Curve::OnSurface(Box::new(ogeom_geom::CurveOnSurface::new(
            chart.into(),
            surface.clone(),
        )));
        let mut cuts = vec![range.0.min(range.1)];
        cuts.extend(corners);
        cuts.push(range.0.max(range.1));
        let target = t.confusion() * 100.;
        let mut pieces = Vec::new();
        let mut error: f64 = 0.;
        for pair in cuts.windows(2) {
            let fit = lifted.fitted_bspline_over((pair[0], pair[1]), target, t)?;
            error = error.max(fit.error);
            pieces.push(fit.curve);
        }
        let (b, moved) = if pieces.len() == 1 {
            (pieces.remove(0), 0.)
        } else {
            ogeom_algo::traced::joined(&pieces, t)?
        };
        if error + moved > target {
            return Err("IGES lifted boundary exceeds fit tolerance".into());
        }
        let range = b.domain();
        out.push((b.into(), range));
    }
    Ok(out)
}

/// Chain authored segments geometrically without sewing separate surfaces.
///
/// # Errors
///
/// Invalid, unsupported, unclosed or unbounded geometry is rejected.
fn wire(model: &mut Model, segments: Vec<Segment>, tol: Tolerances) -> Result<Vec<Shape>> {
    if segments.is_empty() {
        return Err("IGES boundary contains no curve segments".into());
    }
    if segments.len() > 100_000 {
        return Err("IGES boundary exceeds preview edge limit".into());
    }
    if model
        .node_count()
        .checked_add(2 * segments.len() + 2)
        .is_none_or(|n| n > super::MAX_TOPOLOGY)
    {
        return Err("IGES expanded topology exceeds 100000 nodes".into());
    }
    let ends = segments
        .iter()
        .map(|(c, r)| Ok((c.point_at(r.0, tol)?, c.point_at(r.1, tol)?)))
        .collect::<Result<Vec<_>>>()?;
    let weld = tol.confusion() * 100.;
    let walk = |start: Point| -> Option<Vec<bool>> {
        let mut at = start;
        let mut senses = Vec::new();
        for &(a, b) in &ends {
            let forward = at.distance(a) <= at.distance(b);
            if at.distance(if forward { a } else { b }) > weld {
                return None;
            }
            senses.push(forward);
            at = if forward { b } else { a };
        }
        (at.distance(start) <= weld).then_some(senses)
    };
    let (start, senses) = if let Some(s) = walk(ends[0].0) {
        (ends[0].0, s)
    } else if let Some(s) = walk(ends[0].1) {
        (ends[0].1, s)
    } else {
        return Err("IGES boundary segments do not form a closed chain".into());
    };
    let head = ogeom_algo::make_vertex(model, start).shape;
    let mut at = head.clone();
    let n = segments.len();
    let mut edges = Vec::new();
    for (i, ((c, r), forward)) in segments.into_iter().zip(senses).enumerate() {
        let end = if forward { ends[i].1 } else { ends[i].0 };
        let next = if i + 1 == n {
            head.clone()
        } else {
            ogeom_algo::make_vertex(model, end).shape
        };
        let edge = if forward {
            ogeom_algo::make_edge_between(model, c, r, &at, &next, tol)?.shape
        } else {
            ogeom_algo::make_edge_between(model, c, r, &next, &at, tol)?
                .shape
                .reversed()
        };
        edges.push(edge);
        at = next;
    }
    Ok(edges)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use ogeom_geom::{EllipseCurve, PlaneSurface};
    use ogeom_math::{Direction, Ellipse, Frame, Plane, Vector};
    use ogeom_topo::{Filter, ShapeType, explore};

    /// Triangle surface area for independently known primitive dimensions.
    fn area(scene: &crate::scene::Scene) -> f64 {
        scene
            .triangles
            .iter()
            .map(|triangle| {
                let [a, b, c] = triangle.vertices;
                (b - a).cross(c - a).length() * 0.5
            })
            .sum()
    }

    #[test]
    fn mixed_native_solid_and_owned_faces_keep_local_arenas_and_geometry() {
        let tol = Tolerances::millimetres();
        let mut model = Model::new();
        let frame = Frame::new(Point::new(30., 0., 0.), Direction::Z, Direction::X, tol).unwrap();
        let solid = ogeom_algo::make_box(&mut model, frame, (2., 3., 4.), tol)
            .unwrap()
            .shape;
        let path = ogeom_algo::make_polygon(
            &mut model,
            &[
                Point::ORIGIN,
                Point::new(10., 0., 0.),
                Point::new(10., 6., 0.),
            ],
            false,
            tol,
        )
        .unwrap()
        .shape;
        let sheet = ogeom_algo::make_prism(&mut model, &path, Vector::new(0., 0., 5.), tol)
            .unwrap()
            .shape;
        let mut document = ogeom_doc::Document::over(model);
        document.add_part("native_box", solid);
        document.add_part("authored_open_sheet", sheet);
        let source = ogeom_io::write_iges(&document, tol).unwrap();
        let imported = read(&source, tol).unwrap();
        assert_eq!(imported.solids.len(), 1);
        assert_eq!(imported.sheets.len(), 1);
        assert_eq!(imported.report.warnings, Vec::<String>::new());
        assert_eq!(
            explore(
                &imported.model,
                &imported.sheets[0],
                Filter::OfType(ShapeType::Face)
            )
            .unwrap()
            .len(),
            2
        );
        let scene = super::super::parse(source.as_bytes(), "iges").unwrap();
        assert_eq!(scene.triangles.len(), 16);
        assert!((area(&scene) - 132.).abs() < 1e-8);
        let high = scene.positions().fold(
            crate::scene::Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
            |a, p| crate::scene::Vec3::new(a.x.max(p.x), a.y.max(p.y), a.z.max(p.z)),
        );
        assert_eq!(high, crate::scene::Vec3::new(32., 6., 5.));
    }

    #[test]
    fn authored_conic_boundary_and_transform_preserve_disk_area_and_bounds() {
        let tol = Tolerances::millimetres();
        let mut model = Model::new();
        let frame = Frame::new(Point::new(2., -3., 4.), Direction::Z, Direction::Y, tol).unwrap();
        let ellipse = EllipseCurve::new(Ellipse::new(frame, 3., 2., tol).unwrap());
        let start = ellipse.point_at(0., tol).unwrap();
        let vertex = ogeom_algo::make_vertex(&mut model, start).shape;
        let edge = ogeom_algo::make_edge_between(
            &mut model,
            ellipse.into(),
            (0., std::f64::consts::TAU),
            &vertex,
            &vertex,
            tol,
        )
        .unwrap()
        .shape;
        let surface = PlaneSurface::over(Plane::new(frame), (-10., 10.), (-10., 10.)).unwrap();
        let face =
            ogeom_algo::make_face_with_pcurves(&mut model, surface.into(), &[vec![edge]], tol)
                .unwrap()
                .shape;
        let mut document = ogeom_doc::Document::over(model);
        document.add_part("authored_ellipse", face);
        let source = ogeom_io::write_iges(&document, tol).unwrap();
        assert!(
            source
                .lines()
                .any(|line| line.as_bytes()[72] == b'D' && line[..8].trim() == "104")
        );
        let scene = super::super::parse(source.as_bytes(), "iges").unwrap();
        let expected = 6. * std::f64::consts::PI;
        assert!((area(&scene) - expected).abs() / expected < 0.025);
        assert!(scene.positions().all(|p| p.x >= -1e-8
            && p.x <= 4. + 1e-8
            && p.y >= -6. - 1e-8
            && p.y <= 1e-8
            && (p.z - 4.).abs() < 1e-8));
    }

    #[test]
    fn independent_periodic_analytic_cylinder_retains_its_boundary_and_area() {
        let tol = Tolerances::millimetres();
        let mut model = Model::new();
        let cylinder = ogeom_algo::make_cylinder(&mut model, Frame::WORLD, 2., 3., tol)
            .unwrap()
            .shape;
        let lateral = explore(&model, &cylinder, Filter::OfType(ShapeType::Face))
            .unwrap()
            .into_iter()
            .find(|face| {
                model
                    .node(face)
                    .and_then(|n| n.data().as_face())
                    .and_then(|f| model.geometry().surface(f.surface))
                    .is_some_and(|s| matches!(s, SurfaceGeometry::Cylinder(_)))
            })
            .unwrap();
        let mut document = ogeom_doc::Document::over(model);
        document.add_part("authored_cylinder_wall", lateral);
        let source = ogeom_io::write_iges(&document, tol).unwrap();
        let imported = read(&source, tol).unwrap();
        assert_eq!(imported.report.warnings, Vec::<String>::new());
        assert_eq!(
            explore(
                &imported.model,
                &imported.sheets[0],
                Filter::OfType(ShapeType::Face)
            )
            .unwrap()
            .len(),
            1
        );
        let scene = super::super::parse(source.as_bytes(), "iges").unwrap();
        let expected = 12. * std::f64::consts::PI;
        assert!((area(&scene) - expected).abs() / expected < 0.025);
        assert!(scene.positions().all(|p| p.x.abs() <= 2. + 1e-8
            && p.y.abs() <= 2. + 1e-8
            && p.z >= -1e-8
            && p.z <= 3. + 1e-8));
    }

    /// Author a sphere with one vertical great-circle boundary passing through both poles.
    fn pole_crossing_hemisphere() -> String {
        use std::fmt::Write;

        let mut records = super::test_input::header();
        let global_count = records.len() - 1;
        let entities = [
            (116, 0, "0,0,0", 1),
            (196, 0, "1,2", 1),
            (124, 0, "1,0,0,0,0,0,-1,0,0,1,0,0", 1),
            (100, 5, "0,0,0,2,0,2,0", 1),
            (142, 0, "0,3,0,7,2", 1),
            (144, 0, "3,1,0,9", 0),
        ];
        let mut parameters = Vec::new();
        for (index, (kind, transform, data, subordinate)) in entities.into_iter().enumerate() {
            let id = index * 2 + 1;
            let first = [
                kind,
                index + 1,
                0,
                0,
                0,
                0,
                transform,
                0,
                subordinate * 10000,
            ];
            let second = [kind, 0, 0, 1, 0, 0, 0, 0, 0];
            for (fields, record_id) in [(first, id), (second, id + 1)] {
                let mut record = String::with_capacity(80);
                for value in fields {
                    write!(record, "{value:8}").unwrap();
                }
                write!(record, "D{record_id:7}").unwrap();
                records.push(record);
            }
            parameters.push(format!(
                "{:<64}{id:8}P{:7}",
                format!("{kind},{data};"),
                index + 1
            ));
        }
        records.extend(parameters);
        records.push(format!(
            "{:<72}T      1",
            format!(
                "S{:7}G{global_count:7}D{:7}P{:7}",
                1,
                entities.len() * 2,
                entities.len()
            )
        ));
        records.join("\n")
    }

    #[test]
    fn authored_great_circle_splits_at_interior_sphere_poles_without_losing_geometry() {
        let source = pole_crossing_hemisphere();
        let tol = Tolerances::millimetres();
        let imported = read(&source, tol).unwrap();
        assert_eq!(imported.report.warnings, Vec::<String>::new());
        assert_eq!(imported.sheets.len(), 1);
        let sheet = &imported.sheets[0];
        assert_eq!(
            explore(&imported.model, sheet, Filter::OfType(ShapeType::Face))
                .unwrap()
                .len(),
            1
        );
        let vertices = explore(&imported.model, sheet, Filter::OfType(ShapeType::Vertex)).unwrap();
        for pole in [Point::new(0., 0., -2.), Point::new(0., 0., 2.)] {
            assert!(vertices.iter().any(|vertex| {
                let point = imported
                    .model
                    .node(vertex)
                    .unwrap()
                    .data()
                    .as_vertex()
                    .unwrap()
                    .point;
                point.distance(pole) < tol.confusion() * 100.
            }));
        }
        let scene = super::super::parse(source.as_bytes(), "iges").unwrap();
        let expected = 8. * std::f64::consts::PI;
        assert!((area(&scene) - expected).abs() / expected < 0.02);
        assert!(scene.positions().all(|p| (p.length() - 2.).abs() < 1e-8));
        let bounds = scene.positions().fold(
            (
                crate::scene::Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
                crate::scene::Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
            ),
            |(low, high), p| {
                (
                    crate::scene::Vec3::new(low.x.min(p.x), low.y.min(p.y), low.z.min(p.z)),
                    crate::scene::Vec3::new(high.x.max(p.x), high.y.max(p.y), high.z.max(p.z)),
                )
            },
        );
        for actual in [bounds.0.x, bounds.0.z] {
            assert!((actual + 2.).abs() < 0.02);
        }
        for actual in [bounds.1.x, bounds.1.z] {
            assert!((actual - 2.).abs() < 0.02);
        }
        // A complete sphere has twice the expected area and both y signs.
        assert!(bounds.0.y >= -1e-8 || bounds.1.y <= 1e-8);
        assert!(bounds.0.y.abs().max(bounds.1.y.abs()) > 1.9);
    }
}
