//! Static VRML97 geometry with DEF/USE sharing and affine scene transforms.
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use super::mesh::triangulate_polygon;
use super::transform::Transform;
use crate::scene::{Point, Scene, Triangle, Vec3};
use crate::{Error, Result};

/// Maximum numeric values and output geometry elements decoded from VRML.
const MAX_ITEMS: usize = 20_000_000;
/// Maximum parsed nodes and traversed node instances.
const MAX_NODES: usize = 100_000;
#[path = "vrml_geometry.rs"]
mod detailed_geometry;
/// Typed local prototype templates, lexical scopes and bounded structural expansion.
#[path = "vrml_prototypes.rs"]
mod prototypes;
use prototypes::{Budget, Interface, Prototype};
#[path = "vrml_billboard.rs"]
mod billboard;
/// Attach VRML context to an import error.
fn error(message: impl Into<String>) -> Error {
    message.into().into()
}

/// Load a UTF-8 VRML 2 scene into bounded static geometry.
///
/// # Errors
/// Returns an error for malformed tokens or nodes, unresolved definitions,
/// unsupported visible geometry, invalid numeric data, or exceeded traversal limits.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    vrml(std::str::from_utf8(&bytes)?)
}

#[derive(Clone, Debug, PartialEq)]
/// Lexical token in the VRML text syntax.
enum Token {
    /// Unquoted identifier or numeric token.
    Atom(String),
    /// Quoted string with escapes decoded.
    String(String),
    /// Structural punctuation token.
    Symbol(char),
}

/// Incremental lexical allocation preflight, including token spare capacity and string buffers.
#[derive(Default)]
struct LexicalStorage {
    /// Cumulative token-vector and string-buffer capacity bytes.
    bytes: usize,
}

impl LexicalStorage {
    /// Charge storage before capacity growth while enforcing the existing input byte ceiling.
    ///
    /// # Errors
    /// Capacity arithmetic overflows or lexical buffers exceed the byte ceiling.
    fn charge(&mut self, additional: usize) -> Result<()> {
        let bytes = self
            .bytes
            .checked_add(additional)
            .ok_or("VRML lexical storage overflow")?;
        if bytes as u64 > super::MAX_INPUT_BYTES {
            return Err(error(
                "VRML lexical storage exceeds the 512 MiB input ceiling",
            ));
        }
        self.bytes = bytes;
        Ok(())
    }

    /// Append one decoded character with amortized geometric, precharged string growth.
    ///
    /// # Errors
    /// String capacity, cumulative byte accounting or allocation fails.
    fn character(&mut self, text: &mut String, character: char) -> Result<()> {
        let required = text
            .len()
            .checked_add(character.len_utf8())
            .ok_or("VRML string size overflow")?;
        if required > text.capacity() {
            let capacity = required.max(text.capacity().saturating_mul(2)).max(4);
            self.charge(capacity - text.capacity())?;
            text.try_reserve_exact(capacity - text.len())?;
            if text.capacity() > capacity {
                self.charge(text.capacity() - capacity)?;
            }
        }
        text.push(character);
        Ok(())
    }

    /// Append a lexical token only after checking item count and complete spare-capacity growth.
    ///
    /// # Errors
    /// Token count, cumulative capacity arithmetic or allocation exceeds a limit.
    fn token(&mut self, tokens: &mut Vec<Token>, token: Token) -> Result<()> {
        if tokens.len() >= MAX_ITEMS {
            return Err(error("too many VRML tokens"));
        }
        if tokens.len() == tokens.capacity() {
            let capacity = (tokens.len() + 1)
                .max(tokens.capacity().saturating_mul(2))
                .clamp(4, MAX_ITEMS);
            let bytes = (capacity - tokens.capacity())
                .checked_mul(std::mem::size_of::<Token>())
                .ok_or("VRML token storage overflow")?;
            self.charge(bytes)?;
            tokens.try_reserve_exact(capacity - tokens.len())?;
            if tokens.capacity() > capacity {
                self.charge(
                    (tokens.capacity() - capacity)
                        .checked_mul(std::mem::size_of::<Token>())
                        .ok_or("VRML token storage overflow")?,
                )?;
            }
        }
        tokens.push(token);
        Ok(())
    }
}

