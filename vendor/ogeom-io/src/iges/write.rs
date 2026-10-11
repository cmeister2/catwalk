//! From a document to an IGES deck.
//!
//! The writer emits manifold solid B-rep objects (entity 186 over shells
//! (514), faces (510), loops (508), one edge list (504) and one vertex list
//! (502) per solid), and sheets (shells no solid owns, faces no shell owns)
//! as trimmed surfaces (144), one per face, each bounded by curves on the
//! surface (142) over its model-space edges. Surfaces go out in their
//! analytic spellings where IGES has one and as rational B-splines (128)
//! where it does not. Curves defined in a
//! plane of their own, arcs and ellipses, are written in definition space
//! with a transformation matrix (124) carrying them to model space, which is
//! how the format wants them.
//!
//! Everything is written in millimetres, model space, with geometry baked:
//! the same decision the STEP writer made: a file carries positions, not this
//! kernel's location chains.

use ogeom_core::FastMap;
use ogeom_core::{OgeomResult, Tolerances, ogeom_bail};
use ogeom_geom::Curve3d as _;
use ogeom_geom::Reversible as _;
use ogeom_geom::Surface as _;
use ogeom_geom::Transformable as _;
use ogeom_geom::{BSplineCurve, Curve, SurfaceGeometry};
use ogeom_math::{Frame, Point, Transform};
use ogeom_topo::{EdgeRepr, Filter, NodeData, Shape, ShapeType, explore};

/// Write a document's solids and sheets as an IGES file.
///
/// A solid becomes a manifold solid B-rep object (186). A sheet's faces
/// become independent trimmed surfaces (144), which a reader sews back into
/// the sheet they were; a sheet that closes reads back as a solid.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) if the
/// document holds no part, a part holds wireframe (a wire, edge or vertex
/// outside every face) or nothing at all, or a body carries geometry with
/// no IGES spelling here; the error names the entity and the parity row.
pub fn write_iges(document: &ogeom_doc::Document, tol: Tolerances) -> OgeomResult<String> {
    let mut writer = Writer {
        model: document.model(),
        entities: Vec::new(),
        vertices: FastMap::default(),
        edges: FastMap::default(),
        vertex_coords: Vec::new(),
        edge_records: Vec::new(),
        tol,
    };

    let mut parts = Vec::new();
    for (_, product) in document.products() {
        let ogeom_doc::ProductKind::Part { shape } = &product.kind else {
            continue;
        };
        let bodies = crate::bodies::bodies_of(writer.model, shape, "IGES")?;
        parts.push((product.name.clone(), bodies));
    }
    if parts.is_empty() {
        ogeom_bail!(Construction, "the document holds no part to write as IGES");
    }
    for (label, bodies) in parts {
        for solid in &bodies.solids {
            writer.solid(solid, &label)?;
        }
        for shell in &bodies.shells {
            for face in writer.model.ordered_children_of(shell)? {
                writer.trimmed_surface(&face, &label)?;
            }
        }
        for face in &bodies.faces {
            writer.trimmed_surface(face, &label)?;
        }
    }
    Ok(writer.serialize())
}

/// One pending entity: directory fields and parameter text.
struct Pending {
    kind: i64,
    form: i64,
    /// Index into `entities` of a 124 transform, or `None`.
    transform: Option<usize>,
    /// Independent (top-level) or physically subordinate.
    independent: bool,
    label: String,
    params: String,
}

struct Writer<'a> {
    model: &'a ogeom_topo::Model,
    entities: Vec<Pending>,
    /// Vertex index (1-based) in the current solid's 502, keyed by node
    /// *and position*: an instanced node placed twice is two vertices in the
    /// file, and keying by node alone would weld a prism's top to its bottom.
    vertices: FastMap<(ogeom_topo::TShapeId, [u64; 3]), usize>,
    /// Edge index (1-based) in the current solid's 504, keyed by node and
    /// placement for the same reason.
    edges: FastMap<(ogeom_topo::TShapeId, [u64; 3]), usize>,
    /// The current solid's vertex coordinates, in 502 order.
    vertex_coords: Vec<Point>,
    /// The current solid's edge records: (curve entity, start index, end index).
    edge_records: Vec<(usize, usize, usize)>,
    tol: Tolerances,
}

