//! The native `.og` format: a whole document, written so it can be read back.
//!
//! Topology, geometry, placements, per-entity tolerances, provenance and the
//! cached tessellation: everything a [`Model`] holds. This is what the
//! differential harness round-trips through, which sets the bar: a format that
//! quietly dropped what it did not handle would make every comparison through
//! it pass, including the ones that should not.
//!
//! # Text, not binary
//!
//! The point of a round trip is to *disagree usefully* when it goes wrong, and
//! a disagreement you can only see through a hex dump is one nobody will read.
//! So: one record per line, tagged, in dependency order: datums, geometry,
//! entities, then topology. A model written, read and written again produces
//! the same bytes, so `diff` is the whole comparison tool.
//!
//! Floats are printed in Rust's shortest round-tripping form, which is exact:
//! parsing what this writes gives the same `f64` back, bit for bit. It is not a
//! lossy text approximation of a binary truth.
//!
//! # Handles are preserved, not renumbered
//!
//! A document's entity identities are the thing references are recorded
//! against (`docs/DATA_MODEL.md` §8). Rebuilding a model through the builders
//! would mint fresh ones and quietly invalidate every reference in every
//! document that pointed into it, so reading goes through
//! [`Model::from_parts`], which reproduces the arenas exactly and checks that
//! every handle resolves.
//!
//! # What it refuses
//!
//! A representation, curve, surface or node kind this writer does not know how
//! to write is an error, not a silent omission. That is the whole discipline of
//! the format: `write` either produces a file that reads back as the same
//! model, or it says it cannot.

use std::fmt::Write as _;

use ogeom_core::{
    EntityId, Key, OgeomResult, OpId, Provenance, Role, SourceId, Tolerance, Tolerances, ogeom_bail,
};
use ogeom_geom::{
    BSpline2d, BSplineCurve, BSplineSurface, Circle2d, CircleCurve, ConeSurface, Curve, Curve2d,
    Curve3d, CylinderSurface, Ellipse2d, EllipseCurve, ExtrusionSurface, HelixCurve,
    HyperbolaCurve, Line2d, LineCurve, ParabolaCurve, PlanarCurve, PlaneSurface, RevolutionSurface,
    SphereSurface, Surface, SurfaceGeometry, TorusSurface, Trimmed2d, TrimmedCurve, TrimmedSurface,
};
use ogeom_math::{
    Axis, Axis2, Circle, Circle2, Cone, ControlGrid, Cylinder, Direction, Direction2, Ellipse,
    Ellipse2, Frame, Frame2, Hyperbola, KnotVector, Parabola, Plane, Point, Point2, Sphere, Torus,
    Transform, Vector, Weighted,
};
use ogeom_topo::{
    DatumId, EdgeData, EdgeRepr, FaceData, GeometryStore, Location, Model, ModelParts, NodeData,
    Orientation, Shape, ShapeType, TShape, TShapeId, Triangulation, VertexData,
};

pub use ogeom_topo::Absorbed;

/// The newest format version this writes.
///
/// Bumped when the grammar changes in a way an older reader could not follow.
/// A file naming a version this does not know is refused rather than guessed
/// at; every version up to this one still reads, because each version only
/// *added* to the one before: version 2 the conical helix and the periodic
/// B-spline, version 3 entity tables with gaps in their ids. A file is
/// written at the oldest version that holds it.
pub const VERSION: u32 = 3;

/// The version a file whose entity ids run without gaps is written at.
const GAPLESS_VERSION: u32 = 2;

/// The word every file starts with.
const MAGIC: &str = "ogeom";

/// What to include when writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOptions {
    /// Whether to write the cached triangulations.
    ///
    /// They are derived data (recomputable from the topology at any deflection)
    /// and far larger than the topology that produced them, so a file meant
    /// for reading by a human is better without. Left on by default, because
    /// the alternative is a round trip that is not the identity for a model
    /// that has meshes attached, and this is the format the harness compares
    /// through.
    pub triangulations: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            triangulations: true,
        }
    }
}

/// A fresh model holding only what `roots` reach, with the roots in it.
///
/// Every operation adds its results to a model, and one that fails or is
/// retried leaves what it built along the way, unreachable. A long session
/// that has run many operations carries all of that. The native
/// format writes exactly the reachable closure of its roots, so writing and
/// reading back is a compaction: the same shapes, same geometry, same
/// identities for what survives, in a model with nothing else in it. Of the
/// provenance it keeps the entries those identities name and their
/// ancestry; the ids of the rest stay issued and resolve to nothing.
///
/// The handles come back renumbered. To compact a model and keep the
/// handles into it, use [`Model::retain_reachable`].
///
/// # Errors
///
/// As [`write()`] and [`read()`].
pub fn compacted(model: &Model, roots: &[Shape]) -> OgeomResult<(Model, Vec<Shape>)> {
    read(&write(model, roots, WriteOptions::default())?)
}

/// Write a model, and the shapes it is a document *about*.
///
/// `roots` are recorded so a reader gets back the same handles the writer had.
/// A model with no roots is legal (it is a document of loose geometry), but a
/// reader then has nothing to start a traversal from.
///
/// With roots, the file holds what they reach: their nodes, geometry and
/// placements, and the provenance entries of the identities those nodes
/// carry with every entity those entries derive from. Every lineage query
/// on the written shapes answers as it did in `model`. With no roots, the
/// whole model is written.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) if the model holds
/// something this does not know how to write, which is refused rather than
/// dropped, or if a root does not resolve in the model.
pub fn write(model: &Model, roots: &[Shape], options: WriteOptions) -> OgeomResult<String> {
    write_renumbered(model, roots, options).map(|(text, _)| text)
}

/// [`write()`], and where the model's nodes landed in the file.
fn write_renumbered(
    model: &Model,
    roots: &[Shape],
    options: WriteOptions,
) -> OgeomResult<(String, Renumbered)> {
    for root in roots {
        if model.node(root).is_none() {
            ogeom_bail!(Construction, "a root shape is not in this model");
        }
    }
    // The file carries the reachable closure of `roots`, not the model: a
    // snapshot of one body from a large assembly is that body's records,
    // not everyone else's. Empty roots keep the whole
    // model; that is `write_document`'s contract, whose products name
    // shapes the root list does not. When the closure covers every node and
    // every piece of geometry, the subset would be a copy of the model
    // spelled the long way, so the model itself is written and the handles
    // keep their numbers.
    //
    // Provenance is kept the same way: the entries of the identities the
    // closure's nodes carry and the ancestry those entries name, which is
    // what a lineage query on the written shapes walks. A model that ran
    // many operations holds the entries of every intermediate result, and
    // none of those are anyone's ancestry.
    //
    // The closure is walked from the roots and looked up by handle, so the
    // cost follows what is written, not what the model has accumulated.
    //
    // A file numbers its records 0, 1, 2, ...; a model that has dropped
    // entries (`Model::retain_reachable`) has gaps between its handles, so
    // it is written as a subset of itself, renumbered, even when the
    // subset is all of it.
    let closure = if roots.is_empty() {
        if gapless(model) {
            let entries: Vec<(EntityId, &Provenance)> = model.provenance().iter().collect();
            return Ok((
                write_model(model, roots, options, &entries)?,
                Renumbered(None),
            ));
        }
        Closure::everything(model)
    } else {
        closure_of(model, roots)
    };
    let entries = closure.entries(model);
    if !roots.is_empty() && closure.covers(model) && gapless(model) {
        return Ok((
            write_model(model, roots, options, &entries)?,
            Renumbered(None),
        ));
    }
    let (parts, roots, node_map) = subset_parts(model, roots, &closure)?;
    let records = Records {
        scale: parts.tolerances.scale(),
        datums: parts
            .datums
            .iter()
            .enumerate()
            .map(|(i, datum)| (Key::from_parts(dense(i), 0), *datum)),
        geometry: &parts.geometry,
        entries: &entries,
        nodes: parts
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (Key::from_parts(dense(i), 0), node)),
        identities: parts.identity.iter().copied(),
        current_op: parts.current_op,
    };
    Ok((
        emit_records(records, &roots, options)?,
        Renumbered(Some(node_map)),
    ))
}

/// Where a written model's nodes landed in the file: under their own
/// handles, or renumbered by node index where the model was written as a
/// subset of itself.
struct Renumbered(Option<ogeom_core::FastMap<u32, u32>>);

impl Renumbered {
    /// The handle a node is written under.
    fn node(&self, id: TShapeId) -> OgeomResult<TShapeId> {
        match &self.0 {
            None => Ok(id),
            Some(map) => map
                .get(&id.index())
                .map(|&new| Key::from_parts(new, 0))
                .ok_or_else(|| {
                    ogeom_core::ogeom_err!(
                        Dangling,
                        "a document record names a node its model no longer holds"
                    )
                }),
        }
    }

    /// The shape a shape is written as.
    fn shape(&self, shape: &Shape) -> OgeomResult<Shape> {
        Ok(Shape::new(
            self.node(shape.node())?,
            shape.location().clone(),
            shape.orientation(),
        ))
    }
}

/// Whether the model's nodes and geometry sit at indices 0, 1, 2, ... at
/// generation zero, as a file's records are numbered.
fn gapless(model: &Model) -> bool {
    fn run<T>(mut keys: impl Iterator<Item = Key<T>>) -> bool {
        keys.by_ref()
            .enumerate()
            .all(|(i, key)| key.index() as usize == i && key.generation() == 0)
    }
    let geometry = model.geometry();
    run(model.nodes().map(|(id, _)| id))
        && run(geometry.curves().map(|(id, _)| id))
        && run(geometry.pcurves().map(|(id, _)| id))
        && run(geometry.surfaces().map(|(id, _)| id))
        && run(geometry.triangulations().map(|(id, _)| id))
}

/// A dense index for an entry of a subset. The source arena held it as
/// u32, and a subset is no larger, so the fit is by construction.
fn dense(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Which of each arena's entries the roots reach, by handle.
struct Closure {
    nodes: ogeom_core::FastSet<TShapeId>,
    datums: ogeom_core::FastSet<ogeom_topo::DatumId>,
    curves: ogeom_core::FastSet<ogeom_topo::CurveId>,
    pcurves: ogeom_core::FastSet<ogeom_topo::PCurveId>,
    surfaces: ogeom_core::FastSet<ogeom_topo::SurfaceId>,
    meshes: ogeom_core::FastSet<ogeom_topo::TriangulationId>,
    /// The entities whose entries are written: every identity a retained
    /// node carries and every entity those entries derive from, however far
    /// back. Ids are untouched, so a consumer's recorded `EntityId`s stay
    /// valid, which `provenance_and_identity_survive` holds the writer to.
    entities: ogeom_core::FastSet<EntityId>,
}

impl Closure {
    /// Every live entry of the model.
    fn everything(model: &Model) -> Self {
        let geometry = model.geometry();
        Self {
            nodes: model.nodes().map(|(id, _)| id).collect(),
            datums: model.datums().iter().map(|(id, _)| id).collect(),
            curves: geometry.curves().map(|(id, _)| id).collect(),
            pcurves: geometry.pcurves().map(|(id, _)| id).collect(),
            surfaces: geometry.surfaces().map(|(id, _)| id).collect(),
            meshes: geometry.triangulations().map(|(id, _)| id).collect(),
            entities: model.provenance().iter().map(|(id, _)| id).collect(),
        }
    }

    /// Whether the closure keeps every node and every piece of geometry.
    fn covers(&self, model: &Model) -> bool {
        let (curves, pcurves, surfaces) = model.geometry().counts();
        self.curves.len() == curves
            && self.pcurves.len() == pcurves
            && self.surfaces.len() == surfaces
            && self.nodes.len() == model.nodes().count()
    }

    /// The kept entities' entries, in the order their ids were issued.
    fn entries<'m>(&self, model: &'m Model) -> Vec<(EntityId, &'m Provenance)> {
        let mut ids: Vec<EntityId> = self.entities.iter().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|id| Some((id, model.provenance().get(id)?)))
            .collect()
    }
}

/// Walk the roots' sub-shape graphs and note everything they touch.
fn closure_of(model: &Model, roots: &[Shape]) -> Closure {
    let mut c = Closure {
        nodes: ogeom_core::FastSet::default(),
        datums: ogeom_core::FastSet::default(),
        curves: ogeom_core::FastSet::default(),
        pcurves: ogeom_core::FastSet::default(),
        surfaces: ogeom_core::FastSet::default(),
        meshes: ogeom_core::FastSet::default(),
        entities: ogeom_core::FastSet::default(),
    };
    let note_location = |datums: &mut ogeom_core::FastSet<ogeom_topo::DatumId>, l: &Location| {
        for (datum, _) in l.chain() {
            datums.insert(*datum);
        }
    };
    let mut queue: Vec<TShapeId> = Vec::new();
    for root in roots {
        note_location(&mut c.datums, root.location());
        if c.nodes.insert(root.node()) {
            queue.push(root.node());
        }
    }
    let mut ancestry: Vec<EntityId> = Vec::new();
    while let Some(id) = queue.pop() {
        let Some(node) = model.node_by_id(id) else {
            continue;
        };
        if let Some(entity) = model.identity_of(&Shape::of(id))
            && c.entities.insert(entity)
        {
            ancestry.push(entity);
        }
        for child in node.children() {
            note_location(&mut c.datums, child.location());
            if c.nodes.insert(child.node()) {
                queue.push(child.node());
            }
        }
        match node.data() {
            NodeData::Edge(e) => {
                for repr in &e.representations {
                    match repr {
                        EdgeRepr::Curve3d {
                            curve, location, ..
                        } => {
                            c.curves.insert(*curve);
                            note_location(&mut c.datums, location);
                        }
                        EdgeRepr::PCurve {
                            curve,
                            surface,
                            location,
                            ..
                        } => {
                            c.pcurves.insert(*curve);
                            c.surfaces.insert(*surface);
                            note_location(&mut c.datums, location);
                        }
                        EdgeRepr::Seam {
                            forward,
                            reversed,
                            surface,
                            location,
                            ..
                        } => {
                            c.pcurves.insert(*forward);
                            c.pcurves.insert(*reversed);
                            c.surfaces.insert(*surface);
                            note_location(&mut c.datums, location);
                        }
                        EdgeRepr::Polyline { location, .. } => {
                            note_location(&mut c.datums, location);
                        }
                        EdgeRepr::PolygonOnTriangulation { triangulation, .. } => {
                            c.meshes.insert(*triangulation);
                        }
                        // The enum is non-exhaustive; a representation this
                        // walker does not know cannot name handles it should
                        // retain, and the writer itself would refuse it first.
                        _ => {}
                    }
                }
            }
            NodeData::Face(f) => {
                c.surfaces.insert(f.surface);
                note_location(&mut c.datums, &f.location);
                if let Some(mesh) = f.triangulation {
                    c.meshes.insert(mesh);
                }
            }
            NodeData::Vertex(_) | NodeData::Container => {}
        }
    }
    while let Some(entity) = ancestry.pop() {
        if let Some(entry) = model.provenance().get(entity) {
            for &from in entry.inputs() {
                if c.entities.insert(from) {
                    ancestry.push(from);
                }
            }
        }
    }
    c
}

/// The handles of `set`, in arena order, and the map from each one's old
/// index to its new dense one.
fn renumbered<T>(
    set: &ogeom_core::FastSet<Key<T>>,
) -> (Vec<Key<T>>, ogeom_core::FastMap<u32, u32>) {
    let mut ordered: Vec<Key<T>> = set.iter().copied().collect();
    ordered.sort_unstable_by_key(|k| k.index());
    let map = ordered
        .iter()
        .enumerate()
        .map(|(i, k)| (k.index(), dense(i)))
        .collect();
    (ordered, map)
}

