//! LightBurn vector geometry, independently of the saved Thumbnail image.
//!
//! Coordinates are millimetres in the drawing plane. Classic V/P nodes and the
//! compact VertList/PrimList encoding describe the same cubic Bezier geometry.
//! This reader produces vector outlines, rather than simulating laser fills.
use crate::scene::{Line, Scene, Vec3};
use crate::{Error, Result};
use roxmltree::{Node, NodeId};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

/// Maximum XML input size accepted by the vector geometry reader.
const MAX_BYTES: usize = 64 * 1024 * 1024;
/// Maximum shared entries, XML nodes, shapes or emitted line segments.
const MAX_ITEMS: usize = 1_000_000;
/// Maximum supported nested group depth.
const MAX_DEPTH: usize = 32;

/// Convert a contextual parsing message into the crate error type.
fn error(message: impl Into<String>) -> Error {
    message.into().into()
}

/// A path endpoint and its optional outgoing and incoming cubic control coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Vertex {
    /// Drawing-plane endpoint before its shape transform.
    position: Vec3,
    /// Optional c0 X/Y coordinates used when this vertex starts a cubic.
    outgoing: [Option<f64>; 2],
    /// Optional c1 X/Y coordinates used when this vertex ends a cubic.
    incoming: [Option<f64>; 2],
}

/// One line or cubic Bezier segment referencing two path vertices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Primitive {
    /// Whether this segment uses cubic controls instead of a straight line.
    bezier: bool,
    /// Index of the first endpoint in the resolved vertex resource.
    start: usize,
    /// Index of the final endpoint in the resolved vertex resource.
    end: usize,
}

/// Explicit path segments or a shortcut connecting successive vertices.
#[derive(Clone, Debug, PartialEq)]
enum Primitives {
    /// Individually indexed line and cubic segments.
    Explicit(Vec<Primitive>),
    /// Connect consecutive vertices, optionally closing the final endpoint to the first.
    Lines {
        /// Whether the final vertex connects back to the first.
        closed: bool,
    },
}

/// A planar affine transform stored as `[a, b, c, d, tx, ty]`.
#[derive(Clone, Copy)]
struct Matrix([f64; 6]);

impl Matrix {
    /// Planar affine transform that leaves coordinates unchanged.
    const IDENTITY: Self = Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    /// Apply the affine transform to a drawing-plane point and retain zero Z.
    fn point(self, point: Vec3) -> Vec3 {
        let [a, b, c, d, tx, ty] = self.0;
        Vec3::new(
            a * point.x + c * point.y + tx,
            b * point.x + d * point.y + ty,
            0.0,
        )
    }

    /// Compose parent and child affine transforms and reject coefficient overflow.
    fn then_child(self, child: Self) -> Result<Self> {
        let parent = self.0;
        let local = child.0;
        let result = Self([
            parent[0] * local[0] + parent[2] * local[1],
            parent[1] * local[0] + parent[3] * local[1],
            parent[0] * local[2] + parent[2] * local[3],
            parent[1] * local[2] + parent[3] * local[3],
            parent[0] * local[4] + parent[2] * local[5] + parent[4],
            parent[1] * local[4] + parent[3] * local[5] + parent[5],
        ]);
        if result.0.iter().any(|value| !value.is_finite()) {
            return Err(error("LightBurn transform overflow"));
        }
        Ok(result)
    }
}

/// Find a unique direct child element, rejecting duplicate geometry definitions.
fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Result<Option<Node<'a, 'input>>> {
    let mut matching = node.children().filter(|n| n.has_tag_name(name));
    let value = matching.next();
    if matching.next().is_some() {
        return Err(error(format!("duplicate LightBurn {name} element")));
    }
    Ok(value)
}

/// Parse a finite drawing coordinate or affine coefficient.
fn number(text: &str) -> Result<f64> {
    let value: f64 = text.parse()?;
    if !value.is_finite() {
        return Err(error("non-finite LightBurn coordinate"));
    }
    Ok(value)
}

/// Read a finite numeric attribute or its caller-provided default.
fn attribute_number(node: Node<'_, '_>, name: &str, default: Option<f64>) -> Result<f64> {
    node.attribute(name).map_or_else(
        || default.ok_or_else(|| error(format!("LightBurn shape is missing {name}"))),
        number,
    )
}

/// Parse the boolean spellings used by `LightBurn` project attributes.
fn boolean(text: &str) -> Result<bool> {
    match text {
        "True" | "true" | "1" => Ok(true),
        "False" | "false" | "0" => Ok(false),
        _ => Err(error("invalid LightBurn boolean")),
    }
}

