//! Restore native OCCT ASCII BREP carrier placements omitted by ogeom's
//! reader. Rigid placements are materialized into actual curve/surface
//! geometry; faces and their pcurves share the exact same transformed IDs.
use crate::Result;
use ogeom_core::Tolerances;
use ogeom_geom::{Curve, Curve3d, SurfaceGeometry, Transformable};
use ogeom_math::{Matrix3, Plane, Transform, TransformKind, Vector};
use ogeom_topo::{
    CurveId, EdgeRepr, Filter, Location, Model, NodeData, Shape, ShapeType, SurfaceId, explore,
};
use std::collections::HashMap;

/// Native topology kind and carrier-placement references for one BREP record.
struct Record {
    /// Native schema or topology kind selecting the record interpretation.
    kind: ShapeType,
    /// Representation-kind tags paired with native carrier-location indices.
    placements: Vec<(u8, usize)>,
}

/// Native carrier transforms and record associations needed to restore imported BREP geometry.
pub(super) struct Metadata {
    /// Decoded native carrier transforms; index zero is identity.
    transforms: Vec<Transform>,
    /// Canonical transform indices shared by exactly equivalent native placements.
    canonical: Vec<usize>,
    /// Native topology/carrier records in authored import order.
    records: Vec<Record>,
}