/// The closure rebuilt as its own parts, every handle re-densified in the
/// model's own order, plus the roots respelled in the new numbering and
/// each node's new index by its old one. The provenance is not among the
/// parts: the writer takes the kept entries from the model as they are.
fn subset_parts(
    model: &Model,
    roots: &[Shape],
    closure: &Closure,
) -> OgeomResult<(ModelParts, Vec<Shape>, ogeom_core::FastMap<u32, u32>)> {
    let missing = || ogeom_core::ogeom_err!(Dangling, "the closure misses a referenced handle");
    // Old index -> new dense index, in arena order, so the same model and
    // roots write the same bytes every time.
    let (nodes_in_order, node_map) = renumbered(&closure.nodes);
    let (datum_ids, datum_map) = renumbered(&closure.datums);
    let datums = datum_ids
        .iter()
        .map(|&id| model.datums().get(id).ok_or_else(missing))
        .collect::<OgeomResult<Vec<_>>>()?;
    let geometry_in = model.geometry();
    let mut geometry = GeometryStore::new();
    let (curve_ids, curve_map) = renumbered(&closure.curves);
    for id in curve_ids {
        geometry.add_curve(geometry_in.curve(id).ok_or_else(missing)?.clone());
    }
    let (pcurve_ids, pcurve_map) = renumbered(&closure.pcurves);
    for id in pcurve_ids {
        geometry.add_pcurve(geometry_in.pcurve(id).ok_or_else(missing)?.clone());
    }
    let (surface_ids, surface_map) = renumbered(&closure.surfaces);
    for id in surface_ids {
        geometry.add_surface(geometry_in.surface(id).ok_or_else(missing)?.clone());
    }
    let (mesh_ids, mesh_map) = renumbered(&closure.meshes);
    for id in mesh_ids {
        geometry.add_triangulation(geometry_in.triangulation(id).ok_or_else(missing)?.clone());
    }

    let relocate = |l: &Location| -> OgeomResult<Location> {
        let mut out = Location::identity();
        for (datum, power) in l.chain() {
            let new = *datum_map.get(&datum.index()).ok_or_else(missing)?;
            out = out.then(&Location::powered(Key::from_parts(new, 0), *power));
        }
        Ok(out)
    };
    let reshape = |s: &Shape| -> OgeomResult<Shape> {
        let new = *node_map.get(&s.node().index()).ok_or_else(missing)?;
        Ok(Shape::new(
            Key::from_parts(new, 0),
            relocate(s.location())?,
            s.orientation(),
        ))
    };

    let mut nodes: Vec<TShape> = Vec::new();
    for id in &nodes_in_order {
        let node = model.node_by_id(*id).ok_or_else(missing)?;
        let data = match node.data() {
            NodeData::Vertex(v) => {
                NodeData::Vertex(VertexData::with_tolerance(v.point, v.tolerance.get())?)
            }
            NodeData::Edge(e) => {
                let mut edge = EdgeData::new();
                edge.tolerance = e.tolerance;
                edge.degenerate = e.degenerate;
                for repr in &e.representations {
                    edge.add(match repr {
                        EdgeRepr::Curve3d {
                            curve,
                            location,
                            range,
                        } => EdgeRepr::Curve3d {
                            curve: Key::from_parts(
                                *curve_map.get(&curve.index()).ok_or_else(missing)?,
                                0,
                            ),
                            location: relocate(location)?,
                            range: *range,
                        },
                        EdgeRepr::PCurve {
                            curve,
                            surface,
                            location,
                            range,
                        } => EdgeRepr::PCurve {
                            curve: Key::from_parts(
                                *pcurve_map.get(&curve.index()).ok_or_else(missing)?,
                                0,
                            ),
                            surface: Key::from_parts(
                                *surface_map.get(&surface.index()).ok_or_else(missing)?,
                                0,
                            ),
                            location: relocate(location)?,
                            range: *range,
                        },
                        EdgeRepr::Seam {
                            forward,
                            reversed,
                            surface,
                            location,
                            range,
                        } => EdgeRepr::Seam {
                            forward: Key::from_parts(
                                *pcurve_map.get(&forward.index()).ok_or_else(missing)?,
                                0,
                            ),
                            reversed: Key::from_parts(
                                *pcurve_map.get(&reversed.index()).ok_or_else(missing)?,
                                0,
                            ),
                            surface: Key::from_parts(
                                *surface_map.get(&surface.index()).ok_or_else(missing)?,
                                0,
                            ),
                            location: relocate(location)?,
                            range: *range,
                        },
                        EdgeRepr::Polyline {
                            points,
                            parameters,
                            location,
                            deflection,
                        } => EdgeRepr::Polyline {
                            points: points.clone(),
                            parameters: parameters.clone(),
                            location: relocate(location)?,
                            deflection: *deflection,
                        },
                        EdgeRepr::PolygonOnTriangulation {
                            triangulation,
                            indices,
                            location,
                        } => EdgeRepr::PolygonOnTriangulation {
                            triangulation: Key::from_parts(
                                *mesh_map.get(&triangulation.index()).ok_or_else(missing)?,
                                0,
                            ),
                            indices: indices.clone(),
                            location: relocate(location)?,
                        },
                        other => ogeom_bail!(
                            Construction,
                            "an edge representation this writer does not know: {other:?}"
                        ),
                    });
                }
                edge.assert_same_parameter(e.same_parameter());
                NodeData::Edge(Box::new(edge))
            }
            NodeData::Face(f) => {
                let surface =
                    Key::from_parts(*surface_map.get(&f.surface.index()).ok_or_else(missing)?, 0);
                let at = relocate(&f.location)?;
                let mut face = if f.natural_restriction {
                    FaceData::natural(surface, at)
                } else {
                    FaceData::new(surface, at)
                };
                face.tolerance = f.tolerance;
                face.triangulation = match f.triangulation {
                    Some(mesh) => Some(Key::from_parts(
                        *mesh_map.get(&mesh.index()).ok_or_else(missing)?,
                        0,
                    )),
                    None => None,
                };
                NodeData::Face(Box::new(face))
            }
            NodeData::Container => NodeData::Container,
        };
        let children = node
            .children()
            .iter()
            .map(&reshape)
            .collect::<OgeomResult<Vec<_>>>()?;
        nodes.push(TShape::new(node.kind(), data, children));
    }

    let mut identity = Vec::new();
    for (new, id) in nodes_in_order.iter().enumerate() {
        if let Some(entity) = model.identity_of(&Shape::of(*id)) {
            identity.push((Key::from_parts(dense(new), 0), entity));
        }
    }

    let parts = ModelParts {
        nodes,
        datums,
        geometry,
        provenance: Vec::new(),
        identity,
        current_op: model.current_operation(),
        tolerances: model.tolerances(),
    };
    let respelled = roots
        .iter()
        .map(&reshape)
        .collect::<OgeomResult<Vec<_>>>()?;
    Ok((parts, respelled, node_map))
}

/// Write `model` whole, with `entries` for its provenance.
fn write_model(
    model: &Model,
    roots: &[Shape],
    options: WriteOptions,
    entries: &[(EntityId, &Provenance)],
) -> OgeomResult<String> {
    let records = Records {
        scale: model.tolerances().scale(),
        datums: model.datums().iter(),
        geometry: model.geometry(),
        entries,
        nodes: model.nodes(),
        identities: model.identities(),
        current_op: model.current_operation(),
    };
    emit_records(records, roots, options)
}

/// Everything a file holds, in the order it is written.
struct Records<'a, D, N, I> {
    scale: f64,
    datums: D,
    geometry: &'a GeometryStore,
    /// Ascending by id.
    entries: &'a [(EntityId, &'a Provenance)],
    nodes: N,
    identities: I,
    current_op: OpId,
}

