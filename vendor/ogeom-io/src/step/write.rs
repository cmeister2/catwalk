//! Writing STEP: the document's products, assemblies, colours and B-rep.
//!
//! The mirror of the reader, and deliberately written against the same
//! vocabulary: every entity this writer emits is one the reader parses, so
//! writing what was read and reading it back is the honest round-trip test.
//! AP214's schema name goes in the header; the entities used are the common
//! AP203/AP214/AP242 core.
//!
//! Geometry is written in world coordinates: every face, edge and vertex is
//! transformed through its occurrence's own placement chain before it is
//! serialized, and shared nodes are deduplicated *per placement*: a prism's
//! bottom and top edge are one node at two locations, and the file needs
//! both. Surfaces the format has no analytic name for (extrusions,
//! revolutions) go out as their exact rational B-spline patches, so nothing
//! is fitted on the way out.
//!
//! An edge's pcurves go out with it, as the `PCURVE`s of a `SURFACE_CURVE`
//! (a `SEAM_CURVE` where one face's surface meets itself along it), wherever
//! the file's parameterizations are this kernel's own: the curve and the
//! surface written analytically or as the same spline, in right-handed
//! frames, under a placement that neither scales nor mirrors, and the pcurve
//! a line, a forward conic or a spline, restated over the curve's parameter
//! where the edge keeps it over a range of its own. Elsewhere the edge
//! carries its curve alone and a reader derives the pcurves.

use ogeom_core::FastMap;
use ogeom_core::{OgeomResult, Tolerances, ogeom_bail};
use ogeom_doc::{Document, ProductId, ProductKind};
use ogeom_geom::Transformable as _;
use ogeom_geom::{Curve, PlanarCurve, SurfaceGeometry};
use ogeom_math::{Frame, Handedness, Point, Point2, Transform, Vector};
use ogeom_topo::{EdgeRepr, Filter, Model, NodeData, Shape, ShapeType, SurfaceId, explore};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Write a document as a STEP exchange file.
///
/// Products become `PRODUCT` trees; parts carry their solids as
/// `MANIFOLD_SOLID_BREP`s in an `ADVANCED_BREP_SHAPE_REPRESENTATION`, and
/// their sheets (shells no solid owns, faces no shell owns) as
/// `SHELL_BASED_SURFACE_MODEL`s in a `MANIFOLD_SURFACE_SHAPE_REPRESENTATION`;
/// a part holding both carries both representations, related to each other.
/// Assemblies become usage occurrences with their placements; colours
/// become styled items over the written solids, sheets and faces (a fill
/// style) and edges (a curve style). PMI anchors to the written faces and
/// edges through shape aspects.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) if a
/// shape's structure cannot be expressed: a non-rigid instance placement, a
/// solid with no shell, a part holding wireframe (a wire, edge or vertex
/// outside every face) or nothing at all.
pub fn write_step(document: &Document, tol: Tolerances) -> OgeomResult<String> {
    NON_FINITE.with(|seen| seen.set(false));
    let mut writer = Writer {
        model: document.model(),
        entities: Vec::new(),
        points: FastMap::default(),
        directions: FastMap::default(),
        vertices: FastMap::default(),
        edges: FastMap::default(),
        written_nodes: Vec::new(),
        written_edges: Vec::new(),
        curve_font: None,
        surface_curves: BTreeMap::new(),
        parametric_context: None,
        tol,
    };

    let app = writer.entity("APPLICATION_CONTEXT('automotive design')".into());
    let _proto = writer.entity(format!(
        "APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2010,#{app})"
    ));
    let lu = writer.entity("(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.))".into());
    let au = writer.entity("(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.))".into());
    let su = writer.entity("(NAMED_UNIT(*)SI_UNIT($,.STERADIAN.)SOLID_ANGLE_UNIT())".into());
    let unc = writer.entity(format!(
        "UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.0E-06),#{lu},'distance_accuracy_value','')"
    ));
    let gctx = writer.entity(format!(
        "(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc}))GLOBAL_UNIT_ASSIGNED_CONTEXT((#{lu},#{au},#{su}))REPRESENTATION_CONTEXT('Context','3D'))"
    ));
    let pctx = writer.entity(format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));
    let pdctx = writer.entity(format!(
        "PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"
    ));

    // Products, in document order; every product gets its definition and its
    // shape representation before the assembly edges tie them together.
    let mut pd_of: FastMap<ProductId, u64> = FastMap::default();
    let mut sr_of: FastMap<ProductId, u64> = FastMap::default();
    let mut anchor_pds: Option<(u64, u64)> = None;
    for (id, product) in document.products() {
        let name = escape(&product.name);
        let p = writer.entity(format!("PRODUCT('{name}','{name}','',(#{pctx}))"));
        let formation = writer.entity(format!("PRODUCT_DEFINITION_FORMATION('','',#{p})"));
        let pd = writer.entity(format!(
            "PRODUCT_DEFINITION('design','',#{formation},#{pdctx})"
        ));
        pd_of.insert(id, pd);

        let world = writer.frame(&Frame::WORLD);
        let sr = match &product.kind {
            ProductKind::Part { shape } => {
                let bodies = crate::bodies::bodies_of(writer.model, shape, "STEP")?;
                let mut solids = vec![world];
                for solid in &bodies.solids {
                    solids.push(writer.solid(solid)?);
                }
                let mut sheets = vec![world];
                for shell in &bodies.shells {
                    sheets.push(writer.surface_model(shell, false)?);
                }
                for face in &bodies.faces {
                    sheets.push(writer.surface_model(face, true)?);
                }
                // Solids go in a B-rep representation and sheets in a
                // surface one; a part with both names the B-rep and ties
                // the surface representation to it.
                let brep = (solids.len() > 1).then(|| {
                    let list = reference_list(&solids);
                    writer.entity(format!(
                        "ADVANCED_BREP_SHAPE_REPRESENTATION('{name}',({list}),#{gctx})"
                    ))
                });
                let surface = (sheets.len() > 1).then(|| {
                    let list = reference_list(&sheets);
                    writer.entity(format!(
                        "MANIFOLD_SURFACE_SHAPE_REPRESENTATION('{name}',({list}),#{gctx})"
                    ))
                });
                match (brep, surface) {
                    (Some(brep), Some(surface)) => {
                        writer.entity(format!(
                            "SHAPE_REPRESENTATION_RELATIONSHIP('','',#{brep},#{surface})"
                        ));
                        brep
                    }
                    (Some(only), None) | (None, Some(only)) => only,
                    (None, None) => ogeom_bail!(
                        Construction,
                        "a part's shape holds no solid, shell or face to write as STEP"
                    ),
                }
            }
            ProductKind::Assembly { .. } => {
                writer.entity(format!("SHAPE_REPRESENTATION('{name}',(#{world}),#{gctx})"))
            }
        };
        sr_of.insert(id, sr);
        let pds = writer.entity(format!("PRODUCT_DEFINITION_SHAPE('','',#{pd})"));
        writer.entity(format!("SHAPE_DEFINITION_REPRESENTATION(#{pds},#{sr})"));
        if anchor_pds.is_none() && matches!(product.kind, ProductKind::Part { .. }) {
            anchor_pds = Some((pds, sr));
        }
    }

    writer.surface_curves()?;

    // Assembly edges: one usage occurrence per instance, its placement said
    // through the transformation between the parent's world frame and the
    // child's placement frame.
    let mut usage = 0_usize;
    for (id, product) in document.products() {
        let ProductKind::Assembly { children } = &product.kind else {
            continue;
        };
        for instance in children {
            usage += 1;
            let designator = instance
                .name
                .clone()
                .unwrap_or_else(|| format!("occurrence-{usage}"));
            let designator = escape(&designator);
            let (parent_pd, child_pd) = (pd_of[&id], pd_of[&instance.product]);
            let (parent_sr, child_sr) = (sr_of[&id], sr_of[&instance.product]);
            let nauo = writer.entity(format!(
                "NEXT_ASSEMBLY_USAGE_OCCURRENCE('{designator}','{designator}','',#{parent_pd},#{child_pd},$)"
            ));
            // The location resolved through the model's own datum store: a
            // placed dummy shape shares the resolution path every traversal
            // uses.
            let at = location_transform(&instance.location, document.model())?;
            let placed = writer.placement_frame(&at)?;
            let world = writer.frame(&Frame::WORLD);
            let idt = writer.entity(format!(
                "ITEM_DEFINED_TRANSFORMATION('','',#{world},#{placed})"
            ));
            let rr = writer.entity(format!(
                "(REPRESENTATION_RELATIONSHIP('','',#{child_sr},#{parent_sr})REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{idt})SHAPE_REPRESENTATION_RELATIONSHIP())"
            ));
            let pds = writer.entity(format!("PRODUCT_DEFINITION_SHAPE('','',#{nauo})"));
            writer.entity(format!(
                "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{rr},#{pds})"
            ));
        }
    }

    // Colours: a styled item over every written entity whose node the
    // document colours, plus product colours carried by their solids. An
    // edge's colour is a curve style on each of its EDGE_CURVEs.
    let mut styled = Vec::new();
    let node_colours: FastMap<_, _> = document.colours().collect();
    // The written entities by node, each list in the order they were
    // written: a coloured product finds its own without walking them all.
    let written = std::mem::take(&mut writer.written_nodes);
    let mut by_node: FastMap<ogeom_topo::TShapeId, Vec<u64>> = FastMap::default();
    for &(node, step_id) in &written {
        by_node.entry(node).or_default().push(step_id);
    }
    for &(node, step_id) in &written {
        if let Some(colour) = node_colours.get(&node) {
            styled.push(writer.styled_item(step_id, *colour));
        }
    }
    for (_, product) in document.products() {
        let Some(colour) = product.colour else {
            continue;
        };
        let ProductKind::Part { shape } = &product.kind else {
            continue;
        };
        if node_colours.contains_key(&shape.node()) {
            continue;
        }
        for &step_id in by_node.get(&shape.node()).map_or(&[][..], Vec::as_slice) {
            styled.push(writer.styled_item(step_id, colour));
        }
    }
    writer.written_nodes = written;
    let edges = std::mem::take(&mut writer.written_edges);
    for &(node, step_id) in &edges {
        if let Some(colour) = node_colours.get(&node) {
            styled.push(writer.curve_styled_item(step_id, *colour));
        }
    }
    writer.written_edges = edges;
    if !styled.is_empty() {
        let list = styled
            .iter()
            .map(|i| format!("#{i}"))
            .collect::<Vec<_>>()
            .join(",");
        writer.entity(format!(
            "MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION('',({list}),#{gctx})"
        ));
    }

    // Semantic PMI: datums first, so tolerances can reference their letters.
    let pmi = document.pmi();
    if !pmi.is_empty() {
        let Some((pds, absr)) = anchor_pds else {
            ogeom_bail!(
                Construction,
                "PMI needs at least one part to anchor its aspects to"
            );
        };
        writer.pmi(pmi, document.views(), pds, absr, lu, au, gctx)?;
    }

    let mut out = String::new();
    out.push_str("ISO-10303-21;\nHEADER;\n");
    out.push_str("FILE_DESCRIPTION(('written by ogeom'),'2;1');\n");
    out.push_str("FILE_NAME('','',('ogeom'),('ogeom'),'ogeom','ogeom','');\n");
    out.push_str("FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));\n");
    out.push_str("ENDSEC;\nDATA;\n");
    for (i, entity) in writer.entities.iter().enumerate() {
        let _ = writeln!(out, "#{}={entity};", i + 1);
    }
    out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    if NON_FINITE.with(std::cell::Cell::get) {
        ogeom_bail!(
            Construction,
            "the model holds a number that is not finite, which a STEP file cannot state"
        );
    }
    Ok(out)
}

