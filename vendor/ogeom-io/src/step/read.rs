//! From parsed exchange structure to a living model.
//!
//! The reader walks every `MANIFOLD_SOLID_BREP` (`BREP_WITH_VOIDS` is one,
//! by subtype) and every `SHELL_BASED_SURFACE_MODEL`, and rebuilds them
//! bottom-up:
//! points, placements, curves and surfaces into geometry; vertices, edges,
//! loops, faces and shells into topology, shared exactly as the file shares
//! them: a vertex referenced by eight edges is one vertex here too, which is
//! what lets a closed shell close. Edge ranges are re-derived on this
//! kernel's own parameterizations from the vertex geometry, because STEP's
//! parameterizations are its own business and carrying them over blind is
//! how off-by-a-period bugs are born.
//!
//! What the reader does not understand it *counts*: every instance never
//! visited lands in the report's skipped table by keyword, and every
//! compromise (a face without a pcurve, a shell that does not close) is a
//! warning with the instance number in it. An import that succeeded with
//! three warnings is a different thing from one that succeeded, and the
//! report is what keeps the difference visible.

use super::parse::{Arg, Instance, Instances};
use ogeom_algo::{
    make_edge_between, make_face_on, make_shell, make_solid, make_vertex, make_wire,
    project_on_curve,
};
use ogeom_core::{FastMap, FastSet};
use ogeom_core::{OgeomResult, Tolerances, ogeom_bail};
use ogeom_geom::Curve2d as _;
use ogeom_geom::Curve3d as _;
use ogeom_geom::Surface as _;
use ogeom_geom::Transformable as _;
use ogeom_geom::{
    BSplineCurve, CircleCurve, ConeSurface, Curve, CylinderSurface, EllipseCurve, ExtrusionSurface,
    HyperbolaCurve, LineCurve, ParabolaCurve, PlanarCurve, PlaneSurface, SphereSurface,
    SurfaceGeometry, TorusSurface,
};
use ogeom_math::{
    Axis, Blend as _, Circle, Cone, Cylinder, Direction, Ellipse, Frame, Hyperbola, KnotVector,
    Matrix3, Parabola, Plane, Point, Sphere, Torus, Transform, Vector, Weighted,
};
use ogeom_topo::{Location, Model, Shape};
use std::collections::BTreeMap;

/// One use a bound makes of an edge: the edge as the loop walks it, the edge
/// as it was built, the file's id for it, and the curve and range it carries.
type BoundUse = (Shape, Shape, u64, Curve, (f64, f64));

/// How a point of space reads on a surface's unbounded directions, `None`
/// where the direction has none to read.
type ChartReading = dyn Fn(Point) -> (Option<f64>, Option<f64>);

/// How far a plane or a cylinder read from a file extends past what anything
/// in the file uses. A face's trim is its wires; the surface's domain is only
/// the parameter window, and this one is generous without being unbounded.
const SURFACE_EXTENT: f64 = 1e5;

/// What an import brought in, and what it left behind.
#[derive(Debug, Default)]
pub struct StepReport {
    /// Millimetres per file unit, as the file's own unit section states it.
    pub scale_mm: f64,
    /// Instance keywords the reader never visited, with counts. Presentation,
    /// product structure and annotation live here by design; geometry landing
    /// here is a gap worth reading about.
    pub skipped: BTreeMap<String, usize>,
    /// Everything that imported less than perfectly, one line each.
    pub warnings: Vec<String>,
    /// The warning flood, counted: one entry per *kind* of imperfection,
    /// with how often it happened, the worst measured value where the kind
    /// measures one, and one exemplar entity id. A file with hundreds of
    /// warnings summarises to a handful of lines a consumer can show;
    /// `warnings` keeps the full prose. Sorted by count, largest first.
    pub summary: Vec<WarningSummary>,
    /// Faces that read without a complete trim: an edge's boundary sat too
    /// far from the surface for any honest pcurve (beyond the one-millimetre
    /// healing cap), so the face will refuse to triangulate. Deduplicated,
    /// in file order: the structured form of the warnings that name them,
    /// carrying the face itself so the instructed follow-up needs no search.
    /// `check` reports the same faces as broken from the model side. A
    /// refused id whose face never finished building has nothing to act on
    /// and stays in the warnings alone.
    pub untrimmed_faces: Vec<UntrimmedFace>,
}

/// One kind of imperfect import, counted rather than repeated.
#[derive(Debug, Clone)]
pub struct WarningSummary {
    /// The kind, stable across runs: `"vertex-miss"`, `"boundary-slop"`,
    /// `"fit-short"`, `"untrimmed"`.
    pub kind: &'static str,
    /// How many times it happened.
    pub count: usize,
    /// The worst measured value among them: a distance, for every kind
    /// that measures one; zero where none applies.
    pub worst: f64,
    /// One entity id to look at first.
    pub exemplar: u64,
}

/// A face the reader could not trim, with the shape to act on.
#[derive(Debug, Clone)]
pub struct UntrimmedFace {
    /// The file's id for the face: the same id the warnings name.
    pub entity: u64,
    /// The face as built; hand it to `ogeom_heal::fix_face_pcurves` with
    /// the cap the situation deserves.
    pub face: Shape,
}

/// A read exchange file: the model, the bodies found, and the report.
#[derive(Debug)]
pub struct StepImport {
    /// The document everything was built into: the model, plus the file's
    /// product structure, names and colours.
    pub document: ogeom_doc::Document,
    /// One shape per `MANIFOLD_SOLID_BREP` (`BREP_WITH_VOIDS` included,
    /// its cavities among its shells), in file order.
    pub solids: Vec<Shape>,
    /// One shape per `SHELL_BASED_SURFACE_MODEL`, in file order: its shell,
    /// or a compound of its shells when it names several.
    ///
    /// A surface body is what a modeller exports for a part built from faces
    /// rather than from a solid. It stays a shell here even when it happens
    /// to close: the file did not call it a solid, and the document carries
    /// it under its product exactly as the file placed it.
    pub shells: Vec<Shape>,
    /// What happened along the way.
    pub report: StepReport,
}

/// Read a STEP exchange file's B-rep content.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) if the file does
/// not parse, contains no solid, or a solid's structure is broken in a way
/// topology cannot represent. Faces the reader cannot complete become
/// warnings, not errors; the report says exactly what was compromised.
pub fn read_step(text: &str, tol: Tolerances) -> OgeomResult<StepImport> {
    let (_, instances) = super::parse::parse_instances(text)?;
    let mut reader = Reader {
        instances: &instances,
        model: Model::new(),
        report: StepReport {
            scale_mm: 1.0,
            ..StepReport::default()
        },
        angle_scale: 1.0,
        visited: Visited::for_instances(&instances),
        vertices: FastMap::default(),
        edges: FastMap::default(),
        faces: FastMap::default(),
        callout_index: FastMap::default(),
        pcurves: FastMap::default(),
        pieces: FastMap::default(),
        band_faces: FastSet::default(),
        pole_vertices: Vec::new(),
        piece_key: u64::MAX,
        untrimmed_ids: Vec::new(),
        tallies: FastMap::default(),
        cdsr_of_nauo: None,
        properties_of_definition: None,
        sdrs_of_property: None,
        depth: 0,
        tol,
    };
    reader.report.scale_mm = reader.unit_scale();
    reader.angle_scale = reader.angle_unit_scale();

    let mut solids = Vec::new();
    let mut shells = Vec::new();
    let mut by_item: FastMap<u64, Shape> = FastMap::default();
    // Solids and surface models, one walk in file order; a body is a body
    // to the progress bar whichever kind it is.
    let mut ids: Vec<(u64, bool)> = instances
        .iter()
        .filter_map(|(id, inst)| {
            if inst.part("MANIFOLD_SOLID_BREP").is_some() || inst.part("BREP_WITH_VOIDS").is_some()
            {
                Some((id, true))
            } else if inst.part("SHELL_BASED_SURFACE_MODEL").is_some() {
                Some((id, false))
            } else {
                None
            }
        })
        .collect();
    ids.sort_unstable();
    let total = ids.len() as u64;
    for (done, (id, is_solid)) in ids.into_iter().enumerate() {
        ogeom_core::progress::checkpoint()?;
        ogeom_core::progress::stage_at("step: solid", done as u64 + 1, total);
        if is_solid {
            let solid = reader.solid(id)?;
            by_item.insert(id, solid.clone());
            solids.push(solid);
        } else {
            let shell = reader.surface_model(id)?;
            by_item.insert(id, shell.clone());
            shells.push(shell);
        }
    }
    // Every widening the reader made (an edge's tolerance recording how far
    // its pcurves sit from its curve) reaches the vertices that bound it,
    // and a face's reaches its edges: the containment rule the checker holds
    // a solid to, kept by the reader that built it.
    for body in solids.iter().chain(shells.iter()) {
        ogeom_algo::restore_containment(&mut reader.model, body)?;
    }
    // The tallies fold into the summary, largest first, ties by kind so the
    // order is the file's and not the map's.
    {
        let mut entries: Vec<WarningSummary> = reader
            .tallies
            .drain()
            .map(|(kind, (count, worst, exemplar))| WarningSummary {
                kind,
                count,
                worst,
                exemplar,
            })
            .collect();
        entries.sort_by(|a, b| b.count.cmp(&a.count).then(a.kind.cmp(b.kind)));
        reader.report.summary = entries;
    }
    // The noted refusals resolve to the faces themselves, once they
    // exist: the same cache the shells were assembled from answers by the
    // very id the warnings name.
    for id in std::mem::take(&mut reader.untrimmed_ids) {
        if let Some(face) = reader.faces.get(&id) {
            reader.report.untrimmed_faces.push(UntrimmedFace {
                entity: id,
                face: face.clone(),
            });
        }
    }
    if solids.is_empty() && shells.is_empty() {
        ogeom_bail!(
            Construction,
            "the exchange file contains no MANIFOLD_SOLID_BREP, \
             BREP_WITH_VOIDS or SHELL_BASED_SURFACE_MODEL to read"
        );
    }
    let document = reader.document(&by_item, &solids, &shells)?;

    // Everything never visited, counted by its leading keyword.
    for (id, instance) in instances.iter() {
        if !reader.visited.seen(id) {
            *reader
                .report
                .skipped
                .entry(instance.keyword().to_owned())
                .or_default() += 1;
        }
    }

    Ok(StepImport {
        document,
        solids,
        shells,
        report: reader.report,
    })
}

/// An edge as built: the shape, its curve and range, and whether STEP's edge
/// direction runs against the curve.
type BuiltEdge = (Shape, Curve, (f64, f64), bool);

/// A pcurve derived ahead of the face that needs it.
///
/// Deriving one is a pure function of a curve, its range and a surface (no
/// model, no order), and it is most of the time spent building solids. So
/// it is done for a whole solid at once, off the walk that attaches it.
enum PreparedPcurve {
    /// The file stated it, and it stands `off` from the curve once lifted,
    /// within what the edge's own tolerance allows.
    Stated { curve: PlanarCurve, off: f64 },
    /// The projection had a closed form, standing `off` from the curve
    /// once lifted.
    Exact { curve: PlanarCurve, off: f64 },
    /// It did not, and this is the fit, with what the fit cost and how far
    /// it stands from the curve once lifted.
    Fitted {
        curve: PlanarCurve,
        error: f64,
        met: bool,
        worst_off: f64,
        off: f64,
        warning: Option<String>,
    },
    /// Neither worked, for this reason; the face gets the warning.
    Refused(String),
}

struct Reader<'a> {
    instances: &'a Instances,
    model: Model,
    report: StepReport,
    /// Radians per file angle unit; degrees are common.
    angle_scale: f64,
    /// Which instances the reader has looked at.
    visited: Visited,
    vertices: FastMap<u64, Shape>,
    edges: FastMap<u64, BuiltEdge>,
    faces: FastMap<u64, Shape>,
    /// STEP id → index into the callouts just built, for the views pass.
    callout_index: FastMap<u64, usize>,
    /// `(face, edge)` → the pcurve already derived for it, from the parallel
    /// pass at the head of each solid.
    pcurves: FastMap<(u64, u64), PreparedPcurve>,
    /// Edge → the pieces it was cut into where its curve runs through a
    /// pole of a face it bounds, each under a key of its own that stands in
    /// for the edge's id wherever a face's bound names it.
    pieces: FastMap<u64, Vec<(u64, BuiltEdge)>>,
    /// Faces bounded by two whole circles of their surface whose rims share
    /// a vertex column only because one rim was split on the other's: each
    /// builds as a band of rings through its common column.
    band_faces: FastSet<u64>,
    /// The vertices made at poles, shared by every piece that meets one.
    pole_vertices: Vec<Shape>,
    /// The last key handed to a piece; keys count down from the top of the
    /// range, clear of every file id.
    piece_key: u64,
    /// Faces noted untrimmed, by file id; resolved to shapes once the read
    /// is far enough along for the shapes to exist.
    untrimmed_ids: Vec<u64>,
    /// kind → (count, worst, exemplar), folded into the report's summary.
    tallies: FastMap<&'static str, (usize, f64, u64)>,
    /// Usage → its `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION`, built once,
    /// so an assembly edge is a lookup rather than a scan of the whole
    /// exchange, which is O(usages × entities).
    cdsr_of_nauo: Option<FastMap<u64, u64>>,
    /// Definition → its `PROPERTY_DEFINITION`s, and property → its
    /// `SHAPE_DEFINITION_REPRESENTATION`s, built together once, so a datum
    /// target's lookup is not a scan of the whole exchange per property
    /// *per target*. Each list ascends by id, so the first entry that
    /// answers is the lowest id, the one a scan in file order reaches first.
    properties_of_definition: Option<FastMap<u64, Vec<u64>>>,
    sdrs_of_property: Option<FastMap<u64, Vec<u64>>>,
    /// How many geometry or shell builders are open on the stack. Entities
    /// refer to entities of their own kind (an offset surface to its basis,
    /// an oriented shell to its shell), so a file that refers in a circle
    /// would otherwise recurse until the stack runs out.
    depth: u32,
    tol: Tolerances,
}

/// How deep entities of one kind may refer to one another. Real files nest
/// a handful of levels (a trimmed curve of an offset of a replica); a chain
/// this long is a cycle or a hostile file.
const MOST_NESTING: u32 = 64;

impl<'a> Reader<'a> {
    /// Run a builder one level deeper, refusing past [`MOST_NESTING`].
    fn nested<T>(&mut self, build: impl FnOnce(&mut Self) -> OgeomResult<T>) -> OgeomResult<T> {
        if self.depth >= MOST_NESTING {
            ogeom_bail!(
                Construction,
                "entities refer to one another more than {MOST_NESTING} deep; \
                 the file refers in a circle"
            );
        }
        self.depth += 1;
        let built = build(self);
        self.depth -= 1;
        built
    }