impl Writer<'_> {
    fn push(&mut self, p: Pending) -> usize {
        self.entities.push(p);
        self.entities.len() - 1
    }

    /// The directory pointer an entity index will serialize as.
    #[allow(clippy::cast_possible_wrap, reason = "entity counts are small")]
    fn de(&self, index: usize) -> i64 {
        2 * index as i64 + 1
    }

    fn solid(&mut self, solid: &Shape, label: &str) -> OgeomResult<()> {
        // Fresh per-solid lists; their entities are created after the faces
        // so their contents are complete, and patched into the loops by a
        // placeholder scheme below.
        self.vertices.clear();
        self.edges.clear();
        self.vertex_coords.clear();
        self.edge_records.clear();

        // The first shell is the outer boundary and each further one bounds
        // a cavity. A void is written as the standard names it, a shell
        // facing out of the cavity used the other way round, which leaves
        // every face of the solid facing away from the material.
        let shells = explore(self.model, solid, Filter::OfType(ShapeType::Shell))?;
        if shells.is_empty() {
            ogeom_bail!(Construction, "a solid with no shell cannot be written");
        }
        let mut shell_faces = Vec::with_capacity(shells.len());
        for (k, shell) in shells.iter().enumerate() {
            let mut face_entities = Vec::new();
            for face in self.model.ordered_children_of(shell)? {
                let face = if k == 0 { face } else { face.reversed() };
                face_entities.push((self.face(&face)?, face.orientation()));
            }
            shell_faces.push(face_entities);
        }

        // The lists exist in full.
        let vertex_list = {
            let mut params = format!("{}", self.vertex_coords.len());
            for p in &self.vertex_coords {
                params.push_str(&format!(",{},{},{}", fmt(p.x), fmt(p.y), fmt(p.z)));
            }
            self.push(Pending {
                kind: 502,
                form: 1,
                transform: None,
                independent: false,
                label: String::new(),
                params,
            })
        };
        let edge_list = {
            let records = std::mem::take(&mut self.edge_records);
            let mut params = format!("{}", records.len());
            let mut curve_des = Vec::new();
            for (curve_entity, sv, tv) in &records {
                curve_des.push(self.de(*curve_entity));
                params.push_str(&format!(",{},@V,{sv},@V,{tv}", self.de(*curve_entity)));
            }
            self.push(Pending {
                kind: 504,
                form: 1,
                transform: None,
                independent: false,
                label: String::new(),
                params,
            })
        };
        // Loops referred to the lists before the lists existed; the
        // placeholders resolve here.
        let vlist_de = self.de(vertex_list);
        let elist_de = self.de(edge_list);
        for e in &mut self.entities {
            if e.kind == 508 || e.kind == 504 {
                e.params = e.params.replace("@E", &elist_de.to_string());
                e.params = e.params.replace("@V", &vlist_de.to_string());
            }
        }

        let mut shell_des = Vec::with_capacity(shell_faces.len());
        for face_entities in &shell_faces {
            let mut params = format!("{}", face_entities.len());
            for (face, orientation) in face_entities {
                let flag = i32::from(*orientation != ogeom_topo::Orientation::Reversed);
                params.push_str(&format!(",{},{flag}", self.de(*face)));
            }
            let shell_entity = self.push(Pending {
                kind: 514,
                form: 1,
                transform: None,
                independent: false,
                label: String::new(),
                params,
            });
            shell_des.push(self.de(shell_entity));
        }
        let mut params = format!("{},1,{}", shell_des[0], shell_des.len() - 1);
        for void in &shell_des[1..] {
            params.push_str(&format!(",{void},0"));
        }
        self.push(Pending {
            kind: 186,
            form: 0,
            transform: None,
            independent: true,
            label: label.chars().take(8).collect(),
            params,
        });
        Ok(())
    }

    fn face(&mut self, face: &Shape) -> OgeomResult<usize> {
        let placement = face.transform(self.model.datums())?;
        let surface = {
            let Some(node) = self.model.node(face) else {
                ogeom_bail!(Dangling, "face is not in this model");
            };
            let NodeData::Face(data) = node.data() else {
                ogeom_bail!(Construction, "face node holds no face data");
            };
            let Some(surface) = self.model.geometry().surface(data.surface) else {
                ogeom_bail!(Dangling, "face refers to a surface not in this model");
            };
            surface.clone().transformed(&placement, self.tol)?
        };
        let surface_entity = self.surface(&surface)?;
        // A face's loops keep it on their left about its surface's normal,
        // as the face stores them; the shell's flag says which side of the
        // surface the face faces.
        let wires = self
            .model
            .ordered_children_of(&face.oriented(ogeom_topo::Orientation::Forward))?;
        let mut loop_entities = Vec::new();
        for wire in &wires {
            if let Some(entity) = self.wire(wire)? {
                loop_entities.push(entity);
            }
        }
        if loop_entities.is_empty() {
            ogeom_bail!(
                Construction,
                "a face whose every boundary is degenerate cannot be written \
                 as IGES; see docs/PARITY.md, io.iges"
            );
        }
        let mut params = format!("{},{},1", self.de(surface_entity), loop_entities.len());
        for entity in &loop_entities {
            params.push_str(&format!(",{}", self.de(*entity)));
        }
        Ok(self.push(Pending {
            kind: 510,
            form: 1,
            transform: None,
            independent: false,
            label: String::new(),
            params,
        }))
    }

    /// A face as an independent trimmed surface (144): its surface, and one
    /// curve on the surface (142) per boundary over the model-space curves
    /// of the boundary's edges.
    ///
    /// The format gives a trimmed surface no sense of its own: its normal is
    /// its surface's. A reversed face therefore goes out on its surface
    /// turned over, a plane with its normal negated and anything else as its
    /// exact B-spline patch with the two parameter directions exchanged.
    fn trimmed_surface(&mut self, face: &Shape, label: &str) -> OgeomResult<()> {
        let placement = face.transform(self.model.datums())?;
        let surface = {
            let Some(node) = self.model.node(face) else {
                ogeom_bail!(Dangling, "face is not in this model");
            };
            let NodeData::Face(data) = node.data() else {
                ogeom_bail!(Construction, "face node holds no face data");
            };
            let Some(surface) = self.model.geometry().surface(data.surface) else {
                ogeom_bail!(Dangling, "face refers to a surface not in this model");
            };
            surface.clone().transformed(&placement, self.tol)?
        };
        let surface_entity = if face.orientation() == ogeom_topo::Orientation::Reversed {
            self.turned_surface(&surface)?
        } else {
            self.surface(&surface)?
        };
        // Stored order, the outer boundary first; the walk of a reversed
        // face lists its holes first.
        let mut boundaries = Vec::new();
        for wire in self.model.children_of(face)? {
            if let Some(curve) = self.boundary_curve(&wire)? {
                // Unspecified creation, no parameter-space curve, the
                // model-space curve preferred.
                let params = format!("0,{},0,{},2", self.de(surface_entity), self.de(curve));
                boundaries.push(self.push(Pending {
                    kind: 142,
                    form: 0,
                    transform: None,
                    independent: false,
                    label: String::new(),
                    params,
                }));
            }
        }
        let Some((outer, inner)) = boundaries.split_first() else {
            ogeom_bail!(
                Construction,
                "a face whose every boundary is degenerate cannot be written \
                 as IGES; see docs/PARITY.md, io.iges"
            );
        };
        let mut params = format!(
            "{},1,{},{}",
            self.de(surface_entity),
            inner.len(),
            self.de(*outer)
        );
        for boundary in inner {
            params.push_str(&format!(",{}", self.de(*boundary)));
        }
        self.push(Pending {
            kind: 144,
            form: 0,
            transform: None,
            independent: true,
            label: label.chars().take(8).collect(),
            params,
        });
        Ok(())
    }

    /// A wire's edges as one model-space curve: the edge's own curve when
    /// there is one, a composite curve (102) of them when there are more,
    /// `None` when every edge is degenerate. Each piece runs the way the wire
    /// walks it, in the wire's order: the composite curve has no sense flag
    /// for its pieces, so an edge the wire uses reversed goes out as its
    /// curve reversed.
    fn boundary_curve(&mut self, wire: &Shape) -> OgeomResult<Option<usize>> {
        let mut pieces = Vec::new();
        for edge in self.model.ordered_children_of(wire)? {
            let forward = edge.orientation() != ogeom_topo::Orientation::Reversed;
            let (curve, range) = {
                let Some(data) = self.model.node(&edge).and_then(|n| n.data().as_edge()) else {
                    ogeom_bail!(Construction, "edge node holds no edge data");
                };
                if data.degenerate {
                    continue;
                }
                let Some(EdgeRepr::Curve3d { curve, range, .. }) = data.curve3d() else {
                    ogeom_bail!(Construction, "an edge with no curve cannot be written");
                };
                let Some(geometry) = self.model.geometry().curve(*curve) else {
                    ogeom_bail!(Dangling, "curve is not in this model");
                };
                let placement = edge.transform(self.model.datums())?;
                (geometry.clone().transformed(&placement, self.tol)?, *range)
            };
            pieces.push(self.curve(&curve, range, forward)?);
        }
        match pieces.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(*only)),
            _ => {
                let mut params = format!("{}", pieces.len());
                for piece in &pieces {
                    params.push_str(&format!(",{}", self.de(*piece)));
                }
                Ok(Some(self.push(Pending {
                    kind: 102,
                    form: 0,
                    transform: None,
                    independent: false,
                    label: String::new(),
                    params,
                })))
            }
        }
    }

    /// A surface with its normal turned over.
    fn turned_surface(&mut self, surface: &SurfaceGeometry) -> OgeomResult<usize> {
        match surface {
            SurfaceGeometry::Plane(p) => {
                let frame = p.plane().frame();
                let point = self.point_entity(frame.origin());
                let normal = self.direction_entity(-frame.z());
                let params = format!("{},{}", self.de(point), self.de(normal));
                Ok(self.push(Pending {
                    kind: 190,
                    form: 0,
                    transform: None,
                    independent: false,
                    label: String::new(),
                    params,
                }))
            }
            SurfaceGeometry::BSpline(b) => Ok(self.nurbs_surface(b, true)),
            other => {
                let bspline = other.to_bspline(self.tol)?;
                Ok(self.nurbs_surface(&bspline, true))
            }
        }
    }

    /// A wire as a loop (508), or `None` when every edge in it is degenerate:
    /// a pole has no curve, and the reader rebuilds chart degeneracies
    /// from the surface itself.
    fn wire(&mut self, wire: &Shape) -> OgeomResult<Option<usize>> {
        let children = self.model.ordered_children_of(wire)?;
        let degenerate = |edge: &Shape| {
            self.model
                .node(edge)
                .and_then(|n| n.data().as_edge())
                .is_some_and(|d| d.degenerate)
        };
        let live: Vec<&Shape> = children.iter().filter(|e| !degenerate(e)).collect();
        if live.is_empty() {
            return Ok(None);
        }
        let mut entries = Vec::new();
        for edge in live {
            let index = self.edge(edge)?;
            let flag = i32::from(edge.orientation() != ogeom_topo::Orientation::Reversed);
            entries.push(format!("0,@E,{index},{flag},0"));
        }
        let params = format!("{},{}", entries.len(), entries.join(","));
        Ok(Some(self.push(Pending {
            kind: 508,
            form: 1,
            transform: None,
            independent: false,
            label: String::new(),
            params,
        })))
    }

    /// The edge's index in the current solid's 504 list, creating it once
    /// per placement.
    fn edge(&mut self, edge: &Shape) -> OgeomResult<usize> {
        let placement = edge.transform(self.model.datums())?;
        let key = (edge.node(), transform_bits(&placement));
        if let Some(&index) = self.edges.get(&key) {
            return Ok(index);
        }
        let (curve, range) = {
            let Some(data) = self.model.node(edge).and_then(|n| n.data().as_edge()) else {
                ogeom_bail!(Construction, "edge node holds no edge data");
            };
            let Some(EdgeRepr::Curve3d { curve, range, .. }) = data.curve3d() else {
                ogeom_bail!(Construction, "an edge with no curve cannot be written");
            };
            let Some(geometry) = self.model.geometry().curve(*curve) else {
                ogeom_bail!(Dangling, "curve is not in this model");
            };
            (geometry.clone().transformed(&placement, self.tol)?, *range)
        };
        let curve_entity = self.curve(&curve, range, true)?;
        let vertices = self.model.children_of(edge)?;
        let (from, to) = match vertices.len() {
            0 => ogeom_bail!(Construction, "an edge with no vertices cannot be written"),
            1 => (vertices[0].clone(), vertices[0].clone()),
            _ => (vertices[0].clone(), vertices[vertices.len() - 1].clone()),
        };
        // Each vertex carries its own composed placement; children_of has
        // already folded the edge's chain in, and an instanced vertex (a
        // prism's top corner is its bottom corner, moved) adds a hop of its
        // own that the edge's placement alone would drop.
        let sv = {
            let at = from.transform(self.model.datums())?;
            self.vertex(&from, &at)?
        };
        let tv = {
            let at = to.transform(self.model.datums())?;
            self.vertex(&to, &at)?
        };
        self.edge_records.push((curve_entity, sv, tv));
        let index = self.edge_records.len();
        self.edges.insert(key, index);
        Ok(index)
    }

    /// The vertex's index in the current solid's 502 list, creating it once.
    fn vertex(&mut self, vertex: &Shape, placement: &Transform) -> OgeomResult<usize> {
        let Some(data) = self.model.node(vertex).and_then(|n| n.data().as_vertex()) else {
            ogeom_bail!(Construction, "vertex node holds no vertex data");
        };
        let at = placement.apply(data.point);
        let key = (
            vertex.node(),
            [at.x.to_bits(), at.y.to_bits(), at.z.to_bits()],
        );
        if let Some(&index) = self.vertices.get(&key) {
            return Ok(index);
        }
        self.vertex_coords.push(at);
        let index = self.vertex_coords.len();
        self.vertices.insert(key, index);
        Ok(index)
    }

    /// A curve over a range, in its IGES spelling, running from `range.0`
    /// to `range.1` when `forward` and the other way when not.
    fn curve(&mut self, curve: &Curve, range: (f64, f64), forward: bool) -> OgeomResult<usize> {
        // The angle on the underlying conic at each end of the walk.
        let walk = |reversed: bool| {
            let angle = |u: f64| if reversed { -u } else { u };
            let (s, e) = if forward { range } else { (range.1, range.0) };
            // A conic arc runs counter-clockwise about its frame's `z`; one
            // walked clockwise goes out about the frame turned over `x`,
            // where each angle negates.
            let counter_clockwise = reversed != forward;
            let sign = if counter_clockwise { 1.0 } else { -1.0 };
            (sign * angle(s), sign * angle(e), counter_clockwise)
        };
        match curve {
            Curve::Line(line) => {
                let (from, to) = if forward { range } else { (range.1, range.0) };
                let a = line.point_at(from, self.tol)?;
                let b = line.point_at(to, self.tol)?;
                Ok(self.push(Pending {
                    kind: 110,
                    form: 0,
                    transform: None,
                    independent: false,
                    label: String::new(),
                    params: format!(
                        "{},{},{},{},{},{}",
                        fmt(a.x),
                        fmt(a.y),
                        fmt(a.z),
                        fmt(b.x),
                        fmt(b.y),
                        fmt(b.z)
                    ),
                }))
            }
            Curve::Circle(c) => {
                let circle = c.circle();
                let r = circle.radius();
                let (s, e, counter_clockwise) = walk(c.is_reversed());
                let frame = if counter_clockwise {
                    circle.frame()
                } else {
                    circle.frame().with_z_reversed()
                };
                let transform = self.transform_entity(&frame);
                let params = format!(
                    "0.,0.,0.,{},{},{},{}",
                    fmt(r * s.cos()),
                    fmt(r * s.sin()),
                    fmt(r * e.cos()),
                    fmt(r * e.sin()),
                );
                Ok(self.push(Pending {
                    kind: 100,
                    form: 0,
                    transform: Some(transform),
                    independent: false,
                    label: String::new(),
                    params,
                }))
            }
            Curve::Ellipse(el) => {
                let ellipse = el.ellipse();
                let (a, b) = (ellipse.major_radius(), ellipse.minor_radius());
                let (s, e, counter_clockwise) = walk(el.is_reversed());
                let frame = if counter_clockwise {
                    ellipse.frame()
                } else {
                    ellipse.frame().with_z_reversed()
                };
                let transform = self.transform_entity(&frame);
                let at = |t: f64| (a * t.cos(), b * t.sin());
                let (sx, sy) = at(s);
                let (ex, ey) = at(e);
                // x²/a² + y²/b² − 1 = 0, spelt in the general coefficients.
                let params = format!(
                    "{},0.,{},0.,0.,-1.,0.,{},{},{},{}",
                    fmt(1.0 / (a * a)),
                    fmt(1.0 / (b * b)),
                    fmt(sx),
                    fmt(sy),
                    fmt(ex),
                    fmt(ey),
                );
                Ok(self.push(Pending {
                    kind: 104,
                    form: 1,
                    transform: Some(transform),
                    independent: false,
                    label: String::new(),
                    params,
                }))
            }
            // A reversed trim runs its basis backwards, each parameter
            // mirrored in its own domain.
            Curve::Trimmed(t) if t.is_reversed() => {
                let (lo, hi) = ogeom_geom::Curve3d::domain(curve);
                self.curve(t.basis(), (lo + hi - range.1, lo + hi - range.0), !forward)
            }
            Curve::Trimmed(t) => self.curve(t.basis(), range, forward),
            Curve::BSpline(b) if forward => self.nurbs_curve(b, range),
            Curve::BSpline(b) => {
                // Reversal keeps the domain and mirrors each parameter in it.
                let (lo, hi) = ogeom_geom::Curve3d::domain(b);
                let back = reversed_bspline(b)?;
                self.nurbs_curve(&back, (lo + hi - range.1, lo + hi - range.0))
            }
            other => {
                // The exact conversion carries anything with a closed NURBS
                // form; what has none (a helix) is refused by name there.
                let mut bspline = other.to_bspline_over(range, self.tol)?;
                if !forward {
                    bspline = reversed_bspline(&bspline)?;
                }
                self.nurbs_curve(&bspline, ogeom_geom::Curve3d::domain(&bspline))
            }
        }
    }

    fn nurbs_curve(&mut self, curve: &BSplineCurve, range: (f64, f64)) -> OgeomResult<usize> {
        let knots = curve.knots();
        let control = curve.control_points();
        let degree = knots.degree();
        let k = control.len() - 1;
        let rational = curve.is_rational();
        let closed = {
            let (lo, hi) = ogeom_geom::Curve3d::domain(curve);
            let a = curve.point_at(lo, self.tol)?;
            let b = curve.point_at(hi, self.tol)?;
            i32::from(a.distance(b) < self.tol.confusion())
        };
        let mut params = format!("{k},{degree},0,{closed},{},0", i32::from(!rational));
        for t in knots.knots() {
            params.push_str(&format!(",{}", fmt(*t)));
        }
        for w in control {
            params.push_str(&format!(",{}", fmt(w.weight)));
        }
        for w in control {
            let p = (*w).point();
            params.push_str(&format!(",{},{},{}", fmt(p.x), fmt(p.y), fmt(p.z)));
        }
        params.push_str(&format!(",{},{}", fmt(range.0), fmt(range.1)));
        Ok(self.push(Pending {
            kind: 126,
            form: 0,
            transform: None,
            independent: false,
            label: String::new(),
            params,
        }))
    }

    /// A definition-space→model transform (124) for a frame.
    fn transform_entity(&mut self, frame: &Frame) -> usize {
        let (x, y, z) = (frame.x().vector(), frame.y().vector(), frame.z().vector());
        let o = frame.origin();
        let params = format!(
            "{},{},{},{},{},{},{},{},{},{},{},{}",
            fmt(x.x),
            fmt(y.x),
            fmt(z.x),
            fmt(o.x),
            fmt(x.y),
            fmt(y.y),
            fmt(z.y),
            fmt(o.y),
            fmt(x.z),
            fmt(y.z),
            fmt(z.z),
            fmt(o.z),
        );
        self.push(Pending {
            kind: 124,
            form: 0,
            transform: None,
            independent: false,
            label: String::new(),
            params,
        })
    }

    /// A point entity (116).
    fn point_entity(&mut self, p: Point) -> usize {
        self.push(Pending {
            kind: 116,
            form: 0,
            transform: None,
            independent: false,
            label: String::new(),
            params: format!("{},{},{},0", fmt(p.x), fmt(p.y), fmt(p.z)),
        })
    }

    /// A direction entity (123).
    fn direction_entity(&mut self, d: ogeom_math::Direction) -> usize {
        let v = d.vector();
        self.push(Pending {
            kind: 123,
            form: 0,
            transform: None,
            independent: false,
            label: String::new(),
            params: format!("{},{},{}", fmt(v.x), fmt(v.y), fmt(v.z)),
        })
    }

    /// A surface in its IGES spelling.
    fn surface(&mut self, surface: &SurfaceGeometry) -> OgeomResult<usize> {
        use SurfaceGeometry as S;
        match surface {
            S::Plane(p) => {
                let frame = p.plane().frame();
                let point = self.point_entity(frame.origin());
                let normal = self.direction_entity(frame.z());
                let params = format!("{},{}", self.de(point), self.de(normal));
                Ok(self.push(Pending {
                    kind: 190,
                    form: 0,
                    transform: None,
                    independent: false,
                    label: String::new(),
                    params,
                }))
            }
            S::Cylinder(c) => {
                let cyl = c.cylinder();
                let frame = cyl.frame();
                let point = self.point_entity(frame.origin());
                let axis = self.direction_entity(frame.z());
                let params = format!("{},{},{}", self.de(point), self.de(axis), fmt(cyl.radius()));
                Ok(self.push(Pending {
                    kind: 192,
                    form: 0,
                    transform: None,
                    independent: false,
                    label: String::new(),
                    params,
                }))
            }
            S::Cone(c) => {
                let cone = c.cone();
                let frame = cone.frame();
                let point = self.point_entity(frame.origin());
                let axis = self.direction_entity(frame.z());
                let params = format!(
                    "{},{},{},{}",
                    self.de(point),
                    self.de(axis),
                    fmt(cone.reference_radius()),
                    fmt(cone.half_angle().to_degrees()),
                );
                Ok(self.push(Pending {
                    kind: 194,
                    form: 0,
                    transform: None,
                    independent: false,
                    label: String::new(),
                    params,
                }))
            }
            S::Sphere(s) => {
                let sphere = s.sphere();
                let point = self.point_entity(sphere.centre());
                let params = format!("{},{}", self.de(point), fmt(sphere.radius()));
                Ok(self.push(Pending {
                    kind: 196,
                    form: 0,
                    transform: None,
                    independent: false,
                    label: String::new(),
                    params,
                }))
            }
            S::Torus(t) => {
                let torus = t.torus();
                let frame = torus.frame();
                let point = self.point_entity(frame.origin());
                let axis = self.direction_entity(frame.z());
                let params = format!(
                    "{},{},{},{}",
                    self.de(point),
                    self.de(axis),
                    fmt(torus.major_radius()),
                    fmt(torus.minor_radius()),
                );
                Ok(self.push(Pending {
                    kind: 198,
                    form: 0,
                    transform: None,
                    independent: false,
                    label: String::new(),
                    params,
                }))
            }
            S::BSpline(b) => Ok(self.nurbs_surface(b, false)),
            other => {
                // Swept, offset and trimmed carriers convert exactly where a
                // closed NURBS form exists; the conversion refuses by name
                // where it does not.
                let bspline = other.to_bspline(self.tol)?;
                self.surface(&SurfaceGeometry::BSpline(bspline))
            }
        }
    }

    /// A B-spline surface (128). `exchanged` writes it with its two
    /// parameter directions swapped: the same points, the normal turned over.
    fn nurbs_surface(&mut self, b: &ogeom_geom::BSplineSurface, exchanged: bool) -> usize {
        let (mut uk, mut vk) = (b.u_knots(), b.v_knots());
        let grid = b.grid();
        let (mut nu, mut nv) = (grid.u_count(), grid.v_count());
        let ((mut u0, mut u1), (mut v0, mut v1)) = b.domain();
        if exchanged {
            std::mem::swap(&mut uk, &mut vk);
            std::mem::swap(&mut nu, &mut nv);
            std::mem::swap(&mut u0, &mut v0);
            std::mem::swap(&mut u1, &mut v1);
        }
        // The control net at the file's (u, v), whichever way it is written.
        let at = |u: usize, v: usize| {
            if exchanged {
                grid.get(v, u)
            } else {
                grid.get(u, v)
            }
        };
        let (k1, k2) = (nu - 1, nv - 1);
        let (m1, m2) = (uk.degree(), vk.degree());
        let mut params = format!(
            "{k1},{k2},{m1},{m2},0,0,{},0,0",
            i32::from(!b.is_rational())
        );
        for t in uk.knots() {
            params.push_str(&format!(",{}", fmt(*t)));
        }
        for t in vk.knots() {
            params.push_str(&format!(",{}", fmt(*t)));
        }
        // The file wants the first (u) index varying fastest.
        for v in 0..nv {
            for u in 0..nu {
                let w = at(u, v).map_or(1.0, |w| w.weight);
                params.push_str(&format!(",{}", fmt(w)));
            }
        }
        for v in 0..nv {
            for u in 0..nu {
                let p = at(u, v).map_or(Point::ORIGIN, ogeom_math::Weighted::point);
                params.push_str(&format!(",{},{},{}", fmt(p.x), fmt(p.y), fmt(p.z)));
            }
        }
        params.push_str(&format!(",{},{},{},{}", fmt(u0), fmt(u1), fmt(v0), fmt(v1)));
        self.push(Pending {
            kind: 128,
            form: 0,
            transform: None,
            independent: false,
            label: String::new(),
            params,
        })
    }

    /// The deck: start, global, directory, parameters, terminate.
    fn serialize(&self) -> String {
        let mut s = String::new();
        fn push_record(s: &mut String, body: &str, section: char, seq: usize) {
            s.push_str(&format!("{body:<72}{section}{seq:>7}\n"));
        }
        push_record(&mut s, "ogeom IGES writer", 'S', 1);

        // Parameter text per entity, then directory and parameter sections
        // interleaved by the format's mutual pointers.
        let mut param_lines: Vec<Vec<String>> = Vec::with_capacity(self.entities.len());
        for e in &self.entities {
            let full = format!("{},{};", e.kind, e.params);
            param_lines.push(wrap_params(&full));
        }
        let mut param_starts = Vec::with_capacity(self.entities.len());
        let mut next_param = 1usize;
        for lines in &param_lines {
            param_starts.push(next_param);
            next_param += lines.len();
        }

        let globals = [
            "1H,".to_string(),
            "1H;".to_string(),
            "5Hogeom".to_string(),
            "9Hmodel.igs".to_string(),
            "5Hogeom".to_string(),
            "5Hogeom".to_string(),
            "32".to_string(),
            "308".to_string(),
            "15".to_string(),
            "308".to_string(),
            "15".to_string(),
            "5Hogeom".to_string(),
            "1.".to_string(),
            // Millimetres, stated twice as the format wants.
            "2".to_string(),
            "2HMM".to_string(),
            "1".to_string(),
            "0.01".to_string(),
            "15H20260807.000000".to_string(),
            fmt(1e-7),
            "0.".to_string(),
            "5Hogeom".to_string(),
            "5Hogeom".to_string(),
            "11".to_string(),
            "0".to_string(),
            "15H20260807.000000".to_string(),
        ];
        let global_text = globals.join(",") + ";";
        let global_lines = wrap_params(&global_text);
        for (i, line) in global_lines.iter().enumerate() {
            push_record(&mut s, line, 'G', i + 1);
        }

        let mut d_seq = 1usize;
        for (i, e) in self.entities.iter().enumerate() {
            let transform_de = e.transform.map_or(0, |t| self.de(t));
            let status = if e.independent {
                "00000000"
            } else {
                "00010000"
            };
            let line1 = format!(
                "{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{status}",
                e.kind, param_starts[i], 0, 0, 0, 0, transform_de, 0
            );
            push_record(&mut s, &line1, 'D', d_seq);
            let line2 = format!(
                "{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}",
                e.kind,
                0,
                0,
                param_lines[i].len(),
                e.form,
                "",
                "",
                e.label,
                0
            );
            push_record(&mut s, &line2, 'D', d_seq + 1);
            d_seq += 2;
        }

        let mut p_seq = 1usize;
        for (i, lines) in param_lines.iter().enumerate() {
            let back = self.de(i);
            for line in lines {
                s.push_str(&format!("{line:<64}{back:>8}P{p_seq:>7}\n"));
                p_seq += 1;
            }
        }
        let tail = format!(
            "S{:>7}G{:>7}D{:>7}P{:>7}",
            1,
            global_lines.len(),
            d_seq - 1,
            p_seq - 1
        );
        push_record(&mut s, &tail, 'T', 1);
        s
    }
}