impl Metadata {
    /// Read native carrier placements and topology-record associations.
    ///
    /// # Errors
    ///
    /// Missing tables, invalid locations, scaled carriers or malformed representation references are rejected.
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
    )]
    pub(super) fn parse(text: &str) -> Result<Self> {
        let tokens: Vec<_> = text.split_whitespace().collect();
        let number = |at: usize| -> Result<usize> {
            Ok(tokens
                .get(at)
                .ok_or("truncated BREP carrier metadata")?
                .parse()?)
        };
        let table = |name: &str| -> Result<usize> {
            tokens
                .iter()
                .position(|word| *word == name)
                .ok_or_else(|| format!("BREP lacks {name} table").into())
        };
        let location_table = table("Locations")?;
        let location_count = number(location_table + 1)?;
        if location_count > super::MAX_TOPOLOGY {
            return Err("too many BREP carrier locations".into());
        }
        let mut cursor = location_table + 2;
        let mut transforms = vec![Transform::IDENTITY];
        for _ in 0..location_count {
            let kind = number(cursor)?;
            cursor += 1;
            let transform = match kind {
                1 => {
                    let mut rows = [[0.; 3]; 3];
                    let mut translation = [0.; 3];
                    for (row, offset) in rows.iter_mut().zip(&mut translation) {
                        for value in row.iter_mut().chain(std::iter::once(offset)) {
                            *value = tokens
                                .get(cursor)
                                .ok_or("truncated BREP carrier transform")?
                                .parse::<f64>()?;
                            cursor += 1;
                            if !value.is_finite() {
                                return Err("non-finite BREP carrier transform".into());
                            }
                        }
                    }
                    let scale = Matrix3::new(rows).determinant().cbrt();
                    if !scale.is_finite() || scale == 0. {
                        return Err("singular BREP carrier transform".into());
                    }
                    let normalized =
                        std::array::from_fn(|r| std::array::from_fn(|c| rows[r][c] / scale));
                    Transform::from_parts(
                        Matrix3::new(normalized),
                        scale,
                        Vector::new(translation[0], translation[1], translation[2]),
                        1e-9,
                    )?
                }
                2 => {
                    let mut composed = Transform::IDENTITY;
                    let mut links = 0;
                    loop {
                        let index = number(cursor)?;
                        cursor += 1;
                        if index == 0 {
                            break;
                        }
                        links += 1;
                        let power = tokens
                            .get(cursor)
                            .ok_or("truncated BREP carrier power")?
                            .parse::<i64>()?;
                        cursor += 1;
                        if index >= transforms.len() || links > 64 || power.unsigned_abs() > 64 {
                            return Err("BREP carrier composition exceeds limits".into());
                        }
                        let base = transforms[index];
                        let step = if power < 0 { base.inverse()? } else { base };
                        for _ in 0..power.unsigned_abs() {
                            composed = composed * step;
                        }
                    }
                    composed
                }
                _ => return Err("unsupported BREP carrier transform record".into()),
            };
            transforms.push(transform);
        }
        // Equal native matrices can have different directory indices. Share
        // their identity so a face and its pcurves cannot become disconnected
        // merely because the file repeats the same placement record.
        let mut canonical = Vec::with_capacity(transforms.len());
        let mut identities = HashMap::new();
        for (index, transform) in transforms.iter().enumerate() {
            let mut key = [0u64; 12];
            let offset = transform.translation_vector();
            for r in 0..3 {
                for c in 0..3 {
                    let value = transform.linear().get(r, c)? * transform.scale_factor();
                    key[r * 4 + c] = if value == 0. { 0 } else { value.to_bits() };
                }
                let value = [offset.x, offset.y, offset.z][r];
                key[r * 4 + 3] = if value == 0. { 0 } else { value.to_bits() };
            }
            canonical.push(*identities.entry(key).or_insert(index));
        }
        let curves = number(table("Curves")? + 1)?;
        let pcurves = number(table("Curve2ds")? + 1)?;
        let surfaces = number(table("Surfaces")? + 1)?;
        let geometry_id = |at: usize, total: usize| -> Result<()> {
            let index = number(at)?;
            if index == 0 || index > total {
                return Err("BREP carrier references missing geometry".into());
            }
            Ok(())
        };
        let carrier = |at: usize| -> Result<usize> {
            let index = number(at)?;
            let transform = transforms
                .get(index)
                .ok_or("BREP carrier references missing location")?;
            if (transform.scale_factor().abs() - 1.).abs() > 1e-9 {
                return Err("scaled BREP carrier locations are unsupported; occurrence scale remains supported".into());
            }
            Ok(index)
        };
        let topology = table("TShapes")?;
        let count = number(topology + 1)?;
        if count > super::MAX_TOPOLOGY {
            return Err("too many BREP topology records".into());
        }
        let mut records = Vec::with_capacity(count);
        for (at, word) in tokens.iter().enumerate().skip(topology + 2) {
            let kind = match *word {
                "Ve" => ShapeType::Vertex,
                "Ed" => ShapeType::Edge,
                "Wi" => ShapeType::Wire,
                "Fa" => ShapeType::Face,
                "Sh" => ShapeType::Shell,
                "So" => ShapeType::Solid,
                "CS" => ShapeType::CompSolid,
                "Co" => ShapeType::Compound,
                _ => continue,
            };
            let mut placements = Vec::new();
            if kind == ShapeType::Face {
                geometry_id(at + 3, surfaces)?;
                placements.push((0, carrier(at + 4)?));
            } else if kind == ShapeType::Edge {
                let mut cursor = at + 5;
                loop {
                    let repr = number(cursor)?;
                    cursor += 1;
                    let fields = match repr {
                        0 => break,
                        1 => {
                            geometry_id(cursor, curves)?;
                            placements.push((1, carrier(cursor + 1)?));
                            4
                        }
                        2 => {
                            geometry_id(cursor, pcurves)?;
                            geometry_id(cursor + 1, surfaces)?;
                            placements.push((2, carrier(cursor + 2)?));
                            5
                        }
                        3 => {
                            geometry_id(cursor, pcurves)?;
                            geometry_id(cursor + 1, pcurves)?;
                            geometry_id(cursor + 3, surfaces)?;
                            placements.push((3, carrier(cursor + 4)?));
                            7
                        }
                        // Continuity and cached mesh metadata do not produce
                        // typed curve/surface representations in the reader.
                        4 => 5,
                        5 => 2,
                        6 => 3,
                        7 => 4,
                        _ => return Err("unsupported BREP edge carrier representation".into()),
                    };
                    cursor = cursor
                        .checked_add(fields)
                        .ok_or("BREP carrier record overflow")?;
                    if cursor > tokens.len() {
                        return Err("truncated BREP carrier record".into());
                    }
                }
            }
            records.push(Record { kind, placements });
        }
        if records.len() != count {
            return Err("BREP carrier metadata disagrees with topology count".into());
        }
        Ok(Self {
            transforms,
            canonical,
            records,
        })
    }

    /// Materialize carrier transforms into shared curve/surface geometry and restore pcurves.
    ///
    /// # Errors
    ///
    /// Imported topology differs from native records, placements are unsupported or exact pcurve restoration fails.
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
    )]
    pub(super) fn restore(
        &self,
        model: &mut Model,
        root: &Shape,
        tolerances: Tolerances,
    ) -> Result<()> {
        // The native reader adds exactly one topology node per record, in
        // file order. Check both cardinality and every node kind before
        // attaching metadata, rather than assuming a handle representation.
        let nodes: Vec<_> = model.nodes().map(|(id, _)| Shape::of(id)).collect();
        if nodes.len() != self.records.len() {
            return Err("BREP import changed topology record count".into());
        }
        let mut surfaces = HashMap::<(SurfaceId, usize), SurfaceId>::new();
        let mut curves = HashMap::<(CurveId, usize), CurveId>::new();
        let mut face_carriers = HashMap::new();
        let mut edge_carriers = HashMap::new();
        for (record, shape) in self.records.iter().zip(nodes) {
            if model.kind_of(&shape)? != record.kind {
                return Err("BREP import changed topology record order".into());
            }
            let mut data = model
                .node(&shape)
                .ok_or("BREP topology node missing")?
                .data()
                .clone();
            match &mut data {
                NodeData::Face(face) => {
                    face_carriers.insert(shape.node(), (face.surface, record.placements[0].1));
                    face.surface = self.placed_surface(
                        model,
                        &mut surfaces,
                        face.surface,
                        record.placements[0].1,
                        tolerances,
                    )?;
                    face.location = Location::identity();
                }
                NodeData::Edge(edge) => {
                    edge_carriers.insert(
                        shape.node(),
                        edge.representations
                            .iter()
                            .cloned()
                            .zip(record.placements.iter().copied())
                            .collect::<Vec<_>>(),
                    );
                    if edge.representations.len() != record.placements.len() {
                        return Err("BREP import changed edge representation count".into());
                    }
                    for (representation, (kind, native)) in
                        edge.representations.iter_mut().zip(&record.placements)
                    {
                        let canonical = self.canonical[*native];
                        let transform = &self.transforms[*native];
                        match representation {
                            EdgeRepr::Curve3d {
                                curve, location, ..
                            } if *kind == 1 => {
                                if transform.kind() != TransformKind::Identity {
                                    let key = (*curve, canonical);
                                    *curve = if let Some(id) = curves.get(&key) {
                                        *id
                                    } else {
                                        let geometry = model
                                            .geometry()
                                            .curve(*curve)
                                            .ok_or("BREP carrier curve missing")?
                                            .transformed(transform, tolerances)?;
                                        let id = model.geometry_mut().add_curve(geometry);
                                        curves.insert(key, id);
                                        id
                                    };
                                }
                                *location = Location::identity();
                            }
                            EdgeRepr::PCurve { .. } if *kind == 2 => {}
                            EdgeRepr::Seam { .. } if *kind == 3 => {}
                            _ => return Err("BREP import changed edge representation kind".into()),
                        }
                    }
                    // Pcurve carriers are relative to their edge occurrence,
                    // not to the face occurrence. Associate them below only
                    // after comparing both complete placements.
                    edge.representations
                        .retain(|r| matches!(r, EdgeRepr::Curve3d { .. }));
                    // The previous check ran against the unplaced carriers.
                    // The mesher must reevaluate their actual relationship.
                    edge.assert_same_parameter(false);
                }
                _ => {}
            }
            *model
                .node_mut(&shape)
                .ok_or("BREP topology node missing during restoration")?
                .data_mut() = data;
        }
        let mut work = 0usize;
        let mut attached = 0usize;
        for face in explore(model, root, Filter::OfType(ShapeType::Face))? {
            let (original_surface, native) = face_carriers[&face.node()];
            let face_transform = face.transform(model.datums())? * self.transforms[native];
            let surface = model
                .node(&face)
                .and_then(|n| n.data().as_face())
                .ok_or("missing BREP face")?
                .surface;
            for edge in explore(model, &face, Filter::OfType(ShapeType::Edge))? {
                let edge_transform = edge.transform(model.datums())?;
                for (original, (_, native)) in &edge_carriers[&edge.node()] {
                    work = work.saturating_add(1);
                    if work > 10_000_000 {
                        return Err("BREP pcurve placement matching exceeds work limit".into());
                    }
                    if original.surface() != Some(original_surface)
                        || !same_placement(
                            &(edge_transform * self.transforms[*native]),
                            &face_transform,
                            tolerances,
                        )?
                    {
                        continue;
                    }
                    let mut representation = original.clone();
                    match &mut representation {
                        EdgeRepr::PCurve {
                            surface: on,
                            location,
                            ..
                        }
                        | EdgeRepr::Seam {
                            surface: on,
                            location,
                            ..
                        } => {
                            *on = surface;
                            *location = edge.location().clone();
                        }
                        _ => continue,
                    }
                    let NodeData::Edge(data) =
                        model.node_mut(&edge).ok_or("missing BREP edge")?.data_mut()
                    else {
                        return Err("missing BREP edge geometry".into());
                    };
                    work = work.saturating_add(data.representations.len());
                    if work > 10_000_000 {
                        return Err("BREP pcurve placement matching exceeds work limit".into());
                    }
                    if !data.representations.contains(&representation) {
                        attached += 1;
                        if attached > super::MAX_TOPOLOGY {
                            return Err("BREP exceeds 100000 placed pcurve representations".into());
                        }
                        data.representations.push(representation);
                    }
                }
                if restore_planar_pcurve(model, &face, &edge, surface, tolerances)? {
                    attached += 1;
                    if attached > super::MAX_TOPOLOGY {
                        return Err("BREP exceeds 100000 placed pcurve representations".into());
                    }
                }
            }
        }
        Ok(())
    }

    /// Find or create the surface geometry transformed by a canonical carrier.
    ///
    /// # Errors
    ///
    /// The surface or placement is missing, or transforming the geometry fails.
    fn placed_surface(
        &self,
        model: &mut Model,
        cache: &mut HashMap<(SurfaceId, usize), SurfaceId>,
        id: SurfaceId,
        native: usize,
        tolerances: Tolerances,
    ) -> Result<SurfaceId> {
        let transform = &self.transforms[native];
        if transform.kind() == TransformKind::Identity {
            return Ok(id);
        }
        let key = (id, self.canonical[native]);
        if let Some(placed) = cache.get(&key) {
            return Ok(*placed);
        }
        let geometry = model
            .geometry()
            .surface(id)
            .ok_or("BREP carrier surface missing")?
            .transformed(transform, tolerances)?;
        let placed = model.geometry_mut().add_surface(geometry);
        cache.insert(key, placed);
        Ok(placed)
    }
}