/// Read an optional six-coefficient `XForm`, using identity when it is absent.
fn matrix(node: Node<'_, '_>) -> Result<Matrix> {
    let Some(node) = child(node, "XForm")? else {
        return Ok(Matrix::IDENTITY);
    };
    let values: Vec<f64> = node
        .text()
        .unwrap_or_default()
        .split_whitespace()
        .map(number)
        .collect::<Result<_>>()?;
    let values: [f64; 6] = values
        .try_into()
        .map_err(|_| error("LightBurn XForm must contain six values"))?;
    Ok(Matrix(values))
}

/// A compact-data scanner that rejects unknown tokens instead of skipping them.
struct Tokens<'a>(&'a str);

impl Tokens<'_> {
    /// Discard leading whitespace before scanning the next compact token.
    fn whitespace(&mut self) {
        self.0 = self.0.trim_start();
    }

    /// Consume an expected compact-data prefix after optional whitespace.
    fn take(&mut self, token: &str) -> bool {
        self.whitespace();
        if let Some(rest) = self.0.strip_prefix(token) {
            self.0 = rest;
            true
        } else {
            false
        }
    }

    /// Consume one finite numeric token from compact geometry data.
    fn number(&mut self) -> Result<f64> {
        self.whitespace();
        let end = self
            .0
            .find(|c: char| !(c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E')))
            .unwrap_or(self.0.len());
        if end == 0 {
            return Err(error("missing LightBurn compact coordinate"));
        }
        let value = number(&self.0[..end])?;
        self.0 = &self.0[end..];
        Ok(value)
    }

    /// Consume a nonnegative decimal vertex index from compact data.
    fn index(&mut self) -> Result<usize> {
        self.whitespace();
        let end = self
            .0
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(self.0.len());
        if end == 0 {
            return Err(error("missing LightBurn primitive vertex index"));
        }
        let value = self.0[..end].parse()?;
        self.0 = &self.0[end..];
        Ok(value)
    }

    /// Check whether compact data contains only trailing whitespace.
    fn empty(&mut self) -> bool {
        self.whitespace();
        self.0.is_empty()
    }
}

/// Resolve a used Bezier handle, accepting absent coordinates or the native absent-handle sentinel.
fn handle(position: Vec3, x: Option<f64>, y: Option<f64>) -> Result<Vec3> {
    match (x, y) {
        (Some(x), Some(y)) => Ok(Vec3::new(x, y, 0.0)),
        (None | Some(1.0), None) => Ok(position),
        _ => Err(error("incomplete LightBurn Bezier control handle")),
    }
}

/// Decode classic or compact path vertices and their optional editing control coordinates.
fn vertices(node: Node<'_, '_>) -> Result<Option<Vec<Vertex>>> {
    let compact = child(node, "VertList")?;
    let classic = node.children().any(|n| n.has_tag_name("V"));
    if compact.is_some() && classic {
        return Err(error("ambiguous LightBurn classic and compact vertices"));
    }
    if compact.is_none() && !classic {
        return Ok(None);
    }
    let mut result = Vec::new();
    if let Some(compact) = compact {
        if compact.children().any(|n| n.is_element()) {
            return Err(error("unexpected XML inside LightBurn VertList"));
        }
        let mut tokens = Tokens(compact.text().unwrap_or_default());
        while !tokens.empty() {
            if !tokens.take("V") {
                return Err(error("invalid LightBurn compact vertex token"));
            }
            let position = Vec3::new(tokens.number()?, tokens.number()?, 0.0);
            let mut controls = [None; 4];
            loop {
                let mut matched = false;
                for (index, token) in ["c0x", "c0y", "c1x", "c1y"].into_iter().enumerate() {
                    if tokens.take(token) {
                        if controls[index].replace(tokens.number()?).is_some() {
                            return Err(error("duplicate LightBurn control coordinate"));
                        }
                        matched = true;
                        break;
                    }
                }
                if !matched {
                    break;
                }
            }
            tokens.take("S"); // Smooth-node editing flag; control coordinates are authoritative.
            result.push(Vertex {
                position,
                outgoing: [controls[0], controls[1]],
                incoming: [controls[2], controls[3]],
            });
            if result.len() > MAX_ITEMS {
                return Err(error("too many LightBurn vertices"));
            }
        }
    } else {
        for node in node.children().filter(|n| n.has_tag_name("V")) {
            let position = Vec3::new(
                attribute_number(node, "vx", None)?,
                attribute_number(node, "vy", None)?,
                0.0,
            );
            let coordinate = |name| node.attribute(name).map(number).transpose();
            result.push(Vertex {
                position,
                outgoing: [coordinate("c0x")?, coordinate("c0y")?],
                incoming: [coordinate("c1x")?, coordinate("c1y")?],
            });
            if result.len() > MAX_ITEMS {
                return Err(error("too many LightBurn vertices"));
            }
        }
    }
    if result.len() < 2 {
        return Err(error("LightBurn path requires at least two vertices"));
    }
    Ok(Some(result))
}

/// Decode classic or compact segments, including open and closed line shortcuts.
fn primitives(node: Node<'_, '_>) -> Result<Option<Primitives>> {
    let compact = child(node, "PrimList")?;
    let classic = node.children().any(|n| n.has_tag_name("P"));
    if compact.is_some() && classic {
        return Err(error("ambiguous LightBurn classic and compact primitives"));
    }
    if compact.is_none() && !classic {
        return Ok(None);
    }
    let mut result = Vec::new();
    if let Some(compact) = compact {
        if compact.children().any(|n| n.is_element()) {
            return Err(error("unexpected XML inside LightBurn PrimList"));
        }
        let text = compact.text().unwrap_or_default().trim();
        if matches!(text, "LineClosed" | "LineOpen") {
            return Ok(Some(Primitives::Lines {
                closed: text == "LineClosed",
            }));
        }
        let mut tokens = Tokens(text);
        while !tokens.empty() {
            let bezier = if tokens.take("L") {
                false
            } else if tokens.take("B") {
                true
            } else {
                return Err(error("unsupported LightBurn compact primitive"));
            };
            result.push(Primitive {
                bezier,
                start: tokens.index()?,
                end: tokens.index()?,
            });
            if result.len() > MAX_ITEMS {
                return Err(error("too many LightBurn primitives"));
            }
        }
    } else {
        for node in node.children().filter(|n| n.has_tag_name("P")) {
            let bezier = match node.attribute("T") {
                Some("L") => false,
                Some("B") => true,
                _ => return Err(error("unsupported LightBurn classic primitive")),
            };
            let index = |name| -> Result<usize> {
                Ok(node
                    .attribute(name)
                    .ok_or("LightBurn primitive has no vertex index")?
                    .parse()?)
            };
            result.push(Primitive {
                bezier,
                start: index("p0")?,
                end: index("p1")?,
            });
        }
    }
    if result.is_empty() {
        return Err(error("LightBurn path has no drawing primitives"));
    }
    Ok(Some(Primitives::Explicit(result)))
}

/// Read an optional unsigned shared-resource or cut-layer identifier.
fn resource_id(node: Node<'_, '_>, attribute: &str) -> Result<Option<u32>> {
    node.attribute(attribute)
        .map(|v| v.parse().map_err(Into::into))
        .transpose()
}

/// Shared vector resources and bounded geometry accumulated from the project hierarchy.
#[derive(Default)]
struct Reader {
    /// Colored vector outlines emitted from completed shape visits.
    scene: Scene,
    /// Vertex definitions in the independent `VertID` namespace.
    vertex_resources: HashMap<u32, Vec<Vertex>>,
    /// Segment definitions in the independent `PrimID` namespace.
    primitive_resources: HashMap<u32, Primitives>,
    /// Shape XML node identifiers reached through supported containers.
    visited: HashSet<NodeId>,
    /// Number of visited shapes, including groups, for the geometry budget.
    shapes: usize,
}

/// Register a shared definition, rejecting conflicting reuse of the same identifier.
fn insert_resource<T: PartialEq>(map: &mut HashMap<u32, T>, id: u32, value: T) -> Result<()> {
    if let Some(previous) = map.get(&id) {
        if previous != &value {
            return Err(error("conflicting LightBurn shared geometry definitions"));
        }
    } else {
        map.insert(id, value);
    }
    Ok(())
}

impl Reader {
    /// Append a finite colored segment within the output budget, omitting zero-length segments.
    fn line(&mut self, start: Vec3, end: Vec3, color: [u8; 3]) -> Result<()> {
        if !start.is_finite() || !end.is_finite() {
            return Err(error("LightBurn transformed coordinate overflow"));
        }
        if start == end {
            return Ok(());
        }
        if self.scene.lines.len() >= MAX_ITEMS {
            return Err(error("LightBurn output exceeds the line segment limit"));
        }
        self.scene.lines.push(Line {
            vertices: [start, end],
            color: Some(color),
        });
        Ok(())
    }

    /// Set a scale-relative error tolerance and flatten a transformed cubic Bezier curve.
    fn bezier(&mut self, curve: [Vec3; 4], color: [u8; 3]) -> Result<()> {
        let scale = curve
            .iter()
            .map(|p| (*p - curve[0]).length())
            .fold(0.0, f64::max);
        if !scale.is_finite() {
            return Err(error("LightBurn curve extent overflow"));
        }
        self.flatten(curve, (scale * 0.0005).max(1e-10), color, 0)
    }

    /// Subdivide a cubic until its control polygon is flat and monotone along the chord.
    fn flatten(
        &mut self,
        curve: [Vec3; 4],
        tolerance: f64,
        color: [u8; 3],
        depth: usize,
    ) -> Result<()> {
        let [start, control_start, control_end, end] = curve;
        let chord = end - start;
        let length = chord.length();
        // Also require a monotone control polygon along the chord. A collinear
        // curve can overshoot or loop, and must not collapse to its endpoints.
        let flat = if length > 0.0 {
            let direction = chord / length;
            let projection_start = (control_start - start).dot(direction);
            let projection_end = (control_end - start).dot(direction);
            (control_start - start).cross(direction).length() <= tolerance
                && (control_end - start).cross(direction).length() <= tolerance
                && projection_start >= -tolerance
                && projection_end >= projection_start - tolerance
                && projection_end <= length + tolerance
        } else {
            (control_start - start)
                .length()
                .max((control_end - start).length())
                <= tolerance
        };
        if flat {
            return self.line(start, end, color);
        }
        if depth >= 24 {
            return Err(error("LightBurn Bezier subdivision limit exceeded"));
        }
        let ab = (start + control_start) * 0.5;
        let bc = (control_start + control_end) * 0.5;
        let cd = (control_end + end) * 0.5;
        let left = (ab + bc) * 0.5;
        let right = (bc + cd) * 0.5;
        let middle = (left + right) * 0.5;
        self.flatten([start, ab, left, middle], tolerance, color, depth + 1)?;
        self.flatten([middle, right, cd, end], tolerance, color, depth + 1)
    }

    /// Resolve local or shared path data and emit transformed lines and cubic outlines.
    fn path(&mut self, node: Node<'_, '_>, matrix: Matrix, color: [u8; 3]) -> Result<()> {
        let vertices = if let Some(vertices) = vertices(node)? {
            vertices
        } else {
            let id =
                resource_id(node, "VertID")?.ok_or("LightBurn path has no vertices or VertID")?;
            self.vertex_resources
                .get(&id)
                .ok_or("unresolved LightBurn VertID")?
                .clone()
        };
        let primitives = if let Some(primitives) = primitives(node)? {
            primitives
        } else {
            let id =
                resource_id(node, "PrimID")?.ok_or("LightBurn path has no primitives or PrimID")?;
            self.primitive_resources
                .get(&id)
                .ok_or("unresolved LightBurn PrimID")?
                .clone()
        };
        let explicit = match primitives {
            Primitives::Explicit(primitives) => primitives,
            Primitives::Lines { closed } => {
                let count = vertices.len() - usize::from(!closed);
                (0..count)
                    .map(|start| Primitive {
                        bezier: false,
                        start,
                        end: (start + 1) % vertices.len(),
                    })
                    .collect()
            }
        };
        for primitive in explicit {
            let start = vertices
                .get(primitive.start)
                .ok_or("LightBurn primitive index is out of range")?;
            let end = vertices
                .get(primitive.end)
                .ok_or("LightBurn primitive index is out of range")?;
            if primitive.bezier {
                // Native projects can retain incomplete, unused handles on a
                // straight segment. Only handles actually used by a Bezier
                // must resolve to a point (or the absent-handle sentinel).
                self.bezier(
                    [
                        matrix.point(start.position),
                        matrix.point(handle(
                            start.position,
                            start.outgoing[0],
                            start.outgoing[1],
                        )?),
                        matrix.point(handle(end.position, end.incoming[0], end.incoming[1])?),
                        matrix.point(end.position),
                    ],
                    color,
                )?;
            } else {
                self.line(
                    matrix.point(start.position),
                    matrix.point(end.position),
                    color,
                )?;
            }
        }
        Ok(())
    }

    /// Visit a supported shape or group with inherited cut color and composed transforms.
    #[allow(
        clippy::too_many_lines,
        clippy::cast_precision_loss,
        reason = "Keep shape-specific sampling and shared hierarchy checks together; ellipse indices are reduced modulo 128 before exact f64 conversion."
    )]
    fn shape(
        &mut self,
        node: Node<'_, '_>,
        parent: Matrix,
        inherited_layer: u32,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(error("LightBurn group nesting limit exceeded"));
        }
        self.shapes += 1;
        if self.shapes > MAX_ITEMS {
            return Err(error("too many LightBurn shapes"));
        }
        self.visited.insert(node.id());
        let kind = node
            .attribute("Type")
            .ok_or("LightBurn Shape has no Type")?;
        let layer = resource_id(node, "CutIndex")?.unwrap_or(inherited_layer);
        let color = layer_color(layer);
        let transform = parent.then_child(matrix(node)?)?;
        match kind {
            "Group" => {
                if let Some(children) = child(node, "Children")? {
                    for shape in children.children().filter(Node::is_element) {
                        if !shape.has_tag_name("Shape") {
                            return Err(error("unsupported LightBurn group child"));
                        }
                        self.shape(shape, transform, layer, depth + 1)?;
                    }
                }
            }
            "Path" => self.path(node, transform, color)?,
            "Rect" => {
                let width = attribute_number(node, "W", None)?;
                let height = attribute_number(node, "H", None)?;
                let radius = attribute_number(node, "Cr", Some(0.0))?;
                if width <= 0.0 || height <= 0.0 || radius < 0.0 {
                    return Err(error("invalid LightBurn rectangle dimensions"));
                }
                let radius = radius.min(width * 0.5).min(height * 0.5);
                let x = width * 0.5;
                let y = height * 0.5;
                if radius == 0.0 {
                    let corners = [
                        Vec3::new(-x, -y, 0.0),
                        Vec3::new(x, -y, 0.0),
                        Vec3::new(x, y, 0.0),
                        Vec3::new(-x, y, 0.0),
                    ];
                    for index in 0..4 {
                        self.line(
                            transform.point(corners[index]),
                            transform.point(corners[(index + 1) % 4]),
                            color,
                        )?;
                    }
                } else {
                    let mut outline = Vec::with_capacity(132);
                    for (center, begin) in [
                        (Vec3::new(x - radius, y - radius, 0.0), 0.0),
                        (
                            Vec3::new(-x + radius, y - radius, 0.0),
                            std::f64::consts::FRAC_PI_2,
                        ),
                        (
                            Vec3::new(-x + radius, -y + radius, 0.0),
                            std::f64::consts::PI,
                        ),
                        (
                            Vec3::new(x - radius, -y + radius, 0.0),
                            3.0 * std::f64::consts::FRAC_PI_2,
                        ),
                    ] {
                        for step in 0..=32 {
                            let angle =
                                begin + f64::from(step) * std::f64::consts::FRAC_PI_2 / 32.0;
                            outline.push(transform.point(
                                center + Vec3::new(radius * angle.cos(), radius * angle.sin(), 0.0),
                            ));
                        }
                    }
                    for index in 0..outline.len() {
                        self.line(outline[index], outline[(index + 1) % outline.len()], color)?;
                    }
                }
            }
            "Ellipse" => {
                let rx = attribute_number(node, "Rx", None)?;
                let ry = attribute_number(node, "Ry", None)?;
                if rx <= 0.0 || ry <= 0.0 {
                    return Err(error("invalid LightBurn ellipse radii"));
                }
                let point = |index: usize| {
                    let angle = (index % 128) as f64 * std::f64::consts::TAU / 128.0;
                    transform.point(Vec3::new(rx * angle.cos(), ry * angle.sin(), 0.0))
                };
                for index in 0..128 {
                    self.line(point(index), point(index + 1), color)?;
                }
            }
            "Text" => {
                return Err(error(
                    "LightBurn Text requires font shaping; export text as vector paths to render geometry",
                ));
            }
            "Bitmap" => {
                return Err(error(
                    "LightBurn Bitmap composition is not supported by the vector geometry reader",
                ));
            }
            other => return Err(error(format!("unsupported LightBurn shape type {other}"))),
        }
        Ok(())
    }
}