/// The same B-spline traversed the other way, over the same domain.
fn reversed_bspline(b: &BSplineCurve) -> OgeomResult<BSplineCurve> {
    match Curve::BSpline(b.clone()).reversed() {
        Curve::BSpline(back) => Ok(back),
        _ => ogeom_bail!(Construction, "a reversed B-spline is a B-spline"),
    }
}

/// A rigid transform quantized to bits, for per-placement deduplication,
/// the same probe the STEP writer uses.
fn transform_bits(t: &Transform) -> [u64; 3] {
    let p = t.apply(Point::new(0.123_456_789, 9.87, -3.21));
    [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]
}

/// A real in the file, the shorter of its positional and exponent
/// spellings, each the shortest that reads back to the same value, with a
/// decimal point kept so a reader that types by spelling reads a real.
///
/// Positional notation alone spells a coefficient of 1.5e-51 in
/// sixty-nine characters, and a record holds sixty-four.
fn fmt(v: f64) -> String {
    let positional = {
        let s = format!("{v}");
        if s.contains('.') { s } else { format!("{s}.") }
    };
    let exponent = {
        let s = format!("{v:E}");
        match s.split_once('E') {
            Some((mantissa, power)) if !mantissa.contains('.') => {
                format!("{mantissa}.E{power}")
            }
            _ => s,
        }
    };
    if exponent.len() < positional.len() {
        exponent
    } else {
        positional
    }
}