// A native pcurve is usable only where its surface carrier composed with
// its edge occurrence is the same placed surface as the face carrier.
/// Compare native placement matrices within kernel tolerances.
///
/// # Errors
///
/// Accessing or evaluating either transform fails.
fn same_placement(a: &Transform, b: &Transform, tolerances: Tolerances) -> Result<bool> {
    let av = a.translation_vector();
    let bv = b.translation_vector();
    if (av - bv).magnitude() > tolerances.confusion() {
        return Ok(false);
    }
    for r in 0..3 {
        for c in 0..3 {
            let x = a.linear().get(r, c)? * a.scale_factor();
            let y = b.linear().get(r, c)? * b.scale_factor();
            if (x - y).abs() > 1e-9 * x.abs().max(y.abs()).max(1.) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

// OCCT intentionally omits pcurves on planes and derives them lazily.
// The exact projection must preserve the original edge parameterization;
// proving coplanarity avoids flattening a malformed off-surface boundary.
/// Derive an omitted planar pcurve only after proving exact coplanarity.
///
/// # Errors
///
/// Required geometry is missing, relative scale is unsupported or the boundary cannot be projected exactly.
fn restore_planar_pcurve(
    model: &mut Model,
    face: &Shape,
    edge: &Shape,
    surface: SurfaceId,
    tolerances: Tolerances,
) -> Result<bool> {
    let data = model
        .node(edge)
        .and_then(|n| n.data().as_edge())
        .ok_or("missing BREP edge")?;
    if data.pcurve_for(surface, edge.location()).is_some() {
        return Ok(false);
    }
    let Some(SurfaceGeometry::Plane(plane)) = model.geometry().surface(surface) else {
        return Ok(false);
    };
    let plane = plane.plane();
    let Some(EdgeRepr::Curve3d { curve, range, .. }) = data.curve3d() else {
        return Ok(false);
    };
    let range = *range;
    let tolerance = data.tolerance.get().max(tolerances.confusion());
    let placement = face.transform(model.datums())?.inverse()? * edge.transform(model.datums())?;
    if (placement.scale_factor().abs() - 1.).abs() > 1e-9 {
        return Err(
            "scaled relative edge placement cannot preserve omitted planar pcurve parameters"
                .into(),
        );
    }
    let curve = model
        .geometry()
        .curve(*curve)
        .ok_or("missing BREP edge curve")?
        .transformed(&placement, tolerances)?;
    if !lies_in_plane(&curve, range, plane, tolerance, 0) {
        return Err("BREP omitted planar pcurve has an off-plane or unsupported boundary".into());
    }
    let geometry = model
        .geometry()
        .surface(surface)
        .ok_or("missing BREP face surface")?;
    let pcurve = ogeom_intersect::exact_pcurve_over(&curve, range, geometry, tolerances)
        .ok_or("BREP omitted planar pcurve cannot be derived exactly")?;
    let curve = model.geometry_mut().add_pcurve(pcurve);
    let NodeData::Edge(data) = model.node_mut(edge).ok_or("missing BREP edge")?.data_mut() else {
        return Err("missing BREP edge data".into());
    };
    data.representations.push(EdgeRepr::PCurve {
        curve,
        surface,
        location: edge.location().clone(),
        range,
    });
    Ok(true)
}

/// Prove supported curve bounds lie in a plane within the supplied tolerance.
fn lies_in_plane(
    curve: &Curve,
    range: (f64, f64),
    plane: Plane,
    tolerance: f64,
    depth: usize,
) -> bool {
    if depth > 64 {
        return false;
    }
    let frame = plane.frame();
    let distance = |point| frame.to_local(point).z.abs();
    let normal = frame.z().vector();
    match curve {
        Curve::Line(line) => [range.0, range.1].into_iter().all(|t| {
            line.point_at(t, super::TOLERANCES)
                .is_ok_and(|p| distance(p) <= tolerance)
        }),
        Curve::Circle(circle) => {
            let c = circle.circle();
            distance(c.centre())
                + c.radius()
                    * (normal.dot(c.frame().x().vector()).abs()
                        + normal.dot(c.frame().y().vector()).abs())
                <= tolerance
        }
        Curve::Ellipse(ellipse) => {
            let e = ellipse.ellipse();
            distance(e.centre())
                + e.major_radius() * normal.dot(e.frame().x().vector()).abs()
                + e.minor_radius() * normal.dot(e.frame().y().vector()).abs()
                <= tolerance
        }
        Curve::BSpline(spline) => spline
            .control_points()
            .iter()
            .all(|p| p.weight > 0. && distance(p.point()) <= tolerance),
        Curve::Trimmed(trim) => lies_in_plane(trim.basis(), range, plane, tolerance, depth + 1),
        _ => false,
    }
}
