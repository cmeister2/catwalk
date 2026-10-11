//! Reading VRML scenes: the meshes a VRML97 (2.0) or VRML 1.0 file draws,
//! each placed by the transforms above it and coloured by its material.
//!
//! VRML97 is a tree of nodes. Group-like nodes (`Transform`, `Group`,
//! `Anchor`, `Billboard`, `Collision`, `LOD`'s first level, `Switch`'s
//! chosen child) are walked, `DEF` names a node and `USE` places it again,
//! and each `Shape` gives one mesh: an `IndexedFaceSet`'s polygons fanned
//! into triangles, or a `Box`, `Sphere`, `Cylinder` or `Cone` tessellated.
//! Prototypes, routes, scripts, sensors and interpolators draw nothing and
//! are passed over. VRML 1.0 is a state machine instead: `Separator`
//! saves and restores the state, `Coordinate3`, `Material` and the
//! transform nodes set it, and each `IndexedFaceSet` draws with it.

use ogeom_core::FastMap;
use std::rc::Rc;

use ogeom_core::{OgeomResult, ogeom_bail};
use ogeom_math::{Point, Vector};
use ogeom_topo::Triangulation;

use crate::mesh_formats::{ImportedMesh, Placement, normals_from_triangles};

/// Read the meshes a VRML file draws.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) if the
/// header names no VRML version read here, or the text does not parse.
pub fn read_vrml(text: &str) -> OgeomResult<Vec<ImportedMesh>> {
    let header = text.lines().next().unwrap_or("").trim();
    let version_one = if header.starts_with("#VRML V2.0") {
        false
    } else if header.starts_with("#VRML V1.0") {
        true
    } else {
        ogeom_bail!(Construction, "this is not a VRML 1.0 or 2.0 file");
    };
    let tokens = tokenize(text)?;
    let mut parser = Parser {
        tokens,
        at: 0,
        defined: FastMap::default(),
        depth: 0,
    };
    let mut roots = Vec::new();
    while parser.at < parser.tokens.len() {
        if let Some(node) = parser.statement()? {
            roots.push(node);
        }
    }
    let mut out = Vec::new();
    if version_one {
        let mut state = State::default();
        for node in &roots {
            walk_one(node, &mut state, &mut out)?;
        }
    } else {
        for node in &roots {
            walk_two(node, Placement::IDENTITY, &mut out)?;
        }
    }
    Ok(out)
}

// --- tokens and nodes ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Number(f64),
    Text(String),
    Open,
    Close,
    OpenList,
    CloseList,
}

fn tokenize(text: &str) -> OgeomResult<Vec<Token>> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '#' => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '{' | '}' | '[' | ']' => {
                chars.next();
                out.push(match c {
                    '{' => Token::Open,
                    '}' => Token::Close,
                    '[' => Token::OpenList,
                    _ => Token::CloseList,
                });
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                loop {
                    match chars.next() {
                        Some('\\') => {
                            if let Some(e) = chars.next() {
                                s.push(e);
                            }
                        }
                        Some('"') => break,
                        Some(c) => s.push(c),
                        None => ogeom_bail!(Construction, "a VRML string does not close"),
                    }
                }
                out.push(Token::Text(s));
            }
            // Bit masks' parentheses and bars separate words and say
            // nothing a mesh needs.
            c if c.is_whitespace() || matches!(c, ',' | '(' | ')' | '|') => {
                chars.next();
            }
            _ => {
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace()
                        || matches!(c, ',' | '{' | '}' | '[' | ']' | '"' | '#' | '(' | ')' | '|')
                    {
                        break;
                    }
                    word.push(c);
                    chars.next();
                }
                let numeric = word
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'));
                match (numeric, word.parse::<f64>()) {
                    (true, Ok(v)) => out.push(Token::Number(v)),
                    (true, Err(_)) if word.starts_with("0x") || word.starts_with("0X") => {
                        let v = i64::from_str_radix(&word[2..], 16).unwrap_or(0);
                        #[allow(clippy::cast_precision_loss)]
                        out.push(Token::Number(v as f64));
                    }
                    _ => out.push(Token::Word(word)),
                }
            }
        }
    }
    Ok(out)
}

/// A field's value: nodes, or a run of plain values.
#[derive(Debug, Clone)]
enum Value {
    Nodes(Vec<Rc<Node>>),
    Numbers(Vec<f64>),
    Words(Vec<String>),
}

