//! From an IGES deck to a living model.
//!
//! Two kinds of file arrive under one extension. A *solid* file carries
//! manifold solid B-rep objects (entity 186 over shells, faces, loops, edge
//! lists and vertex lists) and reads bottom-up the way the STEP reader does,
//! sharing what the file shares. A *surface* file, the older and far more
//! common kind, is a loose collection of trimmed surfaces; those become
//! faces, the faces are sewn, and a shell that closes becomes a solid. Both
//! kinds re-derive edge ranges on this kernel's own parameterizations from
//! the endpoint geometry, because a 1980s file's parameterizations are its
//! own business.
//!
//! What the reader does not understand it *counts*: every entity never
//! visited lands in the report's skipped table under its type number, and
//! every compromise is a warning naming the directory entry. Refusals are by
//! name: a conic form this reader does not translate says which form and
//! where, and points at the parity ledger's `io.iges` row for the whole
//! picture.

use super::parse::{Entity, File};
use ogeom_algo::{make_edge_between, make_solid, make_vertex, sew};
use ogeom_core::FastMap;
use ogeom_core::{OgeomResult, Tolerances, ogeom_bail};
use ogeom_geom::Curve3d as _;
use ogeom_geom::Transformable as _;
use ogeom_geom::{
    BSplineCurve, BSplineSurface, CircleCurve, ConeSurface, Curve, CylinderSurface, EllipseCurve,
    ExtrusionSurface, HyperbolaCurve, LineCurve, OffsetCurve, OffsetSurface, ParabolaCurve,
    PlaneSurface, RevolutionSurface, SphereSurface, SurfaceGeometry, TorusSurface, TrimmedCurve,
};
use ogeom_math::{
    Circle, Cone, ControlGrid, Cylinder, Direction, Ellipse, Frame, Hyperbola, KnotVector, Matrix3,
    Parabola, Plane, Point, Sphere, Torus, Transform, Vector, Weighted,
};
use ogeom_topo::{Model, Shape};
use std::collections::BTreeMap;

/// How far an unbounded plane or quadric extends past anything the file uses
/// (the same convention the STEP reader states: a face's trim is its wires,
/// and the surface's domain is only a parameter window).
const SURFACE_EXTENT: f64 = 1e5;

/// The constructive solid entities: the primitives, the solids of revolution
/// and extrusion, the ellipsoid, the boolean tree, the solid assembly and
/// the solid instance.
const CSG_KINDS: [i64; 12] = [150, 152, 154, 156, 158, 160, 162, 164, 168, 180, 184, 430];

/// What an import brought in, and what it left behind.
#[derive(Debug, Default)]
pub struct IgesReport {
    /// Millimetres per file unit, as the global section states it.
    pub scale_mm: f64,
    /// Entity types the reader never visited, with counts, keyed as
    /// `"type NNN"` or `"type NNN form F"`. Annotation and drafting land
    /// here by design; geometry landing here is a gap worth reading about.
    pub skipped: BTreeMap<String, usize>,
    /// Everything that imported less than perfectly, one line each.
    pub warnings: Vec<String>,
}

/// A read IGES file: the document, the shapes found, and the report.
#[derive(Debug)]
pub struct IgesImport {
    /// The document everything was built into.
    pub document: ogeom_doc::Document,
    /// One shape per manifold solid, then one per surface group that sewed
    /// closed.
    pub solids: Vec<Shape>,
    /// Sewn shells and loose faces that do not enclose a volume.
    pub sheets: Vec<Shape>,
    /// What happened along the way.
    pub report: IgesReport,
}

/// The pieces an edge-list entry resolves to.
type BuiltEdge = (Shape, Curve, (f64, f64));

/// Read an IGES file's geometry.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) if the
/// deck does not parse, its units are unreadable, or it contains nothing this
/// reader translates into shapes. Individual entities that fail to translate
/// become warnings and skipped counts rather than errors; the report says
/// exactly what was compromised.
pub fn read_iges(text: &str, tol: Tolerances) -> OgeomResult<IgesImport> {
    read_iges_impl(text, tol, false)
}

/// Read genuine IGES geometry for a bounded surface preview, preserving
/// every independent authored trimmed face without sewing or repairing it.
/// Manifold/constructive solids and the geometry compromise report retain
/// their normal behavior. Alternate curves on successfully consumed 142
/// boundaries are validated and accounted for as definitions, not drawn.
///
/// # Errors
/// As [`read_iges`], or when more than 100,000 entities or 4096 independent
/// trimmed faces would be expanded for the preview.
pub fn read_iges_unsewn(text: &str, tol: Tolerances) -> OgeomResult<IgesImport> {
    read_iges_impl(text, tol, true)
}

fn read_iges_impl(text: &str, tol: Tolerances, unsewn: bool) -> OgeomResult<IgesImport> {
    let file = super::parse::parse(text)?;
    if unsewn && file.entities.len() > 100_000 {
        ogeom_bail!(Construction, "IGES preview exceeds 100000 entities");
    }
    let Some(scale_mm) = file.scale_mm() else {
        ogeom_bail!(
            Construction,
            "IGES global section names units this reader cannot convert to millimetres"
        );
    };
    let mut reader = Reader {
        file: &file,
        model: Model::new(),
        report: IgesReport {
            scale_mm,
            ..IgesReport::default()
        },
        visited: BTreeMap::new(),
        vertices: FastMap::default(),
        edges: FastMap::default(),
        pieces: FastMap::default(),
        pole_vertices: Vec::new(),
        trim_poles: Vec::new(),
        vertex_misses: (0, 0.0),
        images: FastMap::default(),
        face_de: None,
        depth: 0,
        tol,
    };

    // Solids first: every 186, in directory order.
    let mut solids: Vec<(i64, Shape)> = Vec::new();
    let solid_des: Vec<i64> = file
        .entities
        .iter()
        .filter(|(_, e)| e.kind == 186)
        .map(|(de, _)| *de)
        .collect();
    let total = solid_des.len() as u64;
    for (done, de) in solid_des.into_iter().enumerate() {
        ogeom_core::progress::checkpoint()?;
        ogeom_core::progress::stage_at("iges: solid", done as u64 + 1, total);
        match reader.manifold_solid(de) {
            Ok(solid) => solids.push((de, solid)),
            Err(e) => reader
                .report
                .warnings
                .push(format!("D{de}: manifold solid failed to build: {e}")),
        }
    }

    // Then the surface file: every *independent* trimmed or bounded surface
    // becomes a face; the faces sew; closed shells become solids. Subordinate
    // entities belong to something else and are not top-level geometry; the
    // subordinate switch is the second two-digit field of the status word.
    let mut faces = Vec::new();
    // Which directory entry built which shape, for levels and groups.
    let mut built_from: Vec<(i64, Shape)> = solids.clone();
    let face_des: Vec<i64> = file
        .entities
        .iter()
        .filter(|(de, e)| {
            matches!(e.kind, 143 | 144)
                && (e.status / 10_000) % 100 == 0
                && !reader.visited.contains_key(de)
        })
        .map(|(de, _)| *de)
        .collect();
    if unsewn && face_des.len() > 4096 {
        ogeom_bail!(Construction, "IGES preview exceeds 4096 independent faces");
    }
    let total = face_des.len() as u64;
    reader.trim_poles = reader.poles_of_trims(&face_des);
    for (done, de) in face_des.into_iter().enumerate() {
        ogeom_core::progress::checkpoint()?;
        ogeom_core::progress::stage_at("iges: face", done as u64 + 1, total);
        match reader.face(de) {
            Ok(face) => {
                built_from.push((de, face.clone()));
                faces.push(face);
            }
            Err(e) => reader
                .report
                .warnings
                .push(format!("D{de}: trimmed surface failed to build: {e}")),
        }
    }
    let mut sheets = Vec::new();
    if unsewn && !faces.is_empty() {
        // A compound retains every actual face, with its own trimming,
        // geometry and placement. No connectivity or shell is invented.
        sheets.push(ogeom_algo::make_compound(&mut reader.model, &faces)?.shape);
    } else if !faces.is_empty() {
        let sewn = sew(&mut reader.model, &faces, tol)?;
        for shell in &sewn.shells {
            // A face's seam sewn from two edges is one edge holding its
            // column as plain pcurves; it is made the seam it is.
            ogeom_algo::join_seam_columns(&mut reader.model, shell, tol)?;
            if ogeom_algo::is_shell_closed(&reader.model, shell)? {
                let solid = make_solid(&mut reader.model, std::slice::from_ref(shell))?.shape;
                solids.push((0, solid));
            } else {
                sheets.push(shell.clone());
            }
        }
    }

    // Constructive solids: every independent primitive, boolean tree,
    // assembly and instance.
    let csg_des: Vec<i64> = file
        .entities
        .iter()
        .filter(|(de, e)| {
            CSG_KINDS.contains(&e.kind)
                && (e.status / 10_000) % 100 == 0
                && !reader.visited.contains_key(de)
        })
        .map(|(de, _)| *de)
        .collect();
    for de in csg_des {
        match reader.csg_solids(de) {
            Ok(built) => {
                for solid in built {
                    built_from.push((de, solid.clone()));
                    solids.push((de, solid));
                }
            }
            Err(e) => reader
                .report
                .warnings
                .push(format!("D{de}: constructive solid failed to build: {e}")),
        }
    }

    // Subfigure instances: each independent 408 places its definition's
    // solids and trimmed surfaces, built once and shared by every instance.
    let (placed_solids, placed_sheets, placed_from) = reader.subfigure_instances(tol)?;
    solids.extend(placed_solids.into_iter().map(|s| (0, s)));
    sheets.extend(placed_sheets);
    built_from.extend(placed_from);

    if solids.is_empty() && sheets.is_empty() {
        // A deck that *had* candidates which all failed is a different
        // refusal from one with nothing to try, and the warnings say why.
        if reader.report.warnings.is_empty() {
            ogeom_bail!(
                Construction,
                "the IGES file contains no manifold solid and no independent \
                 trimmed surface this reader translates"
            );
        }
        ogeom_bail!(
            Construction,
            "every shape in the IGES file failed to build: {}",
            reader.report.warnings.join("; ")
        );
    }

    if reader.vertex_misses.0 > 0 {
        let (count, worst) = reader.vertex_misses;
        reader.report.warnings.push(format!(
            "{count} vertices sat off the curve ends they bound, by up to {worst:.2e}; \
             their tolerances grew to say so"
        ));
    }
    let (callouts, dimensions) = reader.annotations();
    // Groups and level lists become layers once the document stands.
    for (de, entity) in &file.entities {
        if (entity.kind == 402 && matches!(entity.form, 1 | 7 | 14 | 15))
            || (entity.kind == 406 && entity.form == 1)
        {
            reader.visited.insert(*de, ());
        }
    }
    if unsewn {
        // A type-142 boundary can provide both its 3D trim and an advisory
        // parameter-space curve. The reader uses the 3D trim where present;
        // validate the owned alternate definition too, so a redundant
        // pcurve is not mistaken for omitted independent model geometry.
        let alternatives: Vec<i64> = file
            .entities
            .iter()
            .filter_map(|(de, entity)| {
                (entity.kind == 142
                    && reader.visited.contains_key(de)
                    && entity.at(2).int() != 0
                    && entity.at(3).int() != 0)
                    .then_some(entity.at(2).int())
            })
            .collect();
        for pointer in alternatives {
            if !reader.visited.contains_key(&pointer.abs()) {
                reader.curve_segments(pointer)?;
            }
        }
    }
    // Everything never visited, counted by type and form.
    for (de, entity) in &file.entities {
        if !reader.visited.contains_key(de) {
            let key = if entity.form == 0 {
                format!("type {}", entity.kind)
            } else {
                format!("type {} form {}", entity.kind, entity.form)
            };
            *reader.report.skipped.entry(key).or_default() += 1;
        }
    }
    let mut document = reader.document(&solids, &sheets);
    {
        let pmi = document.pmi_mut();
        let base = pmi.dimensions.len();
        for (i, dimension) in dimensions.into_iter().enumerate() {
            let (callout, dimension) = dimension;
            pmi.dimensions.push(dimension);
            let mut callout = callout;
            callout.annotates = Some(ogeom_doc::Annotated::Dimension(base + i));
            pmi.callouts.push(callout);
        }
        pmi.callouts.extend(callouts);
    }
    layers(&file, &mut document, &built_from, &mut reader.report);
    let solids = solids.into_iter().map(|(_, s)| s).collect();
    Ok(IgesImport {
        document,
        solids,
        sheets,
        report: reader.report,
    })
}

struct Reader<'a> {
    file: &'a File,
    model: Model,
    report: IgesReport,
    /// Every directory entry the reader consumed, for the skipped table.
    visited: BTreeMap<i64, ()>,
    /// Vertices by (vertex-list DE, 1-based index), shared, which is what
    /// lets a closed shell close.
    vertices: FastMap<(i64, i64), Shape>,
    /// Edges by (edge-list DE, 1-based index), for the same reason.
    edges: FastMap<(i64, i64), BuiltEdge>,
    /// Edges cut where their curve runs through a pole of a face they
    /// bound: the pieces, in order along the curve.
    pieces: FastMap<(i64, i64), Vec<Shape>>,
    /// The vertices made at poles, shared by every piece that meets one.
    pole_vertices: Vec<Shape>,
    /// The chart poles of every surface among the trimmed surfaces being
    /// read to sew together: each boundary segment through one is cut
    /// there, whichever face it bounds.
    trim_poles: Vec<Point>,
    /// Vertices a curve end missed by more than the confusion tolerance
    /// and that widened to cover it: how many, and the widest miss.
    vertex_misses: (usize, f64),
    /// Edge images derived ahead of the face walk, by the face (510) that
    /// wants them and the bits of the edge's range, each with the curve
    /// it was derived for: [`Reader::image_of`] takes one whose curve is
    /// the edge's, and derives any other itself.
    images: FastMap<(i64, u64, u64), Vec<(Curve, DerivedImage)>>,
    /// The face (510) being built, whose prepared images apply.
    face_de: Option<i64>,
    /// How many curve, surface or shell builders are open on the stack.
    /// Entities refer to entities of their own kind (a composite curve to
    /// its pieces, an offset surface to its basis), so a file that refers
    /// in a circle would otherwise recurse until the stack runs out.
    depth: u32,
    tol: Tolerances,
}