/// Use the standard cut-layer color, falling back to black for unqualified palette entries.
fn layer_color(index: u32) -> [u8; 3] {
    /// The first sixteen standard `LightBurn` cut-layer display colors.
    const COLORS: [[u8; 3]; 16] = [
        [0, 0, 0],
        [0, 0, 255],
        [255, 0, 0],
        [0, 255, 0],
        [255, 255, 0],
        [255, 128, 0],
        [0, 255, 255],
        [255, 0, 255],
        [192, 192, 192],
        [128, 128, 128],
        [128, 0, 0],
        [0, 128, 0],
        [0, 0, 128],
        [128, 128, 0],
        [128, 0, 128],
        [0, 128, 128],
    ];
    COLORS.get(index as usize).copied().unwrap_or([0, 0, 0])
}

/// Read a `LightBurn` project and emit supported vector outlines.
///
/// # Errors
/// Returns an error for unreadable or oversized files, malformed XML or compact
/// data, unresolved or conflicting resources, invalid coordinates or transforms,
/// unsupported shapes or containers, excessive nesting or empty output geometry.
pub(crate) fn load(path: &Path) -> Result<Scene> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    parse(&bytes)
}

/// Validate project XML, resolve shared geometry and traverse one supported drawing hierarchy.
#[allow(
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    reason = "Validate XML and its shared resources before hierarchy emission; the fixed one-million node budget fits u32."
)]
fn parse(bytes: &[u8]) -> Result<Scene> {
    if bytes.len() > MAX_BYTES {
        return Err(error("LightBurn geometry input exceeds 64 MiB"));
    }
    let text = std::str::from_utf8(bytes)?;
    let document = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: MAX_ITEMS as u32,
            ..roxmltree::ParsingOptions::default()
        },
    )?;
    let root = document.root_element();
    if !root.has_tag_name("LightBurnProject") || root.tag_name().namespace().is_some() {
        return Err(error("expected an unnamespaced LightBurnProject XML root"));
    }
    if !matches!(root.attribute("FormatVersion"), None | Some("0" | "1")) {
        return Err(error("unsupported LightBurn geometry FormatVersion"));
    }
    if root
        .descendants()
        .any(|n| n.is_element() && n.tag_name().namespace().is_some())
    {
        return Err(error("namespaced LightBurn geometry is not supported"));
    }
    let mut reader = Reader::default();
    // Resolve shared resources first, including definitions later in the file.
    // VertID and PrimID are separate namespaces, not object identifiers.
    let mut total_vertices = 0_usize;
    let mut total_primitives = 0_usize;
    for node in root.descendants().filter(|n| n.has_tag_name("Shape")) {
        if let Some(id) = resource_id(node, "VertID")?
            && let Some(vertices) = vertices(node)?
        {
            total_vertices += vertices.len();
            if total_vertices > MAX_ITEMS {
                return Err(error("LightBurn shared geometry exceeds the vertex limit"));
            }
            insert_resource(&mut reader.vertex_resources, id, vertices)?;
        }
        if let Some(id) = resource_id(node, "PrimID")?
            && let Some(primitives) = primitives(node)?
        {
            total_primitives += match &primitives {
                Primitives::Explicit(values) => values.len(),
                Primitives::Lines { .. } => 1,
            };
            if total_primitives > MAX_ITEMS {
                return Err(error(
                    "LightBurn shared geometry exceeds the primitive limit",
                ));
            }
            insert_resource(&mut reader.primitive_resources, id, primitives)?;
        }
    }
    let mirror_x = root
        .attribute("MirrorX")
        .map(boolean)
        .transpose()?
        .unwrap_or(false);
    let mirror_y = root
        .attribute("MirrorY")
        .map(boolean)
        .transpose()?
        .unwrap_or(false);
    // LightBurn XML uses screen Y down; the Scene's XY drawing plane uses Y up.
    let origin = Matrix([
        if mirror_x { -1.0 } else { 1.0 },
        0.0,
        0.0,
        if mirror_y { 1.0 } else { -1.0 },
        0.0,
        0.0,
    ]);
    for shape in root.children().filter(|n| n.has_tag_name("Shape")) {
        reader.shape(shape, origin, 0, 0)?;
    }
    if root
        .descendants()
        .any(|node| node.has_tag_name("Shape") && !reader.visited.contains(&node.id()))
    {
        return Err(error(
            "unsupported LightBurn shape container (including multi-page projects)",
        ));
    }
    reader.scene.validate()?;
    Ok(reader.scene)
}