#[derive(Debug)]
struct Node {
    kind: String,
    fields: Vec<(String, Value)>,
    /// A VRML 1.0 group's children, which stand among its fields unnamed.
    children: Vec<Rc<Node>>,
}

impl Node {
    fn field(&self, name: &str) -> Option<&Value> {
        self.fields.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    fn numbers(&self, name: &str) -> Option<&[f64]> {
        match self.field(name) {
            Some(Value::Numbers(v)) => Some(v),
            _ => None,
        }
    }

    fn number(&self, name: &str, default: f64) -> f64 {
        self.numbers(name)
            .and_then(|v| v.first().copied())
            .unwrap_or(default)
    }

    fn nodes(&self, name: &str) -> &[Rc<Node>] {
        match self.field(name) {
            Some(Value::Nodes(v)) => v,
            _ => &[],
        }
    }

    fn node(&self, name: &str) -> Option<&Rc<Node>> {
        self.nodes(name).first()
    }

    fn flag(&self, name: &str, default: bool) -> bool {
        match self.field(name) {
            Some(Value::Words(w)) => w.first().map_or(default, |w| w == "TRUE"),
            _ => default,
        }
    }
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
    defined: FastMap<String, Rc<Node>>,
    /// How many nodes enclose the one being read.
    depth: usize,
}

/// How deep nodes may nest. A scene graph nests a few dozen levels; a file
/// nesting thousands is hostile or broken, and reading it by recursion
/// would run the stack out.
const DEPTH: usize = 256;

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.at).cloned();
        self.at += 1;
        t
    }

    fn word(&mut self) -> OgeomResult<String> {
        match self.next() {
            Some(Token::Word(w)) => Ok(w),
            other => ogeom_bail!(Construction, "a VRML name was expected, found {other:?}"),
        }
    }

    /// Skip a balanced `{ }` or `[ ]` group starting at the cursor.
    fn skip_group(&mut self) {
        let mut depth = 0i32;
        while let Some(t) = self.next() {
            match t {
                Token::Open | Token::OpenList => depth += 1,
                Token::Close | Token::CloseList => {
                    depth -= 1;
                    if depth <= 0 {
                        return;
                    }
                }
                _ => {}
            }
            if depth == 0 {
                return;
            }
        }
    }

    /// A top-level or child statement: a node, or a prototype or route that
    /// draws nothing.
    fn statement(&mut self) -> OgeomResult<Option<Rc<Node>>> {
        match self.peek() {
            Some(Token::Word(w)) if w == "PROTO" => {
                self.next();
                self.word()?;
                self.skip_group();
                self.skip_group();
                Ok(None)
            }
            Some(Token::Word(w)) if w == "EXTERNPROTO" => {
                self.next();
                self.word()?;
                self.skip_group();
                // The URL list or string.
                if matches!(self.peek(), Some(Token::OpenList)) {
                    self.skip_group();
                } else {
                    self.next();
                }
                Ok(None)
            }
            Some(Token::Word(w)) if w == "ROUTE" => {
                // ROUTE a.out TO b.in
                self.at += 4;
                Ok(None)
            }
            Some(Token::Close | Token::CloseList) | None => {
                self.next();
                Ok(None)
            }
            _ => self.node().map(Some),
        }
    }

    fn node(&mut self) -> OgeomResult<Rc<Node>> {
        if self.depth >= DEPTH {
            ogeom_bail!(Construction, "the scene nests deeper than {DEPTH} nodes");
        }
        self.depth += 1;
        let node = self.node_within();
        self.depth -= 1;
        node
    }

    fn node_within(&mut self) -> OgeomResult<Rc<Node>> {
        let first = self.word()?;
        if first == "USE" {
            let name = self.word()?;
            return self.defined.get(&name).cloned().ok_or_else(|| {
                ogeom_core::ogeom_err!(Construction, "USE of {name} before its DEF")
            });
        }
        if first == "NULL" {
            return Ok(Rc::new(Node {
                kind: "NULL".into(),
                fields: Vec::new(),
                children: Vec::new(),
            }));
        }
        let (name, kind) = if first == "DEF" {
            let name = self.word()?;
            (Some(name), self.word()?)
        } else {
            (None, first)
        };
        if !matches!(self.next(), Some(Token::Open)) {
            ogeom_bail!(Construction, "a {kind} node opens with a brace");
        }
        let mut fields = Vec::new();
        let mut children = Vec::new();
        loop {
            match self.peek() {
                Some(Token::Close) => {
                    self.next();
                    break;
                }
                None => ogeom_bail!(Construction, "a {kind} node does not close"),
                Some(Token::Word(w)) if w == "ROUTE" || w == "PROTO" || w == "EXTERNPROTO" => {
                    self.statement()?;
                }
                Some(Token::Word(w)) if w == "DEF" || w == "USE" => {
                    children.push(self.node()?);
                }
                Some(Token::Word(_)) => {
                    let field = self.word()?;
                    // A VRML 1.0 child node: a name followed by a brace.
                    if matches!(self.peek(), Some(Token::Open)) {
                        self.at -= 1;
                        children.push(self.node()?);
                        continue;
                    }
                    // Interface declarations in a script: `field SFType name value`.
                    if matches!(field.as_str(), "eventIn" | "eventOut") {
                        self.word()?;
                        self.word()?;
                        continue;
                    }
                    if matches!(field.as_str(), "field" | "exposedField") {
                        self.word()?;
                        self.word()?;
                    }
                    let value = self.value()?;
                    fields.push((field, value));
                }
                _ => {
                    self.next();
                }
            }
        }
        let node = Rc::new(Node {
            kind,
            fields,
            children,
        });
        if let Some(name) = name {
            self.defined.insert(name, node.clone());
        }
        Ok(node)
    }

    fn value(&mut self) -> OgeomResult<Value> {
        match self.peek() {
            Some(Token::OpenList) => {
                self.next();
                let mut nodes = Vec::new();
                let mut numbers = Vec::new();
                let mut words = Vec::new();
                loop {
                    match self.peek() {
                        Some(Token::CloseList) => {
                            self.next();
                            break;
                        }
                        None => ogeom_bail!(Construction, "a VRML list does not close"),
                        Some(Token::Number(v)) => {
                            numbers.push(*v);
                            self.next();
                        }
                        Some(Token::Text(t)) => {
                            words.push(t.clone());
                            self.next();
                        }
                        Some(Token::Word(w)) if w == "TRUE" || w == "FALSE" => {
                            words.push(w.clone());
                            self.next();
                        }
                        Some(Token::Word(_)) => {
                            if let Some(n) = self.statement()? {
                                nodes.push(n);
                            }
                        }
                        _ => {
                            self.next();
                        }
                    }
                }
                Ok(if !nodes.is_empty() {
                    Value::Nodes(nodes)
                } else if !words.is_empty() {
                    Value::Words(words)
                } else {
                    Value::Numbers(numbers)
                })
            }
            Some(Token::Number(_)) => {
                let mut numbers = Vec::new();
                while let Some(Token::Number(v)) = self.peek() {
                    numbers.push(*v);
                    self.next();
                }
                Ok(Value::Numbers(numbers))
            }
            Some(Token::Text(_)) => {
                let Some(Token::Text(t)) = self.next() else {
                    unreachable!()
                };
                Ok(Value::Words(vec![t]))
            }
            Some(Token::Word(w)) if w == "TRUE" || w == "FALSE" => {
                let w = w.clone();
                self.next();
                Ok(Value::Words(vec![w]))
            }
            Some(Token::Word(w)) if w == "IS" => {
                self.next();
                self.word()?;
                Ok(Value::Words(Vec::new()))
            }
            Some(Token::Word(w))
                if w.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                    && !matches!(self.tokens.get(self.at + 1), Some(Token::Open)) =>
            {
                // An enumerated value (VRML 1.0 `SIDES`-style words come
                // upper case; a lower-case word here is the next field).
                Ok(Value::Words(Vec::new()))
            }
            Some(Token::Word(w))
                if w.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                    && w != "NULL"
                    && w != "DEF"
                    && w != "USE" =>
            {
                let w = w.clone();
                self.next();
                Ok(Value::Words(vec![w]))
            }
            Some(Token::Open) => {
                // A VRML 1.0 bit mask `( A | B )` never braces; skip what does.
                self.skip_group();
                Ok(Value::Words(Vec::new()))
            }
            _ => Ok(Value::Nodes(vec![self.node()?])),
        }
    }
}

