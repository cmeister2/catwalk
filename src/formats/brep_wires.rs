//! Restore native BREP endpoint-connected traversals through public topology constructors.
//!
//! OCCT stores wire children independently of traversal order. Native vertex
//! orientation identifies edge endpoints: Forward is the start, Reversed the
//! end. The published Rust mesher expects those endpoints and wire edges in
//! traversal order; reordering retains every original occurrence and geometry.

use crate::Result;
use ogeom_core::Tolerances;
use ogeom_math::{Point, Transform};
use ogeom_topo::{Model, Orientation, SameKey, Shape, ShapeType, TShape, TShapeId};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Maximum native topology records admitted by the shared owned CAD loader.
const MAX_TOPOLOGY: usize = 100_000;
/// Maximum placement-alias comparisons and wire-edge traversal operations.
const MAX_WORK: usize = 1_000_000;

/// Placement-equivalent native vertex occurrences sharing one canonical endpoint.
struct Endpoint {
    /// Actual rigid or uniformly scaled placement of this native vertex occurrence.
    transform: Transform,
    /// Canonical endpoint index shared by equal node/placement occurrences.
    index: usize,
    /// Actual transformed point, preventing matrix-relative aliases from moving far vertices.
    point: Point,
    /// Authored vertex tolerance scaled into this occurrence's coordinates.
    tolerance: f64,
}

/// Bounded endpoint identity lookup retaining exact native vertex-node ownership.
#[derive(Default)]
struct Endpoints {
    /// Exact symbolic occurrence identities, ignoring orientation.
    exact: HashMap<SameKey, usize>,
    /// Matrix-equivalent placements for one immutable native vertex node.
    aliases: HashMap<TShapeId, Vec<Endpoint>>,
    /// Number of canonical node/placement endpoint identities.
    count: usize,
    /// Charged placement comparison work.
    work: usize,
}

impl Endpoints {
    /// Identify an endpoint without merging different native vertex nodes.
    ///
    /// # Errors
    ///
    /// Native placement evaluation, topology count or alias-comparison limits fail.
    fn index(&mut self, model: &Model, vertex: &Shape) -> Result<usize> {
        let key = SameKey(vertex.clone());
        if let Some(&index) = self.exact.get(&key) {
            return Ok(index);
        }
        let transform = vertex.transform(model.datums())?;
        let physical = vertex_point(model, vertex)?;
        let aliases = self.aliases.entry(vertex.node()).or_default();
        let mut matched = None;
        for endpoint in aliases.iter() {
            charge(&mut self.work, 1)?;
            if equivalent(transform, endpoint.transform)?
                && physical.point.distance(endpoint.point)
                    <= physical.tolerance.max(endpoint.tolerance)
                        + Tolerances::millimetres().confusion()
            {
                matched = Some(endpoint.index);
                break;
            }
        }
        let index = if let Some(index) = matched {
            index
        } else {
            if self.count >= MAX_TOPOLOGY {
                return Err("BREP wire endpoints exceed topology limit".into());
            }
            let index = self.count;
            self.count += 1;
            aliases.push(Endpoint {
                transform,
                index,
                point: physical.point,
                tolerance: physical.tolerance,
            });
            index
        };
        self.exact.insert(key, index);
        Ok(index)
    }
}