/// The text of `records` and `roots`, at the oldest version that holds it.
fn emit_records<'a, D, N, I>(
    records: Records<'a, D, N, I>,
    roots: &[Shape],
    options: WriteOptions,
) -> OgeomResult<String>
where
    D: Iterator<Item = (ogeom_topo::DatumId, ogeom_topo::Datum)>,
    N: Iterator<Item = (TShapeId, &'a TShape)>,
    I: Iterator<Item = (TShapeId, EntityId)>,
{
    // A table with gaps in its ids is what version 3 added; a file without
    // them is written as version 2, which every reader since then follows.
    let gapped = records
        .entries
        .iter()
        .enumerate()
        .any(|(i, (id, _))| id.get() != i as u64 + 1);
    let mut out = String::new();
    out.push_str(&format!(
        "{MAGIC} {}\n",
        if gapped { VERSION } else { GAPLESS_VERSION }
    ));

    // The unit scale, first, because everything after it is measured in those
    // units. A document that did not record it would read back correctly and be
    // *validated* against whatever the reader assumed, so geometry legitimate
    // at one scale could be refused at another, and nothing would say why.
    let mut t = Tokens::default();
    w(&mut t, "units");
    n(&mut t, records.scale);
    emit(&mut out, &t);

    for (id, datum) in records.datums {
        let mut t = Tokens::default();
        w(&mut t, "datum");
        key(&mut t, id);
        transform(&mut t, &datum)?;
        emit(&mut out, &t);
    }

    let geometry = records.geometry;
    for (id, c) in geometry.curves() {
        let mut t = Tokens::default();
        w(&mut t, "curve");
        key(&mut t, id);
        curve(&mut t, c)?;
        emit(&mut out, &t);
    }
    for (id, c) in geometry.pcurves() {
        let mut t = Tokens::default();
        w(&mut t, "pcurve");
        key(&mut t, id);
        pcurve(&mut t, c)?;
        emit(&mut out, &t);
    }
    for (id, s) in geometry.surfaces() {
        let mut t = Tokens::default();
        w(&mut t, "surface");
        key(&mut t, id);
        surface(&mut t, s)?;
        emit(&mut out, &t);
    }
    if options.triangulations {
        for (id, mesh) in geometry.triangulations() {
            write_mesh(&mut out, id, mesh);
        }
    } else if geometry.triangulation_count() > 0 {
        // Said out loud in the file rather than left to be inferred from an
        // absence: a reader comparing two documents needs to know the meshes
        // were left out on purpose.
        out.push_str(&format!(
            "# {} triangulation(s) omitted by request\n",
            geometry.triangulation_count()
        ));
    }

    for &(id, entry) in records.entries {
        let mut t = Tokens::default();
        w(&mut t, "entity");
        u(&mut t, id.get());
        provenance(&mut t, entry);
        emit(&mut out, &t);
    }

    for (id, node) in records.nodes {
        write_node(&mut out, id, node, options)?;
    }
    for (node, entity) in records.identities {
        let mut t = Tokens::default();
        w(&mut t, "identity");
        key(&mut t, node);
        u(&mut t, entity.get());
        emit(&mut out, &t);
    }

    let mut t = Tokens::default();
    w(&mut t, "operation");
    u(&mut t, u64::from(records.current_op.0));
    emit(&mut out, &t);

    for root in roots {
        let mut t = Tokens::default();
        w(&mut t, "root");
        shape(&mut t, root);
        emit(&mut out, &t);
    }
    Ok(out)
}

/// Read a model and its root shapes back.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) if the text is not
/// this format, names a version this does not know, or is malformed;
/// [`OgeomError::Dangling`](ogeom_core::OgeomError::Dangling) if it names a handle that
/// is not in the file.
pub fn read(text: &str) -> OgeomResult<(Model, Vec<Shape>)> {
    let (model, roots, leftover, _) = read_core(text)?;
    if let Some(keyword) = leftover {
        ogeom_bail!(Construction, "unknown record `{keyword}`");
    }
    Ok((model, roots))
}

/// Read a document's contents into an existing model.
///
/// Same grammar, same [`VERSION`]: this reads exactly what [`read`] reads, it
/// just lands the result in a model that already has things in it, which is
/// what lets a serialized tool body meet a live one in a boolean. The heavy
/// lifting is [`Model::absorb`]: every handle shifts past what the model
/// already holds, identities land under new ids, and the returned
/// [`Absorbed`] says where, so references recorded against the source
/// document still resolve.
///
/// A document authored at another unit scale is refused, not rescaled.
///
/// # Errors
///
/// As [`read`], plus the refusals of [`Model::absorb`].
pub fn read_into(model: &mut Model, text: &str) -> OgeomResult<Absorbed> {
    let (parts, roots, leftover, _, gaps) = read_parts(text)?;
    if let Some(keyword) = leftover {
        ogeom_bail!(Construction, "unknown record `{keyword}`");
    }
    let absorbed = model.absorb(parts, &roots)?;
    model.forget_provenance(
        gaps.iter()
            .filter_map(|id| absorbed.entities.get(id).copied()),
    );
    Ok(absorbed)
}

/// The model section, stopping at the first record it does not know.
///
/// Returns the leftover keyword and the cursor standing just past it, so a
/// layered reader (the document's) can pick up where the model's ends.
#[allow(clippy::type_complexity)]
fn read_core(text: &str) -> OgeomResult<(Model, Vec<Shape>, Option<String>, Cursor<'_>)> {
    let (parts, roots, leftover, cursor, gaps) = read_parts(text)?;
    let mut model = Model::from_parts(parts)?;
    model.forget_provenance(gaps);
    // The roots were rebuilt alongside the rest but travel outside `ModelParts`,
    // so they are still unbound: they name no arena and resolve nowhere. Binding
    // them checks them too, which is why a file naming a root that is not there
    // fails here rather than in whatever first tries to use it.
    let roots = roots
        .iter()
        .map(|root| model.bind(root))
        .collect::<OgeomResult<Vec<_>>>()?;
    Ok((model, roots, leftover, cursor))
}

/// The record loop: text to [`ModelParts`] and unbound roots.
///
/// Everything [`read_core`] does short of assembling a model, split out so
/// [`read_into`] can hand the same parts to [`Model::absorb`] instead.
///
/// The last element is the entity ids the file skipped. Their places in
/// the parts' table hold bare primitives so the numbering holds, and the
/// caller forgets them once the model is assembled.
#[allow(clippy::type_complexity)]
fn read_parts(
    text: &str,
) -> OgeomResult<(
    ModelParts,
    Vec<Shape>,
    Option<String>,
    Cursor<'_>,
    Vec<EntityId>,
)> {
    let mut cursor = Cursor::new(text);
    let mut leftover = None;

    if cursor.word()? != MAGIC {
        ogeom_bail!(Construction, "not an ogeom document");
    }
    let version = cursor.count()?;
    if version == 0 || version > VERSION as usize {
        ogeom_bail!(
            Construction,
            "document is version {version}; this reads versions 1 through \
             {VERSION}"
        );
    }

    // The scale is read before anything it measures, so every geometric check
    // below runs against the document's own units rather than an assumption.
    let tol = read_units(&mut cursor)?;
    let mut parts = ModelParts {
        tolerances: tol,
        ..ModelParts::default()
    };
    let mut geometry = GeometryStore::new();
    let mut roots = Vec::new();
    let mut gaps: Vec<EntityId> = Vec::new();

    while !cursor.done() {
        let tag = cursor.word()?.to_string();
        match tag.as_str() {
            "datum" => {
                let key = cursor.key()?;
                expect_datum(key, parts.datums.len())?;
                parts.datums.push(cursor.transform(tol)?);
            }
            "curve" => {
                let (index, _) = cursor.key()?;
                expect_index(index, geometry.counts().0)?;
                geometry.add_curve(cursor.curve(tol)?);
            }
            "pcurve" => {
                let (index, _) = cursor.key()?;
                expect_index(index, geometry.counts().1)?;
                geometry.add_pcurve(cursor.pcurve(tol)?);
            }
            "surface" => {
                let (index, _) = cursor.key()?;
                expect_index(index, geometry.counts().2)?;
                geometry.add_surface(cursor.surface(tol)?);
            }
            "mesh" => {
                let (index, _) = cursor.key()?;
                expect_index(index, geometry.triangulation_count())?;
                geometry.add_triangulation(cursor.mesh()?);
            }
            "entity" => {
                let id = cursor.count()?;
                let next = parts.provenance.len() + 1;
                if id < next || (version < 3 && id != next) {
                    ogeom_bail!(
                        Construction,
                        "entities must be written in the order their identities \
                         were issued; expected {next}, got {id}"
                    );
                }
                for skipped in next..id {
                    let Some(gap) = EntityId::from_raw(skipped as u64) else {
                        ogeom_bail!(Construction, "identity 0 was never issued");
                    };
                    gaps.push(gap);
                    parts.provenance.push(Provenance::Primitive {
                        op: OpId(0),
                        role: Role::SOLE,
                    });
                }
                let entry = cursor.provenance()?;
                if let Some(from) = entry
                    .inputs()
                    .iter()
                    .find(|from| gaps.binary_search(from).is_ok())
                {
                    ogeom_bail!(
                        Construction,
                        "entity {id} is derived from entity {}, which the file left out",
                        from.get()
                    );
                }
                parts.provenance.push(entry);
            }
            "node" => {
                let (index, _) = cursor.key()?;
                expect_index(index, parts.nodes.len())?;
                parts.nodes.push(cursor.node()?);
            }
            "identity" => {
                let node = cursor.shape_key()?;
                let raw = cursor.count()?;
                let Some(entity) = EntityId::from_raw(raw as u64) else {
                    ogeom_bail!(Construction, "identity 0 was never issued");
                };
                parts.identity.push((node, entity));
            }
            "operation" => parts.current_op = OpId(cursor.small()?),
            "root" => roots.push(cursor.shape()?),
            other => {
                leftover = Some(other.to_string());
                break;
            }
        }
    }

    if let Some((_, entity)) = parts
        .identity
        .iter()
        .find(|(_, entity)| gaps.binary_search(entity).is_ok())
    {
        ogeom_bail!(
            Construction,
            "a node carries entity {}, which the file left out",
            entity.get()
        );
    }

    parts.geometry = geometry;
    Ok((parts, roots, leftover, cursor, gaps))
}

/// Read the `units` record, which every document since version 1 carries.
fn read_units(cursor: &mut Cursor<'_>) -> OgeomResult<Tolerances> {
    if cursor.word()? != "units" {
        ogeom_bail!(
            Construction,
            "a document must say what units it is in before anything measured \
             in them"
        );
    }
    Tolerances::with_scale(cursor.number()?)
}

/// Refuse a record written out of arena order.
///
/// The order *is* the numbering (a reader replays inserts to reproduce the
/// handles), so a gap or a repeat would silently shift every later reference.
fn expect_index(index: u32, next: usize) -> OgeomResult<()> {
    if index as usize != next {
        ogeom_bail!(
            Construction,
            "records must run in arena order; expected index {next}, got {index}"
        );
    }
    Ok(())
}

/// As [`expect_index`], for a datum, whose generation must be a fresh arena's.
fn expect_datum(key: (u32, u32), next: usize) -> OgeomResult<()> {
    expect_index(key.0, next)?;
    if key.1 != 0 {
        ogeom_bail!(
            Construction,
            "a datum with generation {} cannot be rebuilt: a fresh arena hands \
             out generation 0, so the handle would not match what the file says",
            key.1
        );
    }
    Ok(())
}

// --- writing -----------------------------------------------------------------

// --- the document layer -------------------------------------------------------

/// Write a document: the model, then the product structure, appearance,
/// names and PMI over it.
///
/// The model section is exactly what [`write()`] emits, so a model-only reader
/// stops cleanly at the document records; writing what [`read_document`]
/// read reproduces the bytes, the same contract the model layer keeps.
///
/// # Errors
///
/// As [`write()`].
pub fn write_document(
    document: &ogeom_doc::Document,
    options: WriteOptions,
) -> OgeomResult<String> {
    let (mut out, renumbered) = write_renumbered(document.model(), &[], options)?;

    for (_, product) in document.products() {
        let mut t = Tokens::default();
        w(&mut t, "product");
        text(&mut t, &product.name);
        match product.colour {
            Some(c) => {
                flag(&mut t, true);
                for channel in [c.r, c.g, c.b, c.a] {
                    n(&mut t, channel);
                }
            }
            None => flag(&mut t, false),
        }
        match &product.kind {
            ogeom_doc::ProductKind::Part { shape: part } => {
                w(&mut t, "part");
                shape(&mut t, &renumbered.shape(part)?);
            }
            ogeom_doc::ProductKind::Assembly { children } => {
                w(&mut t, "assembly");
                u(&mut t, children.len() as u64);
                for instance in children {
                    u(&mut t, u64::from(instance.product.index()));
                    location(&mut t, &instance.location);
                    match &instance.name {
                        Some(name) => text(&mut t, name),
                        None => w(&mut t, "-"),
                    }
                }
            }
        }
        emit(&mut out, &t);
    }

    let mut colours: Vec<_> = document.colours().collect();
    colours.sort_by_key(|(node, _)| (node.index(), node.generation()));
    for (node, colour) in colours {
        let mut t = Tokens::default();
        w(&mut t, "doc-colour");
        key(&mut t, renumbered.node(node)?);
        for channel in [colour.r, colour.g, colour.b, colour.a] {
            n(&mut t, channel);
        }
        emit(&mut out, &t);
    }
    let mut names: Vec<_> = document.names().collect();
    names.sort_by_key(|(node, _)| (node.index(), node.generation()));
    for (node, name) in names {
        let mut t = Tokens::default();
        w(&mut t, "doc-name");
        key(&mut t, renumbered.node(node)?);
        text(&mut t, name);
        emit(&mut out, &t);
    }

    let pmi = document.pmi();
    for dimension in &pmi.dimensions {
        let mut t = Tokens::default();
        w(&mut t, "pmi-dim");
        text(&mut t, &dimension.name);
        w(
            &mut t,
            match dimension.kind {
                ogeom_doc::MeasureKind::Length => "L",
                ogeom_doc::MeasureKind::Angle => "A",
            },
        );
        flag(&mut t, dimension.location);
        optional(&mut t, dimension.plus);
        optional(&mut t, dimension.minus);
        u(&mut t, dimension.values.len() as u64);
        for v in &dimension.values {
            n(&mut t, *v);
        }
        u(&mut t, dimension.features.len() as u64);
        for feature in &dimension.features {
            u(&mut t, feature.len() as u64);
            for node in feature {
                key(&mut t, renumbered.node(*node)?);
            }
        }
        emit(&mut out, &t);
    }
    for tolerance in &pmi.tolerances {
        let mut t = Tokens::default();
        w(&mut t, "pmi-tol");
        text(&mut t, &tolerance.kind);
        text(&mut t, &tolerance.name);
        n(&mut t, tolerance.magnitude);
        u(&mut t, tolerance.modifiers.len() as u64);
        for word in &tolerance.modifiers {
            text(&mut t, word);
        }
        u(&mut t, tolerance.datums.len() as u64);
        for label in &tolerance.datums {
            text(&mut t, label);
        }
        u(&mut t, tolerance.items.len() as u64);
        for node in &tolerance.items {
            key(&mut t, renumbered.node(*node)?);
        }
        emit(&mut out, &t);
    }
    for datum in &pmi.datums {
        let mut t = Tokens::default();
        w(&mut t, "pmi-datum");
        text(&mut t, &datum.label);
        u(&mut t, datum.items.len() as u64);
        for node in &datum.items {
            key(&mut t, renumbered.node(*node)?);
        }
        emit(&mut out, &t);
    }

    for callout in &document.pmi().callouts {
        let mut t = Tokens::default();
        w(&mut t, "pmi-callout");
        text(&mut t, &callout.name);
        match &callout.plane {
            Some(f) => {
                flag(&mut t, true);
                frame(&mut t, f);
            }
            None => flag(&mut t, false),
        }
        u(&mut t, callout.polylines.len() as u64);
        for line in &callout.polylines {
            u(&mut t, line.len() as u64);
            for p in line {
                point(&mut t, *p);
            }
        }
        match callout.annotates {
            Some(ogeom_doc::Annotated::Dimension(i)) => {
                w(&mut t, "D");
                u(&mut t, i as u64);
            }
            Some(ogeom_doc::Annotated::Tolerance(i)) => {
                w(&mut t, "T");
                u(&mut t, i as u64);
            }
            Some(ogeom_doc::Annotated::Datum(i)) => {
                w(&mut t, "M");
                u(&mut t, i as u64);
            }
            None => w(&mut t, "-"),
        }
        emit(&mut out, &t);
    }
    for view in document.views() {
        let mut t = Tokens::default();
        w(&mut t, "doc-view");
        text(&mut t, &view.name);
        frame(&mut t, &view.frame);
        match &view.clipping {
            Some(plane) => {
                flag(&mut t, true);
                frame(&mut t, &plane.frame());
            }
            None => flag(&mut t, false),
        }
        u(&mut t, view.callouts.len() as u64);
        for index in &view.callouts {
            u(&mut t, *index as u64);
        }
        emit(&mut out, &t);
    }
    for note in document.notes() {
        let mut t = Tokens::default();
        w(&mut t, "doc-note");
        text(&mut t, &note.author);
        text(&mut t, &note.text);
        match note.product {
            Some(id) => {
                flag(&mut t, true);
                u(&mut t, document.product_index(id) as u64);
            }
            None => flag(&mut t, false),
        }
        emit(&mut out, &t);
    }

    // The attribute layer: properties, materials, layers, validation:
    // shape-keyed maps in key order, lists in id order, so writing what
    // was read gives the same bytes.
    let mut with_properties: Vec<_> = document.properties().collect();
    with_properties.sort_by_key(|(node, _)| (node.index(), node.generation()));
    for (node, properties) in with_properties {
        for property in properties {
            let mut t = Tokens::default();
            w(&mut t, "doc-prop");
            key(&mut t, renumbered.node(node)?);
            text(&mut t, &property.name);
            match &property.value {
                ogeom_doc::PropertyValue::Text(value) => {
                    w(&mut t, "T");
                    text(&mut t, value);
                }
                ogeom_doc::PropertyValue::Number(value) => {
                    w(&mut t, "N");
                    n(&mut t, *value);
                }
                ogeom_doc::PropertyValue::Flag(value) => {
                    w(&mut t, "F");
                    flag(&mut t, *value);
                }
            }
            emit(&mut out, &t);
        }
    }
    for material in document.materials() {
        let mut t = Tokens::default();
        w(&mut t, "doc-material");
        text(&mut t, &material.name);
        optional(&mut t, material.density);
        match material.colour {
            Some(c) => {
                flag(&mut t, true);
                for channel in [c.r, c.g, c.b, c.a] {
                    n(&mut t, channel);
                }
            }
            None => flag(&mut t, false),
        }
        emit(&mut out, &t);
    }
    let mut assigned: Vec<_> = document.material_assignments().collect();
    assigned.sort_by_key(|(node, _)| (node.index(), node.generation()));
    for (node, material) in assigned {
        let mut t = Tokens::default();
        w(&mut t, "doc-material-of");
        key(&mut t, renumbered.node(node)?);
        u(&mut t, material.index() as u64);
        emit(&mut out, &t);
    }
    for layer in document.layers() {
        let mut t = Tokens::default();
        w(&mut t, "doc-layer");
        text(&mut t, &layer.name);
        flag(&mut t, layer.visible);
        emit(&mut out, &t);
    }
    let mut memberships: Vec<_> = document.layer_memberships().collect();
    memberships.sort_by_key(|(node, _)| (node.index(), node.generation()));
    for (node, layers) in memberships {
        let mut t = Tokens::default();
        w(&mut t, "doc-on-layer");
        key(&mut t, renumbered.node(node)?);
        u(&mut t, layers.len() as u64);
        for layer in layers {
            u(&mut t, layer.index() as u64);
        }
        emit(&mut out, &t);
    }
    let mut checks: Vec<_> = document.validations().collect();
    checks.sort_by_key(|(node, _)| (node.index(), node.generation()));
    for (node, values) in checks {
        let mut t = Tokens::default();
        w(&mut t, "doc-check");
        key(&mut t, renumbered.node(node)?);
        n(&mut t, values.volume);
        n(&mut t, values.area);
        for v in [values.centroid.x, values.centroid.y, values.centroid.z] {
            n(&mut t, v);
        }
        emit(&mut out, &t);
    }
    Ok(out)
}

/// Read a document back: the model plus everything over it.
///
/// # Errors
///
/// As [`read`], plus a malformed document record.
pub fn read_document(text: &str) -> OgeomResult<ogeom_doc::Document> {
    let (model, _roots, mut pending, mut cursor) = read_core(text)?;
    let mut document = ogeom_doc::Document::over(model);
    // Product ids are issued in write order, so index references bind by
    // re-adding in order; instances arrive after every product exists in the
    // file's own ordering discipline, which parts first would break, so
    // instances are held back and applied at the end.
    let mut ids: Vec<ogeom_doc::ProductId> = Vec::new();
    let mut instances: Vec<(usize, usize, Location, Option<String>)> = Vec::new();
    let mut order = 0_usize;

    while let Some(tag) = pending.take().or_else(|| {
        if cursor.done() {
            None
        } else {
            cursor.word().ok().map(ToString::to_string)
        }
    }) {
        match tag.as_str() {
            "product" => {
                let name = read_text(&mut cursor)?;
                let colour = if cursor.flag()? {
                    Some(ogeom_doc::Colour {
                        r: cursor.number()?,
                        g: cursor.number()?,
                        b: cursor.number()?,
                        a: cursor.number()?,
                    })
                } else {
                    None
                };
                match cursor.word()? {
                    "part" => {
                        let part = document.model().bind(&cursor.shape()?)?;
                        let id = document.add_part(&name, part);
                        ids.push(id);
                    }
                    "assembly" => {
                        let id = document.add_assembly(&name);
                        ids.push(id);
                        let count = cursor.count()?;
                        for _ in 0..count {
                            let child = cursor.count()?;
                            let at = cursor.location()?;
                            let instance_name = match cursor.peek() {
                                Some("-") => {
                                    cursor.word()?;
                                    None
                                }
                                _ => Some(read_text(&mut cursor)?),
                            };
                            instances.push((order, child, at, instance_name));
                        }
                    }
                    other => {
                        ogeom_bail!(
                            Construction,
                            "a product is `part` or `assembly`, not `{other}`"
                        )
                    }
                }
                if let Some(c) = colour {
                    let id = ids[ids.len() - 1];
                    document.set_product_colour(id, c)?;
                }
                order += 1;
            }
            "doc-colour" => {
                let node: TShapeId = cursor.handle()?;
                let node = bind_node(&document, node)?;
                let colour = ogeom_doc::Colour {
                    r: cursor.number()?,
                    g: cursor.number()?,
                    b: cursor.number()?,
                    a: cursor.number()?,
                };
                document.set_colour(&Shape::of(node), colour);
            }
            "doc-name" => {
                let node: TShapeId = cursor.handle()?;
                let node = bind_node(&document, node)?;
                let name = read_text(&mut cursor)?;
                document.set_name(&Shape::of(node), name);
            }
            "doc-prop" => {
                let node: TShapeId = cursor.handle()?;
                let node = bind_node(&document, node)?;
                let name = read_text(&mut cursor)?;
                let value = match cursor.word()? {
                    "T" => ogeom_doc::PropertyValue::Text(read_text(&mut cursor)?),
                    "F" => ogeom_doc::PropertyValue::Flag(cursor.flag()?),
                    _ => ogeom_doc::PropertyValue::Number(cursor.number()?),
                };
                document.set_property(&Shape::of(node), ogeom_doc::Property { name, value });
            }
            "doc-material" => {
                let name = read_text(&mut cursor)?;
                let density = read_optional(&mut cursor)?;
                let colour = if cursor.flag()? {
                    Some(ogeom_doc::Colour {
                        r: cursor.number()?,
                        g: cursor.number()?,
                        b: cursor.number()?,
                        a: cursor.number()?,
                    })
                } else {
                    None
                };
                document.add_material(ogeom_doc::Material {
                    name,
                    density,
                    colour,
                });
            }
            "doc-material-of" => {
                let node: TShapeId = cursor.handle()?;
                let node = bind_node(&document, node)?;
                let index = cursor.count()?;
                let Some(id) = document.material_id(index) else {
                    ogeom_bail!(Construction, "material {index} is not in this document");
                };
                document.assign_material(&Shape::of(node), id);
            }
            "doc-layer" => {
                let name = read_text(&mut cursor)?;
                let visible = cursor.flag()?;
                let id = document.add_layer(name);
                document.set_layer_visible(id, visible);
            }
            "doc-on-layer" => {
                let node: TShapeId = cursor.handle()?;
                let node = bind_node(&document, node)?;
                for _ in 0..cursor.count()? {
                    let index = cursor.count()?;
                    let Some(id) = document.layer_id(index) else {
                        ogeom_bail!(Construction, "layer {index} is not in this document");
                    };
                    document.place_on_layer(&Shape::of(node), id);
                }
            }
            "doc-check" => {
                let node: TShapeId = cursor.handle()?;
                let node = bind_node(&document, node)?;
                let volume = cursor.number()?;
                let area = cursor.number()?;
                let centroid =
                    ogeom_math::Point::new(cursor.number()?, cursor.number()?, cursor.number()?);
                document.set_validation(
                    &Shape::of(node),
                    ogeom_doc::ValidationProperties {
                        volume,
                        area,
                        centroid,
                    },
                );
            }
            "pmi-dim" => {
                let name = read_text(&mut cursor)?;
                let kind = match cursor.word()? {
                    "A" => ogeom_doc::MeasureKind::Angle,
                    _ => ogeom_doc::MeasureKind::Length,
                };
                let location = cursor.flag()?;
                let plus = read_optional(&mut cursor)?;
                let minus = read_optional(&mut cursor)?;
                let mut values = Vec::new();
                for _ in 0..cursor.count()? {
                    values.push(cursor.number()?);
                }
                let mut features = Vec::new();
                for _ in 0..cursor.count()? {
                    let mut feature = Vec::new();
                    for _ in 0..cursor.count()? {
                        let node: TShapeId = cursor.handle()?;
                        feature.push(bind_node(&document, node)?);
                    }
                    features.push(feature);
                }
                document.pmi_mut().dimensions.push(ogeom_doc::Dimension {
                    name,
                    values,
                    kind,
                    plus,
                    minus,
                    features,
                    location,
                });
            }
            "pmi-tol" => {
                let kind = read_text(&mut cursor)?;
                let name = read_text(&mut cursor)?;
                let magnitude = cursor.number()?;
                let mut modifiers = Vec::new();
                for _ in 0..cursor.count()? {
                    modifiers.push(read_text(&mut cursor)?);
                }
                let mut datums = Vec::new();
                for _ in 0..cursor.count()? {
                    datums.push(read_text(&mut cursor)?);
                }
                let mut items = Vec::new();
                for _ in 0..cursor.count()? {
                    let node: TShapeId = cursor.handle()?;
                    items.push(bind_node(&document, node)?);
                }
                document
                    .pmi_mut()
                    .tolerances
                    .push(ogeom_doc::GeometricTolerance {
                        kind,
                        name,
                        magnitude,
                        modifiers,
                        datums,
                        items,
                    });
            }
            "pmi-datum" => {
                let label = read_text(&mut cursor)?;
                let mut items = Vec::new();
                for _ in 0..cursor.count()? {
                    let node: TShapeId = cursor.handle()?;
                    items.push(bind_node(&document, node)?);
                }
                document
                    .pmi_mut()
                    .datums
                    .push(ogeom_doc::Datum { label, items });
            }
            "pmi-callout" => {
                let name = read_text(&mut cursor)?;
                let plane = if cursor.flag()? {
                    Some(cursor.frame(Tolerances::millimetres())?)
                } else {
                    None
                };
                let lines = cursor.count()?;
                let mut polylines = Vec::with_capacity(cursor.room(lines));
                for _ in 0..lines {
                    let count = cursor.count()?;
                    let mut line = Vec::with_capacity(cursor.room(count));
                    for _ in 0..count {
                        line.push(cursor.point()?);
                    }
                    polylines.push(line);
                }
                let annotates = match cursor.word()? {
                    "D" => Some(ogeom_doc::Annotated::Dimension(cursor.count()?)),
                    "T" => Some(ogeom_doc::Annotated::Tolerance(cursor.count()?)),
                    "M" => Some(ogeom_doc::Annotated::Datum(cursor.count()?)),
                    _ => None,
                };
                document.pmi_mut().callouts.push(ogeom_doc::Callout {
                    name,
                    plane,
                    polylines,
                    annotates,
                });
            }
            "doc-view" => {
                let name = read_text(&mut cursor)?;
                let frame = cursor.frame(Tolerances::millimetres())?;
                let clipping = if cursor.flag()? {
                    Some(ogeom_math::Plane::new(
                        cursor.frame(Tolerances::millimetres())?,
                    ))
                } else {
                    None
                };
                let count = cursor.count()?;
                let mut callouts = Vec::with_capacity(cursor.room(count));
                for _ in 0..count {
                    callouts.push(cursor.count()?);
                }
                document.add_view(ogeom_doc::View {
                    name,
                    frame,
                    clipping,
                    callouts,
                });
            }
            "doc-note" => {
                let author = read_text(&mut cursor)?;
                let text_body = read_text(&mut cursor)?;
                let product = if cursor.flag()? {
                    let index = cursor.count()?;
                    ids.get(index).copied()
                } else {
                    None
                };
                document.add_note(ogeom_doc::Note {
                    author,
                    text: text_body,
                    product,
                });
            }
            other => ogeom_bail!(Construction, "unknown record `{other}`"),
        }
    }

    for (parent, child, at, name) in instances {
        let (Some(&parent), Some(&child)) = (ids.get(parent), ids.get(child)) else {
            ogeom_bail!(Dangling, "an instance names a product not in the file");
        };
        let at = document.model().bind_location(&at)?;
        document.add_instance_at(parent, child, at, name)?;
    }
    Ok(document)
}

/// A raw node handle bound into the document's model scope.
fn bind_node(document: &ogeom_doc::Document, node: TShapeId) -> OgeomResult<TShapeId> {
    Ok(document.model().bind(&Shape::of(node))?.node())
}

/// Append a text token: `'` then the percent-encoded body, so names survive
/// whitespace tokenization and an empty name survives at all.
fn text(t: &mut Tokens, s: &str) {
    let mut token = String::from("'");
    for byte in s.bytes() {
        if byte.is_ascii_graphic() && byte != b'%' {
            token.push(byte as char);
        } else {
            token.push_str(&format!("%{byte:02X}"));
        }
    }
    t.push(token);
}

/// Read a text token back.
fn read_text(cursor: &mut Cursor<'_>) -> OgeomResult<String> {
    let token = cursor.word()?;
    let Some(body) = token.strip_prefix('\'') else {
        ogeom_bail!(Construction, "expected a text token, got `{token}`");
    };
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() + 1 && i + 2 < bytes.len() + 1 {
            let hex = body.get(i + 1..i + 3).unwrap_or("");
            let Ok(byte) = u8::from_str_radix(hex, 16) else {
                ogeom_bail!(
                    Construction,
                    "a text token escapes `%{hex}`, which is not hex"
                );
            };
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out)
        .map_err(|_| ogeom_core::ogeom_err!(Construction, "a text token is not UTF-8"))
}

/// An optional number: a presence flag then the value.
fn optional(t: &mut Tokens, v: Option<f64>) {
    match v {
        Some(value) => {
            flag(t, true);
            n(t, value);
        }
        None => flag(t, false),
    }
}

/// Read an optional number back.
fn read_optional(cursor: &mut Cursor<'_>) -> OgeomResult<Option<f64>> {
    Ok(if cursor.flag()? {
        Some(cursor.number()?)
    } else {
        None
    })
}

/// Append a word.
fn w(t: &mut Tokens, s: &str) {
    t.next().push_str(s);
}

/// Append a number, in the shortest form that parses back to it exactly.
fn n(t: &mut Tokens, v: f64) {
    let _ = write!(t.next(), "{v:?}");
}

/// Append an unsigned integer.
fn u(t: &mut Tokens, v: u64) {
    let _ = write!(t.next(), "{v}");
}

/// Append an arena handle, as `index:generation`.
fn key<T>(t: &mut Tokens, id: Key<T>) {
    let _ = write!(t.next(), "{}:{}", id.index(), id.generation());
}

/// Append a flag.
fn flag(t: &mut Tokens, v: bool) {
    t.next().push(if v { '1' } else { '0' });
}

/// One record's tokens, joined by single spaces as they are appended.
///
/// Numbers are formatted straight into the line; writing into a `String`
/// cannot fail, which is why the helpers discard `write!`'s result.
#[derive(Default)]
struct Tokens(String);

impl Tokens {
    /// The line, ready for the next token: a space after any token before.
    fn next(&mut self) -> &mut String {
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        &mut self.0
    }

    fn push(&mut self, token: String) {
        self.next().push_str(&token);
    }
}

/// Finish a record.
fn emit(out: &mut String, t: &Tokens) {
    out.push_str(&t.0);
    out.push('\n');
}

fn point(t: &mut Tokens, p: Point) {
    n(t, p.x);
    n(t, p.y);
    n(t, p.z);
}

fn point2(t: &mut Tokens, p: Point2) {
    n(t, p.x);
    n(t, p.y);
}

fn vector(t: &mut Tokens, v: Vector) {
    n(t, v.x);
    n(t, v.y);
    n(t, v.z);
}

fn direction(t: &mut Tokens, d: Direction) {
    vector(t, d.vector());
}

fn direction2(t: &mut Tokens, d: Direction2) {
    n(t, d.vector().x);
    n(t, d.vector().y);
}

/// A frame, as origin then all three axes.
///
/// `y` is written even though a right-handed frame derives it, because a
/// mirrored frame does not: writing only `z` and `x` would quietly turn every
/// left-handed frame right-handed on the way back.
fn frame(t: &mut Tokens, f: &Frame) {
    point(t, f.origin());
    direction(t, f.x());
    direction(t, f.y());
    direction(t, f.z());
}

fn frame2(t: &mut Tokens, f: &Frame2) {
    point2(t, f.origin());
    direction2(t, f.x());
    direction2(t, f.y());
}

fn axis(t: &mut Tokens, a: Axis) {
    point(t, a.location);
    direction(t, a.direction);
}

fn axis2(t: &mut Tokens, a: Axis2) {
    point2(t, a.location);
    direction2(t, a.direction);
}

fn range(t: &mut Tokens, r: (f64, f64)) {
    n(t, r.0);
    n(t, r.1);
}

/// A similarity transform: its unit linear part, its scale, its translation.
fn transform(t: &mut Tokens, x: &Transform) -> OgeomResult<()> {
    let m = x.linear();
    for row in 0..3 {
        for column in 0..3 {
            n(t, m.get(row, column)?);
        }
    }
    n(t, x.scale_factor());
    vector(t, x.translation_vector());
    Ok(())
}

/// A placement chain, or `-` for the identity.
fn location(t: &mut Tokens, l: &Location) {
    if l.is_identity() {
        w(t, "-");
        return;
    }
    let chain: Vec<String> = l
        .chain()
        .iter()
        .map(|(datum, power)| format!("{}:{}^{power}", datum.index(), datum.generation()))
        .collect();
    w(t, &chain.join(","));
}

/// A shape triple, as one token: node, orientation, placement.
fn shape(t: &mut Tokens, s: &Shape) {
    let orientation = match s.orientation() {
        Orientation::Forward => "F",
        Orientation::Reversed => "R",
        Orientation::Internal => "I",
        Orientation::External => "E",
    };
    let mut token = format!(
        "{}:{}/{orientation}",
        s.node().index(),
        s.node().generation()
    );
    if !s.location().is_identity() {
        let chain: Vec<String> = s
            .location()
            .chain()
            .iter()
            .map(|(datum, power)| format!("{}:{}^{power}", datum.index(), datum.generation()))
            .collect();
        token.push('/');
        token.push_str(&chain.join(","));
    }
    w(t, &token);
}

fn knots(t: &mut Tokens, k: &KnotVector) {
    u(t, k.degree() as u64);
    u(t, k.knots().len() as u64);
    for value in k.knots() {
        n(t, *value);
    }
}

fn weighted(t: &mut Tokens, c: Weighted<Point>) {
    // Written in the homogeneous form it is stored in, so no multiply and
    // divide stands between what was held and what comes back.
    point(t, c.scaled);
    n(t, c.weight);
}

fn weighted2(t: &mut Tokens, c: Weighted<Point2>) {
    point2(t, c.scaled);
    n(t, c.weight);
}

fn curve(t: &mut Tokens, c: &Curve) -> OgeomResult<()> {
    match c {
        Curve::Line(l) => {
            w(t, "line");
            axis(t, l.axis());
            range(t, l.domain());
        }
        Curve::Circle(c) => {
            w(t, "circle");
            frame(t, &c.circle().frame());
            n(t, c.circle().radius());
            flag(t, c.is_reversed());
        }
        Curve::Ellipse(e) => {
            w(t, "ellipse");
            frame(t, &e.ellipse().frame());
            n(t, e.ellipse().major_radius());
            n(t, e.ellipse().minor_radius());
            flag(t, e.is_reversed());
        }
        Curve::Hyperbola(h) => {
            w(t, "hyperbola");
            frame(t, &h.hyperbola().frame());
            n(t, h.hyperbola().major_radius());
            n(t, h.hyperbola().minor_radius());
            range(t, h.domain());
            flag(t, h.is_reversed());
        }
        Curve::Parabola(p) => {
            w(t, "parabola");
            frame(t, &p.parabola().frame());
            n(t, p.parabola().focal());
            range(t, p.domain());
            flag(t, p.is_reversed());
        }
        Curve::Helix(h) => {
            if h.taper() == 0.0 {
                w(t, "helix");
                frame(t, h.frame());
                n(t, h.radius());
                n(t, h.pitch());
                range(t, Curve3d::domain(h));
                flag(t, h.is_reversed());
            } else {
                w(t, "conical");
                frame(t, h.frame());
                n(t, h.radius());
                n(t, h.pitch());
                n(t, h.taper());
                range(t, Curve3d::domain(h));
                flag(t, h.is_reversed());
            }
        }
        Curve::BSpline(b) => {
            w(
                t,
                if b.is_periodic() {
                    "bspline_periodic"
                } else {
                    "bspline"
                },
            );
            knots(t, b.knots());
            u(t, b.control_points().len() as u64);
            for c in b.control_points() {
                weighted(t, *c);
            }
        }
        Curve::Trimmed(x) => {
            w(t, "trimmed");
            range(t, x.domain());
            flag(t, x.is_reversed());
            curve(t, x.basis())?;
        }
        Curve::Offset(o) => {
            w(t, "offset");
            n(t, o.distance());
            direction(t, o.reference());
            curve(t, o.basis())?;
        }
        Curve::OnSurface(c) => {
            w(t, "onsurface");
            pcurve(t, c.pcurve())?;
            surface(t, c.surface())?;
        }
    }
    Ok(())
}

fn pcurve(t: &mut Tokens, c: &PlanarCurve) -> OgeomResult<()> {
    match c {
        PlanarCurve::Line(l) => {
            w(t, "line2");
            axis2(t, l.axis());
            range(t, l.domain());
        }
        PlanarCurve::Circle(c) => {
            w(t, "circle2");
            frame2(t, &c.circle().frame());
            n(t, c.circle().radius());
            flag(t, c.is_reversed());
        }
        PlanarCurve::Ellipse(e) => {
            w(t, "ellipse2");
            frame2(t, &e.ellipse().frame());
            n(t, e.ellipse().major_radius());
            n(t, e.ellipse().minor_radius());
            flag(t, e.is_reversed());
        }
        PlanarCurve::BSpline(b) => {
            w(t, "bspline2");
            knots(t, b.knots());
            u(t, b.control_points().len() as u64);
            for c in b.control_points() {
                weighted2(t, *c);
            }
        }
        PlanarCurve::Trimmed(x) => {
            w(t, "trimmed2");
            range(t, x.domain());
            flag(t, x.is_reversed());
            pcurve(t, x.basis())?;
        }
        PlanarCurve::Offset(o) => {
            w(t, "offset2");
            n(t, o.distance());
            pcurve(t, o.basis())?;
        }
        PlanarCurve::Trig(x) => {
            w(t, "trig2");
            point2(t, x.constant());
            n(t, x.linear().x);
            n(t, x.linear().y);
            n(t, x.cosine().x);
            n(t, x.cosine().y);
            n(t, x.sine().x);
            n(t, x.sine().y);
            range(t, Curve2d::domain(x));
            flag(t, x.is_reversed());
        }
    }
    Ok(())
}

fn surface(t: &mut Tokens, s: &SurfaceGeometry) -> OgeomResult<()> {
    let (u_domain, v_domain) = s.domain();
    match s {
        SurfaceGeometry::Plane(p) => {
            w(t, "plane");
            frame(t, &p.plane().frame());
            range(t, u_domain);
            range(t, v_domain);
        }
        SurfaceGeometry::Cylinder(c) => {
            w(t, "cylinder");
            frame(t, &c.cylinder().frame());
            n(t, c.cylinder().radius());
            range(t, v_domain);
        }
        SurfaceGeometry::Cone(c) => {
            w(t, "cone");
            frame(t, &c.cone().frame());
            n(t, c.cone().reference_radius());
            n(t, c.cone().half_angle());
            range(t, v_domain);
        }
        SurfaceGeometry::Sphere(s) => {
            w(t, "sphere");
            frame(t, &s.sphere().frame());
            n(t, s.sphere().radius());
        }
        SurfaceGeometry::Torus(x) => {
            w(t, "torus");
            frame(t, &x.torus().frame());
            n(t, x.torus().major_radius());
            n(t, x.torus().minor_radius());
        }
        SurfaceGeometry::BSpline(b) => {
            w(t, "bsurface");
            knots(t, b.u_knots());
            knots(t, b.v_knots());
            u(t, b.grid().u_count() as u64);
            u(t, b.grid().v_count() as u64);
            for c in b.grid().points() {
                weighted(t, *c);
            }
        }
        SurfaceGeometry::Revolution(r) => {
            w(t, "revolution");
            axis(t, r.axis());
            range(t, u_domain);
            curve(t, r.curve())?;
        }
        SurfaceGeometry::Extrusion(e) => {
            w(t, "extrusion");
            direction(t, e.direction());
            range(t, v_domain);
            curve(t, e.curve())?;
        }
        SurfaceGeometry::Trimmed(x) => {
            w(t, "tsurface");
            range(t, u_domain);
            range(t, v_domain);
            surface(t, x.basis())?;
        }
        SurfaceGeometry::Offset(o) => {
            w(t, "osurface");
            n(t, o.distance());
            surface(t, o.basis())?;
        }
    }
    Ok(())
}

fn provenance(t: &mut Tokens, p: &Provenance) {
    match p {
        Provenance::Primitive { op, role } => {
            w(t, "primitive");
            u(t, u64::from(op.0));
            u(t, u64::from(role.0));
        }
        Provenance::Derived { op, from, role } => {
            w(t, "derived");
            u(t, u64::from(op.0));
            u(t, u64::from(role.0));
            u(t, from.len() as u64);
            for source in from {
                u(t, source.get());
            }
        }
        Provenance::Imported { source, external } => {
            w(t, "imported");
            u(t, u64::from(source.0));
            u(t, *external);
        }
    }
}

/// A cached mesh: a header line, then a line per vertex and a line per triangle.
///
/// Broken across lines rather than run together, because a mesh is the largest
/// thing in the file by far and a one-line-per-vertex diff is the difference
/// between reading a disagreement and scrolling past it.
fn write_mesh(out: &mut String, id: ogeom_topo::TriangulationId, mesh: &Triangulation) {
    let mut t = Tokens::default();
    w(&mut t, "mesh");
    key(&mut t, id);
    u(&mut t, mesh.positions.len() as u64);
    u(&mut t, mesh.triangles.len() as u64);
    flag(&mut t, mesh.deflection_met);
    emit(out, &t);

    for i in 0..mesh.positions.len() {
        let mut t = Tokens::default();
        w(&mut t, "v");
        point(&mut t, mesh.positions[i]);
        // A mesh may carry fewer normals or parameters than positions if it was
        // assembled by hand; the zeros keep the record rectangular and the
        // reader keeps whatever length it is told.
        vector(&mut t, mesh.normals.get(i).copied().unwrap_or(Vector::ZERO));
        let (u_at, v_at) = mesh.parameters.get(i).copied().unwrap_or((0.0, 0.0));
        n(&mut t, u_at);
        n(&mut t, v_at);
        emit(out, &t);
    }
    for triangle in &mesh.triangles {
        let mut t = Tokens::default();
        w(&mut t, "f");
        for index in triangle {
            u(&mut t, u64::from(*index));
        }
        emit(out, &t);
    }
}

fn write_node(
    out: &mut String,
    id: TShapeId,
    node: &TShape,
    options: WriteOptions,
) -> OgeomResult<()> {
    let mut t = Tokens::default();
    w(&mut t, "node");
    key(&mut t, id);
    w(
        &mut t,
        match node.kind() {
            ShapeType::Vertex => "vertex",
            ShapeType::Edge => "edge",
            ShapeType::Wire => "wire",
            ShapeType::Face => "face",
            ShapeType::Shell => "shell",
            ShapeType::Solid => "solid",
            ShapeType::CompSolid => "compsolid",
            ShapeType::Compound => "compound",
        },
    );

    let mut representations = Vec::new();
    match node.data() {
        NodeData::Vertex(v) => {
            n(&mut t, v.tolerance.get());
            point(&mut t, v.point);
        }
        NodeData::Edge(e) => {
            n(&mut t, e.tolerance.get());
            flag(&mut t, e.same_parameter());
            flag(&mut t, e.degenerate);
            // Index paths are derived data over the cached meshes; when the
            // meshes are omitted by request, the paths that index them go
            // with them rather than dangling.
            let kept: Vec<&EdgeRepr> = e
                .representations
                .iter()
                .filter(|repr| {
                    options.triangulations
                        || !matches!(repr, EdgeRepr::PolygonOnTriangulation { .. })
                })
                .collect();
            u(&mut t, kept.len() as u64);
            for repr in kept {
                let mut line = Tokens::default();
                w(&mut line, "r");
                write_repr(&mut line, repr)?;
                representations.push(line);
            }
        }
        NodeData::Face(f) => {
            n(&mut t, f.tolerance.get());
            key(&mut t, f.surface);
            location(&mut t, &f.location);
            flag(&mut t, f.natural_restriction);
            match f.triangulation.filter(|_| options.triangulations) {
                Some(mesh) => key(&mut t, mesh),
                None => w(&mut t, "-"),
            }
        }
        NodeData::Container => {}
    }

    u(&mut t, node.children().len() as u64);
    for child in node.children() {
        shape(&mut t, child);
    }
    emit(out, &t);
    for line in &representations {
        emit(out, line);
    }
    Ok(())
}

fn write_repr(t: &mut Tokens, repr: &EdgeRepr) -> OgeomResult<()> {
    match repr {
        EdgeRepr::Curve3d {
            curve,
            location: at,
            range: r,
        } => {
            w(t, "curve3d");
            key(t, *curve);
            location(t, at);
            range(t, *r);
        }
        EdgeRepr::PCurve {
            curve,
            surface,
            location: at,
            range: r,
        } => {
            w(t, "pcurve");
            key(t, *curve);
            key(t, *surface);
            location(t, at);
            range(t, *r);
        }
        EdgeRepr::Seam {
            forward,
            reversed,
            surface,
            location: at,
            range: r,
        } => {
            w(t, "seam");
            key(t, *forward);
            key(t, *reversed);
            key(t, *surface);
            location(t, at);
            range(t, *r);
        }
        EdgeRepr::Polyline {
            points,
            parameters,
            location: at,
            deflection,
        } => {
            w(t, "polyline");
            location(t, at);
            n(t, *deflection);
            u(t, points.len() as u64);
            for p in points {
                point(t, *p);
            }
            u(t, parameters.len() as u64);
            for at in parameters {
                n(t, *at);
            }
        }
        EdgeRepr::PolygonOnTriangulation {
            triangulation,
            indices,
            location: at,
        } => {
            w(t, "polygon-on");
            key(t, *triangulation);
            location(t, at);
            u(t, indices.len() as u64);
            for index in indices {
                u(t, u64::from(*index));
            }
        }
        // `EdgeRepr` is non-exhaustive, so a variant added later lands here.
        // Refused, not skipped: a file missing one of an edge's descriptions of
        // itself reads back as an edge that has lost a face's pcurve, and
        // nothing downstream would say where it went.
        other => ogeom_bail!(
            Construction,
            "this version writes no edge representation of that kind: {other:?}"
        ),
    }
    Ok(())
}

// --- reading -----------------------------------------------------------------

/// A position in the token stream.
///
/// The file is written a record to a line, but read as a flat stream of
/// whitespace-separated words: layout is for the person reading a diff, and
/// making the parser depend on it would only add a second thing to get wrong.
struct Cursor<'a> {
    items: Vec<&'a str>,
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(text: &'a str) -> Self {
        let items = text
            .lines()
            .map(|line| line.split('#').next().unwrap_or(""))
            .flat_map(str::split_whitespace)
            .collect();
        Self { items, at: 0 }
    }

    fn peek(&self) -> Option<&'a str> {
        self.items.get(self.at).copied()
    }

    fn done(&self) -> bool {
        self.at >= self.items.len()
    }

    fn word(&mut self) -> OgeomResult<&'a str> {
        let Some(item) = self.items.get(self.at) else {
            ogeom_bail!(Construction, "the document ends part-way through a record");
        };
        self.at += 1;
        Ok(item)
    }

    fn number(&mut self) -> OgeomResult<f64> {
        let word = self.word()?;
        word.parse::<f64>()
            .map_err(|_| ogeom_core::ogeom_err!(Construction, "`{word}` is not a number"))
    }

    /// Room to reserve for `n` items read from here on: never more than the
    /// words left, since every item takes at least one, so a count in the
    /// file cannot size an allocation past what the file holds.
    fn room(&self, n: usize) -> usize {
        n.min(self.items.len().saturating_sub(self.at))
    }

    fn count(&mut self) -> OgeomResult<usize> {
        let word = self.word()?;
        word.parse::<usize>()
            .map_err(|_| ogeom_core::ogeom_err!(Construction, "`{word}` is not a count"))
    }

    fn small(&mut self) -> OgeomResult<u32> {
        let word = self.word()?;
        word.parse::<u32>()
            .map_err(|_| ogeom_core::ogeom_err!(Construction, "`{word}` is not an identifier"))
    }

    fn flag(&mut self) -> OgeomResult<bool> {
        match self.word()? {
            "0" => Ok(false),
            "1" => Ok(true),
            other => ogeom_bail!(Construction, "`{other}` is not a flag"),
        }
    }

    /// An `index:generation` pair.
    fn key(&mut self) -> OgeomResult<(u32, u32)> {
        parse_key(self.word()?)
    }

    fn handle<T>(&mut self) -> OgeomResult<Key<T>> {
        let (index, generation) = self.key()?;
        Ok(Key::from_parts(index, generation))
    }

    fn shape_key(&mut self) -> OgeomResult<TShapeId> {
        self.handle()
    }

    fn point(&mut self) -> OgeomResult<Point> {
        Ok(Point::new(self.number()?, self.number()?, self.number()?))
    }

    fn point2(&mut self) -> OgeomResult<Point2> {
        Ok(Point2::new(self.number()?, self.number()?))
    }

    fn vector(&mut self) -> OgeomResult<Vector> {
        Ok(Vector::new(self.number()?, self.number()?, self.number()?))
    }

    /// A direction, checked rather than renormalized.
    ///
    /// Normalizing what is already a unit vector moves it by a bit or two, and
    /// that is enough to make writing what was read produce a different file.
    fn direction(&mut self, tol: Tolerances) -> OgeomResult<Direction> {
        Direction::unit(self.vector()?, tol)
    }

    fn direction2(&mut self, tol: Tolerances) -> OgeomResult<Direction2> {
        Direction2::unit(
            ogeom_math::Vector2::new(self.number()?, self.number()?),
            tol,
        )
    }

    fn frame(&mut self, tol: Tolerances) -> OgeomResult<Frame> {
        let origin = self.point()?;
        let x = self.direction(tol)?;
        let y = self.direction(tol)?;
        let z = self.direction(tol)?;
        Frame::from_axes(origin, x, y, z, tol)
    }

    fn frame2(&mut self, tol: Tolerances) -> OgeomResult<Frame2> {
        let origin = self.point2()?;
        let x = self.direction2(tol)?;
        let y = self.direction2(tol)?;
        Frame2::from_axes(origin, x, y, tol)
    }

    fn axis(&mut self, tol: Tolerances) -> OgeomResult<Axis> {
        Ok(Axis::new(self.point()?, self.direction(tol)?))
    }

    fn axis2(&mut self, tol: Tolerances) -> OgeomResult<Axis2> {
        Ok(Axis2::new(self.point2()?, self.direction2(tol)?))
    }

    fn range(&mut self) -> OgeomResult<(f64, f64)> {
        Ok((self.number()?, self.number()?))
    }

    fn transform(&mut self, tol: Tolerances) -> OgeomResult<Transform> {
        let mut rows = [[0.0_f64; 3]; 3];
        for row in &mut rows {
            for cell in row.iter_mut() {
                *cell = self.number()?;
            }
        }
        let scale = self.number()?;
        let translation = self.vector()?;
        // Put back together from the parts a transform is stored as, rather
        // than multiplied into one matrix and factored apart again: the round
        // trip through a scale that divides out is not exact, and a placement
        // that drifts a little on every save is worse than one that fails.
        // The kind is re-derived, since it is a function of the other three.
        Transform::from_parts(
            ogeom_math::Matrix3::new(rows),
            scale,
            translation,
            tol.angular().max(1e-9),
        )
    }

    fn location(&mut self) -> OgeomResult<Location> {
        let word = self.word()?;
        parse_location(word)
    }

    fn shape(&mut self) -> OgeomResult<Shape> {
        let word = self.word()?;
        let mut parts = word.split('/');
        let (Some(node), Some(orientation)) = (parts.next(), parts.next()) else {
            ogeom_bail!(Construction, "`{word}` is not a shape");
        };
        let (index, generation) = parse_key(node)?;
        let orientation = match orientation {
            "F" => Orientation::Forward,
            "R" => Orientation::Reversed,
            "I" => Orientation::Internal,
            "E" => Orientation::External,
            other => ogeom_bail!(Construction, "`{other}` is not an orientation"),
        };
        let location = match parts.next() {
            Some(chain) => parse_location(chain)?,
            None => Location::identity(),
        };
        if parts.next().is_some() {
            ogeom_bail!(Construction, "`{word}` has more parts than a shape has");
        }
        Ok(Shape::new(
            Key::from_parts(index, generation),
            location,
            orientation,
        ))
    }

    fn knots(&mut self) -> OgeomResult<KnotVector> {
        let degree = self.count()?;
        let n = self.count()?;
        let mut values = Vec::with_capacity(self.room(n));
        for _ in 0..n {
            values.push(self.number()?);
        }
        KnotVector::new(values, degree)
    }

    fn weighted(&mut self) -> OgeomResult<Weighted<Point>> {
        Ok(Weighted {
            scaled: self.point()?,
            weight: self.number()?,
        })
    }

    fn weighted2(&mut self) -> OgeomResult<Weighted<Point2>> {
        Ok(Weighted {
            scaled: self.point2()?,
            weight: self.number()?,
        })
    }

    fn curve(&mut self, tol: Tolerances) -> OgeomResult<Curve> {
        Ok(match self.word()? {
            "line" => {
                let axis = self.axis(tol)?;
                let (lo, hi) = self.range()?;
                LineCurve::over(axis, lo, hi)?.into()
            }
            "circle" => {
                let circle = Circle::new(self.frame(tol)?, self.number()?, tol)?;
                reverse_if(CircleCurve::new(circle).into(), self.flag()?)
            }
            "ellipse" => {
                let frame = self.frame(tol)?;
                let ellipse = Ellipse::new(frame, self.number()?, self.number()?, tol)?;
                reverse_if(EllipseCurve::new(ellipse).into(), self.flag()?)
            }
            "hyperbola" => {
                let frame = self.frame(tol)?;
                let h = Hyperbola::new(frame, self.number()?, self.number()?, tol)?;
                let (lo, hi) = self.range()?;
                let curve = HyperbolaCurve::over(h, lo, hi)?;
                reverse_if(curve.into(), self.flag()?)
            }
            "parabola" => {
                let frame = self.frame(tol)?;
                let p = Parabola::new(frame, self.number()?, tol)?;
                let (lo, hi) = self.range()?;
                let curve = ParabolaCurve::over(p, lo, hi)?;
                reverse_if(curve.into(), self.flag()?)
            }
            "helix" => {
                let frame = self.frame(tol)?;
                let radius = self.number()?;
                let pitch = self.number()?;
                let (lo, hi) = self.range()?;
                let curve = HelixCurve::over(frame, radius, pitch, lo, hi)?;
                reverse_if(curve.into(), self.flag()?)
            }
            "conical" => {
                let frame = self.frame(tol)?;
                let radius = self.number()?;
                let pitch = self.number()?;
                let taper = self.number()?;
                let (lo, hi) = self.range()?;
                let curve = HelixCurve::conical(frame, radius, pitch, taper, lo, hi)?;
                reverse_if(curve.into(), self.flag()?)
            }
            "bspline" => {
                let knots = self.knots()?;
                let n = self.count()?;
                let mut control = Vec::with_capacity(self.room(n));
                for _ in 0..n {
                    control.push(self.weighted()?);
                }
                BSplineCurve::rational(knots, control)?.into()
            }
            "bspline_periodic" => {
                let knots = self.knots()?;
                let n = self.count()?;
                let mut control = Vec::with_capacity(self.room(n));
                for _ in 0..n {
                    control.push(self.weighted()?);
                }
                BSplineCurve::periodic_from_parts(knots, control, tol)?.into()
            }
            "trimmed" => {
                let (lo, hi) = self.range()?;
                let reversed = self.flag()?;
                let basis = self.curve(tol)?;
                let curve = TrimmedCurve::new(basis, lo, hi, tol)?;
                reverse_if(Curve::Trimmed(Box::new(curve)), reversed)
            }
            "offset" => {
                let distance = self.number()?;
                let reference = self.direction(tol)?;
                let basis = self.curve(tol)?;
                Curve::Offset(Box::new(ogeom_geom::OffsetCurve::new(
                    basis, distance, reference,
                )?))
            }
            "onsurface" => {
                let pcurve = self.pcurve(tol)?;
                let surface = self.surface(tol)?;
                Curve::OnSurface(Box::new(ogeom_geom::CurveOnSurface::new(pcurve, surface)))
            }
            other => ogeom_bail!(Construction, "`{other}` is not a curve this reads"),
        })
    }

    fn pcurve(&mut self, tol: Tolerances) -> OgeomResult<PlanarCurve> {
        Ok(match self.word()? {
            "line2" => {
                let axis = self.axis2(tol)?;
                let (lo, hi) = self.range()?;
                Line2d::over(axis, lo, hi)?.into()
            }
            "circle2" => {
                let circle = Circle2::new(self.frame2(tol)?, self.number()?, tol)?;
                reverse_if(Circle2d::new(circle).into(), self.flag()?)
            }
            "ellipse2" => {
                let frame = self.frame2(tol)?;
                let ellipse = Ellipse2::new(frame, self.number()?, self.number()?, tol)?;
                reverse_if(Ellipse2d::new(ellipse).into(), self.flag()?)
            }
            "bspline2" => {
                let knots = self.knots()?;
                let n = self.count()?;
                let mut control = Vec::with_capacity(self.room(n));
                for _ in 0..n {
                    control.push(self.weighted2()?);
                }
                BSpline2d::rational(knots, control)?.into()
            }
            "trimmed2" => {
                let (lo, hi) = self.range()?;
                let reversed = self.flag()?;
                let basis = self.pcurve(tol)?;
                let curve = Trimmed2d::new(basis, lo, hi, tol)?;
                reverse_if(PlanarCurve::Trimmed(Box::new(curve)), reversed)
            }
            "offset2" => {
                let distance = self.number()?;
                let basis = self.pcurve(tol)?;
                PlanarCurve::Offset(Box::new(ogeom_geom::Offset2d::new(basis, distance)?))
            }
            "trig2" => {
                let c = self.point2()?;
                let d = ogeom_math::Vector2::new(self.number()?, self.number()?);
                let a = ogeom_math::Vector2::new(self.number()?, self.number()?);
                let b = ogeom_math::Vector2::new(self.number()?, self.number()?);
                let (lo, hi) = self.range()?;
                let curve = ogeom_geom::Trig2d::new(c, d, a, b, (lo, hi))?;
                reverse_if(PlanarCurve::Trig(curve), self.flag()?)
            }
            other => ogeom_bail!(Construction, "`{other}` is not a planar curve this reads"),
        })
    }

    fn surface(&mut self, tol: Tolerances) -> OgeomResult<SurfaceGeometry> {
        Ok(match self.word()? {
            "plane" => {
                let plane = Plane::new(self.frame(tol)?);
                PlaneSurface::over(plane, self.range()?, self.range()?)?.into()
            }
            "cylinder" => {
                let cylinder = Cylinder::new(self.frame(tol)?, self.number()?, tol)?;
                CylinderSurface::new(cylinder, self.range()?)?.into()
            }
            "cone" => {
                let frame = self.frame(tol)?;
                let cone = Cone::new(frame, self.number()?, self.number()?, tol)?;
                ConeSurface::new(cone, self.range()?)?.into()
            }
            "sphere" => {
                let sphere = Sphere::new(self.frame(tol)?, self.number()?, tol)?;
                SphereSurface::new(sphere).into()
            }
            "torus" => {
                let frame = self.frame(tol)?;
                let torus = Torus::new(frame, self.number()?, self.number()?, tol)?;
                TorusSurface::new(torus).into()
            }
            "bsurface" => {
                let u_knots = self.knots()?;
                let v_knots = self.knots()?;
                let u_count = self.count()?;
                let v_count = self.count()?;
                let Some(cells) = u_count.checked_mul(v_count) else {
                    ogeom_bail!(Construction, "a {u_count} by {v_count} control grid");
                };
                let mut points = Vec::with_capacity(self.room(cells));
                for _ in 0..cells {
                    points.push(self.weighted()?);
                }
                let grid = ControlGrid::new(points, u_count, v_count)?;
                BSplineSurface::rational(u_knots, v_knots, grid)?.into()
            }
            "revolution" => {
                let axis = self.axis(tol)?;
                let (start, end) = self.range()?;
                if start != 0.0 {
                    ogeom_bail!(
                        Construction,
                        "a revolution's angle starts at zero; this one starts at \
                         {start}"
                    );
                }
                let basis = self.curve(tol)?;
                RevolutionSurface::new(basis, axis, end)?.into()
            }
            "extrusion" => {
                let direction = self.direction(tol)?;
                let (start, end) = self.range()?;
                if start != 0.0 {
                    ogeom_bail!(
                        Construction,
                        "an extrusion's extent starts at zero; this one starts \
                         at {start}"
                    );
                }
                let basis = self.curve(tol)?;
                ExtrusionSurface::new(basis, direction, end)?.into()
            }
            "tsurface" => {
                let u_range = self.range()?;
                let v_range = self.range()?;
                let basis = self.surface(tol)?;
                SurfaceGeometry::Trimmed(Box::new(TrimmedSurface::new(
                    basis, u_range, v_range, tol,
                )?))
            }
            "osurface" => {
                let distance = self.number()?;
                let basis = self.surface(tol)?;
                SurfaceGeometry::Offset(Box::new(ogeom_geom::OffsetSurface::new(basis, distance)?))
            }
            other => ogeom_bail!(Construction, "`{other}` is not a surface this reads"),
        })
    }

    fn mesh(&mut self) -> OgeomResult<Triangulation> {
        let vertices = self.count()?;
        let triangles = self.count()?;
        let mut mesh = Triangulation::new();
        mesh.deflection_met = self.flag()?;
        for _ in 0..vertices {
            if self.word()? != "v" {
                ogeom_bail!(Construction, "expected a mesh vertex");
            }
            mesh.positions.push(self.point()?);
            mesh.normals.push(self.vector()?);
            mesh.parameters.push((self.number()?, self.number()?));
        }
        for _ in 0..triangles {
            if self.word()? != "f" {
                ogeom_bail!(Construction, "expected a mesh triangle");
            }
            let mut corners = [0_u32; 3];
            for corner in &mut corners {
                *corner = self.small()?;
                if *corner as usize >= vertices {
                    ogeom_bail!(
                        Dangling,
                        "a triangle names vertex {corner}, and the mesh has \
                         {vertices}"
                    );
                }
            }
            mesh.triangles.push(corners);
        }
        Ok(mesh)
    }

    fn provenance(&mut self) -> OgeomResult<Provenance> {
        Ok(match self.word()? {
            "primitive" => Provenance::Primitive {
                op: OpId(self.small()?),
                role: Role(self.small()?),
            },
            "derived" => {
                let op = OpId(self.small()?);
                let role = Role(self.small()?);
                let n = self.count()?;
                let mut from = Vec::with_capacity(self.room(n));
                for _ in 0..n {
                    let raw = self.count()?;
                    let Some(id) = EntityId::from_raw(raw as u64) else {
                        ogeom_bail!(Construction, "identity 0 was never issued");
                    };
                    from.push(id);
                }
                Provenance::Derived {
                    op,
                    from: from.into_iter().collect(),
                    role,
                }
            }
            "imported" => Provenance::Imported {
                source: SourceId(self.small()?),
                external: self.count()? as u64,
            },
            other => ogeom_bail!(Construction, "`{other}` is not a provenance"),
        })
    }

    fn node(&mut self) -> OgeomResult<TShape> {
        let kind = match self.word()? {
            "vertex" => ShapeType::Vertex,
            "edge" => ShapeType::Edge,
            "wire" => ShapeType::Wire,
            "face" => ShapeType::Face,
            "shell" => ShapeType::Shell,
            "solid" => ShapeType::Solid,
            "compsolid" => ShapeType::CompSolid,
            "compound" => ShapeType::Compound,
            other => ogeom_bail!(Construction, "`{other}` is not a kind of shape"),
        };

        // An edge's representations are records of their own, written after the
        // node line, so its data cannot be finished until the children on that
        // line have been read past.
        let mut pending = None;
        let mut data = match kind {
            ShapeType::Vertex => {
                let tolerance = self.number()?;
                NodeData::Vertex(VertexData::with_tolerance(self.point()?, tolerance)?)
            }
            ShapeType::Edge => {
                let mut edge = EdgeData::new();
                edge.tolerance = Tolerance::new(self.number()?)?;
                let agrees = self.flag()?;
                edge.degenerate = self.flag()?;
                pending = Some((agrees, self.count()?));
                NodeData::Edge(Box::new(edge))
            }
            ShapeType::Face => {
                let tolerance = Tolerance::new(self.number()?)?;
                let surface = self.handle()?;
                let at = self.location()?;
                let natural = self.flag()?;
                let mut face = if natural {
                    FaceData::natural(surface, at)
                } else {
                    FaceData::new(surface, at)
                };
                face.tolerance = tolerance;
                face.triangulation = match self.word()? {
                    "-" => None,
                    word => Some(Key::from_parts_of(parse_key(word)?)),
                };
                NodeData::Face(Box::new(face))
            }
            _ => NodeData::Container,
        };

        let n = self.count()?;
        let mut children = Vec::with_capacity(self.room(n));
        for _ in 0..n {
            children.push(self.shape()?);
        }

        if let (Some((agrees, count)), NodeData::Edge(edge)) = (pending, &mut data) {
            for _ in 0..count {
                if self.word()? != "r" {
                    ogeom_bail!(Construction, "expected an edge representation");
                }
                let repr = self.repr()?;
                edge.add(repr);
            }
            // Set last: `add` clears it, which is the right default for a
            // caller attaching a representation and the wrong one for a reader
            // restoring a claim the document already makes.
            edge.assert_same_parameter(agrees);
        }
        Ok(TShape::new(kind, data, children))
    }

    fn repr(&mut self) -> OgeomResult<EdgeRepr> {
        Ok(match self.word()? {
            "curve3d" => EdgeRepr::Curve3d {
                curve: self.handle()?,
                location: self.location()?,
                range: self.range()?,
            },
            "pcurve" => EdgeRepr::PCurve {
                curve: self.handle()?,
                surface: self.handle()?,
                location: self.location()?,
                range: self.range()?,
            },
            "seam" => EdgeRepr::Seam {
                forward: self.handle()?,
                reversed: self.handle()?,
                surface: self.handle()?,
                location: self.location()?,
                range: self.range()?,
            },
            "polyline" => {
                let location = self.location()?;
                let deflection = self.number()?;
                let n = self.count()?;
                let mut points = Vec::with_capacity(self.room(n));
                for _ in 0..n {
                    points.push(self.point()?);
                }
                let n = self.count()?;
                let mut parameters = Vec::with_capacity(self.room(n));
                for _ in 0..n {
                    parameters.push(self.number()?);
                }
                EdgeRepr::Polyline {
                    points,
                    parameters,
                    location,
                    deflection,
                }
            }
            "polygon-on" => {
                let triangulation = self.handle()?;
                let location = self.location()?;
                let n = self.count()?;
                let mut indices = Vec::with_capacity(self.room(n));
                for _ in 0..n {
                    let raw = self.count()?;
                    indices.push(u32::try_from(raw).map_err(|_| {
                        ogeom_core::ogeom_err!(Construction, "a mesh index does not fit u32")
                    })?);
                }
                EdgeRepr::PolygonOnTriangulation {
                    triangulation,
                    indices,
                    location,
                }
            }
            other => ogeom_bail!(
                Construction,
                "`{other}` is not an edge representation this reads"
            ),
        })
    }
}