// --- VRML97 ---------------------------------------------------------------------

fn walk_two(node: &Rc<Node>, placement: Placement, out: &mut Vec<ImportedMesh>) -> OgeomResult<()> {
    match node.kind.as_str() {
        "Transform" => {
            let here = placement.then(transform_of(node));
            for child in node.nodes("children") {
                walk_two(child, here, out)?;
            }
        }
        "Group" | "Anchor" | "Billboard" | "Collision" | "StaticGroup" => {
            for child in node.nodes("children") {
                walk_two(child, placement, out)?;
            }
        }
        "LOD" => {
            let levels = if node.nodes("level").is_empty() {
                node.nodes("children")
            } else {
                node.nodes("level")
            };
            if let Some(first) = levels.first() {
                walk_two(first, placement, out)?;
            }
        }
        "Switch" => {
            let choices = if node.nodes("choice").is_empty() {
                node.nodes("children")
            } else {
                node.nodes("choice")
            };
            let which = node.number("whichChoice", -1.0);
            if which >= 0.0 {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                if let Some(chosen) = choices.get(which as usize) {
                    walk_two(chosen, placement, out)?;
                }
            }
        }
        "Shape" => {
            let colour = node
                .node("appearance")
                .and_then(|a| a.node("material"))
                .map(|m| material_colour(m, "diffuseColor"));
            let Some(geometry) = node.node("geometry") else {
                return Ok(());
            };
            if let Some(mesh) = geometry_mesh(geometry)? {
                out.push(ImportedMesh {
                    mesh: placed(mesh, placement),
                    colour: colour.flatten(),
                    name: None,
                });
            }
        }
        _ => {}
    }
    Ok(())
}