/// Vector encoding, transform and malformed-project qualification.
#[cfg(test)]
mod tests {
    use super::*;

    /// Wrap authored shapes in a minimal project with a Cartesian-compatible mirrored origin.
    fn project(content: &str) -> Vec<u8> {
        format!(
            "<LightBurnProject FormatVersion=\"1\" MirrorY=\"True\">{content}</LightBurnProject>"
        )
        .into_bytes()
    }

    #[test]
    fn classic_and_compact_beziers_use_start_c0_end_c1_and_same_geometry() {
        let classic = project(
            r#"<Shape Type="Path" CutIndex="2">
            <V vx="0" vy="0" c0x="0" c0y="8" c1x="-20" c1y="-20"/>
            <V vx="8" vy="0" c1x="8" c1y="8" c0x="20" c0y="-20"/>
            <P T="B" p0="0" p1="1"/></Shape>"#,
        );
        let compact = project(
            r#"<Shape Type="Path" CutIndex="2">
            <VertList>V0 0c0x0c0y8c1x-20c1y-20V8 0c0x20c0y-20c1x8c1y8S</VertList>
            <PrimList>B0 1</PrimList></Shape>"#,
        );
        let a = parse(&classic).unwrap();
        let b = parse(&compact).unwrap();
        assert!(a.lines.len() > 16);
        assert_eq!(a.lines.len(), b.lines.len());
        for (a, b) in a.lines.iter().zip(&b.lines) {
            assert_eq!(a.vertices, b.vertices);
            assert_eq!(a.color, Some([255, 0, 0]));
            assert_eq!(a.color, b.color);
        }
        assert_eq!(
            a.lines.first().unwrap().vertices[0],
            Vec3::new(0.0, 0.0, 0.0)
        );
        assert_eq!(
            a.lines.last().unwrap().vertices[1],
            Vec3::new(8.0, 0.0, 0.0)
        );
        assert!(a.positions().any(|p| p == Vec3::new(4.0, 6.0, 0.0)));
        assert_eq!(
            a.positions().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max),
            6.0
        );
    }

    #[test]
    fn parent_child_matrix_order_origin_reflection_and_layer_inheritance() {
        let bytes = br#"<LightBurnProject FormatVersion="0" MirrorX="True" MirrorY="False">
            <Shape Type="Group" CutIndex="1"><XForm>1 0 0 1 10 20</XForm><Children>
              <Shape Type="Path"><XForm>0 1 -1 0 3 4</XForm>
                <V vx="1" vy="2"/><V vx="3" vy="4"/><P T="L" p0="0" p1="1"/>
              </Shape></Children></Shape></LightBurnProject>"#;
        let scene = parse(bytes).unwrap();
        assert_eq!(scene.lines.len(), 1);
        assert_eq!(
            scene.lines[0].vertices,
            [Vec3::new(-11.0, -25.0, 0.0), Vec3::new(-9.0, -27.0, 0.0)]
        );
        assert_eq!(scene.lines[0].color, Some([0, 0, 255]));
    }

    #[test]
    fn shared_vertex_and_primitive_ids_are_independent_and_resolve_forward() {
        let scene = parse(&project(
            r#"
            <Shape Type="Path" VertID="7" PrimID="9"><XForm>1 0 0 1 20 0</XForm></Shape>
            <Shape Type="Path" VertID="7" PrimID="9">
              <VertList>V0 0c0x1c1x1V1 0c0x1c1x1V1 1c0x1c1x1</VertList>
              <PrimList>LineClosed</PrimList></Shape>
            <Shape Type="Path" VertID="7" PrimID="7">
              <PrimList>LineOpen</PrimList></Shape>"#,
        ))
        .unwrap();
        assert_eq!(scene.lines.len(), 8);
        assert_eq!(
            scene.lines[0].vertices,
            [Vec3::new(20.0, 0.0, 0.0), Vec3::new(21.0, 0.0, 0.0)]
        );
        assert_eq!(
            scene.lines[2].vertices,
            [Vec3::new(21.0, 1.0, 0.0), Vec3::new(20.0, 0.0, 0.0)]
        );
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "Exact sampled rectangle corner coordinates prove rounded corners were not replaced with sharp corners."
    )]
    fn rectangle_rounding_and_affine_ellipse_extents_are_actual_geometry() {
        let rectangle = parse(&project(r#"<Shape Type="Rect" W="10" H="6" Cr="1"/>"#)).unwrap();
        assert!(rectangle.lines.len() >= 128);
        assert!(
            !rectangle
                .positions()
                .any(|p| p.x.abs() == 5.0 && p.y.abs() == 3.0)
        );
        let min = rectangle
            .positions()
            .fold(Vec3::new(f64::INFINITY, f64::INFINITY, 0.0), |a, p| {
                Vec3::new(a.x.min(p.x), a.y.min(p.y), 0.0)
            });
        assert_eq!(min, Vec3::new(-5.0, -3.0, 0.0));
        let ellipse = parse(&project(
            r#"<Shape Type="Ellipse" Rx="2" Ry="3"><XForm>0 2 -1 0 10 20</XForm></Shape>"#,
        ))
        .unwrap();
        assert_eq!(ellipse.lines.len(), 128);
        assert!(ellipse.positions().any(|p| p == Vec3::new(10.0, 24.0, 0.0)));
        assert!(
            ellipse
                .positions()
                .any(|p| (p - Vec3::new(7.0, 20.0, 0.0)).length() < 1e-12)
        );
    }

    #[test]
    fn collinear_bezier_overshoot_and_closed_curve_do_not_collapse() {
        let curve = parse(&project(
            r#"<Shape Type="Path"><VertList>
            V0 0c0x10c0y0V1 0c1x-10c1y0</VertList><PrimList>B0 1</PrimList></Shape>"#,
        ))
        .unwrap();
        assert!(curve.lines.len() > 2);
        assert!(curve.positions().any(|p| p.x > 2.0));
        assert!(curve.positions().any(|p| p.x < -2.0));
        let curve = parse(&project(
            r#"<Shape Type="Path"><VertList>
            V0 0c0x10c0y10c1x-10c1y10V1 0</VertList><PrimList>B0 0</PrimList></Shape>"#,
        ))
        .unwrap();
        assert!(curve.lines.len() > 16);
        assert!(curve.positions().any(|p| p.y >= 7.5));
    }

    #[test]
    fn thumbnail_is_never_needed_or_decoded_for_geometry() {
        let content = r#"<Shape Type="Rect" W="10" H="10"/>"#;
        let plain = parse(&project(content)).unwrap();
        let poisoned = parse(&project(&format!(
            "<Thumbnail Source=\"not base64!\"/>{content}"
        )))
        .unwrap();
        assert_eq!(plain.lines.len(), 4);
        assert_eq!(plain.lines[0].vertices, poisoned.lines[0].vertices);
        let legacy = parse(br#"<LightBurnProject AppVersion="0.9.07"><Shape Type="Rect" W="1" H="2"/></LightBurnProject>"#).unwrap();
        assert_eq!(legacy.lines.len(), 4);
    }

    #[test]
    fn unused_editor_control_values_do_not_change_straight_line_geometry() {
        let scene = parse(&project(
            r#"<Shape Type="Path"><VertList>
            V0 0c0x1c1x24V8 0c0x14c1x1</VertList><PrimList>L0 1</PrimList></Shape>"#,
        ))
        .unwrap();
        assert_eq!(scene.lines.len(), 1);
        assert_eq!(
            scene.lines[0].vertices,
            [Vec3::new(0.0, 0.0, 0.0), Vec3::new(8.0, 0.0, 0.0)]
        );
    }

    #[test]
    fn malformed_geometry_unknown_shapes_and_containers_fail_explicitly() {
        let cases = [
            r#"<Shape Type="Text" Str="hello"/>"#,
            r#"<Shape Type="Bitmap" Data="image"/>"#,
            r#"<Shape Type="Polygon"/>"#,
            r#"<Page><Shape Type="Rect" W="1" H="1"/></Page>"#,
            r#"<Shape Type="Path" VertID="1" PrimID="2"/>"#,
            r#"<Shape Type="Path"><VertList>V0 0V1 1</VertList><PrimList>L0 2</PrimList></Shape>"#,
            r#"<Shape Type="Path"><VertList>V0 0V1 1junk</VertList><PrimList>L0 1</PrimList></Shape>"#,
            r#"<Shape Type="Path"><VertList>V0 0c0x2V1 1</VertList><PrimList>B0 1</PrimList></Shape>"#,
            r#"<Shape Type="Path"><VertList>V0 0c0x1c0x1V1 1</VertList><PrimList>L0 1</PrimList></Shape>"#,
            r#"<Shape Type="Path"><VertList>V0 0V1 1</VertList><PrimList>Q0 1</PrimList></Shape>"#,
            r#"<Shape Type="Rect" W="NaN" H="1"/>"#,
            r#"<Shape Type="Rect" W="1" H="1"><XForm>1 0 0 1</XForm></Shape>"#,
            r#"<Shape Type="Rect" W="1" H="1"><XForm>1 0 0 1 0 0</XForm><XForm>1 0 0 1 0 0</XForm></Shape>"#,
            r#"<Shape Type="Rect" W="1" H="-1"/>"#,
            r#"<Shape Type="Path"><V vx="0" vy="0"/><V vx="1" vy="1"/><VertList>V0 0V1 1</VertList><PrimList>L0 1</PrimList></Shape>"#,
        ];
        for case in cases {
            assert!(
                parse(&project(case)).is_err(),
                "accepted malformed geometry: {case}"
            );
        }
        assert!(parse(br#"<LightBurnProject FormatVersion="2"/>"#).is_err());
        assert!(
            parse(br"<!DOCTYPE LightBurnProject [<!ENTITY x 'value'>]><LightBurnProject/>")
                .is_err()
        );
        assert!(parse(br#"<LightBurnProject xmlns="urn:unknown"/>"#).is_err());
    }

    #[test]
    fn conflicting_resource_definitions_and_excessive_nesting_fail() {
        let content = r#"<Shape Type="Path" VertID="1"><VertList>V0 0V1 0</VertList><PrimList>L0 1</PrimList></Shape>
            <Shape Type="Path" VertID="1"><VertList>V0 0V2 0</VertList><PrimList>L0 1</PrimList></Shape>"#;
        assert!(
            parse(&project(content))
                .unwrap_err()
                .to_string()
                .contains("conflicting")
        );
        let mut content = "<Shape Type=\"Group\"><Children>".repeat(MAX_DEPTH + 2);
        content.push_str(r#"<Shape Type="Rect" W="1" H="1"/>"#);
        content.push_str(&"</Children></Shape>".repeat(MAX_DEPTH + 2));
        assert!(
            parse(&project(&content))
                .unwrap_err()
                .to_string()
                .contains("nesting limit")
        );
    }
}