/// Tokenize VRML text while respecting comments and quoted string escapes.
fn tokenize(source: &str) -> Result<Vec<Token>> {
    let mut chars = source.chars().peekable();
    let mut tokens = Vec::new();
    let mut storage = LexicalStorage::default();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() || c == ',' => {}
            '#' => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '{' | '}' | '[' | ']' => storage.token(&mut tokens, Token::Symbol(c))?,
            '"' => {
                let mut s = String::new();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '"' {
                        closed = true;
                        break;
                    }
                    if c == '\\' {
                        let c = chars
                            .next()
                            .ok_or_else(|| error("truncated VRML string escape"))?;
                        if !matches!(c, '\\' | '"') {
                            return Err(error("unsupported VRML string escape"));
                        }
                        storage.character(&mut s, c)?;
                    } else {
                        storage.character(&mut s, c)?;
                    }
                }
                if !closed {
                    return Err(error("unterminated VRML string"));
                }
                storage.token(&mut tokens, Token::String(s))?;
            }
            _ => {
                let mut s = String::new();
                storage.character(&mut s, c)?;
                while chars.peek().is_some_and(|c| {
                    !c.is_whitespace() && !matches!(c, ',' | '#' | '{' | '}' | '[' | ']' | '"')
                }) {
                    storage.character(&mut s, chars.next().unwrap())?;
                }
                storage.token(&mut tokens, Token::Atom(s))?;
            }
        }
    }
    Ok(tokens)
}
#[derive(Clone, Debug)]
/// Parsed value for one VRML field.
enum Value {
    /// Consecutive numeric or identifier atoms.
    Atoms(Vec<String>),
    /// One or more quoted strings.
    Strings(Vec<String>),
    /// A parsed node shared by definitions and uses.
    Node(Rc<Node>),
    /// Bracketed field values retaining their source order.
    List(Vec<Value>),
    /// Explicit null node value.
    Null,
    /// Typed interface association, optionally supplying a static initial field value.
    Binding {
        /// Interface declaration supplying this association.
        name: String,
        /// Whether the association initializes a persistent field rather than an event.
        initialize: bool,
    },
}
#[derive(Debug)]
/// A VRML node with its type and named field values.
struct Node {
    /// Node type identifier.
    kind: String,
    /// Fields indexed by their names.
    fields: HashMap<String, Value>,
    /// Immutable local template for a prototype-instance node, if this is not a built-in node.
    prototype: Option<Rc<Prototype>>,
}
/// Bounded VRML parser with shared definition lookup.
struct Parser {
    /// Lexical input tokens.
    tokens: Vec<Token>,
    /// Index of the next token to consume.
    offset: usize,
    /// Named nodes available to subsequent USE references.
    definitions: HashMap<String, Rc<Node>>,
    /// Lexically nested prototype declarations available after their complete definitions.
    prototypes: Vec<HashMap<String, Rc<Prototype>>>,
    /// Innermost prototype interface available to implementation IS statements.
    interface: Option<Rc<Interface>>,
    /// Prototype names whose implementations are still being parsed.
    declaring: Vec<String>,
    /// Cumulative token/template/instance item, node and byte accounting.
    budget: Budget,
    /// Expanded caller nodes preserving top-level DEF/USE sharing.
    expanded: prototypes::Memo,
}
impl Parser {
    /// Consume one atom token or return a contextual syntax error.
    fn atom(&mut self) -> Result<String> {
        match self.tokens.get(self.offset) {
            Some(Token::Atom(s)) => {
                self.budget
                    .charge(1, 0, s.len() + std::mem::size_of::<String>())?;
                let s = s.clone();
                self.offset += 1;
                Ok(s)
            }
            _ => Err(error("expected a VRML identifier")),
        }
    }
    /// Consume and require one punctuation token.
    fn symbol(&mut self, symbol: char) -> Result<()> {
        if self.tokens.get(self.offset) != Some(&Token::Symbol(symbol)) {
            return Err(error(format!("expected VRML '{symbol}'")));
        }
        self.offset += 1;
        Ok(())
    }
    /// Parse a node, definition, use or null reference with depth and node limits.
    fn node(&mut self, depth: usize) -> Result<Rc<Node>> {
        if depth > 256 || self.budget.nodes >= MAX_NODES {
            return Err(error("VRML node/depth limit exceeded"));
        }
        let kind = self.atom()?;
        if kind == "USE" {
            let name = self.atom()?;
            return self
                .definitions
                .get(&name)
                .cloned()
                .ok_or_else(|| error(format!("VRML USE references undefined DEF {name}")));
        }
        if kind == "DEF" {
            let name = self.atom()?;
            // A repeated DEF replaces the preceding name. Self-referential
            // graph cycles remain undefined and are rejected while parsing.
            self.definitions.remove(&name);
            let node = self.node(depth + 1)?;
            // A same-name DEF inside this body appears later in the lexical
            // scope, so subsequent USE statements must retain that binding.
            // The enclosing node still participates in the scene hierarchy.
            self.definitions.entry(name).or_insert_with(|| node.clone());
            return Ok(node);
        }
        if matches!(kind.as_str(), "PROTO" | "EXTERNPROTO" | "Script") {
            return Err(error(
                "VRML Script and external prototypes are not supported",
            ));
        }
        let prototype = self.prototype(&kind);
        if prototype.is_none() && self.declaring.contains(&kind) {
            return Err(error("recursive VRML prototype implementation"));
        }
        if prototype.is_none() && !self.declaring.is_empty() && !prototypes::builtin(&kind) {
            return Err(error(format!("unknown VRML node type {kind} in prototype")));
        }
        self.symbol('{')?;
        self.budget
            .charge(1, 1, std::mem::size_of::<Node>() + kind.len() + 64)?;
        let mut fields = HashMap::new();
        let mut associated = std::collections::HashSet::new();
        while self.tokens.get(self.offset) != Some(&Token::Symbol('}')) {
            if self.statement_keyword(depth + 1)? {
                continue;
            }
            let name = self.atom()?;
            self.budget
                .charge(1, 0, 4 * std::mem::size_of::<(String, Value)>())?;
            let value = if self.is_atom("IS") {
                self.binding(&kind, prototype.as_deref(), &name, &mut associated)?
            } else if let Some(prototype) = &prototype {
                let declaration = prototype
                    .interface
                    .get(&name)
                    .ok_or_else(|| error(format!("unknown VRML prototype field {name}")))?;
                if !declaration.access.initializes() {
                    return Err(error("VRML event values cannot be initialized in a node"));
                }
                self.typed_value(declaration.ty, depth + 1)?
            } else {
                self.value(depth + 1)?
            };
            if fields.insert(name, value).is_some() {
                return Err(error("duplicate VRML field"));
            }
        }
        self.symbol('}')?;
        Ok(Rc::new(Node {
            kind,
            fields,
            prototype,
        }))
    }
    /// Parse a field value, preserving bracketed values and shared node references.
    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > 256 {
            return Err(error("VRML nesting limit exceeded"));
        }
        match self.tokens.get(self.offset) {
            Some(Token::Symbol('[')) => {
                self.offset += 1;
                let mut values = Vec::new();
                while self.tokens.get(self.offset) != Some(&Token::Symbol(']')) {
                    self.budget.charge(1, 0, 2 * std::mem::size_of::<Value>())?;
                    self.budget.reserve(&mut values, 1)?;
                    values.push(self.value(depth + 1)?);
                }
                self.offset += 1;
                Ok(Value::List(values))
            }
            Some(Token::String(_)) => {
                let mut values = Vec::new();
                while matches!(self.tokens.get(self.offset), Some(Token::String(_))) {
                    self.budget.reserve(&mut values, 1)?;
                    values.push(self.string()?);
                }
                Ok(Value::Strings(values))
            }
            Some(Token::Atom(s)) if s == "NULL" => {
                self.offset += 1;
                Ok(Value::Null)
            }
            Some(Token::Atom(s)) if s == "IS" => {
                Err(error("VRML prototype field bindings are not supported"))
            }
            Some(Token::Atom(s))
                if matches!(s.as_str(), "DEF" | "USE")
                    || self.tokens.get(self.offset + 1) == Some(&Token::Symbol('{')) =>
            {
                self.node(depth + 1).map(Value::Node)
            }
            Some(Token::Atom(_)) => {
                let s = self.atom()?;
                let numeric = s.parse::<f64>().is_ok() || s.starts_with("0x");
                let mut values = Vec::new();
                self.budget.reserve(&mut values, 1)?;
                values.push(s);
                if numeric {
                    while let Some(Token::Atom(s)) = self.tokens.get(self.offset) {
                        if s.parse::<f64>().is_err() && !s.starts_with("0x") {
                            break;
                        }
                        self.budget.reserve(&mut values, 1)?;
                        values.push(self.atom()?);
                    }
                }
                Ok(Value::Atoms(values))
            }
            _ => Err(error("missing or invalid VRML field value")),
        }
    }
}