/// How deep entities of one kind may refer to one another. Real files nest
/// a handful of levels; a chain this long is a cycle or a hostile file.
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

    fn entity(&mut self, de: i64) -> OgeomResult<&'a Entity> {
        let Some(entity) = self.file.entity(de) else {
            ogeom_bail!(Construction, "IGES pointer D{de} names no entity");
        };
        self.visited.insert(de.abs(), ());
        Ok(entity)
    }

    /// The model-space transform an entity carries, identity when none. A
    /// transformation entity may itself be transformed; that composes.
    fn placement(&mut self, entity: &Entity) -> OgeomResult<Transform> {
        self.placement_within(entity, 0)
    }

    /// [`Self::placement`], `depth` transforms in. A chain of transforms
    /// longer than any real file writes is a cycle (a transform placed by
    /// itself), refused rather than followed until the stack runs out.
    fn placement_within(&mut self, entity: &Entity, depth: u32) -> OgeomResult<Transform> {
        const MOST_CHAINED: u32 = 32;
        if entity.transform == 0 {
            return Ok(Transform::IDENTITY);
        }
        if depth >= MOST_CHAINED {
            ogeom_bail!(
                Construction,
                "transforms chain more than {MOST_CHAINED} deep; the file places a \
                 transform by itself"
            );
        }
        let de = entity.transform;
        let t = self.entity(de)?;
        if t.kind != 124 {
            ogeom_bail!(
                Construction,
                "D{de}: a transformation pointer names a type {} entity",
                t.kind
            );
        }
        let v = |i: usize| t.at(i).real();
        let s = self.report.scale_mm;
        let linear = Matrix3::new([[v(0), v(1), v(2)], [v(4), v(5), v(6)], [v(8), v(9), v(10)]]);
        let translation = Vector::new(v(3) * s, v(7) * s, v(11) * s);
        let m = Transform::from_parts(linear, 1.0, translation, self.tol.angular())?;
        if t.transform != 0 {
            let outer = self.placement_within(t, depth + 1)?;
            return Ok(outer * m);
        }
        Ok(m)
    }

    fn point3(&self, e: &Entity, i: usize) -> Point {
        let s = self.report.scale_mm;
        Point::new(
            e.at(i).real() * s,
            e.at(i + 1).real() * s,
            e.at(i + 2).real() * s,
        )
    }

    /// A model-space curve with the range its own definition covers.
    fn curve(&mut self, de: i64) -> OgeomResult<(Curve, (f64, f64))> {
        self.nested(|reader| reader.curve_at(de))
    }

    fn curve_at(&mut self, de: i64) -> OgeomResult<(Curve, (f64, f64))> {
        let entity = self.entity(de)?;
        let scale = self.report.scale_mm;
        let (curve, range) = match entity.kind {
            110 => {
                let a = self.point3(entity, 0);
                let b = self.point3(entity, 3);
                let line = LineCurve::segment(a, b, self.tol)?;
                (Curve::from(line), (0.0, a.distance(b)))
            }
            100 => {
                // Centre, start and end in the definition plane at z = zt;
                // the arc runs counter-clockwise from start to end.
                let zt = entity.at(0).real() * scale;
                let c = Point::new(entity.at(1).real() * scale, entity.at(2).real() * scale, zt);
                let s = Point::new(entity.at(3).real() * scale, entity.at(4).real() * scale, zt);
                let e = Point::new(entity.at(5).real() * scale, entity.at(6).real() * scale, zt);
                let radius = c.distance(s);
                let x = Direction::new(s - c, self.tol)?;
                let frame = Frame::new(c, Direction::Z, x, self.tol)?;
                let circle = Circle::new(frame, radius, self.tol)?;
                let to_end = e - c;
                let ang = to_end
                    .dot(frame.y().vector())
                    .atan2(to_end.dot(frame.x().vector()))
                    .rem_euclid(core::f64::consts::TAU);
                let sweep = if ang <= self.tol.parametric() {
                    core::f64::consts::TAU
                } else {
                    ang
                };
                (Curve::from(CircleCurve::new(circle)), (0.0, sweep))
            }
            104 => self.conic(de, entity)?,
            112 => self.spline_curve(de, entity)?,
            126 => self.nurbs_curve(de, entity)?,
            130 => self.offset_curve(de, entity)?,
            102 => ogeom_bail!(
                Construction,
                "D{de}: a composite curve is a sequence, not a curve; the \
                 caller walks its segments"
            ),
            kind => ogeom_bail!(
                Construction,
                "D{de}: curve entity type {kind}{} is not translated; see \
                 docs/PARITY.md, io.iges",
                super::entity_name(kind).map_or_else(String::new, |n| format!(" ({n})"))
            ),
        };
        // The entity's placement moves the curve into model space.
        let placement = self.placement(entity)?;
        if placement == Transform::IDENTITY {
            Ok((curve, range))
        } else {
            Ok((curve.transformed(&placement, self.tol)?, range))
        }
    }

    /// Conic arc: `A x² + B xy + C y² + D x + E y + F = 0` in the definition
    /// plane. The coefficients say which conic it is (both squares one sign
    /// an ellipse, opposite signs a hyperbola, one square missing a
    /// parabola), and each translates to its own curve, the arc's ends read
    /// off the start and terminate points. A conic whose axes turn (`B` not
    /// zero) is read in the frame turned by `θ`, `tan 2θ = B / (A − C)`,
    /// where its cross term vanishes, and turned back: the curve is exact
    /// either way.
    fn conic(&mut self, de: i64, entity: &Entity) -> OgeomResult<(Curve, (f64, f64))> {
        let scale = self.report.scale_mm;
        let (a, b, c, d, e, f) = (
            entity.at(0).real(),
            entity.at(1).real(),
            entity.at(2).real(),
            entity.at(3).real(),
            entity.at(4).real(),
            entity.at(5).real(),
        );
        let zt = entity.at(6).real() * scale;
        let start = Point::new(entity.at(7).real() * scale, entity.at(8).real() * scale, zt);
        let end = Point::new(
            entity.at(9).real() * scale,
            entity.at(10).real() * scale,
            zt,
        );
        if b.abs() > 1e-12 {
            let theta = 0.5 * b.atan2(a - c);
            let (sin, cos) = theta.sin_cos();
            // x = x' cos θ − y' sin θ, y = x' sin θ + y' cos θ.
            let turned = [
                a * cos * cos + b * cos * sin + c * sin * sin,
                a * sin * sin - b * cos * sin + c * cos * cos,
                d * cos + e * sin,
                -d * sin + e * cos,
                f,
            ];
            let about = ogeom_math::Axis::new(Point::ORIGIN, Direction::Z);
            let back = Transform::rotation(about, theta);
            let into = Transform::rotation(about, -theta);
            let (curve, range) =
                self.axis_aligned_conic(de, turned, zt, into.apply(start), into.apply(end))?;
            return Ok((curve.transformed(&back, self.tol)?, range));
        }
        self.axis_aligned_conic(de, [a, c, d, e, f], zt, start, end)
    }

    /// [`Reader::conic`] once its axes are the definition plane's own:
    /// `A x² + C y² + D x + E y + F = 0`.
    fn axis_aligned_conic(
        &mut self,
        de: i64,
        [a, c, d, e, f]: [f64; 5],
        zt: f64,
        start: Point,
        end: Point,
    ) -> OgeomResult<(Curve, (f64, f64))> {
        let scale = self.report.scale_mm;
        // Both squares present and of one sign: an ellipse, its coefficients
        // made positive.
        if a != 0.0 && c != 0.0 && (a > 0.0) == (c > 0.0) {
            let sign = if a > 0.0 { 1.0 } else { -1.0 };
            return self.ellipse_arc(de, [a, c, d, e, f].map(|k| k * sign), zt, start, end);
        }
        if a != 0.0 && c != 0.0 {
            return Self::hyperbola_arc(de, [a, c, d, e, f], zt, start, end, scale, self.tol);
        }
        if (a == 0.0) != (c == 0.0) {
            return Self::parabola_arc(de, [a, c, d, e, f], zt, start, end, scale, self.tol);
        }
        ogeom_bail!(Construction, "D{de}: conic arc coefficients close no conic")
    }

    /// The ellipse arm of [`Reader::conic`]: `A x² + C y² + D x + E y + F = 0`
    /// with `A` and `C` positive.
    fn ellipse_arc(
        &mut self,
        de: i64,
        [a, c, d, e, f]: [f64; 5],
        zt: f64,
        start: Point,
        end: Point,
    ) -> OgeomResult<(Curve, (f64, f64))> {
        let scale = self.report.scale_mm;
        let cx = -d / (2.0 * a);
        let cy = -e / (2.0 * c);
        let rhs = a * cx * cx + c * cy * cy - f;
        let (ra2, rb2) = (rhs / a, rhs / c);
        if ra2 <= 0.0 || rb2 <= 0.0 {
            ogeom_bail!(
                Construction,
                "D{de}: conic arc coefficients close no ellipse"
            );
        }
        let centre = Point::new(cx * scale, cy * scale, zt);
        let (rx, ry) = (ra2.sqrt() * scale, rb2.sqrt() * scale);
        // The ellipse type wants major ≥ minor; when the x semi-axis is the
        // smaller one, a quarter-turn of the frame swaps the roles.
        let (frame, major, minor) = if rx >= ry {
            (
                Frame::new(centre, Direction::Z, Direction::X, self.tol)?,
                rx,
                ry,
            )
        } else {
            (
                Frame::new(centre, Direction::Z, Direction::Y, self.tol)?,
                ry,
                rx,
            )
        };
        let ellipse = Ellipse::new(frame, major, minor, self.tol)?;
        let angle_of = |p: Point| -> f64 {
            let local = frame.to_local(p);
            (local.y / minor)
                .atan2(local.x / major)
                .rem_euclid(core::f64::consts::TAU)
        };
        let t0 = angle_of(start);
        let mut t1 = angle_of(end);
        if t1 <= t0 + self.tol.parametric() {
            t1 += core::f64::consts::TAU;
        }
        Ok((Curve::from(EllipseCurve::new(ellipse)), (t0, t1)))
    }

    /// The hyperbola arm of [`Reader::conic`]: squares of opposite sign.
    /// The branch is the one the arc's ends stand on, its parameter the
    /// natural one, `(a cosh t, b sinh t)`, and the arc runs in increasing
    /// parameter, the frame turned over where the file's ends run the other
    /// way.
    fn hyperbola_arc(
        de: i64,
        [a, c, d, e, f]: [f64; 5],
        zt: f64,
        start: Point,
        end: Point,
        scale: f64,
        tol: Tolerances,
    ) -> OgeomResult<(Curve, (f64, f64))> {
        let cx = -d / (2.0 * a);
        let cy = -e / (2.0 * c);
        let rhs = a * cx * cx + c * cy * cy - f;
        if rhs == 0.0 {
            ogeom_bail!(
                Construction,
                "D{de}: conic arc coefficients close no hyperbola; they cross"
            );
        }
        // Divided through by the right-hand side, the positive square names
        // the transverse axis.
        let (pa, pc) = (a / rhs, c / rhs);
        let (major, minor, along_x) = if pa > 0.0 {
            ((1.0 / pa).sqrt(), (-1.0 / pc).sqrt(), true)
        } else {
            ((1.0 / pc).sqrt(), (-1.0 / pa).sqrt(), false)
        };
        let centre = Point::new(cx * scale, cy * scale, zt);
        let axis = if along_x { Direction::X } else { Direction::Y };
        // The branch: where the ends stand along the transverse axis.
        let side = (start - centre).dot(axis.vector());
        let x = if side >= 0.0 {
            axis
        } else {
            Direction::new(-axis.vector(), tol)?
        };
        let build = |z: Direction| -> OgeomResult<(Curve, (f64, f64))> {
            let frame = Frame::new(centre, z, x, tol)?;
            let (major, minor) = (major * scale, minor * scale);
            let hyperbola = Hyperbola::new(frame, major, minor, tol)?;
            let t_of = |p: Point| (frame.to_local(p).y / minor).asinh();
            let (t0, t1) = (t_of(start), t_of(end));
            let extent = t0.abs().max(t1.abs()).max(1e-3) * 1.5;
            Ok((
                Curve::from(HyperbolaCurve::new(hyperbola, extent)?),
                (t0, t1),
            ))
        };
        let (curve, (t0, t1)) = build(Direction::Z)?;
        if t1 > t0 {
            return Ok((curve, (t0, t1)));
        }
        // Turned over: the same branch, the parameter running the other way.
        let (curve, (t0, t1)) = build(Direction::new(-Direction::Z.vector(), tol)?)?;
        Ok((curve, (t0, t1)))
    }

    /// The parabola arm of [`Reader::conic`]: one square missing. The axis
    /// is the missing square's direction; the parameter runs along the
    /// other, as this vocabulary's parabola does.
    fn parabola_arc(
        de: i64,
        [a, c, d, e, f]: [f64; 5],
        zt: f64,
        start: Point,
        end: Point,
        scale: f64,
        tol: Tolerances,
    ) -> OgeomResult<(Curve, (f64, f64))> {
        // `A x² + D x + E y + F = 0` opens along y; `C y² + D x + E y + F = 0`
        // along x. Either is `axis = apex + k (across - apex)²`.
        let (apex_across, apex_along, k, along) = if c == 0.0 {
            if e == 0.0 {
                ogeom_bail!(
                    Construction,
                    "D{de}: conic arc coefficients close no parabola"
                );
            }
            let x0 = -d / (2.0 * a);
            let y0 = -(a * x0 * x0 + d * x0 + f) / e;
            (x0, y0, -a / e, Direction::Y)
        } else {
            if d == 0.0 {
                ogeom_bail!(
                    Construction,
                    "D{de}: conic arc coefficients close no parabola"
                );
            }
            let y0 = -e / (2.0 * c);
            let x0 = -(c * y0 * y0 + e * y0 + f) / d;
            (y0, x0, -c / d, Direction::X)
        };
        let apex = if c == 0.0 {
            Point::new(apex_across * scale, apex_along * scale, zt)
        } else {
            Point::new(apex_along * scale, apex_across * scale, zt)
        };
        // The axis points the way the parabola opens.
        let x = if k >= 0.0 {
            along
        } else {
            Direction::new(-along.vector(), tol)?
        };
        // `axis = k across²` against `axis = across² / (4 f)`.
        let focal = scale / (4.0 * k.abs());
        let build = |z: Direction| -> OgeomResult<(Curve, (f64, f64))> {
            let frame = Frame::new(apex, z, x, tol)?;
            let parabola = Parabola::new(frame, focal, tol)?;
            let t_of = |p: Point| frame.to_local(p).y;
            let (t0, t1) = (t_of(start), t_of(end));
            let extent = t0.abs().max(t1.abs()).max(1e-3) * 1.5;
            Ok((Curve::from(ParabolaCurve::new(parabola, extent)?), (t0, t1)))
        };
        let (curve, (t0, t1)) = build(Direction::Z)?;
        if t1 > t0 {
            return Ok((curve, (t0, t1)));
        }
        let (curve, (t0, t1)) = build(Direction::new(-Direction::Z.vector(), tol)?)?;
        Ok((curve, (t0, t1)))
    }

    /// Offset curve (130): a base curve displaced perpendicular to a
    /// reference direction. The file displaces along the reference crossed
    /// with the tangent; this vocabulary's offset runs along the tangent
    /// crossed with the reference, so the distance flips sign. A constant
    /// distance is the exact offset curve. A distance that varies (type 2,
    /// linearly with arc length from `D1` at `TD1` to `D2` at `TD2`; type 3,
    /// as a coordinate of another curve at the same parameter) has no
    /// closed form, and the displaced curve is fitted, same-parameter with
    /// the base curve, to a hundredth of a micron between its samples as
    /// well as at them, or the reach is warned of.
    fn offset_curve(&mut self, de: i64, entity: &Entity) -> OgeomResult<(Curve, (f64, f64))> {
        let scale = self.report.scale_mm;
        let kind = entity.at(1).int();
        let (d1, d2) = (entity.at(5).real(), entity.at(7).real());
        if kind != 1 || (d1 - d2).abs() > 1e-12 {
            return self.varying_offset(de, entity, kind);
        }
        let (basis, range) = self.curve(entity.at(0).int())?;
        let reference = Direction::from_coords(
            entity.at(9).real(),
            entity.at(10).real(),
            entity.at(11).real(),
            self.tol,
        )?;
        let (tt1, tt2) = (entity.at(12).real(), entity.at(13).real());
        let range = if tt2 > tt1 { (tt1, tt2) } else { range };
        let offset = OffsetCurve::new(basis, -d1 * scale, reference)?;
        Ok((Curve::Offset(Box::new(offset)), range))
    }

    /// The varying arm of [`Reader::offset_curve`].
    fn varying_offset(
        &mut self,
        de: i64,
        entity: &Entity,
        kind: i64,
    ) -> OgeomResult<(Curve, (f64, f64))> {
        let scale = self.report.scale_mm;
        let (basis, range) = self.curve(entity.at(0).int())?;
        let reference = Direction::from_coords(
            entity.at(9).real(),
            entity.at(10).real(),
            entity.at(11).real(),
            self.tol,
        )?
        .vector();
        let (tt1, tt2) = (entity.at(12).real(), entity.at(13).real());
        let range = if tt2 > tt1 { (tt1, tt2) } else { range };
        // Sampled at every span of the base, and the fit held to its
        // target between the samples as well as at them, against the law
        // evaluated wherever the fit is measured.
        const SAMPLES: usize = 400;
        let stations = ogeom_algo::traced::stations(&basis, range, SAMPLES);
        let distance: Box<dyn Fn(f64) -> OgeomResult<f64>> = match kind {
            2 => {
                // Linear in arc length along the base, from TD1 to TD2: the
                // length to each station integrated once, and from the
                // station below to any parameter on demand.
                let (d1, td1, d2, td2) = (
                    entity.at(5).real(),
                    entity.at(6).real(),
                    entity.at(7).real(),
                    entity.at(8).real(),
                );
                let mut lengths = vec![0.0_f64];
                for pair in stations.windows(2) {
                    let held = lengths[lengths.len() - 1];
                    lengths.push(
                        held + ogeom_algo::curve_length(&basis, (pair[0], pair[1]), self.tol)?
                            / scale,
                    );
                }
                let span = td2 - td1;
                let (basis, stations, tol) = (basis.clone(), stations.clone(), self.tol);
                Box::new(move |t| {
                    let i = stations.partition_point(|&s| s <= t).saturating_sub(1);
                    let along = lengths[i]
                        + ogeom_algo::curve_length(&basis, (stations[i], t), tol)? / scale;
                    let f = if span.abs() > 0.0 {
                        (along - td1) / span
                    } else {
                        0.0
                    };
                    Ok(d1 + (d2 - d1) * f)
                })
            }
            3 => {
                // A coordinate (TT: 1 x, 2 y, 3 z) of another curve at the
                // same parameter.
                let (function, _) = self.curve(entity.at(2).int())?;
                let which = entity.at(3).int();
                let tol = self.tol;
                Box::new(move |t| {
                    let p = function.point_at(t, tol)?;
                    let value = match which {
                        1 => p.x,
                        2 => p.y,
                        _ => p.z,
                    };
                    Ok(value / scale)
                })
            }
            _ => ogeom_bail!(
                Construction,
                "D{de}: offset curve type {kind} names no distance law"
            ),
        };
        let tol = self.tol;
        let displaced = |t: f64| -> OgeomResult<Point> {
            let p = basis.point_at(t, tol)?;
            let tangent = basis.d1_at(t, tol)?;
            let side = reference.cross(tangent);
            let m = side.magnitude();
            if m <= tol.angular() {
                ogeom_bail!(
                    Construction,
                    "D{de}: the offset's reference runs along its base curve"
                );
            }
            Ok(p + side * (distance(t)? * scale / m))
        };
        let fitted = ogeom_algo::traced::fit_traced(
            displaced,
            &stations,
            3,
            self.tol.confusion() * 100.0,
            self.tol,
        )?;
        if !fitted.met {
            self.report.warnings.push(format!(
                "D{de}: a varying offset curve fitted to {:.2e}",
                fitted.error
            ));
        }
        Ok((Curve::from(fitted.curve), range))
    }

    fn spline_curve(&mut self, de: i64, entity: &Entity) -> OgeomResult<(Curve, (f64, f64))> {
        let n = entity.count(3);
        if n == 0 {
            ogeom_bail!(Construction, "D{de}: a spline curve with no segments");
        }
        let scale = self.report.scale_mm;
        // Break points T(1..=N+1), then 12 coefficients per segment; the
        // polynomial argument runs over [0, h] within each span.
        let t = |i: usize| entity.at(4 + i).real();
        let base = 4 + n + 1;
        let mut control: Vec<Point> = Vec::with_capacity(3 * n + 1);
        let mut knots: Vec<f64> = vec![t(0); 4];
        for seg in 0..n {
            let h = t(seg + 1) - t(seg);
            if h <= 0.0 {
                ogeom_bail!(Construction, "D{de}: spline segment {seg} has no span");
            }
            let co = |k: usize| entity.at(base + 12 * seg + k).real();
            let (ax, bx, cx, dx) = (co(0), co(1), co(2), co(3));
            let (ay, by, cy, dy) = (co(4), co(5), co(6), co(7));
            let (az, bz, cz, dz) = (co(8), co(9), co(10), co(11));
            let at = |s: f64| {
                Point::new(
                    (ax + s * (bx + s * (cx + s * dx))) * scale,
                    (ay + s * (by + s * (cy + s * dy))) * scale,
                    (az + s * (bz + s * (cz + s * dz))) * scale,
                )
            };
            // Bernstein form of a cubic on [0, h]: the endpoints, and one
            // third of the end derivatives standing off them.
            let p0 = at(0.0);
            let p3 = at(h);
            let d0 = Vector::new(bx, by, bz) * (h * scale / 3.0);
            let d1 = Vector::new(
                bx + 2.0 * cx * h + 3.0 * dx * h * h,
                by + 2.0 * cy * h + 3.0 * dy * h * h,
                bz + 2.0 * cz * h + 3.0 * dz * h * h,
            ) * (h * scale / 3.0);
            if seg == 0 {
                control.push(p0);
            }
            control.push(p0 + d0);
            control.push(p3 - d1);
            control.push(p3);
            if seg + 1 < n {
                knots.extend([t(seg + 1); 3]);
            }
        }
        knots.extend([t(n); 4]);
        let curve = BSplineCurve::new(KnotVector::new(knots, 3)?, control, self.tol)?;
        let range = ogeom_geom::Curve3d::domain(&curve);
        Ok((Curve::from(curve), range))
    }

    /// Rational B-spline curve, the direct translation.
    fn nurbs_curve(&mut self, de: i64, entity: &Entity) -> OgeomResult<(Curve, (f64, f64))> {
        let k = entity.count(0);
        let degree = entity.count(1);
        if degree == 0 {
            ogeom_bail!(Construction, "D{de}: a B-spline curve of degree zero");
        }
        let n_ctrl = k + 1;
        let n_knots = n_ctrl + degree + 1;
        let knots: Vec<f64> = (0..n_knots).map(|i| entity.at(6 + i).real()).collect();
        let w_base = 6 + n_knots;
        let p_base = w_base + n_ctrl;
        let scale = self.report.scale_mm;
        let mut control = Vec::with_capacity(n_ctrl);
        for i in 0..n_ctrl {
            let w = entity.at(w_base + i).real();
            let p = Point::new(
                entity.at(p_base + 3 * i).real() * scale,
                entity.at(p_base + 3 * i + 1).real() * scale,
                entity.at(p_base + 3 * i + 2).real() * scale,
            );
            control.push(Weighted::new(p, w, self.tol)?);
        }
        let curve = BSplineCurve::rational(KnotVector::new(knots, degree)?, control)?;
        let v0 = entity.at(p_base + 3 * n_ctrl).real();
        let v1 = entity.at(p_base + 3 * n_ctrl + 1).real();
        let domain = ogeom_geom::Curve3d::domain(&curve);
        let range = if v1 > v0 { (v0, v1) } else { domain };
        Ok((Curve::from(curve), range))
    }

    /// A model-space surface.
    fn surface(&mut self, de: i64) -> OgeomResult<SurfaceGeometry> {
        self.nested(|reader| reader.surface_at(de))
    }

    fn surface_at(&mut self, de: i64) -> OgeomResult<SurfaceGeometry> {
        let entity = self.entity(de)?;
        let scale = self.report.scale_mm;
        let surface: SurfaceGeometry = match entity.kind {
            108 => {
                // A x + B y + C z = D, unbounded; any face's trim bounds it.
                let normal = Vector::new(
                    entity.at(0).real(),
                    entity.at(1).real(),
                    entity.at(2).real(),
                );
                let d = entity.at(3).real();
                let origin = Point::ORIGIN + normal * (d / normal.dot(normal)) * scale;
                let plane = Plane::through(origin, Direction::new(normal, self.tol)?);
                PlaneSurface::over(
                    plane,
                    (-SURFACE_EXTENT, SURFACE_EXTENT),
                    (-SURFACE_EXTENT, SURFACE_EXTENT),
                )?
                .into()
            }
            190 => {
                let point = self.location_entity(entity.at(0).int())?;
                let normal = self.direction_entity(entity.at(1).int())?;
                // A parameterized plane names the direction its `u` runs.
                let plane = match self.reference_direction(entity, 2)? {
                    Some(x) => Plane::new(Frame::new(point, normal, x, self.tol)?),
                    None => Plane::through(point, normal),
                };
                PlaneSurface::over(
                    plane,
                    (-SURFACE_EXTENT, SURFACE_EXTENT),
                    (-SURFACE_EXTENT, SURFACE_EXTENT),
                )?
                .into()
            }
            120 => {
                // Axis line, generatrix, start and terminate angles.
                let axis = {
                    let (line, _) = self.curve(entity.at(0).int())?;
                    let Curve::Line(l) = line else {
                        ogeom_bail!(
                            Construction,
                            "D{de}: a surface of revolution's axis is not a line"
                        );
                    };
                    l.axis()
                };
                let (curve, range) = self.curve(entity.at(1).int())?;
                let mut curve = trimmed_to(curve, range, self.tol)?;
                let sa = entity.at(2).real();
                let ta = entity.at(3).real();
                let sweep = if ta > sa {
                    ta - sa
                } else {
                    core::f64::consts::TAU
                };
                if sa.abs() > self.tol.parametric() {
                    curve = curve.transformed(&Transform::rotation(axis, sa), self.tol)?;
                }
                RevolutionSurface::new(curve, axis, sweep)?.into()
            }
            122 => {
                // Directrix, plus the far end of a generator drawn through
                // the directrix's start point.
                let (curve, range) = self.curve(entity.at(0).int())?;
                let start = curve.point_at(range.0, self.tol)?;
                let far = self.point3(entity, 1);
                let vec = far - start;
                let curve = trimmed_to(curve, range, self.tol)?;
                ExtrusionSurface::new(curve, Direction::new(vec, self.tol)?, vec.magnitude())?
                    .into()
            }
            128 => self.nurbs_surface(de, entity)?,
            114 => self.spline_surface(de, entity)?,
            118 => self.ruled_surface(de, entity)?,
            140 => {
                // Normal, distance, base surface: displaced along the
                // file's direction, which is the base's own normal or its
                // opposite; the sign carries the difference.
                let given = Direction::from_coords(
                    entity.at(0).real(),
                    entity.at(1).real(),
                    entity.at(2).real(),
                    self.tol,
                )?;
                let distance = entity.at(3).real() * scale;
                let basis = self.surface(entity.at(4).int())?;
                let ((ua, ub), (va, vb)) = ogeom_geom::Surface::domain(&basis);
                let own = ogeom_geom::Surface::normal_at(
                    &basis,
                    f64::midpoint(ua, ub),
                    f64::midpoint(va, vb),
                    self.tol,
                )?;
                let signed = if own.vector().dot(given.vector()) < 0.0 {
                    -distance
                } else {
                    distance
                };
                SurfaceGeometry::Offset(Box::new(OffsetSurface::new(basis, signed)?))
            }
            192 => {
                let point = self.location_entity(entity.at(0).int())?;
                let dir = self.direction_entity(entity.at(1).int())?;
                let radius = entity.at(2).real() * scale;
                let frame = self.framed(point, dir, entity, 3)?;
                CylinderSurface::new(
                    Cylinder::new(frame, radius, self.tol)?,
                    (-SURFACE_EXTENT, SURFACE_EXTENT),
                )?
                .into()
            }
            194 => {
                let point = self.location_entity(entity.at(0).int())?;
                let dir = self.direction_entity(entity.at(1).int())?;
                let radius = entity.at(2).real() * scale;
                let half_angle = entity.at(3).real().to_radians();
                let frame = self.framed(point, dir, entity, 4)?;
                ConeSurface::new(
                    Cone::new(frame, radius, half_angle, self.tol)?,
                    (-SURFACE_EXTENT, SURFACE_EXTENT),
                )?
                .into()
            }
            196 => {
                let centre = self.location_entity(entity.at(0).int())?;
                let radius = entity.at(1).real() * scale;
                // A parameterized sphere names its axis and where `u` starts.
                if entity.at(2).int() != 0 {
                    let axis = self.direction_entity(entity.at(2).int())?;
                    let frame = self.framed(centre, axis, entity, 3)?;
                    SphereSurface::new(Sphere::new(frame, radius, self.tol)?).into()
                } else {
                    SphereSurface::new(Sphere::centred(centre, radius, self.tol)?).into()
                }
            }
            198 => {
                let centre = self.location_entity(entity.at(0).int())?;
                let dir = self.direction_entity(entity.at(1).int())?;
                let major = entity.at(2).real() * scale;
                let minor = entity.at(3).real() * scale;
                let frame = self.framed(centre, dir, entity, 4)?;
                TorusSurface::new(Torus::new(frame, major, minor, self.tol)?).into()
            }
            kind => ogeom_bail!(
                Construction,
                "D{de}: surface entity type {kind}{} is not translated; see \
                 docs/PARITY.md, io.iges",
                super::entity_name(kind).map_or_else(String::new, |n| format!(" ({n})"))
            ),
        };
        let placement = self.placement(entity)?;
        if placement == Transform::IDENTITY {
            Ok(surface)
        } else {
            Ok(surface.transformed(&placement, self.tol)?)
        }
    }

    /// Ruled surface (118): the straight lines between points of equal
    /// parameter on two curves, as a patch of degree one across. Both
    /// curves in their exact spline form over `[0, 1]`, raised to one
    /// degree and refined to one knot vector, the second walked backward
    /// where the file's direction flag says so. The file's form 0 joins
    /// points of equal *arc length* fraction; that is read as equal
    /// parameter, exact for the uniform-speed curves and a hair off for the
    /// rest.
    fn ruled_surface(&mut self, de: i64, entity: &Entity) -> OgeomResult<SurfaceGeometry> {
        let (first, first_range) = self.curve(entity.at(0).int())?;
        let (second, second_range) = self.curve(entity.at(1).int())?;
        let mut a = first.to_bspline_over(first_range, self.tol)?;
        let mut b = second.to_bspline_over(second_range, self.tol)?;
        if entity.at(2).int() == 1 {
            let Curve::BSpline(turned) =
                ogeom_geom::Reversible::reversed(&Curve::BSpline(b.clone()))
            else {
                ogeom_bail!(Construction, "D{de}: a reversed spline is not a spline");
            };
            b = turned;
        }
        while a.degree() < b.degree() {
            a = a.elevated(self.tol)?;
        }
        while b.degree() < a.degree() {
            b = b.elevated(self.tol)?;
        }
        let tol = self.tol;
        let unify = |target: &mut BSplineCurve, source: &BSplineCurve| -> OgeomResult<()> {
            let (lo, hi) = source.knots().domain();
            for (value, multiplicity) in source.knots().distinct() {
                if value <= lo + 1e-12 || value >= hi - 1e-12 {
                    continue;
                }
                let held = target.knots().multiplicity_of(value);
                if multiplicity > held {
                    *target = target.with_knot_inserted(value, multiplicity - held, tol)?;
                }
            }
            Ok(())
        };
        unify(&mut a, &b)?;
        unify(&mut b, &a)?;
        let nu = a.control_points().len();
        if b.control_points().len() != nu {
            ogeom_bail!(
                Construction,
                "D{de}: the ruled surface's curves refine to different nets"
            );
        }
        let mut points: Vec<Weighted<Point>> = Vec::with_capacity(nu * 2);
        for i in 0..nu {
            points.push(a.control_points()[i]);
            points.push(b.control_points()[i]);
        }
        let grid = ControlGrid::new(points, nu, 2)?;
        let across = KnotVector::new(vec![0.0, 0.0, 1.0, 1.0], 1)?;
        Ok(BSplineSurface::rational(a.knots().clone(), across, grid)?.into())
    }

    /// Parametric spline surface (114): a grid of bicubic polynomial
    /// patches, each over its own span of break points, its sixteen
    /// coefficients per coordinate multiplying `s^m t^l` at index `m + 4l`
    /// with `s` and `t` measured from the patch's first break points. Each
    /// patch is converted to Bézier form exactly (its monomials scaled to
    /// the unit square, then to the Bernstein basis both ways) and the
    /// patches joined on shared rows, every break point a knot of
    /// multiplicity three: the spline curve's (112) construction, both ways.
    /// After each row of patches the file carries one arbitrary patch, and
    /// after the last row one arbitrary row; both are skipped.
    fn spline_surface(&mut self, de: i64, entity: &Entity) -> OgeomResult<SurfaceGeometry> {
        let m = entity.count(2);
        let n = entity.count(3);
        if m == 0 || n == 0 {
            ogeom_bail!(Construction, "D{de}: a spline surface with no patches");
        }
        let scale = self.report.scale_mm;
        let tu = |i: usize| entity.at(4 + i).real();
        let tv = |j: usize| entity.at(4 + m + 1 + j).real();
        let base = 4 + (m + 1) + (n + 1);
        let (rows, columns) = (3 * m + 1, 3 * n + 1);
        let mut net = vec![Point::ORIGIN; rows * columns];
        // Monomial coefficients on [0, 1] to Bernstein control values:
        // `p_r = Σ_{k ≤ r} C(r, k) / C(3, k) · b_k`.
        const TO_BERNSTEIN: [[f64; 4]; 4] = [
            [1.0, 0.0, 0.0, 0.0],
            [1.0, 1.0 / 3.0, 0.0, 0.0],
            [1.0, 2.0 / 3.0, 1.0 / 3.0, 0.0],
            [1.0, 1.0, 1.0, 1.0],
        ];
        for i in 0..m {
            let h = tu(i + 1) - tu(i);
            for j in 0..n {
                let k = tv(j + 1) - tv(j);
                if h <= 0.0 || k <= 0.0 {
                    ogeom_bail!(
                        Construction,
                        "D{de}: spline surface patch ({i}, {j}) has no span"
                    );
                }
                let at = base + (i * (n + 1) + j) * 48;
                let mut coords = [[[0.0_f64; 4]; 4]; 3];
                for (axis, block) in coords.iter_mut().enumerate() {
                    // b[mu][lv]: the coefficient of σ^mu τ^lv on the unit square.
                    let mut b = [[0.0_f64; 4]; 4];
                    for (mu, row) in b.iter_mut().enumerate() {
                        for (lv, value) in row.iter_mut().enumerate() {
                            let raw = entity.at(at + 16 * axis + mu + 4 * lv).real();
                            #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
                            let scaled = raw * h.powi(mu as i32) * k.powi(lv as i32);
                            *value = scaled;
                        }
                    }
                    // Bernstein in s, then in t.
                    let mut q = [[0.0_f64; 4]; 4];
                    for r in 0..4 {
                        for lv in 0..4 {
                            q[r][lv] = (0..4).map(|mu| TO_BERNSTEIN[r][mu] * b[mu][lv]).sum();
                        }
                    }
                    for r in 0..4 {
                        for c in 0..4 {
                            block[r][c] = (0..4).map(|lv| TO_BERNSTEIN[c][lv] * q[r][lv]).sum();
                        }
                    }
                }
                for r in 0..4 {
                    for c in 0..4 {
                        net[(3 * i + r) * columns + 3 * j + c] = Point::new(
                            coords[0][r][c] * scale,
                            coords[1][r][c] * scale,
                            coords[2][r][c] * scale,
                        );
                    }
                }
            }
        }
        let knots = |count: usize, at: &dyn Fn(usize) -> f64| -> Vec<f64> {
            let mut out = vec![at(0); 4];
            for i in 1..count {
                out.extend([at(i); 3]);
            }
            out.extend([at(count); 4]);
            out
        };
        let grid = ControlGrid::new(
            net.into_iter()
                .map(|p| Weighted::new(p, 1.0, self.tol))
                .collect::<OgeomResult<Vec<_>>>()?,
            rows,
            columns,
        )?;
        Ok(ogeom_geom::BSplineSurface::rational(
            KnotVector::new(knots(m, &tu), 3)?,
            KnotVector::new(knots(n, &tv), 3)?,
            grid,
        )?
        .into())
    }

    fn nurbs_surface(&mut self, de: i64, entity: &Entity) -> OgeomResult<SurfaceGeometry> {
        let k1 = entity.count(0);
        let k2 = entity.count(1);
        let m1 = entity.count(2);
        let m2 = entity.count(3);
        if m1 == 0 || m2 == 0 {
            ogeom_bail!(Construction, "D{de}: a B-spline surface of degree zero");
        }
        let (nu, nv) = (k1 + 1, k2 + 1);
        if nu
            .checked_mul(nv)
            .is_none_or(|cells| cells > entity.params.len())
        {
            ogeom_bail!(
                Construction,
                "D{de}: a {nu} by {nv} control grid in {} parameters",
                entity.params.len()
            );
        }
        let (nku, nkv) = (nu + m1 + 1, nv + m2 + 1);
        let base = 9;
        let u_knots: Vec<f64> = (0..nku).map(|i| entity.at(base + i).real()).collect();
        let v_knots: Vec<f64> = (0..nkv).map(|i| entity.at(base + nku + i).real()).collect();
        let w_base = base + nku + nkv;
        let p_base = w_base + nu * nv;
        let scale = self.report.scale_mm;
        // The file lists control points with the first (u) index varying
        // fastest; the grid stores row-major with u as the slow index, so
        // the read reorders.
        let raw = |u: usize, v: usize| -> OgeomResult<Weighted<Point>> {
            let i = v * nu + u;
            let w = entity.at(w_base + i).real();
            let p = Point::new(
                entity.at(p_base + 3 * i).real() * scale,
                entity.at(p_base + 3 * i + 1).real() * scale,
                entity.at(p_base + 3 * i + 2).real() * scale,
            );
            Weighted::new(p, w, self.tol)
        };
        let mut weighted = Vec::with_capacity(nu * nv);
        for u in 0..nu {
            for v in 0..nv {
                weighted.push(raw(u, v)?);
            }
        }
        let grid = ControlGrid::new(weighted, nu, nv)?;
        Ok(ogeom_geom::BSplineSurface::rational(
            KnotVector::new(u_knots, m1)?,
            KnotVector::new(v_knots, m2)?,
            grid,
        )?
        .into())
    }

    /// A point entity (116) or bare coordinate triple carrier.
    fn location_entity(&mut self, de: i64) -> OgeomResult<Point> {
        let e = self.entity(de)?;
        if e.kind != 116 {
            ogeom_bail!(
                Construction,
                "D{de}: expected a point entity, found type {}",
                e.kind
            );
        }
        Ok(self.point3(e, 0))
    }

    /// A direction entity (123) as a unit vector.
    /// The reference direction a parameterized analytic surface names at
    /// `field`, where it names one: where its angle or `u` starts.
    fn reference_direction(
        &mut self,
        entity: &Entity,
        field: usize,
    ) -> OgeomResult<Option<Direction>> {
        let de = entity.at(field).int();
        if de == 0 || !self.file.entities.contains_key(&de) {
            return Ok(None);
        }
        Ok(Some(self.direction_entity(de)?))
    }

    /// A frame about `axis` at `origin`, its `x` the surface's reference
    /// direction where it names one (squared off the axis), else any.
    fn framed(
        &mut self,
        origin: Point,
        axis: Direction,
        entity: &Entity,
        field: usize,
    ) -> OgeomResult<Frame> {
        if let Some(reference) = self.reference_direction(entity, field)? {
            let r = reference.vector();
            let square = r - axis.vector() * r.dot(axis.vector());
            if let Ok(x) = Direction::new(square, self.tol) {
                return Frame::new(origin, axis, x, self.tol);
            }
        }
        frame_about(origin, axis, self.tol)
    }

    fn direction_entity(&mut self, de: i64) -> OgeomResult<Direction> {
        let e = self.entity(de)?;
        if e.kind != 123 {
            ogeom_bail!(
                Construction,
                "D{de}: expected a direction entity, found type {}",
                e.kind
            );
        }
        Direction::from_coords(e.at(0).real(), e.at(1).real(), e.at(2).real(), self.tol)
    }

    /// The boundary of a trimmed face as model-space curve segments: from a
    /// curve-on-surface (142), a boundary entity (141), or a bare curve,
    /// walking composite curves flat.
    fn boundary_segments(&mut self, de: i64) -> OgeomResult<Vec<(Curve, (f64, f64))>> {
        let entity = self.entity(de)?;
        match entity.kind {
            // Curve on surface: the model-space curve is the trim's truth;
            // the file's pcurve is advisory, because faces recompute exact
            // pcurves and say when they cannot.
            // Creation, surface, parameter-space curve, model-space curve.
            142 => {
                let c = entity.at(3).int();
                if c != 0 {
                    return self.curve_segments(c);
                }
                let (surface_de, b) = (entity.at(1).int(), entity.at(2).int());
                self.lifted_segments(de, surface_de, b)
            }
            141 => {
                let n = entity.count(3);
                let mut out = Vec::new();
                let mut i = 4;
                for _ in 0..n {
                    let cptr = entity.at(i).int();
                    let k = entity.count(i + 2);
                    i += 3 + k;
                    // The sense flag is advisory here too: the wire builder
                    // chains segments by their geometry.
                    out.extend(self.curve_segments(cptr)?);
                }
                Ok(out)
            }
            _ => self.curve_segments(de),
        }
    }

    /// A trim given only in the surface's parameters: each parameter-space
    /// segment lifted through the surface into a model-space curve, fitted
    /// same-parameter with the composition to a hundredth of a micron. On a
    /// B-spline surface the file's parameters are the surface's own; an
    /// analytic surface's IGES parameterization is the file's convention,
    /// not this kernel's, and its lift is refused by name.
    fn lifted_segments(
        &mut self,
        de: i64,
        surface_de: i64,
        b: i64,
    ) -> OgeomResult<Vec<(Curve, (f64, f64))>> {
        let kind = self.entity(surface_de)?.kind;
        let surface = self.surface(surface_de)?;
        let scale = self.report.scale_mm;
        // The file's parameters into this kernel's chart. A B-spline's are
        // its own; the analytic surfaces' are the format's: lengths along
        // a plane's axes, a cylinder's and cone's angle in degrees with a
        // length along the axis (a cone's along its slant), a sphere's
        // two angles in degrees, and a torus's pair turned: its first
        // round the tube, its second (measured back from a full turn)
        // round the axis.
        let half_angle = match &surface {
            SurfaceGeometry::Cone(c) => c.cone().half_angle(),
            _ => 0.0,
        };
        // A tabulated cylinder's parameters are both fractions: of its
        // directrix's run and of the generator. A surface of revolution's
        // are its generatrix's own parameter and the angle turned from the
        // start angle; a line's own runs over [0, 1], a spline's is its own.
        let swept = match (kind, &surface) {
            (122, SurfaceGeometry::Extrusion(e)) => {
                let (lo, hi) = ogeom_geom::Curve3d::domain(e.curve());
                let extent = ogeom_geom::Surface::domain(&surface).1;
                Some(((lo, hi - lo), (extent.0, extent.1 - extent.0), false))
            }
            (120, SurfaceGeometry::Revolution(r)) => {
                let generatrix = self.entity(surface_de)?.at(1).int();
                let (lo, hi) = ogeom_geom::Curve3d::domain(r.curve());
                match self.entity(generatrix)?.kind {
                    110 => Some(((lo, hi - lo), (0.0, 1.0), true)),
                    126 => Some(((0.0, 1.0), (0.0, 1.0), true)),
                    _ => None,
                }
            }
            _ => None,
        };
        let start_angle = if kind == 120 {
            self.entity(surface_de)?.at(2).real()
        } else {
            0.0
        };
        let map = |p: ogeom_math::Point2| -> Option<ogeom_math::Point2> {
            use ogeom_math::Point2 as P;
            // The parameter-space curve was read as a model-space one and
            // carries the unit scale on both coordinates.
            let (u, v) = (p.x / scale, p.y / scale);
            match kind {
                128 => Some(P::new(u, v)),
                190 => Some(P::new(u * scale, v * scale)),
                192 => Some(P::new(u.to_radians(), v * scale)),
                194 => Some(P::new(u.to_radians(), v * scale * half_angle.cos())),
                196 => Some(P::new(u.to_radians(), v.to_radians())),
                198 => Some(P::new((360.0 - v).to_radians(), u.to_radians())),
                122 => swept.map(|((u0, du), (v0, dv), _)| P::new(u0 + du * u, v0 + dv * v)),
                // The generatrix's parameter in `u`, the angle in `v`; the
                // surface was built with the start angle turned in.
                120 => swept.map(|((u0, du), _, _)| P::new(v - start_angle, u0 + du * u)),
                _ => None,
            }
        };
        if map(ogeom_math::Point2::new(0.0, 0.0)).is_none() {
            ogeom_bail!(
                Construction,
                "D{de}: a curve-on-surface carries no model-space curve, and \
                 its surface's parameters are the file's convention for a \
                 kind this reader does not translate; see docs/PARITY.md, io.iges"
            );
        }
        let mut out = Vec::new();
        for (curve, range) in self.curve_segments(b)? {
            // The parameter-space curve read as a model-space one carries
            // the unit scale; the surface's parameters do not. Every map
            // above is affine, so a spline's chart is the spline on its
            // mapped control points, weights kept: exact, corners and all.
            // Any other curve is fitted at its own parameters, at every
            // span and between the samples, and a miss is said out loud.
            let chart = match &curve {
                Curve::BSpline(spline) => {
                    let mut control = Vec::with_capacity(spline.control_points().len());
                    for c in spline.control_points() {
                        let p = c.point();
                        let Some(q) = map(ogeom_math::Point2::new(p.x, p.y)) else {
                            ogeom_bail!(
                                Construction,
                                "D{de}: a trim's parameters did not translate"
                            );
                        };
                        control.push(Weighted::new(q, c.weight, self.tol)?);
                    }
                    ogeom_geom::BSpline2d::rational(spline.knots().clone(), control)?
                }
                _ => {
                    const SAMPLES: usize = 200;
                    let target = 1e-10;
                    let stations = ogeom_algo::traced::stations(&curve, range, SAMPLES);
                    let fitted = ogeom_algo::traced::fit_traced_2d(
                        |t| {
                            let p = curve.point_at(t, self.tol)?;
                            map(ogeom_math::Point2::new(p.x, p.y)).ok_or_else(|| {
                                ogeom_core::ogeom_err!(
                                    Construction,
                                    "D{de}: a trim's parameters did not translate"
                                )
                            })
                        },
                        &stations,
                        3,
                        target,
                        self.tol,
                    )?;
                    if !fitted.met {
                        self.report.warnings.push(format!(
                            "D{de}: a parameter-space trim fitted to within {:.2e} of \
                             the file's, in the surface's parameters",
                            fitted.error
                        ));
                    }
                    fitted.curve
                }
            };
            // Lifted piece by piece between the chart's corners, so the
            // space curve turns each corner where the chart does, and the
            // pieces joined back into one curve.
            let corners = ogeom_algo::traced::corners_2d(&chart, range);
            let lifted = Curve::OnSurface(Box::new(ogeom_geom::CurveOnSurface::new(
                chart.into(),
                surface.clone(),
            )));
            let target = self.tol.confusion() * 100.0;
            let mut cuts = vec![range.0.min(range.1)];
            cuts.extend(corners);
            cuts.push(range.0.max(range.1));
            let mut pieces = Vec::with_capacity(cuts.len() - 1);
            let mut error = 0.0_f64;
            for pair in cuts.windows(2) {
                let piece = lifted.fitted_bspline_over((pair[0], pair[1]), target, self.tol)?;
                error = error.max(piece.error);
                pieces.push(piece.curve);
            }
            let fitted = if pieces.len() == 1 {
                ogeom_geom::fit::Fitted {
                    curve: pieces.remove(0),
                    error,
                    met: error <= target,
                }
            } else {
                let (curve, moved) = ogeom_algo::traced::joined(&pieces, self.tol)?;
                let error = error + moved;
                ogeom_geom::fit::Fitted {
                    curve,
                    error,
                    met: error <= target,
                }
            };
            if !fitted.met {
                self.report.warnings.push(format!(
                    "D{de}: a parameter-space trim lifted to within {:.2e}",
                    fitted.error
                ));
            }
            let domain = ogeom_geom::Curve3d::domain(&fitted.curve);
            out.push((Curve::from(fitted.curve), domain));
        }
        Ok(out)
    }

    fn curve_segments(&mut self, de: i64) -> OgeomResult<Vec<(Curve, (f64, f64))>> {
        self.nested(|reader| reader.curve_segments_at(de))
    }

    fn curve_segments_at(&mut self, de: i64) -> OgeomResult<Vec<(Curve, (f64, f64))>> {
        let entity = self.entity(de)?;
        if entity.kind == 102 {
            let n = entity.count(0);
            let mut out = Vec::new();
            for i in 0..n {
                out.extend(self.curve_segments(entity.at(1 + i).int())?);
            }
            return Ok(out);
        }
        Ok(vec![self.curve(de)?])
    }

    /// A trimmed (144) or bounded (143) surface as a face.
    fn face(&mut self, de: i64) -> OgeomResult<Shape> {
        let entity = self.entity(de)?;
        let (surface_de, boundaries): (i64, Vec<i64>) = match entity.kind {
            144 => {
                let s = entity.at(0).int();
                let outer_given = entity.at(1).int() == 1;
                let n_inner = entity.count(2);
                let mut bs = Vec::new();
                if outer_given && entity.at(3).int() != 0 {
                    bs.push(entity.at(3).int());
                }
                for i in 0..n_inner {
                    bs.push(entity.at(4 + i).int());
                }
                (s, bs)
            }
            143 => {
                let s = entity.at(1).int();
                let n = entity.count(2);
                (s, (0..n).map(|i| entity.at(3 + i).int()).collect())
            }
            kind => ogeom_bail!(Construction, "D{de}: type {kind} is not a trimmed surface"),
        };
        let surface = self.surface(surface_de)?;
        if boundaries.is_empty() {
            // The surface's own natural boundary: a closed or bounded
            // surface can stand alone as a face.
            return Ok(ogeom_algo::make_natural_face(&mut self.model, surface)?.shape);
        }
        let mut wires = Vec::new();
        for boundary in boundaries {
            let segments = self.boundary_segments(boundary)?;
            let segments = self.cut_at_trim_poles(segments);
            let edges = self.wire_edges(de, segments)?;
            self.cover_trim_poles(&edges)?;
            wires.push(edges);
        }
        self.assemble_face(surface, wires)
    }

    /// The chart poles of the surfaces of these trimmed (144) and bounded
    /// (143) surfaces, one point per pole; anything unreadable is passed
    /// over for its face to report.
    fn poles_of_trims(&mut self, face_des: &[i64]) -> Vec<Point> {
        let mut out: Vec<Point> = Vec::new();
        for &de in face_des {
            let surface_de = match self.entity(de) {
                Ok(e) if e.kind == 144 => e.at(0).int(),
                Ok(e) if e.kind == 143 => e.at(1).int(),
                _ => continue,
            };
            let Ok(surface) = self.surface(surface_de) else {
                continue;
            };
            for pole in crate::pcurves::chart_poles(&surface, self.tol) {
                if out.iter().all(|p| p.distance(pole) > self.tol.confusion()) {
                    out.push(pole);
                }
            }
        }
        out
    }

    /// Boundary segments cut where they run through a pole in
    /// `trim_poles`, each piece over its own window of the curve.
    ///
    /// No single image of a curve through a pole follows it through the
    /// chart: at the pole it leaves along another column than the one it
    /// arrived on. Trimmed surfaces share no edges, so the cut is made on
    /// every face's segments alike, a face whose surface has no pole there
    /// included: the neighbour across a sphere's meridian rim then has the
    /// same pieces, and the faces sew piece to piece.
    fn cut_at_trim_poles(&self, segments: Vec<(Curve, (f64, f64))>) -> Vec<(Curve, (f64, f64))> {
        if self.trim_poles.is_empty() {
            return segments;
        }
        let mut out = Vec::with_capacity(segments.len());
        for (curve, range) in segments {
            let crossings =
                crate::pcurves::pole_crossings(&curve, range, &self.trim_poles, self.tol);
            let mut from = range.0;
            for (t, ..) in crossings {
                out.push((curve.clone(), (from, t)));
                from = t;
            }
            out.push((curve, (from, range.1)));
        }
        out
    }

    /// Widen each vertex of `edges` that stands near a pole in
    /// `trim_poles`, off it by more than the confusion distance, to reach
    /// the pole: the vertex a cut put on the curve is the pole's.
    fn cover_trim_poles(&mut self, edges: &[Shape]) -> OgeomResult<()> {
        let reach = self.tol.confusion() * 100.0;
        for edge in edges {
            let Some((first, last)) = ogeom_algo::edge_vertices(&self.model, edge)? else {
                continue;
            };
            for vertex in [first, last] {
                let Some(at) = self
                    .model
                    .node(&vertex)
                    .and_then(|n| n.data().as_vertex())
                    .map(|d| d.point)
                else {
                    continue;
                };
                let off = self
                    .trim_poles
                    .iter()
                    .map(|p| p.distance(at))
                    .fold(f64::INFINITY, f64::min);
                if off > self.tol.confusion() && off <= reach {
                    self.model.widen(
                        &vertex,
                        ogeom_core::Tolerance::new(off + self.tol.confusion())?,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Boundary segments into a closed chain of edges, head to tail.
    ///
    /// Surface files are loose about sense (a boundary's segments arrive in
    /// order but each may run either way, the first included), so the chain
    /// is stitched by geometry: the walk starts from either end of the first
    /// segment, each later segment joins whichever of its ends sits at the
    /// chain's current head, and the last vertex is the first, which is what
    /// closes the wire. The start whose walk closes is taken.
    fn wire_edges(
        &mut self,
        face_de: i64,
        segments: Vec<(Curve, (f64, f64))>,
    ) -> OgeomResult<Vec<Shape>> {
        if segments.is_empty() {
            ogeom_bail!(Construction, "D{face_de}: a boundary with no curves");
        }
        let ends: Vec<(Point, Point)> = segments
            .iter()
            .map(|(c, r)| Ok((c.point_at(r.0, self.tol)?, c.point_at(r.1, self.tol)?)))
            .collect::<OgeomResult<_>>()?;
        let n = segments.len();
        let weld = self.tol.confusion() * 100.0;

        // One closed segment closes on a single vertex.
        if n == 1 {
            let (curve, range) = segments
                .into_iter()
                .next()
                .unwrap_or_else(|| unreachable!());
            let (s, e) = ends[0];
            if s.distance(e) > weld {
                ogeom_bail!(
                    Construction,
                    "D{face_de}: a one-curve boundary whose ends sit {:.2e} apart",
                    s.distance(e)
                );
            }
            let v = make_vertex(&mut self.model, s).shape;
            let edge = make_edge_between(&mut self.model, curve, range, &v, &v, self.tol)?.shape;
            return Ok(vec![edge]);
        }

        // Each segment's sense along the walk from `start` and how far the
        // walk ends from it, or the first segment that leaves a gap and how
        // wide it is.
        let walk = |start: Point| -> Result<(Vec<bool>, f64), (usize, f64)> {
            let mut at = start;
            let mut senses = Vec::with_capacity(n);
            for (i, &(s, e)) in ends.iter().enumerate() {
                let forward = at.distance(s) <= at.distance(e);
                let gap = at.distance(if forward { s } else { e });
                if gap > weld {
                    return Err((i, gap));
                }
                senses.push(forward);
                at = if forward { e } else { s };
            }
            Ok((senses, at.distance(start)))
        };
        // The walk that closes, the forward one first; failing that, a walk
        // with no gap inside it, whose last edge is welded to the head.
        let walks = [ends[0].0, ends[0].1].map(|start| (start, walk(start)));
        let chosen = walks
            .iter()
            .find(|(_, w)| matches!(w, Ok((_, gap)) if *gap <= weld))
            .or_else(|| walks.iter().find(|(_, w)| w.is_ok()));
        let (start, senses) = match chosen {
            Some((start, Ok((senses, _)))) => (*start, senses.clone()),
            _ => {
                // Report the walk that got further.
                let reached = |w: &Result<(Vec<bool>, f64), (usize, f64)>| match w {
                    Err(failure) => *failure,
                    Ok(_) => (n, 0.0),
                };
                let (a, b) = (reached(&walks[0].1), reached(&walks[1].1));
                let (i, gap) = if b.0 > a.0 { b } else { a };
                ogeom_bail!(
                    Construction,
                    "D{face_de}: boundary segment {i} starts {gap:.2e} from \
                     where the previous one ended"
                );
            }
        };

        let head = make_vertex(&mut self.model, start).shape;
        let mut at_vertex = head.clone();
        let mut edges = Vec::with_capacity(n);
        for (i, (((curve, range), (s, e)), forward)) in
            segments.into_iter().zip(ends).zip(senses).enumerate()
        {
            let this_end = if forward { e } else { s };
            let last = i + 1 == n;
            let next_vertex = if last {
                head.clone()
            } else {
                make_vertex(&mut self.model, this_end).shape
            };
            // The edge is built along its curve's own direction; a segment
            // running against the chain is used reversed, exactly as a
            // hand-built prism's rim edges are.
            let edge = if forward {
                make_edge_between(
                    &mut self.model,
                    curve,
                    range,
                    &at_vertex,
                    &next_vertex,
                    self.tol,
                )?
                .shape
            } else {
                make_edge_between(
                    &mut self.model,
                    curve,
                    range,
                    &next_vertex,
                    &at_vertex,
                    self.tol,
                )?
                .shape
                .reversed()
            };
            edges.push(edge);
            at_vertex = next_vertex;
        }
        Ok(edges)
    }

    /// A face from a surface and wires of already-built edges: the exact
    /// pcurve where the pair has a closed form, the fitted one where it does
    /// not, and both sides of the chart for an edge the wire uses twice,
    /// which is what a seam is.
    fn assemble_face(
        &mut self,
        surface: SurfaceGeometry,
        wires: Vec<Vec<Shape>>,
    ) -> OgeomResult<Shape> {
        let surface_id = self.model.geometry_mut().add_surface(surface.clone());
        let mut wire_shapes = Vec::with_capacity(wires.len());
        for edges in &wires {
            self.chart_wire(edges, &surface, surface_id)?;
            wire_shapes.push(ogeom_algo::make_wire(&mut self.model, edges, self.tol)?.shape);
        }
        // Every image on one branch, then a boundary meeting a pole from two
        // sides of the chart closed across the pole row.
        ogeom_algo::chain_wire_branches(&mut self.model, surface_id, &wire_shapes, self.tol)?;
        for wire in &mut wire_shapes {
            let edges = self.model.ordered_children_of(wire)?;
            let closed =
                ogeom_algo::closed_at_poles(&mut self.model, surface_id, &edges, self.tol)?;
            if closed.len() != edges.len() {
                *wire = ogeom_algo::make_wire(&mut self.model, &closed, self.tol)?.shape;
            }
        }
        Ok(ogeom_algo::make_face_on(&mut self.model, surface_id, &wire_shapes, self.tol)?.shape)
    }

    /// One wire's pcurves, chained around the face's chart.
    ///
    /// Each edge's image is computed on its own (the exact projection where
    /// the pair has a closed form, the fitted one where it does not), and a
    /// periodic chart then has a branch to choose. Read one edge at a time
    /// the choice is arbitrary, and the wire comes apart: a bore's wall
    /// arrives with one rim written over `[−π, π]` and the other over
    /// `[−π/2, 3π/2]`, both the right circle and neither meeting the seam
    /// the wire closes on. So each image after the first is shifted by whole
    /// periods until its start meets where the last one ended, which is the
    /// rule `make_face_with_pcurves` follows for shapes this kernel builds
    /// itself.
    ///
    /// A seam falls out of the same walk. The wire uses it twice, up one
    /// column of the chart and down the other, and those columns *are* one
    /// image a period apart, so the chaining produces both, and the use
    /// that runs forward is the forward side.
    fn chart_wire(
        &mut self,
        edges: &[Shape],
        surface: &SurfaceGeometry,
        surface_id: ogeom_topo::SurfaceId,
    ) -> OgeomResult<()> {
        use ogeom_geom::Curve2d as _;
        let mut counts: FastMap<ogeom_topo::TShapeId, usize> = FastMap::default();
        for edge in edges {
            *counts.entry(edge.node()).or_default() += 1;
        }
        // One image per edge, however many times the wire walks it.
        let mut images: FastMap<ogeom_topo::TShapeId, (ogeom_geom::PlanarCurve, (f64, f64))> =
            FastMap::default();
        // The sides each walk of it left, by the direction that walk ran.
        let mut sides: FastMap<
            ogeom_topo::TShapeId,
            (
                Option<ogeom_geom::PlanarCurve>,
                Option<ogeom_geom::PlanarCurve>,
            ),
        > = FastMap::default();
        let mut order: Vec<ogeom_topo::TShapeId> = Vec::new();
        let mut previous: Option<ogeom_math::Point2> = None;
        for edge in edges {
            let (curve, range) = {
                let Some(data) = self.model.node(edge).and_then(|n| n.data().as_edge()) else {
                    continue;
                };
                let Some(ogeom_topo::EdgeRepr::Curve3d { curve, range, .. }) = data.curve3d()
                else {
                    continue;
                };
                let Some(geometry) = self.model.geometry().curve(*curve) else {
                    continue;
                };
                (geometry.clone(), *range)
            };
            if let ogeom_core::collections::hash_map::Entry::Vacant(slot) =
                images.entry(edge.node())
            {
                let Some(image) = self.image_of(edge, &curve, range, surface)? else {
                    continue;
                };
                slot.insert((image, range));
                order.push(edge.node());
            }
            let Some((image, range)) = images.get(&edge.node()).cloned() else {
                continue;
            };
            let backwards = edge.orientation() == ogeom_topo::Orientation::Reversed;
            let (start, end) = if backwards {
                (range.1, range.0)
            } else {
                (range.0, range.1)
            };
            let image =
                crate::pcurves::shifted_to_meet(&image, start, previous, surface, self.tol)?;
            previous = Some(image.point_at(end, self.tol)?);
            let walked = sides.entry(edge.node()).or_default();
            if backwards {
                walked.1 = Some(image);
            } else {
                walked.0 = Some(image);
            }
        }
        for node in order {
            let Some((image, range)) = images.get(&node).cloned() else {
                continue;
            };
            let Some(edge) = edges.iter().find(|e| e.node() == node) else {
                continue;
            };
            let walked = sides.remove(&node).unwrap_or((None, None));
            let seam = counts.get(&node).copied().unwrap_or(0) > 1;
            let columns = match &walked {
                (Some(forward), Some(reversed)) => {
                    let at = forward.point_at(range.0, self.tol)?;
                    let other = reversed.point_at(range.0, self.tol)?;
                    (at.distance(other) > self.tol.confusion()).then_some((forward, reversed))
                }
                _ => None,
            };
            match (seam, columns) {
                (true, Some((forward, reversed))) => {
                    ogeom_algo::attach_seam(
                        &mut self.model,
                        edge,
                        forward.clone(),
                        reversed.clone(),
                        surface_id,
                        ogeom_topo::Location::identity(),
                        range,
                    )?;
                }
                // The walk left the seam's two uses in one place: the chart
                // closes without being periodic (a skinned wall's is such a
                // chart, clamped and closed), and there is no period to
                // shift by. The other column goes a chart's width over,
                // toward the middle.
                (true, None) => {
                    let other = crate::pcurves::seam_other_side(&image, range, surface, self.tol)?;
                    ogeom_algo::attach_seam(
                        &mut self.model,
                        edge,
                        image,
                        other,
                        surface_id,
                        ogeom_topo::Location::identity(),
                        range,
                    )?;
                }
                (false, _) => {
                    let (forward, reversed) = walked;
                    ogeom_algo::attach_pcurve(
                        &mut self.model,
                        edge,
                        forward.or(reversed).unwrap_or(image),
                        surface_id,
                        ogeom_topo::Location::identity(),
                        range,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// One edge's image on one surface, exact where the pair has a closed
    /// form and fitted where it does not: the same policy the STEP reader
    /// applies, through the shared machinery. Where the wire puts it on a
    /// periodic chart is [`Self::chart_wire`]'s business.
    fn image_of(
        &mut self,
        edge: &Shape,
        curve: &Curve,
        range: (f64, f64),
        surface: &SurfaceGeometry,
    ) -> OgeomResult<Option<ogeom_geom::PlanarCurve>> {
        let prepared = self.face_de.and_then(|face| {
            let key = (face, range.0.to_bits(), range.1.to_bits());
            let slot = self.images.get_mut(&key)?;
            let at = slot.iter().position(|(c, _)| c == curve)?;
            Some(slot.swap_remove(at).1)
        });
        let (found, effects) =
            prepared.unwrap_or_else(|| derive_image(curve, range, surface, self.tol));
        for effect in effects {
            match effect {
                Effect::Warn(warning) => self.report.warnings.push(warning),
                Effect::Gap(off) => self.state_gap(edge, off)?,
            }
        }
        Ok(found)
    }

    /// Derive, in parallel, the image of every edge of these faces (510)
    /// on its face's surface, for [`Self::image_of`] to take as the walk
    /// reaches it.
    ///
    /// Deriving one is a pure function of a curve, its range and a
    /// surface, and it is most of the time spent building faces. What is
    /// read to find the jobs leaves no trace: an edge not yet built is
    /// resolved from the file without building it, and the warnings and
    /// visits that reading makes are dropped, to be made again by the walk
    /// in its own order. Best-effort: what this cannot resolve, or
    /// resolves to another curve than the walk finds, the walk derives.
    fn prepare_images(&mut self, face_des: &[i64]) {
        let warned = self.report.warnings.len();
        let visited = self.visited.clone();
        let mut surfaces: Vec<SurfaceGeometry> = Vec::new();
        let mut jobs: Vec<(i64, usize, Curve, (f64, f64))> = Vec::new();
        let mut seen: ogeom_core::FastSet<(i64, i64, i64)> = ogeom_core::FastSet::default();
        for &face_de in face_des {
            let Ok(entity) = self.entity(face_de) else {
                continue;
            };
            if entity.kind != 510 {
                continue;
            }
            let Ok(surface) = self.surface(entity.at(0).int()) else {
                continue;
            };
            surfaces.push(surface);
            let at = surfaces.len() - 1;
            for i in 0..entity.count(1) {
                let Ok(entries) = self.loop_entries(entity.at(3 + i).int()) else {
                    continue;
                };
                for (list_de, index, _) in entries.into_iter().flatten() {
                    if !seen.insert((face_de, list_de, index)) {
                        continue;
                    }
                    for (curve, range) in self.edge_geometry(list_de, index) {
                        jobs.push((face_de, at, curve, range));
                    }
                }
            }
        }
        self.report.warnings.truncate(warned);
        self.visited = visited;
        // Below a handful of edges the threads cost more than the work.
        if jobs.len() < 16 {
            return;
        }
        let tol = self.tol;
        let derived = ogeom_core::parallel::map_ordered(&jobs, |_, (_, at, curve, range)| {
            derive_image(curve, *range, &surfaces[*at], tol)
        });
        for ((face, _, curve, range), image) in jobs.into_iter().zip(derived) {
            self.images
                .entry((face, range.0.to_bits(), range.1.to_bits()))
                .or_default()
                .push((curve, image));
        }
    }

    /// The curves and ranges edge `index` of an edge list (504) is built
    /// on: its pieces' where it was cut at poles, its own where it is
    /// built, and otherwise as [`Self::list_edge`] would build it. Empty
    /// where the file cannot say.
    fn edge_geometry(&mut self, list_de: i64, index: i64) -> Vec<(Curve, (f64, f64))> {
        let key = (list_de, index);
        let built: Vec<Shape> = match (self.pieces.get(&key), self.edges.get(&key)) {
            (Some(pieces), _) => pieces.clone(),
            (None, Some((edge, ..))) => vec![edge.clone()],
            (None, None) => {
                return self
                    .edge_on_file(list_de, index)
                    .map(|(curve, range, ..)| vec![(curve, range)])
                    .unwrap_or_default();
            }
        };
        built
            .iter()
            .filter_map(|edge| {
                let data = self.model.node(edge)?.data().as_edge()?;
                let Some(ogeom_topo::EdgeRepr::Curve3d { curve, range, .. }) = data.curve3d()
                else {
                    return None;
                };
                Some((self.model.geometry().curve(*curve)?.clone(), *range))
            })
            .collect()
    }

    /// Widen `edge` and its vertices to cover a gap it was measured to
    /// stand from a face it bounds. Past the millimetre a file's slop stays
    /// under, the pcurve is a fit gone astray (a curve through a pole left
    /// whole, whose image leaps across the chart there), and an edge that
    /// wide would swallow its neighbours; it is reported instead.
    fn state_gap(&mut self, edge: &Shape, off: f64) -> OgeomResult<()> {
        if off > self.tol.confusion() * 1e7 {
            self.report.warnings.push(format!(
                "a projected pcurve fit stopped at {off:.2e}; \
                 the face's mesh may sit that far off along this edge"
            ));
        } else if off > self.tol.confusion() {
            self.model.widen(
                edge,
                ogeom_core::Tolerance::new(off + self.tol.confusion())?,
            )?;
        }
        Ok(())
    }

    /// A manifold solid B-rep object: shell of faces of loops of edges.
    fn manifold_solid(&mut self, de: i64) -> OgeomResult<Shape> {
        let entity = self.entity(de)?;
        // Each shell comes with a flag saying whether it faces as its faces
        // do; a void's shell faces out of the cavity and is used the other
        // way round, so every face of the solid faces away from the material.
        // An absent flag agrees.
        let agrees = |flag: &crate::iges::parse::Value| {
            matches!(flag, crate::iges::parse::Value::Default) || flag.int() != 0
        };
        let used = |shell: Shape, agrees: bool| if agrees { shell } else { shell.reversed() };
        let shell = used(self.shell(entity.at(0).int())?, agrees(entity.at(1)));
        let n_voids = entity.count(2);
        let mut shells = vec![shell];
        for i in 0..n_voids {
            let void = self.shell(entity.at(3 + 2 * i).int())?;
            shells.push(used(void, agrees(entity.at(4 + 2 * i))));
        }
        let solid = make_solid(&mut self.model, &shells)?.shape;
        // A fitted trim widens its edge to the offset it measured; the
        // edge's vertices come along, and the solid keeps the containment
        // rule the checker holds it to.
        ogeom_algo::restore_containment(&mut self.model, &solid)?;
        Ok(solid)
    }

    fn shell(&mut self, de: i64) -> OgeomResult<Shape> {
        self.nested(|reader| reader.shell_at(de))
    }

    fn shell_at(&mut self, de: i64) -> OgeomResult<Shape> {
        let entity = self.entity(de)?;
        if entity.kind != 514 {
            ogeom_bail!(
                Construction,
                "D{de}: expected a shell, found type {}",
                entity.kind
            );
        }
        let n = entity.count(0);
        let face_des: Vec<i64> = (0..n).map(|i| entity.at(1 + 2 * i).int()).collect();
        self.cut_at_poles(&face_des);
        self.prepare_images(&face_des);
        let mut faces = Vec::with_capacity(n);
        for i in 0..n {
            let face_de = entity.at(1 + 2 * i).int();
            let same_sense = entity.at(2 + 2 * i).int() != 0;
            let face = self.brep_face(face_de)?;
            faces.push(if same_sense { face } else { face.reversed() });
        }
        self.images.clear();
        Ok(ogeom_algo::make_shell(&mut self.model, &faces)?.shape)
    }

    fn brep_face(&mut self, de: i64) -> OgeomResult<Shape> {
        let entity = self.entity(de)?;
        if entity.kind != 510 {
            ogeom_bail!(
                Construction,
                "D{de}: expected a face, found type {}",
                entity.kind
            );
        }
        let surface = self.surface(entity.at(0).int())?;
        let n_loops = entity.count(1);
        // Parameter 2 is the outer-loop flag; the loop pointers follow.
        let mut wires = Vec::with_capacity(n_loops);
        for i in 0..n_loops {
            wires.push(self.loop_edges(entity.at(3 + i).int())?);
        }
        let outer = self.face_de.replace(de);
        let face = self.assemble_face(surface, wires);
        self.face_de = outer;
        face
    }

    /// A loop's (508) entries in order: for an edge, its list, its index
    /// and whether the loop walks it along its curve; `None` for a vertex.
    fn loop_entries(&mut self, de: i64) -> OgeomResult<Vec<Option<(i64, i64, bool)>>> {
        let entity = self.entity(de)?;
        if entity.kind != 508 {
            ogeom_bail!(
                Construction,
                "D{de}: expected a loop, found type {}",
                entity.kind
            );
        }
        let n = entity.count(0);
        let mut entries = Vec::with_capacity(n);
        let mut i = 1;
        for _ in 0..n {
            let is_vertex = entity.at(i).int() == 1;
            let list_de = entity.at(i + 1).int();
            let index = entity.at(i + 2).int();
            let orientation = entity.at(i + 3).int();
            let k = entity.count(i + 4);
            i += 5 + 2 * k;
            entries.push((!is_vertex).then_some((list_de, index, orientation != 0)));
        }
        Ok(entries)
    }

    fn loop_edges(&mut self, de: i64) -> OgeomResult<Vec<Shape>> {
        let mut edges = Vec::new();
        for entry in self.loop_entries(de)? {
            let Some((list_de, index, along)) = entry else {
                // A vertex entry marks a degenerate use; the face builder
                // rebuilds chart degeneracies from the surface itself.
                self.report.warnings.push(format!(
                    "D{de}: a loop lists a vertex entry, which this reader skips"
                ));
                continue;
            };
            let pieces = match self.pieces.get(&(list_de, index)) {
                Some(pieces) => pieces.clone(),
                None => vec![self.list_edge(list_de, index)?.0],
            };
            if along {
                edges.extend(pieces);
            } else {
                edges.extend(pieces.into_iter().rev().map(|p| p.reversed()));
            }
        }
        Ok(edges)
    }

    /// Cut every edge of these faces (510) whose curve runs through a pole
    /// of a face it bounds (a sphere's pole, a cone's apex, a patch's
    /// collapsed side) at that pole, before any face is built: no single
    /// image of such a curve follows it through the chart, and each piece
    /// has one. The cut is made on the shared edge, so every face it bounds
    /// walks the same pieces. Best-effort: anything unreadable here is left
    /// whole for the face to report.
    fn cut_at_poles(&mut self, face_des: &[i64]) {
        let mut crossings: BTreeMap<(i64, i64), Vec<crate::pcurves::Crossing>> = BTreeMap::new();
        for &face_de in face_des {
            let Ok(entity) = self.entity(face_de) else {
                continue;
            };
            if entity.kind != 510 {
                continue;
            }
            let loops: Vec<i64> = (0..entity.count(1))
                .map(|i| entity.at(3 + i).int())
                .collect();
            let Ok(surface) = self.surface(entity.at(0).int()) else {
                continue;
            };
            let poles = crate::pcurves::chart_poles(&surface, self.tol);
            if poles.is_empty() {
                continue;
            }
            for loop_de in loops {
                let Ok(entries) = self.loop_entries(loop_de) else {
                    continue;
                };
                for (list_de, index, _) in entries.into_iter().flatten() {
                    let key = (list_de, index);
                    let Ok((_, curve, range)) = self.list_edge(list_de, index) else {
                        continue;
                    };
                    let found = crate::pcurves::pole_crossings(&curve, range, &poles, self.tol);
                    let entry = crossings.entry(key).or_default();
                    for crossing in found {
                        if entry.iter().all(|(t, ..)| (t - crossing.0).abs() > 1e-9) {
                            entry.push(crossing);
                        }
                    }
                }
            }
        }
        for (key, mut at) in crossings {
            if at.is_empty() || self.pieces.contains_key(&key) {
                continue;
            }
            at.sort_by(|x, y| x.0.total_cmp(&y.0));
            let Some((edge, curve, range)) = self.edges.get(&key).cloned() else {
                continue;
            };
            if let Ok(cut) = crate::pcurves::split_at_poles(
                &mut self.model,
                &edge,
                &curve,
                range,
                &at,
                &mut self.pole_vertices,
                self.tol,
            ) {
                self.pieces
                    .insert(key, cut.into_iter().map(|(piece, _)| piece).collect());
            }
        }
    }

    /// Edge `index` (1-based) of an edge list (504), built once and shared.
    fn list_edge(&mut self, list_de: i64, index: i64) -> OgeomResult<BuiltEdge> {
        let key = (list_de, index);
        if let Some(found) = self.edges.get(&key) {
            return Ok(found.clone());
        }
        let (curve, range, start, end) = self.edge_on_file(list_de, index)?;
        let vs = self.list_vertex(start.0, start.1)?;
        let ve = self.list_vertex(end.0, end.1)?;
        // IGES states no tolerances, so a vertex is built at the confusion
        // tolerance and a curve end that misses it by rounding (a writer's
        // last digit, a few tenths of a nanometre) would refuse the whole
        // solid. As the STEP reader does, the vertex's tolerance grows to
        // state the miss, up to the millimetre past which a boundary is not
        // this curve's at all; beyond that the edge still refuses by name.
        let cap = self.tol.confusion() * 1e7;
        for (vertex, t) in [(&vs, range.0), (&ve, range.1)] {
            let (Ok(end), Some(stated)) = (
                curve.point_at(t, self.tol),
                self.model
                    .node(vertex)
                    .and_then(|n| n.data().as_vertex())
                    .map(|d| (d.point, d.tolerance)),
            ) else {
                continue;
            };
            let gap = end.distance(stated.0);
            if gap > stated.1.get() && gap <= cap {
                self.model.widen(
                    vertex,
                    ogeom_core::Tolerance::new(gap + self.tol.confusion())?,
                )?;
                self.vertex_misses.0 += 1;
                self.vertex_misses.1 = self.vertex_misses.1.max(gap);
            }
        }
        let edge =
            make_edge_between(&mut self.model, curve.clone(), range, &vs, &ve, self.tol)?.shape;
        let built = (edge, curve, range);
        self.edges.insert(key, built.clone());
        Ok(built)
    }

    /// Edge `index` (1-based) of an edge list (504) as the file gives it,
    /// built into nothing: its curve, its range and the vertex list
    /// entries it starts and ends at.
    #[allow(clippy::type_complexity, reason = "one edge entry's parts")]
    fn edge_on_file(
        &mut self,
        list_de: i64,
        index: i64,
    ) -> OgeomResult<(Curve, (f64, f64), (i64, i64), (i64, i64))> {
        let entity = self.entity(list_de)?;
        if entity.kind != 504 {
            ogeom_bail!(
                Construction,
                "D{list_de}: expected an edge list, found type {}",
                entity.kind
            );
        }
        let i = index
            .checked_sub(1)
            .and_then(|i| usize::try_from(i).ok())
            .ok_or_else(|| {
                ogeom_core::ogeom_err!(Construction, "D{list_de}: edge index {index} out of range")
            })?;
        let base = 1 + 5 * i;
        let curve_de = entity.at(base).int();
        let start = (entity.at(base + 1).int(), entity.at(base + 2).int());
        let end = (entity.at(base + 3).int(), entity.at(base + 4).int());
        let (curve, mut range) = self.curve(curve_de)?;
        let ps = self.vertex_point(start.0, start.1)?;
        let pe = self.vertex_point(end.0, end.1)?;
        // Re-derive the range from the vertices on this kernel's own
        // parameterization, exactly as the STEP reader does and for the same
        // reason: the file's parameterization is its own business.
        if let (Some(a), Some(b)) = (parameter_on(&curve, ps), parameter_on(&curve, pe)) {
            let period = if curve.is_periodic() {
                let (lo, hi) = curve.domain();
                hi - lo
            } else {
                0.0
            };
            range = if ps.distance(pe) < self.tol.confusion() && period > 0.0 {
                (a, a + period)
            } else if period > 0.0 && b <= a + self.tol.parametric() {
                (a, b + period)
            } else {
                (a, b)
            };
            // A vertex a few nanometres past a bounded curve's end projects
            // past it: the range is held to the curve's own domain, and the
            // vertex widens below to cover the rest of the miss.
            if period == 0.0 {
                let (lo, hi) = curve.domain();
                range = (range.0.clamp(lo, hi), range.1.clamp(lo, hi));
            }
        }
        Ok((curve, range, start, end))
    }

    /// Where vertex `index` (1-based) of a vertex list (502) stands.
    fn vertex_point(&mut self, list_de: i64, index: i64) -> OgeomResult<Point> {
        if let Some(found) = self.vertices.get(&(list_de, index)) {
            return Ok(self.point_of(found));
        }
        let entity = self.entity(list_de)?;
        if entity.kind != 502 {
            ogeom_bail!(
                Construction,
                "D{list_de}: expected a vertex list, found type {}",
                entity.kind
            );
        }
        let i = index
            .checked_sub(1)
            .and_then(|i| usize::try_from(i).ok())
            .ok_or_else(|| {
                ogeom_core::ogeom_err!(
                    Construction,
                    "D{list_de}: vertex index {index} out of range"
                )
            })?;
        Ok(self.point3(entity, 1 + 3 * i))
    }

    /// Vertex `index` (1-based) of a vertex list (502), built once and
    /// shared; sharing is what lets a closed shell close.
    fn list_vertex(&mut self, list_de: i64, index: i64) -> OgeomResult<Shape> {
        let key = (list_de, index);
        if let Some(found) = self.vertices.get(&key) {
            return Ok(found.clone());
        }
        let point = self.vertex_point(list_de, index)?;
        let vertex = make_vertex(&mut self.model, point).shape;
        self.vertices.insert(key, vertex.clone());
        Ok(vertex)
    }

    fn point_of(&self, vertex: &Shape) -> Point {
        self.model
            .node(vertex)
            .and_then(|n| n.data().as_vertex())
            .map_or(Point::ORIGIN, |d| d.point)
    }

    /// The document: parts named from entity labels, colours from the fixed
    /// palette and from 314 entities the directory colour fields point at.
    /// A constructive solid entity's solids: one for a primitive, a boolean
    /// tree or an instance, one per item for an assembly.
    fn csg_solids(&mut self, de: i64) -> OgeomResult<Vec<Shape>> {
        let entity = self.entity(de)?;
        if entity.kind != 184 {
            return Ok(vec![self.csg(de)?]);
        }
        // Solid assembly: items, then a placement matrix for each.
        let n = entity.count(0);
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let item = entity.at(1 + i).int();
            let matrix = entity.at(1 + n + i).int();
            let mut solid = self.csg(item)?;
            if matrix != 0 {
                let placement = {
                    let holder = Entity {
                        kind: 0,
                        form: 0,
                        transform: matrix,
                        colour: 0,
                        level: 0,
                        status: 0,
                        label: String::new(),
                        params: Vec::new(),
                    };
                    self.placement(&holder)?
                };
                let location = ogeom_topo::Location::of(self.model.add_datum(placement));
                solid = solid.moved(&location);
            }
            out.push(solid);
        }
        Ok(out)
    }

    /// One constructive solid: a primitive (150 to 168), a boolean tree (180),
    /// a solid instance (430) or a manifold solid (186), under the entity's
    /// own placement.
    #[allow(clippy::too_many_lines, reason = "one case per primitive")]
    fn csg(&mut self, de: i64) -> OgeomResult<Shape> {
        let entity = self.entity(de)?;
        let s = self.report.scale_mm;
        let tol = self.tol;
        let r = |i: usize| entity.at(i).real();
        let length = |i: usize| entity.at(i).real() * s;
        let point = |i: usize| Point::new(r(i) * s, r(i + 1) * s, r(i + 2) * s);
        // An axis the file leaves at its default is the default axis.
        let direction = |i: usize, default: Vector| -> OgeomResult<Direction> {
            let v = Vector::new(r(i), r(i + 1), r(i + 2));
            Direction::new(if v.magnitude() > 0.0 { v } else { default }, tol)
        };
        let shape = match entity.kind {
            150 => {
                let frame = Frame::new(
                    point(3),
                    direction(9, Vector::Z)?,
                    direction(6, Vector::X)?,
                    tol,
                )?;
                ogeom_algo::make_box(
                    &mut self.model,
                    frame,
                    (length(0), length(1), length(2)),
                    tol,
                )?
                .shape
            }
            152 => {
                // The file's wedge narrows along its local y, to `LTX` along
                // x at y = LY; this vocabulary's narrows along its z. Its
                // frame is the file's turned: z along the file's y, x along
                // the file's z, so its y runs along the file's x.
                let x_file = direction(7, Vector::X)?;
                let z_file = direction(10, Vector::Z)?;
                let y_file = Direction::new(z_file.vector().cross(x_file.vector()), tol)?;
                let frame = Frame::new(point(4), y_file, z_file, tol)?;
                ogeom_algo::make_wedge(
                    &mut self.model,
                    frame,
                    (length(2), length(0), length(1)),
                    (length(2), length(3)),
                    tol,
                )?
                .shape
            }
            154 => {
                let frame = frame_about(point(2), direction(5, Vector::Z)?, tol)?;
                ogeom_algo::make_cylinder(&mut self.model, frame, length(1), length(0), tol)?.shape
            }
            156 => {
                let frame = frame_about(point(3), direction(6, Vector::Z)?, tol)?;
                ogeom_algo::make_cone(&mut self.model, frame, length(1), length(2), length(0), tol)?
                    .shape
            }
            158 => {
                let frame = frame_about(point(1), Direction::Z, tol)?;
                ogeom_algo::make_sphere(&mut self.model, frame, length(0), tol)?.shape
            }
            160 => {
                let frame = frame_about(point(2), direction(5, Vector::Z)?, tol)?;
                ogeom_algo::make_torus(&mut self.model, frame, length(0), length(1), tol)?.shape
            }
            162 => {
                // A planar profile turned about an axis by a fraction of a
                // full turn; an open profile (form 1) closed to the axis.
                let axis = ogeom_math::Axis::new(point(2), direction(5, Vector::Z)?);
                let fraction = match r(1) {
                    f if f > 0.0 => f.min(1.0),
                    _ => 1.0,
                };
                let face = self.profile_face(de, entity.at(0).int(), Some(axis))?;
                ogeom_algo::make_revolution(
                    &mut self.model,
                    &face,
                    axis,
                    core::f64::consts::TAU * fraction,
                    tol,
                )?
                .shape
            }
            164 => {
                let face = self.profile_face(de, entity.at(0).int(), None)?;
                let along = direction(2, Vector::Z)?.vector() * length(1);
                ogeom_algo::make_prism(&mut self.model, &face, along, tol)?.shape
            }
            168 => {
                // A ball of unit radius, stretched along the local axes.
                let x = direction(6, Vector::X)?.vector();
                let z = direction(9, Vector::Z)?.vector();
                let y = z.cross(x);
                let (a, b, c) = (length(0), length(1), length(2));
                let linear = Matrix3::new([
                    [x.x * a, y.x * b, z.x * c],
                    [x.y * a, y.y * b, z.y * c],
                    [x.z * a, y.z * b, z.z * c],
                ]);
                let ball = ogeom_algo::make_sphere(&mut self.model, Frame::WORLD, 1.0, tol)?.shape;
                let stretch = ogeom_math::GeneralTransform::new(linear, point(3).to_vector());
                ogeom_algo::general_transformed_shape(&mut self.model, &ball, &stretch, tol)?.shape
            }
            180 => {
                // Post-order: operands pushed, each operation code (1 union,
                // 2 intersection, 3 difference) combining the last two.
                let n = entity.count(0);
                let mut stack: Vec<Shape> = Vec::new();
                for i in 0..n {
                    let item = entity.at(1 + i).int();
                    if item < 0 {
                        stack.push(self.csg(-item)?);
                        continue;
                    }
                    let (Some(b), Some(a)) = (stack.pop(), stack.pop()) else {
                        ogeom_bail!(
                            Construction,
                            "D{de}: a boolean tree's operation has too few operands"
                        );
                    };
                    let result = match item {
                        1 => ogeom_bool::fuse(&mut self.model, &a, &b, tol)?,
                        2 => ogeom_bool::common(&mut self.model, &a, &b, tol)?,
                        3 => ogeom_bool::cut(&mut self.model, &a, &b, tol)?,
                        code => ogeom_bail!(
                            Construction,
                            "D{de}: boolean operation code {code} names no operation"
                        ),
                    };
                    stack.push(result.shape);
                }
                let [solid] = stack.as_slice() else {
                    ogeom_bail!(
                        Construction,
                        "D{de}: a boolean tree leaves {} results",
                        stack.len()
                    );
                };
                solid.clone()
            }
            430 => self.csg(entity.at(0).int())?,
            186 => self.manifold_solid(de)?,
            kind => ogeom_bail!(
                Construction,
                "D{de}: type {kind} is not a constructive solid"
            ),
        };
        let placement = self.placement(entity)?;
        if placement == Transform::IDENTITY {
            return Ok(shape);
        }
        let location = ogeom_topo::Location::of(self.model.add_datum(placement));
        Ok(shape.moved(&location))
    }

    /// A planar face bounded by a closed curve entity, or, where the curve
    /// is open and an axis is given, by the curve closed to the axis.
    fn profile_face(
        &mut self,
        de: i64,
        curve: i64,
        axis: Option<ogeom_math::Axis>,
    ) -> OgeomResult<Shape> {
        let mut segments = self.curve_segments(curve)?;
        let start = segments[0].0.point_at(segments[0].1.0, self.tol)?;
        let last = &segments[segments.len() - 1];
        let end = last.0.point_at(last.1.1, self.tol)?;
        if start.distance(end) > self.tol.confusion() * 100.0 {
            let Some(axis) = axis else {
                ogeom_bail!(Construction, "D{de}: an extruded profile does not close");
            };
            let foot = |p: Point| {
                let o = axis.location;
                let d = axis.direction.vector();
                o + d * (p - o).dot(d)
            };
            let (end_foot, start_foot) = (foot(end), foot(start));
            for (a, b) in [(end, end_foot), (end_foot, start_foot), (start_foot, start)] {
                if a.distance(b) > self.tol.confusion() * 100.0 {
                    let line: Curve = LineCurve::segment(a, b, self.tol)?.into();
                    let domain = line.domain();
                    segments.push((line, domain));
                }
            }
        }
        let edges = self.wire_edges(de, segments)?;
        let wire = ogeom_algo::make_wire(&mut self.model, &edges, self.tol)?.shape;
        let Some(plane) = ogeom_algo::find_plane(&self.model, &wire, self.tol)? else {
            ogeom_bail!(Construction, "D{de}: a solid's profile is not planar");
        };
        let surface: SurfaceGeometry = PlaneSurface::over(
            plane,
            (-SURFACE_EXTENT, SURFACE_EXTENT),
            (-SURFACE_EXTENT, SURFACE_EXTENT),
        )?
        .into();
        Ok(ogeom_algo::make_face_with_pcurves(&mut self.model, surface, &[edges], self.tol)?.shape)
    }

    /// Every independent singular subfigure instance (408) placed: its
    /// definition's (308) solids moved into place, and its trimmed surfaces
    /// moved and sewn, closed shells becoming solids as a surface file's
    /// do. A definition is built once, however many instances place it:
    /// the instances share its topology under their own placements.
    #[allow(
        clippy::type_complexity,
        reason = "the solids, the sheets, and what built what"
    )]
    fn subfigure_instances(
        &mut self,
        tol: Tolerances,
    ) -> OgeomResult<(Vec<Shape>, Vec<Shape>, Vec<(i64, Shape)>)> {
        let instances: Vec<i64> = self
            .file
            .entities
            .iter()
            .filter(|(_, e)| e.kind == 408 && (e.status / 10_000) % 100 == 0)
            .map(|(de, _)| *de)
            .collect();
        let mut built: FastMap<i64, (Vec<Shape>, Vec<Shape>)> = FastMap::default();
        let (mut solids, mut sheets, mut from) = (Vec::new(), Vec::new(), Vec::new());
        for de in instances {
            let (placed_solids, faces) = match self.instance(de, &mut built, 0) {
                Ok(found) => found,
                Err(e) => {
                    self.report
                        .warnings
                        .push(format!("D{de}: subfigure instance not placed: {e}"));
                    continue;
                }
            };
            from.extend(placed_solids.iter().map(|s| (de, s.clone())));
            from.extend(faces.iter().map(|f| (de, f.clone())));
            solids.extend(placed_solids);
            if faces.is_empty() {
                continue;
            }
            let sewn = sew(&mut self.model, &faces, tol)?;
            for shell in &sewn.shells {
                ogeom_algo::join_seam_columns(&mut self.model, shell, tol)?;
                if ogeom_algo::is_shell_closed(&self.model, shell)? {
                    solids.push(make_solid(&mut self.model, std::slice::from_ref(shell))?.shape);
                } else {
                    sheets.push(shell.clone());
                }
            }
        }
        Ok((solids, sheets, from))
    }

    /// One instance's solids and faces, placed: its own placement, then
    /// the translation and uniform scale it states.
    fn instance(
        &mut self,
        de: i64,
        built: &mut FastMap<i64, (Vec<Shape>, Vec<Shape>)>,
        depth: usize,
    ) -> OgeomResult<(Vec<Shape>, Vec<Shape>)> {
        if depth > 32 {
            ogeom_bail!(
                Construction,
                "D{de}: subfigures nest past any sensible depth"
            );
        }
        let entity = self.entity(de)?;
        let definition = entity.at(0).int();
        let s = self.report.scale_mm;
        let offset = Vector::new(
            entity.at(1).real() * s,
            entity.at(2).real() * s,
            entity.at(3).real() * s,
        );
        let factor = match entity.at(4).real() {
            f if f > 0.0 => f,
            _ => 1.0,
        };
        let (solids, faces) = self.subfigure(definition, built, depth)?;
        let motion = self.placement(entity)?
            * Transform::translation(offset)
            * Transform::scaling(Point::ORIGIN, factor, self.tol)?;
        let location = ogeom_topo::Location::of(self.model.add_datum(motion));
        Ok((
            solids.iter().map(|x| x.moved(&location)).collect(),
            faces.iter().map(|x| x.moved(&location)).collect(),
        ))
    }

    /// A subfigure definition's (308) solids and trimmed surfaces, at the
    /// definition's own coordinates, nested instances placed within it.
    fn subfigure(
        &mut self,
        de: i64,
        built: &mut FastMap<i64, (Vec<Shape>, Vec<Shape>)>,
        depth: usize,
    ) -> OgeomResult<(Vec<Shape>, Vec<Shape>)> {
        if let Some(found) = built.get(&de) {
            return Ok(found.clone());
        }
        let entity = self.entity(de)?;
        if entity.kind != 308 {
            ogeom_bail!(
                Construction,
                "D{de}: an instance names a type {} entity, not a subfigure \
                 definition",
                entity.kind
            );
        }
        let count = entity.count(2);
        let members: Vec<i64> = (0..count).map(|i| entity.at(3 + i).int()).collect();
        let (mut solids, mut faces) = (Vec::new(), Vec::new());
        let trims: Vec<i64> = members
            .iter()
            .copied()
            .filter(|&m| {
                self.file
                    .entity(m)
                    .is_some_and(|e| matches!(e.kind, 143 | 144))
            })
            .collect();
        let poles = self.poles_of_trims(&trims);
        // A nested definition reads its own trims against its own poles,
        // and the enclosing ones are back in force once it is read.
        let outer_poles = core::mem::replace(&mut self.trim_poles, poles);
        let mut read_members = || -> OgeomResult<()> {
            for &member in &members {
                let Some(kind) = self.file.entity(member).map(|m| m.kind) else {
                    continue;
                };
                match kind {
                    186 => solids.push(self.manifold_solid(member)?),
                    143 | 144 => faces.push(self.face(member)?),
                    408 => {
                        let (s, f) = self.instance(member, built, depth + 1)?;
                        solids.extend(s);
                        faces.extend(f);
                    }
                    _ => {}
                }
            }
            Ok(())
        };
        let read = read_members();
        self.trim_poles = outer_poles;
        read?;
        built.insert(de, (solids.clone(), faces.clone()));
        Ok((solids, faces))
    }

    /// The model-space annotation: every independent drafting entity a
    /// drawing does not own, as a callout of the lines the file draws (its
    /// leaders, witness lines and symbol geometry, carried by their own
    /// placements into the part's coordinates) named by its note's text.
    /// A dimension whose text reads as a number is also the semantic
    /// dimension of that value, which the callout draws; which geometry
    /// it measures the file does not say. A drawing's own annotation lives
    /// on the sheet, not on the part, and stays in the skipped table.
    fn annotations(
        &mut self,
    ) -> (
        Vec<ogeom_doc::Callout>,
        Vec<(ogeom_doc::Callout, ogeom_doc::Dimension)>,
    ) {
        let mut on_sheets: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
        for entity in self.file.entities.values().filter(|e| e.kind == 404) {
            let views = entity.count(0);
            let at = 1 + 3 * views;
            let count = entity.count(at);
            for i in 0..count {
                on_sheets.insert(entity.at(at + 1 + i).int().abs());
            }
        }
        let drafting: Vec<i64> = self
            .file
            .entities
            .iter()
            .filter(|(de, e)| {
                matches!(
                    e.kind,
                    202 | 206 | 208 | 210 | 212 | 214 | 216 | 218 | 220 | 222 | 228
                ) && (e.status / 10_000) % 100 == 0
                    && !on_sheets.contains(de)
            })
            .map(|(de, _)| *de)
            .collect();
        let mut callouts = Vec::new();
        let mut dimensions = Vec::new();
        for de in drafting {
            match self.drafting(de) {
                Ok((callout, Some(dimension))) => dimensions.push((callout, dimension)),
                Ok((callout, None)) => callouts.push(callout),
                Err(e) => self
                    .report
                    .warnings
                    .push(format!("D{de}: annotation not drawn: {e}")),
            }
        }
        (callouts, dimensions)
    }

    /// One drafting entity as a callout, and the dimension it states where
    /// it is one.
    fn drafting(
        &mut self,
        de: i64,
    ) -> OgeomResult<(ogeom_doc::Callout, Option<ogeom_doc::Dimension>)> {
        let entity = self.entity(de)?;
        let pointer = |i: usize| entity.at(i).int();
        let mut polylines: Vec<Vec<Point>> = Vec::new();
        let mut text = String::new();
        let mut measure: Option<(&str, ogeom_doc::MeasureKind)> = None;
        let draw = |reader: &mut Self, lines: &mut Vec<Vec<Point>>, de: i64| -> OgeomResult<()> {
            if de != 0 {
                lines.push(reader.drafted_line(de)?);
            }
            Ok(())
        };
        match entity.kind {
            212 => text = self.note_text(de)?,
            214 => polylines.push(self.drafted_line(de)?),
            216 => {
                text = self.note_text(pointer(0))?;
                for i in 1..=4 {
                    draw(self, &mut polylines, pointer(i))?;
                }
                measure = Some(("linear distance", ogeom_doc::MeasureKind::Length));
            }
            206 => {
                text = self.note_text(pointer(0))?;
                for i in 1..=2 {
                    draw(self, &mut polylines, pointer(i))?;
                }
                measure = Some(("diameter", ogeom_doc::MeasureKind::Length));
            }
            222 => {
                text = self.note_text(pointer(0))?;
                draw(self, &mut polylines, pointer(1))?;
                if entity.params.len() > 4 {
                    draw(self, &mut polylines, pointer(4))?;
                }
                measure = Some(("radius", ogeom_doc::MeasureKind::Length));
            }
            202 => {
                text = self.note_text(pointer(0))?;
                for i in [1, 2, 6, 7] {
                    draw(self, &mut polylines, pointer(i))?;
                }
                measure = Some(("angle", ogeom_doc::MeasureKind::Angle));
            }
            218 | 220 => {
                text = self.note_text(pointer(0))?;
                draw(self, &mut polylines, pointer(1))?;
                measure = Some(("ordinate", ogeom_doc::MeasureKind::Length));
                if entity.kind == 220 {
                    measure = None;
                }
            }
            208 | 210 => {
                // A flag note: its place and angle, then its note and
                // leaders; a label: its note, then its leaders.
                let at = if entity.kind == 208 { 4 } else { 0 };
                text = self.note_text(pointer(at))?;
                let n = entity.count(at + 1);
                for i in 0..n {
                    draw(self, &mut polylines, pointer(at + 2 + i))?;
                }
            }
            228 => {
                // Note, the symbol's geometry, then its leaders.
                text = self.note_text(pointer(0))?;
                let ng = entity.count(1);
                for i in 0..ng {
                    polylines.push(self.sampled_curve(pointer(2 + i))?);
                }
                let nl = entity.count(2 + ng);
                for i in 0..nl {
                    draw(self, &mut polylines, pointer(3 + ng + i))?;
                }
            }
            kind => ogeom_bail!(Construction, "type {kind} is not drafting"),
        }
        let placement = self.placement(entity)?;
        let plane = Frame::new(
            placement.apply(Point::ORIGIN),
            Direction::new(placement.apply_vector(Vector::Z), self.tol)?,
            Direction::new(placement.apply_vector(Vector::X), self.tol)?,
            self.tol,
        )
        .ok();
        let callout = ogeom_doc::Callout {
            name: text.clone(),
            plane,
            polylines,
            annotates: None,
        };
        let dimension = measure.and_then(|(name, kind)| {
            let value = first_number(&text)?;
            let value = match kind {
                ogeom_doc::MeasureKind::Angle => value.to_radians(),
                ogeom_doc::MeasureKind::Length => value * self.report.scale_mm,
            };
            Some(ogeom_doc::Dimension {
                name: name.to_owned(),
                values: vec![value],
                kind,
                plus: None,
                minus: None,
                features: Vec::new(),
                location: entity.kind == 216 || entity.kind == 218,
            })
        });
        Ok((callout, dimension))
    }

    /// A general note's (212) text, its strings one line each.
    fn note_text(&mut self, de: i64) -> OgeomResult<String> {
        if de == 0 {
            return Ok(String::new());
        }
        let entity = self.entity(de)?;
        if entity.kind != 212 {
            ogeom_bail!(Construction, "D{de}: type {} is not a note", entity.kind);
        }
        let count = entity.count(0);
        let mut lines = Vec::with_capacity(count);
        for i in 0..count {
            if let super::parse::Value::Text(t) = entity.at(1 + 12 * i + 11) {
                lines.push(t.clone());
            }
        }
        Ok(lines.join("\n"))
    }

    /// A leader (214) or witness line (106) as the polyline it draws, in
    /// the part's coordinates.
    fn drafted_line(&mut self, de: i64) -> OgeomResult<Vec<Point>> {
        let entity = self.entity(de)?;
        let s = self.report.scale_mm;
        let mut points = Vec::new();
        match entity.kind {
            214 => {
                let n = entity.count(0);
                let z = entity.at(3).real();
                points.push(Point::new(
                    entity.at(4).real() * s,
                    entity.at(5).real() * s,
                    z * s,
                ));
                for i in 0..n {
                    points.push(Point::new(
                        entity.at(6 + 2 * i).real() * s,
                        entity.at(7 + 2 * i).real() * s,
                        z * s,
                    ));
                }
            }
            106 => {
                let n = entity.count(1);
                let z = entity.at(2).real();
                for i in 0..n {
                    points.push(Point::new(
                        entity.at(3 + 2 * i).real() * s,
                        entity.at(4 + 2 * i).real() * s,
                        z * s,
                    ));
                }
            }
            _ => return self.sampled_curve(de),
        }
        let placement = self.placement(entity)?;
        Ok(points.into_iter().map(|p| placement.apply(p)).collect())
    }

    /// Any curve the reader translates, as a polyline for drawing.
    fn sampled_curve(&mut self, de: i64) -> OgeomResult<Vec<Point>> {
        let (curve, range) = self.curve(de)?;
        (0..=32)
            .map(|i| {
                curve.point_at(
                    range.0 + (range.1 - range.0) * f64::from(i) / 32.0,
                    self.tol,
                )
            })
            .collect()
    }

    fn document(&mut self, solids: &[(i64, Shape)], sheets: &[Shape]) -> ogeom_doc::Document {
        let colours: Vec<(Shape, ogeom_doc::Colour)> = solids
            .iter()
            .filter_map(|(de, s)| self.colour_of(*de).map(|c| (s.clone(), c)))
            .collect();
        let mut document = ogeom_doc::Document::over(std::mem::take(&mut self.model));
        for (i, (de, solid)) in solids.iter().enumerate() {
            let label = self
                .file
                .entity(*de)
                .map(|e| e.label.clone())
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| format!("solid-{i}"));
            document.add_part(label, solid.clone());
        }
        for (i, sheet) in sheets.iter().enumerate() {
            document.add_part(format!("sheet-{i}"), sheet.clone());
        }
        for (shape, colour) in colours {
            document.set_colour(&shape, colour);
        }
        document
    }

    /// The colour a directory entry states: a negated field points at a 314
    /// entity's RGB percentages; a small positive names the fixed palette.
    fn colour_of(&mut self, de: i64) -> Option<ogeom_doc::Colour> {
        let entity = self.file.entity(de)?;
        let c = entity.colour;
        if c < 0 {
            let e = self.file.entity(-c)?;
            self.visited.insert(-c, ());
            if e.kind != 314 {
                return None;
            }
            return Some(ogeom_doc::Colour::rgb(
                (e.at(0).real() / 100.0).clamp(0.0, 1.0),
                (e.at(1).real() / 100.0).clamp(0.0, 1.0),
                (e.at(2).real() / 100.0).clamp(0.0, 1.0),
            ));
        }
        // 1 black, 2 red, 3 green, 4 blue, 5 yellow, 6 magenta, 7 cyan,
        // 8 white: the specification's own palette.
        let palette = [
            (0.0, 0.0, 0.0),
            (1.0, 0.0, 0.0),
            (0.0, 1.0, 0.0),
            (0.0, 0.0, 1.0),
            (1.0, 1.0, 0.0),
            (1.0, 0.0, 1.0),
            (0.0, 1.0, 1.0),
            (1.0, 1.0, 1.0),
        ];
        let index = usize::try_from(c).ok()?.checked_sub(1)?;
        palette
            .get(index)
            .map(|&(r, g, b)| ogeom_doc::Colour::rgb(r, g, b))
    }
}

/// The file's levels and groups as the document's layers: every shape on
/// the layer of the level its entity sits on (a negative level names a
/// definition-levels property listing several), and every group (402,
/// forms 1, 7, 14 and 15) a layer named by the group's label holding the
/// shapes its members built.
fn layers(
    file: &File,
    document: &mut ogeom_doc::Document,
    built_from: &[(i64, Shape)],
    report: &mut IgesReport,
) {
    let mut by_name: FastMap<String, ogeom_doc::LayerId> = FastMap::default();
    let mut layer = |document: &mut ogeom_doc::Document, name: String| {
        *by_name
            .entry(name.clone())
            .or_insert_with(|| document.add_layer(name))
    };
    for (de, shape) in built_from {
        let Some(entity) = file.entity(*de) else {
            continue;
        };
        let levels: Vec<i64> = if entity.level > 0 {
            vec![entity.level]
        } else if entity.level < 0 {
            match file.entity(-entity.level) {
                Some(p) if p.kind == 406 && p.form == 1 => {
                    let n = p.count(0);
                    (0..n).map(|i| p.at(1 + i).int()).collect()
                }
                _ => {
                    report
                        .warnings
                        .push(format!("D{de}: its level names no levels property"));
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        for level in levels {
            let id = layer(document, format!("level {level}"));
            document.place_on_layer(shape, id);
        }
    }
    for (de, group) in &file.entities {
        if group.kind != 402 || !matches!(group.form, 1 | 7 | 14 | 15) {
            continue;
        }
        let n = group.count(0);
        let members: Vec<i64> = (0..n).map(|i| group.at(1 + i).int().abs()).collect();
        let shapes: Vec<&Shape> = built_from
            .iter()
            .filter(|(from, _)| members.contains(from))
            .map(|(_, s)| s)
            .collect();
        if shapes.is_empty() {
            continue;
        }
        let name = if group.label.is_empty() {
            format!("group D{de}")
        } else {
            group.label.clone()
        };
        let id = layer(document, name);
        for shape in shapes {
            document.place_on_layer(shape, id);
        }
    }
}

/// The value a dimension's text states: `R5.5`, `Ø10`, `45°` read as 5.5,
/// 10 and 45, and a patterned callout's leading count (`2X Ø5`, `4 x 12.7`)
/// passed over for the value after it.
fn first_number(text: &str) -> Option<f64> {
    let trimmed = text.trim_start();
    let count = trimmed
        .find(|c: char| !c.is_ascii_digit())
        .filter(|&n| n > 0)
        .and_then(|n| {
            let rest = trimmed[n..].trim_start();
            rest.strip_prefix(['X', 'x']).map(|after| after.len())
        });
    let text = match count {
        Some(len) => &trimmed[trimmed.len() - len..],
        None => text,
    };
    let start = text.find(|c: char| c.is_ascii_digit() || c == '.')?;
    let tail = &text[start..];
    let end = tail
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(tail.len());
    tail[..end].parse().ok()
}

/// A trimmed carrier where the range is a strict part of the domain: a
/// generatrix used by a sweep is exactly its stated span.
/// What deriving an edge's image asks of the reader besides the image: a
/// warning to add, or a gap the edge must state, in the order they arise.
enum Effect {
    Warn(String),
    Gap(f64),
}

/// An edge's image on a surface, `None` where none could be derived, and
/// what deriving it asks of the reader.
type DerivedImage = (Option<ogeom_geom::PlanarCurve>, Vec<Effect>);

/// One edge's image on one surface, exact where the pair has a closed form
/// and fitted where it does not: the same policy the STEP reader applies,
/// through the shared machinery.
fn derive_image(
    curve: &Curve,
    range: (f64, f64),
    surface: &SurfaceGeometry,
    tol: Tolerances,
) -> DerivedImage {
    use ogeom_geom::PlanarCurve;
    let widen = |p: PlanarCurve| -> PlanarCurve {
        if let PlanarCurve::Line(l) = &p {
            use ogeom_geom::Curve2d as _;
            let (lo, hi) = (l.domain().0.min(range.0), l.domain().1.max(range.1));
            if let Ok(wider) = ogeom_geom::Line2d::over(l.axis(), lo, hi) {
                return wider.into();
            }
        }
        p
    };
    let mut effects = Vec::new();
    let found = match ogeom_intersect::exact_pcurve_over(curve, range, surface, tol).map(widen) {
        Some(exact) => exact,
        None => match crate::pcurves::fit_projected_pcurve(curve, range, surface, tol) {
            Ok((fitted, error, met, worst_off, slop)) => {
                if let Some(w) = slop {
                    effects.push(Effect::Warn(w));
                }
                if !met {
                    effects.push(Effect::Warn(format!(
                        "a projected pcurve fit stopped at {error:.2e}; \
                         the face's mesh may sit that far off along this edge"
                    )));
                }
                effects.push(Effect::Gap(worst_off));
                fitted
            }
            Err(e) => {
                effects.push(Effect::Warn(format!(
                    "no pcurve for an edge on this surface ({e}); the \
                     face may not triangulate"
                )));
                return (None, effects);
            }
        },
    };
    // The edge provably stands this far from the face it bounds, whether
    // the curve lies off the surface or a fit strays between its
    // samples. A pcurve that does not evaluate over the edge's range
    // states no gap here; the face that uses it fails on it there.
    let off =
        ogeom_algo::pcurve_fit::lifted_gap((curve, range), (&found, range), surface, false, tol)
            .unwrap_or(0.0);
    effects.push(Effect::Gap(off));
    (Some(found), effects)
}

fn trimmed_to(curve: Curve, range: (f64, f64), tol: Tolerances) -> OgeomResult<Curve> {
    let (lo, hi) = curve.domain();
    if (range.0 - lo).abs() < tol.parametric() && (range.1 - hi).abs() < tol.parametric() {
        return Ok(curve);
    }
    Ok(Curve::from(TrimmedCurve::new(
        curve, range.0, range.1, tol,
    )?))
}

use crate::inversion::parameter_on;

/// A frame with the given axis direction, reference direction chosen stably.
fn frame_about(origin: Point, axis: Direction, tol: Tolerances) -> OgeomResult<Frame> {
    let seed = if axis.vector().dot(Vector::X).abs() < 0.9 {
        Vector::X
    } else {
        Vector::Y
    };
    let x = Direction::from_cross(axis.vector(), seed, tol)?;
    Frame::new(origin, axis, x, tol)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::parse::Value;
    use super::*;

    const T: Tolerances = Tolerances::millimetres();

    fn entity(kind: i64, form: i64, params: Vec<Value>) -> Entity {
        Entity {
            kind,
            form,
            transform: 0,
            colour: 0,
            level: 0,
            status: 0,
            label: String::new(),
            params,
        }
    }

    fn file(entities: Vec<(i64, Entity)>) -> File {
        let mut global = vec![Value::Default; 16];
        global[13] = Value::Int(2);
        File {
            global,
            entities: entities.into_iter().collect(),
        }
    }

    fn reader(file: &File) -> Reader<'_> {
        Reader {
            file,
            model: Model::new(),
            report: IgesReport {
                scale_mm: 1.0,
                ..IgesReport::default()
            },
            visited: BTreeMap::new(),
            vertices: FastMap::default(),
            edges: FastMap::default(),
            pieces: FastMap::default(),
            pole_vertices: Vec::new(),
            trim_poles: Vec::new(),
            vertex_misses: (0, 0.0),
            images: FastMap::default(),
            face_de: None,
            depth: 0,
            tol: T,
        }
    }

    /// A transform placed by itself, and a composite curve made of itself:
    /// both are refused, not followed until the stack runs out.
    #[test]
    fn entities_that_refer_in_a_circle_are_refused() {
        let mut turned = entity(124, 0, vec![Value::Real(1.0); 12]);
        turned.transform = 1;
        let mut line = entity(110, 0, vec![Value::Real(0.0); 6]);
        line.transform = 1;
        let f = file(vec![(1, turned), (3, line)]);
        let mut r = reader(&f);
        assert!(r.curve(3).is_err());

        let composite = entity(102, 0, vec![Value::Int(1), Value::Int(1)]);
        let f = file(vec![(1, composite)]);
        let mut r = reader(&f);
        assert!(r.curve(1).is_err());
    }

    fn reals(values: &[f64]) -> Vec<Value> {
        values.iter().map(|v| Value::Real(*v)).collect()
    }

    fn line(from: [f64; 3], to: [f64; 3]) -> Entity {
        entity(
            110,
            0,
            reals(&[from[0], from[1], from[2], to[0], to[1], to[2]]),
        )
    }

    /// A conic arc whose squares differ in sign is a hyperbola, and one
    /// missing a square is a parabola; each reads as its own curve, the arc
    /// running from the file's start to its terminate point whichever way
    /// round the file lists them.
    #[test]
    fn hyperbola_and_parabola_conic_arcs_read_as_their_curves() {
        let (cosh1, sinh1) = (1.0_f64.cosh(), 1.0_f64.sinh());
        // x²/4 − y²/9 = 1, the arc from the vertex to parameter one.
        let hyperbola = |forward: bool| {
            let (s, e) = if forward {
                ([2.0, 0.0], [2.0 * cosh1, 3.0 * sinh1])
            } else {
                ([2.0 * cosh1, 3.0 * sinh1], [2.0, 0.0])
            };
            entity(
                104,
                2,
                reals(&[
                    0.25,
                    0.0,
                    -1.0 / 9.0,
                    0.0,
                    0.0,
                    -1.0,
                    0.0,
                    s[0],
                    s[1],
                    e[0],
                    e[1],
                ]),
            )
        };
        // x² − 4 y = 0 opening along y, and y² − 4 x = 0 opening along x.
        let parabola_y = entity(
            104,
            3,
            reals(&[1.0, 0.0, 0.0, 0.0, -4.0, 0.0, 0.0, 0.0, 0.0, 4.0, 4.0]),
        );
        let parabola_x = entity(
            104,
            3,
            reals(&[0.0, 0.0, 1.0, -4.0, 0.0, 0.0, 0.0, 4.0, 4.0, 0.0, 0.0]),
        );
        let deck = file(vec![
            (1, hyperbola(true)),
            (3, hyperbola(false)),
            (5, parabola_y),
            (7, parabola_x),
        ]);
        let mut reader = reader(&deck);
        for de in [1, 3] {
            let (curve, (t0, t1)) = reader.curve(de).unwrap();
            assert!(matches!(curve, Curve::Hyperbola(_)), "D{de} is a hyperbola");
            assert!(t1 > t0, "the arc runs forward: {t0} .. {t1}");
            let entity = deck.entity(de).unwrap();
            let start = Point::new(entity.at(7).real(), entity.at(8).real(), 0.0);
            let end = Point::new(entity.at(9).real(), entity.at(10).real(), 0.0);
            assert!(curve.point_at(t0, T).unwrap().distance(start) < 1e-9);
            assert!(curve.point_at(t1, T).unwrap().distance(end) < 1e-9);
            let mid = curve.point_at(f64::midpoint(t0, t1), T).unwrap();
            assert!((mid.x * mid.x / 4.0 - mid.y * mid.y / 9.0 - 1.0).abs() < 1e-9);
        }
        for (de, along_y) in [(5, true), (7, false)] {
            let (curve, (t0, t1)) = reader.curve(de).unwrap();
            assert!(matches!(curve, Curve::Parabola(_)), "D{de} is a parabola");
            assert!(t1 > t0);
            let (start, end) = if along_y {
                (Point::ORIGIN, Point::new(4.0, 4.0, 0.0))
            } else {
                (Point::new(4.0, 4.0, 0.0), Point::ORIGIN)
            };
            assert!(curve.point_at(t0, T).unwrap().distance(start) < 1e-9);
            assert!(curve.point_at(t1, T).unwrap().distance(end) < 1e-9);
            let mid = curve.point_at(f64::midpoint(t0, t1), T).unwrap();
            let residual = if along_y {
                mid.x * mid.x - 4.0 * mid.y
            } else {
                mid.y * mid.y - 4.0 * mid.x
            };
            assert!(residual.abs() < 1e-9, "on the parabola: {mid:?}");
        }
    }

    /// A ruled surface between two lines is the bilinear patch between
    /// them, the second line walked backward where the direction flag says.
    #[test]
    fn a_ruled_surface_reads_as_the_patch_between_its_curves() {
        let deck = file(vec![
            (1, line([0.0, 0.0, 0.0], [10.0, 0.0, 0.0])),
            (3, line([0.0, 5.0, 2.0], [10.0, 5.0, 2.0])),
            (
                5,
                entity(
                    118,
                    1,
                    vec![Value::Int(1), Value::Int(3), Value::Int(0), Value::Int(0)],
                ),
            ),
            (
                7,
                entity(
                    118,
                    1,
                    vec![Value::Int(1), Value::Int(3), Value::Int(1), Value::Int(0)],
                ),
            ),
        ]);
        let mut reader = reader(&deck);
        let straight = reader.surface(5).unwrap();
        let turned = reader.surface(7).unwrap();
        use ogeom_geom::Surface as _;
        let middle = straight.point_at(0.5, 0.5, T).unwrap();
        assert!(
            middle.distance(Point::new(5.0, 2.5, 1.0)) < 1e-9,
            "{middle:?}"
        );
        let far = straight.point_at(0.0, 1.0, T).unwrap();
        assert!(far.distance(Point::new(0.0, 5.0, 2.0)) < 1e-9, "{far:?}");
        let far = turned.point_at(0.0, 1.0, T).unwrap();
        assert!(far.distance(Point::new(10.0, 5.0, 2.0)) < 1e-9, "{far:?}");
    }

    /// An offset surface displaces its base along the file's direction,
    /// and an offset curve along the file's reference crossed with the
    /// tangent, whichever way round this vocabulary spells either.
    #[test]
    fn offset_entities_displace_the_way_the_file_says() {
        let deck = file(vec![
            (1, entity(108, 0, reals(&[0.0, 0.0, 1.0, 0.0]))),
            (
                3,
                entity(
                    140,
                    0,
                    vec![
                        Value::Real(0.0),
                        Value::Real(0.0),
                        Value::Real(1.0),
                        Value::Real(3.0),
                        Value::Int(1),
                    ],
                ),
            ),
            (
                5,
                entity(
                    140,
                    0,
                    vec![
                        Value::Real(0.0),
                        Value::Real(0.0),
                        Value::Real(-1.0),
                        Value::Real(3.0),
                        Value::Int(1),
                    ],
                ),
            ),
            (7, line([0.0, 0.0, 0.0], [10.0, 0.0, 0.0])),
            (
                9,
                entity(
                    130,
                    0,
                    vec![
                        Value::Int(7),
                        Value::Int(1),
                        Value::Int(0),
                        Value::Int(3),
                        Value::Int(0),
                        Value::Real(2.0),
                        Value::Real(0.0),
                        Value::Real(2.0),
                        Value::Real(0.0),
                        Value::Real(0.0),
                        Value::Real(0.0),
                        Value::Real(1.0),
                        Value::Real(0.0),
                        Value::Real(10.0),
                    ],
                ),
            ),
        ]);
        let mut reader = reader(&deck);
        use ogeom_geom::Surface as _;
        let up = reader.surface(3).unwrap();
        assert!((up.point_at(1.0, 2.0, T).unwrap().z - 3.0).abs() < 1e-9);
        let down = reader.surface(5).unwrap();
        assert!((down.point_at(1.0, 2.0, T).unwrap().z + 3.0).abs() < 1e-9);
        let (offset, (t0, t1)) = reader.curve(9).unwrap();
        assert!(matches!(offset, Curve::Offset(_)));
        assert!((t0, t1) == (0.0, 10.0));
        let at = offset.point_at(5.0, T).unwrap();
        assert!(at.distance(Point::new(5.0, 2.0, 0.0)) < 1e-9, "{at:?}");
    }

    /// A conic whose axes turn reads as the ellipse it is: `x'²/16 + y'²/4
    /// = 1` turned thirty degrees, written with its cross term.
    #[test]
    fn a_rotated_conic_reads_as_its_turned_curve() {
        let theta = 30.0_f64.to_radians();
        let (sin, cos) = theta.sin_cos();
        let (a2, b2) = (16.0, 4.0);
        let coefficients = [
            cos * cos / a2 + sin * sin / b2,
            2.0 * sin * cos * (1.0 / a2 - 1.0 / b2),
            sin * sin / a2 + cos * cos / b2,
            0.0,
            0.0,
            -1.0,
        ];
        let start = [4.0 * cos, 4.0 * sin];
        let end = [-2.0 * sin, 2.0 * cos];
        let mut values = coefficients.to_vec();
        values.extend([0.0, start[0], start[1], end[0], end[1]]);
        let deck = file(vec![(1, entity(104, 1, reals(&values)))]);
        let mut reader = reader(&deck);
        let (curve, (t0, t1)) = reader.curve(1).unwrap();
        assert!(matches!(curve, Curve::Ellipse(_)), "a turned ellipse");
        assert!(
            curve
                .point_at(t0, T)
                .unwrap()
                .distance(Point::new(start[0], start[1], 0.0))
                < 1e-9
        );
        assert!(
            curve
                .point_at(t1, T)
                .unwrap()
                .distance(Point::new(end[0], end[1], 0.0))
                < 1e-9
        );
        for k in 0..=8 {
            let p = curve
                .point_at(t0 + (t1 - t0) * f64::from(k) / 8.0, T)
                .unwrap();
            let [a, b, c, _, _, f] = coefficients;
            let residual = a * p.x * p.x + b * p.x * p.y + c * p.y * p.y + f;
            assert!(residual.abs() < 1e-9, "on the conic: {p:?}");
        }
    }

    /// An offset growing linearly with arc length (type 2) from one at the
    /// base line's start to three at its end: the displaced curve rises
    /// along the reference crossed with the tangent.
    #[test]
    fn a_linearly_varying_offset_curve_reads_as_its_fitted_displacement() {
        let deck = file(vec![
            (1, line([0.0, 0.0, 0.0], [10.0, 0.0, 0.0])),
            (
                3,
                entity(
                    130,
                    0,
                    vec![
                        Value::Int(1),
                        Value::Int(2),
                        Value::Int(0),
                        Value::Int(0),
                        Value::Int(0),
                        Value::Real(1.0),
                        Value::Real(0.0),
                        Value::Real(3.0),
                        Value::Real(10.0),
                        Value::Real(0.0),
                        Value::Real(0.0),
                        Value::Real(1.0),
                        Value::Real(0.0),
                        Value::Real(0.0),
                    ],
                ),
            ),
        ]);
        let mut reader = reader(&deck);
        let (curve, (t0, t1)) = reader.curve(3).unwrap();
        for k in 0..=10 {
            let t = t0 + (t1 - t0) * f64::from(k) / 10.0;
            let p = curve.point_at(t, T).unwrap();
            let want = Point::new(p.x, 1.0 + 2.0 * p.x / 10.0, 0.0);
            assert!(p.distance(want) < 1e-5, "{p:?} against {want:?}");
        }
        assert!(
            curve
                .point_at(t0, T)
                .unwrap()
                .distance(Point::new(0.0, 1.0, 0.0))
                < 1e-5
        );
        assert!(
            curve
                .point_at(t1, T)
                .unwrap()
                .distance(Point::new(10.0, 3.0, 0.0))
                < 1e-5
        );
    }

    /// An offset varying with arc length along a base of more spans than
    /// any fixed sampling follows reads as the displaced curve everywhere
    /// along it, not only at the samples.
    #[test]
    fn a_varying_offset_of_a_many_span_base_holds_between_the_samples() {
        use ogeom_geom::Curve3d as _;
        const SPANS: usize = 400;
        let n = SPANS + 3;
        let mut base = vec![
            Value::Int(i64::try_from(n - 1).unwrap()),
            Value::Int(3),
            Value::Int(1),
            Value::Int(0),
            Value::Int(1),
            Value::Int(0),
        ];
        let mut knots = vec![0.0; 4];
        #[allow(clippy::cast_precision_loss)]
        knots.extend((1..SPANS).map(|i| i as f64 / SPANS as f64));
        knots.extend([1.0; 4]);
        base.extend(reals(&knots));
        base.extend(reals(&vec![1.0; n]));
        for i in 0..n {
            #[allow(clippy::cast_precision_loss)]
            let x = 10.0 * i as f64 / (n - 1) as f64;
            base.extend(reals(&[
                x,
                if i.is_multiple_of(2) { 5e-4 } else { -5e-4 },
                0.0,
            ]));
        }
        base.extend(reals(&[0.0, 1.0, 0.0, 0.0, 1.0]));
        let deck = file(vec![(1, entity(126, 0, base)), (3, entity(130, 0, vec![]))]);
        let mut reader = reader(&deck);
        let (basis, range) = reader.curve(1).unwrap();
        let length = ogeom_algo::curve_length(&basis, range, T).unwrap();
        let offset = entity(
            130,
            0,
            vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(0),
                Value::Int(0),
                Value::Int(0),
                Value::Real(0.05),
                Value::Real(0.0),
                Value::Real(0.1),
                Value::Real(length),
                Value::Real(0.0),
                Value::Real(0.0),
                Value::Real(1.0),
                Value::Real(0.0),
                Value::Real(0.0),
            ],
        );
        let deck = file(vec![(1, deck.entities[&1].clone()), (3, offset)]);
        let mut reader = super::tests::reader(&deck);
        let (curve, (t0, t1)) = reader.curve(3).unwrap();
        let mut worst = 0.0_f64;
        let mut along = 0.0;
        let mut last = t0;
        for k in 0..=4000 {
            let t = t0 + (t1 - t0) * f64::from(k) / 4000.0;
            along += ogeom_algo::curve_length(&basis, (last, t), T).unwrap();
            last = t;
            let d = 0.05 + 0.05 * along / length;
            let side = Vector::Z.cross(basis.d1_at(t, T).unwrap());
            let want = basis.point_at(t, T).unwrap() + side * (d / side.magnitude());
            worst = worst.max(curve.point_at(t, T).unwrap().distance(want));
        }
        assert!(
            worst < 1e-4,
            "the offset strays {worst} between its samples: {:?}",
            reader.report.warnings
        );
    }

    /// The parametric spline surface `(s, t, s·t)`: one patch over
    /// `[0, 2] × [0, 3]`, and the same surface as two patches split at
    /// `s = 1` (the second re-expanded about its own break point). Both
    /// read as the exact bicubic B-spline through the same points.
    #[test]
    fn a_parametric_spline_surface_reads_as_its_exact_bspline() {
        let patch = |x: &[(usize, f64)], y: &[(usize, f64)], z: &[(usize, f64)]| -> Vec<f64> {
            let mut out = vec![0.0; 48];
            for (axis, terms) in [x, y, z].iter().enumerate() {
                for &(k, v) in *terms {
                    out[16 * axis + k] = v;
                }
            }
            out
        };
        let header = |m: i64, n: i64, tu: &[f64], tv: &[f64]| -> Vec<Value> {
            let mut out = vec![Value::Int(3), Value::Int(0), Value::Int(m), Value::Int(n)];
            out.extend(reals(tu));
            out.extend(reals(tv));
            out
        };
        // x = s, y = t, z = s·t: index 1 is s, 4 is t, 5 is s·t.
        let whole = {
            let mut v = header(1, 1, &[0.0, 2.0], &[0.0, 3.0]);
            v.extend(reals(&patch(&[(1, 1.0)], &[(4, 1.0)], &[(5, 1.0)])));
            v.extend(reals(&[0.0; 48]));
            v.extend(reals(&[0.0; 96]));
            v
        };
        // Split at s = 1: the second patch about s = 1 is x = 1 + s',
        // z = t + s'·t.
        let split = {
            let mut v = header(2, 1, &[0.0, 1.0, 2.0], &[0.0, 3.0]);
            v.extend(reals(&patch(&[(1, 1.0)], &[(4, 1.0)], &[(5, 1.0)])));
            v.extend(reals(&[0.0; 48]));
            v.extend(reals(&patch(
                &[(0, 1.0), (1, 1.0)],
                &[(4, 1.0)],
                &[(4, 1.0), (5, 1.0)],
            )));
            v.extend(reals(&[0.0; 48]));
            v.extend(reals(&[0.0; 96]));
            v
        };
        let deck = file(vec![(1, entity(114, 0, whole)), (3, entity(114, 0, split))]);
        let mut reader = reader(&deck);
        for de in [1, 3] {
            let surface = reader.surface(de).unwrap();
            for (u, v) in [(0.0, 0.0), (0.5, 1.0), (1.0, 2.5), (1.5, 0.7), (2.0, 3.0)] {
                let p = ogeom_geom::Surface::point_at(&surface, u, v, T).unwrap();
                let want = Point::new(u, v, u * v);
                assert!(p.distance(want) < 1e-12, "D{de} at ({u}, {v}): {p:?}");
            }
        }
    }

    /// A curve-on-surface is creation, surface, parameter-space curve,
    /// model-space curve: the model curve is read where the file gives it,
    /// and a trim given only in the surface's parameters is lifted through
    /// a B-spline surface.
    #[test]
    fn a_parameter_space_trim_lifts_through_its_bspline_surface() {
        // The bilinear patch (10u, 10v, 0) over the unit square.
        let surface = entity(
            128,
            0,
            vec![
                Value::Int(1),
                Value::Int(1),
                Value::Int(1),
                Value::Int(1),
                Value::Int(0),
                Value::Int(0),
                Value::Int(1),
                Value::Int(0),
                Value::Int(0),
                Value::Real(0.0),
                Value::Real(0.0),
                Value::Real(1.0),
                Value::Real(1.0),
                Value::Real(0.0),
                Value::Real(0.0),
                Value::Real(1.0),
                Value::Real(1.0),
                Value::Real(1.0),
                Value::Real(1.0),
                Value::Real(1.0),
                Value::Real(1.0),
                Value::Real(0.0),
                Value::Real(0.0),
                Value::Real(0.0),
                Value::Real(10.0),
                Value::Real(0.0),
                Value::Real(0.0),
                Value::Real(0.0),
                Value::Real(10.0),
                Value::Real(0.0),
                Value::Real(10.0),
                Value::Real(10.0),
                Value::Real(0.0),
                Value::Real(0.0),
                Value::Real(1.0),
                Value::Real(0.0),
                Value::Real(1.0),
            ],
        );
        let on_surface = |model_curve: i64| {
            entity(
                142,
                0,
                vec![
                    Value::Int(0),
                    Value::Int(1),
                    Value::Int(3),
                    Value::Int(model_curve),
                    Value::Int(0),
                ],
            )
        };
        let deck = file(vec![
            (1, surface),
            (3, line([0.2, 0.2, 0.0], [0.8, 0.8, 0.0])),
            (5, on_surface(0)),
            (7, line([2.0, 2.0, 0.0], [8.0, 8.0, 0.0])),
            (9, on_surface(7)),
        ]);
        let mut reader = reader(&deck);
        for de in [5, 9] {
            let segments = reader.boundary_segments(de).unwrap();
            let [(curve, (t0, t1))] = segments.as_slice() else {
                panic!("one segment");
            };
            assert!(
                curve
                    .point_at(*t0, T)
                    .unwrap()
                    .distance(Point::new(2.0, 2.0, 0.0))
                    < 1e-6
            );
            assert!(
                curve
                    .point_at(*t1, T)
                    .unwrap()
                    .distance(Point::new(8.0, 8.0, 0.0))
                    < 1e-6
            );
            let mid = curve.point_at(f64::midpoint(*t0, *t1), T).unwrap();
            assert!(
                (mid.x - mid.y).abs() < 1e-6 && mid.z.abs() < 1e-9,
                "D{de} {mid:?}"
            );
        }
    }

    /// A trim given only in a patch's parameters as a polyline of more
    /// corners than any fixed sampling holds lifts with every corner where
    /// the file put it.
    #[test]
    fn a_many_cornered_parameter_space_trim_keeps_its_corners() {
        // The bilinear patch (10u, 10v, 0) over the unit square.
        let mut patch = vec![
            Value::Int(1),
            Value::Int(1),
            Value::Int(1),
            Value::Int(1),
            Value::Int(0),
            Value::Int(0),
            Value::Int(1),
            Value::Int(0),
            Value::Int(0),
        ];
        patch.extend(reals(&[0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0]));
        patch.extend(reals(&[1.0; 4]));
        patch.extend(reals(&[
            0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0,
        ]));
        patch.extend(reals(&[0.0, 1.0, 0.0, 1.0]));
        // A zigzag of 301 corners in the patch's parameters.
        const CORNERS: usize = 301;
        let corner = |i: usize| -> [f64; 2] {
            #[allow(clippy::cast_precision_loss)]
            let f = i as f64 / (CORNERS - 1) as f64;
            [0.1 + 0.8 * f, if i.is_multiple_of(2) { 0.49 } else { 0.51 }]
        };
        let mut zigzag = vec![
            Value::Int(i64::try_from(CORNERS - 1).unwrap()),
            Value::Int(1),
            Value::Int(1),
            Value::Int(0),
            Value::Int(1),
            Value::Int(0),
        ];
        let mut knots = vec![0.0];
        #[allow(clippy::cast_precision_loss)]
        knots.extend((0..CORNERS).map(|i| i as f64 / (CORNERS - 1) as f64));
        knots.push(1.0);
        zigzag.extend(reals(&knots));
        zigzag.extend(reals(&[1.0; CORNERS]));
        for i in 0..CORNERS {
            let [u, v] = corner(i);
            zigzag.extend(reals(&[u, v, 0.0]));
        }
        zigzag.extend(reals(&[0.0, 1.0, 0.0, 0.0, 1.0]));
        let deck = file(vec![
            (1, entity(128, 0, patch)),
            (3, entity(126, 0, zigzag)),
            (
                5,
                entity(
                    142,
                    0,
                    vec![
                        Value::Int(0),
                        Value::Int(1),
                        Value::Int(3),
                        Value::Int(0),
                        Value::Int(0),
                    ],
                ),
            ),
        ]);
        let mut reader = reader(&deck);
        let segments = reader.boundary_segments(5).unwrap();
        let [(curve, (t0, t1))] = segments.as_slice() else {
            panic!("one segment");
        };
        // Each corner, lifted, against the nearest point of the read trim.
        let fine: Vec<Point> = (0..=60_000)
            .map(|k| {
                let t = t0 + (t1 - t0) * f64::from(k) / 60_000.0;
                curve.point_at(t, T).unwrap()
            })
            .collect();
        let mut worst = 0.0_f64;
        for i in (0..CORNERS).step_by(7) {
            let [u, v] = corner(i);
            let want = Point::new(10.0 * u, 10.0 * v, 0.0);
            let near = fine
                .iter()
                .map(|p| p.distance(want))
                .fold(f64::INFINITY, f64::min);
            worst = worst.max(near);
        }
        assert!(
            worst < 1e-6,
            "a corner lost by {worst}: {:?}",
            reader.report.warnings
        );
    }

    /// A trim given in an analytic surface's own parameters lifts through
    /// the format's parameterization: a cylinder's angle in degrees and
    /// height, a sphere's two angles, a torus's pair turned; each angle
    /// measured from the surface's reference direction.
    #[test]
    fn a_parameter_space_trim_lifts_through_an_analytic_surface() {
        let point = |p: [f64; 3]| entity(116, 0, reals(&p));
        let direction = |d: [f64; 3]| entity(123, 0, reals(&d));
        let on_surface = |surface: i64| {
            entity(
                142,
                0,
                vec![
                    Value::Int(0),
                    Value::Int(surface),
                    Value::Int(3),
                    Value::Int(0),
                    Value::Int(0),
                ],
            )
        };
        // Location, axis +z, reference +y (so angle zero points along +y).
        type Case = (Entity, [f64; 2], [f64; 2], Point, Point);
        let cases: Vec<Case> = vec![
            (
                // Cylinder radius 2: angle 0 -> 90 degrees, height 0 -> 5.
                entity(
                    192,
                    1,
                    vec![
                        Value::Int(11),
                        Value::Int(13),
                        Value::Real(2.0),
                        Value::Int(15),
                    ],
                ),
                [0.0, 0.0],
                [90.0, 5.0],
                Point::new(0.0, 2.0, 0.0),
                Point::new(-2.0, 0.0, 5.0),
            ),
            (
                // Sphere radius 3: from (0, 0) to (90 degrees, 90 degrees), the pole.
                entity(
                    196,
                    1,
                    vec![
                        Value::Int(11),
                        Value::Real(3.0),
                        Value::Int(13),
                        Value::Int(15),
                    ],
                ),
                [0.0, 0.0],
                [90.0, 45.0],
                Point::new(0.0, 3.0, 0.0),
                Point::new(
                    -3.0 * core::f64::consts::FRAC_1_SQRT_2,
                    0.0,
                    3.0 * core::f64::consts::FRAC_1_SQRT_2,
                ),
            ),
            (
                // Torus 5 by 1: IGES (u round the tube, v round the axis
                // measured back from a full turn). (0, 360) is the outer
                // equator at angle zero; (90, 270) a quarter round each.
                entity(
                    198,
                    1,
                    vec![
                        Value::Int(11),
                        Value::Int(13),
                        Value::Real(5.0),
                        Value::Real(1.0),
                        Value::Int(15),
                    ],
                ),
                [0.0, 360.0],
                [90.0, 270.0],
                Point::new(0.0, 6.0, 0.0),
                Point::new(-5.0, 0.0, 1.0),
            ),
        ];
        for (surface, from, to, at_from, at_to) in cases {
            let deck = file(vec![
                (1, surface),
                (3, line([from[0], from[1], 0.0], [to[0], to[1], 0.0])),
                (5, on_surface(1)),
                (11, point([0.0, 0.0, 0.0])),
                (13, direction([0.0, 0.0, 1.0])),
                (15, direction([0.0, 1.0, 0.0])),
            ]);
            let mut reader = reader(&deck);
            let segments = reader.boundary_segments(5).unwrap();
            let [(curve, (t0, t1))] = segments.as_slice() else {
                panic!("one segment");
            };
            let (a, b) = (
                curve.point_at(*t0, T).unwrap(),
                curve.point_at(*t1, T).unwrap(),
            );
            assert!(a.distance(at_from) < 1e-6, "{a:?} against {at_from:?}");
            assert!(b.distance(at_to) < 1e-6, "{b:?} against {at_to:?}");
        }
    }

    /// A tabulated cylinder's trim is in fractions of its directrix and
    /// generator; a surface of revolution's in its generatrix's own
    /// parameter (a line's over [0, 1]) and the angle turned.
    #[test]
    fn a_parameter_space_trim_lifts_through_a_swept_surface() {
        let on_surface = |surface: i64| {
            entity(
                142,
                0,
                vec![
                    Value::Int(0),
                    Value::Int(surface),
                    Value::Int(3),
                    Value::Int(0),
                    Value::Int(0),
                ],
            )
        };
        // 122: the directrix (0,0,0)-(10,0,0), generator to (0,0,4): the
        // parameters (0.5, 0) and (0.5, 1) are (5,0,0) and (5,0,4).
        let tabulated = entity(
            122,
            0,
            vec![
                Value::Int(7),
                Value::Real(0.0),
                Value::Real(0.0),
                Value::Real(4.0),
            ],
        );
        // 120: the axis along z through the origin, the generatrix the
        // line (2,0,0)-(2,0,6) turned from 0 to a quarter turn: (0, 0) is
        // (2,0,0) and (1, pi/2) is (0,2,6).
        let revolved = entity(
            120,
            0,
            vec![
                Value::Int(9),
                Value::Int(7),
                Value::Real(0.0),
                Value::Real(core::f64::consts::FRAC_PI_2),
            ],
        );
        let cases = [
            (
                tabulated,
                line([0.0, 0.0, 0.0], [10.0, 0.0, 0.0]),
                [0.5, 0.0],
                [0.5, 1.0],
                Point::new(5.0, 0.0, 0.0),
                Point::new(5.0, 0.0, 4.0),
            ),
            (
                revolved,
                line([2.0, 0.0, 0.0], [2.0, 0.0, 6.0]),
                [0.0, 0.0],
                [1.0, core::f64::consts::FRAC_PI_2],
                Point::new(2.0, 0.0, 0.0),
                Point::new(0.0, 2.0, 6.0),
            ),
        ];
        for (surface, curve, from, to, at_from, at_to) in cases {
            let deck = file(vec![
                (1, surface),
                (3, line([from[0], from[1], 0.0], [to[0], to[1], 0.0])),
                (5, on_surface(1)),
                (7, curve),
                (9, line([0.0, 0.0, 0.0], [0.0, 0.0, 1.0])),
            ]);
            let mut reader = reader(&deck);
            let segments = reader.boundary_segments(5).unwrap();
            let [(lifted, (t0, t1))] = segments.as_slice() else {
                panic!("one segment");
            };
            let (a, b) = (
                lifted.point_at(*t0, T).unwrap(),
                lifted.point_at(*t1, T).unwrap(),
            );
            assert!(a.distance(at_from) < 1e-6, "{a:?} against {at_from:?}");
            assert!(b.distance(at_to) < 1e-6, "{b:?} against {at_to:?}");
        }
    }

    /// A linear dimension in model space draws its leaders and witness
    /// lines, is named by its note's text, and states the value that text
    /// reads; a note a drawing owns stays on the sheet.
    #[test]
    fn a_model_space_dimension_reads_as_its_callout_and_value() {
        let note = |text: &str| {
            let mut e = entity(
                212,
                0,
                vec![
                    Value::Int(1),
                    Value::Int(5),
                    Value::Real(10.0),
                    Value::Real(3.0),
                    Value::Int(1),
                    Value::Real(0.0),
                    Value::Real(0.0),
                    Value::Int(0),
                    Value::Int(0),
                    Value::Real(10.0),
                    Value::Real(12.0),
                    Value::Real(0.0),
                    Value::Text(text.to_owned()),
                ],
            );
            e.status = 10_000;
            e
        };
        let leader = |head: [f64; 2], tail: [f64; 2]| {
            let mut e = entity(
                214,
                1,
                reals(&[1.0, 1.0, 0.5, 0.0, head[0], head[1], tail[0], tail[1]]),
            );
            e.status = 10_000;
            e
        };
        let witness = |a: [f64; 2], b: [f64; 2]| {
            let mut e = entity(106, 40, reals(&[1.0, 2.0, 0.0, a[0], a[1], b[0], b[1]]));
            e.status = 10_000;
            e
        };
        let dimension = entity(
            216,
            0,
            vec![
                Value::Int(3),
                Value::Int(5),
                Value::Int(7),
                Value::Int(9),
                Value::Int(11),
            ],
        );
        let mut sheet_note = note("on the sheet");
        sheet_note.status = 0;
        let drawing = entity(404, 0, vec![Value::Int(0), Value::Int(1), Value::Int(13)]);
        let deck = file(vec![
            (1, dimension),
            (3, note("2X 25.40")),
            (5, leader([0.0, 11.0], [8.0, 11.0])),
            (7, leader([25.4, 11.0], [17.0, 11.0])),
            (9, witness([0.0, 0.0], [0.0, 12.0])),
            (11, witness([25.4, 0.0], [25.4, 12.0])),
            (13, sheet_note),
            (15, drawing),
        ]);
        let mut reader = reader(&deck);
        let (callouts, dimensions) = reader.annotations();
        assert!(callouts.is_empty(), "the sheet's note stays on the sheet");
        let [(callout, dimension)] = dimensions.as_slice() else {
            panic!("one dimension, found {}", dimensions.len());
        };
        assert_eq!(callout.name, "2X 25.40");
        assert_eq!(
            callout.polylines.len(),
            4,
            "two leaders and two witness lines"
        );
        assert!(
            (dimension.values[0] - 25.4).abs() < 1e-12,
            "{:?}",
            dimension.values
        );
        assert!(dimension.location);
        assert!(!reader.visited.contains_key(&13));
    }

    /// Two instances of one subfigure: a ten-square trimmed plane placed at
    /// x = 100, and at y = 50 twice the size. Each is the definition's face
    /// under its own placement, and the definition is built once.
    #[test]
    fn subfigure_instances_place_their_definition() {
        let dependent = |mut e: Entity| {
            e.status = 10_000;
            e
        };
        let square = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let mut entities = vec![(1, dependent(entity(108, 0, reals(&[0.0, 0.0, 1.0, 0.0]))))];
        let mut sides = Vec::new();
        for i in 0..4 {
            let (a, b) = (square[i], square[(i + 1) % 4]);
            let de = 3 + 2 * i64::try_from(i).unwrap();
            entities.push((de, dependent(line([a[0], a[1], 0.0], [b[0], b[1], 0.0]))));
            sides.push(Value::Int(de));
        }
        let mut composite = vec![Value::Int(4)];
        composite.extend(sides);
        entities.push((11, dependent(entity(102, 0, composite))));
        entities.push((
            13,
            dependent(entity(
                144,
                0,
                vec![Value::Int(1), Value::Int(1), Value::Int(0), Value::Int(11)],
            )),
        ));
        entities.push((
            15,
            entity(
                308,
                0,
                vec![
                    Value::Int(0),
                    Value::Text("tile".to_owned()),
                    Value::Int(1),
                    Value::Int(13),
                ],
            ),
        ));
        entities.push((
            17,
            entity(
                408,
                0,
                vec![
                    Value::Int(15),
                    Value::Real(100.0),
                    Value::Real(0.0),
                    Value::Real(0.0),
                    Value::Real(1.0),
                ],
            ),
        ));
        entities.push((
            19,
            entity(
                408,
                0,
                vec![
                    Value::Int(15),
                    Value::Real(0.0),
                    Value::Real(50.0),
                    Value::Real(0.0),
                    Value::Real(2.0),
                ],
            ),
        ));
        let deck = file(entities);
        let mut reader = reader(&deck);
        let (solids, sheets, _) = reader.subfigure_instances(T).unwrap();
        assert!(solids.is_empty());
        assert_eq!(sheets.len(), 2);
        let deflection = ogeom_mesh::Deflection::default();
        let mut measured: Vec<(f64, Point)> = sheets
            .iter()
            .map(|sheet| {
                let props =
                    ogeom_algo::surface_properties(&reader.model, sheet, deflection, T).unwrap();
                (props.mass, props.centre)
            })
            .collect();
        measured.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert!((measured[0].0 - 100.0).abs() < 1e-9, "{measured:?}");
        assert!(measured[0].1.distance(Point::new(105.0, 5.0, 0.0)) < 1e-9);
        assert!((measured[1].0 - 400.0).abs() < 1e-9, "{measured:?}");
        assert!(measured[1].1.distance(Point::new(10.0, 60.0, 0.0)) < 1e-9);
        // One definition, one face node under two placements.
        let face_of = |shape: &Shape| {
            ogeom_topo::explore(
                &reader.model,
                shape,
                ogeom_topo::Filter::OfType(ogeom_topo::ShapeType::Face),
            )
            .unwrap()
            .remove(0)
        };
        assert_eq!(face_of(&sheets[0]).node(), face_of(&sheets[1]).node());
    }

    /// Levels and groups are layers: each face on its level's layer, and a
    /// group's members on the group's own.
    #[test]
    fn levels_and_groups_read_as_layers() {
        let square = |z: f64, base: i64| -> Vec<(i64, Entity)> {
            let dependent = |mut e: Entity| {
                e.status = 10_000;
                e
            };
            let corners = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
            let mut out = vec![(base, dependent(entity(108, 0, reals(&[0.0, 0.0, 1.0, z]))))];
            let mut sides = vec![Value::Int(4)];
            for i in 0..4 {
                let (a, b) = (corners[i], corners[(i + 1) % 4]);
                let de = base + 2 + 2 * i64::try_from(i).unwrap();
                out.push((de, dependent(line([a[0], a[1], z], [b[0], b[1], z]))));
                sides.push(Value::Int(de));
            }
            out.push((base + 10, dependent(entity(102, 0, sides))));
            out
        };
        let mut entities = square(0.0, 1);
        entities.extend(square(5.0, 21));
        let mut low = entity(
            144,
            0,
            vec![Value::Int(1), Value::Int(1), Value::Int(0), Value::Int(11)],
        );
        low.level = 3;
        let mut high = entity(
            144,
            0,
            vec![Value::Int(21), Value::Int(1), Value::Int(0), Value::Int(31)],
        );
        high.level = 7;
        entities.push((41, low));
        entities.push((43, high));
        let mut group = entity(402, 7, vec![Value::Int(1), Value::Int(43)]);
        group.label = "LID".to_owned();
        entities.push((45, group));
        let deck = file(entities);
        let mut reader = reader(&deck);
        let low_face = reader.face(41).unwrap();
        let high_face = reader.face(43).unwrap();
        let built = vec![(41, low_face.clone()), (43, high_face.clone())];
        let mut document = ogeom_doc::Document::over(std::mem::take(&mut reader.model));
        let mut report = IgesReport::default();
        layers(&deck, &mut document, &built, &mut report);
        let names_of = |shape: &Shape| -> Vec<String> {
            let mut names: Vec<String> = document
                .layers_of(shape)
                .iter()
                .map(|id| document.layer(*id).unwrap().name.clone())
                .collect();
            names.sort();
            names
        };
        assert_eq!(names_of(&low_face), vec!["level 3".to_owned()]);
        assert_eq!(
            names_of(&high_face),
            vec!["LID".to_owned(), "level 7".to_owned()]
        );
    }

    /// Constructive solids against their closed forms: a block drilled by a
    /// cylinder through a boolean tree, a wedge, a cone frustum, an
    /// ellipsoid, a half-turn of revolution, an extruded square, and an
    /// assembly placing one block twice.
    #[test]
    fn constructive_solids_read_as_their_volumes() {
        let pi = core::f64::consts::PI;
        let square = |base: i64, corners: [[f64; 3]; 4]| -> Vec<(i64, Entity)> {
            let mut out = Vec::new();
            let mut sides = vec![Value::Int(4)];
            for i in 0..4 {
                let de = base + 2 * i64::try_from(i).unwrap();
                out.push((de, line(corners[i], corners[(i + 1) % 4])));
                sides.push(Value::Int(de));
            }
            out.push((base + 8, entity(102, 0, sides)));
            out
        };
        let mut entities = vec![
            // Block 20³ at the origin.
            (
                1,
                entity(
                    150,
                    0,
                    reals(&[
                        20.0, 20.0, 20.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                    ]),
                ),
            ),
            // Cylinder r 5, h 40, up the block's middle from below.
            (
                3,
                entity(
                    154,
                    0,
                    reals(&[40.0, 5.0, 10.0, 10.0, -10.0, 0.0, 0.0, 1.0]),
                ),
            ),
            (
                5,
                entity(
                    180,
                    0,
                    vec![Value::Int(3), Value::Int(-1), Value::Int(-3), Value::Int(3)],
                ),
            ),
            // Wedge 10 × 5 × 4, narrowing to 6 along x at y = 5.
            (
                7,
                entity(
                    152,
                    0,
                    reals(&[
                        10.0, 5.0, 4.0, 6.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                    ]),
                ),
            ),
            // Frustum h 10 from r 5 to r 2.
            (
                9,
                entity(
                    156,
                    0,
                    reals(&[10.0, 5.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
                ),
            ),
            // Ellipsoid 3 × 2 × 1.
            (
                11,
                entity(
                    168,
                    0,
                    reals(&[3.0, 2.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
                ),
            ),
            // Half a turn of the rectangle x 2..4, z 0..3 about z.
            (
                13,
                entity(
                    162,
                    0,
                    vec![
                        Value::Int(29),
                        Value::Real(0.5),
                        Value::Real(0.0),
                        Value::Real(0.0),
                        Value::Real(0.0),
                        Value::Real(0.0),
                        Value::Real(0.0),
                        Value::Real(1.0),
                    ],
                ),
            ),
            // The square 0..2 in xy extruded 5 up z.
            (
                15,
                entity(
                    164,
                    0,
                    vec![
                        Value::Int(49),
                        Value::Real(5.0),
                        Value::Real(0.0),
                        Value::Real(0.0),
                        Value::Real(1.0),
                    ],
                ),
            ),
            // The block twice: as it stands, and moved 100 along x.
            (
                17,
                entity(
                    124,
                    0,
                    reals(&[1.0, 0.0, 0.0, 100.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                ),
            ),
            (
                19,
                entity(
                    184,
                    0,
                    vec![
                        Value::Int(2),
                        Value::Int(1),
                        Value::Int(1),
                        Value::Int(0),
                        Value::Int(17),
                    ],
                ),
            ),
        ];
        entities.extend(square(
            21,
            [
                [2.0, 0.0, 0.0],
                [4.0, 0.0, 0.0],
                [4.0, 0.0, 3.0],
                [2.0, 0.0, 3.0],
            ],
        ));
        entities.extend(square(
            41,
            [
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 2.0, 0.0],
                [0.0, 2.0, 0.0],
            ],
        ));
        let deck = file(entities);
        let mut reader = reader(&deck);
        let volume = |reader: &Reader<'_>, shape: &Shape| {
            ogeom_algo::volume_properties(
                &reader.model,
                shape,
                ogeom_mesh::Deflection::with_chord(1e-3).unwrap(),
                T,
            )
            .unwrap()
            .mass
        };
        for (de, want, within) in [
            (5, 8000.0 - pi * 25.0 * 20.0, 1e-6),
            (7, (10.0 + 6.0) / 2.0 * 5.0 * 4.0, 1e-9),
            (9, pi * 10.0 / 3.0 * (25.0 + 10.0 + 4.0), 1e-6),
            // Exact on its equation (below); the mesh measuring the volume
            // falls short by the chord over the area, a part in a thousand.
            (11, 4.0 / 3.0 * pi * 6.0, 2e-3),
            (13, 0.5 * pi * (16.0 - 4.0) * 3.0, 1e-6),
            (15, 20.0, 1e-9),
        ] {
            let solids = reader.csg_solids(de).unwrap();
            let [solid] = solids.as_slice() else {
                panic!("D{de} is one solid");
            };
            let got = volume(&reader, solid);
            assert!(
                (got - want).abs() <= within * want.max(1.0),
                "D{de}: {got} against {want}"
            );
        }
        // The ellipsoid's surface on its own equation, at points spread
        // over every face.
        let ellipsoid = reader.csg_solids(11).unwrap().remove(0);
        for face in ogeom_topo::explore(
            &reader.model,
            &ellipsoid,
            ogeom_topo::Filter::OfType(ogeom_topo::ShapeType::Face),
        )
        .unwrap()
        {
            let data = reader
                .model
                .node(&face)
                .unwrap()
                .data()
                .as_face()
                .unwrap()
                .clone();
            let surface = reader
                .model
                .geometry()
                .surface(data.surface)
                .unwrap()
                .clone();
            let ((u0, u1), (v0, v1)) = ogeom_geom::Surface::domain(&surface);
            for i in 0..=6 {
                for j in 0..=6 {
                    let (u, v) = (
                        u0 + (u1 - u0) * f64::from(i) / 6.0,
                        v0 + (v1 - v0) * f64::from(j) / 6.0,
                    );
                    let p = ogeom_geom::Surface::point_at(&surface, u, v, T).unwrap();
                    let residual = p.x * p.x / 9.0 + p.y * p.y / 4.0 + p.z * p.z - 1.0;
                    assert!(residual.abs() < 1e-9, "off the ellipsoid: {p:?}");
                }
            }
        }
        let placed = reader.csg_solids(19).unwrap();
        assert_eq!(placed.len(), 2);
        let centres: Vec<Point> = placed
            .iter()
            .map(|s| {
                ogeom_algo::volume_properties(
                    &reader.model,
                    s,
                    ogeom_mesh::Deflection::default(),
                    T,
                )
                .unwrap()
                .centre
            })
            .collect();
        assert!(
            centres
                .iter()
                .any(|c| c.distance(Point::new(10.0, 10.0, 10.0)) < 1e-9)
        );
        assert!(
            centres
                .iter()
                .any(|c| c.distance(Point::new(110.0, 10.0, 10.0)) < 1e-9)
        );
    }
}