std::thread_local! {
    /// Set when [`real`] meets a value Part 21 has no spelling for, so the
    /// write refuses rather than emit a file no reader accepts.
    static NON_FINITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The state of one write: the entity buffer and the per-placement caches.
struct Writer<'a> {
    model: &'a Model,
    entities: Vec<String>,
    points: FastMap<[u64; 3], u64>,
    directions: FastMap<[u64; 3], u64>,
    /// Vertex occurrences by node and world position bits.
    vertices: FastMap<(ogeom_topo::TShapeId, [u64; 3]), u64>,
    /// Edge occurrences by node and placement bits.
    edges: FastMap<(ogeom_topo::TShapeId, [u64; 3]), u64>,
    /// Every solid and face written, with its entity id: the hooks colours
    /// attach to.
    written_nodes: Vec<(ogeom_topo::TShapeId, u64)>,
    /// Every `EDGE_CURVE` written, with its edge node: one per placement
    /// of the edge.
    written_edges: Vec<(ogeom_topo::TShapeId, u64)>,
    /// The one curve font every edge style shares, once written.
    curve_font: Option<u64>,
    /// Every `EDGE_CURVE` written, by its entity id, with the pcurves its
    /// faces gave it; rewritten over a surface curve once all are known.
    surface_curves: BTreeMap<u64, EdgeCurve>,
    /// The parameter-space context every pcurve's representation shares.
    parametric_context: Option<u64>,
    tol: Tolerances,
}

/// An `EDGE_CURVE` as written, and the pcurves its faces hold on it.
struct EdgeCurve {
    from: u64,
    to: u64,
    curve: u64,
    /// The edge's range on its curve.
    range: (f64, f64),
    /// Whether the curve went out in this kernel's own parameterization,
    /// so a pcurve over the same range means the same points to a reader.
    exact: bool,
    /// Per face surface entity, its pcurves (one, or a seam's two) and
    /// how their parameter follows the curve's.
    pcurves: Vec<(u64, Vec<PlanarCurve>, Pace)>,
}

/// How a pcurve's parameter follows its edge's curve's: `shift + scale * t`
/// at the curve's `t`. An edge may state its pcurve over a range of its
/// own, the two related this way.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pace {
    scale: f64,
    shift: f64,
}

impl Pace {
    const SAME: Self = Self {
        scale: 1.0,
        shift: 0.0,
    };

    /// The pace taking the curve's range onto the pcurve's, the same one
    /// where the two agree.
    fn between(curve: (f64, f64), pcurve: (f64, f64), slack: f64) -> Option<Self> {
        if (curve.0 - pcurve.0).abs() <= slack && (curve.1 - pcurve.1).abs() <= slack {
            return Some(Self::SAME);
        }
        let scale = (pcurve.1 - pcurve.0) / (curve.1 - curve.0);
        let shift = scale.mul_add(-curve.0, pcurve.0);
        (scale.is_finite() && shift.is_finite() && scale > 0.0).then_some(Self { scale, shift })
    }
}