impl Node {
    /// Reject fields whose unsupported behavior would change imported geometry.
    fn check(&self, fields: &[&str]) -> Result<()> {
        if let Some(name) = self
            .fields
            .keys()
            .find(|name| !fields.contains(&name.as_str()))
        {
            return Err(error(format!(
                "unsupported VRML {} field {name}",
                self.kind
            )));
        }
        Ok(())
    }
    /// Read finite numeric field values, or return the supplied omitted default.
    fn numbers(&self, name: &str) -> Result<Vec<f64>> {
        fn flatten(value: &Value, output: &mut Vec<f64>) -> Result<()> {
            match value {
                Value::Atoms(values) => {
                    for value in values {
                        let n: f64 = value.parse()?;
                        if !n.is_finite() {
                            return Err(error("non-finite VRML number"));
                        }
                        output.push(n);
                    }
                }
                Value::List(values) => {
                    for value in values {
                        flatten(value, output)?;
                    }
                }
                _ => return Err(error("VRML numeric field contains non-numeric data")),
            }
            Ok(())
        }
        let mut values = Vec::new();
        if let Some(value) = self.fields.get(name) {
            flatten(value, &mut values)?;
        }
        Ok(values)
    }
    /// Read exactly one numeric field value.
    fn scalar(&self, name: &str, default: f64) -> Result<f64> {
        if !self.fields.contains_key(name) {
            return Ok(default);
        }
        let values = self.numbers(name)?;
        if values.len() != 1 {
            return Err(error(format!("VRML {name} needs one number")));
        }
        Ok(values[0])
    }
    /// Read a three-component numeric field value.
    fn vector(&self, name: &str, default: Vec3) -> Result<Vec3> {
        if !self.fields.contains_key(name) {
            return Ok(default);
        }
        let values = self.numbers(name)?;
        if values.len() != 3 {
            return Err(error(format!("VRML {name} needs three numbers")));
        }
        Ok(Vec3::new(values[0], values[1], values[2]))
    }
    /// Read a TRUE or FALSE field with its omitted default.
    fn boolean(&self, name: &str, default: bool) -> Result<bool> {
        match self.fields.get(name) {
            None => Ok(default),
            Some(Value::Atoms(values)) if values.len() == 1 => match values[0].as_str() {
                "TRUE" => Ok(true),
                "FALSE" => Ok(false),
                _ => Err(error("VRML boolean must be TRUE or FALSE")),
            },
            _ => Err(error("invalid VRML boolean field")),
        }
    }
    /// Read child nodes while preserving explicit null entries.
    fn node_options(&self, name: &str) -> Result<Vec<Option<Rc<Node>>>> {
        fn flatten(value: &Value, nodes: &mut Vec<Option<Rc<Node>>>) -> Result<()> {
            match value {
                Value::Node(node) => nodes.push(Some(node.clone())),
                Value::Null => nodes.push(None),
                Value::List(values) => {
                    for value in values {
                        flatten(value, nodes)?;
                    }
                }
                _ => return Err(error("VRML node field contains a non-node value")),
            }
            Ok(())
        }
        let mut nodes = Vec::new();
        if let Some(value) = self.fields.get(name) {
            flatten(value, &mut nodes)?;
        }
        Ok(nodes)
    }
    /// Read non-null child nodes from a node-valued field.
    fn nodes(&self, name: &str) -> Result<Vec<Rc<Node>>> {
        Ok(self.node_options(name)?.into_iter().flatten().collect())
    }
    /// Read at most one child node from a field.
    fn single(&self, name: &str) -> Result<Option<Rc<Node>>> {
        let mut nodes = self.node_options(name)?;
        if nodes.len() > 1 {
            return Err(error(format!("VRML {name} must contain at most one node")));
        }
        Ok(nodes.pop().flatten())
    }
}

/// Group flat numeric field components into three-dimensional vectors.
fn vectors(node: &Node, name: &str) -> Result<Vec<Vec3>> {
    let values = node.numbers(name)?;
    if values.len() % 3 != 0 {
        return Err(error("VRML vector list needs complete triples"));
    }
    Ok(values
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| Vec3::new(p[0], p[1], p[2]))
        .collect())
}
/// Encode a linear RGB field color as display bytes.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Color components are clamped and rounded to the inclusive byte range before conversion."
)]
fn color(value: Vec3) -> [u8; 3] {
    [value.x, value.y, value.z].map(|v| (v * 255.).clamp(0., 255.).round() as u8)
}
/// Split index lists at VRML's negative face separators.
#[allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "The exact -1 sentinel separates lists; other indices are validated integral values bounded by twenty million."
)]
fn index_lists(values: &[f64]) -> Result<Vec<Vec<usize>>> {
    let mut faces = Vec::new();
    let mut current = Vec::new();
    for &n in values {
        if n == -1. {
            if current.is_empty() {
                return Err(error("empty VRML index list"));
            }
            faces.push(std::mem::take(&mut current));
        } else {
            if n < 0. || n.fract() != 0. || n > MAX_ITEMS as f64 {
                return Err(error("invalid VRML index"));
            }
            current.push(n as usize);
        }
    }
    if !current.is_empty() {
        faces.push(current);
    }
    Ok(faces)
}
/// Read a finite axis-angle rotation from a node field.
fn rotation(node: &Node, name: &str, inverse: bool) -> Result<Transform> {
    let values = node.numbers(name)?;
    if values.is_empty() && !node.fields.contains_key(name) {
        return Ok(Transform::IDENTITY);
    }
    if values.len() != 4 {
        return Err(error("VRML rotation needs an axis and an angle in radians"));
    }
    axis_rotation(
        Vec3::new(values[0], values[1], values[2]),
        values[3] * if inverse { -1. } else { 1. },
    )
}
/// Compose a rotation matrix from a normalized axis and angle.
fn axis_rotation(axis: Vec3, angle: f64) -> Result<Transform> {
    if axis.length() == 0. {
        return Err(error("VRML rotation axis is zero"));
    }
    let axis = axis.normalized();
    let a = [axis.x, axis.y, axis.z];
    let (s, c) = angle.sin_cos();
    let t = 1. - c;
    let rows = [
        [
            c + a[0] * a[0] * t,
            a[0] * a[1] * t - a[2] * s,
            a[0] * a[2] * t + a[1] * s,
        ],
        [
            a[1] * a[0] * t + a[2] * s,
            c + a[1] * a[1] * t,
            a[1] * a[2] * t - a[0] * s,
        ],
        [
            a[2] * a[0] * t - a[1] * s,
            a[2] * a[1] * t + a[0] * s,
            c + a[2] * a[2] * t,
        ],
    ];
    let mut matrix = Transform::IDENTITY.0;
    for (column, values) in matrix.iter_mut().take(3).enumerate() {
        for row in 0..3 {
            values[row] = rows[row][column];
        }
    }
    Ok(Transform(matrix))
}
/// Create an affine translation matrix.
fn translation(v: Vec3) -> Transform {
    let mut m = Transform::IDENTITY.0;
    m[3][0] = v.x;
    m[3][1] = v.y;
    m[3][2] = v.z;
    Transform(m)
}
/// Compose VRML translation, center, rotation, scale orientation and scale.
fn transform(node: &Node) -> Result<Transform> {
    let center = node.vector("center", Vec3::default())?;
    let translate = node.vector("translation", Vec3::default())?;
    let scale = node.vector("scale", Vec3::new(1., 1., 1.))?;
    if scale.x <= 0. || scale.y <= 0. || scale.z <= 0. {
        return Err(error("VRML scale components must be positive"));
    }
    let mut s = Transform::IDENTITY.0;
    s[0][0] = scale.x;
    s[1][1] = scale.y;
    s[2][2] = scale.z;
    Ok(translation(translate)
        .then(translation(center))
        .then(rotation(node, "rotation", false)?)
        .then(rotation(node, "scaleOrientation", false)?)
        .then(Transform(s))
        .then(rotation(node, "scaleOrientation", true)?)
        .then(translation(-center)))
}