/// A VRML97 `Transform`'s map: `T C R SR S -SR -C`.
fn transform_of(node: &Node) -> Placement {
    let triple = |name: &str, default: [f64; 3]| -> Vector {
        let v = node.numbers(name).unwrap_or(&[]);
        if v.len() >= 3 {
            Vector::new(v[0], v[1], v[2])
        } else {
            Vector::new(default[0], default[1], default[2])
        }
    };
    let rotation = |name: &str| -> Placement {
        let v = node.numbers(name).unwrap_or(&[]);
        if v.len() >= 4 {
            axis_angle(Vector::new(v[0], v[1], v[2]), v[3])
        } else {
            Placement::IDENTITY
        }
    };
    let translate = |v: Vector| Placement {
        translation: v,
        ..Placement::IDENTITY
    };
    let scale = triple("scale", [1.0, 1.0, 1.0]);
    let scaling = Placement {
        columns: [
            Vector::new(scale.x, 0.0, 0.0),
            Vector::new(0.0, scale.y, 0.0),
            Vector::new(0.0, 0.0, scale.z),
        ],
        translation: Vector::new(0.0, 0.0, 0.0),
    };
    let centre = triple("center", [0.0, 0.0, 0.0]);
    let so = rotation("scaleOrientation");
    let so_inverse = inverse_rotation(so);
    translate(triple("translation", [0.0, 0.0, 0.0]))
        .then(translate(centre))
        .then(rotation("rotation"))
        .then(so)
        .then(scaling)
        .then(so_inverse)
        .then(translate(-centre))
}

fn axis_angle(axis: Vector, angle: f64) -> Placement {
    let m = axis.magnitude();
    if m <= 0.0 {
        return Placement::IDENTITY;
    }
    let (x, y, z) = (axis.x / m, axis.y / m, axis.z / m);
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    Placement {
        columns: [
            Vector::new(t * x * x + c, t * x * y + s * z, t * x * z - s * y),
            Vector::new(t * x * y - s * z, t * y * y + c, t * y * z + s * x),
            Vector::new(t * x * z + s * y, t * y * z - s * x, t * z * z + c),
        ],
        translation: Vector::new(0.0, 0.0, 0.0),
    }
}

fn inverse_rotation(r: Placement) -> Placement {
    let [a, b, c] = r.columns;
    Placement {
        columns: [
            Vector::new(a.x, b.x, c.x),
            Vector::new(a.y, b.y, c.y),
            Vector::new(a.z, b.z, c.z),
        ],
        translation: Vector::new(0.0, 0.0, 0.0),
    }
}