/// Reorder native endpoint and wire occurrences without changing their topology identities.
///
/// # Errors
///
/// Invalid endpoint roles, disconnected or ambiguous traversals, missing closure or work limits fail.
pub(super) fn restore(model: &mut Model) -> Result<()> {
    let nodes: Vec<_> = model
        .nodes()
        .map(|(id, node)| (Shape::of(id), node.kind()))
        .collect();
    if nodes.len() > MAX_TOPOLOGY {
        return Err("BREP wire normalization exceeds topology limit".into());
    }
    let mut face_wires = HashSet::new();
    for (shape, kind) in &nodes {
        if *kind == ShapeType::Face {
            for wire in model.node(shape).ok_or("BREP face disappeared")?.children() {
                face_wires.insert(wire.node());
            }
        }
    }
    for (shape, kind) in &nodes {
        if *kind != ShapeType::Edge {
            continue;
        }
        let node = model.node(shape).ok_or("BREP edge disappeared")?;
        let children = node.children();
        if children.len() != 2 {
            continue;
        }
        if let Ok((from, to)) = edge_ends(model, shape) {
            let start = children.iter().position(|vertex| *vertex == from);
            let end = children.iter().position(|vertex| *vertex == to);
            if let (Some(start), Some(end)) = (start, end)
                && start != end
                && start != 0
            {
                let reordered = vec![children[start].clone(), children[end].clone()];
                let data = node.data().clone();
                *model.node_mut(shape).ok_or("BREP edge disappeared")? =
                    TShape::new(ShapeType::Edge, data, reordered);
            }
        }
    }
    let mut endpoints = Endpoints::default();
    let mut work = 0usize;
    for (wire, kind) in nodes {
        if kind != ShapeType::Wire {
            continue;
        }
        // Free wires are previewed by evaluating each independent stored curve
        // range. They can validly omit topological endpoint vertices, and no
        // face-boundary walk consumes their child order.
        if !face_wires.contains(&wire.node()) {
            continue;
        }
        let edges = model
            .node(&wire)
            .ok_or("BREP wire disappeared")?
            .children()
            .to_vec();
        charge(&mut work, edges.len())?;
        let physical = edges
            .iter()
            .map(|edge| physical_edge_ends(model, edge))
            .collect::<Result<Vec<_>>>()?;
        // Published-kernel authored wires can use independent coincident
        // vertices, or retain Forward on both endpoint occurrences. Preserve
        // an already complete traversal proved from actual stored geometry.
        if physically_connected(&physical) {
            continue;
        }
        let mut ends = Vec::with_capacity(edges.len());
        for edge in &edges {
            let (start, end) = edge_ends(model, edge)?;
            ends.push((
                endpoints.index(model, &start)?,
                endpoints.index(model, &end)?,
            ));
        }
        let closed = true;
        if connected(&ends, closed) {
            continue;
        }
        let order = traversal(&ends, closed, &mut work)?;
        let reordered = order
            .into_iter()
            .map(|index| edges[index].clone())
            .collect();
        *model.node_mut(&wire).ok_or("BREP wire disappeared")? =
            TShape::container(ShapeType::Wire, reordered);
    }
    Ok(())
}