/// Static VRML traversal and accumulated scene geometry.
struct Importer {
    /// Rendered points, lines and triangles.
    scene: Scene,
    /// Number of visited instances for the traversal budget.
    visits: usize,
}
impl Importer {
    /// Transform and append a finite point with its optional color.
    fn point(&mut self, position: Vec3, world: Transform, color: Option<[u8; 3]>) -> Result<()> {
        let p = world.apply(position);
        let position = Vec3::new(p.x, -p.z, p.y);
        if !position.is_finite() {
            return Err(error("VRML transformed coordinate is non-finite"));
        }
        self.scene.points.push(Point { position, color });
        self.limit()
    }
    /// Transform and append a triangle respecting winding and optional color.
    fn face(
        &mut self,
        vertices: &[Vec3],
        world: Transform,
        color: Option<[u8; 3]>,
        ccw: bool,
    ) -> Result<()> {
        for triangle in triangulate_polygon(vertices)? {
            let mut vertices = triangle.map(|v| {
                let p = world.apply(v);
                Vec3::new(p.x, -p.z, p.y)
            });
            if vertices.iter().any(|p| !p.is_finite()) {
                return Err(error("VRML transformed coordinate is non-finite"));
            }
            if !ccw {
                vertices.swap(1, 2);
            }
            self.scene.triangles.push(Triangle { vertices, color });
            self.limit()?;
        }
        Ok(())
    }
    /// Enforce the aggregate output geometry budget.
    fn limit(&self) -> Result<()> {
        if self.scene.points.len() + self.scene.triangles.len() + self.scene.lines.len() > MAX_ITEMS
        {
            return Err(error("too much VRML geometry"));
        }
        Ok(())
    }
    /// Visit visible scene nodes, propagating transforms and shape appearance.
    #[allow(
        clippy::too_many_lines,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "The bounded node dispatch preserves inherited state; switch selections are validated nonnegative integral indices before casting."
    )]
    fn walk(&mut self, node: &Node, world: Transform, depth: usize) -> Result<()> {
        self.visits += 1;
        if depth > 256 || self.visits > MAX_NODES {
            return Err(error("VRML scene expansion limit exceeded"));
        }
        match node.kind.as_str() {
            "Transform" | "Group" | "Anchor" | "Collision" => {
                let fields = if node.kind == "Transform" {
                    vec![
                        "children",
                        "bboxCenter",
                        "bboxSize",
                        "center",
                        "rotation",
                        "scale",
                        "scaleOrientation",
                        "translation",
                    ]
                } else if node.kind == "Anchor" {
                    vec![
                        "children",
                        "bboxCenter",
                        "bboxSize",
                        "description",
                        "parameter",
                        "url",
                    ]
                } else if node.kind == "Collision" {
                    vec!["children", "bboxCenter", "bboxSize", "collide", "proxy"]
                } else {
                    vec!["children", "bboxCenter", "bboxSize"]
                };
                node.check(&fields)?;
                let world = if node.kind == "Transform" {
                    world.then(transform(node)?)
                } else {
                    world
                };
                for child in node.nodes("children")? {
                    self.walk(&child, world, depth + 1)?;
                }
            }
            "Billboard" => {
                node.check(&["children", "axisOfRotation", "bboxCenter", "bboxSize"])?;
                let axis = node.vector("axisOfRotation", Vec3::new(0., 1., 0.))?;
                let world = billboard::placement(world, axis)?;
                for child in node.nodes("children")? {
                    self.walk(&child, world, depth + 1)?;
                }
            }
            "Switch" => {
                node.check(&["choice", "whichChoice"])?;
                let selection = node.scalar("whichChoice", -1.)?;
                if selection.fract() != 0. || selection < -1. || selection > MAX_NODES as f64 {
                    return Err(error("invalid VRML Switch choice"));
                }
                let choices = node.node_options("choice")?;
                if selection >= 0. {
                    let selected = choices
                        .get(selection as usize)
                        .ok_or_else(|| error("VRML Switch choice is out of bounds"))?;
                    if let Some(selected) = selected {
                        self.walk(selected, world, depth + 1)?;
                    }
                }
            }
            "LOD" => {
                node.check(&["level", "center", "range"])?;
                // A fixed thumbnail uses the first, most detailed LOD.
                if let Some(Some(level)) = node.node_options("level")?.first() {
                    self.walk(level, world, depth + 1)?;
                }
            }
            "Shape" => {
                node.check(&["appearance", "geometry"])?;
                let geometry = node.single("geometry")?;
                let mut material_color = Some([255, 255, 255]);
                if let Some(appearance) = node.single("appearance")? {
                    if appearance.kind != "Appearance" {
                        return Err(error("VRML appearance must be an Appearance node"));
                    }
                    appearance.check(&["material", "texture", "textureTransform"])?;
                    if let Some(material) = appearance.single("material")? {
                        if material.kind != "Material" {
                            return Err(error("VRML material must be a Material node"));
                        }
                        material.check(&[
                            "ambientIntensity",
                            "diffuseColor",
                            "emissiveColor",
                            "shininess",
                            "specularColor",
                            "transparency",
                        ])?;
                        material_color = Some(color(
                            if geometry.as_ref().is_some_and(|g| {
                                matches!(g.kind.as_str(), "IndexedLineSet" | "PointSet")
                            }) {
                                material.vector("emissiveColor", Vec3::default())?
                            } else {
                                material.vector("diffuseColor", Vec3::new(0.8, 0.8, 0.8))?
                            },
                        ));
                    }
                }
                if let Some(geometry) = geometry {
                    self.geometry(&geometry, world, material_color)?;
                }
            }
            "WorldInfo"
            | "NavigationInfo"
            | "Viewpoint"
            | "Background"
            | "Fog"
            | "DirectionalLight"
            | "PointLight"
            | "SpotLight"
            | "TimeSensor"
            | "TouchSensor"
            | "ProximitySensor"
            | "VisibilitySensor"
            | "PlaneSensor"
            | "SphereSensor"
            | "CylinderSensor"
            | "PositionInterpolator"
            | "OrientationInterpolator"
            | "CoordinateInterpolator"
            | "ColorInterpolator"
            | "ScalarInterpolator"
            | "NormalInterpolator"
            | "Coordinate"
            | "Color"
            | "Normal"
            | "TextureCoordinate"
            | "Material"
            | "Appearance"
            | "ImageTexture"
            | "MovieTexture"
            | "PixelTexture"
            | "TextureTransform" => {}
            kind => return Err(error(format!("unsupported VRML scene node {kind}"))),
        }
        Ok(())
    }
    /// Decode a shape's geometry and append its transformed static representation.
    #[allow(
        clippy::too_many_lines,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Geometry dispatch keeps field validation with decoding; color indices and byte averages are bounded, as are small tessellation angles."
    )]
    fn geometry(&mut self, node: &Node, world: Transform, material: Option<[u8; 3]>) -> Result<()> {
        match node.kind.as_str() {
            "IndexedLineSet" => self.indexed_lines(node, world, material)?,
            "ElevationGrid" => self.elevation_grid(node, world, material)?,
            "Extrusion" => self.extrusion(node, world, material)?,
            "IndexedFaceSet" | "PointSet" => {
                if node.kind == "PointSet" {
                    node.check(&["coord", "color"])?;
                } else {
                    node.check(&[
                        "coord",
                        "coordIndex",
                        "color",
                        "colorIndex",
                        "colorPerVertex",
                        "normal",
                        "normalIndex",
                        "normalPerVertex",
                        "texCoord",
                        "texCoordIndex",
                        "ccw",
                        "solid",
                        "convex",
                        "creaseAngle",
                    ])?;
                }
                let Some(coordinates) = node.single("coord")? else {
                    return Ok(());
                };
                if coordinates.kind != "Coordinate" {
                    return Err(error("VRML coord must be a Coordinate node"));
                }
                coordinates.check(&["point"])?;
                let positions = vectors(&coordinates, "point")?;
                let colors = node
                    .single("color")?
                    .map(|colors| -> Result<Vec<[u8; 3]>> {
                        if colors.kind != "Color" {
                            return Err(error("VRML color must be a Color node"));
                        }
                        colors.check(&["color"])?;
                        Ok(vectors(&colors, "color")?.into_iter().map(color).collect())
                    })
                    .transpose()?;
                if node.kind == "PointSet" {
                    if colors
                        .as_ref()
                        .is_some_and(|colors| colors.len() < positions.len())
                    {
                        return Err(error("VRML PointSet has fewer colors than coordinates"));
                    }
                    for (i, p) in positions.into_iter().enumerate() {
                        self.point(p, world, colors.as_ref().map(|c| c[i]).or(material))?;
                    }
                    return Ok(());
                }
                let faces = index_lists(&node.numbers("coordIndex")?)?;
                let per_vertex = node.boolean("colorPerVertex", true)?;
                let raw_colors = node.numbers("colorIndex")?;
                let color_faces = if per_vertex {
                    index_lists(&raw_colors)?
                } else {
                    Vec::new()
                };
                if per_vertex && !color_faces.is_empty() && color_faces.len() != faces.len() {
                    return Err(error("VRML colorIndex face count differs from coordIndex"));
                }
                if !per_vertex && !raw_colors.is_empty() && raw_colors.len() != faces.len() {
                    return Err(error(
                        "VRML per-face colorIndex count differs from coordIndex",
                    ));
                }
                for (face_index, indices) in faces.iter().enumerate() {
                    let vertices = indices
                        .iter()
                        .map(|&i| {
                            positions
                                .get(i)
                                .copied()
                                .ok_or_else(|| error("VRML coordinate index out of bounds"))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let mut face_color = material;
                    if let Some(colors) = &colors {
                        if per_vertex {
                            let color_indices = color_faces.get(face_index).unwrap_or(indices);
                            if color_indices.len() != indices.len() {
                                return Err(error("VRML color and coordinate face lengths differ"));
                            }
                            let face_colors = color_indices
                                .iter()
                                .map(|&i| {
                                    colors
                                        .get(i)
                                        .copied()
                                        .ok_or_else(|| error("VRML color index out of bounds"))
                                })
                                .collect::<Result<Vec<_>>>()?;
                            if !face_colors.is_empty() {
                                face_color = Some(std::array::from_fn(|i| {
                                    (face_colors.iter().map(|c| u64::from(c[i])).sum::<u64>()
                                        / face_colors.len() as u64)
                                        as u8
                                }));
                            }
                        } else {
                            let index = raw_colors
                                .get(face_index)
                                .copied()
                                .unwrap_or(face_index as f64);
                            if index < 0. || index.fract() != 0. || index > MAX_ITEMS as f64 {
                                return Err(error("invalid VRML per-face color index"));
                            }
                            face_color =
                                Some(*colors.get(index as usize).ok_or_else(|| {
                                    error("VRML per-face color index out of bounds")
                                })?);
                        }
                    }
                    self.face(&vertices, world, face_color, node.boolean("ccw", true)?)?;
                }
            }
            "Box" => {
                node.check(&["size"])?;
                let size = node.vector("size", Vec3::new(2., 2., 2.))?;
                if size.x <= 0. || size.y <= 0. || size.z <= 0. {
                    return Err(error("VRML Box size must be positive"));
                }
                let vertices: [Vec3; 8] = std::array::from_fn(|i| {
                    Vec3::new(
                        if i & 1 == 0 {
                            -size.x / 2.
                        } else {
                            size.x / 2.
                        },
                        if i & 2 == 0 {
                            -size.y / 2.
                        } else {
                            size.y / 2.
                        },
                        if i & 4 == 0 {
                            -size.z / 2.
                        } else {
                            size.z / 2.
                        },
                    )
                });
                for face in [
                    [0, 4, 6, 2],
                    [1, 3, 7, 5],
                    [0, 1, 5, 4],
                    [2, 6, 7, 3],
                    [0, 2, 3, 1],
                    [4, 5, 7, 6],
                ] {
                    self.face(&face.map(|i| vertices[i]), world, material, true)?;
                }
            }
            "Sphere" => {
                node.check(&["radius"])?;
                let radius = node.scalar("radius", 1.)?;
                if radius <= 0. {
                    return Err(error("VRML Sphere radius must be positive"));
                }
                let point = |latitude: usize, longitude: usize| {
                    let phi = std::f64::consts::PI * latitude as f64 / 16.;
                    let theta = std::f64::consts::TAU * longitude as f64 / 32.;
                    Vec3::new(
                        radius * phi.sin() * theta.cos(),
                        radius * phi.cos(),
                        radius * phi.sin() * theta.sin(),
                    )
                };
                for lat in 0..16 {
                    for lon in 0..32 {
                        self.face(
                            &[
                                point(lat, lon),
                                point(lat, lon + 1),
                                point(lat + 1, lon + 1),
                            ],
                            world,
                            material,
                            true,
                        )?;
                        self.face(
                            &[
                                point(lat, lon),
                                point(lat + 1, lon + 1),
                                point(lat + 1, lon),
                            ],
                            world,
                            material,
                            true,
                        )?;
                    }
                }
            }
            "Cylinder" | "Cone" => {
                let cone = node.kind == "Cone";
                if cone {
                    node.check(&["bottomRadius", "height", "side", "bottom"])?;
                } else {
                    node.check(&["radius", "height", "side", "top", "bottom"])?;
                }
                let radius = node.scalar(if cone { "bottomRadius" } else { "radius" }, 1.)?;
                let height = node.scalar("height", 2.)?;
                if radius <= 0. || height <= 0. {
                    return Err(error("VRML radial primitive dimensions must be positive"));
                }
                let point = |i: usize, y: f64| {
                    let a = std::f64::consts::TAU * i as f64 / 32.;
                    Vec3::new(radius * a.cos(), y, radius * a.sin())
                };
                for i in 0..32 {
                    let (a, b) = (point(i, -height / 2.), point(i + 1, -height / 2.));
                    if node.boolean("bottom", true)? {
                        self.face(
                            &[Vec3::new(0., -height / 2., 0.), a, b],
                            world,
                            material,
                            true,
                        )?;
                    }
                    if cone {
                        if node.boolean("side", true)? {
                            self.face(
                                &[a, Vec3::new(0., height / 2., 0.), b],
                                world,
                                material,
                                true,
                            )?;
                        }
                    } else {
                        let (c, d) = (point(i + 1, height / 2.), point(i, height / 2.));
                        if node.boolean("top", true)? {
                            self.face(
                                &[Vec3::new(0., height / 2., 0.), c, d],
                                world,
                                material,
                                true,
                            )?;
                        }
                        if node.boolean("side", true)? {
                            self.face(&[a, d, c, b], world, material, true)?;
                        }
                    }
                }
            }
            kind => return Err(error(format!("unsupported VRML geometry {kind}"))),
        }
        Ok(())
    }
}

/// Parse root nodes and traverse the resulting static VRML scene.
fn vrml(source: &str) -> Result<Scene> {
    let header = source
        .lines()
        .next()
        .unwrap_or("")
        .trim_start_matches('\u{feff}')
        .trim();
    if header != "#VRML V2.0 utf8" {
        return Err(error(
            "expected VRML97 '#VRML V2.0 utf8' header; VRML 1.0 is not supported",
        ));
    }
    let tokens = tokenize(source.trim_start_matches('\u{feff}'))?;
    let mut parser = Parser::new(tokens)?;
    let mut importer = Importer {
        scene: Scene::default(),
        visits: 0,
    };
    while parser.offset < parser.tokens.len() {
        if parser.statement_keyword(0)? {
            continue;
        }
        let node = parser.node(0)?;
        let node = prototypes::expand(&node, &mut parser.expanded, &mut parser.budget)?;
        importer.walk(&node, Transform::IDENTITY, 0)?;
    }
    importer.scene.validate()?;
    Ok(importer.scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexed_faces_def_use_transforms_and_y_up() {
        let source = r"#VRML V2.0 utf8
          DEF coords Coordinate { point [ 0 0 0, 1 0 0, 0 1 0 ] }
          DEF geom IndexedFaceSet { coord USE coords coordIndex [0,1,2,-1] }
          Transform { translation 2 3 4 scale 2 2 2 children [
            Shape { geometry USE geom appearance Appearance { material Material { diffuseColor 1 0 0 } } }
          ] }
          Shape { geometry USE geom }
        ";
        // Geometry DEFs are normally nested in a Shape; keep the top-level
        // definition as a data declaration without independently rendering it.
        let source = source
            .replace(
                "DEF geom IndexedFaceSet",
                "DEF geomHolder Shape { geometry DEF geom IndexedFaceSet",
            )
            .replace("coordIndex [0,1,2,-1] }", "coordIndex [0,1,2,-1] } }");
        let scene = vrml(&source).unwrap();
        assert_eq!(scene.triangles.len(), 3);
        assert_eq!(scene.triangles[1].vertices[2], Vec3::new(2., -4., 5.));
        assert_eq!(scene.triangles[1].color, Some([255, 0, 0]));
    }
    #[test]
    fn concave_faces_and_face_colors() {
        let scene = vrml("#VRML V2.0 utf8\nShape { geometry IndexedFaceSet { coord Coordinate { point [0 0 0 2 0 0 2 2 0 1 1 0 0 2 0] } coordIndex [0 1 2 3 4 -1] colorPerVertex FALSE color Color { color [0 1 0] } colorIndex [0] } }").unwrap();
        assert_eq!(scene.triangles.len(), 3);
        assert!(scene.triangles.iter().all(|t| t.color == Some([0, 255, 0])));
        let area: f64 = scene
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    / 2.
            })
            .sum();
        assert!((area - 3.).abs() < 1e-12);
    }
    #[test]
    fn box_sphere_cone_cylinder_and_point_set() {
        for geometry in ["Box {}", "Sphere {}", "Cone {}", "Cylinder {}"] {
            let scene = vrml(&format!("#VRML V2.0 utf8\nShape {{ geometry {geometry} }}")).unwrap();
            assert!(!scene.triangles.is_empty());
            scene.validate().unwrap();
        }
        let scene = vrml("#VRML V2.0 utf8\nShape { geometry PointSet { coord Coordinate { point [1 2 3] } color Color { color [1 0 0] } } }").unwrap();
        assert_eq!(scene.points[0].position, Vec3::new(1., -3., 2.));
        assert_eq!(scene.points[0].color, Some([255, 0, 0]));
    }
    #[test]
    fn center_rotation_and_switch_selection() {
        let source = "#VRML V2.0 utf8\nTransform { center 1 0 0 rotation 0 0 1 3.141592653589793 children [Switch { whichChoice 1 choice [Shape { geometry Sphere {} } Shape { geometry PointSet { coord Coordinate { point [2 0 0] } } } ]}] }";
        let scene = vrml(source).unwrap();
        assert!(scene.triangles.is_empty());
        assert!((scene.points[0].position.x).abs() < 1e-12);
        let scene = vrml(
            "#VRML V2.0 utf8\nSwitch { whichChoice 1 choice [NULL Shape { geometry Box {} }] }",
        )
        .unwrap();
        assert_eq!(scene.triangles.len(), 12);
    }

    /// Nested same-name definitions retain the closest preceding lexical binding.
    #[test]
    fn nested_def_shadowing_preserves_outer_geometry_and_later_inner_uses() {
        let source = r"#VRML V2.0 utf8
          DEF Same Transform {translation 10 0 0 children [
            DEF Same Shape {geometry Box {size 2 2 2}}
          ]}
          USE Same";
        let scene = vrml(source).unwrap();
        assert_eq!(scene.triangles.len(), 24);
        let x_bounds = |triangles: &[Triangle]| {
            triangles
                .iter()
                .flat_map(|triangle| triangle.vertices)
                .fold([f64::INFINITY, f64::NEG_INFINITY], |[low, high], point| {
                    [low.min(point.x), high.max(point.x)]
                })
        };
        assert_eq!(
            x_bounds(&scene.triangles).map(f64::to_bits),
            [-1_f64, 11.].map(f64::to_bits)
        );
        assert_eq!(
            x_bounds(&scene.triangles[..12]).map(f64::to_bits),
            [9_f64, 11.].map(f64::to_bits)
        );
        assert_eq!(
            x_bounds(&scene.triangles[12..]).map(f64::to_bits),
            [-1_f64, 1.].map(f64::to_bits)
        );

        // The preceding and nested Shape have identical content but distinct
        // lexical definitions; content equality cannot decide which binding wins.
        let identical = source.replace(
            "DEF Same Transform",
            "DEF Same Shape {geometry Box {size 2 2 2}}\nDEF Same Transform",
        );
        let scene = vrml(&identical).unwrap();
        assert_eq!(scene.triangles.len(), 36);
        for (triangles, expected) in
            scene
                .triangles
                .as_chunks::<12>()
                .0
                .iter()
                .zip([[-1_f64, 1.], [9., 11.], [-1., 1.]])
        {
            assert_eq!(
                x_bounds(triangles).map(f64::to_bits),
                expected.map(f64::to_bits)
            );
        }

        let siblings = r"#VRML V2.0 utf8
          DEF Same Transform {translation 10 0 0 children [
            DEF Same Transform {translation 2 0 0 children [
              DEF Same Shape {geometry Box {size 2 2 2}}
            ]}
            USE Same
          ]}
          USE Same";
        let scene = vrml(siblings).unwrap();
        assert_eq!(scene.triangles.len(), 36);
        for (triangles, expected) in
            scene
                .triangles
                .as_chunks::<12>()
                .0
                .iter()
                .zip([[11_f64, 13.], [9., 11.], [-1., 1.]])
        {
            assert_eq!(
                x_bounds(triangles).map(f64::to_bits),
                expected.map(f64::to_bits)
            );
        }
    }

    /// Unshadowed definitions remain available, while active self references stay undefined.
    #[test]
    fn nested_def_names_preserve_unshadowed_uses_and_reject_cycles() {
        let source = r"#VRML V2.0 utf8
          DEF Same Transform {translation 10 0 0 children [
            DEF Inner Shape {geometry Box {size 2 2 2}}
          ]}
          USE Same
          USE Inner";
        let scene = vrml(source).unwrap();
        assert_eq!(scene.triangles.len(), 36);
        for (triangles, center) in scene
            .triangles
            .as_chunks::<12>()
            .0
            .iter()
            .zip([10., 10., 0.])
        {
            assert!(
                triangles
                    .iter()
                    .flat_map(|triangle| triangle.vertices)
                    .all(|point| (point.x - center).abs().to_bits() == 1_f64.to_bits())
            );
        }
        let duplicate = vrml(
            r"#VRML V2.0 utf8
              DEF Same Shape {geometry Box {size 2 2 2}}
              DEF Same Shape {geometry Box {size 4 4 4}}
              USE Same",
        )
        .unwrap();
        assert_eq!(duplicate.triangles.len(), 36);
        for (triangles, extent) in duplicate
            .triangles
            .as_chunks::<12>()
            .0
            .iter()
            .zip([1_f64, 2., 2.])
        {
            assert!(
                triangles
                    .iter()
                    .flat_map(|triangle| triangle.vertices)
                    .all(|point| point.x.abs().to_bits() == extent.to_bits())
            );
        }
        for invalid in [
            "DEF Same Group {children USE Same}",
            "DEF Same Shape {geometry Box {}} DEF Same Group {children USE Same}",
            "DEF Same Group {children [DEF Same Group {children USE Same}]}",
        ] {
            assert!(
                vrml(&format!("#VRML V2.0 utf8\n{invalid}")).is_err(),
                "{invalid}"
            );
        }
    }
    fn signed_volume(scene: &Scene) -> f64 {
        scene
            .triangles
            .iter()
            .map(|t| t.vertices[0].dot(t.vertices[1].cross(t.vertices[2])) / 6.)
            .sum()
    }
    #[test]
    fn indexed_lines_preserve_indices_transform_and_emissive_or_authored_color() {
        let scene = vrml(r"#VRML V2.0 utf8
        Transform { translation 2 3 4 scale 2 2 2 children [
          Shape { appearance Appearance { material Material { diffuseColor 1 0 0 emissiveColor 0 1 0 } }
            geometry IndexedLineSet { coord Coordinate { point [0 0 0 1 0 0 1 1 0] } coordIndex [0 1 2 -1] } }
          Shape { geometry IndexedLineSet { coord Coordinate { point [0 0 0 1 0 0 1 1 0] }
            coordIndex [0 1 -1 1 2] colorPerVertex FALSE color Color { color [1 0 0 0 0 1] } colorIndex [1 0] } }
        ] }").unwrap();
        assert_eq!(scene.lines.len(), 4);
        assert_eq!(
            scene.lines[0].vertices,
            [Vec3::new(2., -4., 3.), Vec3::new(4., -4., 3.)]
        );
        assert_eq!(scene.lines[0].color, Some([0, 255, 0]));
        assert_eq!(scene.lines[2].color, Some([0, 0, 255]));
        assert_eq!(scene.lines[3].color, Some([255, 0, 0]));
        let bad = r"#VRML V2.0 utf8
        Shape { geometry IndexedLineSet { coord Coordinate { point [0 0 0 1 0 0] } coordIndex [0 9] } }";
        assert!(vrml(bad).is_err());
        let colored = bad.replace("[0 9]", "[0 1]").replace(
            "coordIndex [0 1]",
            "coordIndex [0 1] color Color { color [1 0 0] } colorIndex [0 1]",
        );
        assert!(vrml(&colored).is_err());
    }
    #[test]
    fn elevation_grid_preserves_height_spacing_winding_and_quad_colors() {
        let source = r"#VRML V2.0 utf8
        Transform { translation 10 20 30 children [Shape { geometry ElevationGrid {
          xDimension 3 zDimension 2 xSpacing 2 zSpacing 3 height [0 1 2 0 1 2]
          colorPerVertex FALSE color Color { color [1 0 0 0 1 0] }
        } }] }";
        let scene = vrml(source).unwrap();
        assert_eq!(scene.triangles.len(), 4);
        assert!(scene.positions().any(|p| p == Vec3::new(14., -33., 22.)));
        let area: f64 = scene
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    / 2.
            })
            .sum();
        assert!((area - 6. * 5f64.sqrt()).abs() < 1e-9);
        assert!(
            scene.triangles[..2]
                .iter()
                .all(|t| t.color == Some([255, 0, 0]))
        );
        assert!(
            scene.triangles[2..]
                .iter()
                .all(|t| t.color == Some([0, 255, 0]))
        );
        assert!(scene.triangles.iter().all(|t| {
            (t.vertices[1] - t.vertices[0])
                .cross(t.vertices[2] - t.vertices[0])
                .z
                > 0.
        }));
        assert!(vrml(&source.replace("height [0 1 2 0 1 2]", "height [0 1]")).is_err());
        assert!(vrml(&source.replace("xSpacing 2", "xSpacing 0")).is_err());
        let saddle = vrml("#VRML V2.0 utf8\nShape { geometry ElevationGrid { xDimension 2 zDimension 2 height [0 100 100 0] } }").unwrap();
        assert_eq!(saddle.triangles.len(), 2);
        let area: f64 = saddle
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    / 2.
            })
            .sum();
        assert!((area - 20_001f64.sqrt()).abs() < 1e-9);
    }
    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "This test identifies caps at exactly authored z coordinates with no interpolated transform."
    )]
    fn extrusion_concave_caps_and_open_sides_have_real_geometry() {
        let source = r"#VRML V2.0 utf8
        Shape { geometry Extrusion { convex FALSE crossSection [0 0 0 2 1 1 2 2 2 0 0 0] spine [0 0 0 0 3 0] } }";
        let scene = vrml(source).unwrap();
        assert!((signed_volume(&scene) - 9.).abs() < 1e-9);
        let cap_area: f64 = scene
            .triangles
            .iter()
            .filter(|t| t.vertices.iter().all(|p| p.z == 3.))
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    / 2.
            })
            .sum();
        assert!((cap_area - 3.).abs() < 1e-9);
        let open = vrml(
            &source
                .replace("2 0 0 0]", "2 0]")
                .replace("convex FALSE", "convex FALSE beginCap FALSE endCap FALSE"),
        )
        .unwrap();
        assert_eq!(open.triangles.len(), 8);
        assert!(
            open.triangles
                .iter()
                .all(|t| t.vertices.iter().any(|p| p.z == 0.)
                    && t.vertices.iter().any(|p| p.z == 3.))
        );
        assert!(crate::render::render(&scene, &crate::render::RenderOptions::default()).is_ok());
    }
    #[test]
    fn extrusion_applies_spine_frame_scale_orientation_and_coincident_sections() {
        let source = r"#VRML V2.0 utf8
        Transform { translation 10 20 30 children [Shape { geometry Extrusion {
          spine [0 0 0 0 2 0] scale [1 1 2 2] orientation [0 1 0 0 0 1 0 1.5707963267948966]
        } }] }";
        let scene = vrml(source).unwrap();
        assert_eq!(scene.triangles.len(), 12);
        assert!(
            scene
                .positions()
                .any(|p| (p - Vec3::new(12., -28., 22.)).length() < 1e-9)
        );
        let along_x =
            vrml("#VRML V2.0 utf8\nShape { geometry Extrusion { spine [0 0 0 3 0 0] } }").unwrap();
        assert!((signed_volume(&along_x) - 12.).abs() < 1e-9);
        assert!(
            along_x
                .positions()
                .all(|p| (-1e-12..=3. + 1e-12).contains(&p.x))
        );
        let revolution = vrml("#VRML V2.0 utf8\nShape { geometry Extrusion { beginCap FALSE endCap FALSE crossSection [1 0 2 0] spine [0 0 0 0 0 0] orientation [0 1 0 0 0 1 0 1.5707963267948966] } }").unwrap();
        assert!(!revolution.triangles.is_empty());
        let bend = vrml(
            "#VRML V2.0 utf8\nShape { geometry Extrusion { spine [0 0 0 0 1 0 1 2 0 2 2 1] } }",
        )
        .unwrap();
        assert!(bend.positions().all(Vec3::is_finite));
        assert!(vrml(&source.replace("scale [1 1 2 2]", "scale [1 1 0 2]")).is_err());
    }
    #[test]
    fn invalid_inputs_are_errors() {
        for source in [
            "#VRML V1.0 ascii\nSeparator {}",
            "#VRML V2.0 utf8\nShape { geometry USE missing }",
            "#VRML V2.0 utf8\nShape { geometry Box { size 0 1 1 } }",
            "#VRML V2.0 utf8\nShape { geometry IndexedLineSet {} }",
            "#VRML V2.0 utf8\nShape { geometry Box {}",
            "#VRML V2.0 utf8\nPROTO thing [] {}",
            "#VRML V2.0 utf8\nInline { url \"model.wrl\" }",
        ] {
            assert!(vrml(source).is_err(), "{source}");
        }
        assert!(vrml("#VRML V2.0 utf8\nShape { geometry IndexedFaceSet { coord Coordinate { point [0 0 0 1 0 0 0 1 0] } coordIndex [0 1 3 -1] } }").is_err());
    }
}