fn material_colour(material: &Node, field: &str) -> Option<[f64; 4]> {
    let c = material.numbers(field)?;
    if c.len() < 3 {
        return None;
    }
    let transparency = material.number("transparency", 0.0);
    Some([c[0], c[1], c[2], 1.0 - transparency])
}

fn geometry_mesh(geometry: &Node) -> OgeomResult<Option<Triangulation>> {
    let (positions, triangles) = match geometry.kind.as_str() {
        "IndexedFaceSet" => {
            let points = geometry
                .node("coord")
                .and_then(|c| c.numbers("point"))
                .unwrap_or(&[]);
            let index = geometry.numbers("coordIndex").unwrap_or(&[]);
            faces(points, index, geometry.flag("ccw", true))?
        }
        "Box" => {
            let size = geometry.numbers("size").unwrap_or(&[2.0, 2.0, 2.0]);
            let s = if size.len() >= 3 {
                [size[0], size[1], size[2]]
            } else {
                [2.0; 3]
            };
            cuboid(s)
        }
        "Sphere" => ball(geometry.number("radius", 1.0)),
        "Cylinder" => lathe(
            geometry.number("radius", 1.0),
            geometry.number("radius", 1.0),
            geometry.number("height", 2.0),
            geometry.flag("bottom", true),
            geometry.flag("top", true),
            geometry.flag("side", true),
        ),
        "Cone" => lathe(
            geometry.number("bottomRadius", 1.0),
            0.0,
            geometry.number("height", 2.0),
            geometry.flag("bottom", true),
            false,
            geometry.flag("side", true),
        ),
        _ => return Ok(None),
    };
    if triangles.is_empty() {
        return Ok(None);
    }
    let normals = normals_from_triangles(&positions, &triangles);
    let parameters = vec![(0.0, 0.0); positions.len()];
    Ok(Some(Triangulation {
        positions,
        normals,
        parameters,
        triangles,
        deflection_met: true,
    }))
}

/// Polygons, `-1` apart, fanned into triangles.
fn faces(points: &[f64], index: &[f64], ccw: bool) -> OgeomResult<(Vec<Point>, Vec<[u32; 3]>)> {
    let positions: Vec<Point> = points
        .as_chunks::<3>()
        .0
        .iter()
        .map(|[x, y, z]| Point::new(*x, *y, *z))
        .collect();
    let mut triangles = Vec::new();
    let mut polygon: Vec<u32> = Vec::new();
    let close = |polygon: &mut Vec<u32>, triangles: &mut Vec<[u32; 3]>| {
        for k in 1..polygon.len().saturating_sub(1) {
            let t = [polygon[0], polygon[k], polygon[k + 1]];
            triangles.push(if ccw { t } else { [t[0], t[2], t[1]] });
        }
        polygon.clear();
    };
    for &i in index {
        if i < 0.0 {
            close(&mut polygon, &mut triangles);
            continue;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = i as usize;
        if i >= positions.len() {
            ogeom_bail!(
                Construction,
                "a face names point {i} of {}",
                positions.len()
            );
        }
        #[allow(clippy::cast_possible_truncation)]
        polygon.push(i as u32);
    }
    close(&mut polygon, &mut triangles);
    Ok((positions, triangles))
}

fn cuboid(size: [f64; 3]) -> (Vec<Point>, Vec<[u32; 3]>) {
    let [x, y, z] = size.map(|s| s * 0.5);
    let positions: Vec<Point> = (0..8)
        .map(|i| {
            Point::new(
                if i & 1 == 0 { -x } else { x },
                if i & 2 == 0 { -y } else { y },
                if i & 4 == 0 { -z } else { z },
            )
        })
        .collect();
    let quads = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ];
    let triangles = quads
        .iter()
        .flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]])
        .collect();
    (positions, triangles)
}

const AROUND: u32 = 32;