impl Writer<'_> {
    fn entity(&mut self, text: String) -> u64 {
        self.entities.push(text);
        self.entities.len() as u64
    }

    fn point(&mut self, p: Point) -> u64 {
        let key = [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
        if let Some(&id) = self.points.get(&key) {
            return id;
        }
        let id = self.entity(format!(
            "CARTESIAN_POINT('',({},{},{}))",
            real(p.x),
            real(p.y),
            real(p.z)
        ));
        self.points.insert(key, id);
        id
    }

    fn direction(&mut self, v: Vector) -> u64 {
        let key = [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
        if let Some(&id) = self.directions.get(&key) {
            return id;
        }
        let id = self.entity(format!(
            "DIRECTION('',({},{},{}))",
            real(v.x),
            real(v.y),
            real(v.z)
        ));
        self.directions.insert(key, id);
        id
    }

    fn frame(&mut self, frame: &Frame) -> u64 {
        let origin = self.point(frame.origin());
        let z = self.direction(frame.z().vector());
        let x = self.direction(frame.x().vector());
        self.entity(format!("AXIS2_PLACEMENT_3D('',#{origin},#{z},#{x})"))
    }

    /// A rigid transform as the frame it carries the world onto.
    fn placement_frame(&mut self, at: &Transform) -> OgeomResult<u64> {
        let origin = at.apply(Point::ORIGIN);
        let z = at.apply_vector(Vector::new(0.0, 0.0, 1.0));
        let x = at.apply_vector(Vector::new(1.0, 0.0, 0.0));
        let frame = Frame::new(
            origin,
            ogeom_math::Direction::new(z, self.tol)?,
            ogeom_math::Direction::new(x, self.tol)?,
            self.tol,
        )
        .map_err(|_| {
            ogeom_core::ogeom_err!(
                Construction,
                "an instance placement is not rigid; STEP cannot state it"
            )
        })?;
        Ok(self.frame(&frame))
    }

    /// A sheet as a `SHELL_BASED_SURFACE_MODEL` of one shell: a free shell
    /// as an `OPEN_SHELL`, or a `CLOSED_SHELL` when it closes, and a lone
    /// face (`lone`) as an `OPEN_SHELL` of that face alone.
    fn surface_model(&mut self, sheet: &Shape, lone: bool) -> OgeomResult<u64> {
        let faces = if lone {
            vec![sheet.clone()]
        } else {
            self.model.ordered_children_of(sheet)?
        };
        if faces.is_empty() {
            ogeom_bail!(Construction, "a shell with no face cannot be written");
        }
        let keyword = if !lone && ogeom_algo::is_shell_closed(self.model, sheet)? {
            "CLOSED_SHELL"
        } else {
            "OPEN_SHELL"
        };
        let mut ids = Vec::with_capacity(faces.len());
        for face in &faces {
            ids.push(self.face(face)?);
        }
        let list = reference_list(&ids);
        let shell = self.entity(format!("{keyword}('',({list}))"));
        let model = self.entity(format!("SHELL_BASED_SURFACE_MODEL('',(#{shell}))"));
        if !lone {
            self.written_nodes.push((sheet.node(), model));
        }
        Ok(model)
    }

    /// A solid, with its voids when it has any: the first shell is the
    /// outer boundary, and each further shell bounds a cavity. A void is
    /// written as the standard names it, a closed shell facing out of the
    /// cavity used the other way round, which leaves every face of the
    /// solid facing away from the material.
    fn solid(&mut self, solid: &Shape) -> OgeomResult<u64> {
        let shells = explore(self.model, solid, Filter::OfType(ShapeType::Shell))?;
        let Some((outer, voids)) = shells.split_first() else {
            ogeom_bail!(Construction, "a solid with no shell cannot be written");
        };
        let shell_id = self.closed_shell(outer, false)?;
        let msb = if voids.is_empty() {
            self.entity(format!("MANIFOLD_SOLID_BREP('',#{shell_id})"))
        } else {
            let mut uses = Vec::with_capacity(voids.len());
            for void in voids {
                let own = self.closed_shell(void, true)?;
                uses.push(self.entity(format!("ORIENTED_CLOSED_SHELL('',*,#{own},.F.)")));
            }
            let list = uses
                .iter()
                .map(|i| format!("#{i}"))
                .collect::<Vec<_>>()
                .join(",");
            self.entity(format!("BREP_WITH_VOIDS('',#{shell_id},({list}))"))
        };
        self.written_nodes.push((solid.node(), msb));
        Ok(msb)
    }

    /// A shell's faces as a `CLOSED_SHELL`, each turned when `turned`.
    fn closed_shell(&mut self, shell: &Shape, turned: bool) -> OgeomResult<u64> {
        let mut faces = Vec::new();
        for face in self.model.ordered_children_of(shell)? {
            let face = if turned { face.reversed() } else { face };
            faces.push(self.face(&face)?);
        }
        let list = faces
            .iter()
            .map(|i| format!("#{i}"))
            .collect::<Vec<_>>()
            .join(",");
        Ok(self.entity(format!("CLOSED_SHELL('',({list}))")))
    }

    fn face(&mut self, face: &Shape) -> OgeomResult<u64> {
        let placement = face.transform(self.model.datums())?;
        let (surface, held) = {
            let Some(node) = self.model.node(face) else {
                ogeom_bail!(Dangling, "face is not in this model");
            };
            let NodeData::Face(data) = node.data() else {
                ogeom_bail!(Construction, "face node holds no face data");
            };
            let Some(surface) = self.model.geometry().surface(data.surface) else {
                ogeom_bail!(Dangling, "face refers to a surface not in this model");
            };
            (
                surface.clone().transformed(&placement, self.tol)?,
                data.surface,
            )
        };
        let surface_id = self.surface(&surface)?;
        // The face's pcurves mean the same points in the file only where
        // the surface's parameters do.
        let chart = (rigid(&placement) && surface_states_parameters(&surface))
            .then_some((held, surface_id));

        // Each loop walked under the face's sense, in the order the face
        // stores them: the outer one first, which the walk of a reversed
        // face lists last.
        let mut bounds = Vec::new();
        for (index, wire) in self.model.children_of(face)?.iter().enumerate() {
            let keyword = if index == 0 {
                "FACE_OUTER_BOUND"
            } else {
                "FACE_BOUND"
            };
            let loop_id = self.wire(wire, chart)?;
            bounds.push(self.entity(format!("{keyword}('',#{loop_id},.T.)")));
        }
        let list = bounds
            .iter()
            .map(|i| format!("#{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let sense = if face.orientation() == ogeom_topo::Orientation::Reversed {
            ".F."
        } else {
            ".T."
        };
        let id = self.entity(format!("ADVANCED_FACE('',({list}),#{surface_id},{sense})"));
        self.written_nodes.push((face.node(), id));
        Ok(id)
    }

    /// A wire as an `EDGE_LOOP`, or a `VERTEX_LOOP` when every edge in it is
    /// degenerate: a pole or an apex has no curve to serialize, and STEP's
    /// own spelling for it is the loop of one vertex. Each edge's pcurve on
    /// `chart`, the face's surface and its entity, is noted for its edge.
    fn wire(&mut self, wire: &Shape, chart: Option<(SurfaceId, u64)>) -> OgeomResult<u64> {
        let children = self.model.ordered_children_of(wire)?;
        let degenerate = |edge: &Shape| {
            self.model
                .node(edge)
                .and_then(|n| n.data().as_edge())
                .is_some_and(|d| d.degenerate)
        };
        if !children.is_empty() && children.iter().all(degenerate) {
            let edge = &children[0];
            let Some(vertex) = self.model.children_of(edge)?.first().cloned() else {
                ogeom_bail!(Construction, "a degenerate edge has no vertex");
            };
            let vertex_id = self.vertex(&vertex, &edge.transform(self.model.datums())?)?;
            return Ok(self.entity(format!("VERTEX_LOOP('',#{vertex_id})")));
        }
        let mut oriented = Vec::new();
        for edge in &children {
            if degenerate(edge) {
                continue;
            }
            let edge_id = self.edge(edge)?;
            if let Some(chart) = chart {
                self.note_pcurves(edge, edge_id, chart);
            }
            let sense = if edge.orientation() == ogeom_topo::Orientation::Reversed {
                ".F."
            } else {
                ".T."
            };
            oriented.push(self.entity(format!("ORIENTED_EDGE('',*,*,#{edge_id},{sense})")));
        }
        let list = oriented
            .iter()
            .map(|i| format!("#{i}"))
            .collect::<Vec<_>>()
            .join(",");
        Ok(self.entity(format!("EDGE_LOOP('',({list}))")))
    }

    fn edge(&mut self, edge: &Shape) -> OgeomResult<u64> {
        let placement = edge.transform(self.model.datums())?;
        let key = (edge.node(), transform_bits(&placement));
        if let Some(&id) = self.edges.get(&key) {
            return Ok(id);
        }
        let (curve, range) = {
            let Some(node) = self.model.node(edge) else {
                ogeom_bail!(Dangling, "edge is not in this model");
            };
            let Some(data) = node.data().as_edge() else {
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
        let exact = curve_states_parameter(&curve);
        let curve_id = self.curve(&curve, range)?;
        let vertices = self.model.children_of(edge)?;
        let (from, to) = match vertices.len() {
            0 => ogeom_bail!(Construction, "an edge with no vertices cannot be written"),
            1 => (vertices[0].clone(), vertices[0].clone()),
            _ => (vertices[0].clone(), vertices[vertices.len() - 1].clone()),
        };
        // Each vertex's own composed placement, not the edge's: children_of
        // already folds the edge's chain in, and an instanced vertex adds a
        // hop of its own that the edge's placement alone would drop.
        let from_placement = from.transform(self.model.datums())?;
        let to_placement = to.transform(self.model.datums())?;
        let from_id = self.vertex(&from, &from_placement)?;
        let to_id = self.vertex(&to, &to_placement)?;
        let id = self.entity(format!(
            "EDGE_CURVE('',#{from_id},#{to_id},#{curve_id},.T.)"
        ));
        self.edges.insert(key, id);
        self.written_edges.push((edge.node(), id));
        self.surface_curves.insert(
            id,
            EdgeCurve {
                from: from_id,
                to: to_id,
                curve: curve_id,
                range,
                exact,
                pcurves: Vec::new(),
            },
        );
        Ok(id)
    }

    /// Note the pcurves `edge` holds on a face's surface, the surface and
    /// the entity written for it, where they state the edge over its own
    /// range in a form the file carries exactly.
    fn note_pcurves(&mut self, edge: &Shape, edge_id: u64, (held, surface_id): (SurfaceId, u64)) {
        let Some(record) = self.surface_curves.get_mut(&edge_id) else {
            return;
        };
        if !record.exact || record.pcurves.iter().any(|(s, ..)| *s == surface_id) {
            return;
        }
        let Some(data) = self.model.node(edge).and_then(|n| n.data().as_edge()) else {
            return;
        };
        let (ids, range) = match data.pcurve_for(held, edge.location()) {
            Some(EdgeRepr::PCurve { curve, range, .. }) => (vec![*curve], *range),
            Some(EdgeRepr::Seam {
                forward,
                reversed,
                range,
                ..
            }) => (vec![*forward, *reversed], *range),
            _ => return,
        };
        let Some(pace) = Pace::between(record.range, range, self.tol.parametric()) else {
            return;
        };
        let mut curves = Vec::with_capacity(ids.len());
        for id in ids {
            match self.model.geometry().pcurve(id) {
                Some(pcurve) if planar_stated(pcurve, pace) => curves.push(pcurve.clone()),
                _ => return,
            }
        }
        record.pcurves.push((surface_id, curves, pace));
    }

    /// Every written edge that gathered pcurves, rewritten over a
    /// `SURFACE_CURVE` carrying them, or a `SEAM_CURVE` where its two lie
    /// on one face's surface. The `EDGE_CURVE` keeps its entity id, so
    /// whatever already refers to it still does.
    fn surface_curves(&mut self) -> OgeomResult<()> {
        let records = std::mem::take(&mut self.surface_curves);
        for (edge_id, record) in &records {
            let count: usize = record.pcurves.iter().map(|(_, c, _)| c.len()).sum();
            let keyword = match (record.pcurves.len(), count) {
                (1, 2) => "SEAM_CURVE",
                (1, 1) | (2, 2) => "SURFACE_CURVE",
                _ => continue,
            };
            let mut uses = Vec::with_capacity(count);
            for (surface_id, curves, pace) in &record.pcurves {
                for curve in curves {
                    let planar = self.planar_curve(curve, *pace)?;
                    let context = self.parametric_context();
                    let definition = self.entity(format!(
                        "DEFINITIONAL_REPRESENTATION('',(#{planar}),#{context})"
                    ));
                    uses.push(self.entity(format!("PCURVE('',#{surface_id},#{definition})")));
                }
            }
            let list = reference_list(&uses);
            let curve = self.entity(format!(
                "{keyword}('',#{},({list}),.PCURVE_S1.)",
                record.curve
            ));
            let (from, to) = (record.from, record.to);
            let Some(text) = usize::try_from(*edge_id)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| self.entities.get_mut(i))
            else {
                ogeom_bail!(Construction, "an edge entity was lost before its pcurves");
            };
            *text = format!("EDGE_CURVE('',#{from},#{to},#{curve},.T.)");
        }
        self.surface_curves = records;
        Ok(())
    }

    /// The context a pcurve's representation lives in: two dimensions of
    /// a surface's parameters.
    fn parametric_context(&mut self) -> u64 {
        if let Some(id) = self.parametric_context {
            return id;
        }
        let id = self.entity(
            "(GEOMETRIC_REPRESENTATION_CONTEXT(2)PARAMETRIC_REPRESENTATION_CONTEXT()REPRESENTATION_CONTEXT('2D SPACE',''))".into(),
        );
        self.parametric_context = Some(id);
        id
    }

    fn point2(&mut self, p: Point2) -> u64 {
        self.entity(format!("CARTESIAN_POINT('',({},{}))", real(p.x), real(p.y)))
    }

    fn direction2(&mut self, v: ogeom_math::Vector2) -> u64 {
        self.entity(format!("DIRECTION('',({},{}))", real(v.x), real(v.y)))
    }

    fn placement2(&mut self, frame: &ogeom_math::Frame2) -> u64 {
        let origin = self.point2(frame.origin());
        let x = self.direction2(frame.x().vector());
        self.entity(format!("AXIS2_PLACEMENT_2D('',#{origin},#{x})"))
    }

    /// A pcurve [`planar_stated`] admits, over its edge's curve's
    /// parameter: at the curve's `t`, the pcurve's point at `pace`'s image
    /// of `t`.
    fn planar_curve(&mut self, curve: &PlanarCurve, pace: Pace) -> OgeomResult<u64> {
        match curve {
            PlanarCurve::Line(line) => {
                let axis = line.axis();
                let direction = axis.direction.vector();
                let origin = self.point2(axis.location + direction * pace.shift);
                let d = self.direction2(direction);
                let vector = self.entity(format!("VECTOR('',#{d},{})", real(pace.scale)));
                Ok(self.entity(format!("LINE('',#{origin},#{vector})")))
            }
            PlanarCurve::Circle(c) => {
                let circle = c.circle();
                let frame = self.placement2(&circle.frame());
                Ok(self.entity(format!("CIRCLE('',#{frame},{})", real(circle.radius()))))
            }
            PlanarCurve::Ellipse(e) => {
                let ellipse = e.ellipse();
                let frame = self.placement2(&ellipse.frame());
                Ok(self.entity(format!(
                    "ELLIPSE('',#{frame},{},{})",
                    real(ellipse.major_radius()),
                    real(ellipse.minor_radius())
                )))
            }
            PlanarCurve::BSpline(b) => {
                let control: Vec<String> = b
                    .control_points()
                    .iter()
                    .map(|c| {
                        let p = Point2::from_vector(c.scaled.to_vector() / c.weight);
                        format!("#{}", self.point2(p))
                    })
                    .collect();
                let (mults, knots) = compress_knots(b.knots().knots());
                let knots: Vec<f64> = if pace == Pace::SAME {
                    knots
                } else {
                    knots
                        .iter()
                        .map(|k| (k - pace.shift) / pace.scale)
                        .collect()
                };
                let degree = b.knots().degree();
                let control = control.join(",");
                let mults = mults
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                let knots = knots.iter().map(|k| real(*k)).collect::<Vec<_>>().join(",");
                if b.is_rational() {
                    let weights = b
                        .control_points()
                        .iter()
                        .map(|c| real(c.weight))
                        .collect::<Vec<_>>()
                        .join(",");
                    Ok(self.entity(format!(
                        "(BOUNDED_CURVE()B_SPLINE_CURVE({degree},({control}),.UNSPECIFIED.,.F.,.F.)B_SPLINE_CURVE_WITH_KNOTS(({mults}),({knots}),.UNSPECIFIED.)CURVE()GEOMETRIC_REPRESENTATION_ITEM()RATIONAL_B_SPLINE_CURVE(({weights}))REPRESENTATION_ITEM(''))"
                    )))
                } else {
                    Ok(self.entity(format!(
                        "B_SPLINE_CURVE_WITH_KNOTS('',{degree},({control}),.UNSPECIFIED.,.F.,.F.,({mults}),({knots}),.UNSPECIFIED.)"
                    )))
                }
            }
            PlanarCurve::Trimmed(t) => self.planar_curve(t.basis(), pace),
            PlanarCurve::Offset(_) | PlanarCurve::Trig(_) => ogeom_bail!(
                Construction,
                "a pcurve with no exact spelling in the file was admitted"
            ),
        }
    }

    fn vertex(&mut self, vertex: &Shape, placement: &Transform) -> OgeomResult<u64> {
        let Some(data) = self.model.node(vertex).and_then(|n| n.data().as_vertex()) else {
            ogeom_bail!(Construction, "vertex node holds no vertex data");
        };
        let at = placement.apply(data.point);
        let key = (
            vertex.node(),
            [at.x.to_bits(), at.y.to_bits(), at.z.to_bits()],
        );
        if let Some(&id) = self.vertices.get(&key) {
            return Ok(id);
        }
        let point = self.point(at);
        let id = self.entity(format!("VERTEX_POINT('',#{point})"));
        self.vertices.insert(key, id);
        Ok(id)
    }

    /// A curve for an `EDGE_CURVE`: the analytic spelling where STEP has
    /// one, the exact B-spline conversion where it does not.
    fn curve(&mut self, curve: &Curve, range: (f64, f64)) -> OgeomResult<u64> {
        match curve {
            Curve::Line(line) => {
                let axis = line.axis();
                let origin = self.point(axis.location);
                let d = self.direction(axis.direction.vector());
                let vector = self.entity(format!("VECTOR('',#{d},1.0)"));
                Ok(self.entity(format!("LINE('',#{origin},#{vector})")))
            }
            // A circle or ellipse running backwards goes out as its forward
            // spelling: STEP's runs counter-clockwise about its axis.
            Curve::Circle(c) => {
                let circle = c.forward(self.tol)?.circle();
                let frame = self.frame(&circle.frame());
                Ok(self.entity(format!("CIRCLE('',#{frame},{})", real(circle.radius()))))
            }
            Curve::Ellipse(el) => {
                let ellipse = el.forward(self.tol)?.ellipse();
                let frame = self.frame(&ellipse.frame());
                Ok(self.entity(format!(
                    "ELLIPSE('',#{frame},{},{})",
                    real(ellipse.major_radius()),
                    real(ellipse.minor_radius())
                )))
            }
            Curve::BSpline(b) => self.bspline_curve(b),
            // The open conics in STEP's own spelling, where their parameter
            // runs STEP's way; a reversed one is written as its spline.
            Curve::Hyperbola(h) if !h.is_reversed() => {
                let hyperbola = h.hyperbola();
                let frame = self.frame(&hyperbola.frame());
                Ok(self.entity(format!(
                    "HYPERBOLA('',#{frame},{},{})",
                    real(hyperbola.major_radius()),
                    real(hyperbola.minor_radius())
                )))
            }
            Curve::Parabola(p) if !p.is_reversed() => {
                let parabola = p.parabola();
                let frame = self.frame(&parabola.frame());
                Ok(self.entity(format!("PARABOLA('',#{frame},{})", real(parabola.focal()))))
            }
            Curve::Offset(o) => {
                let basis = self.curve(o.basis(), range)?;
                let reference = self.direction(o.reference().vector());
                Ok(self.entity(format!(
                    "OFFSET_CURVE_3D('',#{basis},{},.F.,#{reference})",
                    real(o.distance())
                )))
            }
            other => {
                // Exact for the conics and trims: the conversion is exact,
                // not a fit. A helix refuses inside the
                // conversion: it has no exact spline form, and writing a
                // fit without saying so is the lie this crate does not tell.
                let spline = other.to_bspline_over(range, self.tol)?;
                self.bspline_curve(&spline)
            }
        }
    }

    fn bspline_curve(&mut self, spline: &ogeom_geom::BSplineCurve) -> OgeomResult<u64> {
        let control: Vec<String> = spline
            .control_points()
            .iter()
            .map(|c| {
                let p = Point::from_vector(c.scaled.to_vector() / c.weight);
                format!("#{}", self.point(p))
            })
            .collect();
        let (mults, knots) = compress_knots(spline.knots().knots());
        let degree = spline.knots().degree();
        let rational = spline
            .control_points()
            .iter()
            .any(|c| (c.weight - 1.0).abs() > 1e-12);
        let control = control.join(",");
        let mults = mults
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let knots = knots.iter().map(|k| real(*k)).collect::<Vec<_>>().join(",");
        if rational {
            let weights = spline
                .control_points()
                .iter()
                .map(|c| real(c.weight))
                .collect::<Vec<_>>()
                .join(",");
            Ok(self.entity(format!(
                "(BOUNDED_CURVE()B_SPLINE_CURVE({degree},({control}),.UNSPECIFIED.,.F.,.F.)B_SPLINE_CURVE_WITH_KNOTS(({mults}),({knots}),.UNSPECIFIED.)CURVE()GEOMETRIC_REPRESENTATION_ITEM()RATIONAL_B_SPLINE_CURVE(({weights}))REPRESENTATION_ITEM(''))"
            )))
        } else {
            Ok(self.entity(format!(
                "B_SPLINE_CURVE_WITH_KNOTS('',{degree},({control}),.UNSPECIFIED.,.F.,.F.,({mults}),({knots}),.UNSPECIFIED.)"
            )))
        }
    }

    /// A surface: analytic where STEP has the word, the exact B-spline patch
    /// where it does not. Trimmed surfaces write their basis; the trim is
    /// the face's own topology.
    fn surface(&mut self, surface: &SurfaceGeometry) -> OgeomResult<u64> {
        match surface {
            SurfaceGeometry::Plane(s) => {
                let frame = self.frame(&s.plane().frame());
                Ok(self.entity(format!("PLANE('',#{frame})")))
            }
            SurfaceGeometry::Cylinder(s) => {
                let cylinder = s.cylinder();
                let frame = self.frame(&cylinder.frame());
                Ok(self.entity(format!(
                    "CYLINDRICAL_SURFACE('',#{frame},{})",
                    real(cylinder.radius())
                )))
            }
            SurfaceGeometry::Cone(s) => {
                let cone = s.cone();
                let frame = self.frame(&cone.frame());
                Ok(self.entity(format!(
                    "CONICAL_SURFACE('',#{frame},{},{})",
                    real(cone.radius_at(0.0)),
                    real(cone.half_angle())
                )))
            }
            SurfaceGeometry::Sphere(s) => {
                let sphere = s.sphere();
                let frame = self.frame(&sphere.frame());
                Ok(self.entity(format!(
                    "SPHERICAL_SURFACE('',#{frame},{})",
                    real(sphere.radius())
                )))
            }
            SurfaceGeometry::Torus(s) => {
                let torus = s.torus();
                let frame = self.frame(&torus.frame());
                Ok(self.entity(format!(
                    "TOROIDAL_SURFACE('',#{frame},{},{})",
                    real(torus.major_radius()),
                    real(torus.minor_radius())
                )))
            }
            SurfaceGeometry::BSpline(b) => self.bspline_surface(b),
            SurfaceGeometry::Trimmed(t) => self.surface(t.basis()),
            // Swept and offset surfaces in STEP's own spelling: exact, and
            // read back as themselves.
            SurfaceGeometry::Revolution(r) => {
                let curve = r.curve();
                let range = ogeom_geom::Curve3d::domain(curve);
                let swept = self.curve(curve, range)?;
                let axis = r.axis();
                let location = self.point(axis.location);
                let direction = self.direction(axis.direction.vector());
                let placement =
                    self.entity(format!("AXIS1_PLACEMENT('',#{location},#{direction})"));
                Ok(self.entity(format!("SURFACE_OF_REVOLUTION('',#{swept},#{placement})")))
            }
            SurfaceGeometry::Extrusion(e) => {
                let curve = e.curve();
                let range = ogeom_geom::Curve3d::domain(curve);
                let swept = self.curve(curve, range)?;
                let direction = self.direction(e.direction().vector());
                let vector = self.entity(format!("VECTOR('',#{direction},1.0)"));
                Ok(self.entity(format!(
                    "SURFACE_OF_LINEAR_EXTRUSION('',#{swept},#{vector})"
                )))
            }
            SurfaceGeometry::Offset(o) => {
                let basis = self.surface(o.basis())?;
                Ok(self.entity(format!(
                    "OFFSET_SURFACE('',#{basis},{},.F.)",
                    real(o.distance())
                )))
            }
        }
    }

    fn bspline_surface(&mut self, patch: &ogeom_geom::BSplineSurface) -> OgeomResult<u64> {
        let grid = patch.grid();
        let (nu, nv) = (grid.u_count(), grid.v_count());
        let mut rows = Vec::with_capacity(nu);
        let mut weights_rows = Vec::with_capacity(nu);
        let mut rational = false;
        for u in 0..nu {
            let mut row = Vec::with_capacity(nv);
            let mut wrow = Vec::with_capacity(nv);
            for v in 0..nv {
                let Some(c) = grid.get(u, v) else {
                    ogeom_bail!(Construction, "a control grid cell is missing");
                };
                let p = Point::from_vector(c.scaled.to_vector() / c.weight);
                row.push(format!("#{}", self.point(p)));
                wrow.push(real(c.weight));
                rational |= (c.weight - 1.0).abs() > 1e-12;
            }
            rows.push(format!("({})", row.join(",")));
            weights_rows.push(format!("({})", wrow.join(",")));
        }
        let grid_text = rows.join(",");
        let (u_deg, v_deg) = (patch.u_knots().degree(), patch.v_knots().degree());
        let (um, uk) = compress_knots(patch.u_knots().knots());
        let (vm, vk) = compress_knots(patch.v_knots().knots());
        let fmt_m = |m: &[usize]| {
            m.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        };
        let fmt_k = |k: &[f64]| k.iter().map(|v| real(*v)).collect::<Vec<_>>().join(",");
        let (um, uk, vm, vk) = (fmt_m(&um), fmt_k(&uk), fmt_m(&vm), fmt_k(&vk));
        if rational {
            let weights = weights_rows.join(",");
            Ok(self.entity(format!(
                "(BOUNDED_SURFACE()B_SPLINE_SURFACE({u_deg},{v_deg},({grid_text}),.UNSPECIFIED.,.F.,.F.,.F.)B_SPLINE_SURFACE_WITH_KNOTS(({um}),({vm}),({uk}),({vk}),.UNSPECIFIED.)GEOMETRIC_REPRESENTATION_ITEM()RATIONAL_B_SPLINE_SURFACE(({weights}))REPRESENTATION_ITEM('')SURFACE())"
            )))
        } else {
            Ok(self.entity(format!(
                "B_SPLINE_SURFACE_WITH_KNOTS('',{u_deg},{v_deg},({grid_text}),.UNSPECIFIED.,.F.,.F.,.F.,({um}),({vm}),({uk}),({vk}),.UNSPECIFIED.)"
            )))
        }
    }

    /// The document's PMI, written over the aspects of the anchor part.
    #[allow(clippy::many_single_char_names)]
    #[allow(
        clippy::too_many_arguments,
        reason = "five STEP context ids that travel together"
    )]
    fn pmi(
        &mut self,
        pmi: &ogeom_doc::Pmi,
        views: &[ogeom_doc::View],
        pds: u64,
        absr: u64,
        lu: u64,
        au: u64,
        gctx: u64,
    ) -> OgeomResult<()> {
        let by_node: FastMap<ogeom_topo::TShapeId, u64> = self
            .written_nodes
            .iter()
            .chain(&self.written_edges)
            .copied()
            .collect();
        let aspect_for = |w: &mut Self, items: &[ogeom_topo::TShapeId]| -> u64 {
            let aspect = w.entity(format!("SHAPE_ASPECT('','',#{pds},.T.)"));
            for item in items {
                if let Some(&step_id) = by_node.get(item) {
                    w.entity(format!(
                        "GEOMETRIC_ITEM_SPECIFIC_USAGE('','',#{aspect},#{absr},#{step_id})"
                    ));
                }
            }
            aspect
        };

        // Which STEP id each annotation was written as, so the presentation
        // below can point a callout at the annotation it draws.
        let mut annotation_ids: FastMap<ogeom_doc::Annotated, u64> = FastMap::default();
        let mut datum_ids: FastMap<&str, u64> = FastMap::default();
        for datum in &pmi.datums {
            let label = escape(&datum.label);
            let id = self.entity(format!("DATUM('','',#{pds},.F.,'{label}')"));
            for item in &datum.items {
                if let Some(&step_id) = by_node.get(item) {
                    self.entity(format!(
                        "GEOMETRIC_ITEM_SPECIFIC_USAGE('','',#{id},#{absr},#{step_id})"
                    ));
                }
            }
            datum_ids.insert(datum.label.as_str(), id);
            annotation_ids.insert(ogeom_doc::Annotated::Datum(datum_ids.len() - 1), id);
        }

        for (at, dimension) in pmi.dimensions.iter().enumerate() {
            let name = escape(&dimension.name);
            let angular = dimension.kind == ogeom_doc::MeasureKind::Angle;
            let dim = if dimension.location {
                // A location runs between two features; each keeps its own
                // aspect, so what was read as two ends writes as two ends.
                let empty = Vec::new();
                let first = dimension.features.first().unwrap_or(&empty);
                let second = dimension.features.get(1).unwrap_or(first);
                let a = aspect_for(self, first);
                let b = aspect_for(self, second);
                if angular {
                    self.entity(format!("ANGULAR_LOCATION('{name}','',#{a},#{b},.EQUAL.)"))
                } else {
                    self.entity(format!("DIMENSIONAL_LOCATION('{name}','',#{a},#{b})"))
                }
            } else {
                let empty = Vec::new();
                let items = dimension.features.first().unwrap_or(&empty);
                let aspect = aspect_for(self, items);
                if angular {
                    self.entity(format!("ANGULAR_SIZE(#{aspect},'{name}',.EQUAL.)"))
                } else {
                    self.entity(format!("DIMENSIONAL_SIZE(#{aspect},'{name}')"))
                }
            };
            let measures: Vec<String> = dimension
                .values
                .iter()
                .map(|&v| format!("#{}", self.measure(v, dimension.kind, lu, au)))
                .collect();
            let list = measures.join(",");
            annotation_ids.insert(ogeom_doc::Annotated::Dimension(at), dim);
            let sdr = self.entity(format!(
                "SHAPE_DIMENSION_REPRESENTATION('',({list}),#{gctx})"
            ));
            self.entity(format!(
                "DIMENSIONAL_CHARACTERISTIC_REPRESENTATION(#{dim},#{sdr})"
            ));
            if dimension.plus.is_some() || dimension.minus.is_some() {
                let lower = self.measure(dimension.minus.unwrap_or(0.0), dimension.kind, lu, au);
                let upper = self.measure(dimension.plus.unwrap_or(0.0), dimension.kind, lu, au);
                let tv = self.entity(format!("TOLERANCE_VALUE(#{lower},#{upper})"));
                self.entity(format!("PLUS_MINUS_TOLERANCE(#{tv},#{dim})"));
            }
        }

        for (at, tolerance) in pmi.tolerances.iter().enumerate() {
            let aspect = aspect_for(self, &tolerance.items);
            let name = escape(&tolerance.name);
            let magnitude =
                self.measure(tolerance.magnitude, ogeom_doc::MeasureKind::Length, lu, au);
            let keyword = format!("{}_TOLERANCE", tolerance.kind.to_uppercase());
            // A hyphen-joined label is a composite reference: its
            // constituent datums go through a compartment that binds them
            // into one.
            let refs: Vec<String> = tolerance
                .datums
                .iter()
                .filter_map(|d| {
                    if let Some(id) = datum_ids.get(d.as_str()) {
                        return Some(format!("#{id}"));
                    }
                    let parts: Vec<String> = d
                        .split('-')
                        .filter_map(|label| datum_ids.get(label))
                        .map(|i| format!("#{i}"))
                        .collect();
                    if parts.len() < 2 {
                        return None;
                    }
                    let list = parts.join(",");
                    let compartment = self.entity(format!(
                        "DATUM_REFERENCE_COMPARTMENT('','',#{pds},.F.,({list}),())"
                    ));
                    Some(format!("#{compartment}"))
                })
                .collect();
            let refs = refs.join(",");
            let written = if tolerance.modifiers.is_empty() {
                if refs.is_empty() {
                    self.entity(format!("{keyword}('{name}','',#{magnitude},#{aspect})"))
                } else {
                    self.entity(format!(
                        "{keyword}('{name}','',#{magnitude},#{aspect},({refs}))"
                    ))
                }
            } else {
                // Modifiers force the complex form: the shared attributes
                // sit on the GEOMETRIC_TOLERANCE part, the datum list and
                // the modifier words on their own parts, the subtype empty.
                let words: Vec<String> = tolerance
                    .modifiers
                    .iter()
                    .map(|m| format!(".{}.", m.to_uppercase()))
                    .collect();
                let words = words.join(",");
                let mut parts = vec![
                    format!("GEOMETRIC_TOLERANCE('{name}','',#{magnitude},#{aspect})"),
                    format!("GEOMETRIC_TOLERANCE_WITH_MODIFIERS(({words}))"),
                    format!("{keyword}()"),
                ];
                if !refs.is_empty() {
                    parts.push(format!(
                        "GEOMETRIC_TOLERANCE_WITH_DATUM_REFERENCE(({refs}))"
                    ));
                }
                parts.sort();
                self.entity(format!("({})", parts.concat()))
            };
            annotation_ids.insert(ogeom_doc::Annotated::Tolerance(at), written);
        }

        // Datum targets: the pads a datum is established at. The identifier
        // is the letter and the number the drawing shows (`A1`), and the
        // placement and sizes go in a shape representation the target's own
        // property definition names, which is where the reader looks for them.
        for target in &pmi.targets {
            let identifier = escape(&target.identifier());
            let description = match target.kind {
                ogeom_doc::DatumTargetKind::Point => "point",
                ogeom_doc::DatumTargetKind::Line { .. } => "line",
                ogeom_doc::DatumTargetKind::Rectangle { .. } => "rectangle",
                ogeom_doc::DatumTargetKind::Circle { .. } => "circle",
            };
            let id = self.entity(format!(
                "PLACED_DATUM_TARGET_FEATURE('','{description}',#{pds},.F.,'{identifier}')"
            ));
            for item in &target.items {
                if let Some(&step_id) = by_node.get(item) {
                    self.entity(format!(
                        "GEOMETRIC_ITEM_SPECIFIC_USAGE('','',#{id},#{absr},#{step_id})"
                    ));
                }
            }
            // Tie the target to its own datum, so a reader that meets the
            // target first can still say which datum it establishes.
            if let Some(&datum_id) = datum_ids.get(target.datum.as_str()) {
                self.entity(format!(
                    "SHAPE_ASPECT_RELATIONSHIP('','',#{datum_id},#{id})"
                ));
            }
            let frame = target
                .frame
                .unwrap_or_else(|| Frame::about(target.at, ogeom_math::Direction::Z));
            let placement = self.frame(&frame);
            let sizes: Vec<f64> = match target.kind {
                ogeom_doc::DatumTargetKind::Point => Vec::new(),
                ogeom_doc::DatumTargetKind::Line { length } => vec![length],
                ogeom_doc::DatumTargetKind::Rectangle { length, width } => vec![length, width],
                ogeom_doc::DatumTargetKind::Circle { diameter } => vec![diameter],
            };
            let mut items = vec![format!("#{placement}")];
            for size in sizes {
                let measure = self.measure(size, ogeom_doc::MeasureKind::Length, lu, au);
                items.push(format!("#{measure}"));
            }
            let list = items.join(",");
            let rep = self.entity(format!(
                "SHAPE_REPRESENTATION('{identifier}',({list}),#{gctx})"
            ));
            let property = self.entity(format!("PROPERTY_DEFINITION('','',#{id})"));
            self.entity(format!(
                "SHAPE_DEFINITION_REPRESENTATION(#{property},#{rep})"
            ));
        }

        // Presentation: the drawn annotations. One curve set per callout over
        // one coordinates list, held by an occurrence, held by the callout;
        // the plane it is drawn in; and the association that says which
        // semantic annotation it is a picture of.
        let mut callout_ids: Vec<u64> = Vec::new();
        if !pmi.callouts.is_empty() {
            let mut drawn: Vec<(u64, ogeom_doc::Annotated)> = Vec::new();
            let mut planes: Vec<String> = Vec::new();
            for callout in &pmi.callouts {
                let name = escape(&callout.name);
                let mut coordinates: Vec<String> = Vec::new();
                let mut lines: Vec<String> = Vec::new();
                let mut next = 1_usize;
                for polyline in &callout.polylines {
                    let mut indices: Vec<String> = Vec::with_capacity(polyline.len());
                    for p in polyline {
                        coordinates.push(format!("({},{},{})", real(p.x), real(p.y), real(p.z)));
                        indices.push(next.to_string());
                        next += 1;
                    }
                    lines.push(format!("({})", indices.join(",")));
                }
                let count = coordinates.len();
                let coordinates = coordinates.join(",");
                let list = self.entity(format!(
                    "COORDINATES_LIST('{name}',{count},({coordinates}))"
                ));
                let lines = lines.join(",");
                let set = self.entity(format!("TESSELLATED_CURVE_SET('{name}',#{list},({lines}))"));
                // No style is written, and that is a statement rather than an
                // omission: a style is about rendering, and this kernel keeps
                // no draughting style model to have one from.
                let occurrence = self.entity(format!(
                    "TESSELLATED_ANNOTATION_OCCURRENCE('{name}',(),#{set})"
                ));
                let id = self.entity(format!("DRAUGHTING_CALLOUT('{name}',(#{occurrence}))"));
                callout_ids.push(id);
                if let Some(frame) = callout.plane {
                    let placement = self.frame(&frame);
                    let plane = self.entity(format!("PLANE('{name}',#{placement})"));
                    planes.push(
                        self.entity(format!("ANNOTATION_PLANE('{name}',(),#{plane},(#{id}))"))
                            .to_string(),
                    );
                }
                if let Some(annotates) = callout.annotates {
                    drawn.push((id, annotates));
                }
            }
            let list: Vec<String> = planes.iter().map(|p| format!("#{p}")).collect();
            let list = list.join(",");
            let model = self.entity(format!("DRAUGHTING_MODEL('',({list}),#{gctx})"));
            for (callout, annotates) in drawn {
                let Some(&annotation) = annotation_ids.get(&annotates) else {
                    continue;
                };
                self.entity(format!(
                    "DRAUGHTING_MODEL_ITEM_ASSOCIATION('PMI representation to presentation \
                     link','',#{annotation},#{model},#{callout})"
                ));
            }
        }

        // Saved views: a named draughting model per view, holding a camera
        // and the callouts the view presents. The camera's view volume is
        // written `$`: this writer keeps no viewing frustum to state, and
        // the readers that matter (this one included) take the name and
        // the placement and leave the rest.
        for view in views {
            let name = escape(&view.name);
            let placement = self.frame(&view.frame);
            let camera = self.entity(format!("CAMERA_MODEL_D3('{name}',#{placement},$)"));
            let mut items = vec![format!("#{camera}")];
            for &index in &view.callouts {
                if let Some(id) = callout_ids.get(index) {
                    items.push(format!("#{id}"));
                }
            }
            let items = items.join(",");
            self.entity(format!("DRAUGHTING_MODEL('{name}',({items}),#{gctx})"));
        }
        Ok(())
    }

    /// A measure representation item, in the document's own units.
    fn measure(&mut self, value: f64, kind: ogeom_doc::MeasureKind, lu: u64, au: u64) -> u64 {
        let v = real(value);
        match kind {
            ogeom_doc::MeasureKind::Length => self.entity(format!(
                "(LENGTH_MEASURE_WITH_UNIT()MEASURE_REPRESENTATION_ITEM()MEASURE_WITH_UNIT(LENGTH_MEASURE({v}),#{lu})REPRESENTATION_ITEM(''))"
            )),
            ogeom_doc::MeasureKind::Angle => self.entity(format!(
                "(MEASURE_REPRESENTATION_ITEM()MEASURE_WITH_UNIT(PLANE_ANGLE_MEASURE({v}),#{au})PLANE_ANGLE_MEASURE_WITH_UNIT()REPRESENTATION_ITEM(''))"
            )),
        }
    }

    /// A styled item colouring one written entity.
    fn styled_item(&mut self, item: u64, colour: ogeom_doc::Colour) -> u64 {
        let rgb = self.entity(format!(
            "COLOUR_RGB('',{},{},{})",
            real(colour.r),
            real(colour.g),
            real(colour.b)
        ));
        let fasc = self.entity(format!("FILL_AREA_STYLE_COLOUR('',#{rgb})"));
        let fas = self.entity(format!("FILL_AREA_STYLE('',(#{fasc}))"));
        let ssfa = self.entity(format!("SURFACE_STYLE_FILL_AREA(#{fas})"));
        let sss = self.entity(format!("SURFACE_SIDE_STYLE('',(#{ssfa}))"));
        let ssu = self.entity(format!("SURFACE_STYLE_USAGE(.BOTH.,#{sss})"));
        let psa = self.entity(format!("PRESENTATION_STYLE_ASSIGNMENT((#{ssu}))"));
        self.entity(format!("STYLED_ITEM('',(#{psa}),#{item})"))
    }

    /// A styled item colouring one written edge: a continuous curve style
    /// of the conventional display width.
    fn curve_styled_item(&mut self, item: u64, colour: ogeom_doc::Colour) -> u64 {
        let font = match self.curve_font {
            Some(font) => font,
            None => {
                let font = self.entity("DRAUGHTING_PRE_DEFINED_CURVE_FONT('continuous')".into());
                self.curve_font = Some(font);
                font
            }
        };
        let rgb = self.entity(format!(
            "COLOUR_RGB('',{},{},{})",
            real(colour.r),
            real(colour.g),
            real(colour.b)
        ));
        let style = self.entity(format!(
            "CURVE_STYLE('',#{font},POSITIVE_LENGTH_MEASURE(0.1),#{rgb})"
        ));
        let psa = self.entity(format!("PRESENTATION_STYLE_ASSIGNMENT((#{style}))"));
        self.entity(format!("STYLED_ITEM('',(#{psa}),#{item})"))
    }
}

/// Whether a placement keeps every surface's parameters: no scale, no
/// mirror.
fn rigid(placement: &Transform) -> bool {
    (placement.scale_factor() - 1.0).abs() <= 1e-12 && placement.preserves_handedness()
}

/// Whether a curve goes out in this kernel's own parameterization: the
/// analytic spellings STEP parameterizes the same way, in right-handed
/// frames and running forward, a spline as itself, an offset of one of
/// these. Everything else goes out as a conversion with a parameter of its
/// own.
fn curve_states_parameter(curve: &Curve) -> bool {
    match curve {
        Curve::Line(_) | Curve::BSpline(_) => true,
        Curve::Circle(c) => {
            !c.is_reversed() && c.circle().frame().handedness() == Handedness::Right
        }
        Curve::Ellipse(e) => {
            !e.is_reversed() && e.ellipse().frame().handedness() == Handedness::Right
        }
        Curve::Hyperbola(h) => {
            !h.is_reversed() && h.hyperbola().frame().handedness() == Handedness::Right
        }
        Curve::Offset(o) => curve_states_parameter(o.basis()),
        _ => false,
    }
}

/// Whether a surface goes out in this kernel's own parameterization: the
/// elementary surfaces in right-handed frames, splines as themselves, and
/// sweeps and offsets of curves and surfaces that do.
fn surface_states_parameters(surface: &SurfaceGeometry) -> bool {
    let right = |frame: Frame| frame.handedness() == Handedness::Right;
    match surface {
        SurfaceGeometry::Plane(s) => right(s.plane().frame()),
        SurfaceGeometry::Cylinder(s) => right(s.cylinder().frame()),
        SurfaceGeometry::Cone(s) => right(s.cone().frame()),
        SurfaceGeometry::Sphere(s) => right(s.sphere().frame()),
        SurfaceGeometry::Torus(s) => right(s.torus().frame()),
        SurfaceGeometry::BSpline(_) => true,
        SurfaceGeometry::Revolution(r) => curve_states_parameter(r.curve()),
        SurfaceGeometry::Extrusion(e) => curve_states_parameter(e.curve()),
        SurfaceGeometry::Trimmed(t) => surface_states_parameters(t.basis()),
        SurfaceGeometry::Offset(o) => surface_states_parameters(o.basis()),
    }
}

/// Whether a pcurve has an exact spelling in the file over its edge's
/// curve's parameter, `pace` relating the two: a line or a spline at any
/// pace, a circle or ellipse running forward in a right-handed frame at
/// the curve's own, or a forward trim of one of these, which states its
/// basis.
fn planar_stated(curve: &PlanarCurve, pace: Pace) -> bool {
    match curve {
        PlanarCurve::Line(_) | PlanarCurve::BSpline(_) => true,
        PlanarCurve::Circle(c) => {
            pace == Pace::SAME
                && !c.is_reversed()
                && c.circle().frame().handedness() == Handedness::Right
        }
        PlanarCurve::Ellipse(e) => {
            pace == Pace::SAME
                && !e.is_reversed()
                && e.ellipse().frame().handedness() == Handedness::Right
        }
        PlanarCurve::Trimmed(t) => !t.is_reversed() && planar_stated(t.basis(), pace),
        PlanarCurve::Offset(_) | PlanarCurve::Trig(_) => false,
    }
}

/// A Part 21 real: shortest round-trip form, decimal point guaranteed,
/// exponent uppercased.
fn real(v: f64) -> String {
    if !v.is_finite() {
        NON_FINITE.with(|seen| seen.set(true));
        return "0.0".into();
    }
    let mut s = format!("{v:?}");
    if let Some(e) = s.find(['e', 'E']) {
        let (mantissa, exponent) = s.split_at(e);
        let mut m = mantissa.to_string();
        if !m.contains('.') {
            m.push_str(".0");
        }
        s = format!("{m}E{}", &exponent[1..]);
    } else if !s.contains('.') {
        s.push_str(".0");
    }
    s
}

/// Entity ids as a Part 21 list's contents: `#1,#2,#3`.
fn reference_list(ids: &[u64]) -> String {
    ids.iter()
        .map(|i| format!("#{i}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// A string literal's body, quotes doubled per Part 21.
fn escape(s: &str) -> String {
    // Part 21 strings are ASCII: a quote is doubled, a backslash doubled,
    // and every run of characters past ASCII written as UTF-16 in hex
    // between `\X2\` and `\X0\`.
    let mut out = String::with_capacity(s.len());
    let mut wide: Vec<u16> = Vec::new();
    let flush = |wide: &mut Vec<u16>, out: &mut String| {
        if wide.is_empty() {
            return;
        }
        out.push_str("\\X2\\");
        for unit in wide.drain(..) {
            out.push_str(&format!("{unit:04X}"));
        }
        out.push_str("\\X0\\");
    };
    for c in s.chars() {
        if c.is_ascii() {
            flush(&mut wide, &mut out);
            match c {
                '\'' => out.push_str("''"),
                '\\' => out.push_str("\\\\"),
                _ => out.push(c),
            }
        } else {
            let mut units = [0_u16; 2];
            wide.extend_from_slice(c.encode_utf16(&mut units));
        }
    }
    flush(&mut wide, &mut out);
    out
}

/// Knots as STEP states them: distinct values with multiplicities.
fn compress_knots(knots: &[f64]) -> (Vec<usize>, Vec<f64>) {
    let mut mults = Vec::new();
    let mut values = Vec::new();
    for &k in knots {
        match values.last() {
            Some(&last) if k == last => {
                if let Some(m) = mults.last_mut() {
                    *m += 1;
                }
            }
            _ => {
                values.push(k);
                mults.push(1);
            }
        }
    }
    (mults, values)
}

/// A rigid transform quantized to bits, for per-placement deduplication.
fn transform_bits(t: &Transform) -> [u64; 3] {
    let p = t.apply(Point::new(0.123_456_789, 9.87, -3.21));
    [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]
}

/// A location resolved to the rigid transform it composes to.
fn location_transform(location: &ogeom_topo::Location, model: &Model) -> OgeomResult<Transform> {
    let mut out = Transform::IDENTITY;
    for &(datum, power) in location.chain() {
        let Some(t) = model.datums().get(datum) else {
            ogeom_bail!(Dangling, "an instance placement names a missing datum");
        };
        let step = if power >= 0 { t } else { t.inverse()? };
        for _ in 0..power.unsigned_abs() {
            out = out * step;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod escape_tests {
    /// Names past ASCII, quotes and backslashes survive a write and a read.
    #[test]
    fn strings_round_trip_through_part_21_escapes() {
        for name in [
            "Welle \u{f8}10",
            "\u{65e5}\u{672c}",
            "it's",
            "back\\slash",
            "emoji \u{1f600} end",
        ] {
            let written = super::escape(name);
            assert!(written.is_ascii(), "{written}");
            let read = super::super::parse::decode_escapes(&written.replace("''", "'"));
            assert_eq!(read, name, "{written}");
        }
        // Escapes as other exporters write them.
        assert_eq!(super::super::parse::decode_escapes("\\X\\F8"), "\u{f8}");
        assert_eq!(super::super::parse::decode_escapes("a\\S\\xb"), "a\u{f8}b");
        assert_eq!(super::super::parse::decode_escapes("\\PA\\ok"), "ok");
    }
}