/// Select the oriented native start/end vertex roles, independent of original storage order.
///
/// # Errors
///
/// Required native endpoints or their distinct Forward/Reversed roles are absent or ambiguous.
fn edge_ends(model: &Model, edge: &Shape) -> Result<(Shape, Shape)> {
    let vertices = model.children_of(edge)?;
    if vertices.len() == 1 {
        let data = model
            .node(edge)
            .and_then(|node| node.data().as_edge())
            .ok_or("BREP wire edge is missing")?;
        if data.degenerate {
            return Ok((vertices[0].clone(), vertices[0].clone()));
        }
        if let Some(ogeom_topo::EdgeRepr::Curve3d { curve, range, .. }) = data.curve3d() {
            use ogeom_geom::Curve3d;
            let curve = model
                .geometry()
                .curve(*curve)
                .ok_or("BREP wire edge curve is missing")?;
            let tolerance = data
                .tolerance
                .get()
                .max(Tolerances::millimetres().confusion());
            if curve
                .point_at(range.0, Tolerances::millimetres())?
                .distance(curve.point_at(range.1, Tolerances::millimetres())?)
                <= tolerance
            {
                return Ok((vertices[0].clone(), vertices[0].clone()));
            }
        }
    }
    if vertices.len() != 2 {
        return Err("BREP wire requires supported native endpoint roles".into());
    }
    let forward: Vec<_> = vertices
        .iter()
        .filter(|vertex| vertex.orientation() == Orientation::Forward)
        .collect();
    let reversed: Vec<_> = vertices
        .iter()
        .filter(|vertex| vertex.orientation() == Orientation::Reversed)
        .collect();
    if forward.len() != 1 || reversed.len() != 1 {
        let first = vertex_point(model, &vertices[0])?;
        let second = vertex_point(model, &vertices[1])?;
        let data = model
            .node(edge)
            .and_then(|node| node.data().as_edge())
            .ok_or("BREP wire edge is missing")?;
        // The published sphere writer retains the same pole vertex twice,
        // both Forward, on a degenerate edge with only a surface pcurve.
        // Its native flag and actual coincident placed endpoints establish
        // a collapsed edge without inventing a spatial curve or vertices.
        if data.degenerate && matches_point(first, second) {
            return Ok((vertices[0].clone(), vertices[1].clone()));
        }
        let (start, end) = curve_ends(model, edge)?;
        if matches_point(start, first) && matches_point(end, second) {
            return Ok((vertices[0].clone(), vertices[1].clone()));
        }
        if matches_point(start, second) && matches_point(end, first) {
            return Ok((vertices[1].clone(), vertices[0].clone()));
        }
        return Err("BREP wire has ambiguous native endpoint roles inconsistent with stored curve endpoints".into());
    }
    Ok((forward[0].clone(), reversed[0].clone()))
}

/// Actual stored endpoint position and authored uncertainty, without synthesizing topology.
#[derive(Clone, Copy)]
struct PhysicalEndpoint {
    /// Actual curve or transformed vertex endpoint in the wire's coordinates.
    point: Point,
    /// Authored edge or vertex tolerance in the same coordinates.
    tolerance: f64,
}

/// Evaluate a native vertex under its actual complete occurrence placement.
///
/// # Errors
///
/// Vertex geometry, placement or finite authored uncertainty is invalid.
fn vertex_point(model: &Model, vertex: &Shape) -> Result<PhysicalEndpoint> {
    let data = model
        .node(vertex)
        .and_then(|node| node.data().as_vertex())
        .ok_or("BREP endpoint is missing native vertex geometry")?;
    let transform = vertex.transform(model.datums())?;
    let point = transform.apply(data.point);
    let tolerance = data.tolerance.get() * transform.scale_factor().abs();
    if ![point.x, point.y, point.z, tolerance]
        .into_iter()
        .all(f64::is_finite)
    {
        return Err("BREP endpoint has nonfinite placed geometry or tolerance".into());
    }
    Ok(PhysicalEndpoint { point, tolerance })
}

/// Evaluate the oriented start/end of one exact native 3D curve range.
///
/// # Errors
///
/// Required native curve, parameter range, placement or finite evaluations are invalid.
fn curve_ends(model: &Model, edge: &Shape) -> Result<(PhysicalEndpoint, PhysicalEndpoint)> {
    use ogeom_geom::Curve3d;
    let data = model
        .node(edge)
        .and_then(|node| node.data().as_edge())
        .ok_or("BREP wire edge geometry is missing")?;
    let Some(ogeom_topo::EdgeRepr::Curve3d { curve, range, .. }) = data.curve3d() else {
        return Err("BREP ambiguous wire endpoints lack a native 3D curve".into());
    };
    let geometry = model
        .geometry()
        .curve(*curve)
        .ok_or("BREP endpoint curve is missing")?;
    let placement = edge.transform(model.datums())?;
    let tolerance = data.tolerance.get() * placement.scale_factor().abs();
    let evaluate = |parameter| -> Result<PhysicalEndpoint> {
        let point = placement.apply(geometry.point_at(parameter, Tolerances::millimetres())?);
        if ![point.x, point.y, point.z, tolerance]
            .into_iter()
            .all(f64::is_finite)
        {
            return Err("BREP curve endpoints have nonfinite geometry or tolerance".into());
        }
        Ok(PhysicalEndpoint { point, tolerance })
    };
    let (start, end) = (evaluate(range.0)?, evaluate(range.1)?);
    Ok(if edge.orientation() == Orientation::Reversed {
        (end, start)
    } else {
        (start, end)
    })
}