    /// The instance behind an id. Borrowed from the exchange, not from the
    /// reader, so its arguments can be read while the reader builds.
    fn instance(&mut self, id: u64) -> OgeomResult<&'a Instance> {
        self.visited.mark(id);
        let instances = self.instances;
        instances.get(id).ok_or_else(|| {
            ogeom_core::ogeom_err!(
                Construction,
                "the file references #{id}, which does not exist"
            )
        })
    }

    /// A face's arguments (name, bounds, surface, sense), whether the file
    /// wrote it as the `ADVANCED_FACE` every modern writer uses or as the
    /// plain `FACE_SURFACE` it specialises, which carries the same four.
    fn face_args(&mut self, id: u64) -> OgeomResult<&'a [Arg]> {
        let instance = self.instance(id)?;
        if let Some(args) = instance
            .part("ADVANCED_FACE")
            .or_else(|| instance.part("FACE_SURFACE"))
        {
            return Ok(args);
        }
        ogeom_bail!(
            Construction,
            "#{id} is {}, where a face (ADVANCED_FACE or FACE_SURFACE) was needed",
            instance.keyword()
        );
    }

    fn args(&mut self, id: u64, keyword: &str) -> OgeomResult<&'a [Arg]> {
        let instance = self.instance(id)?;
        let Some(args) = instance.part(keyword) else {
            ogeom_bail!(
                Construction,
                "#{id} is {}, where {keyword} was needed",
                instance.keyword()
            );
        };
        Ok(args)
    }

    // --- units ---------------------------------------------------------------

    /// The unit instances the representation context actually assigns.
    ///
    /// A file may carry both a radian and a degree (definition and
    /// conversion), and which applies is not a matter of existence but of
    /// assignment: `GLOBAL_UNIT_ASSIGNED_CONTEXT` lists the ones in force.
    fn assigned_units(&self) -> Vec<u64> {
        // A file may carry several unit contexts (inches for the model,
        // millimetres for its annotation sheet), and the shapes' own
        // representations name the one their coordinates mean. That context
        // goes first; the rest follow in entity order, so the answer never
        // depends on how a map happens to iterate.
        let mut cited: Vec<u64> = self
            .instances
            .values()
            .filter(|inst| {
                inst.parts
                    .iter()
                    .any(|(k, _)| k.ends_with("SHAPE_REPRESENTATION"))
            })
            .filter_map(|inst| {
                inst.parts
                    .iter()
                    .find(|(k, _)| k.ends_with("SHAPE_REPRESENTATION"))
                    .and_then(|(_, args)| args.last())
                    .and_then(Arg::reference)
            })
            .collect();
        cited.sort_unstable();
        cited.dedup();

        let mut contexts: Vec<u64> = self
            .instances
            .iter()
            .filter(|(_, inst)| inst.part("GLOBAL_UNIT_ASSIGNED_CONTEXT").is_some())
            .map(|(id, _)| id)
            .collect();
        contexts.sort_unstable();
        contexts.sort_by_cached_key(|id| !cited.contains(id));

        let mut out = Vec::new();
        for id in contexts {
            if let Some(instance) = self.instances.get(id)
                && let Some(args) = instance.part("GLOBAL_UNIT_ASSIGNED_CONTEXT")
            {
                for arg in args.iter().filter_map(Arg::list).flatten() {
                    if let Some(r) = arg.reference() {
                        out.push(r);
                    }
                }
            }
        }
        out
    }

    /// Millimetres per file length unit, from the units the context assigns.
    fn unit_scale(&mut self) -> f64 {
        let assigned = self.assigned_units();
        for id in assigned {
            let Some(instance) = self.instances.get(id) else {
                continue;
            };
            let instance = instance.clone();
            if instance.part("LENGTH_UNIT").is_none() {
                continue;
            }
            if let Some(args) = instance.part("SI_UNIT") {
                return match args.first() {
                    Some(Arg::Enum(prefix)) => match prefix.as_str() {
                        "MILLI" => 1.0,
                        "CENTI" => 10.0,
                        "DECI" => 100.0,
                        "KILO" => 1e6,
                        "MICRO" => 1e-3,
                        other => {
                            self.report
                                .warnings
                                .push(format!("#{id}: unknown SI prefix {other}; taking metres"));
                            1000.0
                        }
                    },
                    _ => 1000.0,
                };
            }
            if let Some(args) = instance.part("CONVERSION_BASED_UNIT")
                && let Some(measure) = args.get(1).and_then(Arg::reference)
                && let Some(inner) = self.instances.get(measure)
            {
                let factor = inner
                    .parts
                    .iter()
                    .flat_map(|(_, a)| a.iter())
                    .find_map(|a| match a {
                        Arg::Typed(typed) if typed.0 == "LENGTH_MEASURE" => {
                            typed.1.first().and_then(Arg::number)
                        }
                        _ => None,
                    });
                let base = inner
                    .parts
                    .iter()
                    .flat_map(|(_, a)| a.iter())
                    .find_map(Arg::reference)
                    .and_then(|b| self.instances.get(b))
                    .and_then(|u| u.part("SI_UNIT"))
                    .map_or(1000.0, |args| match args.first() {
                        Some(Arg::Enum(p)) if p == "MILLI" => 1.0,
                        Some(Arg::Enum(p)) if p == "CENTI" => 10.0,
                        _ => 1000.0,
                    });
                if let Some(f) = factor {
                    return f * base;
                }
            }
        }
        self.report
            .warnings
            .push("no length unit found; taking millimetres".to_owned());
        1.0
    }

    /// Radians per file angle unit: the same dance as length, for the files
    /// that measure their cones in degrees.
    fn angle_unit_scale(&mut self) -> f64 {
        let assigned = self.assigned_units();
        for id in assigned {
            let Some(instance) = self.instances.get(id) else {
                continue;
            };
            let instance = instance.clone();
            if instance.part("PLANE_ANGLE_UNIT").is_none() {
                continue;
            }
            if instance.part("SI_UNIT").is_some() {
                return 1.0;
            }
            if let Some(args) = instance.part("CONVERSION_BASED_UNIT")
                && let Some(measure) = args.get(1).and_then(Arg::reference)
                && let Some(inner) = self.instances.get(measure)
            {
                let factor = inner
                    .parts
                    .iter()
                    .flat_map(|(_, a)| a.iter())
                    .find_map(|a| match a {
                        Arg::Typed(typed) if typed.0 == "PLANE_ANGLE_MEASURE" => {
                            typed.1.first().and_then(Arg::number)
                        }
                        _ => None,
                    });
                if let Some(f) = factor {
                    return f;
                }
            }
        }
        1.0
    }

    // --- geometry ------------------------------------------------------------

    fn point(&mut self, id: u64) -> OgeomResult<Point> {
        let args = self.args(id, "CARTESIAN_POINT")?;
        let Some(coords) = args.get(1).and_then(Arg::list) else {
            ogeom_bail!(Construction, "#{id}: a point without coordinates");
        };
        let scale = self.report.scale_mm;
        let value = |i: usize| coords.get(i).and_then(Arg::number).unwrap_or(0.0) * scale;
        Ok(Point::new(value(0), value(1), value(2)))
    }

    fn direction(&mut self, id: u64) -> OgeomResult<Direction> {
        let args = self.args(id, "DIRECTION")?;
        let Some(coords) = args.get(1).and_then(Arg::list) else {
            ogeom_bail!(Construction, "#{id}: a direction without components");
        };
        let value = |i: usize| coords.get(i).and_then(Arg::number).unwrap_or(0.0);
        Direction::new(Vector::new(value(0), value(1), value(2)), self.tol)
    }

    /// An `AXIS2_PLACEMENT_3D` as a frame, with the standard's defaults for
    /// what the file leaves out.
    fn frame(&mut self, id: u64) -> OgeomResult<Frame> {
        let args = self.args(id, "AXIS2_PLACEMENT_3D")?;
        let origin = args
            .get(1)
            .and_then(Arg::reference)
            .map(|r| self.point(r))
            .transpose()?
            .unwrap_or(Point::ORIGIN);
        let z = args
            .get(2)
            .and_then(Arg::reference)
            .map(|r| self.direction(r))
            .transpose()?
            .unwrap_or(Direction::Z);
        let x = match args.get(3).and_then(Arg::reference) {
            Some(r) => self.direction(r)?,
            None => Direction::from_cross(z.vector(), Vector::new(0.31, 0.52, 0.8), self.tol)?,
        };
        Frame::new(origin, z, x, self.tol)
    }

    fn surface(&mut self, id: u64) -> OgeomResult<Option<SurfaceGeometry>> {
        self.nested(|reader| reader.surface_at(id))
    }

    fn surface_at(&mut self, id: u64) -> OgeomResult<Option<SurfaceGeometry>> {
        let (keyword, args) = {
            let instance = self.instance(id)?;
            (
                instance.keyword(),
                instance
                    .parts
                    .first()
                    .map_or(&[][..], |(_, a)| a.as_slice()),
            )
        };
        let scale = self.report.scale_mm;
        let radius_arg = |args: &[Arg], i: usize| args.get(i).and_then(Arg::number);
        // B-spline surfaces arrive two ways: a simple instance with every
        // attribute in one list, or a complex instance whose parts each
        // carry their own slice, the rational form always the latter.
        {
            let (base, knots_part, weights) = {
                let instance = self.instance(id)?;
                (
                    instance.part("B_SPLINE_SURFACE"),
                    instance.part("B_SPLINE_SURFACE_WITH_KNOTS"),
                    instance.part("RATIONAL_B_SPLINE_SURFACE"),
                )
            };
            if let Some(kp) = knots_part {
                let (degrees, grid_arg, mults_knots) = if let Some(base) = base {
                    (
                        (base.first().cloned(), base.get(1).cloned()),
                        base.get(2).cloned(),
                        (
                            kp.first().cloned(),
                            kp.get(1).cloned(),
                            kp.get(2).cloned(),
                            kp.get(3).cloned(),
                        ),
                    )
                } else {
                    (
                        (kp.get(1).cloned(), kp.get(2).cloned()),
                        kp.get(3).cloned(),
                        (
                            kp.get(8).cloned(),
                            kp.get(9).cloned(),
                            kp.get(10).cloned(),
                            kp.get(11).cloned(),
                        ),
                    )
                };
                return self
                    .bspline_surface(id, degrees, grid_arg, mults_knots, weights, None)
                    .map(Some);
            }
        }
        // A complex instance whose knots are implied by its form, or a
        // simple one of those forms.
        {
            let (base, weights, form) = {
                let instance = self.instance(id)?;
                (
                    instance.part("B_SPLINE_SURFACE"),
                    instance.part("RATIONAL_B_SPLINE_SURFACE"),
                    ["BEZIER_SURFACE", "UNIFORM_SURFACE", "QUASI_UNIFORM_SURFACE"]
                        .into_iter()
                        .find(|k| instance.part(k).is_some()),
                )
            };
            if let (Some(base), Some(form)) = (base, form) {
                return self
                    .bspline_surface(
                        id,
                        (base.first().cloned(), base.get(1).cloned()),
                        base.get(2).cloned(),
                        (None, None, None, None),
                        weights,
                        Some(form),
                    )
                    .map(Some);
            }
            if let Some(form) = ["BEZIER_SURFACE", "UNIFORM_SURFACE", "QUASI_UNIFORM_SURFACE"]
                .into_iter()
                .find(|k| *k == keyword)
            {
                return self
                    .bspline_surface(
                        id,
                        (args.get(1).cloned(), args.get(2).cloned()),
                        args.get(3).cloned(),
                        (None, None, None, None),
                        None,
                        Some(form),
                    )
                    .map(Some);
            }
        }
        let out = match keyword {
            "PLANE" => {
                let frame = self.frame(ref_at(args, 1))?;
                Some(
                    PlaneSurface::over(
                        Plane::new(frame),
                        (-SURFACE_EXTENT, SURFACE_EXTENT),
                        (-SURFACE_EXTENT, SURFACE_EXTENT),
                    )?
                    .into(),
                )
            }
            "CYLINDRICAL_SURFACE" => {
                let frame = self.frame(ref_at(args, 1))?;
                let radius = radius_arg(args, 2).unwrap_or(0.0) * scale;
                Some(
                    CylinderSurface::new(
                        Cylinder::new(frame, radius, self.tol)?,
                        (-SURFACE_EXTENT, SURFACE_EXTENT),
                    )?
                    .into(),
                )
            }
            "CONICAL_SURFACE" => {
                let frame = self.frame(ref_at(args, 1))?;
                let radius = radius_arg(args, 2).unwrap_or(0.0) * scale;
                let angle = radius_arg(args, 3).unwrap_or(0.0) * self.angle_scale;
                Some(
                    ConeSurface::new(
                        Cone::new(frame, radius, angle, self.tol)?,
                        (-SURFACE_EXTENT, SURFACE_EXTENT),
                    )?
                    .into(),
                )
            }
            "SPHERICAL_SURFACE" => {
                let frame = self.frame(ref_at(args, 1))?;
                let radius = radius_arg(args, 2).unwrap_or(0.0) * scale;
                Some(SphereSurface::new(Sphere::new(frame, radius, self.tol)?).into())
            }
            "TOROIDAL_SURFACE" => {
                let frame = self.frame(ref_at(args, 1))?;
                let major = radius_arg(args, 2).unwrap_or(0.0) * scale;
                let minor = radius_arg(args, 3).unwrap_or(0.0) * scale;
                Some(TorusSurface::new(Torus::new(frame, major, minor, self.tol)?).into())
            }
            "SURFACE_OF_LINEAR_EXTRUSION" => {
                // A curve swept along a vector, unbounded either way. A
                // file writes a drum's wall this way as often as it writes a
                // cylinder (a circle swept along its own axis) and a wall
                // as a line swept: those are read as the cylinder and the
                // plane they are, exact and known everywhere downstream.
                // Anything else sweeps as itself, over a window the face's
                // own edges then widen to fit.
                let Some(curve) = self.curve(ref_at(args, 1))? else {
                    self.report.warnings.push(format!(
                        "#{id}: an extrusion's swept curve is not read; its face is skipped"
                    ));
                    return Ok(None);
                };
                let vector = self.args(ref_at(args, 2), "VECTOR")?;
                let direction = self.direction(vector[1].reference().unwrap_or(0))?;
                // Parallel to the file's own precision in directions: a
                // writer states an axis to nine digits, a whisker off the
                // sweep it meant to be exactly along.
                let parallel =
                    |axis: Direction| axis.vector().cross(direction.vector()).magnitude() <= 1e-6;
                match &curve {
                    Curve::Circle(c) if parallel(c.circle().frame().z()) => Some(
                        CylinderSurface::new(
                            Cylinder::new(c.circle().frame(), c.circle().radius(), self.tol)?,
                            (-SURFACE_EXTENT, SURFACE_EXTENT),
                        )?
                        .into(),
                    ),
                    Curve::Line(l) if !parallel(l.axis().direction) => {
                        let axis = l.axis();
                        let normal = Direction::from_cross(
                            axis.direction.vector(),
                            direction.vector(),
                            self.tol,
                        )?;
                        let frame = Frame::new(axis.location, normal, axis.direction, self.tol)?;
                        Some(
                            PlaneSurface::over(
                                Plane::new(frame),
                                (-SURFACE_EXTENT, SURFACE_EXTENT),
                                (-SURFACE_EXTENT, SURFACE_EXTENT),
                            )?
                            .into(),
                        )
                    }
                    _ => Some(
                        ExtrusionSurface::over(
                            curve,
                            direction,
                            (-SURFACE_EXTENT, SURFACE_EXTENT),
                        )?
                        .into(),
                    ),
                }
            }
            "SURFACE_OF_REVOLUTION" => {
                let Some(curve) = self.curve(ref_at(args, 1))? else {
                    self.report.warnings.push(format!(
                        "#{id}: a revolution's swept curve is not read; its face is skipped"
                    ));
                    return Ok(None);
                };
                let placement = self.args(ref_at(args, 2), "AXIS1_PLACEMENT")?;
                let location = self.point(placement[1].reference().unwrap_or(0))?;
                let direction = match placement.get(2).and_then(Arg::reference) {
                    Some(r) => self.direction(r)?,
                    None => Direction::Z,
                };
                Some(
                    ogeom_geom::RevolutionSurface::new(
                        curve,
                        Axis {
                            location,
                            direction,
                        },
                        core::f64::consts::TAU,
                    )?
                    .into(),
                )
            }
            "OFFSET_SURFACE" => {
                let Some(basis) = self.surface(ref_at(args, 1))? else {
                    return Ok(None);
                };
                let distance = args.get(2).and_then(Arg::number).unwrap_or(0.0) * scale;
                if distance == 0.0 {
                    return Ok(Some(basis));
                }
                // An analytic basis offsets to the analytic surface it is,
                // which every downstream path speaks exactly.
                let offset = ogeom_geom::OffsetSurface::new(basis, distance)?;
                Some(match offset.analytic(self.tol)? {
                    Some(analytic) => analytic,
                    None => SurfaceGeometry::Offset(Box::new(offset)),
                })
            }
            // A face's own edges bound it; the window these name is the
            // basis's, restated.
            "RECTANGULAR_TRIMMED_SURFACE" | "CURVE_BOUNDED_SURFACE" => {
                self.surface(ref_at(args, 1))?
            }
            "DEGENERATE_TOROIDAL_SURFACE" => {
                let frame = self.frame(ref_at(args, 1))?;
                let major = radius_arg(args, 2).unwrap_or(0.0) * scale;
                let minor = radius_arg(args, 3).unwrap_or(0.0) * scale;
                Some(TorusSurface::new(Torus::new(frame, major, minor, self.tol)?).into())
            }
            "RECTANGULAR_COMPOSITE_SURFACE" => Some(self.composite_surface(id, args)?),
            "SURFACE_REPLICA" => {
                let Some(parent) = self.surface(ref_at(args, 1))? else {
                    return Ok(None);
                };
                let motion = self.transformation_operator(ref_at(args, 2))?;
                Some(parent.transformed(&motion, self.tol)?)
            }
            other => {
                self.report.warnings.push(format!(
                    "#{id}: surface kind {other} is not read yet; its face is skipped"
                ));
                None
            }
        };
        Ok(out)
    }

    /// Expand STEP's multiplicity-compressed knots.
    fn expand_knots(mults: Option<Arg>, knots: Option<Arg>) -> OgeomResult<Vec<f64>> {
        let (Some(Arg::List(mults)), Some(Arg::List(knots))) = (mults, knots) else {
            return Ok(Vec::new());
        };
        expanded_knots(&mults, &knots)
    }

    #[allow(clippy::type_complexity)]
    fn bspline_surface(
        &mut self,
        id: u64,
        degrees: (Option<Arg>, Option<Arg>),
        grid_arg: Option<Arg>,
        mults_knots: (Option<Arg>, Option<Arg>, Option<Arg>, Option<Arg>),
        weights: Option<&[Arg]>,
        form: Option<&str>,
    ) -> OgeomResult<SurfaceGeometry> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (u_degree, v_degree) = (
            file_degree(degrees.0.as_ref(), 1.0)?,
            file_degree(degrees.1.as_ref(), 1.0)?,
        );
        let Some(Arg::List(rows)) = grid_arg else {
            ogeom_bail!(
                Construction,
                "#{id}: a b-spline surface without control points"
            );
        };
        let mut points = Vec::new();
        let (mut u_count, mut v_count) = (0, 0);
        for row in &rows {
            let Some(cells) = row.list() else { continue };
            u_count += 1;
            v_count = cells.len();
            for cell in cells {
                points.push(self.point(cell.reference().unwrap_or(0))?);
            }
        }
        let (u_raw, v_raw) = match form {
            Some(form) => (
                implied_knots(form, u_degree, u_count),
                implied_knots(form, v_degree, v_count),
            ),
            None => (
                Self::expand_knots(mults_knots.0, mults_knots.2)?,
                Self::expand_knots(mults_knots.1, mults_knots.3)?,
            ),
        };
        let u_knots = KnotVector::new(u_raw, u_degree)?;
        let v_knots = KnotVector::new(v_raw, v_degree)?;
        let surface = if let Some(weights) = weights {
            let flat: Vec<f64> = weights
                .first()
                .and_then(Arg::list)
                .unwrap_or(&[])
                .iter()
                .filter_map(Arg::list)
                .flatten()
                .filter_map(Arg::number)
                .collect();
            let weighted: Vec<ogeom_math::Weighted<Point>> = points
                .iter()
                .zip(flat.iter().chain(std::iter::repeat(&1.0)))
                .map(|(p, w)| ogeom_math::Weighted::new(*p, *w, self.tol))
                .collect::<OgeomResult<_>>()?;
            ogeom_geom::BSplineSurface::rational(
                u_knots,
                v_knots,
                ogeom_math::ControlGrid::new(weighted, u_count, v_count)?,
            )?
        } else {
            ogeom_geom::BSplineSurface::new(
                u_knots,
                v_knots,
                &ogeom_math::ControlGrid::new(points, u_count, v_count)?,
                self.tol,
            )?
        };
        Ok(surface.into())
    }

    fn curve(&mut self, id: u64) -> OgeomResult<Option<Curve>> {
        self.nested(|reader| reader.curve_at(id))
    }

    fn curve_at(&mut self, id: u64) -> OgeomResult<Option<Curve>> {
        let (keyword, args) = {
            let instance = self.instance(id)?;
            (
                instance.keyword(),
                instance
                    .parts
                    .first()
                    .map_or(&[][..], |(_, a)| a.as_slice()),
            )
        };
        let scale = self.report.scale_mm;
        {
            let (base, kp, weights) = {
                let instance = self.instance(id)?;
                (
                    instance.part("B_SPLINE_CURVE"),
                    instance.part("B_SPLINE_CURVE_WITH_KNOTS"),
                    instance.part("RATIONAL_B_SPLINE_CURVE"),
                )
            };
            // A complex instance: the base part carries degree and control
            // points; the knots come from the knots part or are implied by
            // the form (Bézier, uniform, quasi-uniform); weights from the
            // rational part where there is one.
            let form = {
                let instance = self.instance(id)?;
                ["BEZIER_CURVE", "UNIFORM_CURVE", "QUASI_UNIFORM_CURVE"]
                    .into_iter()
                    .find(|k| instance.part(k).is_some())
            };
            if let Some(base) = base
                && (kp.is_some() || form.is_some())
            {
                let degree = file_degree(base.first(), 1.0)?;
                let control: Vec<Point> = base
                    .get(1)
                    .and_then(Arg::list)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Arg::reference)
                    .map(|r| self.point(r))
                    .collect::<OgeomResult<_>>()?;
                let knots = match (&kp, form) {
                    (Some(kp), _) => Self::expand_knots(kp.first().cloned(), kp.get(1).cloned())?,
                    (None, Some(form)) => implied_knots(form, degree, control.len()),
                    (None, None) => unreachable!("guarded above"),
                };
                let knots = KnotVector::new(knots, degree)?;
                let flat: Vec<f64> = weights
                    .as_ref()
                    .and_then(|w| w.first())
                    .and_then(Arg::list)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Arg::number)
                    .collect();
                let weighted: Vec<ogeom_math::Weighted<Point>> = control
                    .iter()
                    .zip(flat.iter().chain(std::iter::repeat(&1.0)))
                    .map(|(p, w)| ogeom_math::Weighted::new(*p, *w, self.tol))
                    .collect::<OgeomResult<_>>()?;
                return Ok(Some(BSplineCurve::rational(knots, weighted)?.into()));
            }
        }
        let out: Option<Curve> = match keyword {
            "LINE" => {
                let through = self.point(ref_at(args, 1))?;
                // The vector's magnitude scales STEP's parameter; ranges here
                // are re-derived from vertex geometry, so only the direction
                // matters.
                let vector = self.args(ref_at(args, 2), "VECTOR")?;
                let direction = self.direction(vector[1].reference().unwrap_or(0))?;
                Some(
                    LineCurve::new(Axis {
                        location: through,
                        direction,
                    })
                    .into(),
                )
            }
            "CIRCLE" => {
                let frame = self.frame(ref_at(args, 1))?;
                let radius = args.get(2).and_then(Arg::number).unwrap_or(0.0) * scale;
                Some(CircleCurve::new(Circle::new(frame, radius, self.tol)?).into())
            }
            "ELLIPSE" => {
                let frame = self.frame(ref_at(args, 1))?;
                let a = args.get(2).and_then(Arg::number).unwrap_or(0.0) * scale;
                let b = args.get(3).and_then(Arg::number).unwrap_or(0.0) * scale;
                Some(EllipseCurve::new(Ellipse::new(frame, a, b, self.tol)?).into())
            }
            "B_SPLINE_CURVE_WITH_KNOTS" => {
                let degree = file_degree(args.get(1), 0.0)?;
                let control: Vec<Point> = args
                    .get(2)
                    .and_then(Arg::list)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Arg::reference)
                    .map(|r| self.point(r))
                    .collect::<OgeomResult<_>>()?;
                let mults = args.get(6).and_then(Arg::list).unwrap_or(&[]).to_vec();
                let knots = args.get(7).and_then(Arg::list).unwrap_or(&[]).to_vec();
                let expanded = expanded_knots(&mults, &knots)?;
                Some(
                    BSplineCurve::new(KnotVector::new(expanded, degree)?, control, self.tol)?
                        .into(),
                )
            }
            "SURFACE_CURVE" | "SEAM_CURVE" | "INTERSECTION_CURVE" => {
                // A curve dressed in its surface associations: what many
                // exporters write for every edge. The 3D curve is the first
                // argument; the pcurves are read where a face asks for them.
                self.curve(args.get(1).and_then(Arg::reference).unwrap_or(0))?
            }
            "BEZIER_CURVE" | "UNIFORM_CURVE" | "QUASI_UNIFORM_CURVE" => {
                let degree = file_degree(args.get(1), 1.0)?;
                let control: Vec<Point> = args
                    .get(2)
                    .and_then(Arg::list)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Arg::reference)
                    .map(|r| self.point(r))
                    .collect::<OgeomResult<_>>()?;
                let knots = implied_knots(keyword, degree, control.len());
                Some(BSplineCurve::new(KnotVector::new(knots, degree)?, control, self.tol)?.into())
            }
            "HYPERBOLA" => {
                let frame = self.frame(ref_at(args, 1))?;
                let a = args.get(2).and_then(Arg::number).unwrap_or(0.0) * scale;
                let b = args.get(3).and_then(Arg::number).unwrap_or(0.0) * scale;
                // The branch's reach in its own parameter: cosh 20 carries a
                // hyperbola far past any part.
                Some(HyperbolaCurve::new(Hyperbola::new(frame, a, b, self.tol)?, 20.0)?.into())
            }
            "PARABOLA" => {
                let frame = self.frame(ref_at(args, 1))?;
                let focal = args.get(2).and_then(Arg::number).unwrap_or(0.0) * scale;
                Some(
                    ParabolaCurve::new(Parabola::new(frame, focal, self.tol)?, SURFACE_EXTENT)?
                        .into(),
                )
            }
            "TRIMMED_CURVE" => {
                // An edge's ends are its vertices; the trims restate them.
                self.curve(args.get(1).and_then(Arg::reference).unwrap_or(0))?
            }
            "OFFSET_CURVE_3D" => {
                let Some(basis) = self.curve(args.get(1).and_then(Arg::reference).unwrap_or(0))?
                else {
                    return Ok(None);
                };
                let distance = args.get(2).and_then(Arg::number).unwrap_or(0.0) * scale;
                let reference =
                    self.direction(args.get(4).and_then(Arg::reference).unwrap_or(0))?;
                Some(Curve::Offset(Box::new(ogeom_geom::OffsetCurve::new(
                    basis, distance, reference,
                )?)))
            }
            "POLYLINE" => {
                let points: Vec<Point> = args
                    .get(1)
                    .and_then(Arg::list)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Arg::reference)
                    .map(|r| self.point(r))
                    .collect::<OgeomResult<_>>()?;
                if points.len() < 2 {
                    ogeom_bail!(Construction, "#{id}: a polyline of fewer than two points");
                }
                // Degree one through the points, a knot at each.
                let mut knots = vec![0.0];
                #[allow(clippy::cast_precision_loss)]
                knots.extend((0..points.len()).map(|i| i as f64));
                #[allow(clippy::cast_precision_loss)]
                knots.push((points.len() - 1) as f64);
                Some(BSplineCurve::new(KnotVector::new(knots, 1)?, points, self.tol)?.into())
            }
            "COMPOSITE_CURVE" => {
                // Its segments, each over its own trim and in its own
                // sense, as splines joined end to start.
                let mut joined: Option<(KnotVector, Vec<ogeom_math::Weighted<Point>>)> = None;
                let segments: Vec<u64> = args
                    .get(1)
                    .and_then(Arg::list)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Arg::reference)
                    .collect();
                for segment in segments {
                    let sargs = self.args(segment, "COMPOSITE_CURVE_SEGMENT")?;
                    let same_sense = !sargs.get(1).is_some_and(|a| a.is_enum("F"));
                    let parent = sargs.get(2).and_then(Arg::reference).unwrap_or(0);
                    let Some((curve, range)) = self.bounded_curve(parent)? else {
                        return Ok(None);
                    };
                    let mut spline = curve.to_bspline_over(range, self.tol)?;
                    if !same_sense {
                        let (knots, control) =
                            ogeom_math::bspline::reverse(spline.knots(), spline.control_points());
                        spline = BSplineCurve::rational(knots, control)?;
                    }
                    joined = Some(match joined {
                        None => (spline.knots().clone(), spline.control_points().to_vec()),
                        Some(held) => join_splines(held, &spline, self.tol)?,
                    });
                }
                let Some((knots, control)) = joined else {
                    ogeom_bail!(Construction, "#{id}: a composite curve with no segments");
                };
                Some(BSplineCurve::rational(knots, control)?.into())
            }
            "CURVE_REPLICA" => {
                let Some(parent) = self.curve(args.get(1).and_then(Arg::reference).unwrap_or(0))?
                else {
                    return Ok(None);
                };
                let motion = self
                    .transformation_operator(args.get(2).and_then(Arg::reference).unwrap_or(0))?;
                Some(parent.transformed(&motion, self.tol)?)
            }
            other => {
                self.report.warnings.push(format!(
                    "#{id}: curve kind {other} is not read yet; its edge is skipped"
                ));
                None
            }
        };
        Ok(out)
    }

    /// A rectangular composite surface: its grid of patches joined into one
    /// B-spline surface, so a face across the grid is a face on one surface
    /// and needs no rebuilding. Each patch in its spline form over its own
    /// bounds, turned where its sense says, raised to the grid's degrees,
    /// its knots unified with its column's and its row's, and joined to its
    /// neighbours on their shared boundary. Neighbours that do not meet
    /// there are refused by name.
    fn composite_surface(&mut self, id: u64, args: &[Arg]) -> OgeomResult<SurfaceGeometry> {
        let rows: Vec<Vec<u64>> = args
            .get(1)
            .and_then(Arg::list)
            .unwrap_or(&[])
            .iter()
            .map(|row| {
                row.list()
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Arg::reference)
                    .collect()
            })
            .collect();
        if rows.is_empty()
            || rows
                .iter()
                .any(|r| r.len() != rows[0].len() || r.is_empty())
        {
            ogeom_bail!(
                Construction,
                "#{id}: a composite surface's grid is not rectangular"
            );
        }
        let mut grid: Vec<Vec<Patch>> = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut out = Vec::with_capacity(row.len());
            for &patch in row {
                let pargs = self.args(patch, "SURFACE_PATCH")?;
                let parent = pargs.get(1).and_then(Arg::reference).unwrap_or(0);
                let u_sense = !pargs.get(4).is_some_and(|a| a.is_enum("F"));
                let v_sense = !pargs.get(5).is_some_and(|a| a.is_enum("F"));
                let spline = self.bounded_surface_spline(parent)?;
                let mut p = Patch::of(&spline)?;
                if !u_sense {
                    p = p.reversed_u();
                }
                if !v_sense {
                    p = p.transposed().reversed_u().transposed();
                }
                out.push(p);
            }
            grid.push(out);
        }
        join_patches(grid, self.tol).map_err(|e| {
            ogeom_core::ogeom_err!(
                Construction,
                "#{id}: a composite surface's patches do not join: {e}"
            )
        })
    }

    /// A composite's patch surface as a B-spline over its own bounds: a
    /// B-spline as it is; a rectangularly trimmed surface's basis over its
    /// window, in the file's reading of the basis's parameters.
    fn bounded_surface_spline(&mut self, id: u64) -> OgeomResult<ogeom_geom::BSplineSurface> {
        let keyword = self.instance(id)?.keyword().to_owned();
        if keyword == "RECTANGULAR_TRIMMED_SURFACE" {
            let args = self.args(id, "RECTANGULAR_TRIMMED_SURFACE")?;
            let basis_id = args.get(1).and_then(Arg::reference).unwrap_or(0);
            let Some(basis) = self.surface(basis_id)? else {
                ogeom_bail!(Construction, "#{id}: a patch's basis is not read");
            };
            let angular = matches!(
                self.instance(basis_id)?.keyword(),
                "CYLINDRICAL_SURFACE"
                    | "CONICAL_SURFACE"
                    | "SPHERICAL_SURFACE"
                    | "TOROIDAL_SURFACE"
            );
            let n = |i: usize| args.get(i).and_then(Arg::number).unwrap_or(0.0);
            let (u_scale, v_scale) = if angular {
                (self.angle_scale, self.report.scale_mm)
            } else {
                (self.report.scale_mm, self.report.scale_mm)
            };
            let (u1, u2) = (n(2) * u_scale, n(3) * u_scale);
            let (v1, v2) = (n(4) * v_scale, n(5) * v_scale);
            let window = ((u1.min(u2), u1.max(u2)), (v1.min(v2), v1.max(v2)));
            let trimmed = ogeom_geom::TrimmedSurface::new(basis, window.0, window.1, self.tol)?;
            return SurfaceGeometry::Trimmed(Box::new(trimmed)).to_bspline(self.tol);
        }
        let Some(surface) = self.surface(id)? else {
            ogeom_bail!(Construction, "#{id}: a patch's surface is not read");
        };
        surface.to_bspline(self.tol)
    }

    /// A curve with the stretch of it an entity bounds: a trimmed curve's
    /// basis between its trims (a point where the file gives one, else its
    /// parameter in the file's own reading of the basis), in its own sense;
    /// any other curve over its own domain, which must be bounded.
    fn bounded_curve(&mut self, id: u64) -> OgeomResult<Option<(Curve, (f64, f64))>> {
        self.nested(|reader| reader.bounded_curve_at(id))
    }

    fn bounded_curve_at(&mut self, id: u64) -> OgeomResult<Option<(Curve, (f64, f64))>> {
        let keyword = self.instance(id)?.keyword().to_owned();
        if keyword != "TRIMMED_CURVE" {
            let Some(curve) = self.curve(id)? else {
                return Ok(None);
            };
            let (lo, hi) = curve.domain();
            if !lo.is_finite() || !hi.is_finite() || hi - lo > SURFACE_EXTENT {
                ogeom_bail!(
                    Construction,
                    "#{id}: an unbounded curve cannot stand as a composite's segment"
                );
            }
            return Ok(Some((curve, (lo, hi))));
        }
        let args = self.args(id, "TRIMMED_CURVE")?;
        let basis_id = args.get(1).and_then(Arg::reference).unwrap_or(0);
        let Some(basis) = self.curve(basis_id)? else {
            return Ok(None);
        };
        let basis_keyword = self.instance(basis_id)?.keyword().to_owned();
        let forward = !args.get(4).is_some_and(|a| a.is_enum("F"));
        let mut ends = Vec::with_capacity(2);
        for trim in [args.get(2), args.get(3)] {
            let items = trim.and_then(Arg::list).unwrap_or(&[]).to_vec();
            let mut at: Option<Point> = None;
            for item in &items {
                if let Some(r) = item.reference() {
                    at = Some(self.point(r)?);
                }
            }
            if at.is_none() {
                for item in &items {
                    if let Arg::Typed(typed) = item
                        && typed.0 == "PARAMETER_VALUE"
                        && let Some(v) = typed.1.first().and_then(Arg::number)
                    {
                        at =
                            Some(self.step_parameter_point(&basis, &basis_keyword, basis_id, v)?);
                    }
                }
            }
            let Some(point) = at else {
                ogeom_bail!(
                    Construction,
                    "#{id}: a trim names neither point nor parameter"
                );
            };
            let t = match self.parameter_of(&basis, point) {
                Some(t) => t,
                None => project_on_curve(&basis, point, 256, self.tol)?.parameter,
            };
            ends.push(t);
        }
        let (mut a, mut b) = (ends[0], ends[1]);
        if !forward {
            core::mem::swap(&mut a, &mut b);
        }
        if basis.is_periodic() && b <= a {
            let (lo, hi) = basis.domain();
            b += hi - lo;
        }
        let range = if a <= b { (a, b) } else { (b, a) };
        let mut curve = basis;
        if !forward || a > b {
            // Travelled backward: the same points, the parameter turned.
            let spline = curve.to_bspline_over(range, self.tol)?;
            let (knots, control) =
                ogeom_math::bspline::reverse(spline.knots(), spline.control_points());
            curve = BSplineCurve::rational(knots, control)?.into();
            let domain = curve.domain();
            return Ok(Some((curve, domain)));
        }
        Ok(Some((curve, range)))
    }

    /// Where the file's parameter `v` stands on a basis curve, read as the
    /// file reads it: a line's by its own vector's length, a conic's as an
    /// angle in the file's angle unit, a spline's as it is.
    fn step_parameter_point(
        &mut self,
        basis: &Curve,
        keyword: &str,
        basis_id: u64,
        v: f64,
    ) -> OgeomResult<Point> {
        match keyword {
            "LINE" => {
                let args = self.args(basis_id, "LINE")?;
                let through = self.point(ref_at(args, 1))?;
                let vector = self.args(ref_at(args, 2), "VECTOR")?;
                let direction = self.direction(vector[1].reference().unwrap_or(0))?;
                let magnitude = vector.get(2).and_then(Arg::number).unwrap_or(1.0);
                Ok(through + direction.vector() * (v * magnitude * self.report.scale_mm))
            }
            "CIRCLE" | "ELLIPSE" => basis.point_at(v * self.angle_scale, self.tol),
            _ => basis.point_at(v, self.tol),
        }
    }

    /// A Cartesian transformation operator as the motion it states: the
    /// axes it names (completed square to one another), its origin, and
    /// its uniform scale.
    fn transformation_operator(&mut self, id: u64) -> OgeomResult<Transform> {
        let args = self.args(id, "CARTESIAN_TRANSFORMATION_OPERATOR_3D")?;
        let direction_at = |this: &mut Self, i: usize| -> OgeomResult<Option<Vector>> {
            match args.get(i).and_then(Arg::reference) {
                Some(r) => Ok(Some(this.direction(r)?.vector())),
                None => Ok(None),
            }
        };
        let (axis1, axis2, axis3) = (
            direction_at(self, 1)?,
            direction_at(self, 2)?,
            direction_at(self, 5)?,
        );
        let origin = self.point(args.get(3).and_then(Arg::reference).unwrap_or(0))?;
        let scale = args.get(4).and_then(Arg::number).unwrap_or(1.0);
        let z = match (axis3, axis1, axis2) {
            (Some(z), _, _) => z,
            (None, Some(x), Some(y)) => x.cross(y),
            _ => Vector::Z,
        };
        let z = Direction::new(z, self.tol)?.vector();
        let x = match axis1 {
            Some(x) => x - z * x.dot(z),
            None => {
                let seed = if z.x.abs() < 0.9 {
                    Vector::X
                } else {
                    Vector::Y
                };
                seed - z * seed.dot(z)
            }
        };
        let x = Direction::new(x, self.tol)?.vector();
        let y = z.cross(x);
        let linear = Matrix3::new([[x.x, y.x, z.x], [x.y, y.y, z.y], [x.z, y.z, z.z]]);
        Transform::from_parts(linear, scale, origin.to_vector(), self.tol.angular())
    }

    /// The parameter of a point on one of this kernel's curves.
    fn parameter_of(&self, curve: &Curve, p: Point) -> Option<f64> {
        crate::inversion::parameter_on(curve, p)
    }

    /// The pcurves the file states for edge `edge_id` on surface entity
    /// `surface_id`: the `PCURVE`s of its `SURFACE_CURVE` or `SEAM_CURVE`
    /// drawn on that surface, as written. Nothing where the file states
    /// none, or states them in a form not read here.
    fn file_pcurves(&mut self, edge_id: u64, surface_id: u64) -> Vec<PlanarCurve> {
        let mut out = Vec::new();
        let Ok(args) = self.args(edge_id, "EDGE_CURVE") else {
            return out;
        };
        let Ok(instance) = self.instance(ref_at(args, 3)) else {
            return out;
        };
        let Some(dressed) = instance
            .part("SURFACE_CURVE")
            .or_else(|| instance.part("SEAM_CURVE"))
            .or_else(|| instance.part("INTERSECTION_CURVE"))
        else {
            return out;
        };
        let uses: Vec<u64> = dressed
            .get(2)
            .and_then(Arg::list)
            .unwrap_or(&[])
            .iter()
            .filter_map(Arg::reference)
            .collect();
        for id in uses {
            let Ok(pargs) = self.args(id, "PCURVE") else {
                continue;
            };
            if pargs.get(1).and_then(Arg::reference) != Some(surface_id) {
                continue;
            }
            let Ok(dargs) = self.args(ref_at(pargs, 2), "DEFINITIONAL_REPRESENTATION") else {
                continue;
            };
            let Some(item) = dargs
                .get(1)
                .and_then(Arg::list)
                .and_then(|items| items.first())
                .and_then(Arg::reference)
            else {
                continue;
            };
            if let Ok(Some(curve)) = self.nested(|reader| reader.planar_curve(item)) {
                out.push(curve);
            }
        }
        out
    }

    /// A curve in a surface's parameters, its coordinates as written: a
    /// line, a circle, an ellipse, a spline, or a trim of one, which is its
    /// basis.
    fn planar_curve(&mut self, id: u64) -> OgeomResult<Option<PlanarCurve>> {
        let instance = self.instance(id)?;
        let (base, knotted, weights) = (
            instance.part("B_SPLINE_CURVE"),
            instance.part("B_SPLINE_CURVE_WITH_KNOTS"),
            instance.part("RATIONAL_B_SPLINE_CURVE"),
        );
        // A spline's degree, control points and knots sit at these places
        // in a simple instance, and split between parts in a complex one.
        let spline = match (base, knotted) {
            (Some(base), Some(knotted)) => {
                Some((base.first(), base.get(1), knotted.first(), knotted.get(1)))
            }
            (None, Some(knotted)) => Some((
                knotted.get(1),
                knotted.get(2),
                knotted.get(6),
                knotted.get(7),
            )),
            _ => None,
        };
        if let Some((degree, control, mults, knots)) = spline {
            let degree = file_degree(degree, 0.0)?;
            let mut points = Vec::new();
            for r in control
                .and_then(Arg::list)
                .unwrap_or(&[])
                .iter()
                .filter_map(Arg::reference)
            {
                points.push(self.point2(r)?);
            }
            let expanded = expanded_knots(
                mults.and_then(Arg::list).unwrap_or(&[]),
                knots.and_then(Arg::list).unwrap_or(&[]),
            )?;
            let flat: Vec<f64> = weights
                .and_then(|w| w.first())
                .and_then(Arg::list)
                .unwrap_or(&[])
                .iter()
                .filter_map(Arg::number)
                .collect();
            let control = points
                .into_iter()
                .zip(flat.iter().chain(std::iter::repeat(&1.0)))
                .map(|(p, w)| Weighted::new(p, *w, self.tol))
                .collect::<OgeomResult<Vec<_>>>()?;
            let knots = KnotVector::new(expanded, degree)?;
            return Ok(Some(
                ogeom_geom::BSpline2d::rational(knots, control)?.into(),
            ));
        }
        let args = instance
            .parts
            .first()
            .map_or(&[][..], |(_, a)| a.as_slice());
        let out: PlanarCurve = match instance.keyword() {
            "LINE" => {
                let through = self.point2(ref_at(args, 1))?;
                let vector = self.args(ref_at(args, 2), "VECTOR")?;
                let direction = self.direction2(vector[1].reference().unwrap_or(0))?;
                // The vector's magnitude is the parameter's pace along the
                // line. A unit one is the length a line here runs by; any
                // other is the straight spline of degree one at that pace.
                let magnitude = vector.get(2).and_then(Arg::number).unwrap_or(1.0);
                if magnitude == 1.0 {
                    let axis = ogeom_math::Axis2 {
                        location: through,
                        direction,
                    };
                    ogeom_geom::Line2d::over(axis, -SURFACE_EXTENT, SURFACE_EXTENT)?.into()
                } else {
                    let step = direction.vector() * magnitude;
                    let ends = vec![
                        through - step * SURFACE_EXTENT,
                        through + step * SURFACE_EXTENT,
                    ];
                    let knots = vec![
                        -SURFACE_EXTENT,
                        -SURFACE_EXTENT,
                        SURFACE_EXTENT,
                        SURFACE_EXTENT,
                    ];
                    ogeom_geom::BSpline2d::new(KnotVector::new(knots, 1)?, ends, self.tol)?.into()
                }
            }
            "CIRCLE" => {
                let frame = self.frame2(ref_at(args, 1))?;
                let radius = args.get(2).and_then(Arg::number).unwrap_or(0.0);
                ogeom_geom::Circle2d::new(ogeom_math::Circle2::new(frame, radius, self.tol)?).into()
            }
            "ELLIPSE" => {
                let frame = self.frame2(ref_at(args, 1))?;
                let a = args.get(2).and_then(Arg::number).unwrap_or(0.0);
                let b = args.get(3).and_then(Arg::number).unwrap_or(0.0);
                ogeom_geom::Ellipse2d::new(ogeom_math::Ellipse2::new(frame, a, b, self.tol)?).into()
            }
            "TRIMMED_CURVE" => {
                // The trims restate the edge's ends; the basis carries the
                // parameter, so it is the pcurve.
                return self.planar_curve(ref_at(args, 1));
            }
            _ => return Ok(None),
        };
        Ok(Some(out))
    }

    /// A point of a surface's parameters, as written.
    fn point2(&mut self, id: u64) -> OgeomResult<ogeom_math::Point2> {
        let args = self.args(id, "CARTESIAN_POINT")?;
        let Some(coords) = args.get(1).and_then(Arg::list) else {
            ogeom_bail!(Construction, "#{id}: a point without coordinates");
        };
        let value = |i: usize| coords.get(i).and_then(Arg::number).unwrap_or(0.0);
        Ok(ogeom_math::Point2::new(value(0), value(1)))
    }

    fn direction2(&mut self, id: u64) -> OgeomResult<ogeom_math::Direction2> {
        let args = self.args(id, "DIRECTION")?;
        let Some(coords) = args.get(1).and_then(Arg::list) else {
            ogeom_bail!(Construction, "#{id}: a direction without components");
        };
        let value = |i: usize| coords.get(i).and_then(Arg::number).unwrap_or(0.0);
        ogeom_math::Direction2::new(ogeom_math::Vector2::new(value(0), value(1)), self.tol)
    }

    /// An `AXIS2_PLACEMENT_2D` as a frame, its reference direction the
    /// standard's default where the file leaves it out.
    fn frame2(&mut self, id: u64) -> OgeomResult<ogeom_math::Frame2> {
        let args = self.args(id, "AXIS2_PLACEMENT_2D")?;
        let origin = self.point2(ref_at(args, 1))?;
        let x = match args.get(2).and_then(Arg::reference) {
            Some(r) => self.direction2(r)?,
            None => ogeom_math::Direction2::X,
        };
        Ok(ogeom_math::Frame2::new(origin, x))
    }

    // --- topology ------------------------------------------------------------

    fn vertex(&mut self, id: u64) -> OgeomResult<Shape> {
        if let Some(shape) = self.vertices.get(&id) {
            return Ok(shape.clone());
        }
        let args = self.args(id, "VERTEX_POINT")?;
        let point = self.point(ref_at(args, 1))?;
        let shape = make_vertex(&mut self.model, point).shape;
        self.vertices.insert(id, shape.clone());
        Ok(shape)
    }

    /// An `EDGE_CURVE`, built once, curve-forward.
    ///
    /// Returns the edge, its curve, its range, and whether STEP's edge
    /// direction runs *against* the curve, which each use folds into its
    /// own orientation.
    fn edge(&mut self, id: u64) -> OgeomResult<Option<BuiltEdge>> {
        if let Some(found) = self.edges.get(&id) {
            return Ok(Some(found.clone()));
        }
        let args = self.args(id, "EDGE_CURVE")?;
        let v1 = ref_at(args, 1);
        let v2 = ref_at(args, 2);
        let Some(mut curve) = self.curve(ref_at(args, 3))? else {
            return Ok(None);
        };
        let same_sense = !args.get(4).is_some_and(|a| a.is_enum("F"));

        let p1 = {
            let vargs = self.args(v1, "VERTEX_POINT")?;
            self.point(ref_at(vargs, 1))?
        };
        let p2 = {
            let vargs = self.args(v2, "VERTEX_POINT")?;
            self.point(ref_at(vargs, 1))?
        };

        // The edge is built along the curve's own parameter; a STEP edge
        // running the other way is flagged, and every use composes the flag
        // into its orientation.
        let (start, end, mut flipped) = if same_sense {
            (p1, p2, false)
        } else {
            (p2, p1, true)
        };
        let (range, closed) = match (
            self.parameter_of(&curve, start),
            self.parameter_of(&curve, end),
        ) {
            (Some(a), Some(b)) => {
                let period = if curve.is_periodic() {
                    let (lo, hi) = curve.domain();
                    hi - lo
                } else {
                    0.0
                };
                if v1 == v2 {
                    ((a, a + if period > 0.0 { period } else { 0.0 }), true)
                } else if period > 0.0 && b <= a + self.tol.parametric() {
                    ((a, b + period), false)
                } else if b < a - self.tol.parametric() {
                    // The vertices stand at descending parameters on an
                    // open curve whatever the sense flag says: a
                    // mesh-to-STEP converter writes every edge forward and
                    // lets the line run the other way. The edge is built
                    // along the curve's own parameter and runs against it.
                    flipped = !flipped;
                    self.report.warnings.push(format!(
                        "#{id}: the edge's vertices run against its curve's \
                         parameter despite its sense flag; the edge was reversed"
                    ));
                    self.tally("edge-against-curve", a - b, id);
                    ((b, a), false)
                } else {
                    ((a, b), false)
                }
            }
            _ => {
                // No closed-form inversion: take the curve's own domain and
                // hold the endpoints to it.
                //
                // A closed edge whose one vertex sits on the curve but away
                // from its seam (a fitted loop written with its start
                // wherever the fit began, the vertex millimetres along it)
                // would otherwise be held to a seam the vertex misses by
                // that much, and the vertex's tolerance widened to say so.
                // The seam is moved to the vertex instead: the same curve,
                // begun where the edge does.
                if v1 == v2
                    && let Curve::BSpline(spline) = &curve
                {
                    let (lo, hi) = curve.domain();
                    let gap = curve.point_at(lo, self.tol)?.distance(start);
                    if gap > self.tol.confusion() * 10.0 {
                        let samples = (spline.control_points().len() * 8).max(64);
                        if let Ok(found) = project_on_curve(&curve, start, samples, self.tol)
                            && found.distance <= self.tol.confusion() * 1e4
                            && found.parameter > lo + self.tol.parametric()
                            && found.parameter < hi - self.tol.parametric()
                            && let Ok(moved) = spline.reseamed_at(found.parameter, self.tol)
                        {
                            curve = Curve::BSpline(moved);
                            self.report.warnings.push(format!(
                                "#{id}: a closed edge's vertex sits {gap:.2e} along its \
                                 curve from the seam; the seam was moved to the vertex"
                            ));
                            self.tally("reseamed", gap, id);
                        }
                    }
                }
                let (lo, hi) = curve.domain();
                let head = curve.point_at(lo, self.tol)?;
                let tail = curve.point_at(hi, self.tol)?;
                let (head_miss, tail_miss) = (head.distance(start), tail.distance(end));
                if head_miss > self.tol.confusion() * 10.0
                    || tail_miss > self.tol.confusion() * 10.0
                {
                    // An open edge whose vertices stand *on* the curve but
                    // short of its ends (a file that writes the whole
                    // spline and lets the vertices say where the edge
                    // stops, millimetres in) takes the window between the
                    // vertices' own feet. Held to the whole curve, the edge
                    // overshoots its neighbours and the face it bounds draws
                    // as nothing at all.
                    let window = if v1 == v2 {
                        None
                    } else {
                        window_between_feet(&curve, start, end, self.tol)
                    };
                    if let Some(window) = window {
                        self.report.warnings.push(format!(
                            "#{id}: edge endpoints sit {head_miss:.2e} and {tail_miss:.2e} \
                             from its curve's ends; the window between the vertices' \
                             feet on the curve was taken"
                        ));
                        self.tally("vertex-window", head_miss.max(tail_miss), id);
                        (window, false)
                    } else {
                        self.report.warnings.push(format!(
                            "#{id}: edge endpoints sit {head_miss:.2e} and {tail_miss:.2e} \
                             from its curve's ends; the curve's own domain was taken"
                        ));
                        ((lo, hi), v1 == v2)
                    }
                } else {
                    ((lo, hi), v1 == v2)
                }
            }
        };
        let _ = closed;

        let (vlo, vhi) = if flipped {
            (self.vertex(v2)?, self.vertex(v1)?)
        } else {
            (self.vertex(v1)?, self.vertex(v2)?)
        };
        // Real files are imprecise. Where the curve's end misses its vertex
        // by more than the default
        // tolerance, the vertex's tolerance *grows* to state the gap; the
        // data model's growing tolerances are exactly for this, and the
        // warning keeps the healing visible.
        for (vertex, t) in [(&vlo, range.0), (&vhi, range.1)] {
            let end = curve.point_at(t, self.tol)?;
            let stated = {
                let Some(node) = self.model.node(vertex) else {
                    continue;
                };
                let Some(data) = node.data().as_vertex() else {
                    continue;
                };
                (data.point, data.tolerance)
            };
            let gap = end.distance(stated.0);
            if gap > stated.1.get() {
                if let Some(node) = self.model.node_mut(vertex)
                    && let ogeom_topo::NodeData::Vertex(data) = node.data_mut()
                {
                    data.tolerance = data.tolerance.widen_to(gap + self.tol.confusion());
                }
                self.warn_vertex_miss(id, gap);
            }
        }
        let shape =
            make_edge_between(&mut self.model, curve.clone(), range, &vlo, &vhi, self.tol)?.shape;
        let entry = (shape, curve, range, flipped);
        self.edges.insert(id, entry.clone());
        Ok(Some(entry))
    }

    /// The edge a bound names as the bound walks it: its pieces where it
    /// was cut at a pole, in order along its curve, or the edge itself
    /// under its own id.
    fn edge_pieces(&mut self, id: u64) -> OgeomResult<Option<Vec<(u64, BuiltEdge)>>> {
        if let Some(pieces) = self.pieces.get(&id) {
            return Ok(Some(pieces.clone()));
        }
        Ok(self.edge(id)?.map(|built| vec![(id, built)]))
    }

    /// The keys a bound's use of edge `id` stands under.
    fn piece_keys(&self, id: u64) -> Vec<u64> {
        self.pieces.get(&id).map_or_else(
            || vec![id],
            |pieces| pieces.iter().map(|(k, _)| *k).collect(),
        )
    }

    /// What a file's edge id stands for in the read model: the pieces it
    /// was cut into at a pole, in order along its curve, or the edge
    /// itself; nothing when it never built. Everything that names an edge
    /// by id (a style, a shape aspect) resolves through here, so it lands
    /// on the edges the faces use.
    fn edge_shapes(&self, id: u64) -> Vec<Shape> {
        if let Some(pieces) = self.pieces.get(&id) {
            return pieces
                .iter()
                .map(|(_, (piece, ..))| piece.clone())
                .collect();
        }
        self.edges
            .get(&id)
            .map(|(shape, ..)| shape.clone())
            .into_iter()
            .collect()
    }

    /// Cut every edge of these faces whose curve runs through a pole of a
    /// face it bounds (a sphere's pole, a cone's apex, a patch's collapsed
    /// side) at that pole, before any face is built.
    ///
    /// No single image of such a curve follows it through the chart: at the
    /// pole it leaves along another column than the one it arrived on, and
    /// a fit across the jump stands millimetres off the edge. Cut there,
    /// each piece has an image of its own (a meridian of a sphere is
    /// exactly a straight column), and the face's wire closes across the
    /// pole row as it does at any pole vertex. The cut is made on the edge,
    /// so every face it bounds walks the same pieces.
    ///
    /// Best-effort: anything unreadable here is left whole, and the face
    /// that uses it reports it as it would have.
    fn cut_at_poles(&mut self, face_ids: &[u64]) {
        let mut crossings: BTreeMap<u64, Vec<crate::pcurves::Crossing>> = BTreeMap::new();
        for &fid in face_ids {
            let Ok(args) = self.face_args(fid) else {
                continue;
            };
            let Some(surface) = args
                .get(2)
                .and_then(Arg::reference)
                .and_then(|sid| self.surface(sid).ok().flatten())
            else {
                continue;
            };
            let poles = crate::pcurves::chart_poles(&surface, self.tol);
            if poles.is_empty() {
                continue;
            }
            let bounds: Vec<u64> = args
                .get(1)
                .and_then(Arg::list)
                .unwrap_or(&[])
                .iter()
                .filter_map(Arg::reference)
                .collect();
            for bound in bounds {
                let Ok((loop_id, _)) = self.bound_args(bound) else {
                    continue;
                };
                let Ok(loop_args) = self.args(loop_id, "EDGE_LOOP") else {
                    continue;
                };
                let edge_ids: Vec<u64> = loop_args
                    .get(1)
                    .and_then(Arg::list)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Arg::reference)
                    .filter_map(|oe| {
                        self.args(oe, "ORIENTED_EDGE")
                            .ok()
                            .and_then(|a| a.get(3).and_then(Arg::reference))
                    })
                    .collect();
                for edge_id in edge_ids {
                    if self.pieces.contains_key(&edge_id) {
                        continue;
                    }
                    let Ok(Some((_, curve, range, _))) = self.edge(edge_id) else {
                        continue;
                    };
                    let found = crate::pcurves::pole_crossings(&curve, range, &poles, self.tol);
                    if found.is_empty() {
                        continue;
                    }
                    let entry = crossings.entry(edge_id).or_default();
                    for crossing in found {
                        if entry.iter().all(|(t, ..)| (t - crossing.0).abs() > 1e-9) {
                            entry.push(crossing);
                        }
                    }
                }
            }
        }
        for (edge_id, mut at) in crossings {
            at.sort_by(|x, y| x.0.total_cmp(&y.0));
            let Some((shape, curve, range, flipped)) = self.edges.get(&edge_id).cloned() else {
                continue;
            };
            let Ok(cut) = crate::pcurves::split_at_poles(
                &mut self.model,
                &shape,
                &curve,
                range,
                &at,
                &mut self.pole_vertices,
                self.tol,
            ) else {
                continue;
            };
            let pieces = cut
                .into_iter()
                .map(|(piece, window)| {
                    self.piece_key -= 1;
                    (self.piece_key, (piece, curve.clone(), window, flipped))
                })
                .collect();
            self.pieces.insert(edge_id, pieces);
        }
    }

    /// Split one rim of every band whose two rims start on different
    /// columns, before any face is built.
    ///
    /// A face on a periodic surface bounded by two whole circles that are
    /// parallels of it needs a seam joining a vertex of one rim to a vertex
    /// of the other along one column. Where the rims' vertices stand on
    /// different columns, the second rim (or the first, where the second
    /// is already in pieces) is split on the other's column. The cut is
    /// made on the edge, so every face it bounds walks both pieces, and the
    /// band is then built through the common column.
    ///
    /// A split rim changes every face that holds it, so the faces are
    /// passed over again until a pass splits and marks nothing: a band on
    /// the far side of the rim, seen whole before the split, is marked as a
    /// band of rings then, or it would be built with a rim in pieces and no
    /// seam.
    ///
    /// Best-effort: a face whose rims resist is left as it was, and the
    /// face reports it as it would have.
    fn split_band_rims(&mut self, face_ids: &[u64]) {
        loop {
            let before = (self.pieces.len(), self.band_faces.len());
            for &fid in face_ids {
                let _ = self.split_band_rim(fid);
            }
            if (self.pieces.len(), self.band_faces.len()) == before {
                break;
            }
        }
    }

    /// [`Self::split_band_rims`] for one face.
    fn split_band_rim(&mut self, fid: u64) -> OgeomResult<()> {
        let args = self.face_args(fid)?.to_vec();
        let Some(surface) = args
            .get(2)
            .and_then(Arg::reference)
            .and_then(|sid| self.surface(sid).ok().flatten())
        else {
            return Ok(());
        };
        if !surface.is_periodic_u() {
            return Ok(());
        }
        let bounds: Vec<u64> = args
            .get(1)
            .and_then(Arg::list)
            .unwrap_or(&[])
            .iter()
            .filter_map(Arg::reference)
            .collect();
        let [first, second] = bounds[..] else {
            return Ok(());
        };
        // Each bound one oriented edge: the rim's file id.
        let mut rims = Vec::with_capacity(2);
        for bound in [first, second] {
            let (loop_id, _) = self.bound_args(bound)?;
            let Ok(loop_args) = self.args(loop_id, "EDGE_LOOP") else {
                return Ok(());
            };
            let edge_ids: Vec<u64> = loop_args
                .get(1)
                .and_then(Arg::list)
                .unwrap_or(&[])
                .iter()
                .filter_map(Arg::reference)
                .filter_map(|oe| {
                    self.args(oe, "ORIENTED_EDGE")
                        .ok()
                        .and_then(|a| a.get(3).and_then(Arg::reference))
                })
                .collect();
            let [edge_id] = edge_ids[..] else {
                return Ok(());
            };
            rims.push(edge_id);
        }
        // Each rim's edges as the faces will walk them, and its circle.
        let mut rings: Vec<(Vec<Shape>, ogeom_math::Circle)> = Vec::with_capacity(2);
        for &edge_id in &rims {
            let Some(pieces) = self.edge_pieces(edge_id)? else {
                return Ok(());
            };
            let Curve::Circle(c) = &pieces[0].1.1 else {
                return Ok(());
            };
            let circle = c.circle();
            let shapes: Vec<Shape> = pieces.into_iter().map(|(_, (s, ..))| s).collect();
            if shapes.len() == 1 {
                let Some((a, b)) = ogeom_algo::edge_vertices(&self.model, &shapes[0])? else {
                    return Ok(());
                };
                if !a.is_same(&b) {
                    return Ok(());
                }
            }
            rings.push((shapes, circle));
        }
        let all: Vec<&Shape> = rings.iter().flat_map(|(s, _)| s.iter()).collect();
        if !ogeom_algo::rings_are_parallels(&self.model, &surface, &all, self.tol)? {
            return Ok(());
        }
        let starts = |model: &Model, ring: &[Shape]| -> OgeomResult<Vec<(Shape, Point)>> {
            let mut out = Vec::with_capacity(ring.len());
            for edge in ring {
                if let Some((v, _)) = ogeom_algo::edge_vertices(model, edge)?
                    && let Some(data) = model.node(&v).and_then(|n| n.data().as_vertex())
                {
                    let at = data.point;
                    out.push((v, at));
                }
            }
            Ok(out)
        };
        let starts_a = starts(&self.model, &rings[0].0)?;
        let starts_b = starts(&self.model, &rings[1].0)?;
        let shared = starts_a.iter().any(|(_, p)| {
            starts_b
                .iter()
                .any(|(_, q)| same_column(&rings[1].1, *p, *q, self.tol))
        });
        let split_any = rings.iter().any(|(s, _)| s.len() > 1);
        if shared {
            if split_any {
                self.band_faces.insert(fid);
            }
            return Ok(());
        }
        // Split a rim still whole on the other's first vertex column.
        let (target, anchor) = if rings[1].0.len() == 1 {
            (1, &starts_a[0].0)
        } else if rings[0].0.len() == 1 {
            (0, &starts_b[0].0)
        } else {
            return Ok(());
        };
        let edge_id = rims[target];
        let Some((shape, curve, _, flipped)) = self.edges.get(&edge_id).cloned() else {
            return Ok(());
        };
        let Some(split) =
            ogeom_algo::split_ring_on_column(&mut self.model, &surface, &shape, anchor, self.tol)?
        else {
            return Ok(());
        };
        let mut pieces = Vec::with_capacity(2);
        for piece in split.history.modified(&shape) {
            let Some(ogeom_topo::EdgeRepr::Curve3d { range, .. }) = self
                .model
                .node(piece)
                .and_then(|n| n.data().as_edge())
                .and_then(|d| d.curve3d())
            else {
                return Ok(());
            };
            self.piece_key -= 1;
            pieces.push((
                self.piece_key,
                (piece.clone(), curve.clone(), *range, flipped),
            ));
        }
        self.pieces.insert(edge_id, pieces);
        self.band_faces.insert(fid);
        Ok(())
    }

    /// A band face whose rim was split on the other rim's column: the band
    /// of rings through a column both rims have a vertex on. Each ring's
    /// occurrences are turned to start there and keep the direction the
    /// file walks them, which says which side the face faces.
    fn band_of_split_rims(
        &mut self,
        surface: &SurfaceGeometry,
        wires: &[Shape],
    ) -> OgeomResult<Shape> {
        let [wire_a, wire_b] = wires else {
            ogeom_bail!(Construction, "a band has two rims");
        };
        let ring_a = self.model.ordered_children_of(wire_a)?;
        let ring_b = self.model.ordered_children_of(wire_b)?;
        let circle = {
            let Some(ogeom_topo::EdgeRepr::Curve3d { curve, .. }) = self
                .model
                .node(&ring_b[0])
                .and_then(|n| n.data().as_edge())
                .and_then(|d| d.curve3d())
            else {
                ogeom_bail!(Construction, "a band rim has no curve");
            };
            let Some(Curve::Circle(c)) = self.model.geometry().curve(*curve) else {
                ogeom_bail!(Construction, "a band rim is not a circle");
            };
            c.circle()
        };
        let start = |model: &Model, edge: &Shape| -> OgeomResult<Point> {
            let Some((v, _)) = ogeom_algo::edge_vertices(model, edge)? else {
                ogeom_bail!(Construction, "a band rim has no vertex");
            };
            let Some(data) = model.node(&v).and_then(|n| n.data().as_vertex()) else {
                ogeom_bail!(Construction, "a band rim's vertex holds no point");
            };
            Ok(data.point)
        };
        for i in 0..ring_a.len() {
            let p = start(&self.model, &ring_a[i])?;
            for j in 0..ring_b.len() {
                let q = start(&self.model, &ring_b[j])?;
                if !same_column(&circle, p, q, self.tol) {
                    continue;
                }
                let mut a = ring_a.clone();
                a.rotate_left(i);
                let mut b = ring_b.clone();
                b.rotate_left(j);
                return ogeom_algo::make_band_of_rings(&mut self.model, surface, &a, &b, self.tol);
            }
        }
        ogeom_bail!(Construction, "the band's rims share no vertex column")
    }

    fn face(&mut self, id: u64) -> OgeomResult<Option<Shape>> {
        if let Some(shape) = self.faces.get(&id) {
            return Ok(Some(shape.clone()));
        }
        let args = self.face_args(id)?;
        let bounds: Vec<u64> = args
            .get(1)
            .and_then(Arg::list)
            .unwrap_or(&[])
            .iter()
            .filter_map(Arg::reference)
            .collect();
        let Some(surface) = self.surface(ref_at(args, 2))? else {
            return Ok(None);
        };
        let face_forward = !args.get(3).is_some_and(|a| a.is_enum("F"));
        let surface_id = self.model.geometry_mut().add_surface(surface.clone());

        // Outer bound first, so the face's first wire is its outer ring.
        let mut ordered = bounds.clone();
        ordered.sort_by_key(|b| {
            self.instances
                .get(*b)
                .map_or(1, |i| i32::from(i.part("FACE_OUTER_BOUND").is_none()))
        });

        // Which edges this face uses twice: those are seams, and get both
        // sides' pcurves.
        let mut edge_uses: FastMap<u64, usize> = FastMap::default();
        for &bound in &ordered {
            let bargs = self.bound_args(bound)?;
            if self.instance(bargs.0)?.part("VERTEX_LOOP").is_some() {
                continue;
            }
            let loop_args = self.args(bargs.0, "EDGE_LOOP")?;
            for oe in loop_args.get(1).and_then(Arg::list).unwrap_or(&[]) {
                if let Some(oe_id) = oe.reference() {
                    let oargs = self.args(oe_id, "ORIENTED_EDGE")?;
                    if let Some(e) = oargs.get(3).and_then(Arg::reference) {
                        for key in self.piece_keys(e) {
                            *edge_uses.entry(key).or_default() += 1;
                        }
                    }
                }
            }
        }

        let mut wires = Vec::new();
        let mut annotated: FastSet<u64> = FastSet::default();
        for &bound in &ordered {
            let (loop_id, bound_forward) = self.bound_args(bound)?;
            if let Some(vertex_loop) = {
                let instance = self.instance(loop_id)?;
                instance.part("VERTEX_LOOP")
            } {
                // A loop of one vertex: a pole or an apex. It has no edges,
                // but it still bounds the face in parameter space, as a
                // degenerate edge running across the chart at the row the
                // point collapses to, exactly as native cones and spheres
                // are built.
                let vertex_id = vertex_loop.get(1).and_then(Arg::reference).unwrap_or(0);
                let vargs = self.args(vertex_id, "VERTEX_POINT")?;
                let at = self.point(ref_at(vargs, 1))?;
                let vertex = self.vertex(vertex_id)?;
                let projection = ogeom_algo::project_on_surface(&surface, at, 32, self.tol)?;
                let ((ua, ub), _) = surface.domain();
                let row = projection.parameters.1;
                let mut data = ogeom_topo::EdgeData::new();
                data.degenerate = true;
                let edge = self
                    .model
                    .add_edge(data, &[vertex.clone(), vertex.clone()])?;
                let pcurve: PlanarCurve = ogeom_geom::Line2d::segment(
                    ogeom_math::Point2::new(ua, row),
                    ogeom_math::Point2::new(ub, row),
                    self.tol,
                )?
                .into();
                ogeom_algo::attach_pcurve(
                    &mut self.model,
                    &edge,
                    pcurve,
                    surface_id,
                    Location::identity(),
                    (0.0, ub - ua),
                )?;
                wires.push(make_wire(&mut self.model, &[edge], self.tol)?.shape);
                continue;
            }
            let loop_args = self.args(loop_id, "EDGE_LOOP")?;
            let mut uses: Vec<BoundUse> = Vec::new();
            for oe in loop_args.get(1).and_then(Arg::list).unwrap_or(&[]) {
                let Some(oe_id) = oe.reference() else {
                    continue;
                };
                let oargs = self.args(oe_id, "ORIENTED_EDGE")?;
                let Some(edge_id) = oargs.get(3).and_then(Arg::reference) else {
                    continue;
                };
                let forward = !oargs.get(4).is_some_and(|a| a.is_enum("F"));
                let Some(pieces) = self.edge_pieces(edge_id)? else {
                    self.report.warnings.push(format!(
                        "#{id}: a bound references unreadable edge #{edge_id}; \
                         the face is skipped"
                    ));
                    return Ok(None);
                };
                let flipped = pieces.first().is_some_and(|(_, built)| built.3);
                // The use's direction composes the loop's, the bound's and
                // the edge-against-curve flag. The pieces of a cut edge go
                // in the order the loop as written walks them; reversing a
                // reversed bound below turns them with the rest.
                let mut use_forward = forward == bound_forward;
                if flipped {
                    use_forward = !use_forward;
                }
                let mut walked: Vec<BoundUse> = pieces
                    .into_iter()
                    .map(|(key, (shape, curve, range, _))| {
                        let placed = if use_forward {
                            shape.clone()
                        } else {
                            shape.reversed()
                        };
                        (placed, shape, key, curve, range)
                    })
                    .collect();
                if forward == flipped {
                    walked.reverse();
                }
                uses.extend(walked);
            }
            if !bound_forward {
                uses.reverse();
            }
            // The window may have grown to hold this bound, and the images
            // are derived on the surface as it stands.
            self.widen_window(surface_id, &uses)?;
            let widened = self
                .model
                .geometry()
                .surface(surface_id)
                .cloned()
                .unwrap_or_else(|| surface.clone());
            self.chart_bound(id, &uses, &widened, surface_id, &edge_uses, &mut annotated)?;
            let edges: Vec<Shape> = uses.into_iter().map(|(placed, ..)| placed).collect();
            if edges.is_empty() {
                continue;
            }
            wires.push(make_wire(&mut self.model, &edges, self.tol)?.shape);
        }
        if wires.is_empty() {
            self.report
                .warnings
                .push(format!("#{id}: a face with no readable bounds is skipped"));
            return Ok(None);
        }
        // As the bounds left it: a window stretched to hold them is the
        // surface every path below builds on, including the band, which
        // registers the value it is handed rather than the one on record.
        let surface = self
            .model
            .geometry()
            .surface(surface_id)
            .cloned()
            .unwrap_or(surface);

        // A band whose rim was split on the other rim's column takes its
        // seam there. The rings are walked as the file walks them about the
        // face's own normal, which already says which side the face faces.
        if wires.len() == 2 && self.band_faces.contains(&id) {
            match self.band_of_split_rims(&surface, &wires) {
                Ok(shape) => {
                    self.faces.insert(id, shape.clone());
                    return Ok(Some(shape));
                }
                Err(e) => {
                    self.report.warnings.push(format!(
                        "#{id}: no seam could be synthesised ({e}); the \
                         face may not triangulate"
                    ));
                }
            }
        }

        // A periodic face bound only by closed rings (a cylinder band
        // between two circles) arrives without a seam edge, which is a
        // legitimate STEP shape and an open rectangle in this kernel's
        // chart. The seam is synthesised the way native cylinders build it:
        // one edge at the period join, appearing in the wire twice.
        if wires.len() == 2
            && surface.is_periodic_u()
            && let [(e_lo, _v_lo), (e_hi, _v_hi)] =
                closed_ring_edges(&self.model, &wires)?.as_slice()
            // Only rings that are parallels of the surface make a band.
            // Two closed circles that merely lie on it (a button head's
            // rims, square to the screw on a sphere whose chart runs along
            // z) bound a legitimate face on their own, nested loops in the
            // chart, and take the ordinary path below without a word.
            && ogeom_algo::rings_are_parallels(&self.model, &surface, &[e_lo, e_hi], self.tol)?
        {
            {
                // The band construction is ogeom-algo's make_revolution_band:
                // one authority shared with the healer. Anything it refuses
                // (a spline ring, ring vertices at different angles, a
                // surface with no closed-form iso-curve) becomes a warning
                // and the raw bounds, not an error.
                match ogeom_algo::make_revolution_band(
                    &mut self.model,
                    &surface,
                    e_lo,
                    e_hi,
                    self.tol,
                ) {
                    Ok(built) => {
                        let shape = if face_forward {
                            built
                        } else {
                            built.reversed()
                        };
                        self.faces.insert(id, shape.clone());
                        return Ok(Some(shape));
                    }
                    Err(e) => {
                        self.report.warnings.push(format!(
                            "#{id}: no seam could be synthesised ({e}); the \
                             face may not triangulate"
                        ));
                    }
                }
            }
        }

        // A cone face bounded by a *single* closed ring: the apex is the
        // other boundary, and some files simply never write it: no vertex
        // loop, nothing. The geometry leaves one choice of what the face
        // means, so the apex is synthesised and the band built as if the
        // file had said so.
        if wires.len() == 1
            && matches!(surface, SurfaceGeometry::Cone(_))
            && let [(ring, _)] = closed_ring_edges(&self.model, &wires)?.as_slice()
        {
            match ogeom_algo::make_apex_band(&mut self.model, &surface, ring, self.tol) {
                Ok(built) => {
                    let shape = if face_forward {
                        built
                    } else {
                        built.reversed()
                    };
                    self.faces.insert(id, shape.clone());
                    return Ok(Some(shape));
                }
                Err(e) => {
                    self.report.warnings.push(format!(
                        "#{id}: no apex could be synthesised ({e}); the face \
                         may not triangulate"
                    ));
                }
            }
        }

        // Every wire's images on one branch of a periodic chart: a file's
        // pcurves, or fits of them, answer in whatever phase they like, and
        // a hole loop straddling a drum's seam arrives half on each branch.
        ogeom_algo::chain_wire_branches(&mut self.model, surface_id, &wires, self.tol)?;
        // With every image on one branch, a boundary meeting a pole from two
        // sides of the chart is closed across the pole row.
        for wire in &mut wires {
            let edges = self.model.ordered_children_of(wire)?;
            let closed =
                ogeom_algo::closed_at_poles(&mut self.model, surface_id, &edges, self.tol)?;
            if closed.len() != edges.len() {
                *wire = make_wire(&mut self.model, &closed, self.tol)?.shape;
            }
        }
        // A file walks each loop with the face's material on its left about
        // the face's own normal; a face turned against its surface is stored
        // with its loops walked back, so that turning it walks them as
        // written.
        if !face_forward {
            wires = walked_back(&mut self.model, &wires, self.tol)?;
        }
        let built = make_face_on(&mut self.model, surface_id, &wires, self.tol)?.shape;
        let shape = if face_forward {
            built
        } else {
            built.reversed()
        };
        self.faces.insert(id, shape.clone());
        Ok(Some(shape))
    }

    /// A bound's loop and orientation, whichever of the two bound kinds it is.
    fn bound_args(&mut self, id: u64) -> OgeomResult<(u64, bool)> {
        let instance = self.instance(id)?;
        let args = instance
            .part("FACE_OUTER_BOUND")
            .or_else(|| instance.part("FACE_BOUND"))
            .ok_or_else(|| ogeom_core::ogeom_err!(Construction, "#{id} is not a face bound"))?
            .to_vec();
        let loop_id = args.get(1).and_then(Arg::reference).unwrap_or(0);
        let forward = !args.get(2).is_some_and(|a| a.is_enum("F"));
        Ok((loop_id, forward))
    }

    /// Stretch a surface's parameter window to hold the edges that bound it.
    ///
    /// The window a reader gives a plane, a cylinder or a cone is a
    /// convention: those surfaces are unbounded, and [`SURFACE_EXTENT`] is
    /// a guess at how far past its own geometry a file will reach. A file
    /// can falsify the guess: a surface whose own origin sits far from the
    /// face it carries has trim parameters past any fixed window, the
    /// surface refuses to be evaluated where its own face lies, and the
    /// face draws as a hole.
    ///
    /// So the window is measured rather than guessed: the edges' own
    /// points, projected onto the surface's chart by the geometry that
    /// defines it, with the guess kept as a floor. Widening is safe for a
    /// surface other faces share, since a window only ever grows.
    fn widen_window(
        &mut self,
        surface_id: ogeom_topo::SurfaceId,
        uses: &[BoundUse],
    ) -> OgeomResult<()> {
        use ogeom_geom::Curve3d as _;
        let Some(surface) = self.model.geometry().surface(surface_id).cloned() else {
            return Ok(());
        };
        // Where each surface keeps its unbounded directions, and how a point
        // of space reads on them.
        let along: Box<ChartReading> = match &surface {
            SurfaceGeometry::Plane(p) => {
                let frame = p.plane().frame();
                let (origin, x, y) = (frame.origin(), frame.x().vector(), frame.y().vector());
                Box::new(move |at: Point| (Some((at - origin).dot(x)), Some((at - origin).dot(y))))
            }
            SurfaceGeometry::Cylinder(c) => {
                let axis = c.cylinder().axis();
                let (origin, direction) = (axis.location, axis.direction.vector());
                Box::new(move |at: Point| (None, Some((at - origin).dot(direction))))
            }
            SurfaceGeometry::Cone(c) => {
                let axis = c.cone().axis();
                let (origin, direction) = (axis.location, axis.direction.vector());
                Box::new(move |at: Point| (None, Some((at - origin).dot(direction))))
            }
            // A swept curve: a point's sweep parameter is its reach along
            // the direction less the swept curve's own, read against the
            // middle of the curve's reach; the curve's half-reach either
            // way is inside the margin the window grows by.
            SurfaceGeometry::Extrusion(e) => {
                let direction = e.direction().vector();
                let (lo, hi) = e.curve().domain();
                let (mut least, mut most) = (f64::INFINITY, f64::NEG_INFINITY);
                for k in 0..=32 {
                    let t = lo + (hi - lo) * f64::from(k) / 32.0;
                    if let Ok(p) = e.curve().point_at(t, self.tol) {
                        let reach = p.to_vector().dot(direction);
                        least = least.min(reach);
                        most = most.max(reach);
                    }
                }
                let middle = f64::midpoint(least, most);
                Box::new(move |at: Point| (None, Some(at.to_vector().dot(direction) - middle)))
            }
            _ => return Ok(()),
        };
        const STATIONS: usize = 4;
        let (mut ua, mut ub) = (-SURFACE_EXTENT, SURFACE_EXTENT);
        let (mut va, mut vb) = (-SURFACE_EXTENT, SURFACE_EXTENT);
        for (_, _, _, curve, range) in uses {
            for step in 0..=STATIONS {
                #[allow(clippy::cast_precision_loss)]
                let t = range.0 + (range.1 - range.0) * (step as f64 / STATIONS as f64);
                let Ok(at) = curve.point_at(t, self.tol) else {
                    continue;
                };
                let (u, v) = along(at);
                if let Some(u) = u {
                    ua = ua.min(u);
                    ub = ub.max(u);
                }
                if let Some(v) = v {
                    va = va.min(v);
                    vb = vb.max(v);
                }
            }
        }
        let ((was_u, was_v), grown) = (surface.domain(), (ub - ua) * 0.05);
        if ua >= was_u.0 && ub <= was_u.1 && va >= was_v.0 && vb <= was_v.1 {
            return Ok(());
        }
        let (u, v) = (
            (ua.min(was_u.0) - grown, ub.max(was_u.1) + grown),
            (va.min(was_v.0) - grown, vb.max(was_v.1) + grown),
        );
        let wider = match surface {
            SurfaceGeometry::Plane(p) => PlaneSurface::over(p.plane(), u, v)?.into(),
            SurfaceGeometry::Cylinder(c) => CylinderSurface::new(c.cylinder(), v)?.into(),
            SurfaceGeometry::Cone(c) => ConeSurface::new(c.cone(), v)?.into(),
            SurfaceGeometry::Extrusion(e) => {
                ExtrusionSurface::over(e.curve().clone(), e.direction(), v)?.into()
            }
            other => other,
        };
        if let Some(held) = self.model.geometry_mut().surface_mut(surface_id) {
            *held = wider;
        }
        Ok(())
    }

    /// One bound's images, chained around the face's chart.
    ///
    /// Each is derived on its own (the exact projection where the pair has
    /// a closed form, the fitted one where it does not), and a periodic
    /// chart then leaves a branch to choose. Chosen edge by edge the choice
    /// is arbitrary and the wire comes apart: a drilled block's bore wall
    /// reads back with one rim two whole turns from the other, both the
    /// right circle and neither meeting the seam the wire closes on. So
    /// each image after the first is slid by whole periods until its start
    /// meets where the last one ended.
    ///
    /// A seam falls out of the same walk. The wire uses it twice, up one
    /// column of the chart and down the other, and those columns are one
    /// image a period apart, so the walk finds both, and the use that runs
    /// forward is the forward side. Where it cannot, because the chart
    /// closes without being periodic, the other column goes a chart's width
    /// over, toward the middle.
    fn chart_bound(
        &mut self,
        face_id: u64,
        uses: &[BoundUse],
        surface: &SurfaceGeometry,
        surface_id: ogeom_topo::SurfaceId,
        edge_uses: &FastMap<u64, usize>,
        annotated: &mut FastSet<u64>,
    ) -> OgeomResult<()> {
        use ogeom_geom::Curve2d as _;
        let mut images: FastMap<u64, PlanarCurve> = FastMap::default();
        let mut sides: FastMap<u64, (Option<PlanarCurve>, Option<PlanarCurve>)> =
            FastMap::default();
        let mut order: Vec<u64> = Vec::new();
        let mut previous: Option<ogeom_math::Point2> = None;
        for (placed, shape, edge_id, curve, range) in uses {
            if !images.contains_key(edge_id) {
                let Some(image) =
                    self.image_for(face_id, *edge_id, shape, curve, *range, surface)?
                else {
                    continue;
                };
                images.insert(*edge_id, image);
                order.push(*edge_id);
            }
            let Some(image) = images.get(edge_id).cloned() else {
                continue;
            };
            let backwards = placed.orientation() == ogeom_topo::Orientation::Reversed;
            let (start, end) = if backwards {
                (range.1, range.0)
            } else {
                (range.0, range.1)
            };
            let image =
                crate::pcurves::shifted_to_meet(&image, start, previous, surface, self.tol)?;
            previous = Some(image.point_at(end, self.tol)?);
            let walked = sides.entry(*edge_id).or_default();
            if backwards {
                walked.1 = Some(image);
            } else {
                walked.0 = Some(image);
            }
        }
        for edge_id in order {
            if !annotated.insert(edge_id) {
                continue;
            }
            let Some((shape, range)) = uses
                .iter()
                .find(|(_, _, id, _, _)| *id == edge_id)
                .map(|(_, shape, _, _, range)| (shape.clone(), *range))
            else {
                continue;
            };
            let Some(image) = images.get(&edge_id).cloned() else {
                continue;
            };
            let walked = sides.remove(&edge_id).unwrap_or((None, None));
            let seam = edge_uses.get(&edge_id).copied().unwrap_or(1) > 1;
            let columns = match &walked {
                (Some(forward), Some(reversed)) => {
                    let at = forward.point_at(range.0, self.tol)?;
                    let other = reversed.point_at(range.0, self.tol)?;
                    (at.distance(other) > self.tol.confusion())
                        .then(|| (forward.clone(), reversed.clone()))
                }
                _ => None,
            };
            match (seam, columns) {
                (true, Some((forward, reversed))) => {
                    self.record_columns(&shape, forward, reversed, surface_id, range)?;
                }
                (true, None) => {
                    self.record_pcurve(&shape, image, surface_id, true, range)?;
                }
                (false, _) => {
                    let (forward, reversed) = walked;
                    let placed = forward.or(reversed).unwrap_or(image);
                    self.record_pcurve(&shape, placed, surface_id, false, range)?;
                }
            }
        }
        Ok(())
    }

    fn image_for(
        &mut self,
        face_id: u64,
        edge_id: u64,
        edge: &Shape,
        curve: &Curve,
        range: (f64, f64),
        surface: &SurfaceGeometry,
    ) -> OgeomResult<Option<PlanarCurve>> {
        // Taken from the table where the pass at the head of the solid
        // already derived it. The fallbacks below serve the faces no pass
        // covered: a face reached outside a solid walk, or one whose
        // preparation refused.
        let prepared = match self.pcurves.remove(&(face_id, edge_id)) {
            Some(prepared) => prepared,
            None => {
                let stated = self.stated_pcurves(face_id, edge_id);
                derive_pcurve(curve, range, surface, &stated, self.tol)
            }
        };
        let (pcurve, off) = match prepared {
            PreparedPcurve::Exact { curve, off } | PreparedPcurve::Stated { curve, off } => {
                (over_range(&curve, range), off)
            }
            PreparedPcurve::Fitted {
                curve: fitted,
                error,
                met,
                worst_off,
                off,
                warning,
            } => {
                if let Some(w) = warning {
                    self.warn_slop(w, worst_off, face_id);
                }
                if !met {
                    self.warn_fit_short(face_id, error);
                }
                (fitted, off.max(worst_off))
            }
            PreparedPcurve::Refused(why) => {
                self.report.warnings.push(format!(
                    "face #{face_id}: no pcurve for an edge on this surface ({why}); \
                     the face may not triangulate"
                ));
                self.note_untrimmed(face_id);
                return Ok(None);
            }
        };
        // The edge provably stands `off` from the face it bounds, whether
        // the curve lies off the surface or a fit strays between its
        // samples; its tolerance grows to cover that, and its vertices
        // follow once the solid is whole. Past the millimetre a file's
        // slop stays under, the pcurve is a fit gone astray (a curve
        // through a pole left whole, whose image leaps across the chart
        // there), and
        // an edge that wide would swallow its neighbours; the face is
        // reported instead.
        if off > self.tol.confusion() * 1e7 {
            self.warn_fit_short(face_id, off);
        } else if off > self.tol.confusion()
            && let Some(node) = self.model.node_mut(edge)
            && let ogeom_topo::NodeData::Edge(data) = node.data_mut()
        {
            data.tolerance = data.tolerance.widen_to(off + self.tol.confusion());
        }
        Ok(Some(pcurve))
    }

    /// The pcurves the file states for an edge on a face's surface. A key
    /// standing for a piece of an edge cut at a pole names no edge in the
    /// file, and the file's pcurves run over the whole edge in any case.
    fn stated_pcurves(&mut self, face_id: u64, edge_id: u64) -> Vec<PlanarCurve> {
        if self.pieces.contains_key(&edge_id) || !self.instances.contains(edge_id) {
            return Vec::new();
        }
        let Some(surface_id) = self
            .face_args(face_id)
            .ok()
            .and_then(|args| args.get(2).and_then(Arg::reference))
        else {
            return Vec::new();
        };
        self.file_pcurves(edge_id, surface_id)
    }

    /// Count one occurrence of a warning kind toward the summary.
    fn tally(&mut self, kind: &'static str, measured: f64, exemplar: u64) {
        let entry = self.tallies.entry(kind).or_insert((0, 0.0, exemplar));
        entry.0 += 1;
        if measured > entry.1 {
            entry.1 = measured;
            entry.2 = exemplar;
        }
    }

    /// A curve end missing its vertex: the prose, and the count.
    fn warn_vertex_miss(&mut self, id: u64, gap: f64) {
        self.report.warnings.push(format!(
            "#{id}: a curve end misses its vertex by {gap:.2e}; the vertex \
             tolerance grew to say so"
        ));
        self.tally("vertex-miss", gap, id);
    }

    /// A fitted trim that stopped short of its target: prose and count.
    fn warn_fit_short(&mut self, face_id: u64, error: f64) {
        self.report.warnings.push(format!(
            "face #{face_id}: a projected pcurve fit stopped at {error:.2e}; \
             the face's mesh may sit that far off along this edge"
        ));
        self.tally("fit-short", error, face_id);
    }

    /// The file's own boundary slop, carried and counted.
    fn warn_slop(&mut self, prose: String, worst: f64, exemplar: u64) {
        self.report.warnings.push(prose);
        self.tally("boundary-slop", worst, exemplar);
    }

    /// Note a face left without a complete trim, once.
    fn note_untrimmed(&mut self, face_id: u64) {
        if self.untrimmed_ids.last() != Some(&face_id) {
            self.untrimmed_ids.push(face_id);
            self.tally("untrimmed", 0.0, face_id);
        }
    }

    /// Attach a seam whose two columns the wire's own walk already found.
    fn record_columns(
        &mut self,
        edge: &Shape,
        forward: PlanarCurve,
        reversed: PlanarCurve,
        surface_id: ogeom_topo::SurfaceId,
        range: (f64, f64),
    ) -> OgeomResult<()> {
        ogeom_algo::attach_seam(
            &mut self.model,
            edge,
            forward,
            reversed,
            surface_id,
            Location::identity(),
            range,
        )
    }

    /// Attach a derived pcurve, seaming it where the edge bounds the chart
    /// twice and the walk could not say where its other column stands.
    fn record_pcurve(
        &mut self,
        edge: &Shape,
        pcurve: PlanarCurve,
        surface_id: ogeom_topo::SurfaceId,
        seam: bool,
        range: (f64, f64),
    ) -> OgeomResult<()> {
        if seam {
            let Some(surface) = self.model.geometry().surface(surface_id).cloned() else {
                ogeom_bail!(Dangling, "the surface is not in this model");
            };
            // One side is where the projection landed; the other is one
            // period over, along the axis the surface actually closes on.
            let other = crate::pcurves::seam_other_side(&pcurve, range, &surface, self.tol)?;
            ogeom_algo::attach_seam(
                &mut self.model,
                edge,
                pcurve,
                other,
                surface_id,
                Location::identity(),
                range,
            )?;
        } else {
            ogeom_algo::attach_pcurve(
                &mut self.model,
                edge,
                pcurve,
                surface_id,
                Location::identity(),
                range,
            )?;
        }
        Ok(())
    }

    /// Derive every pcurve this solid's faces will want, in parallel.
    ///
    /// Deriving one is a pure function of a curve, its range and a surface:
    /// it reads no model and depends on no order, and it is most of the time
    /// spent building solids. The walk that follows attaches them, in file
    /// order.
    ///
    /// Resolving *what* to derive still runs on the walk's own thread, since
    /// it builds edges and vertices into the model. That part is cheap; it is
    /// the projection and the fitting that are not.
    ///
    /// Best-effort by design: anything this cannot resolve is simply left out
    /// of the table, and `chart_bound` derives it on the walk. So a face
    /// shape this does not anticipate costs time, never correctness.
    fn prepare_pcurves(&mut self, face_ids: &[u64]) {
        struct Job {
            face: u64,
            edge: u64,
            curve: Curve,
            range: (f64, f64),
            /// The pcurves the file states for it, tried first.
            stated: Vec<PlanarCurve>,
            /// Index into `surfaces`. The surface is held once per face, not
            /// once per edge: a B-spline patch owns its whole control grid,
            /// and cloning that per edge costs more than the projection it
            /// was cloned for.
            surface: usize,
        }
        let mut surfaces: Vec<SurfaceGeometry> = Vec::new();
        let mut jobs: Vec<Job> = Vec::new();
        let mut seen: FastSet<(u64, u64)> = FastSet::default();
        for &fid in face_ids {
            let Ok(args) = self.face_args(fid) else {
                continue;
            };
            let Some(surface) = args
                .get(2)
                .and_then(Arg::reference)
                .and_then(|sid| self.surface(sid).ok().flatten())
            else {
                continue;
            };
            surfaces.push(surface);
            let at = surfaces.len() - 1;
            let bounds: Vec<u64> = args
                .get(1)
                .and_then(Arg::list)
                .unwrap_or(&[])
                .iter()
                .filter_map(Arg::reference)
                .collect();
            for bound in bounds {
                let Ok((loop_id, _)) = self.bound_args(bound) else {
                    continue;
                };
                let Ok(loop_args) = self.args(loop_id, "EDGE_LOOP") else {
                    continue;
                };
                let uses: Vec<u64> = loop_args
                    .get(1)
                    .and_then(Arg::list)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Arg::reference)
                    .collect();
                for oe_id in uses {
                    let Ok(oargs) = self.args(oe_id, "ORIENTED_EDGE") else {
                        continue;
                    };
                    let Some(edge_id) = oargs.get(3).and_then(Arg::reference) else {
                        continue;
                    };
                    if !seen.insert((fid, edge_id)) {
                        continue;
                    }
                    if let Ok(Some(pieces)) = self.edge_pieces(edge_id) {
                        for (key, (_, curve, range, _)) in pieces {
                            let stated = self.stated_pcurves(fid, key);
                            jobs.push(Job {
                                face: fid,
                                edge: key,
                                curve,
                                range,
                                stated,
                                surface: at,
                            });
                        }
                    }
                }
            }
        }
        // Below a handful of edges the threads cost more than the work; the
        // sequential path through `chart_bound` is already correct, so the
        // table is simply left empty and the walk derives them itself.
        if jobs.len() < 16 {
            return;
        }
        let tol = self.tol;
        let derived = ogeom_core::parallel::map_ordered(&jobs, |_, job| {
            derive_pcurve(
                &job.curve,
                job.range,
                &surfaces[job.surface],
                &job.stated,
                tol,
            )
        });
        for (job, pcurve) in jobs.iter().zip(derived) {
            self.pcurves.insert((job.face, job.edge), pcurve);
        }
    }

    /// The solid a `MANIFOLD_SOLID_BREP` names: its shell's faces, sewn.
    ///
    /// `BREP_WITH_VOIDS` is the same entity with cavities: a subtype of
    /// `MANIFOLD_SOLID_BREP`, so its first two attributes are the name and
    /// the outer shell, and a third names the shells that bound the voids.
    /// A reader matching on the leading keyword alone does not see it, and
    /// the part vanishes from the import. The voids join the solid as
    /// shells of their own, every normal pointing away from the material:
    /// out of the body on the outside, into the cavity within. A void is
    /// oriented by its geometry where its flags
    /// would have it add material.
    fn solid(&mut self, id: u64) -> OgeomResult<Shape> {
        let instance = self.instance(id)?;
        let args = instance
            .part("MANIFOLD_SOLID_BREP")
            .or_else(|| instance.part("BREP_WITH_VOIDS"))
            .ok_or_else(|| ogeom_core::ogeom_err!(Construction, "#{id} is not a solid"))?
            .to_vec();
        let shell_id = args.get(1).and_then(Arg::reference).unwrap_or(0);
        let Some(shell) = self.shell(shell_id)? else {
            ogeom_bail!(Construction, "#{id}: a solid with no readable faces");
        };
        if !ogeom_algo::is_shell_closed(&self.model, &shell)? {
            self.report.warnings.push(format!(
                "#{id}: the shell does not close as read; measures needing an \
                 inside will refuse it"
            ));
        }
        let mut shells = vec![shell];
        for void_id in args
            .get(2)
            .and_then(Arg::list)
            .unwrap_or(&[])
            .iter()
            .filter_map(Arg::reference)
            .collect::<Vec<u64>>()
        {
            match self.shell(void_id) {
                Ok(Some(void)) => {
                    // A void is a cavity whatever its flags say: its faces
                    // point into it, so it encloses negative volume. One a
                    // writer turned twice reads as added material, and is
                    // turned back as it is built.
                    let encloses = ogeom_mesh::triangulate(
                        &self.model,
                        &void,
                        ogeom_mesh::Deflection::default(),
                        self.tol,
                    )
                    .ok()
                    .filter(ogeom_topo::Triangulation::is_closed)
                    .map(|mesh| mesh.volume());
                    if encloses.is_some_and(|v| v > 0.0) {
                        self.report.warnings.push(format!(
                            "#{id}: void shell #{void_id} was written the wrong way \
                             out; it is read as the cavity it bounds"
                        ));
                        shells.push(void.reversed());
                    } else {
                        shells.push(void);
                    }
                }
                Ok(None) => self.report.warnings.push(format!(
                    "#{id}: void shell #{void_id} has no readable faces; the \
                     cavity is missing from the solid"
                )),
                Err(refusal) => self.report.warnings.push(format!(
                    "#{id}: void shell #{void_id} could not be read ({refusal}); \
                     the cavity is missing from the solid"
                )),
            }
        }
        Ok(make_solid(&mut self.model, &shells)?.shape)
    }

    /// The shape a `SHELL_BASED_SURFACE_MODEL` names: each of its shells
    /// sewn from its faces, a compound of them when there are several.
    ///
    /// A surface body is what a modeller exports for a part built from
    /// faces rather than from a solid, often one single-face body per face,
    /// and a reader that walks only `MANIFOLD_SOLID_BREP` leaves such a
    /// part invisible. The
    /// shells stay shells: the file did not call them solids, and a closed
    /// one is still the file's surface model, not this reader's promotion.
    fn surface_model(&mut self, id: u64) -> OgeomResult<Shape> {
        let args = self.args(id, "SHELL_BASED_SURFACE_MODEL")?;
        let shell_ids: Vec<u64> = args
            .get(1)
            .and_then(Arg::list)
            .unwrap_or(&[])
            .iter()
            .filter_map(Arg::reference)
            .collect();
        let mut shells = Vec::new();
        for shell_id in shell_ids {
            if let Some(shell) = self.shell(shell_id)? {
                shells.push(shell);
            }
        }
        match shells.len() {
            0 => ogeom_bail!(
                Construction,
                "#{id}: a surface model with no readable faces"
            ),
            1 => Ok(shells.remove(0)),
            _ => Ok(ogeom_algo::build::make_compound(&mut self.model, &shells)?.shape),
        }
    }

    /// The faces of a `CLOSED_SHELL` or `OPEN_SHELL`, sewn; `None` when not
    /// one of them could be read.
    ///
    /// An `ORIENTED_CLOSED_SHELL` is a use of another shell the other way
    /// round (how a solid's voids are named) and resolves to that shell,
    /// reversed when the use says so.
    fn shell(&mut self, shell_id: u64) -> OgeomResult<Option<Shape>> {
        self.nested(|reader| reader.shell_at(shell_id))
    }

    fn shell_at(&mut self, shell_id: u64) -> OgeomResult<Option<Shape>> {
        let shell_instance = self.instance(shell_id)?;
        if let Some(oriented) = shell_instance.part("ORIENTED_CLOSED_SHELL") {
            let Some(base) = oriented.get(2).and_then(Arg::reference) else {
                ogeom_bail!(Construction, "#{shell_id}: an oriented shell names none");
            };
            let forward = !oriented.get(3).is_some_and(|a| a.is_enum("F"));
            return Ok(self
                .shell(base)?
                .map(|shell| if forward { shell } else { shell.reversed() }));
        }
        let shell_args = shell_instance
            .part("CLOSED_SHELL")
            .or_else(|| shell_instance.part("OPEN_SHELL"))
            .ok_or_else(|| ogeom_core::ogeom_err!(Construction, "#{shell_id} is not a shell"))?
            .to_vec();
        let face_ids: Vec<u64> = shell_args
            .get(1)
            .and_then(Arg::list)
            .unwrap_or(&[])
            .iter()
            .filter_map(Arg::reference)
            .collect();
        self.cut_at_poles(&face_ids);
        self.split_band_rims(&face_ids);
        self.prepare_pcurves(&face_ids);
        let mut read = Vec::new();
        for fid in face_ids {
            if let Some(face) = self.face(fid)? {
                read.push((fid, face));
            }
        }
        if read.is_empty() {
            return Ok(None);
        }
        self.turn_backward_loops(shell_id, &mut read)?;
        let faces: Vec<Shape> = read.into_iter().map(|(_, face)| face).collect();
        Ok(Some(make_shell(&mut self.model, &faces)?.shape))
    }

    /// Walk back the loops of the faces a shell's own edges say run
    /// against their sense.
    ///
    /// Each edge between two faces is walked once each way, every face
    /// keeping its material on the left of its loops. A file whose faces
    /// turned against their surfaces were written with their loops walked
    /// backward has such faces walking every edge the same way as the face
    /// beside them. Where the shell's walks pick out a set of faces whose
    /// loops, walked back, leave every edge walked once each way, and every
    /// face in the set is turned against its surface, those faces are
    /// rebuilt with their loops walked back, and the report says how many.
    /// A shell whose walks pick out no such set is left as read.
    fn turn_backward_loops(&mut self, shell_id: u64, read: &mut [(u64, Shape)]) -> OgeomResult<()> {
        // Each edge's uses: the face, and whether it walks the edge forward.
        let mut uses: FastMap<(ogeom_topo::TShapeId, Location), Vec<(usize, bool)>> =
            FastMap::default();
        for (i, (_, face)) in read.iter().enumerate() {
            for wire in self.model.children_of(face)? {
                for edge in self.model.children_of(&wire)? {
                    if self
                        .model
                        .node(&edge)
                        .and_then(|n| n.data().as_edge())
                        .is_some_and(|d| d.degenerate)
                    {
                        continue;
                    }
                    uses.entry((edge.node(), edge.location().clone()))
                        .or_default()
                        .push((i, edge.orientation() == ogeom_topo::Orientation::Forward));
                }
            }
        }
        // Whether each face's loops must be walked back relative to a
        // reference face of its group: walked the same way by two faces,
        // an edge asks one of the two to turn; once each way, both or
        // neither.
        let pairs: Vec<(usize, usize, bool)> = uses
            .values()
            .filter_map(|u| match u.as_slice() {
                [(a, x), (b, y)] if a != b => Some((*a, *b, x == y)),
                _ => None,
            })
            .collect();
        if !pairs.iter().any(|&(_, _, same)| same) {
            return Ok(());
        }
        let mut beside: Vec<Vec<(usize, bool)>> = vec![Vec::new(); read.len()];
        for &(a, b, same) in &pairs {
            beside[a].push((b, same));
            beside[b].push((a, same));
        }
        let mut turn: Vec<Option<bool>> = vec![None; read.len()];
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for start in 0..read.len() {
            if turn[start].is_some() {
                continue;
            }
            turn[start] = Some(false);
            let mut group = vec![start];
            let mut k = 0;
            while k < group.len() {
                let f = group[k];
                k += 1;
                let mine = turn[f] == Some(true);
                for &(g, same) in &beside[f] {
                    let want = mine ^ same;
                    match turn[g] {
                        None => {
                            turn[g] = Some(want);
                            group.push(g);
                        }
                        Some(had) if had != want => return Ok(()),
                        Some(_) => {}
                    }
                }
            }
            groups.push(group);
        }
        let backward = |i: usize| read[i].1.orientation() == ogeom_topo::Orientation::Reversed;
        let mut walk_back = Vec::new();
        for group in groups {
            let (marked, unmarked): (Vec<usize>, Vec<usize>) =
                group.iter().partition(|&&f| turn[f] == Some(true));
            let candidates = [marked, unmarked];
            let Some(chosen) = candidates
                .iter()
                .filter(|set| set.iter().all(|&f| backward(f)))
                .min_by_key(|set| set.len())
            else {
                return Ok(());
            };
            walk_back.extend(chosen.iter().copied());
        }
        if walk_back.is_empty() {
            return Ok(());
        }
        for &i in &walk_back {
            let (fid, face) = read[i].clone();
            let built = face.oriented(ogeom_topo::Orientation::Forward);
            let Some(surface_id) = self
                .model
                .node(&built)
                .and_then(|n| n.data().as_face())
                .map(|d| d.surface)
            else {
                ogeom_bail!(Construction, "#{fid}: a read face holds no face data");
            };
            let wires = self.model.ordered_children_of(&built)?;
            let wires = walked_back(&mut self.model, &wires, self.tol)?;
            let rebuilt = make_face_on(&mut self.model, surface_id, &wires, self.tol)?.shape;
            let turned = rebuilt.oriented(face.orientation());
            self.faces.insert(fid, turned.clone());
            read[i].1 = turned;
        }
        self.report.warnings.push(format!(
            "#{shell_id}: {} face(s) turned against their surfaces walked their \
             loops backward; their loops were walked back",
            walk_back.len()
        ));
        Ok(())
    }

    // --- product structure, names, colours -----------------------------------

    /// Assemble the document: the model, plus everything the file says about
    /// products, assemblies, placements and appearance.
    ///
    /// Takes the model out of the reader; geometry reading is over by the
    /// time structure is read. Structure that resists becomes a warning and a
    /// flat document, never an error: the geometry is already good, and a
    /// mangled product tree should not take it down.
    fn document(
        &mut self,
        by_item: &FastMap<u64, Shape>,
        solids: &[Shape],
        shells: &[Shape],
    ) -> OgeomResult<ogeom_doc::Document> {
        // The graph is walked before the model moves, because frames scale
        // through the reader's own unit handling.
        let structure = self.product_structure(by_item);
        let colours = self.colours(by_item);
        let pmi = self.pmi_of();

        let mut document = ogeom_doc::Document::over(std::mem::take(&mut self.model));
        match structure {
            Some(products) => self.build_products(&mut document, products),
            None => {
                for (i, solid) in solids.iter().enumerate() {
                    document.add_part(format!("solid-{i}"), solid.clone());
                }
                for (i, shell) in shells.iter().enumerate() {
                    document.add_part(format!("shell-{i}"), shell.clone());
                }
            }
        }
        for (shape, colour) in colours {
            document.set_colour(&shape, colour);
        }
        *document.pmi_mut() = pmi;
        for view in self.views() {
            document.add_view(view);
        }
        Ok(document)
    }

    /// The file's product graph, or `None` when it has none worth the name.
    fn product_structure(&mut self, by_item: &FastMap<u64, Shape>) -> Option<Vec<PdEntry>> {
        // PRODUCT_DEFINITION -> name, via formation and product.
        let mut pds: Vec<u64> = self
            .instances
            .iter()
            .filter(|(_, inst)| inst.part("PRODUCT_DEFINITION").is_some())
            .filter(|(_, inst)| inst.part("PRODUCT_DEFINITION_RELATIONSHIP").is_none())
            .map(|(id, _)| id)
            .collect();
        pds.sort_unstable();
        if pds.is_empty() {
            return None;
        }

        // SHAPE_DEFINITION_REPRESENTATION: definition (a PRODUCT_DEFINITION_SHAPE
        // over a PD or a usage) -> shape representation.
        let mut sr_of_pd: FastMap<u64, u64> = FastMap::default();
        let sdrs: Vec<(u64, u64)> = self
            .instances
            .iter()
            .filter(|(_, inst)| inst.part("SHAPE_DEFINITION_REPRESENTATION").is_some())
            .filter_map(|(id, _)| {
                let args = self.args(id, "SHAPE_DEFINITION_REPRESENTATION").ok()?;
                Some((args.first()?.reference()?, args.get(1)?.reference()?))
            })
            .collect();
        for (pds_id, sr) in sdrs {
            if let Some(definition) = self.definition_of_shape(pds_id) {
                sr_of_pd.insert(definition, sr);
            }
        }

        // A product's solids may live one representation over: AP203 files
        // routinely tie the product to a bare axis representation and hang
        // the B-rep off it through a plain SHAPE_REPRESENTATION_RELATIONSHIP.
        // Only the plain ones are followed; the transformation-carrying kind
        // is an assembly edge, and following it would leak one product's
        // geometry into another.
        let mut linked: FastMap<u64, Vec<u64>> = FastMap::default();
        let mut srrs: Vec<u64> = self
            .instances
            .iter()
            .filter(|(_, inst)| {
                inst.part("SHAPE_REPRESENTATION_RELATIONSHIP").is_some()
                    && inst
                        .part("REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION")
                        .is_none()
            })
            .map(|(id, _)| id)
            .collect();
        srrs.sort_unstable();
        for srr in srrs {
            let args = {
                let Ok(instance) = self.instance(srr) else {
                    continue;
                };
                let Some(args) = instance
                    .part("SHAPE_REPRESENTATION_RELATIONSHIP")
                    .or_else(|| instance.part("REPRESENTATION_RELATIONSHIP"))
                else {
                    continue;
                };
                args.to_vec()
            };
            if let (Some(a), Some(b)) = (
                args.get(2).and_then(Arg::reference),
                args.get(3).and_then(Arg::reference),
            ) {
                linked.entry(a).or_default().push(b);
                linked.entry(b).or_default().push(a);
            }
        }

        let mut entries = Vec::new();
        for pd in pds {
            let name = self.product_name(pd).unwrap_or_else(|| format!("#{pd}"));
            let mut shapes: Vec<Shape> = Vec::new();
            if let Some(&sr) = sr_of_pd.get(&pd) {
                let mut reps = vec![sr];
                reps.extend(linked.get(&sr).into_iter().flatten().copied());
                for rep in reps {
                    if let Some(items) = self.representation_items(rep) {
                        shapes.extend(items.iter().filter_map(|item| by_item.get(item).cloned()));
                    }
                }
            }
            entries.push(PdEntry {
                pd,
                name,
                shapes,
                children: Vec::new(),
            });
        }

        // NEXT_ASSEMBLY_USAGE_OCCURRENCE: parent -> child, with the placement
        // recovered from the CONTEXT_DEPENDENT_SHAPE_REPRESENTATION over it.
        let mut nauos: Vec<u64> = self
            .instances
            .iter()
            .filter(|(_, inst)| inst.part("NEXT_ASSEMBLY_USAGE_OCCURRENCE").is_some())
            .map(|(id, _)| id)
            .collect();
        nauos.sort_unstable();
        let entry_of_pd: FastMap<u64, usize> =
            entries.iter().enumerate().map(|(i, e)| (e.pd, i)).collect();
        for nauo in nauos {
            let Ok(args) = self.args(nauo, "NEXT_ASSEMBLY_USAGE_OCCURRENCE") else {
                continue;
            };
            let (Some(parent), Some(child)) = (
                args.get(3).and_then(Arg::reference),
                args.get(4).and_then(Arg::reference),
            ) else {
                continue;
            };
            let name = args
                .get(5)
                .and_then(|a| match a {
                    Arg::Str(s) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                })
                .or_else(|| {
                    self.args(nauo, "NEXT_ASSEMBLY_USAGE_OCCURRENCE")
                        .ok()
                        .and_then(|args| match args.get(1) {
                            Some(Arg::Str(s)) if !s.is_empty() => Some(s.clone()),
                            _ => None,
                        })
                });
            let child_sr = sr_of_pd.get(&child).copied();
            let at = self
                .usage_transform(nauo, child_sr)
                .unwrap_or(Transform::IDENTITY);
            if let Some(&at_index) = entry_of_pd.get(&parent) {
                entries[at_index].children.push((child, at, name));
            }
        }
        Some(entries)
    }

    /// The `PRODUCT_DEFINITION` (or usage) a `PRODUCT_DEFINITION_SHAPE` is over.
    fn definition_of_shape(&mut self, pds_id: u64) -> Option<u64> {
        let args = self.args(pds_id, "PRODUCT_DEFINITION_SHAPE").ok()?;
        args.get(2).and_then(Arg::reference)
    }

    /// A product definition's product name.
    fn product_name(&mut self, pd: u64) -> Option<String> {
        let formation = self
            .args(pd, "PRODUCT_DEFINITION")
            .ok()?
            .get(2)
            .and_then(Arg::reference)?;
        // The formation, plain or with its source named (a mesh converter's
        // habit, and a modeller's for an assembly's parts) carries the
        // product in its third slot either way, whether it stands alone or
        // as one part of a complex instance.
        let product = [
            "PRODUCT_DEFINITION_FORMATION",
            "PRODUCT_DEFINITION_FORMATION_WITH_SPECIFIED_SOURCE",
        ]
        .into_iter()
        .find_map(|keyword| {
            self.instance(formation)
                .ok()?
                .part(keyword)
                .or_else(|| self.args(formation, keyword).ok())?
                .get(2)
                .and_then(Arg::reference)
        })?;
        // The product's name, or its id where the name is blank: a mesh
        // converter fills the id and leaves the name empty.
        let args = self.args(product, "PRODUCT").ok()?;
        [args.get(1), args.first()]
            .into_iter()
            .find_map(|arg| match arg {
                Some(Arg::Str(name)) if !name.is_empty() => Some(name.clone()),
                _ => None,
            })
    }

    /// A representation's item references.
    fn representation_items(&mut self, sr: u64) -> Option<Vec<u64>> {
        let instance = self.instance(sr).ok()?;
        // Every subtype a file names a shape representation by: the B-rep
        // kinds, the surface-model kinds a mesh converter writes, and the
        // plain one.
        let args = [
            "SHAPE_REPRESENTATION",
            "ADVANCED_BREP_SHAPE_REPRESENTATION",
            "MANIFOLD_SURFACE_SHAPE_REPRESENTATION",
            "FACETED_BREP_SHAPE_REPRESENTATION",
            "GEOMETRICALLY_BOUNDED_SURFACE_SHAPE_REPRESENTATION",
            "GEOMETRICALLY_BOUNDED_WIREFRAME_SHAPE_REPRESENTATION",
            "REPRESENTATION",
        ]
        .into_iter()
        .find_map(|keyword| instance.part(keyword))?
        .to_vec();
        Some(
            args.get(1)?
                .list()?
                .iter()
                .filter_map(Arg::reference)
                .collect(),
        )
    }

    /// The placement a usage's `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION` states.
    ///
    /// The transformation aligns an axis placement in the child's space with
    /// one in the parent's; which item is which follows from which side of
    /// the representation relationship is the child's own shape
    /// representation, not from argument order, because real files disagree
    /// about the order.
    fn usage_transform(&mut self, nauo: u64, child_sr: Option<u64>) -> Option<Transform> {
        if self.cdsr_of_nauo.is_none() {
            // One pass over the CDSRs, each resolved to the usage it
            // describes, in ascending id order, so a usage described twice
            // keeps the lowest-id one.
            let mut cdsrs: Vec<u64> = self
                .instances
                .iter()
                .filter(|(_, inst)| {
                    inst.part("CONTEXT_DEPENDENT_SHAPE_REPRESENTATION")
                        .is_some()
                })
                .map(|(id, _)| id)
                .collect();
            cdsrs.sort_unstable();
            let mut index: FastMap<u64, u64> = FastMap::default();
            for id in cdsrs {
                let Some(owner) = self
                    .args(id, "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION")
                    .ok()
                    .and_then(|args| args.get(1).and_then(Arg::reference))
                    .and_then(|pds| self.definition_of_shape(pds))
                else {
                    continue;
                };
                index.entry(owner).or_insert(id);
            }
            self.cdsr_of_nauo = Some(index);
        }
        let cdsr = *self.cdsr_of_nauo.as_ref()?.get(&nauo)?;
        let rr = self
            .args(cdsr, "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION")
            .ok()?
            .first()
            .and_then(Arg::reference)?;
        let (rep_1, rep_2) = {
            let args = self.args(rr, "REPRESENTATION_RELATIONSHIP").ok()?;
            (
                args.get(2).and_then(Arg::reference)?,
                args.get(3).and_then(Arg::reference)?,
            )
        };
        let idt = self
            .args(rr, "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION")
            .ok()?
            .first()
            .and_then(Arg::reference)?;
        let (item_1, item_2) = {
            let args = self.args(idt, "ITEM_DEFINED_TRANSFORMATION").ok()?;
            (
                args.get(2).and_then(Arg::reference)?,
                args.get(3).and_then(Arg::reference)?,
            )
        };
        let frame_1 = self.frame(item_1).ok()?;
        let frame_2 = self.frame(item_2).ok()?;
        // item_1 pairs with rep_1. When rep_1 is the child's representation,
        // the child's frame_1 lands on the parent's frame_2.
        let child_first = match child_sr {
            Some(sr) => rep_1 == sr || rep_2 != sr,
            None => true,
        };
        Some(if child_first {
            Transform::from_frame(&frame_2) * Transform::to_frame(&frame_1)
        } else {
            Transform::from_frame(&frame_1) * Transform::to_frame(&frame_2)
        })
    }

    /// Products into the document: assemblies for the parents, parts for the
    /// shaped, instances for the usage edges.
    fn build_products(&mut self, document: &mut ogeom_doc::Document, entries: Vec<PdEntry>) {
        let mut ids: FastMap<u64, ogeom_doc::ProductId> = FastMap::default();
        for entry in &entries {
            let shape = match entry.shapes.len() {
                0 => None,
                1 => Some(entry.shapes[0].clone()),
                _ => match ogeom_algo::build::make_compound(document.model_mut(), &entry.shapes) {
                    Ok(built) => Some(built.shape),
                    Err(_) => Some(entry.shapes[0].clone()),
                },
            };
            if entry.children.is_empty() {
                if let Some(shape) = shape {
                    ids.insert(entry.pd, document.add_part(&entry.name, shape));
                }
                // A product with neither shape nor children holds nothing a
                // document can say; it is left out.
            } else {
                let assembly = document.add_assembly(&entry.name);
                ids.insert(entry.pd, assembly);
                // An assembly with its own geometry keeps it as a body part
                // placed at identity: rare, but files do it.
                if let Some(shape) = shape {
                    let body = document.add_part(format!("{}-body", entry.name), shape);
                    let _ = document.add_instance(assembly, body, Transform::IDENTITY, None);
                }
            }
        }
        for entry in &entries {
            let Some(&parent) = ids.get(&entry.pd) else {
                continue;
            };
            for (child, at, name) in &entry.children {
                let Some(&child_id) = ids.get(child) else {
                    self.report.warnings.push(format!(
                        "#{child}: an assembly child holds nothing readable"
                    ));
                    continue;
                };
                if let Err(e) = document.add_instance(parent, child_id, *at, name.clone()) {
                    self.report
                        .warnings
                        .push(format!("assembly edge #{} -> #{child}: {e}", entry.pd));
                }
            }
        }
    }

    /// Colours from styled items, keyed to the shapes they style.
    fn colours(&mut self, by_item: &FastMap<u64, Shape>) -> Vec<(Shape, ogeom_doc::Colour)> {
        let mut styled: Vec<u64> = self
            .instances
            .iter()
            .filter(|(_, inst)| {
                inst.part("STYLED_ITEM").is_some() || inst.part("OVER_RIDING_STYLED_ITEM").is_some()
            })
            .map(|(id, _)| id)
            .collect();
        // Sorted so an item styled twice resolves the same way every run;
        // overriding styles carry higher instance numbers in practice, and a
        // later same-shape entry wins the map insertion downstream.
        styled.sort_unstable();
        let mut out = Vec::new();
        for id in styled {
            let args = {
                let Ok(instance) = self.instance(id) else {
                    continue;
                };
                let Some(args) = instance
                    .part("STYLED_ITEM")
                    .or_else(|| instance.part("OVER_RIDING_STYLED_ITEM"))
                else {
                    continue;
                };
                args.to_vec()
            };
            let Some(item) = args.get(2).and_then(Arg::reference) else {
                continue;
            };
            let shapes = match by_item.get(&item).or_else(|| self.faces.get(&item)) {
                Some(shape) => vec![shape.clone()],
                None => self.edge_shapes(item),
            };
            if shapes.is_empty() {
                continue;
            }
            let styles: Vec<u64> = args
                .get(1)
                .and_then(Arg::list)
                .map(|list| list.iter().filter_map(Arg::reference).collect())
                .unwrap_or_default();
            if let Some(colour) = styles.iter().find_map(|&style| self.colour_in(style, 0)) {
                out.extend(shapes.into_iter().map(|shape| (shape, colour)));
            }
        }
        out
    }

    /// The first `COLOUR_RGB` reachable from a presentation style, depth-bounded.
    ///
    /// The styled-item chain has five links and real files rearrange them, so
    /// the walk follows references rather than the textbook path.
    fn colour_in(&mut self, id: u64, depth: usize) -> Option<ogeom_doc::Colour> {
        if depth > 6 {
            return None;
        }
        let (keyword, args) = {
            let instance = self.instance(id).ok()?;
            let all: Vec<Arg> = instance
                .parts()
                .flat_map(|(_, args)| args.iter().cloned())
                .collect();
            (instance.keyword().to_owned(), all)
        };
        if keyword == "COLOUR_RGB" {
            let channel = |i: usize| args.get(i).and_then(Arg::number);
            return Some(ogeom_doc::Colour::rgb(
                channel(1)?,
                channel(2)?,
                channel(3)?,
            ));
        }
        let mut refs: Vec<u64> = Vec::new();
        collect_refs(&args, &mut refs);
        refs.into_iter()
            .find_map(|next| self.colour_in(next, depth + 1))
    }

    // --- semantic PMI --------------------------------------------------------

    /// The file's semantic PMI: dimensions, geometric tolerances, datums.
    ///
    /// Annotations that resist stay out with a warning; PMI never takes the
    /// geometry down.
    fn pmi_of(&mut self) -> ogeom_doc::Pmi {
        let mut pmi = ogeom_doc::Pmi::new();
        // Which STEP instance each annotation came from, so the presentation
        // pass can name the annotation a callout draws exactly rather than by
        // matching a string two annotations may share.
        let mut annotation_ids: FastMap<u64, ogeom_doc::Annotated> = FastMap::default();

        // Which topology each shape aspect describes: directly through
        // GEOMETRIC_ITEM_SPECIFIC_USAGE, and one relationship step outward,
        // because composite aspects hold their pieces through relationships.
        let mut aspect_items: FastMap<u64, Vec<ogeom_topo::TShapeId>> = FastMap::default();
        let mut gisus = self.ids_with("GEOMETRIC_ITEM_SPECIFIC_USAGE");
        gisus.sort_unstable();
        for id in gisus {
            let Ok(args) = self.args(id, "GEOMETRIC_ITEM_SPECIFIC_USAGE") else {
                continue;
            };
            let (Some(aspect), Some(item)) = (
                args.get(2).and_then(Arg::reference),
                args.get(4).and_then(Arg::reference),
            ) else {
                continue;
            };
            let nodes = match self.faces.get(&item) {
                Some(face) => vec![face.node()],
                None => self.edge_shapes(item).iter().map(Shape::node).collect(),
            };
            if !nodes.is_empty() {
                aspect_items.entry(aspect).or_default().extend(nodes);
            }
        }
        let mut adjacency: FastMap<u64, Vec<u64>> = FastMap::default();
        for id in self.ids_with("SHAPE_ASPECT_RELATIONSHIP") {
            let Ok(args) = self.args(id, "SHAPE_ASPECT_RELATIONSHIP") else {
                continue;
            };
            if let (Some(a), Some(b)) = (
                args.get(2).and_then(Arg::reference),
                args.get(3).and_then(Arg::reference),
            ) {
                adjacency.entry(a).or_default().push(b);
                adjacency.entry(b).or_default().push(a);
            }
        }
        let items_for = |aspect: u64| -> Vec<ogeom_topo::TShapeId> {
            // Three relationship steps: a composite aspect holds components,
            // a derived aspect sits behind a composite, and a datum one link
            // behind its features: the deepest chain real files exhibit.
            let mut reach = vec![aspect];
            for _ in 0..3 {
                let mut next = reach.clone();
                for a in &reach {
                    next.extend(adjacency.get(a).into_iter().flatten().copied());
                }
                next.sort_unstable();
                next.dedup();
                reach = next;
            }
            let mut out: Vec<ogeom_topo::TShapeId> = Vec::new();
            for a in reach {
                out.extend(aspect_items.get(&a).into_iter().flatten().copied());
            }
            out.sort_unstable();
            out.dedup();
            out
        };

        // Dimensions: characteristic -> representation, values from the
        // measure items, bounds from any plus/minus tolerance over the same
        // characteristic.
        let mut plus_minus: FastMap<u64, (Option<f64>, Option<f64>)> = FastMap::default();
        for id in self.ids_with("PLUS_MINUS_TOLERANCE") {
            let Ok(args) = self.args(id, "PLUS_MINUS_TOLERANCE") else {
                continue;
            };
            let (Some(tv), Some(dim)) = (
                args.first().and_then(Arg::reference),
                args.get(1).and_then(Arg::reference),
            ) else {
                continue;
            };
            let Ok(tv_args) = self.args(tv, "TOLERANCE_VALUE") else {
                continue;
            };
            let lower = tv_args
                .first()
                .and_then(Arg::reference)
                .and_then(|r| self.measure_value(r))
                .map(|(v, _)| v);
            let upper = tv_args
                .get(1)
                .and_then(Arg::reference)
                .and_then(|r| self.measure_value(r))
                .map(|(v, _)| v);
            plus_minus.insert(dim, (lower, upper));
        }
        let mut dcrs = self.ids_with("DIMENSIONAL_CHARACTERISTIC_REPRESENTATION");
        dcrs.sort_unstable();
        for id in dcrs {
            let Ok(args) = self.args(id, "DIMENSIONAL_CHARACTERISTIC_REPRESENTATION") else {
                continue;
            };
            let (Some(dim), Some(sdr)) = (
                args.first().and_then(Arg::reference),
                args.get(1).and_then(Arg::reference),
            ) else {
                continue;
            };
            let mut values = Vec::new();
            let mut kind = ogeom_doc::MeasureKind::Length;
            if let Ok(sdr_args) = self.args(sdr, "SHAPE_DIMENSION_REPRESENTATION") {
                for item in sdr_args.get(1).and_then(Arg::list).unwrap_or(&[]) {
                    if let Some(r) = item.reference()
                        && let Some((v, k)) = self.measure_value(r)
                    {
                        values.push(v);
                        kind = k;
                    }
                }
            }
            let (name, location, aspects) = self.dimension_shape(dim);
            let features: Vec<Vec<ogeom_topo::TShapeId>> =
                aspects.into_iter().map(&items_for).collect();
            let (minus, plus) = plus_minus.get(&dim).copied().unwrap_or((None, None));
            annotation_ids.insert(dim, ogeom_doc::Annotated::Dimension(pmi.dimensions.len()));
            pmi.dimensions.push(ogeom_doc::Dimension {
                name,
                values,
                kind,
                plus,
                minus,
                features,
                location,
            });
        }

        // Geometric tolerances: any instance whose subtype names one. The
        // complex form keeps its attributes on the GEOMETRIC_TOLERANCE part;
        // the simple form flattens them into the subtype's own list.
        let is_subtype = |k: &str| {
            k.ends_with("_TOLERANCE")
                && !matches!(
                    k,
                    "GEOMETRIC_TOLERANCE"
                        | "GEOMETRIC_TOLERANCE_WITH_DATUM_REFERENCE"
                        | "GEOMETRIC_TOLERANCE_WITH_DEFINED_UNIT"
                        | "GEOMETRIC_TOLERANCE_WITH_MODIFIERS"
                        | "GEOMETRIC_TOLERANCE_WITH_MAXIMUM_TOLERANCE"
                        | "PLUS_MINUS_TOLERANCE"
                        | "TOLERANCE_VALUE"
                )
        };
        let mut gts: Vec<u64> = self
            .instances
            .iter()
            .filter(|(_, inst)| inst.parts().any(|(k, _)| is_subtype(k)))
            .map(|(id, _)| id)
            .collect();
        gts.sort_unstable();
        for id in gts {
            let (name, magnitude_ref, aspect, kind, modifiers, datum_refs) = {
                let Ok(instance) = self.instance(id) else {
                    continue;
                };
                let subtype = instance
                    .parts()
                    .map(|(k, _)| k.to_owned())
                    .find(|k| is_subtype(k));
                let Some(subtype) = subtype else {
                    continue;
                };
                let base = instance
                    .part("GEOMETRIC_TOLERANCE")
                    .or_else(|| instance.part(&subtype))
                    .unwrap_or(&[])
                    .to_vec();
                let name = match base.first() {
                    Some(Arg::Str(s)) => s.clone(),
                    _ => String::new(),
                };
                let magnitude_ref = base.get(2).and_then(Arg::reference);
                let aspect = base.get(3).and_then(Arg::reference);
                let kind = Some(subtype.trim_end_matches("_TOLERANCE").to_lowercase());
                // Modifiers ride on their own part in the complex form, as
                // a list of enumeration words.
                let modifiers: Vec<String> = instance
                    .part("GEOMETRIC_TOLERANCE_WITH_MODIFIERS")
                    .and_then(|args| args.first())
                    .and_then(Arg::list)
                    .map(|list| {
                        list.iter()
                            .filter_map(|arg| match arg {
                                Arg::Enum(word) => Some(word.to_lowercase()),
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // The complex form keeps the datum list on its own part;
                // the simple form appends it as the subtype's fifth argument.
                let datum_refs: Vec<u64> = instance
                    .part("GEOMETRIC_TOLERANCE_WITH_DATUM_REFERENCE")
                    .and_then(|args| args.first())
                    .and_then(Arg::list)
                    .or_else(|| base.get(4).and_then(Arg::list))
                    .map(|list| list.iter().filter_map(Arg::reference).collect())
                    .unwrap_or_default();
                (name, magnitude_ref, aspect, kind, modifiers, datum_refs)
            };
            let Some(kind) = kind else {
                continue;
            };
            let magnitude = magnitude_ref
                .and_then(|r| self.measure_value(r))
                .map_or(0.0, |(v, _)| v);
            let datums: Vec<String> = datum_refs
                .iter()
                .filter_map(|&r| self.datum_letter(r, 0))
                .collect();
            let items = aspect.map(items_for).unwrap_or_default();
            annotation_ids.insert(id, ogeom_doc::Annotated::Tolerance(pmi.tolerances.len()));
            pmi.tolerances.push(ogeom_doc::GeometricTolerance {
                kind,
                name,
                magnitude,
                modifiers,
                datums,
                items,
            });
        }

        // Datums: the letters, with the features they mark reached through
        // the aspect graph.
        let mut datums = self.ids_with("DATUM");
        datums.sort_unstable();
        for id in datums {
            let letter = {
                let Ok(instance) = self.instance(id) else {
                    continue;
                };
                if instance.part("DATUM_FEATURE").is_some()
                    || instance.part("DATUM_REFERENCE").is_some()
                    || instance.part("DATUM_REFERENCE_COMPARTMENT").is_some()
                    || instance.part("DATUM_SYSTEM").is_some()
                {
                    continue;
                }
                let Some(args) = instance.part("DATUM") else {
                    continue;
                };
                match args.get(4) {
                    Some(Arg::Str(s)) if !s.is_empty() => s.clone(),
                    _ => continue,
                }
            };
            annotation_ids.insert(id, ogeom_doc::Annotated::Datum(pmi.datums.len()));
            pmi.datums.push(ogeom_doc::Datum {
                label: letter,
                items: items_for(id),
            });
        }

        // Datum targets: the pads a datum is actually established at. The
        // target's identifier is the letter's number (`A1` is target 1 of
        // datum A), and its placement and size come through the shape
        // representation the feature is associated with.
        let mut targets = self.ids_with("PLACED_DATUM_TARGET_FEATURE");
        targets.sort_unstable();
        for id in targets {
            if let Some(target) = self.datum_target(id, &items_for) {
                pmi.targets.push(target);
            }
        }

        // Presentation: what a viewer draws. A callout holds tessellated
        // annotation occurrences, each an indexed set of polylines over a
        // coordinates list; an annotation plane says which plane they are
        // drawn in and which callouts it holds; and a model item association
        // says which semantic annotation a callout is the picture of.
        let (callouts, callout_index) = self.callouts(&annotation_ids);
        pmi.callouts = callouts;
        self.callout_index = callout_index;
        pmi
    }

    /// Saved views: every *named* draughting model is one; the unnamed one
    /// is the annotation-plane container this writer emits itself. The
    /// camera item gives the frame; the callout items give the subset.
    fn views(&mut self) -> Vec<ogeom_doc::View> {
        let mut out = Vec::new();
        for id in self.ids_with("DRAUGHTING_MODEL") {
            let Ok(args) = self.args(id, "DRAUGHTING_MODEL") else {
                continue;
            };
            let name = match args.first() {
                Some(Arg::Str(text)) => text.clone(),
                _ => String::new(),
            };
            if name.is_empty() {
                continue;
            }
            let mut frame = None;
            let mut callouts = Vec::new();
            for item in args.get(1).and_then(Arg::list).unwrap_or(&[]) {
                let Some(item) = item.reference() else {
                    continue;
                };
                if let Ok(cam) = self.args(item, "CAMERA_MODEL_D3") {
                    if let Some(placement) = cam.get(1).and_then(Arg::reference) {
                        frame = self.frame(placement).ok();
                    }
                    continue;
                }
                if let Some(&index) = self.callout_index.get(&item) {
                    callouts.push(index);
                }
            }
            let Some(frame) = frame else { continue };
            out.push(ogeom_doc::View {
                name,
                frame,
                clipping: None,
                callouts,
            });
        }
        out
    }

    /// One placed datum target: which datum, which number, where and how big.
    fn datum_target(
        &mut self,
        id: u64,
        items_for: &dyn Fn(u64) -> Vec<ogeom_topo::TShapeId>,
    ) -> Option<ogeom_doc::DatumTarget> {
        let (target_id, description) = {
            let instance = self.instance(id).ok()?;
            let args = instance.part("PLACED_DATUM_TARGET_FEATURE")?;
            let target = match args.get(4) {
                Some(Arg::Str(s)) if !s.is_empty() => s.clone(),
                _ => return None,
            };
            let description = match args.get(1) {
                Some(Arg::Str(s)) => s.to_ascii_lowercase(),
                _ => String::new(),
            };
            (target, description)
        };
        // `A1`: the letter is the datum, the digits its number.
        let split = target_id
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(target_id.len());
        let (letter, number) = target_id.split_at(split);
        let index = number.parse::<u32>().unwrap_or(1);
        let datum = if letter.is_empty() {
            self.datum_letter(id, 0).unwrap_or_default()
        } else {
            letter.to_owned()
        };

        // The target's placement and size live in a shape representation the
        // target's own property definition names. The lengths come in the
        // file's own unit, as every length does. Both hops go through the
        // indexes rather than a scan of every entity per property *per
        // target*.
        self.ensure_property_indexes();
        let mut frame = None;
        let mut lengths: Vec<f64> = Vec::new();
        let properties: Vec<u64> = self
            .properties_of_definition
            .as_ref()
            .and_then(|m| m.get(&id).cloned())
            .unwrap_or_default();
        for property in properties {
            let sdrs: Vec<u64> = self
                .sdrs_of_property
                .as_ref()
                .and_then(|m| m.get(&property).cloned())
                .unwrap_or_default();
            for sdr in sdrs {
                let Ok(args) = self.args(sdr, "SHAPE_DEFINITION_REPRESENTATION") else {
                    continue;
                };
                let Some(rep) = args.get(1).and_then(Arg::reference) else {
                    continue;
                };
                for item in self.representation_items(rep).unwrap_or_default() {
                    let keyword = self
                        .instance(item)
                        .map(|i| i.keyword().to_owned())
                        .unwrap_or_default();
                    match keyword.as_str() {
                        "AXIS2_PLACEMENT_3D" => frame = self.frame(item).ok(),
                        "LENGTH_MEASURE_WITH_UNIT" | "MEASURE_REPRESENTATION_ITEM" => {
                            if let Some((value, _)) = self.measure_value(item) {
                                lengths.push(value);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // What the target *is* follows from the description the file gives
        // and how many sizes it carries: an area needs two, a circle one, a
        // line one, a point none.
        let kind = if description.contains("circle") || description.contains("circular") {
            ogeom_doc::DatumTargetKind::Circle {
                diameter: lengths.first().copied().unwrap_or(0.0),
            }
        } else if description.contains("rectangle") || lengths.len() >= 2 {
            ogeom_doc::DatumTargetKind::Rectangle {
                length: lengths.first().copied().unwrap_or(0.0),
                width: lengths.get(1).copied().unwrap_or(0.0),
            }
        } else if description.contains("line") || lengths.len() == 1 {
            ogeom_doc::DatumTargetKind::Line {
                length: lengths.first().copied().unwrap_or(0.0),
            }
        } else {
            ogeom_doc::DatumTargetKind::Point
        };
        Some(ogeom_doc::DatumTarget {
            datum,
            index,
            kind,
            at: frame.map_or(Point::ORIGIN, |f: Frame| f.origin()),
            frame,
            items: items_for(id),
        })
    }

    /// The drawn annotations: callouts, their polylines, their planes, and
    /// which semantic annotation each one draws.
    fn callouts(
        &mut self,
        annotation_ids: &FastMap<u64, ogeom_doc::Annotated>,
    ) -> (Vec<ogeom_doc::Callout>, FastMap<u64, usize>) {
        // Which callout each annotation plane holds, and the plane's frame.
        let mut plane_of: FastMap<u64, Frame> = FastMap::default();
        for id in self.ids_with("ANNOTATION_PLANE") {
            let Ok(args) = self.args(id, "ANNOTATION_PLANE") else {
                continue;
            };
            let frame = args
                .get(2)
                .and_then(Arg::reference)
                .and_then(|plane| {
                    let inner = self.args(plane, "PLANE").ok()?;
                    inner.get(1).and_then(Arg::reference)
                })
                .and_then(|placement| self.frame(placement).ok());
            let Some(frame) = frame else { continue };
            for element in args.get(3).and_then(Arg::list).unwrap_or(&[]) {
                if let Some(callout) = element.reference() {
                    plane_of.insert(callout, frame);
                }
            }
        }

        // Which semantic annotation each callout draws. The association names
        // the annotation's own STEP id, so the link is made by matching that
        // against the ids the semantic pass already resolved.
        // A callout may be associated more than once (once with the shape
        // aspect the annotation is *about*, once with the annotation itself),
        // so every association is kept and the first that resolves to a
        // semantic annotation is the answer.
        let mut draws: FastMap<u64, Vec<u64>> = FastMap::default();
        let mut associations = self.ids_with("DRAUGHTING_MODEL_ITEM_ASSOCIATION");
        associations.sort_unstable();
        for id in associations {
            let Ok(args) = self.args(id, "DRAUGHTING_MODEL_ITEM_ASSOCIATION") else {
                continue;
            };
            if let (Some(definition), Some(item)) = (
                args.get(2).and_then(Arg::reference),
                args.get(4).and_then(Arg::reference),
            ) {
                draws.entry(item).or_default().push(definition);
            }
        }

        let mut out = Vec::new();
        let mut index_of: FastMap<u64, usize> = FastMap::default();
        let mut callouts = self.ids_with("DRAUGHTING_CALLOUT");
        callouts.sort_unstable();
        for id in callouts {
            let Ok(args) = self.args(id, "DRAUGHTING_CALLOUT") else {
                continue;
            };
            let name = match args.first() {
                Some(Arg::Str(s)) => s.clone(),
                _ => String::new(),
            };
            let mut polylines = Vec::new();
            for element in args.get(1).and_then(Arg::list).unwrap_or(&[]).to_vec() {
                let Some(occurrence) = element.reference() else {
                    continue;
                };
                polylines.extend(self.annotation_polylines(occurrence));
            }
            if polylines.is_empty() && !plane_of.contains_key(&id) {
                continue;
            }
            index_of.insert(id, out.len());
            out.push(ogeom_doc::Callout {
                name,
                plane: plane_of.get(&id).copied(),
                polylines,
                annotates: draws
                    .get(&id)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .find_map(|d| annotation_ids.get(&d).copied()),
            });
        }
        (out, index_of)
    }

    /// The polylines one annotation occurrence draws.
    ///
    /// A tessellated occurrence names a curve set, which names a coordinates
    /// list and gives each polyline as *one-based* indices into it. That is
    /// the whole of the drawn geometry, and it is read as it is written
    /// rather than resampled.
    fn annotation_polylines(&mut self, occurrence: u64) -> Vec<Vec<Point>> {
        let item = {
            let Ok(instance) = self.instance(occurrence) else {
                return Vec::new();
            };
            let args = instance
                .part("TESSELLATED_ANNOTATION_OCCURRENCE")
                .or_else(|| instance.part("ANNOTATION_OCCURRENCE"))
                .or_else(|| instance.part("STYLED_ITEM"));
            match args.and_then(|a| a.get(2).and_then(Arg::reference)) {
                Some(item) => item,
                None => return Vec::new(),
            }
        };
        self.tessellated_polylines(item, 0)
    }

    /// The polylines a tessellated item holds, however deep it nests them.
    ///
    /// The drawn geometry of one annotation is a *set*: a frame's box, its
    /// leader, its text strokes, each its own curve set over its own
    /// coordinates list, gathered under one item, which may itself be
    /// repositioned by a placement, and that placement is applied here rather
    /// than left for a consumer to discover.
    fn tessellated_polylines(&mut self, item: u64, depth: usize) -> Vec<Vec<Point>> {
        if depth > 4 {
            return Vec::new();
        }
        let (children, curve_set, placement) = {
            let Ok(instance) = self.instance(item) else {
                return Vec::new();
            };
            let children: Vec<u64> = instance
                .part("TESSELLATED_GEOMETRIC_SET")
                .and_then(|args| args.first().and_then(Arg::list))
                .map(|list| list.iter().filter_map(Arg::reference).collect())
                .unwrap_or_default();
            let curve_set = instance.part("TESSELLATED_CURVE_SET");
            let placement = instance
                .part("REPOSITIONED_TESSELLATED_ITEM")
                .and_then(|args| args.first().and_then(Arg::reference));
            (children, curve_set, placement)
        };

        let mut out = Vec::new();
        for child in children {
            out.extend(self.tessellated_polylines(child, depth + 1));
        }
        if let Some(args) = curve_set
            && let Some(list) = args.get(1).and_then(Arg::reference)
        {
            let points = self.coordinates_list(list);
            for line in args.get(2).and_then(Arg::list).unwrap_or(&[]) {
                let Some(indices) = line.list() else { continue };
                let mut polyline = Vec::with_capacity(indices.len());
                for index in indices {
                    // One-based, as the format states them.
                    let Some(k) = index.number() else { continue };
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "an index into a list the file itself sized"
                    )]
                    let k = k as usize;
                    if k >= 1 && k <= points.len() {
                        polyline.push(points[k - 1]);
                    }
                }
                if polyline.len() >= 2 {
                    out.push(polyline);
                }
            }
        }
        if let Some(placement) = placement
            && let Ok(frame) = self.frame(placement)
        {
            for polyline in &mut out {
                for p in polyline.iter_mut() {
                    *p = frame.to_world(*p);
                }
            }
        }
        out
    }

    /// A `COORDINATES_LIST`'s points, in the document's own length unit.
    fn coordinates_list(&mut self, id: u64) -> Vec<Point> {
        let Ok(args) = self.args(id, "COORDINATES_LIST") else {
            return Vec::new();
        };
        let scale = self.report.scale_mm;
        args.get(2)
            .and_then(Arg::list)
            .unwrap_or(&[])
            .iter()
            .filter_map(|entry| {
                let coords = entry.list()?;
                let value = |i: usize| coords.get(i).and_then(Arg::number).unwrap_or(0.0) * scale;
                Some(Point::new(value(0), value(1), value(2)))
            })
            .collect()
    }

    /// Every instance id carrying a part with this keyword.
    fn ids_with(&self, keyword: &str) -> Vec<u64> {
        self.instances
            .iter()
            .filter(|(_, inst)| inst.part(keyword).is_some())
            .map(|(id, _)| id)
            .collect()
    }

    /// One pass over the exchange builds both property-chain indexes:
    /// definition → its `PROPERTY_DEFINITION`s (by the definition argument),
    /// property → its `SHAPE_DEFINITION_REPRESENTATION`s (by the definition
    /// they represent). Ascending ids inside each list, so a lookup visits
    /// candidates in file order, lowest id first.
    fn ensure_property_indexes(&mut self) {
        if self.properties_of_definition.is_some() {
            return;
        }
        let mut properties: FastMap<u64, Vec<u64>> = FastMap::default();
        let mut sdrs: FastMap<u64, Vec<u64>> = FastMap::default();
        let instances = self.instances;
        for (id, instance) in instances.iter() {
            if let Some(args) = instance.part("PROPERTY_DEFINITION") {
                // Read for the index is read: the skipped table should not
                // claim the reader never looked.
                self.visited.mark(id);
                if let Some(definition) = args.get(2).and_then(Arg::reference) {
                    properties.entry(definition).or_default().push(id);
                }
            }
            if let Some(args) = instance.part("SHAPE_DEFINITION_REPRESENTATION") {
                self.visited.mark(id);
                if let Some(property) = args.first().and_then(Arg::reference) {
                    sdrs.entry(property).or_default().push(id);
                }
            }
        }
        for list in properties.values_mut().chain(sdrs.values_mut()) {
            list.sort_unstable();
        }
        self.properties_of_definition = Some(properties);
        self.sdrs_of_property = Some(sdrs);
    }

    /// A measure item's value and kind, scaled into the document's units.
    fn measure_value(&mut self, id: u64) -> Option<(f64, ogeom_doc::MeasureKind)> {
        let args = {
            let instance = self.instance(id).ok()?;
            instance.part("MEASURE_WITH_UNIT")?.to_vec()
        };
        match args.first() {
            Some(Arg::Typed(typed)) => {
                let (kind, inner) = (&typed.0, &typed.1);
                let value = inner.first().and_then(Arg::number)?;
                if kind.contains("ANGLE") {
                    Some((value * self.angle_scale, ogeom_doc::MeasureKind::Angle))
                } else {
                    Some((value * self.report.scale_mm, ogeom_doc::MeasureKind::Length))
                }
            }
            _ => None,
        }
    }

    /// A dimensional characteristic's name, kind and aspects.
    ///
    /// Sizes apply to one feature; locations (linear or angular) run
    /// between two. `ANGULAR_SIZE` and `ANGULAR_LOCATION` are the same
    /// shapes with an extra angle-selection argument at the end.
    fn dimension_shape(&mut self, dim: u64) -> (String, bool, Vec<u64>) {
        let size = {
            let Ok(instance) = self.instance(dim) else {
                return (String::new(), false, Vec::new());
            };
            instance
                .part("DIMENSIONAL_SIZE")
                .or_else(|| instance.part("ANGULAR_SIZE"))
        };
        if let Some(args) = size {
            let name = match args.get(1) {
                Some(Arg::Str(s)) => s.clone(),
                _ => String::new(),
            };
            let aspects = args.first().and_then(Arg::reference).into_iter().collect();
            return (name, false, aspects);
        }
        let location = {
            let Ok(instance) = self.instance(dim) else {
                return (String::new(), false, Vec::new());
            };
            instance
                .part("DIMENSIONAL_LOCATION")
                .or_else(|| instance.part("ANGULAR_LOCATION"))
        };
        if let Some(args) = location {
            let name = match args.first() {
                Some(Arg::Str(s)) => s.clone(),
                _ => String::new(),
            };
            let aspects = [args.get(2), args.get(3)]
                .into_iter()
                .flatten()
                .filter_map(Arg::reference)
                .collect();
            return (name, true, aspects);
        }
        (String::new(), false, Vec::new())
    }

    /// The datum letter reachable from a datum reference, depth-bounded: the
    /// reference chain runs through compartments and systems, and files
    /// arrange it differently.
    fn datum_letter(&mut self, id: u64, depth: usize) -> Option<String> {
        if depth > 4 {
            return None;
        }
        let (letter, members, refs) = {
            let instance = self.instance(id).ok()?;
            let letter = instance.part("DATUM").and_then(|args| match args.get(4) {
                Some(Arg::Str(s)) if !s.is_empty() => Some(s.clone()),
                _ => None,
            });
            // A compartment or common datum carrying a list of constituent
            // references is a composite: every constituent contributes a
            // letter, and the letters act as one datum.
            let members: Vec<u64> = ["DATUM_REFERENCE_COMPARTMENT", "COMMON_DATUM"]
                .iter()
                .find_map(|k| instance.part(k))
                .into_iter()
                .flatten()
                .find_map(Arg::list)
                .map(|list| list.iter().filter_map(Arg::reference).collect())
                .unwrap_or_default();
            let mut refs = Vec::new();
            for (_, args) in instance.parts() {
                collect_refs(args, &mut refs);
            }
            (letter, members, refs)
        };
        if let Some(letter) = letter {
            return Some(letter);
        }
        if !members.is_empty() {
            let letters: Vec<String> = members
                .into_iter()
                .filter_map(|r| self.datum_letter(r, depth + 1))
                .collect();
            if !letters.is_empty() {
                return Some(letters.join("-"));
            }
        }
        refs.into_iter()
            .find_map(|r| self.datum_letter(r, depth + 1))
    }
}

/// A product definition gathered from the file: its shapes and its usage
/// edges, before anything is committed to the document.
struct PdEntry {
    pd: u64,
    name: String,
    shapes: Vec<Shape>,
    children: Vec<(u64, Transform, Option<String>)>,
}

/// Each wire walked the other way round: its edges in reverse order, each
/// reversed.
fn walked_back(model: &mut Model, wires: &[Shape], tol: Tolerances) -> OgeomResult<Vec<Shape>> {
    wires
        .iter()
        .map(|wire| {
            let back = model.ordered_children_of(&wire.reversed())?;
            Ok(make_wire(model, &back, tol)?.shape)
        })
        .collect()
}

/// An edge's pcurve on a surface: the file's own where it lies within the
/// edge's tolerance of the curve once lifted, else derived, exact where
/// the pair has a closed form and fitted where it does not, with how far it
/// stands from the curve once lifted through the surface.
fn derive_pcurve(
    curve: &Curve,
    range: (f64, f64),
    surface: &SurfaceGeometry,
    stated: &[PlanarCurve],
    tol: Tolerances,
) -> PreparedPcurve {
    // A pcurve that does not evaluate over the edge's range states no gap
    // here; the face that uses it fails on it there.
    let lifted = |pcurve: &PlanarCurve| {
        ogeom_algo::pcurve_fit::lifted_gap((curve, range), (pcurve, range), surface, false, tol)
            .unwrap_or(0.0)
    };
    let nearest = nearest_stated(curve, range, surface, stated, tol);
    // One that follows the curve exactly needs nothing derived beside it.
    if let Some((curve, off)) = &nearest
        && *off <= tol.confusion()
    {
        return PreparedPcurve::Stated {
            curve: curve.clone(),
            off: *off,
        };
    }
    let derived =
        if let Some(exact) = ogeom_intersect::exact_pcurve_over(curve, range, surface, tol) {
            let exact = over_range(&exact, range);
            let off = lifted(&exact);
            PreparedPcurve::Exact { curve: exact, off }
        } else {
            // No closed form: a spline surface, or a combination the
            // projection table lacks. The pcurve is fitted at the curve's own
            // parameters, so same-parameter is preserved by construction.
            match crate::pcurves::fit_projected_pcurve(curve, range, surface, tol) {
                Ok((fitted, error, met, worst_off, warning)) => {
                    let off = lifted(&fitted);
                    PreparedPcurve::Fitted {
                        curve: fitted,
                        error,
                        met,
                        worst_off,
                        off,
                        warning,
                    }
                }
                Err(e) => PreparedPcurve::Refused(e.to_string()),
            }
        };
    // The edge's tolerance on this face is how far its curve stands from
    // the surface, which the projection measures. A pcurve whose trace
    // keeps within that of the projected trace lifts to within twice it of
    // the curve, and within that bound the file's pcurve is the same
    // boundary drawn as its sender drew it, and is kept.
    let allowed = match &derived {
        PreparedPcurve::Exact { off, .. } => Some(*off),
        PreparedPcurve::Fitted { off, worst_off, .. } => Some(off.max(*worst_off)),
        PreparedPcurve::Stated { .. } | PreparedPcurve::Refused(_) => None,
    };
    match (nearest, allowed) {
        (Some((curve, off)), Some(allowed)) if off <= 2.0f64.mul_add(allowed, tol.confusion()) => {
            PreparedPcurve::Stated { curve, off }
        }
        _ => derived,
    }
}

/// A pcurve as it is measured over an edge's range: a line's stated
/// domain widened to cover it, since a line evaluates anywhere.
fn over_range(pcurve: &PlanarCurve, range: (f64, f64)) -> PlanarCurve {
    if let PlanarCurve::Line(l) = pcurve {
        let (lo, hi) = (l.domain().0.min(range.0), l.domain().1.max(range.1));
        if let Ok(wider) = ogeom_geom::Line2d::over(l.axis(), lo, hi) {
            return wider.into();
        }
    }
    pcurve.clone()
}

/// The file's pcurve nearest the curve once lifted, with how far it
/// stands. One that does not evaluate over the edge's range, or whose
/// image leaves the surface's chart along a direction that does not close,
/// is left out.
///
/// A closed curve's range here may begin whole periods from where the
/// file's parameter began it, so each is also tried a period either way:
/// on a sphere a meridian's image a period off lifts to the same points,
/// at latitudes past the poles, and only the right one stays in the chart.
/// The first that follows the curve exactly ends the search.
fn nearest_stated(
    curve: &Curve,
    range: (f64, f64),
    surface: &SurfaceGeometry,
    stated: &[PlanarCurve],
    tol: Tolerances,
) -> Option<(PlanarCurve, f64)> {
    let period = curve.is_periodic().then(|| {
        let (lo, hi) = curve.domain();
        hi - lo
    });
    let turns: &[f64] = if period.is_some() {
        &[0.0, 1.0, -1.0]
    } else {
        &[0.0]
    };
    let mut best: Option<(PlanarCurve, f64)> = None;
    for pcurve in stated {
        for turn in turns {
            let Some(shifted) = period_shifted(pcurve, turn * period.unwrap_or(0.0)) else {
                continue;
            };
            let shifted = over_range(&shifted, range);
            if !within_chart(&shifted, range, surface, tol) {
                continue;
            }
            let Ok(off) = ogeom_algo::pcurve_fit::lifted_gap(
                (curve, range),
                (&shifted, range),
                surface,
                false,
                tol,
            ) else {
                continue;
            };
            if best.as_ref().is_none_or(|(_, held)| off < *held) {
                best = Some((shifted, off));
            }
            if off <= tol.confusion() {
                return best;
            }
        }
    }
    best
}

/// A pcurve read `delta` later along its parameter: at `t`, the point the
/// pcurve has at `t - delta`. `None` for a kind that cannot be restated so.
fn period_shifted(pcurve: &PlanarCurve, delta: f64) -> Option<PlanarCurve> {
    if delta == 0.0 {
        return Some(pcurve.clone());
    }
    match pcurve {
        PlanarCurve::Line(l) => {
            let axis = l.axis();
            let moved = ogeom_math::Axis2 {
                location: axis.location - axis.direction.vector() * delta,
                direction: axis.direction,
            };
            let (lo, hi) = l.domain();
            ogeom_geom::Line2d::over(moved, lo + delta, hi + delta)
                .ok()
                .map(Into::into)
        }
        PlanarCurve::BSpline(b) => {
            let knots: Vec<f64> = b.knots().knots().iter().map(|k| k + delta).collect();
            let knots = KnotVector::new(knots, b.knots().degree()).ok()?;
            ogeom_geom::BSpline2d::rational(knots, b.control_points().to_vec())
                .ok()
                .map(Into::into)
        }
        // A conic's parameter is its angle: whole turns leave it in place.
        PlanarCurve::Circle(_) | PlanarCurve::Ellipse(_)
            if (delta / core::f64::consts::TAU).fract().abs() < 1e-12 =>
        {
            Some(pcurve.clone())
        }
        _ => None,
    }
}

/// Whether a pcurve's image over a range stays inside the surface's chart
/// along each direction that does not close on itself.
fn within_chart(
    pcurve: &PlanarCurve,
    range: (f64, f64),
    surface: &SurfaceGeometry,
    tol: Tolerances,
) -> bool {
    let ((u0, u1), (v0, v1)) = surface.domain();
    let slack = tol.parametric();
    (0..=4).all(|i| {
        let t = (range.1 - range.0).mul_add(f64::from(i) / 4.0, range.0);
        pcurve.point_at(t, tol).is_ok_and(|p| {
            (surface.is_periodic_u() || (p.x >= u0 - slack && p.x <= u1 + slack))
                && (surface.is_periodic_v() || (p.y >= v0 - slack && p.y <= v1 + slack))
        })
    })
}

/// Every reference in an argument tree, in order.
fn collect_refs(args: &[Arg], out: &mut Vec<u64>) {
    for arg in args {
        match arg {
            Arg::Ref(id) => out.push(*id),
            Arg::List(inner) => collect_refs(inner, out),
            Arg::Typed(typed) => collect_refs(&typed.1, out),
            _ => {}
        }
    }
}

/// Whether two points stand on one column about `circle`'s axis: the same
/// angle about it, to within the confusion distance along the circle
/// through `q`.
fn same_column(circle: &ogeom_math::Circle, p: Point, q: Point, tol: Tolerances) -> bool {
    let (lp, lq) = (circle.frame().to_local(p), circle.frame().to_local(q));
    let apart = (lp.y.atan2(lp.x) - lq.y.atan2(lq.x)).rem_euclid(core::f64::consts::TAU);
    let turn = apart.min(core::f64::consts::TAU - apart);
    turn * lq.x.hypot(lq.y) <= tol.confusion()
}

/// For a two-wire periodic face: each wire's single closed edge with its
/// vertex, empty when the shape is anything else.
fn closed_ring_edges(model: &Model, wires: &[Shape]) -> OgeomResult<Vec<(Shape, Shape)>> {
    let mut out = Vec::new();
    for wire in wires {
        let edges = ogeom_topo::explore(
            model,
            wire,
            ogeom_topo::Filter::OfType(ogeom_topo::ShapeType::Edge),
        )?;
        if edges.len() != 1 {
            return Ok(Vec::new());
        }
        let edge = edges[0].clone();
        let Some((a, b)) = ogeom_algo::edge_vertices(model, &edge)? else {
            return Ok(Vec::new());
        };
        if !a.is_same(&b) {
            return Ok(Vec::new());
        }
        out.push((edge, a));
    }
    Ok(out)
}

/// The window of `curve` between the feet of two vertices that stand on it
/// short of its ends: both within a hair of the curve, in the curve's own
/// order, and not the whole curve. `None` where the vertices are off the
/// curve, reversed against it, or already at its ends.
fn window_between_feet(
    curve: &Curve,
    start: Point,
    end: Point,
    tol: Tolerances,
) -> Option<(f64, f64)> {
    let (lo, hi) = curve.domain();
    let samples = match curve {
        Curve::BSpline(spline) => (spline.control_points().len() * 8).max(64),
        _ => 64,
    };
    let foot = |p: Point| -> Option<f64> {
        let found = project_on_curve(curve, p, samples, tol).ok()?;
        (found.distance <= tol.confusion() * 1e4).then_some(found.parameter)
    };
    let (a, b) = (foot(start)?, foot(end)?);
    let slack = tol.parametric().max((hi - lo) * 1e-9);
    if b <= a + slack {
        return None;
    }
    if (a - lo).abs() <= slack && (b - hi).abs() <= slack {
        return None;
    }
    Some((a, b))
}

/// The knots a STEP B-spline form implies when it states none: a Bézier
/// form's pieces joined at whole numbers, a uniform form's knots one apart
/// from `-degree`, a quasi-uniform form's the same clamped at both ends.
fn implied_knots(form: &str, degree: usize, count: usize) -> Vec<f64> {
    let p = degree.max(1);
    #[allow(clippy::cast_precision_loss)]
    let at = |i: usize| i as f64;
    if form.starts_with("BEZIER") {
        let pieces = (count.saturating_sub(1) / p).max(1);
        let mut out = vec![0.0; p + 1];
        for s in 1..pieces {
            out.extend(std::iter::repeat_n(at(s), p));
        }
        out.extend(std::iter::repeat_n(at(pieces), p + 1));
        out
    } else if form.starts_with("QUASI_UNIFORM") {
        let interior = count.saturating_sub(p + 1);
        let mut out = vec![0.0; p + 1];
        out.extend((1..=interior).map(at));
        out.extend(std::iter::repeat_n(at(interior + 1), p + 1));
        out
    } else {
        #[allow(clippy::cast_precision_loss)]
        (0..count + p + 1).map(|i| i as f64 - p as f64).collect()
    }
}

/// Two splines joined end to start: the lower degree raised to the
/// higher, the second's weights scaled to meet the first's at the join (a
/// rational curve is unchanged by scaling all its weights), and the join
/// knot left at full multiplicity.
fn join_splines(
    held: (KnotVector, Vec<Weighted<Point>>),
    next: &BSplineCurve,
    tol: Tolerances,
) -> OgeomResult<(KnotVector, Vec<Weighted<Point>>)> {
    let (mut knots, mut control) = held;
    let mut next = next.clone();
    while next.degree() < knots.degree() {
        next = next.elevated(tol)?;
    }
    while knots.degree() < next.degree() {
        (knots, control) = ogeom_math::bspline::elevate_degree(&knots, &control, tol)?;
    }
    let (Some(end), Some(start)) = (control.last(), next.control_points().first()) else {
        ogeom_bail!(Construction, "a composite segment has no control points");
    };
    let factor = end.weight / start.weight;
    let scaled: Vec<Weighted<Point>> = next
        .control_points()
        .iter()
        .map(|w| Weighted {
            scaled: w.scaled.scale(factor),
            weight: w.weight * factor,
        })
        .collect();
    ogeom_math::bspline::join(&(knots, control), &(next.knots().clone(), scaled))
}

/// One patch of a composite surface as a tensor spline: its knots each way,
/// on the unit interval, and its net indexed `[u][v]`.
#[derive(Debug, Clone)]
struct Patch {
    u: KnotVector,
    v: KnotVector,
    net: Vec<Vec<Weighted<Point>>>,
}

impl Patch {
    fn of(surface: &ogeom_geom::BSplineSurface) -> OgeomResult<Self> {
        let grid = surface.grid();
        let (nu, nv) = (grid.u_count(), grid.v_count());
        let points = grid.points();
        let net = (0..nu)
            .map(|i| (0..nv).map(|j| points[i * nv + j]).collect())
            .collect();
        Ok(Self {
            u: surface.u_knots().reparameterized(0.0, 1.0)?,
            v: surface.v_knots().reparameterized(0.0, 1.0)?,
            net,
        })
    }

    fn transposed(self) -> Self {
        let (nu, nv) = (self.net.len(), self.net[0].len());
        let net = (0..nv)
            .map(|j| (0..nu).map(|i| self.net[i][j]).collect())
            .collect();
        Self {
            u: self.v,
            v: self.u,
            net,
        }
    }

    fn reversed_u(mut self) -> Self {
        self.net.reverse();
        self.u = self.u.reversed();
        self
    }

    /// Apply a curve operation along `u` to every column of the net.
    fn along_u(
        self,
        op: impl Fn(&KnotVector, &[Weighted<Point>]) -> OgeomResult<(KnotVector, Vec<Weighted<Point>>)>,
    ) -> OgeomResult<Self> {
        let nv = self.net[0].len();
        let mut knots = self.u.clone();
        let mut columns = Vec::with_capacity(nv);
        for j in 0..nv {
            let column: Vec<Weighted<Point>> = self.net.iter().map(|row| row[j]).collect();
            let (k, c) = op(&self.u, &column)?;
            knots = k;
            columns.push(c);
        }
        let nu = columns[0].len();
        let net = (0..nu)
            .map(|i| (0..nv).map(|j| columns[j][i]).collect())
            .collect();
        Ok(Self {
            u: knots,
            v: self.v,
            net,
        })
    }
}

/// A grid of patches (`grid[i][j]`, `i` along `u`) joined into one surface.
fn join_patches(mut grid: Vec<Vec<Patch>>, tol: Tolerances) -> OgeomResult<SurfaceGeometry> {
    let (m, n) = (grid.len(), grid[0].len());
    // Degrees raised to the grid's highest each way.
    let pu = grid
        .iter()
        .flatten()
        .map(|p| p.u.degree())
        .max()
        .unwrap_or(1);
    let pv = grid
        .iter()
        .flatten()
        .map(|p| p.v.degree())
        .max()
        .unwrap_or(1);
    for patch in grid.iter_mut().flatten() {
        let mut p = patch.clone();
        while p.u.degree() < pu {
            p = p.along_u(|k, c| ogeom_math::bspline::elevate_degree(k, c, tol))?;
        }
        p = p.transposed();
        while p.u.degree() < pv {
            p = p.along_u(|k, c| ogeom_math::bspline::elevate_degree(k, c, tol))?;
        }
        *patch = p.transposed();
    }
    // Knots unified: along u within each grid row of patches sharing an
    // index `i` (their u knots must agree to share v boundaries), along v
    // within each index `j`.
    let unify = |patches: Vec<Patch>, along_v: bool| -> OgeomResult<Vec<Patch>> {
        let turned: Vec<Patch> = if along_v {
            patches.into_iter().map(Patch::transposed).collect()
        } else {
            patches
        };
        let mut wanted: Vec<(f64, usize)> = Vec::new();
        for p in &turned {
            for (value, count) in p.u.distinct() {
                if value <= 1e-12 || value >= 1.0 - 1e-12 {
                    continue;
                }
                match wanted.iter_mut().find(|(v, _)| (v - value).abs() <= 1e-12) {
                    Some(slot) => slot.1 = slot.1.max(count),
                    None => wanted.push((value, count)),
                }
            }
        }
        let mut out = Vec::with_capacity(turned.len());
        for mut p in turned {
            for &(value, count) in &wanted {
                let have = p.u.multiplicity_of(value);
                if have < count {
                    p = p.along_u(|k, c| {
                        ogeom_math::bspline::insert_knot(k, c, value, count - have, tol)
                    })?;
                }
            }
            out.push(if along_v { p.transposed() } else { p });
        }
        Ok(out)
    };
    for row in &mut grid {
        *row = unify(std::mem::take(row), true)?;
    }
    for j in 0..n {
        let column: Vec<Patch> = grid.iter().map(|row| row[j].clone()).collect();
        for (row, p) in grid.iter_mut().zip(unify(column, false)?) {
            row[j] = p;
        }
    }
    // Joined: shared boundaries once, each join a knot of full multiplicity.
    let near = |a: &Weighted<Point>, b: &Weighted<Point>| {
        a.point().distance(b.point()) <= tol.confusion() * 100.0
            && (a.weight - b.weight).abs() <= 1e-9 * a.weight.abs().max(1.0)
    };
    let mut rows: Vec<Vec<Weighted<Point>>> = Vec::new();
    for (i, grid_row) in grid.iter().enumerate() {
        let nu = grid_row[0].net.len();
        for a in 0..nu {
            if i > 0 && a == 0 {
                // The row this patch shares with the one before it.
                let prev = rows.len() - 1;
                let mut shared = 0;
                for (j, patch) in grid_row.iter().enumerate() {
                    for (b, cell) in patch.net[0].iter().enumerate() {
                        if j > 0 && b == 0 {
                            continue;
                        }
                        if !near(&rows[prev][shared], cell) {
                            ogeom_bail!(Construction, "neighbouring patches part along u");
                        }
                        shared += 1;
                    }
                }
                continue;
            }
            let mut row = Vec::new();
            for (j, patch) in grid_row.iter().enumerate() {
                for (b, cell) in patch.net[a].iter().enumerate() {
                    if j > 0 && b == 0 {
                        if !near(row.last().unwrap_or(cell), cell) {
                            ogeom_bail!(Construction, "neighbouring patches part along v");
                        }
                        continue;
                    }
                    row.push(*cell);
                }
            }
            rows.push(row);
        }
    }
    // Each patch's knots shifted to its own unit of the whole; at a join
    // one copy of the shared end goes, leaving the multiplicity at the
    // degree, and the next patch's run follows past its own clamped start.
    let joined_knots = |parts: Vec<&KnotVector>, degree: usize| -> OgeomResult<KnotVector> {
        let mut out: Vec<f64> = Vec::new();
        for (k, knots) in parts.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let shift = k as f64;
            let values: Vec<f64> = knots.knots().iter().map(|x| x + shift).collect();
            if k == 0 {
                out.extend(values);
            } else {
                out.pop();
                out.extend_from_slice(&values[degree + 1..]);
            }
        }
        KnotVector::new(out, degree)
    };
    let u = joined_knots((0..m).map(|i| &grid[i][0].u).collect(), pu)?;
    let v = joined_knots((0..n).map(|j| &grid[0][j].v).collect(), pv)?;
    let (nu, nv) = (rows.len(), rows[0].len());
    let cells: Vec<Weighted<Point>> = rows.into_iter().flatten().collect();
    Ok(
        ogeom_geom::BSplineSurface::rational(u, v, ogeom_math::ControlGrid::new(cells, nu, nv)?)?
            .into(),
    )
}

/// The highest spline degree a file may state. The standard sets no limit;
/// exporters stay far below this, and a degree past it would only size
/// allocations from a number in the file.
const MOST_DEGREE: usize = 64;

/// A spline degree read from the file, `missing` where the argument is
/// absent, refused past [`MOST_DEGREE`] or where it is not a count.
fn file_degree(arg: Option<&Arg>, missing: f64) -> OgeomResult<usize> {
    let degree = arg.and_then(Arg::number).unwrap_or(missing);
    #[allow(clippy::cast_precision_loss)]
    if !degree.is_finite() || degree < 0.0 || degree > MOST_DEGREE as f64 {
        ogeom_bail!(Construction, "a spline of degree {degree}");
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(degree as usize)
}

/// Knots written once each with their multiplicities, spelt out. A
/// multiplicity past one more than [`MOST_DEGREE`] is refused: no valid
/// spline repeats a knot more often than its order.
fn expanded_knots(mults: &[Arg], knots: &[Arg]) -> OgeomResult<Vec<f64>> {
    let mut out = Vec::new();
    for (m, k) in mults.iter().zip(knots) {
        let count = m.number().unwrap_or(1.0);
        #[allow(clippy::cast_precision_loss)]
        if !count.is_finite() || count < 0.0 || count > (MOST_DEGREE + 1) as f64 {
            ogeom_bail!(Construction, "a knot of multiplicity {count}");
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        out.extend(std::iter::repeat_n(
            k.number().unwrap_or(0.0),
            count as usize,
        ));
    }
    Ok(out)
}

/// Which instances the reader has looked at.
///
/// One slot per possible id where the instance table has one, as it does
/// for the dense ids exporters write: the reader touches instances millions
/// of times, and a direct index beats hashing every touch. A file whose ids
/// are sparse (one instance numbered in the billions) gets a set instead of
/// a table that size.
enum Visited {
    Dense(Vec<bool>),
    Sparse(ogeom_core::FastSet<u64>),
}

impl Visited {
    fn for_instances(instances: &Instances) -> Self {
        match instances {
            Instances::Dense(slots) => Self::Dense(vec![false; slots.len()]),
            Instances::Sparse(_) => Self::Sparse(ogeom_core::FastSet::default()),
        }
    }

    fn mark(&mut self, id: u64) {
        match self {
            Self::Dense(slots) => {
                if let Some(slot) = usize::try_from(id).ok().and_then(|i| slots.get_mut(i)) {
                    *slot = true;
                }
            }
            Self::Sparse(seen) => {
                seen.insert(id);
            }
        }
    }

    fn seen(&self, id: u64) -> bool {
        match self {
            Self::Dense(slots) => usize::try_from(id)
                .ok()
                .and_then(|i| slots.get(i).copied())
                .unwrap_or(false),
            Self::Sparse(seen) => seen.contains(&id),
        }
    }
}

/// The reference in argument `i`, or 0 (which names no entity, so the lookup
/// that follows refuses it) where the argument is missing or not a reference.
fn ref_at(args: &[Arg], i: usize) -> u64 {
    args.get(i).and_then(Arg::reference).unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const T: Tolerances = Tolerances::millimetres();

    /// An edge's window between the feet of vertices standing on the
    /// curve short of its ends: taken in the curve's order, refused where
    /// the vertices are the ends already, run against the curve, or off it.
    #[test]
    fn an_edges_window_is_taken_between_its_vertices_feet() {
        let spline: Curve = BSplineCurve::new(
            KnotVector::clamped_uniform(3, 6).unwrap(),
            vec![
                Point::new(0.0, 0.0, 0.0),
                Point::new(1.0, 2.0, 0.5),
                Point::new(2.5, 1.0, -0.5),
                Point::new(4.0, 3.0, 1.0),
                Point::new(5.0, 0.5, 0.0),
                Point::new(6.0, 2.0, 2.0),
            ],
            T,
        )
        .unwrap()
        .into();
        let at = |t: f64| spline.point_at(t, T).unwrap();
        let (a, b) = window_between_feet(&spline, at(0.3), at(0.7), T).unwrap();
        assert!(
            (a - 0.3).abs() < 1e-9 && (b - 0.7).abs() < 1e-9,
            "{a} .. {b}"
        );
        let (a, b) = window_between_feet(&spline, at(0.0), at(0.7), T).unwrap();
        assert!(a.abs() < 1e-9 && (b - 0.7).abs() < 1e-9, "{a} .. {b}");
        assert!(window_between_feet(&spline, at(0.0), at(1.0), T).is_none());
        assert!(window_between_feet(&spline, at(0.7), at(0.3), T).is_none());
        let off = at(0.3) + Vector::new(0.0, 0.0, 0.5);
        assert!(window_between_feet(&spline, off, at(0.7), T).is_none());
    }

    /// A parallel of a cone read as a circle a few tenths of a micron wider
    /// than the cone there, its range running past one turn: the exact
    /// pcurve is stated over the whole range, and the gap it stands off
    /// the circle is measured along all of it.
    #[test]
    fn a_wrapped_parallel_states_its_gap() {
        let cone: SurfaceGeometry = ConeSurface::new(
            Cone::new(Frame::WORLD, 2.0, core::f64::consts::FRAC_PI_4, T).unwrap(),
            (-10.0, 10.0),
        )
        .unwrap()
        .into();
        let wider = 3e-7;
        let circle: Curve =
            CircleCurve::new(Circle::new(Frame::WORLD, 2.0 + wider, T).unwrap()).into();
        let range = (3.0, 8.5);
        let PreparedPcurve::Exact { curve, off } = derive_pcurve(&circle, range, &cone, &[], T)
        else {
            panic!("a parallel has an exact pcurve");
        };
        assert!(curve.point_at(range.1, T).is_ok(), "{curve:?}");
        assert!((off - wider).abs() < wider * 1e-3, "{off}");
    }
}