/// Reverse a curve if the file says it runs backwards.
fn reverse_if<T: ogeom_geom::Reversible>(curve: T, reversed: bool) -> T {
    if reversed { curve.reversed() } else { curve }
}

fn parse_key(word: &str) -> OgeomResult<(u32, u32)> {
    let Some((index, generation)) = word.split_once(':') else {
        ogeom_bail!(Construction, "`{word}` is not a handle");
    };
    let (Ok(index), Ok(generation)) = (index.parse::<u32>(), generation.parse::<u32>()) else {
        ogeom_bail!(Construction, "`{word}` is not a handle");
    };
    Ok((index, generation))
}

fn parse_location(word: &str) -> OgeomResult<Location> {
    if word == "-" {
        return Ok(Location::identity());
    }
    let mut location = Location::identity();
    for step in word.split(',') {
        let Some((handle, power)) = step.rsplit_once('^') else {
            ogeom_bail!(Construction, "`{step}` is not a placement step");
        };
        let (index, generation) = parse_key(handle)?;
        let Ok(power) = power.parse::<i32>() else {
            ogeom_bail!(Construction, "`{power}` is not a power");
        };
        let datum: DatumId = Key::from_parts(index, generation);
        location = location.then(&Location::powered(datum, power));
    }
    Ok(location)
}