/// Retain native vertex roles where valid, otherwise prove endpoints from the stored curve range.
///
/// # Errors
///
/// Neither supported native vertex roles nor actual 3D curve endpoints are available.
fn physical_edge_ends(model: &Model, edge: &Shape) -> Result<(PhysicalEndpoint, PhysicalEndpoint)> {
    match edge_ends(model, edge) {
        Ok((start, end)) => Ok((vertex_point(model, &start)?, vertex_point(model, &end)?)),
        Err(_) => curve_ends(model, edge),
    }
}

/// Whether actual authored endpoints meet within their native positional uncertainty.
fn matches_point(a: PhysicalEndpoint, b: PhysicalEndpoint) -> bool {
    a.point.distance(b.point)
        <= a.tolerance.max(b.tolerance) + Tolerances::millimetres().confusion()
}

/// Prove the existing complete closed wire traversal before changing any child order.
fn physically_connected(ends: &[(PhysicalEndpoint, PhysicalEndpoint)]) -> bool {
    !ends.is_empty()
        && ends
            .windows(2)
            .all(|pair| matches_point(pair[0].1, pair[1].0))
        && ends
            .last()
            .is_some_and(|end| matches_point(end.1, ends[0].0))
}

/// Whether all existing directed edges already form the required complete traversal.
fn connected(ends: &[(usize, usize)], closed: bool) -> bool {
    !ends.is_empty()
        && ends.windows(2).all(|pair| pair[0].1 == pair[1].0)
        && (!closed || ends.last().is_some_and(|end| end.1 == ends[0].0))
}

/// Find one complete native directed traversal, allowing ordered closed seam loops at a vertex.
///
/// # Errors
///
/// Degree imbalance, disconnected components, ambiguous nonloop branches or missing closure are rejected.
fn traversal(ends: &[(usize, usize)], closed: bool, work: &mut usize) -> Result<Vec<usize>> {
    let mut outgoing: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut degree = BTreeMap::<usize, (usize, usize)>::new();
    for (index, &(start, end)) in ends.iter().enumerate() {
        outgoing.entry(start).or_default().push(index);
        degree.entry(start).or_default().0 += 1;
        degree.entry(end).or_default().1 += 1;
    }
    let mut start = ends.first().ok_or("BREP wire has no edges")?.0;
    let mut open_start = None;
    let mut open_end = None;
    for (&vertex, &(outgoing, incoming)) in &degree {
        if outgoing == incoming {
            continue;
        }
        if !closed && outgoing == incoming + 1 && open_start.replace(vertex).is_none() {
            continue;
        }
        if !closed && incoming == outgoing + 1 && open_end.replace(vertex).is_none() {
            continue;
        }
        return Err("BREP wire has invalid directed endpoint degrees".into());
    }
    if open_start.is_some() != open_end.is_some() {
        return Err("BREP wire has incomplete open endpoints".into());
    }
    if let Some(vertex) = open_start {
        start = vertex;
    }
    let mut current = start;
    let mut order = Vec::with_capacity(ends.len());
    if closed {
        // A closed boundary has no distinguished starting vertex. Retain its
        // original first occurrence rather than treating that freely chosen
        // cyclic anchor as an ambiguous branch. Subsequent continuations must
        // still be unique, and the walk must consume every native occurrence.
        let first = outgoing
            .get_mut(&start)
            .ok_or("BREP wire is missing its original first occurrence")?;
        let index = first
            .iter()
            .position(|&index| index == 0)
            .ok_or("BREP wire is missing its original first occurrence")?;
        first.remove(index);
        order.push(0);
        current = ends[0].1;
    }
    for _ in order.len()..ends.len() {
        let candidates = outgoing
            .get_mut(&current)
            .ok_or("BREP wire has disconnected edge occurrences")?;
        charge(work, candidates.len())?;
        let nonloops = candidates
            .iter()
            .filter(|&&index| ends[index].0 != ends[index].1)
            .count();
        if nonloops > 1 {
            return Err("BREP unordered wire has ambiguous nonloop branches".into());
        }
        let chosen = candidates
            .iter()
            .position(|&index| ends[index].0 == ends[index].1)
            .unwrap_or(0);
        if candidates.is_empty() {
            return Err("BREP wire traversal closed before all occurrences".into());
        }
        let index = candidates.remove(chosen);
        order.push(index);
        current = ends[index].1;
    }
    let expected = open_end.unwrap_or(start);
    if current != expected || order.len() != ends.len() {
        return Err("BREP wire traversal is not complete or closed".into());
    }
    Ok(order)
}