/// Parameter text into records of at most 64 data columns, split between
/// parameters.
///
/// The reader joins a parameter record's data columns as they stand,
/// padding included, so a parameter is never split where padding would
/// land inside it: one that does not fit starts a new record. One longer
/// than a whole record (only a long name can be) fills every record it
/// crosses to exactly 64 columns, so the pieces rejoin with nothing
/// between them.
fn wrap_params(text: &str) -> Vec<String> {
    const COLUMNS: usize = 64;
    let mut lines = Vec::new();
    let mut current = String::new();
    for piece in text.split_inclusive(',') {
        if current.len() + piece.len() > COLUMNS && piece.len() <= COLUMNS {
            lines.push(std::mem::take(&mut current));
        }
        let mut rest = piece;
        while current.len() + rest.len() > COLUMNS {
            let mut room = COLUMNS - current.len();
            while !rest.is_char_boundary(room) {
                room -= 1;
            }
            current.push_str(&rest[..room]);
            rest = &rest[room..];
            lines.push(std::mem::take(&mut current));
        }
        current.push_str(rest);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::{fmt, wrap_params};

    #[test]
    fn a_real_takes_its_shorter_spelling_and_reads_back() {
        for v in [
            0.0,
            1.0,
            -2.5,
            1e-51,
            1.48776941870366e-51,
            2.76e-76,
            1e20,
            123_456.789,
        ] {
            let s = fmt(v);
            assert!(s.len() <= 24, "{v} spelt {s}");
            assert!(s.contains('.'), "{s} reads as a real");
            let back: f64 = s.replace('D', "E").parse().unwrap();
            assert_eq!(back, v, "{s}");
        }
        assert_eq!(fmt(1.0), "1.");
        assert_eq!(fmt(1e-51), "1.E-51");
    }

    #[test]
    fn a_parameter_longer_than_a_record_fills_the_records_it_crosses() {
        let name = format!("100H{},", "x".repeat(100));
        let text = format!("128,{name}1.5,2.;");
        let lines = wrap_params(&text);
        assert!(lines.iter().all(|l| l.len() <= 64));
        // The pieces rejoin as written: every record the long name crosses
        // is full, so padding never lands inside it.
        let long: Vec<&String> = lines.iter().filter(|l| l.contains('x')).collect();
        for l in &long[..long.len() - 1] {
            assert_eq!(l.len(), 64);
        }
        assert_eq!(lines.concat(), text);
    }
}