/// A handle from an already-parsed `(index, generation)` pair.
trait FromParsedKey: Sized {
    fn from_parts_of(parts: (u32, u32)) -> Self;
}

impl<T> FromParsedKey for Key<T> {
    fn from_parts_of((index, generation): (u32, u32)) -> Self {
        Self::from_parts(index, generation)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use ogeom_algo::{
        check, check_tessellation, make_box, make_cone, make_cylinder, make_prism, make_revolution,
        make_sphere, make_torus, make_wedge,
    };
    use ogeom_math::{Frame, Vector};
    use ogeom_mesh::{Deflection, triangulate};
    use ogeom_topo::explore_unique;

    const T: Tolerances = Tolerances::millimetres();

    fn fine() -> Deflection {
        Deflection {
            chord: 0.02,
            ..Deflection::default()
        }
    }

    /// One model holding every kind of shape this can build, and the roots that
    /// name them.
    #[test]
    fn a_document_round_trips_with_everything_it_says() {
        use ogeom_doc::{Colour, Dimension, GeometricTolerance, MeasureKind};
        let mut document = ogeom_doc::Document::new();
        let plate = make_box(document.model_mut(), Frame::WORLD, (40.0, 40.0, 5.0), T)
            .unwrap()
            .shape;
        let bolt = make_box(document.model_mut(), Frame::WORLD, (8.0, 8.0, 20.0), T)
            .unwrap()
            .shape;
        let plate_id = document.add_part("plate", plate.clone());
        let bolt_id = document.add_part("bolt", bolt.clone());
        let assembly = document.add_assembly("bolted plate");
        document
            .add_instance(assembly, plate_id, Transform::IDENTITY, None)
            .unwrap();
        document
            .add_instance(
                assembly,
                bolt_id,
                Transform::translation(ogeom_math::Vector::new(10.0, 10.0, 5.0)),
                Some("bolt one".into()),
            )
            .unwrap();
        document
            .set_product_colour(plate_id, Colour::rgb(0.1, 0.8, 0.2))
            .unwrap();
        let face = explore_unique(document.model(), &bolt, ShapeType::Face).unwrap()[0].clone();
        document.set_colour(&face, Colour::rgb(0.9, 0.1, 0.1));
        document.set_name(&face, "the mounting face");
        document.pmi_mut().dimensions.push(Dimension {
            name: "length".into(),
            values: vec![20.0],
            kind: MeasureKind::Length,
            plus: Some(0.1),
            minus: Some(-0.1),
            features: vec![vec![face.node()]],
            location: false,
        });
        document.pmi_mut().tolerances.push(GeometricTolerance {
            kind: "flatness".into(),
            name: "Flatness.1".into(),
            magnitude: 0.05,
            modifiers: vec!["maximum_material_requirement".into()],
            datums: vec!["A".into(), "A-B".into()],
            items: vec![face.node()],
        });
        document.pmi_mut().datums.push(ogeom_doc::Datum {
            label: "A".into(),
            items: vec![face.node()],
        });

        // The attribute layer, all of it: a property of each kind, a
        // material with density and colour assigned to the bolt, two
        // layers with the face on both and one hidden, and validation
        // values recorded against the plate.
        document.set_property(
            &face,
            ogeom_doc::Property {
                name: "finish".into(),
                value: ogeom_doc::PropertyValue::Text("brushed".into()),
            },
        );
        document.set_property(
            &face,
            ogeom_doc::Property {
                name: "cost".into(),
                value: ogeom_doc::PropertyValue::Number(12.5),
            },
        );
        document.set_property(
            &bolt,
            ogeom_doc::Property {
                name: "critical".into(),
                value: ogeom_doc::PropertyValue::Flag(true),
            },
        );
        let steel = document.add_material(ogeom_doc::Material {
            name: "AISI 304".into(),
            density: Some(7900.0),
            colour: Some(Colour::rgb(0.7, 0.7, 0.75)),
        });
        document.assign_material(&bolt, steel);
        let outline = document.add_layer("outline");
        let hidden = document.add_layer("construction");
        document.set_layer_visible(hidden, false);
        document.place_on_layer(&face, outline);
        document.place_on_layer(&face, hidden);
        document.set_validation(
            &plate,
            ogeom_doc::ValidationProperties {
                volume: 8000.0,
                area: 2400.0,
                centroid: ogeom_math::Point::new(10.0, 10.0, 2.5),
            },
        );

        let text = write_document(&document, WriteOptions::default()).unwrap();
        let back = read_document(&text).unwrap();
        let again = write_document(&back, WriteOptions::default()).unwrap();
        assert_eq!(text, again, "the second write reproduces the first");

        // The attribute layer survives whole.
        // The face rebinds through its persisted key: the names carry it.
        let read_face = back
            .names()
            .find(|(_, n)| *n == "the mounting face")
            .map(|(node, _)| ogeom_topo::Shape::of(node))
            .unwrap();
        let properties = back.properties_of(&read_face);
        assert_eq!(properties.len(), 2);
        assert!(
            properties.iter().any(|p| p.name == "finish"
                && p.value == ogeom_doc::PropertyValue::Text("brushed".into()))
        );
        assert!(
            properties
                .iter()
                .any(|p| p.name == "cost" && p.value == ogeom_doc::PropertyValue::Number(12.5))
        );
        assert_eq!(back.materials().len(), 1);
        assert_eq!(back.materials()[0].name, "AISI 304");
        assert_eq!(back.materials()[0].density, Some(7900.0));
        assert_eq!(back.layers().len(), 2);
        assert!(back.layers()[0].visible);
        assert!(!back.layers()[1].visible);
        assert_eq!(back.layers_of(&read_face).len(), 2);
        let validation = back
            .validations()
            .next()
            .map(|(_, v)| v)
            .expect("the plate's check values survive");
        assert!(validation.agrees_with(
            &ogeom_doc::ValidationProperties {
                volume: 8000.0,
                area: 2400.0,
                centroid: ogeom_math::Point::new(10.0, 10.0, 2.5),
            },
            1e-9,
        ));

        // Structure: same products, same occurrences at the same places.
        let names: Vec<&str> = back.products().map(|(_, p)| p.name.as_str()).collect();
        assert_eq!(names, ["plate", "bolt", "bolted plate"]);
        let root = back.roots()[0];
        let mut occurrences = back.occurrences_of(root).unwrap();
        occurrences.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(occurrences.len(), 2);
        assert_eq!(occurrences[0].path, "bolted plate/bolt one");
        let world = occurrences[0]
            .shape
            .transform(back.model().datums())
            .unwrap()
            .apply(Point::new(0.0, 0.0, 0.0));
        assert!(world.is_equal(Point::new(10.0, 10.0, 5.0), T));

        // Appearance, names and PMI, intact.
        let bolt_face = explore_unique(back.model(), &occurrences[0].shape, ShapeType::Face)
            .unwrap()[0]
            .clone();
        assert_eq!(back.colour_of(&bolt_face), Some(Colour::rgb(0.9, 0.1, 0.1)));
        assert_eq!(back.name_of(&bolt_face), Some("the mounting face"));
        assert_eq!(back.pmi().dimensions.len(), 1);
        assert_eq!(back.pmi().dimensions[0].values, [20.0]);
        assert_eq!(back.pmi().tolerances[0].kind, "flatness");
        assert_eq!(
            back.pmi().tolerances[0].modifiers,
            ["maximum_material_requirement"]
        );
        assert_eq!(back.pmi().tolerances[0].datums, ["A", "A-B"]);
        assert_eq!(back.pmi().datums[0].label, "A");
        assert_eq!(back.pmi().datums[0].items.len(), 1);
    }

    #[test]
    fn asymmetric_conic_domains_round_trip() {
        // The domain a constructor happens to produce is symmetric; the
        // format carries whatever the curve actually spans.
        let mut model = Model::new();
        let hyperbola = ogeom_geom::HyperbolaCurve::over(
            Hyperbola::new(Frame::WORLD, 2.0, 1.0, T).unwrap(),
            -0.5,
            1.75,
        )
        .unwrap();
        let parabola = ogeom_geom::ParabolaCurve::over(
            Parabola::new(Frame::WORLD, 1.5, T).unwrap(),
            0.25,
            3.0,
        )
        .unwrap();
        for curve in [Curve::Hyperbola(hyperbola), Curve::Parabola(parabola)] {
            let domain = Curve3d::domain(&curve);
            ogeom_algo::make_edge(&mut model, curve, domain, T).unwrap();
        }
        let text = write(&model, &[], WriteOptions::default()).unwrap();
        let (back, _) = read(&text).unwrap();
        let again = write(&back, &[], WriteOptions::default()).unwrap();
        assert_eq!(text, again, "the second write reproduces the first");
        let domains: Vec<(f64, f64)> = back
            .geometry()
            .curves()
            .map(|(_, c)| Curve3d::domain(c))
            .collect();
        assert!(domains.contains(&(-0.5, 1.75)));
        assert!(domains.contains(&(0.25, 3.0)));
    }

    #[test]
    fn derived_geometry_round_trips_byte_stable() {
        use ogeom_geom::{
            CurveOnSurface, CylinderSurface, OffsetCurve, OffsetSurface, PlanarCurve,
        };
        use ogeom_math::Cylinder;

        let mut model = Model::new();
        // An edge on an offset circle: the concentric circle by another
        // spelling.
        let circle: Curve =
            ogeom_geom::CircleCurve::new(ogeom_math::Circle::new(Frame::WORLD, 2.0, T).unwrap())
                .into();
        let offset = Curve::Offset(Box::new(
            OffsetCurve::new(circle, 1.0, ogeom_math::Direction::Z).unwrap(),
        ));
        let domain = Curve3d::domain(&offset);
        ogeom_algo::make_edge(&mut model, offset, domain, T).unwrap();

        // An edge on a surface curve: a sloped chart line on a cylinder.
        let cylinder = SurfaceGeometry::Cylinder(
            CylinderSurface::new(Cylinder::new(Frame::WORLD, 3.0, T).unwrap(), (0.0, 8.0)).unwrap(),
        );
        let chart_line = PlanarCurve::Line(
            ogeom_geom::Line2d::segment(
                ogeom_math::Point2::new(0.0, 0.0),
                ogeom_math::Point2::new(3.0, 5.0),
                T,
            )
            .unwrap(),
        );
        let lifted = Curve::OnSurface(Box::new(CurveOnSurface::new(chart_line, cylinder.clone())));
        let domain = Curve3d::domain(&lifted);
        ogeom_algo::make_edge(&mut model, lifted, domain, T).unwrap();

        // A loose offset surface in the store.
        model
            .geometry_mut()
            .add_surface(SurfaceGeometry::Offset(Box::new(
                OffsetSurface::new(cylinder, -0.5).unwrap(),
            )));

        let text = write(&model, &[], WriteOptions::default()).unwrap();
        let (back, _) = read(&text).unwrap();
        let again = write(&back, &[], WriteOptions::default()).unwrap();
        assert_eq!(text, again, "the second write reproduces the first");
        assert!(
            back.geometry()
                .curves()
                .any(|(_, c)| matches!(c, Curve::Offset(_)))
                && back
                    .geometry()
                    .curves()
                    .any(|(_, c)| matches!(c, Curve::OnSurface(_)))
        );
    }

    #[test]
    fn a_version_one_document_still_reads() {
        // Version 2 only added records; a version 1 file contains none of
        // them, and the promise is that it keeps reading forever.
        let mut model = Model::new();
        ogeom_algo::make_box(&mut model, Frame::WORLD, (2.0, 2.0, 2.0), T).unwrap();
        let text = write(&model, &[], WriteOptions::default()).unwrap();
        let downgraded = text.replacen("ogeom 2", "ogeom 1", 1);
        assert_ne!(text, downgraded, "the header really carried version 2");
        let (back, _) = read(&downgraded).unwrap();
        assert_eq!(
            back.geometry().surfaces().count(),
            model.geometry().surfaces().count()
        );
    }

    #[test]
    fn a_conical_helix_and_a_periodic_bspline_round_trip_byte_stable() {
        let mut model = Model::new();
        let helix = ogeom_geom::HelixCurve::conical(
            Frame::new(
                ogeom_math::Point::new(1.0, 2.0, -0.5),
                ogeom_math::Direction::from_coords(0.0, 0.6, 0.8, T).unwrap(),
                ogeom_math::Direction::X,
                T,
            )
            .unwrap(),
            4.0,
            3.0,
            1.25,
            0.5,
            9.0,
        )
        .unwrap();
        let domain = Curve3d::domain(&helix);
        ogeom_algo::make_edge(&mut model, Curve::Helix(helix), domain, T).unwrap();

        let ring: Vec<ogeom_math::Point> = (0..8)
            .map(|i| {
                let a = core::f64::consts::TAU * f64::from(i) / 8.0;
                ogeom_math::Point::new(a.cos() * 4.0, a.sin() * 4.0, 0.0)
            })
            .collect();
        let loop_curve = ogeom_geom::BSplineCurve::periodic(&ring, 3, T).unwrap();
        let domain = Curve3d::domain(&loop_curve);
        ogeom_algo::make_edge(&mut model, Curve::BSpline(loop_curve), domain, T).unwrap();

        let text = write(&model, &[], WriteOptions::default()).unwrap();
        let (back, _) = read(&text).unwrap();
        let again = write(&back, &[], WriteOptions::default()).unwrap();
        assert_eq!(text, again, "the second write reproduces the first");
        assert!(
            back.geometry()
                .curves()
                .any(|(_, c)| matches!(c, Curve::Helix(h) if (h.taper() - 1.25).abs() < 1e-12))
        );
        assert!(
            back.geometry()
                .curves()
                .any(|(_, c)| matches!(c, Curve::BSpline(b) if b.is_periodic()))
        );
    }

    #[test]
    fn a_helix_edge_round_trips_byte_stable() {
        let mut model = Model::new();
        let helix = ogeom_geom::HelixCurve::over(
            Frame::new(
                ogeom_math::Point::new(1.0, 2.0, -0.5),
                ogeom_math::Direction::from_coords(0.0, 0.6, 0.8, T).unwrap(),
                ogeom_math::Direction::X,
                T,
            )
            .unwrap(),
            4.0,
            -1.5,
            0.5,
            9.0,
        )
        .unwrap();
        let domain = Curve3d::domain(&helix);
        ogeom_algo::make_edge(&mut model, Curve::Helix(helix), domain, T).unwrap();
        let text = write(&model, &[], WriteOptions::default()).unwrap();
        let (back, _) = read(&text).unwrap();
        let again = write(&back, &[], WriteOptions::default()).unwrap();
        assert_eq!(text, again, "the second write reproduces the first");
        let restored = back
            .geometry()
            .curves()
            .find_map(|(_, c)| match c {
                Curve::Helix(h) => Some(*h),
                _ => None,
            })
            .expect("the helix survives");
        assert_eq!(restored.radius(), 4.0);
        assert_eq!(restored.pitch(), -1.5);
        assert_eq!(Curve3d::domain(&restored), (0.5, 9.0));
    }

    fn everything() -> (Model, Vec<Shape>) {
        let mut model = Model::new();
        let mut roots = vec![
            make_box(&mut model, Frame::WORLD, (2.0, 3.0, 4.0), T)
                .unwrap()
                .shape,
            make_cylinder(&mut model, Frame::WORLD, 2.0, 5.0, T)
                .unwrap()
                .shape,
            make_sphere(&mut model, Frame::WORLD, 3.0, T).unwrap().shape,
            make_cone(&mut model, Frame::WORLD, 3.0, 1.0, 4.0, T)
                .unwrap()
                .shape,
            make_cone(&mut model, Frame::WORLD, 3.0, 0.0, 4.0, T)
                .unwrap()
                .shape,
            make_torus(&mut model, Frame::WORLD, 5.0, 2.0, T)
                .unwrap()
                .shape,
            make_wedge(&mut model, Frame::WORLD, (4.0, 4.0, 6.0), (2.0, 2.0), T)
                .unwrap()
                .shape,
        ];

        // A prism, so an extrusion surface and a placement datum are in there.
        let face = explore_unique(&model, &roots[0], ShapeType::Face).unwrap()[0].clone();
        roots.push(
            make_prism(&mut model, &face, Vector::new(0.0, 0.0, 2.0), T)
                .unwrap()
                .shape,
        );
        // And a revolution, so a surface of revolution and a seam are too. The
        // profile has to lie in a plane the axis runs through and stay clear of
        // it, or the swept solid would pass through itself, so it is the box's
        // `-y` face, spanning x and z, turned about a z axis five units away.
        let side = explore_unique(&model, &roots[0], ShapeType::Face)
            .unwrap()
            .into_iter()
            .find(|f| {
                model
                    .provenance_of(f)
                    .and_then(ogeom_core::Provenance::role)
                    == Some(ogeom_algo::primitive::roles::FACE_MIN_Y)
            })
            .expect("the box has a -y face");
        let axis = ogeom_math::Axis::new(
            ogeom_math::Point::new(-5.0, 0.0, 0.0),
            ogeom_math::Direction::Z,
        );
        roots.push(
            make_revolution(&mut model, &side, axis, 1.0, T)
                .unwrap()
                .shape,
        );
        (model, roots)
    }

    #[test]
    fn a_file_that_is_not_one_is_refused() {
        assert!(read("").is_err());
        assert!(read("something else 1").is_err());
        assert!(read(&format!("{MAGIC} 99")).is_err());
        assert!(read(&format!("{MAGIC} 1\nnonsense\n")).is_err());
    }

    #[test]
    fn writing_what_was_read_gives_the_same_bytes() {
        // The property the whole format exists for. If a round trip through it
        // is not the identity, `diff` says exactly where, which is why this is
        // text and not a binary blob.
        let (model, roots) = everything();
        let first = write(&model, &roots, WriteOptions::default()).unwrap();
        let (restored, restored_roots) = read(&first).unwrap();
        let second = write(&restored, &restored_roots, WriteOptions::default()).unwrap();

        if first != second {
            let mismatch = first
                .lines()
                .zip(second.lines())
                .enumerate()
                .find(|(_, (a, b))| a != b);
            panic!("the round trip changed the document: {mismatch:?}");
        }
        assert_eq!(restored_roots.len(), roots.len());
    }

    #[test]
    fn the_restored_model_is_the_same_model() {
        // Byte equality of the file is necessary and not sufficient: what
        // matters is that the shapes still answer the same way. Same counts,
        // same validity, same measurements, same handles.
        let (model, roots) = everything();
        let text = write(&model, &roots, WriteOptions::default()).unwrap();
        let (restored, restored_roots) = read(&text).unwrap();

        assert_eq!(restored.node_count(), model.node_count());
        assert_eq!(restored.geometry().counts(), model.geometry().counts());
        assert_eq!(restored.current_operation(), model.current_operation());

        for (before, after) in roots.iter().zip(&restored_roots) {
            // The *slot* came back, not merely something like it: same index,
            // same generation. Not the same handle, though: a restored
            // document is a new set of arenas and its handles say so, which is
            // what stops one document's shape from resolving against another.
            assert_eq!(before.node().index(), after.node().index());
            assert_eq!(before.node().generation(), after.node().generation());
            assert_ne!(
                before.node().scope(),
                after.node().scope(),
                "a restored document should not answer to the original's handles"
            );
            assert!(model.node(after).is_none(), "and not the other way round");
            // A reference kept across a save is re-found by its *identity*,
            // not by its handle. `bind` deliberately refuses a shape from
            // another document (relabelling one would hand back something that
            // resolves and answers about a different entity), so the way
            // through is the identity (`docs/DATA_MODEL.md` §8).
            assert!(
                restored.bind(before).is_err(),
                "a foreign handle should not be re-homed"
            );

            for kind in [
                ShapeType::Face,
                ShapeType::Edge,
                ShapeType::Vertex,
                ShapeType::Shell,
            ] {
                assert_eq!(
                    explore_unique(&restored, after, kind).unwrap().len(),
                    explore_unique(&model, before, kind).unwrap().len(),
                    "{kind:?} count changed"
                );
            }

            assert!(
                check(&restored, after, T).unwrap().is_valid(),
                "restored shape is invalid: {}",
                check(&restored, after, T).unwrap()
            );
            assert!(
                check_tessellation(&restored, after, fine(), T)
                    .unwrap()
                    .is_valid(),
                "restored shape's mesh came apart"
            );

            let before_mesh = triangulate(&model, before, fine(), T).unwrap();
            let after_mesh = triangulate(&restored, after, fine(), T).unwrap();
            assert_eq!(after_mesh.triangle_count(), before_mesh.triangle_count());
            assert_relative_eq!(
                after_mesh.volume(),
                before_mesh.volume(),
                max_relative = 0.0
            );
        }
    }

    #[test]
    fn provenance_and_identity_survive() {
        // The reason reading does not go through the builders. A rebuild
        // through them would mint fresh identities, and every reference
        // recorded against the old ones ("the top face of that box") would
        // resolve to nothing or, worse, to something else.
        let (model, roots) = everything();
        let text = write(&model, &roots, WriteOptions::default()).unwrap();
        let (restored, restored_roots) = read(&text).unwrap();

        for (before, after) in roots.iter().zip(&restored_roots) {
            let faces = explore_unique(&model, before, ShapeType::Face).unwrap();
            let restored_faces = explore_unique(&restored, after, ShapeType::Face).unwrap();
            assert_eq!(faces.len(), restored_faces.len());
            for (a, b) in faces.iter().zip(&restored_faces) {
                assert_eq!(
                    restored.identity_of(b),
                    model.identity_of(a),
                    "an identity was renumbered"
                );
                // And the identity is enough to find the entity again in the
                // reloaded document, which is what makes a saved reference
                // survive at all.
                if let Some(id) = model.identity_of(a) {
                    let found = restored.shape_of(id).expect("the entity is still there");
                    assert!(found.is_partner(b), "identity found the wrong node");
                }
                assert_eq!(
                    restored.provenance_of(b),
                    model.provenance_of(a),
                    "a provenance record changed"
                );
                assert_eq!(restored.roots_of(b), model.roots_of(a));
            }
        }
        assert_eq!(
            restored.provenance().len(),
            model.provenance().len(),
            "the table changed length"
        );
    }

    #[test]
    fn everything_reads_into_a_live_model_and_still_answers_the_same() {
        // The same claims `the_restored_model_is_the_same_model` makes, with
        // the document landing in a model that already has things in it.
        let (model, roots) = everything();
        let text = write(&model, &roots, WriteOptions::default()).unwrap();

        let mut target = Model::new();
        let resident = make_box(&mut target, Frame::WORLD, (7.0, 7.0, 7.0), T)
            .unwrap()
            .shape;
        let absorbed = read_into(&mut target, &text).unwrap();
        assert_eq!(absorbed.shapes.len(), roots.len());

        for (before, after) in roots.iter().zip(&absorbed.shapes) {
            for kind in [
                ShapeType::Face,
                ShapeType::Edge,
                ShapeType::Vertex,
                ShapeType::Shell,
            ] {
                assert_eq!(
                    explore_unique(&target, after, kind).unwrap().len(),
                    explore_unique(&model, before, kind).unwrap().len(),
                    "{kind:?} count changed"
                );
            }
            assert!(
                check(&target, after, T).unwrap().is_valid(),
                "absorbed shape is invalid: {}",
                check(&target, after, T).unwrap()
            );
            let before_mesh = triangulate(&model, before, fine(), T).unwrap();
            let after_mesh = triangulate(&target, after, fine(), T).unwrap();
            assert_eq!(after_mesh.triangle_count(), before_mesh.triangle_count());
            assert_relative_eq!(
                after_mesh.volume(),
                before_mesh.volume(),
                max_relative = 0.0
            );
        }

        // What lived here before is untouched and still measures.
        assert!(check(&target, &resident, T).unwrap().is_valid());
        assert_relative_eq!(
            triangulate(&target, &resident, fine(), T).unwrap().volume(),
            343.0,
            max_relative = 1e-9
        );
    }

    #[test]
    fn identities_read_into_a_live_model_survive_under_an_offset() {
        // `provenance_and_identity_survive`'s sibling: the ids are not the
        // file's own (the target had already issued some), but the remap
        // table says exactly where each one landed, and everything found
        // through it answers the same.
        let (model, roots) = everything();
        let text = write(&model, &roots, WriteOptions::default()).unwrap();

        let mut target = Model::new();
        make_box(&mut target, Frame::WORLD, (7.0, 7.0, 7.0), T).unwrap();
        let issued_before = target.provenance().len() as u64;
        assert!(issued_before > 0);
        let absorbed = read_into(&mut target, &text).unwrap();

        for (before, after) in roots.iter().zip(&absorbed.shapes) {
            let faces = explore_unique(&model, before, ShapeType::Face).unwrap();
            let absorbed_faces = explore_unique(&target, after, ShapeType::Face).unwrap();
            assert_eq!(faces.len(), absorbed_faces.len());
            for (a, b) in faces.iter().zip(&absorbed_faces) {
                let old = model.identity_of(a).expect("faces carry identities");
                let new = absorbed.entities[&old];
                assert_eq!(new.get(), old.get() + issued_before, "not a plain shift");
                assert_eq!(target.identity_of(b), Some(new));
                let found = target.shape_of(new).expect("the entity is findable");
                assert!(found.is_partner(b), "identity found the wrong node");
                assert_eq!(
                    target.provenance_of(b).and_then(Provenance::role),
                    model.provenance_of(a).and_then(Provenance::role),
                    "a role changed in the shift"
                );
            }
        }
        assert_eq!(
            target.provenance().len() as u64,
            issued_before + model.provenance().len() as u64
        );
    }

    #[test]
    fn a_document_in_other_units_does_not_read_into_a_millimetre_model() {
        let mut metric = Model::with_tolerances(Tolerances::metres());
        let solid = make_box(
            &mut metric,
            Frame::WORLD,
            (1.0, 1.0, 1.0),
            Tolerances::metres(),
        )
        .unwrap()
        .shape;
        let text = write(
            &metric,
            std::slice::from_ref(&solid),
            WriteOptions::default(),
        )
        .unwrap();

        let mut target = Model::new();
        make_box(&mut target, Frame::WORLD, (1.0, 1.0, 1.0), T).unwrap();
        let nodes_before = target.node_count();
        assert!(
            read_into(&mut target, &text).is_err(),
            "a metre document should not land in a millimetre model"
        );
        assert_eq!(
            target.node_count(),
            nodes_before,
            "a refused read should leave the model alone"
        );
    }

    #[test]
    fn reading_the_same_document_into_a_model_twice_gives_independent_copies() {
        let mut model = Model::new();
        let solid = make_box(&mut model, Frame::WORLD, (1.0, 2.0, 3.0), T)
            .unwrap()
            .shape;
        let text = write(
            &model,
            std::slice::from_ref(&solid),
            WriteOptions::default(),
        )
        .unwrap();

        let mut target = Model::new();
        let first = read_into(&mut target, &text).unwrap();
        let second = read_into(&mut target, &text).unwrap();

        let a = &first.shapes[0];
        let b = &second.shapes[0];
        assert_ne!(
            a.node().index(),
            b.node().index(),
            "the copies share a node"
        );
        // Containers carry no identity; a face of each copy does, and they
        // must differ: the second read is a second body, not an alias.
        let face_a = explore_unique(&target, a, ShapeType::Face).unwrap()[0].clone();
        let face_b = explore_unique(&target, b, ShapeType::Face).unwrap()[0].clone();
        assert_ne!(
            target.identity_of(&face_a),
            target.identity_of(&face_b),
            "the copies share an identity"
        );
        assert_relative_eq!(
            triangulate(&target, a, fine(), T).unwrap().volume(),
            triangulate(&target, b, fine(), T).unwrap().volume(),
            max_relative = 0.0
        );
    }

    #[test]
    fn read_into_reads_the_same_version_read_does() {
        // `read_into` reads the same grammar under the same VERSION, so both
        // readers refuse the same unknown one.
        let future = format!("{MAGIC} 99\nunits 1.0\n");
        assert!(read(&future).is_err());
        let mut target = Model::new();
        assert!(read_into(&mut target, &future).is_err());
    }

    #[test]
    fn a_document_with_meshes_reads_into_a_live_model() {
        let mut model = Model::new();
        let solid = make_box(&mut model, Frame::WORLD, (1.0, 2.0, 3.0), T)
            .unwrap()
            .shape;
        ogeom_mesh::tessellate(&mut model, &solid, fine(), T).unwrap();
        assert!(model.geometry().triangulation_count() > 0);
        let text = write(
            &model,
            std::slice::from_ref(&solid),
            WriteOptions::default(),
        )
        .unwrap();

        let mut target = Model::new();
        make_box(&mut target, Frame::WORLD, (7.0, 7.0, 7.0), T).unwrap();
        let absorbed = read_into(&mut target, &text).unwrap();
        assert_eq!(
            target.geometry().triangulation_count(),
            model.geometry().triangulation_count()
        );
        // The absorbed faces' cached-mesh handles resolve where they landed.
        for face in explore_unique(&target, &absorbed.shapes[0], ShapeType::Face).unwrap() {
            let node = target.node(&face).unwrap();
            let mesh = node
                .data()
                .as_face()
                .unwrap()
                .triangulation
                .expect("the cache travelled");
            assert!(target.geometry().triangulation(mesh).is_some());
        }
    }

    #[test]
    fn tolerances_survive_entity_by_entity() {
        // Per-entity and they only grow (`docs/DATA_MODEL.md` §5). A format
        // that wrote one number for the document would quietly tighten every
        // edge a repair had widened.
        let mut model = Model::new();
        let solid = make_box(&mut model, Frame::WORLD, (1.0, 1.0, 1.0), T)
            .unwrap()
            .shape;
        let edge = explore_unique(&model, &solid, ShapeType::Edge).unwrap()[0].clone();
        model.widen(&edge, Tolerance::new(1e-3).unwrap()).unwrap();

        let text = write(
            &model,
            std::slice::from_ref(&solid),
            WriteOptions::default(),
        )
        .unwrap();
        let (restored, roots) = read(&text).unwrap();
        for kind in [ShapeType::Vertex, ShapeType::Edge, ShapeType::Face] {
            let before = explore_unique(&model, &solid, kind).unwrap();
            let after = explore_unique(&restored, &roots[0], kind).unwrap();
            for (a, b) in before.iter().zip(&after) {
                assert_eq!(
                    restored.tolerance_of(b).unwrap().map(Tolerance::get),
                    model.tolerance_of(a).unwrap().map(Tolerance::get),
                    "a {kind:?} tolerance changed"
                );
            }
        }
        assert!(
            restored
                .tolerance_of(&roots[0])
                .into_iter()
                .flatten()
                .count()
                <= 1
        );
    }

    #[test]
    fn a_cached_mesh_survives_and_omitting_it_says_so() {
        let mut model = Model::new();
        let solid = make_box(&mut model, Frame::WORLD, (1.0, 2.0, 3.0), T)
            .unwrap()
            .shape;
        ogeom_mesh::tessellate(&mut model, &solid, fine(), T).unwrap();
        assert!(model.geometry().triangulation_count() > 0);

        let text = write(
            &model,
            std::slice::from_ref(&solid),
            WriteOptions::default(),
        )
        .unwrap();
        let (restored, _) = read(&text).unwrap();
        assert_eq!(
            restored.geometry().triangulation_count(),
            model.geometry().triangulation_count()
        );
        for (before, after) in model
            .geometry()
            .triangulations()
            .zip(restored.geometry().triangulations())
        {
            assert_eq!(before.1, after.1, "a cached mesh changed");
        }

        // Omitted on request, and the file says so rather than leaving it to be
        // inferred from an absence.
        let without = write(
            &model,
            std::slice::from_ref(&solid),
            WriteOptions {
                triangulations: false,
            },
        )
        .unwrap();
        assert!(without.contains("triangulation(s) omitted by request"));
        assert!(
            without.len() < text.len(),
            "omitting the mesh saved nothing"
        );
        let (bare, bare_roots) = read(&without).unwrap();
        assert_eq!(bare.geometry().triangulation_count(), 0);
        // And the shape is still a shape: the mesh was a cache, not the model.
        assert!(check(&bare, &bare_roots[0], T).unwrap().is_valid());
    }

    #[test]
    fn a_document_naming_a_handle_that_is_not_there_is_refused() {
        // A corrupt file is an error, not a model that answers about the wrong
        // entity.
        let mut model = Model::new();
        let solid = make_box(&mut model, Frame::WORLD, (1.0, 1.0, 1.0), T)
            .unwrap()
            .shape;
        let text = write(
            &model,
            std::slice::from_ref(&solid),
            WriteOptions::default(),
        )
        .unwrap();

        let broken = text.replace("curve3d 0:0", "curve3d 999:0");
        assert_ne!(broken, text, "the substitution found nothing to do");
        assert!(read(&broken).is_err(), "a dangling curve was accepted");

        // And an identity nobody issued.
        let broken = format!("{text}identity 0:0 99999\n");
        assert!(read(&broken).is_err(), "a dangling identity was accepted");

        // And a node bound to an identity that is fine, but to a node that is
        // not there.
        let broken = format!("{text}identity 9999:0 1\n");
        assert!(
            read(&broken).is_err(),
            "a dangling node binding was accepted"
        );
    }

    #[test]
    fn records_out_of_arena_order_are_refused() {
        // The order *is* the numbering: a reader replays inserts to reproduce
        // the handles. A gap would shift every later reference by one and
        // nothing would say so.
        let mut model = Model::new();
        let solid = make_box(&mut model, Frame::WORLD, (1.0, 1.0, 1.0), T)
            .unwrap()
            .shape;
        let text = write(
            &model,
            std::slice::from_ref(&solid),
            WriteOptions::default(),
        )
        .unwrap();
        let shuffled: String = {
            let mut lines: Vec<&str> = text.lines().collect();
            let first = lines
                .iter()
                .position(|l| l.starts_with("curve 0:0"))
                .expect("a curve");
            lines.swap(first, first + 1);
            lines.join("\n")
        };
        assert!(
            read(&shuffled).is_err(),
            "an out-of-order curve was accepted"
        );
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let mut model = Model::new();
        let solid = make_box(&mut model, Frame::WORLD, (1.0, 1.0, 1.0), T)
            .unwrap()
            .shape;
        let text = write(
            &model,
            std::slice::from_ref(&solid),
            WriteOptions::default(),
        )
        .unwrap();
        let annotated = format!("# a note\n\n{text}\n\n# and another\n");
        let (restored, roots) = read(&annotated).unwrap();
        assert_eq!(restored.node_count(), model.node_count());
        assert_eq!(roots.len(), 1);
    }

    #[test]
    fn a_root_that_is_not_in_the_model_is_refused() {
        let mut model = Model::new();
        make_box(&mut model, Frame::WORLD, (1.0, 1.0, 1.0), T).unwrap();
        let beyond = Shape::of(Key::from_parts(9999, 0));
        assert!(
            write(
                &model,
                std::slice::from_ref(&beyond),
                WriteOptions::default()
            )
            .is_err()
        );

        // A handle from a different model is caught by the same check: arena
        // keys carry the identifier of the arena that issued them, so a foreign
        // one resolves to nothing here rather than to whatever node sits at
        // that index.
        let mut elsewhere = Model::new();
        let foreign = ogeom_algo::make_sphere(&mut elsewhere, Frame::WORLD, 1.0, T)
            .unwrap()
            .shape;
        assert!(
            write(
                &model,
                std::slice::from_ref(&foreign),
                WriteOptions::default()
            )
            .is_err()
        );
    }

    #[test]
    fn every_float_comes_back_bit_for_bit() {
        // The format is exact, not merely close. A text format that printed six
        // decimals would give a round trip that always disagrees a little, and
        // then a real disagreement has nothing to stand out against.
        let mut model = Model::new();
        let awkward = [
            0.1_f64,
            -0.0,
            1e-300,
            1.0 / 3.0,
            f64::MAX / 4.0,
            f64::MIN_POSITIVE,
            std::f64::consts::PI,
        ];
        let mut points = Vec::new();
        for value in awkward {
            points.push(model.add_point(ogeom_math::Point::new(value, -value, value * 2.0)));
        }
        let text = write(&model, &points, WriteOptions::default()).unwrap();
        let (restored, roots) = read(&text).unwrap();
        for (before, after) in points.iter().zip(&roots) {
            let a = model
                .node(before)
                .unwrap()
                .data()
                .as_vertex()
                .unwrap()
                .point;
            let b = restored
                .node(after)
                .unwrap()
                .data()
                .as_vertex()
                .unwrap()
                .point;
            assert_eq!(a.x.to_bits(), b.x.to_bits());
            assert_eq!(a.y.to_bits(), b.y.to_bits());
            assert_eq!(a.z.to_bits(), b.z.to_bits());
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod unit_tests {
    use super::*;
    use ogeom_algo::make_box;
    use ogeom_math::Frame;

    #[test]
    fn a_documents_unit_scale_survives_the_round_trip() {
        // Without this the file reads back correctly and is *validated* against
        // whatever the reader assumed, so geometry legitimate at one scale
        // could be refused at another with nothing to say why.
        for tolerances in [
            Tolerances::millimetres(),
            Tolerances::metres(),
            Tolerances::inches(),
        ] {
            let mut model = Model::with_tolerances(tolerances);
            let solid = make_box(&mut model, Frame::WORLD, (1.0, 2.0, 3.0), tolerances)
                .unwrap()
                .shape;
            let text = write(
                &model,
                std::slice::from_ref(&solid),
                WriteOptions::default(),
            )
            .unwrap();

            let (restored, restored_roots) = read(&text).unwrap();
            assert_eq!(
                restored.tolerances().scale(),
                tolerances.scale(),
                "the scale changed"
            );
            assert_eq!(
                restored.tolerances().confusion(),
                tolerances.confusion(),
                "and so did what counts as the same point"
            );
            // And writing it again gives the same bytes, scale included. The
            // roots have to be *this* model's; a second `read` makes a third
            // document, whose handles this one rightly will not accept.
            let again = write(&restored, &restored_roots, WriteOptions::default()).unwrap();
            assert_eq!(text, again);
        }
    }

    #[test]
    fn a_document_that_does_not_say_its_units_is_refused() {
        let mut model = Model::new();
        let solid = make_box(
            &mut model,
            Frame::WORLD,
            (1.0, 1.0, 1.0),
            Tolerances::millimetres(),
        )
        .unwrap()
        .shape;
        let text = write(
            &model,
            std::slice::from_ref(&solid),
            WriteOptions::default(),
        )
        .unwrap();

        let without: String = text
            .lines()
            .filter(|line| !line.starts_with("units "))
            .collect::<Vec<_>>()
            .join("\n");
        let err = read(&without).unwrap_err();
        assert!(
            err.to_string().contains("units"),
            "unexpected message: {err}"
        );

        // And a scale that is not a scale.
        let broken = text.replace("units ", "units -");
        assert!(read(&broken).is_err());
    }
}