fn ball(radius: f64) -> (Vec<Point>, Vec<[u32; 3]>) {
    let rows = AROUND / 2;
    let mut positions = Vec::new();
    for i in 0..=rows {
        let phi = core::f64::consts::PI * f64::from(i) / f64::from(rows);
        for j in 0..AROUND {
            let theta = core::f64::consts::TAU * f64::from(j) / f64::from(AROUND);
            positions.push(Point::new(
                radius * phi.sin() * theta.sin(),
                radius * phi.cos(),
                radius * phi.sin() * theta.cos(),
            ));
        }
    }
    let mut triangles = Vec::new();
    for i in 0..rows {
        for j in 0..AROUND {
            let k = (j + 1) % AROUND;
            let (a, b) = (i * AROUND + j, i * AROUND + k);
            let (c, d) = ((i + 1) * AROUND + j, (i + 1) * AROUND + k);
            if i > 0 {
                triangles.push([a, c, b]);
            }
            if i + 1 < rows {
                triangles.push([b, c, d]);
            }
        }
    }
    (positions, triangles)
}

/// A drum or cone about `y`, centred, with the caps asked for.
fn lathe(
    bottom: f64,
    top: f64,
    height: f64,
    bottom_cap: bool,
    top_cap: bool,
    side: bool,
) -> (Vec<Point>, Vec<[u32; 3]>) {
    let h = height * 0.5;
    let mut positions = Vec::new();
    let ring = |positions: &mut Vec<Point>, r: f64, y: f64| -> u32 {
        #[allow(clippy::cast_possible_truncation)]
        let start = positions.len() as u32;
        for j in 0..AROUND {
            let theta = core::f64::consts::TAU * f64::from(j) / f64::from(AROUND);
            positions.push(Point::new(r * theta.sin(), y, r * theta.cos()));
        }
        start
    };
    let low = ring(&mut positions, bottom, -h);
    let high = ring(&mut positions, top, h);
    #[allow(clippy::cast_possible_truncation)]
    let centres = positions.len() as u32;
    positions.push(Point::new(0.0, -h, 0.0));
    positions.push(Point::new(0.0, h, 0.0));
    let mut triangles = Vec::new();
    for j in 0..AROUND {
        let k = (j + 1) % AROUND;
        if side {
            triangles.push([low + j, low + k, high + j]);
            if top > 0.0 {
                triangles.push([low + k, high + k, high + j]);
            }
        }
        if bottom_cap && bottom > 0.0 {
            triangles.push([centres, low + k, low + j]);
        }
        if top_cap && top > 0.0 {
            triangles.push([centres + 1, high + j, high + k]);
        }
    }
    (positions, triangles)
}

fn placed(mut mesh: Triangulation, placement: Placement) -> Triangulation {
    for p in &mut mesh.positions {
        *p = placement.point(*p);
    }
    if placement.mirrors() {
        // A mirroring map turns the winding inside out; turn it back.
        for t in &mut mesh.triangles {
            t.swap(1, 2);
        }
    }
    mesh.normals = normals_from_triangles(&mesh.positions, &mesh.triangles);
    mesh
}

// --- VRML 1.0 --------------------------------------------------------------------

#[derive(Clone)]
struct State {
    placement: Placement,
    points: Rc<Vec<f64>>,
    colour: Option<[f64; 4]>,
    ccw: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            placement: Placement::IDENTITY,
            points: Rc::new(Vec::new()),
            colour: None,
            ccw: true,
        }
    }
}