/// Compare actual native occurrence placements while retaining one immutable vertex-node identity.
///
/// # Errors
///
/// Matrix access fails or a native placement cannot be interpreted.
fn equivalent(a: Transform, b: Transform) -> Result<bool> {
    if (a.translation_vector() - b.translation_vector()).magnitude()
        > Tolerances::millimetres().confusion()
    {
        return Ok(false);
    }
    for row in 0..3 {
        for column in 0..3 {
            let x = a.linear().get(row, column)? * a.scale_factor();
            let y = b.linear().get(row, column)? * b.scale_factor();
            if (x - y).abs() > 1e-9 * x.abs().max(y.abs()).max(1.0) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Bound all topology normalization and placement-comparison work before each step.
///
/// # Errors
///
/// Work arithmetic overflows or the one-million-operation ceiling is exceeded.
fn charge(work: &mut usize, additional: usize) -> Result<()> {
    *work = work
        .checked_add(additional)
        .filter(|&value| value <= MAX_WORK)
        .ok_or("BREP wire normalization exceeds work limit")?;
    Ok(())
}

/// Far-coordinate placement identities must retain their actual positional separation.
#[cfg(test)]
mod tests {
    use super::{Endpoints, Model, Point, Transform};
    use ogeom_math::{Matrix3, Vector};
    use ogeom_topo::Location;

    #[test]
    fn matrix_near_aliases_cannot_merge_endpoints_a_hundred_millimetres_apart() {
        let mut model = Model::new();
        let vertex = model.add_point(Point::new(1e12, 0.0, 0.0));
        let identity = model.add_datum(Transform::IDENTITY);
        let first = vertex.located(Location::of(identity));
        let another_identity = model.add_datum(Transform::IDENTITY);
        let equal = vertex.located(Location::of(another_identity));
        let angle = 1e-10_f64;
        let transform = Transform::from_parts(
            Matrix3::new([
                [angle.cos(), -angle.sin(), 0.0],
                [angle.sin(), angle.cos(), 0.0],
                [0.0, 0.0, 1.0],
            ]),
            1.0,
            Vector::ZERO,
            1e-9,
        )
        .unwrap();
        let rotation = model.add_datum(transform);
        let separate = vertex.located(Location::of(rotation));
        let mut endpoints = Endpoints::default();
        let original = endpoints.index(&model, &first).unwrap();
        assert_eq!(endpoints.index(&model, &equal).unwrap(), original);
        assert_ne!(endpoints.index(&model, &separate).unwrap(), original);
        let displacement = first
            .transform(model.datums())
            .unwrap()
            .apply(Point::new(1e12, 0.0, 0.0))
            .distance(
                separate
                    .transform(model.datums())
                    .unwrap()
                    .apply(Point::new(1e12, 0.0, 0.0)),
            );
        assert!((displacement - 100.0).abs() < 1e-9);
    }
}