fn walk_one(node: &Rc<Node>, state: &mut State, out: &mut Vec<ImportedMesh>) -> OgeomResult<()> {
    let triple = |name: &str, default: f64| -> Vector {
        let v = node.numbers(name).unwrap_or(&[]);
        if v.len() >= 3 {
            Vector::new(v[0], v[1], v[2])
        } else {
            Vector::new(default, default, default)
        }
    };
    match node.kind.as_str() {
        "Separator" | "TransformSeparator" => {
            let mut inner = state.clone();
            for child in &node.children {
                walk_one(child, &mut inner, out)?;
            }
            if node.kind == "TransformSeparator" {
                let placement = state.placement;
                *state = inner;
                state.placement = placement;
            }
        }
        "Group" => {
            for child in &node.children {
                walk_one(child, state, out)?;
            }
        }
        "Switch" => {
            let which = node.number("whichChild", -1.0);
            if which >= 0.0 {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                if let Some(chosen) = node.children.get(which as usize) {
                    walk_one(chosen, state, out)?;
                }
            } else if which <= -3.0 {
                for child in &node.children {
                    walk_one(child, state, out)?;
                }
            }
        }
        "Coordinate3" => {
            state.points = Rc::new(node.numbers("point").unwrap_or(&[]).to_vec());
        }
        "Material" => {
            state.colour = material_colour(node, "diffuseColor");
        }
        "ShapeHints" => {
            if let Some(Value::Words(w)) = node.field("vertexOrdering") {
                state.ccw = w.first().is_none_or(|w| w != "CLOCKWISE");
            }
        }
        "Translation" => {
            state.placement = state.placement.then(Placement {
                translation: triple("translation", 0.0),
                ..Placement::IDENTITY
            });
        }
        "Rotation" => {
            let v = node.numbers("rotation").unwrap_or(&[]);
            if v.len() >= 4 {
                state.placement = state
                    .placement
                    .then(axis_angle(Vector::new(v[0], v[1], v[2]), v[3]));
            }
        }
        "Scale" => {
            let s = triple("scaleFactor", 1.0);
            state.placement = state.placement.then(Placement {
                columns: [
                    Vector::new(s.x, 0.0, 0.0),
                    Vector::new(0.0, s.y, 0.0),
                    Vector::new(0.0, 0.0, s.z),
                ],
                translation: Vector::new(0.0, 0.0, 0.0),
            });
        }
        "Transform" => {
            let mut renamed: Vec<(String, Value)> = node.fields.clone();
            for (name, _) in &mut renamed {
                if name == "scaleFactor" {
                    *name = "scale".into();
                }
            }
            let as_two = Node {
                kind: "Transform".into(),
                fields: renamed,
                children: Vec::new(),
            };
            state.placement = state.placement.then(transform_of(&as_two));
        }
        "MatrixTransform" => {
            let m = node.numbers("matrix").unwrap_or(&[]);
            if m.len() >= 16 {
                // Row vectors: each row is where an axis goes.
                state.placement = state.placement.then(Placement {
                    columns: [
                        Vector::new(m[0], m[1], m[2]),
                        Vector::new(m[4], m[5], m[6]),
                        Vector::new(m[8], m[9], m[10]),
                    ],
                    translation: Vector::new(m[12], m[13], m[14]),
                });
            }
        }
        "IndexedFaceSet" => {
            let index = node.numbers("coordIndex").unwrap_or(&[]);
            let (positions, triangles) = faces(&state.points, index, state.ccw)?;
            // Only the points the faces use.
            let mut used: FastMap<u32, u32> = FastMap::default();
            let mut kept = Vec::new();
            let triangles: Vec<[u32; 3]> = triangles
                .iter()
                .map(|t| {
                    t.map(|i| {
                        *used.entry(i).or_insert_with(|| {
                            kept.push(positions[i as usize]);
                            #[allow(clippy::cast_possible_truncation)]
                            let at = (kept.len() - 1) as u32;
                            at
                        })
                    })
                })
                .collect();
            if !triangles.is_empty() {
                let normals = normals_from_triangles(&kept, &triangles);
                let parameters = vec![(0.0, 0.0); kept.len()];
                let mesh = Triangulation {
                    positions: kept,
                    normals,
                    parameters,
                    triangles,
                    deflection_met: true,
                };
                out.push(ImportedMesh {
                    mesh: placed(mesh, state.placement),
                    colour: state.colour,
                    name: None,
                });
            }
        }
        "Cube" | "Sphere" | "Cylinder" | "Cone" => {
            let (positions, triangles) = match node.kind.as_str() {
                "Cube" => cuboid([
                    node.number("width", 2.0),
                    node.number("height", 2.0),
                    node.number("depth", 2.0),
                ]),
                "Sphere" => ball(node.number("radius", 1.0)),
                "Cylinder" => lathe(
                    node.number("radius", 1.0),
                    node.number("radius", 1.0),
                    node.number("height", 2.0),
                    true,
                    true,
                    true,
                ),
                _ => lathe(
                    node.number("bottomRadius", 1.0),
                    0.0,
                    node.number("height", 2.0),
                    true,
                    false,
                    true,
                ),
            };
            let normals = normals_from_triangles(&positions, &triangles);
            let parameters = vec![(0.0, 0.0); positions.len()];
            out.push(ImportedMesh {
                mesh: placed(
                    Triangulation {
                        positions,
                        normals,
                        parameters,
                        triangles,
                        deflection_met: true,
                    },
                    state.placement,
                ),
                colour: state.colour,
                name: None,
            });
        }
        _ => {}
    }
    Ok(())
}
