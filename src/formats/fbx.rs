//! Binary FBX is decoded by pure Rust fbxcel; the ASCII front end feeds the
//! same bounded tree. Transform composition follows Autodesk's documented
//! pivot chain and three scaling-inheritance modes.
//! https://help.autodesk.com/cloudhelp/2019/ENU/FBX-Developer-Help/cpp_ref/_transformations_2main_8cxx-example.html
use super::{MAX_INPUT_BYTES, mesh::triangulate_polygon, read_file, transform::Transform};
use crate::{
    Result,
    scene::{Line, Point, Scene, Triangle, Vec3},
};
use fbxcel::{
    low::v7400::AttributeValue as Value,
    tree::{
        any::AnyTree,
        v7400::{NodeId, Tree},
    },
};
use std::{
    collections::{HashMap, HashSet},
    io::Cursor,
    path::Path,
};

const MAX_ELEMENTS: usize = 5_000_000;
const MAX_NODES: usize = 500_000;
const MAX_DEPTH: usize = 256;
const MAGIC: &[u8] = b"Kaydara FBX Binary  \0\x1a\0";

#[derive(Debug)]
struct Node {
    name: String,
    values: Vec<Value>,
    children: Vec<Node>,
}
impl Node {
    fn child(&self, name: &str) -> Option<&Self> {
        self.children.iter().find(|n| n.name == name)
    }
    fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Self> {
        self.children.iter().filter(move |n| n.name == name)
    }
    fn text(&self) -> Option<&str> {
        self.values.first().and_then(Value::get_string)
    }
}

pub(super) fn load(path: &Path) -> Result<Scene> {
    let mut bytes = read_file(path)?;
    let tree = if bytes.starts_with(MAGIC) {
        for offset in preflight_binary(&bytes)? {
            // Some exporters use the nonstandard byte-array code `c` for
            // embedded pixels. Its layout matches `b`; the resulting values
            // are opaque metadata and never enter geometry or color decoding.
            bytes[offset] = b'b';
        }
        let any = AnyTree::from_seekable_reader(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        match any {
            AnyTree::V7400(_, mut tree, _) => {
                let root = tree.root().node_id();
                take_node(&mut tree, root, 0)?
            }
            _ => return Err("unsupported FBX binary version".into()),
        }
    } else {
        Ascii::new(std::str::from_utf8(&bytes)?)?.parse()?
    };
    build_scene(&tree)
}

fn take_node(tree: &mut Tree, id: NodeId, depth: usize) -> Result<Node> {
    if depth > MAX_DEPTH {
        return Err("FBX tree exceeds 256 levels".into());
    }
    let handle = id.to_handle(tree);
    let name = handle.name().to_owned();
    let ids: Vec<_> = handle.children().map(|n| n.node_id()).collect();
    let values = tree.take_attributes_vec(id);
    let children = ids
        .into_iter()
        .map(|id| take_node(tree, id, depth + 1))
        .collect::<Result<_>>()?;
    Ok(Node {
        name,
        values,
        children,
    })
}

/// Validate array allocations before passing compressed properties to fbxcel.
fn preflight_binary(bytes: &[u8]) -> Result<Vec<usize>> {
    let version = u32::from_le_bytes(
        bytes
            .get(23..27)
            .ok_or("truncated FBX header")?
            .try_into()?,
    );
    if !(7000..8000).contains(&version) {
        return Err(format!("unsupported binary FBX version {version}; expected FBX 7.x").into());
    }
    let wide = version >= 7500;
    let mut scan = BinaryScan {
        bytes,
        wide,
        nodes: 0,
        decoded: 0,
        opaque_bytes: Vec::new(),
    };
    let mut offset = 27;
    let mut terminated = false;
    while offset < bytes.len() {
        match scan.node(offset, 0)? {
            Some(next) => offset = next,
            None => {
                terminated = true;
                break;
            }
        }
    }
    if !terminated {
        return Err("truncated FBX root terminator".into());
    }
    Ok(scan.opaque_bytes)
}
struct BinaryScan<'a> {
    bytes: &'a [u8],
    wide: bool,
    nodes: usize,
    decoded: u64,
    opaque_bytes: Vec<usize>,
}
impl BinaryScan<'_> {
    fn uint(&self, offset: usize, wide: bool) -> Result<u64> {
        let len = if wide { 8 } else { 4 };
        let bytes = self
            .bytes
            .get(offset..offset.checked_add(len).ok_or("FBX offset overflow")?)
            .ok_or("truncated FBX node")?;
        Ok(if wide {
            u64::from_le_bytes(bytes.try_into()?)
        } else {
            u32::from_le_bytes(bytes.try_into()?).into()
        })
    }
    fn node(&mut self, offset: usize, depth: usize) -> Result<Option<usize>> {
        if depth > MAX_DEPTH {
            return Err("FBX tree exceeds 256 levels".into());
        }
        let n = if self.wide { 8 } else { 4 };
        let header = n * 3 + 1;
        let end = usize::try_from(self.uint(offset, self.wide)?)?;
        let count = usize::try_from(self.uint(offset + n, self.wide)?)?;
        let property_bytes = usize::try_from(self.uint(offset + 2 * n, self.wide)?)?;
        let name_len = usize::from(
            *self
                .bytes
                .get(offset + 3 * n)
                .ok_or("truncated FBX node name")?,
        );
        if end == 0 {
            if count != 0 || property_bytes != 0 || name_len != 0 {
                return Err("invalid FBX null record".into());
            }
            return Ok(None);
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES || count > MAX_ELEMENTS {
            return Err("FBX tree or property count exceeds its limit".into());
        }
        let mut cursor = offset
            .checked_add(header + name_len)
            .ok_or("FBX node offset overflow")?;
        let properties_end = cursor
            .checked_add(property_bytes)
            .ok_or("FBX properties overflow")?;
        if end > self.bytes.len() || properties_end > end || cursor > end {
            return Err("FBX node lies outside its parent/input".into());
        }
        let name = &self.bytes[offset + header..offset + header + name_len];
        for _ in 0..count {
            let ty = *self.bytes.get(cursor).ok_or("truncated FBX property")?;
            cursor += 1;
            let width = match ty {
                b'Y' => 2,
                b'C' => 1,
                b'I' | b'F' => 4,
                b'D' | b'L' => 8,
                b'S' | b'R' => {
                    let len = usize::try_from(self.uint(cursor, false)?)?;
                    cursor += 4;
                    len
                }
                b'f' | b'd' | b'l' | b'i' | b'b' | b'c' => {
                    if ty == b'c' {
                        if name != b"ImageData" && name != b"Content" {
                            return Err(
                                "FBX byte arrays are only supported for opaque image data".into()
                            );
                        }
                        self.opaque_bytes.push(cursor - 1);
                    }
                    let count = self.uint(cursor, false)?;
                    let encoding = self.uint(cursor + 4, false)?;
                    let size = self.uint(cursor + 8, false)?;
                    cursor += 12;
                    let element_size = match ty {
                        b'd' | b'l' => 8,
                        b'f' | b'i' => 4,
                        _ => 1,
                    };
                    let decoded = count
                        .checked_mul(element_size)
                        .ok_or("FBX array byte count overflow")?;
                    self.decoded = self
                        .decoded
                        .checked_add(decoded)
                        .ok_or("FBX decoded byte count overflow")?;
                    if count > MAX_ELEMENTS as u64 || self.decoded > MAX_INPUT_BYTES {
                        return Err("FBX arrays exceed their element/512 MiB limit".into());
                    }
                    if encoding > 1 || (encoding == 0 && size != decoded) {
                        return Err("invalid FBX array encoding or byte length".into());
                    }
                    usize::try_from(size)?
                }
                _ => return Err(format!("unknown FBX property type {}", ty as char).into()),
            };
            cursor = cursor
                .checked_add(width)
                .ok_or("FBX property offset overflow")?;
            if cursor > properties_end {
                return Err("FBX property exceeds its node".into());
            }
        }
        if cursor != properties_end {
            return Err("FBX property byte count mismatch".into());
        }
        while cursor < end {
            match self.node(cursor, depth + 1)? {
                Some(next) if next <= end && next > cursor => cursor = next,
                Some(_) => return Err("FBX child exceeds its parent".into()),
                None => {
                    if cursor + header != end {
                        return Err("FBX null record must terminate its node".into());
                    }
                    cursor = end;
                }
            }
        }
        Ok(Some(end))
    }
}

#[derive(Debug, Clone)]
enum Token {
    Word(String),
    Text(String),
    Colon,
    Comma,
    Open,
    Close,
    Star,
    Newline,
}
struct Ascii {
    tokens: Vec<Token>,
    cursor: usize,
    nodes: usize,
    values: usize,
}
impl Ascii {
    fn new(text: &str) -> Result<Self> {
        let bytes = text.as_bytes();
        let mut i = 0;
        let mut tokens = Vec::new();
        while i < bytes.len() {
            let token = match bytes[i] {
                b' ' | b'\t' | b'\r' => {
                    i += 1;
                    continue;
                }
                b';' => {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                    continue;
                }
                b'\n' => {
                    i += 1;
                    Token::Newline
                }
                b':' => {
                    i += 1;
                    Token::Colon
                }
                b',' => {
                    i += 1;
                    Token::Comma
                }
                b'{' => {
                    i += 1;
                    Token::Open
                }
                b'}' => {
                    i += 1;
                    Token::Close
                }
                b'*' => {
                    i += 1;
                    Token::Star
                }
                b'"' => {
                    i += 1;
                    let mut s = Vec::new();
                    let mut closed = false;
                    while i < bytes.len() {
                        match bytes[i] {
                            b'"' => {
                                i += 1;
                                closed = true;
                                break;
                            }
                            b'\\' if bytes.get(i + 1).is_some_and(|&b| b == b'"' || b == b'\\') => {
                                i += 1;
                                s.push(bytes[i]);
                                i += 1;
                            }
                            b => {
                                s.push(b);
                                i += 1;
                            }
                        }
                    }
                    if !closed {
                        return Err("unterminated FBX ASCII string".into());
                    }
                    Token::Text(String::from_utf8(s)?)
                }
                _ => {
                    let start = i;
                    while i < bytes.len()
                        && !matches!(
                            bytes[i],
                            b' ' | b'\t'
                                | b'\r'
                                | b'\n'
                                | b':'
                                | b','
                                | b'{'
                                | b'}'
                                | b';'
                                | b'"'
                                | b'*'
                        )
                    {
                        i += 1;
                    }
                    if start == i {
                        return Err("invalid FBX ASCII character".into());
                    }
                    Token::Word(text[start..i].to_owned())
                }
            };
            tokens.push(token);
            if tokens.len() > MAX_ELEMENTS * 2 {
                return Err("FBX ASCII token count exceeds its limit".into());
            }
        }
        Ok(Self {
            tokens,
            cursor: 0,
            nodes: 0,
            values: 0,
        })
    }
    fn parse(mut self) -> Result<Node> {
        let children = self.children(0, false)?;
        if self.cursor != self.tokens.len() {
            return Err("trailing FBX ASCII tokens".into());
        }
        Ok(Node {
            name: String::new(),
            values: Vec::new(),
            children,
        })
    }
    fn skip_lines(&mut self) {
        while matches!(self.tokens.get(self.cursor), Some(Token::Newline)) {
            self.cursor += 1;
        }
    }
    fn children(&mut self, depth: usize, closed: bool) -> Result<Vec<Node>> {
        if depth > MAX_DEPTH {
            return Err("FBX ASCII tree exceeds 256 levels".into());
        }
        let mut nodes = Vec::new();
        loop {
            self.skip_lines();
            match self.tokens.get(self.cursor) {
                None if !closed => break,
                None => return Err("unterminated FBX ASCII node".into()),
                Some(Token::Close) if closed => {
                    self.cursor += 1;
                    break;
                }
                _ => nodes.push(self.node(depth)?),
            }
        }
        Ok(nodes)
    }
    fn node(&mut self, depth: usize) -> Result<Node> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err("FBX ASCII exceeds 500,000 nodes".into());
        }
        let Some(Token::Word(name)) = self.tokens.get(self.cursor).cloned() else {
            return Err("FBX ASCII expected a node name".into());
        };
        self.cursor += 1;
        if !matches!(self.tokens.get(self.cursor), Some(Token::Colon)) {
            return Err("FBX ASCII expected ':'".into());
        }
        self.cursor += 1;
        let mut values = Vec::new();
        let mut continuation = false;
        loop {
            let value = match self.tokens.get(self.cursor).cloned() {
                Some(Token::Comma) => {
                    self.cursor += 1;
                    continuation = true;
                    continue;
                }
                Some(Token::Newline) if continuation => {
                    self.cursor += 1;
                    continue;
                }
                Some(Token::Word(_))
                    if matches!(self.tokens.get(self.cursor + 1), Some(Token::Colon)) =>
                {
                    break;
                }
                Some(Token::Word(w)) => {
                    self.cursor += 1;
                    if let Ok(n) = w.parse::<i64>() {
                        Value::I64(n)
                    } else if let Ok(n) = w.parse::<f64>() {
                        Value::F64(n)
                    } else {
                        Value::String(w)
                    }
                }
                Some(Token::Text(s)) => {
                    self.cursor += 1;
                    Value::String(s)
                }
                Some(Token::Star) => {
                    self.cursor += 1;
                    let Some(Token::Word(count)) = self.tokens.get(self.cursor) else {
                        return Err("FBX ASCII array needs a count".into());
                    };
                    let count = count.parse::<usize>()?;
                    if count > MAX_ELEMENTS {
                        return Err("FBX ASCII array exceeds five million elements".into());
                    }
                    self.cursor += 1;
                    Value::String(format!("*{count}"))
                }
                _ => break,
            };
            continuation = false;
            values.push(value);
            self.values += 1;
            if self.values > MAX_ELEMENTS {
                return Err("FBX ASCII property count exceeds five million".into());
            }
        }
        self.skip_lines();
        let children = if matches!(self.tokens.get(self.cursor), Some(Token::Open)) {
            self.cursor += 1;
            self.children(depth + 1, true)?
        } else {
            Vec::new()
        };
        Ok(Node {
            name,
            values,
            children,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Id {
    Number(i64),
    Name(String),
}
fn id(v: &Value) -> Result<Id> {
    match v {
        Value::I64(v) => Ok(Id::Number(*v)),
        Value::I32(v) => Ok(Id::Number((*v).into())),
        Value::String(v) => Ok(Id::Name(v.clone())),
        _ => Err("FBX object IDs must be integers or legacy names".into()),
    }
}
fn scalar(v: &Value) -> Result<f64> {
    let n = match v {
        Value::Bool(v) => f64::from(*v),
        Value::I16(v) => (*v).into(),
        Value::I32(v) => (*v).into(),
        Value::I64(v) => *v as f64,
        Value::F32(v) => (*v).into(),
        Value::F64(v) => *v,
        _ => return Err("FBX expected a numeric value".into()),
    };
    if !n.is_finite() {
        return Err("FBX numeric value must be finite".into());
    }
    Ok(n)
}
fn integer(v: &Value) -> Result<i64> {
    match v {
        Value::I64(v) => Ok(*v),
        Value::I32(v) => Ok((*v).into()),
        Value::I16(v) => Ok((*v).into()),
        _ => Err("FBX expected an integer".into()),
    }
}
fn array(node: &Node) -> Result<Vec<f64>> {
    if let Some(a) = node.child("a") {
        let values = a.values.iter().map(scalar).collect::<Result<Vec<_>>>()?;
        if let Some(count) = node.text().and_then(|v| v.strip_prefix('*'))
            && count.parse::<usize>()? != values.len()
        {
            return Err("FBX ASCII array count mismatch".into());
        }
        return Ok(values);
    }
    match node.values.as_slice() {
        [Value::ArrF64(v)] => Ok(v.clone()),
        [Value::ArrF32(v)] => Ok(v.iter().map(|&v| v.into()).collect()),
        [Value::ArrI32(v)] => Ok(v.iter().map(|&v| v.into()).collect()),
        [Value::ArrI64(v)] => Ok(v.iter().map(|&v| v as f64).collect()),
        [Value::ArrBool(v)] => Ok(v.iter().map(|&v| f64::from(v)).collect()),
        values => values.iter().map(scalar).collect(),
    }
}
fn int_array(node: &Node) -> Result<Vec<i64>> {
    if let Some(a) = node.child("a") {
        let values = a.values.iter().map(integer).collect::<Result<Vec<_>>>()?;
        if let Some(count) = node.text().and_then(|v| v.strip_prefix('*'))
            && count.parse::<usize>()? != values.len()
        {
            return Err("FBX ASCII array count mismatch".into());
        }
        return Ok(values);
    }
    match node.values.as_slice() {
        [Value::ArrI64(v)] => Ok(v.clone()),
        [Value::ArrI32(v)] => Ok(v.iter().map(|&v| v.into()).collect()),
        values => values.iter().map(integer).collect(),
    }
}

#[derive(Clone, Default)]
struct Properties<'a>(HashMap<&'a str, &'a [Value]>);
impl<'a> Properties<'a> {
    fn add(&mut self, node: &'a Node) {
        for block in node
            .children
            .iter()
            .filter(|n| n.name == "Properties70" || n.name == "Properties60")
        {
            for p in &block.children {
                let offset = if p.name == "P" {
                    4
                } else if p.name == "Property" {
                    3
                } else {
                    continue;
                };
                if let Some(name) = p.values.first().and_then(Value::get_string)
                    && let Some(values) = p.values.get(offset..)
                {
                    self.0.insert(name, values);
                }
            }
        }
    }
    fn num(&self, name: &str, default: f64) -> Result<f64> {
        match self.0.get(name) {
            Some([v]) => scalar(v),
            None => Ok(default),
            _ => Err(format!("FBX property {name} needs one numeric value").into()),
        }
    }
    fn vec(&self, name: &str, default: Vec3) -> Result<Vec3> {
        match self.0.get(name) {
            Some([x, y, z]) => Ok(Vec3::new(scalar(x)?, scalar(y)?, scalar(z)?)),
            None => Ok(default),
            _ => Err(format!("FBX property {name} needs three numeric values").into()),
        }
    }
}
fn defaults<'a>(root: &'a Node, class: &str) -> Properties<'a> {
    let mut properties = Properties::default();
    if let Some(definitions) = root.child("Definitions") {
        for object in definitions
            .children("ObjectType")
            .filter(|n| n.text() == Some(class))
        {
            for template in object.children("PropertyTemplate") {
                properties.add(template);
            }
        }
    }
    properties
}
fn translation(v: Vec3) -> Transform {
    Transform([
        [1., 0., 0., 0.],
        [0., 1., 0., 0.],
        [0., 0., 1., 0.],
        [v.x, v.y, v.z, 1.],
    ])
}
fn scale(v: Vec3) -> Transform {
    Transform([
        [v.x, 0., 0., 0.],
        [0., v.y, 0., 0.],
        [0., 0., v.z, 0.],
        [0., 0., 0., 1.],
    ])
}
fn inverse_rotation(m: Transform) -> Transform {
    let mut inverse = Transform::IDENTITY;
    for c in 0..3 {
        for r in 0..3 {
            inverse.0[c][r] = m.0[r][c];
        }
    }
    inverse
}
fn rotation(v: Vec3, order: f64) -> Result<Transform> {
    let [x, y, z] = [v.x.to_radians(), v.y.to_radians(), v.z.to_radians()];
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    let axes = [
        Transform([
            [1., 0., 0., 0.],
            [0., cx, sx, 0.],
            [0., -sx, cx, 0.],
            [0., 0., 0., 1.],
        ]),
        Transform([
            [cy, 0., -sy, 0.],
            [0., 1., 0., 0.],
            [sy, 0., cy, 0.],
            [0., 0., 0., 1.],
        ]),
        Transform([
            [cz, sz, 0., 0.],
            [-sz, cz, 0., 0.],
            [0., 0., 1., 0.],
            [0., 0., 0., 1.],
        ]),
    ];
    let sequence = match order {
        0. => [0, 1, 2],
        1. => [0, 2, 1],
        2. => [1, 2, 0],
        3. => [1, 0, 2],
        4. => [2, 0, 1],
        5. => [2, 1, 0],
        _ => {
            return Err(
                "unsupported FBX rotation order (spherical rotations are not supported)".into(),
            );
        }
    };
    Ok(sequence
        .into_iter()
        .fold(Transform::IDENTITY, |m, i| axes[i].then(m)))
}
fn finite(m: Transform) -> Result<Transform> {
    if m.0.iter().flatten().any(|v| !v.is_finite()) {
        return Err("FBX transform exceeds finite numeric range".into());
    }
    Ok(m)
}

struct Model<'a> {
    node: &'a Node,
    properties: Properties<'a>,
    parent: Option<Id>,
    geometries: Vec<Id>,
    materials: Vec<Id>,
    animated: HashMap<String, Vec3>,
}
#[derive(Clone, Copy)]
struct ModelState {
    world: Transform,
    rotation: Transform,
    local_scale: Vec3,
    hidden: bool,
}
fn local_transform(model: &Model<'_>) -> Result<(Transform, Transform, Vec3, Transform)> {
    let p = &model.properties;
    let zero = Vec3::default();
    let one = Vec3::new(1., 1., 1.);
    let t = model
        .animated
        .get("Lcl Translation")
        .copied()
        .unwrap_or(p.vec("Lcl Translation", zero)?);
    let r = model
        .animated
        .get("Lcl Rotation")
        .copied()
        .unwrap_or(p.vec("Lcl Rotation", zero)?);
    let s = model
        .animated
        .get("Lcl Scaling")
        .copied()
        .unwrap_or(p.vec("Lcl Scaling", one)?);
    let rotate = rotation(p.vec("PreRotation", zero)?, 0.)?
        .then(rotation(r, p.num("RotationOrder", 0.)?)?)
        .then(inverse_rotation(rotation(
            p.vec("PostRotation", zero)?,
            0.,
        )?));
    let rp = p.vec("RotationPivot", zero)?;
    let sp = p.vec("ScalingPivot", zero)?;
    let local = translation(t)
        .then(translation(p.vec("RotationOffset", zero)?))
        .then(translation(rp))
        .then(rotate)
        .then(translation(-rp))
        .then(translation(p.vec("ScalingOffset", zero)?))
        .then(translation(sp))
        .then(scale(s))
        .then(translation(-sp));
    let geometric = translation(p.vec("GeometricTranslation", zero)?)
        .then(rotation(p.vec("GeometricRotation", zero)?, 0.)?)
        .then(scale(p.vec("GeometricScaling", one)?));
    Ok((finite(local)?, rotate, s, finite(geometric)?))
}

fn model_state(
    id: &Id,
    models: &HashMap<Id, Model<'_>>,
    cache: &mut HashMap<Id, ModelState>,
    active: &mut HashSet<Id>,
) -> Result<ModelState> {
    if let Some(state) = cache.get(id) {
        return Ok(*state);
    }
    if active.len() >= MAX_DEPTH || !active.insert(id.clone()) {
        return Err("cyclic or excessively deep FBX model hierarchy".into());
    }
    let model = &models[id];
    let parent = match &model.parent {
        Some(parent) => model_state(parent, models, cache, active)?,
        None => ModelState {
            world: Transform::IDENTITY,
            rotation: Transform::IDENTITY,
            local_scale: Vec3::new(1., 1., 1.),
            hidden: false,
        },
    };
    let (local, rot, s, _) = local_transform(model)?;
    let mut linear = parent.world;
    linear.0[3] = [0., 0., 0., 1.];
    let parent_scale = inverse_rotation(parent.rotation).then(linear);
    let rs = match model.properties.num("InheritType", 0.)? {
        0. => parent.rotation.then(rot).then(parent_scale).then(scale(s)),
        1. => linear.then(rot).then(scale(s)),
        2. => {
            if parent.local_scale.x == 0.
                || parent.local_scale.y == 0.
                || parent.local_scale.z == 0.
            {
                return Err("FBX Rrs inheritance cannot invert a zero parent scale".into());
            }
            parent
                .rotation
                .then(rot)
                .then(parent_scale)
                .then(scale(Vec3::new(
                    1. / parent.local_scale.x,
                    1. / parent.local_scale.y,
                    1. / parent.local_scale.z,
                )))
                .then(scale(s))
        }
        _ => return Err("unsupported FBX scaling inheritance type".into()),
    };
    let local_t = Vec3::new(local.0[3][0], local.0[3][1], local.0[3][2]);
    let world = finite(translation(parent.world.apply(local_t)).then(rs))?;
    let world_rot = extract_rotation(rs).unwrap_or(parent.rotation.then(rot));
    let hidden = model.properties.num("Visibility", 1.)? <= 0.
        || (parent.hidden && model.properties.num("Visibility Inheritance", 1.)? > 0.);
    let state = ModelState {
        world,
        rotation: world_rot,
        local_scale: s,
        hidden,
    };
    active.remove(id);
    cache.insert(id.clone(), state);
    Ok(state)
}
fn extract_rotation(m: Transform) -> Option<Transform> {
    let x = Vec3::new(m.0[0][0], m.0[0][1], m.0[0][2]);
    let y = Vec3::new(m.0[1][0], m.0[1][1], m.0[1][2]);
    if x.length() < 1e-30 {
        return None;
    }
    let x = x.normalized();
    let y = y - x * x.dot(y);
    if y.length() < 1e-30 {
        return None;
    }
    let y = y.normalized();
    let z = x.cross(y);
    Some(Transform([
        [x.x, x.y, x.z, 0.],
        [y.x, y.y, y.z, 0.],
        [z.x, z.y, z.z, 0.],
        [0., 0., 0., 1.],
    ]))
}

struct Connection {
    kind: String,
    child: Id,
    parent: Id,
    property: Option<String>,
}
fn connections(root: &Node) -> Result<Vec<Connection>> {
    let mut connections = Vec::new();
    if let Some(block) = root.child("Connections") {
        for node in &block.children {
            if !matches!(node.name.as_str(), "C" | "Connect") {
                continue;
            }
            if node.values.len() < 3 {
                return Err("FBX connection requires a type and two object IDs".into());
            }
            let kind = node.values[0]
                .get_string()
                .ok_or("FBX connection type must be a string")?
                .to_owned();
            connections.push(Connection {
                kind,
                child: id(&node.values[1])?,
                parent: id(&node.values[2])?,
                property: node
                    .values
                    .get(3)
                    .and_then(Value::get_string)
                    .map(str::to_owned),
            });
        }
    }
    Ok(connections)
}
fn subtype(node: &Node) -> &str {
    node.values.last().and_then(Value::get_string).unwrap_or("")
}

/// FBX stores cluster `Transform` in mesh-node-to-bone bind space;
/// `TransformLink` stores the bone's world bind matrix. The SDK reconstructs
/// the mesh bind matrix from their product. Use the authored relative matrix
/// together with the evaluated bone world matrix, including mesh geometry
/// offsets exactly once. See ufbx's read/update_initial_clusters routines.
struct Deformation<'a> {
    nodes: HashMap<Id, &'a Node>,
    children: HashMap<Id, Vec<Id>>,
    animated: HashMap<Id, f64>,
    blender: bool,
}
impl<'a> Deformation<'a> {
    fn new(
        root: &Node,
        objects: &'a Node,
        links: &[Connection],
        meshes: &HashMap<Id, &'a Node>,
        shapes: &HashMap<Id, &'a Node>,
        models: &HashMap<Id, Model<'_>>,
    ) -> Result<Self> {
        let mut nodes = shapes.clone();
        for node in objects.children("Deformer") {
            let key = id(node.values.first().ok_or("FBX deformer has no ID")?)?;
            if !matches!(
                subtype(node),
                "Skin" | "Cluster" | "BlendShape" | "BlendShapeChannel"
            ) {
                return Err(format!("unsupported FBX deformer type {}", subtype(node)).into());
            }
            if subtype(node) == "Skin" {
                let method = field_text(node, "SkinningType", "Linear")?;
                if !matches!(method, "Linear" | "Rigid") {
                    return Err(format!("unsupported FBX skinning method {method}").into());
                }
            }
            nodes.insert(key, node);
        }
        let mut children: HashMap<Id, Vec<Id>> = HashMap::new();
        let mut parents = HashSet::new();
        let mut bone_parents = HashSet::new();
        for link in links.iter().filter(|c| c.kind == "OO") {
            let child = nodes.get(&link.child);
            let parent = nodes.get(&link.parent);
            if let Some(child) = child {
                let valid = match subtype(child) {
                    "Skin" | "BlendShape" => {
                        meshes.contains_key(&link.parent)
                            || models
                                .get(&link.parent)
                                .is_some_and(|m| m.node.child("Vertices").is_some())
                    }
                    "Cluster" => parent.is_some_and(|n| subtype(n) == "Skin"),
                    "BlendShapeChannel" => parent.is_some_and(|n| subtype(n) == "BlendShape"),
                    "Shape" => parent.is_some_and(|n| subtype(n) == "BlendShapeChannel"),
                    _ => false,
                };
                if !valid || !parents.insert(link.child.clone()) {
                    return Err(
                        "FBX deformer/shape has a missing, invalid or duplicate parent".into(),
                    );
                }
                children
                    .entry(link.parent.clone())
                    .or_default()
                    .push(link.child.clone());
            } else if parent.is_some_and(|n| subtype(n) == "Cluster") {
                if !models.contains_key(&link.child) || !bone_parents.insert(link.parent.clone()) {
                    return Err("FBX skin cluster requires exactly one existing bone model".into());
                }
                children
                    .entry(link.parent.clone())
                    .or_default()
                    .push(link.child.clone());
            }
        }
        if nodes.keys().any(|key| !parents.contains(key)) {
            return Err("FBX deformer or target shape is unattached".into());
        }
        for (key, node) in &nodes {
            if subtype(node) == "Cluster" && !bone_parents.contains(key) {
                return Err("FBX skin cluster has no bone model".into());
            }
        }
        // Scalar morph animation follows the same first-key static policy as
        // model transforms. At the earliest key time, all other channels are
        // clamped to their first key, so no tangent interpolation is needed.
        let animated = first_morph_animation(objects, links, &nodes)?;
        let blender = root
            .child("FBXHeaderExtension")
            .and_then(|n| n.child("Creator"))
            .or_else(|| root.child("Creator"))
            .and_then(Node::text)
            .is_some_and(|creator| creator.to_ascii_lowercase().contains("blender"));
        Ok(Self {
            nodes,
            children,
            animated,
            blender,
        })
    }
    fn children_of(&self, key: &Id) -> &[Id] {
        self.children
            .get(key)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
    #[allow(clippy::too_many_arguments)]
    fn positions(
        &self,
        key: &Id,
        mesh: &Node,
        mesh_world: Transform,
        geometric: Transform,
        models: &HashMap<Id, Model<'_>>,
        states: &mut HashMap<Id, ModelState>,
        work: &mut usize,
    ) -> Result<Vec<Vec3>> {
        let mut positions = mesh_positions(mesh)?;
        if !self.children_of(key).is_empty() {
            deformation_work(work, positions.len())?;
        }
        for blend in self
            .children_of(key)
            .iter()
            .filter(|id| subtype(self.nodes[id]) == "BlendShape")
        {
            for channel in self.children_of(blend) {
                self.blend(channel, &mut positions, work)?;
            }
        }
        let skins: Vec<_> = self
            .children_of(key)
            .iter()
            .filter(|id| subtype(self.nodes[id]) == "Skin")
            .collect();
        if skins.len() > 1 {
            return Err(
                "multiple FBX skins on one mesh require a separate deformer-stack evaluator".into(),
            );
        }
        if let Some(skin) = skins.first() {
            self.skin(
                skin,
                &mut positions,
                mesh_world,
                geometric,
                models,
                states,
                work,
            )?;
        }
        if positions.iter().any(|p| !p.is_finite()) {
            return Err("FBX deformation produced non-finite coordinates".into());
        }
        Ok(positions)
    }
    fn blend(&self, key: &Id, positions: &mut [Vec3], work: &mut usize) -> Result<()> {
        let node = self.nodes[key];
        let mut properties = Properties::default();
        properties.add(node);
        let default = node
            .child("DeformPercent")
            .map(|n| scalar(n.values.first().ok_or("missing FBX DeformPercent value")?))
            .transpose()?
            .unwrap_or(0.);
        let percent = self
            .animated
            .get(key)
            .copied()
            .unwrap_or(properties.num("DeformPercent", default)?);
        let targets = self.children_of(key);
        if targets.is_empty() {
            return Err("FBX blend shape channel has no target shape".into());
        }
        let full = node
            .child("FullWeights")
            .map(array)
            .transpose()?
            .unwrap_or_else(|| vec![100.; targets.len()]);
        let mut offsets = Vec::with_capacity(targets.len());
        for target in targets {
            let shape = self.nodes[target];
            let indices = int_array(
                shape
                    .child("Indexes")
                    .ok_or("FBX target shape has no Indexes")?,
            )?;
            let values = mesh_positions(shape)?;
            if indices.len() != values.len() {
                return Err("FBX target shape indices and offset counts disagree".into());
            }
            deformation_work(work, indices.len())?;
            let mut seen = HashSet::with_capacity(indices.len());
            let mut sparse = Vec::with_capacity(indices.len());
            for (index, offset) in indices.into_iter().zip(values) {
                let index = usize::try_from(index)?;
                if index >= positions.len() || !seen.insert(index) {
                    return Err(
                        "FBX target shape has an out-of-range or duplicate vertex index".into(),
                    );
                }
                sparse.push((index, offset));
            }
            offsets.push(sparse);
        }
        // Old Blender exporters write painted per-vertex full weights for a
        // single target. Identify that exporter convention explicitly.
        if self.blender && targets.len() == 1 && full.len() == offsets[0].len() && full.len() != 1 {
            for ((index, offset), weight) in offsets[0].iter().zip(&full) {
                if !weight.is_finite() || !(0. ..=100.).contains(weight) {
                    return Err("invalid painted FBX blend shape weight".into());
                }
                positions[*index] = positions[*index] + *offset * (percent * weight * 0.0001);
            }
            return Ok(());
        }
        if full.len() != targets.len() || full.iter().any(|v| !v.is_finite() || *v == 0.) {
            return Err("FBX blend shape target/full-weight counts or values are invalid".into());
        }
        let mut order: Vec<_> = (0..full.len()).collect();
        order.sort_by(|&a, &b| full[a].total_cmp(&full[b]));
        if order.windows(2).any(|p| full[p[0]] == full[p[1]]) {
            return Err("FBX blend shape full weights must be distinct".into());
        }
        if percent == 0. {
            return Ok(());
        }
        let mut knots: Vec<(f64, Option<usize>)> =
            order.into_iter().map(|i| (full[i], Some(i))).collect();
        knots.push((0., None));
        knots.sort_by(|a, b| a.0.total_cmp(&b.0));
        let bracket = knots
            .windows(2)
            .find(|p| percent <= p[1].0)
            .unwrap_or(&knots[knots.len() - 2..]);
        let t = (percent - bracket[0].0) / (bracket[1].0 - bracket[0].0);
        for (target, weight) in [(bracket[0].1, 1. - t), (bracket[1].1, t)] {
            if let Some(target) = target {
                for &(index, offset) in &offsets[target] {
                    positions[index] = positions[index] + offset * weight;
                }
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn skin(
        &self,
        key: &Id,
        positions: &mut [Vec3],
        mesh_world: Transform,
        geometric: Transform,
        models: &HashMap<Id, Model<'_>>,
        states: &mut HashMap<Id, ModelState>,
        work: &mut usize,
    ) -> Result<()> {
        let clusters = self.children_of(key);
        if clusters.is_empty() {
            return Err("FBX skin has no clusters".into());
        }
        let inverse_mesh = inverse_affine(mesh_world.then(geometric))?;
        let mut sums = vec![Vec3::default(); positions.len()];
        let mut weights = vec![0.; positions.len()];
        let mut common_mode = None;
        for cluster in clusters {
            let node = self.nodes[cluster];
            let mode = field_text(node, "Mode", "Normalize")?;
            if !matches!(mode, "Normalize" | "TotalOne") {
                return Err(format!("unsupported FBX cluster link mode {mode}").into());
            }
            if common_mode
                .replace(mode)
                .is_some_and(|previous| previous != mode)
            {
                return Err("FBX clusters on one skin have inconsistent link modes".into());
            }
            if node.child("TransformAssociateModel").is_some() {
                // Many exporters always emit this field. The SDK uses it
                // only for additive clusters, already rejected above.
                cluster_matrix(node, "TransformAssociateModel")?;
            }
            let bind = cluster_matrix(node, "Transform")?;
            let link_bind = cluster_matrix(node, "TransformLink")?;
            inverse_affine(bind)?;
            inverse_affine(link_bind)?;
            let bone = self
                .children_of(cluster)
                .first()
                .ok_or("FBX cluster has no bone")?;
            let current = model_state(bone, models, states, &mut HashSet::new())?.world;
            let transform = finite(inverse_mesh.then(current).then(bind).then(geometric))?;
            let indices = node.child("Indexes").map(int_array).transpose()?;
            let values = node.child("Weights").map(array).transpose()?;
            let (indices, values) = match (indices, values) {
                (Some(i), Some(w)) if i.len() == w.len() => (i, w),
                (None, None) => (Vec::new(), Vec::new()),
                _ => return Err("FBX skin cluster index and weight counts disagree".into()),
            };
            deformation_work(work, indices.len())?;
            let mut seen = HashSet::with_capacity(indices.len());
            for (index, weight) in indices.into_iter().zip(values) {
                let index = usize::try_from(index)?;
                if index >= positions.len()
                    || !seen.insert(index)
                    || !weight.is_finite()
                    || weight < 0.
                {
                    return Err("FBX skin cluster has invalid indices or weights".into());
                }
                sums[index] = sums[index] + transform.apply(positions[index]) * weight;
                weights[index] += weight;
            }
        }
        for ((position, sum), weight) in positions.iter_mut().zip(sums).zip(weights) {
            if !weight.is_finite() || !sum.is_finite() {
                return Err("FBX skin weight accumulation overflow".into());
            }
            if weight == 0. {
                continue;
            }
            *position = if common_mode == Some("Normalize") {
                sum / weight
            } else {
                sum + *position * (1. - weight)
            };
        }
        Ok(())
    }
}
fn deformation_work(work: &mut usize, extra: usize) -> Result<()> {
    *work = work
        .checked_add(extra)
        .ok_or("FBX deformation work overflow")?;
    if *work > MAX_ELEMENTS {
        return Err(
            "FBX deformation exceeds five million control points/influences/offsets".into(),
        );
    }
    Ok(())
}
fn field_text<'a>(node: &'a Node, name: &str, default: &'a str) -> Result<&'a str> {
    match node.child(name) {
        None => Ok(default),
        Some(field) => match field.values.as_slice() {
            [Value::String(value)] => Ok(value),
            _ => Err(format!("FBX {name} field requires one string").into()),
        },
    }
}
fn cluster_matrix(node: &Node, name: &str) -> Result<Transform> {
    let values = array(
        node.child(name)
            .ok_or_else(|| format!("FBX cluster has no {name} matrix"))?,
    )?;
    if values.len() != 16 || values.iter().any(|v| !v.is_finite()) {
        return Err("FBX cluster matrices need sixteen finite values".into());
    }
    let matrix = Transform(std::array::from_fn(|c| {
        std::array::from_fn(|r| values[c * 4 + r])
    }));
    if matrix.0[0][3] != 0. || matrix.0[1][3] != 0. || matrix.0[2][3] != 0. || matrix.0[3][3] != 1.
    {
        return Err("FBX cluster matrix must be affine".into());
    }
    Ok(matrix)
}
fn inverse_affine(matrix: Transform) -> Result<Transform> {
    finite(matrix)?;
    let norm = matrix.0[..3]
        .iter()
        .flat_map(|c| &c[..3])
        .fold(0f64, |a, b| a.max(b.abs()));
    if norm == 0. {
        return Err("singular FBX skin transform".into());
    }
    let columns: [Vec3; 3] = std::array::from_fn(|c| {
        Vec3::new(
            matrix.0[c][0] / norm,
            matrix.0[c][1] / norm,
            matrix.0[c][2] / norm,
        )
    });
    let determinant = columns[0].dot(columns[1].cross(columns[2]));
    if !determinant.is_finite() || determinant.abs() < 1e-14 {
        return Err("singular or ill-conditioned FBX skin transform".into());
    }
    let rows = [
        columns[1].cross(columns[2]),
        columns[2].cross(columns[0]),
        columns[0].cross(columns[1]),
    ];
    let mut inverse = Transform::IDENTITY;
    for (row, v) in rows.into_iter().enumerate() {
        for (column, value) in [v.x, v.y, v.z].into_iter().enumerate() {
            inverse.0[column][row] = value / determinant / norm;
        }
    }
    let t = inverse.apply(Vec3::new(-matrix.0[3][0], -matrix.0[3][1], -matrix.0[3][2]));
    inverse.0[3] = [t.x, t.y, t.z, 1.];
    finite(inverse)
}
fn first_morph_animation(
    objects: &Node,
    links: &[Connection],
    deformers: &HashMap<Id, &Node>,
) -> Result<HashMap<Id, f64>> {
    let curves: HashMap<_, _> = objects
        .children("AnimationCurve")
        .map(|n| Ok((id(n.values.first().ok_or("FBX curve has no ID")?)?, n)))
        .collect::<Result<_>>()?;
    let nodes: HashMap<_, _> = objects
        .children("AnimationCurveNode")
        .map(|n| Ok((id(n.values.first().ok_or("FBX curve node has no ID")?)?, n)))
        .collect::<Result<_>>()?;
    let mut incoming: HashMap<_, Vec<_>> = HashMap::new();
    for connection in links
        .iter()
        .filter(|c| c.kind == "OP" && curves.contains_key(&c.child))
    {
        incoming
            .entry(connection.parent.clone())
            .or_default()
            .push(connection);
    }
    let mut node_values: HashMap<Id, Option<f64>> = HashMap::new();
    let mut curve_values: HashMap<Id, Option<f64>> = HashMap::new();
    let mut animated = HashMap::new();
    for link in links.iter().filter(|c| {
        c.kind == "OP" && deformers.contains_key(&c.parent) && nodes.contains_key(&c.child)
    }) {
        if subtype(deformers[&link.parent]) != "BlendShapeChannel"
            || link.property.as_deref() != Some("DeformPercent")
        {
            return Err("unsupported animated FBX deformer property".into());
        }
        let first = if let Some(value) = node_values.get(&link.child) {
            *value
        } else {
            let mut first = None;
            for axis in incoming.get(&link.child).into_iter().flatten() {
                if !matches!(axis.property.as_deref(), Some("d|DeformPercent" | "d|X")) {
                    return Err("unknown FBX morph animation axis".into());
                }
                let value = if let Some(value) = curve_values.get(&axis.child) {
                    *value
                } else {
                    let curve = curves[&axis.child];
                    let times = int_array(
                        curve
                            .child("KeyTime")
                            .ok_or("FBX morph curve has no key times")?,
                    )?;
                    let values = array(
                        curve
                            .child("KeyValueFloat")
                            .ok_or("FBX morph curve has no key values")?,
                    )?;
                    if times.len() != values.len()
                        || times.windows(2).any(|p| p[0] > p[1])
                        || values.iter().any(|v| !v.is_finite())
                    {
                        return Err("invalid FBX morph animation key arrays".into());
                    }
                    let value = values.first().copied();
                    curve_values.insert(axis.child.clone(), value);
                    value
                };
                if let Some(value) = value
                    && first.replace(value).is_some()
                {
                    return Err("FBX morph channel has multiple animation curves".into());
                }
            }
            node_values.insert(link.child.clone(), first);
            first
        };
        if let Some(value) = first
            && animated.insert(link.parent.clone(), value).is_some()
        {
            return Err("FBX layered morph animation requires a separate pose evaluator".into());
        }
    }
    Ok(animated)
}

fn build_scene(root: &Node) -> Result<Scene> {
    let objects = root
        .child("Objects")
        .ok_or("FBX file has no Objects section")?;
    let links = connections(root)?;
    let mut models = HashMap::new();
    let mut geometries = HashMap::new();
    let mut shapes = HashMap::new();
    let mut materials = HashMap::new();
    let mut order = Vec::new();
    let mut object_ids = HashSet::new();
    for node in &objects.children {
        if let Some(value) = node.values.first() {
            let key = id(value)?;
            if !object_ids.insert(key) {
                return Err("duplicate FBX object ID".into());
            }
        }
        if node.name == "Constraint" {
            return Err("FBX constraints require a separate pose evaluator".into());
        }
        if !matches!(node.name.as_str(), "Model" | "Geometry" | "Material") {
            continue;
        }
        let key = id(node.values.first().ok_or("FBX object has no ID")?)?;
        match node.name.as_str() {
            "Model" => {
                if !matches!(
                    subtype(node),
                    "Mesh" | "Null" | "Root" | "LimbNode" | "Camera" | "Light" | ""
                ) {
                    return Err(format!("unsupported FBX model type {}", subtype(node)).into());
                }
                let mut properties = defaults(root, "Model");
                properties.add(node);
                let model = Model {
                    node,
                    properties,
                    parent: None,
                    geometries: Vec::new(),
                    materials: Vec::new(),
                    animated: HashMap::new(),
                };
                if models.insert(key.clone(), model).is_some() {
                    return Err("duplicate FBX model ID".into());
                }
                order.push(key);
            }
            "Geometry" => match subtype(node) {
                "Mesh" => {
                    geometries.insert(key, node);
                }
                "Shape" => {
                    shapes.insert(key, node);
                }
                other => return Err(format!("unsupported FBX geometry type {other}").into()),
            },
            "Material" => {
                let mut p = defaults(root, "Material");
                p.add(node);
                let diffuse = p.vec("DiffuseColor", p.vec("Diffuse", Vec3::new(0.8, 0.8, 0.8))?)?
                    * p.num("DiffuseFactor", 1.)?;
                materials.insert(key, diffuse);
            }
            _ => unreachable!(),
        }
    }
    let deformation = Deformation::new(root, objects, &links, &geometries, &shapes, &models)?;
    let mut attached = HashSet::new();
    for link in links.iter().filter(|c| c.kind == "OO") {
        if models.contains_key(&link.child) {
            if models.contains_key(&link.parent) {
                let model = models.get_mut(&link.child).unwrap();
                if model.parent.replace(link.parent.clone()).is_some() {
                    return Err("FBX model has multiple parents".into());
                }
            } else if !matches!(&link.parent, Id::Number(0))
                && !matches!(&link.parent,Id::Name(v) if v=="Model::Scene"||v=="Scene")
                && !object_ids.contains(&link.parent)
            {
                return Err("FBX model connects to a missing parent".into());
            }
        } else if let Some(model) = models.get_mut(&link.parent) {
            if geometries.contains_key(&link.child) {
                model.geometries.push(link.child.clone());
                attached.insert(link.child.clone());
            }
            if materials.contains_key(&link.child) {
                model.materials.push(link.child.clone());
            }
        } else if geometries.contains_key(&link.child) && !matches!(&link.parent, Id::Number(0)) {
            return Err("FBX geometry connects to a missing or unsupported model".into());
        }
    }
    first_animation(objects, &links, &mut models)?;
    let basis = axis_basis(root)?;
    let mut states = HashMap::new();
    let mut scene = Scene::default();
    let mut work = 0;
    for key in order {
        let state = model_state(&key, &models, &mut states, &mut HashSet::new())?;
        if state.hidden {
            continue;
        }
        let model = &models[&key];
        let (_, _, _, geometric) = local_transform(model)?;
        let world = basis.then(state.world).then(geometric);
        let colors = model
            .materials
            .iter()
            .map(|id| materials[id])
            .collect::<Vec<_>>();
        if model.node.child("Vertices").is_some() {
            let positions = deformation.positions(
                &key,
                model.node,
                state.world,
                geometric,
                &models,
                &mut states,
                &mut work,
            )?;
            append_mesh(model.node, positions, world, &colors, &mut scene)?;
        }
        for geometry in &model.geometries {
            let node = geometries[geometry];
            let positions = deformation.positions(
                geometry,
                node,
                state.world,
                geometric,
                &models,
                &mut states,
                &mut work,
            )?;
            append_mesh(node, positions, world, &colors, &mut scene)?;
        }
    }
    for (key, node) in geometries {
        if !attached.contains(&key) {
            let positions = deformation.positions(
                &key,
                node,
                Transform::IDENTITY,
                Transform::IDENTITY,
                &models,
                &mut states,
                &mut work,
            )?;
            append_mesh(node, positions, basis, &[], &mut scene)?;
        }
    }
    scene.validate()?;
    Ok(scene)
}

fn axis_basis(root: &Node) -> Result<Transform> {
    let mut p = Properties::default();
    if let Some(settings) = root.child("GlobalSettings") {
        p.add(settings);
    }
    let up = p.num("UpAxis", 1.)?;
    let front = p.num("FrontAxis", 2.)?;
    let right = p.num("CoordAxis", 0.)?;
    if [up, front, right]
        .iter()
        .any(|&v| v != 0. && v != 1. && v != 2.)
        || up == front
        || up == right
        || front == right
    {
        return Err("FBX coordinate axes must be distinct X/Y/Z axes".into());
    }
    let signs = [
        p.num("CoordAxisSign", 1.)?,
        p.num("FrontAxisSign", -1.)?,
        p.num("UpAxisSign", 1.)?,
    ];
    if signs.iter().any(|&v| v != -1. && v != 1.) {
        return Err("FBX coordinate axis signs must be -1 or 1".into());
    }
    let unit = p.num("UnitScaleFactor", 1.)? / 100.;
    if unit <= 0. {
        return Err("FBX unit scale must be positive".into());
    }
    let mut matrix = [[0.; 4]; 4];
    matrix[right as usize][0] = signs[0] * unit;
    matrix[front as usize][1] = signs[1] * unit;
    matrix[up as usize][2] = signs[2] * unit;
    matrix[3][3] = 1.;
    Ok(Transform(matrix))
}

fn first_animation(
    objects: &Node,
    links: &[Connection],
    models: &mut HashMap<Id, Model<'_>>,
) -> Result<()> {
    let mut curves = HashMap::new();
    let mut curve_nodes = HashMap::new();
    for node in &objects.children {
        if node.name == "AnimationCurve" {
            let key = id(node.values.first().ok_or("FBX animation curve has no ID")?)?;
            curves.insert(key, node);
        } else if node.name == "AnimationCurveNode" {
            let key = id(node
                .values
                .first()
                .ok_or("FBX animation curve node has no ID")?)?;
            curve_nodes.insert(key, node);
        }
    }
    for link in links.iter().filter(|c| c.kind == "OP") {
        if !models.contains_key(&link.parent) || !curve_nodes.contains_key(&link.child) {
            continue;
        }
        let Some(property) = link.property.as_deref() else {
            continue;
        };
        if !matches!(property, "Lcl Translation" | "Lcl Rotation" | "Lcl Scaling") {
            if !links
                .iter()
                .any(|c| c.kind == "OP" && c.parent == link.child && curves.contains_key(&c.child))
            {
                continue;
            }
            return Err(format!("unsupported animated FBX property {property}").into());
        }
        let model = models.get_mut(&link.parent).unwrap();
        if model.animated.contains_key(property) {
            return Err("FBX layered animation needs a separate pose evaluator".into());
        }
        let mut vector = model.properties.vec(
            property,
            if property == "Lcl Scaling" {
                Vec3::new(1., 1., 1.)
            } else {
                Vec3::default()
            },
        )?;
        for axis in links
            .iter()
            .filter(|c| c.kind == "OP" && c.parent == link.child)
        {
            let Some(curve) = curves.get(&axis.child) else {
                continue;
            };
            let times = int_array(
                curve
                    .child("KeyTime")
                    .ok_or("FBX animation curve has no key times")?,
            )?;
            let values = array(
                curve
                    .child("KeyValueFloat")
                    .ok_or("FBX animation curve has no key values")?,
            )?;
            if times.len() != values.len() || times.windows(2).any(|p| p[0] > p[1]) {
                return Err("invalid FBX animation key arrays".into());
            }
            if let Some(&v) = values.first() {
                if !v.is_finite() {
                    return Err("FBX animation value must be finite".into());
                }
                match axis.property.as_deref() {
                    Some("d|X") => vector.x = v,
                    Some("d|Y") => vector.y = v,
                    Some("d|Z") => vector.z = v,
                    _ => return Err("unknown FBX animation axis".into()),
                }
            }
        }
        model.animated.insert(property.to_owned(), vector);
    }
    Ok(())
}

fn srgb(v: f64) -> u8 {
    let v = v.clamp(0., 1.);
    ((if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    }) * 255.)
        .round() as u8
}
fn rgb(v: Vec3) -> [u8; 3] {
    [srgb(v.x), srgb(v.y), srgb(v.z)]
}
struct ColorLayer {
    values: Vec<Vec3>,
    indices: Vec<i64>,
    mapping: String,
    reference: String,
}
impl ColorLayer {
    fn new(node: &Node) -> Result<Option<Self>> {
        let Some(layer) = node.child("LayerElementColor") else {
            return Ok(None);
        };
        let colors = array(
            layer
                .child("Colors")
                .ok_or("FBX color layer has no Colors array")?,
        )?;
        if !colors.len().is_multiple_of(4) || colors.iter().any(|v| !v.is_finite()) {
            return Err("invalid FBX RGBA color array".into());
        }
        let values = colors
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| Vec3::new(p[0], p[1], p[2]))
            .collect();
        let indices = layer
            .child("ColorIndex")
            .map(int_array)
            .transpose()?
            .unwrap_or_default();
        let mapping = layer
            .child("MappingInformationType")
            .and_then(Node::text)
            .unwrap_or("ByPolygonVertex")
            .to_owned();
        let mut reference = layer
            .child("ReferenceInformationType")
            .and_then(Node::text)
            .unwrap_or("Direct")
            .to_owned();
        // Exporters can author IndexToDirect while omitting ColorIndex.
        // Assimp's importer uses the direct mapping for that combination.
        if reference == "IndexToDirect" && layer.child("ColorIndex").is_none() {
            reference = "Direct".to_owned();
        }
        if !matches!(
            mapping.as_str(),
            "ByPolygonVertex"
                | "ByVertice"
                | "ByVertex"
                | "ByControlPoint"
                | "ByPolygon"
                | "AllSame"
        ) || !matches!(reference.as_str(), "Direct" | "IndexToDirect" | "Index")
        {
            return Err("unsupported FBX vertex color mapping/reference".into());
        }
        Ok(Some(Self {
            values,
            indices,
            mapping,
            reference,
        }))
    }
    fn color(&self, vertices: &[usize], face: usize, corner: usize) -> Result<Vec3> {
        let mut sum = Vec3::default();
        for (offset, &vertex) in vertices.iter().enumerate() {
            let mut index = match self.mapping.as_str() {
                "AllSame" => 0,
                "ByPolygon" => face,
                "ByPolygonVertex" => corner + offset,
                _ => vertex,
            };
            if self.reference != "Direct" {
                index = usize::try_from(
                    *self
                        .indices
                        .get(index)
                        .ok_or("FBX color index array is too short")?,
                )?;
            }
            sum = sum
                + *self
                    .values
                    .get(index)
                    .ok_or("FBX color array is too short")?;
        }
        Ok(sum / (vertices.len() as f64))
    }
}
struct FaceLayer<T> {
    values: Vec<T>,
    all_same: bool,
}
impl<T: Copy> FaceLayer<T> {
    fn get(&self, face: usize) -> Result<T> {
        self.values
            .get(if self.all_same { 0 } else { face })
            .copied()
            .ok_or_else(|| "FBX polygon layer has too few values".into())
    }
}
fn face_layer<'a>(
    node: &'a Node,
    kind: &str,
    array_name: &str,
) -> Result<Option<(&'a Node, bool)>> {
    let Some(layer) = node.child(kind) else {
        return Ok(None);
    };
    let all_same = match layer
        .child("MappingInformationType")
        .and_then(Node::text)
        .unwrap_or("AllSame")
    {
        "AllSame" => true,
        "ByPolygon" => false,
        _ => return Err(format!("unsupported FBX {kind} polygon mapping").into()),
    };
    Ok(Some((
        layer
            .child(array_name)
            .ok_or_else(|| format!("FBX {kind} has no {array_name} array"))?,
        all_same,
    )))
}
fn mesh_positions(node: &Node) -> Result<Vec<Vec3>> {
    let vertices = array(
        node.child("Vertices")
            .ok_or("FBX mesh has no Vertices array")?,
    )?;
    if !vertices.len().is_multiple_of(3) || vertices.iter().any(|v| !v.is_finite()) {
        return Err("FBX mesh coordinates must be finite triples".into());
    }
    Ok(vertices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|v| Vec3::new(v[0], v[1], v[2]))
        .collect())
}
fn append_mesh(
    node: &Node,
    positions: Vec<Vec3>,
    world: Transform,
    materials: &[Vec3],
    scene: &mut Scene,
) -> Result<()> {
    let indices = node
        .child("PolygonVertexIndex")
        .map(int_array)
        .transpose()?
        .unwrap_or_default();
    let colors = ColorLayer::new(node)?;
    let material_layer = face_layer(node, "LayerElementMaterial", "Materials")?
        .map(|(n, all_same)| {
            Ok::<_, crate::Error>(FaceLayer {
                values: int_array(n)?,
                all_same,
            })
        })
        .transpose()?;
    let holes = face_layer(node, "LayerElementHole", "Hole")?
        .map(|(n, all_same)| {
            Ok::<_, crate::Error>(FaceLayer {
                values: array(n)?,
                all_same,
            })
        })
        .transpose()?;
    if indices.is_empty() {
        let color = materials.first().copied().map(rgb);
        scene
            .points
            .extend(positions.into_iter().map(|position| Point {
                position: world.apply(position),
                color,
            }));
        return geometry_limit(scene);
    }
    let mut polygon = Vec::new();
    let mut ids = Vec::new();
    let mut face = 0;
    let mut corner = 0;
    for value in indices {
        let index = usize::try_from(if value < 0 { !value } else { value })?;
        polygon.push(
            *positions
                .get(index)
                .ok_or("FBX polygon vertex index is out of range")?,
        );
        ids.push(index);
        if polygon.len() > 65_536 {
            return Err("FBX polygon exceeds 65,536 vertices".into());
        }
        if value < 0 {
            let material = if let Some(layer) = &material_layer {
                let i = layer.get(face)?;
                if i == -1 || materials.is_empty() {
                    None
                } else {
                    Some(
                        *materials
                            .get(usize::try_from(i)?)
                            .ok_or("FBX material index is out of range")?,
                    )
                }
            } else {
                materials.first().copied()
            };
            let color = if let Some(layer) = &colors {
                let c = layer.color(&ids, face, corner)?;
                Some(rgb(if let Some(m) = material {
                    Vec3::new(c.x * m.x, c.y * m.y, c.z * m.z)
                } else {
                    c
                }))
            } else {
                material.map(rgb)
            };
            let hole = holes
                .as_ref()
                .map(|layer| layer.get(face))
                .transpose()?
                .unwrap_or(0.);
            if hole != 0. && hole != 1. {
                return Err("FBX polygon hole flags must be zero or one".into());
            }
            if hole == 0. {
                match polygon.as_slice() {
                    [position] => scene.points.push(Point {
                        position: world.apply(*position),
                        color,
                    }),
                    [a, b] => scene.lines.push(Line {
                        vertices: [world.apply(*a), world.apply(*b)],
                        color,
                    }),
                    _ => {
                        for vertices in triangulate_polygon(&polygon)? {
                            scene.triangles.push(Triangle {
                                vertices: vertices.map(|v| world.apply(v)),
                                color,
                            });
                        }
                    }
                }
            }
            geometry_limit(scene)?;
            corner += polygon.len();
            polygon.clear();
            ids.clear();
            face += 1;
        }
    }
    if !polygon.is_empty() {
        return Err("unterminated FBX polygon index sequence".into());
    }
    Ok(())
}
fn geometry_limit(scene: &Scene) -> Result<()> {
    if scene
        .triangles
        .len()
        .saturating_add(scene.points.len())
        .saturating_add(scene.lines.len())
        > MAX_ELEMENTS
    {
        return Err("FBX scene exceeds five million geometry elements".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::Write,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new(bytes: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!(
                "catwalk-fbx-{}-{}.fbx",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::write(&path, bytes).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    const GLOBAL: &str = r#"GlobalSettings: {
        Properties70: {
            P: "UpAxis", "int", "Integer", "", 2
            P: "FrontAxis", "int", "Integer", "", 1
            P: "CoordAxis", "int", "Integer", "", 0
            P: "UpAxisSign", "int", "Integer", "", 1
            P: "FrontAxisSign", "int", "Integer", "", 1
            P: "CoordAxisSign", "int", "Integer", "", 1
            P: "UnitScaleFactor", "double", "Number", "", 100
        }
    }"#;
    const TRIANGLE: &str = r#"Geometry: 100, "Geometry::triangle", "Mesh" {
        Vertices: *9 { a: 0,0,0, 1,0,0, 0,1,0 }
        PolygonVertexIndex: *3 { a: 0,1,-3 }
    }"#;
    fn ascii(objects: &str, links: &str) -> String {
        format!("{GLOBAL}\nObjects: {{\n{objects}\n}}\nConnections: {{\n{links}\n}}")
    }
    fn parse(text: &str) -> Result<Scene> {
        let tree = Ascii::new(text)?.parse()?;
        build_scene(&tree)
    }
    fn close(actual: Vec3, expected: [f64; 3]) {
        let e = Vec3::new(expected[0], expected[1], expected[2]);
        assert!((actual - e).length() < 1e-9, "{actual:?} != {e:?}");
    }

    fn skin_fixture(mode: &str, bone_a: f64, bone_b: f64, weight_a: f64, weight_b: f64) -> String {
        ascii(
            &format!(
                r#"{TRIANGLE}
            Model: 1,"mesh","Mesh" {{ Properties70: {{
                P: "Lcl Translation","Vector3D","Vector","",10,0,0
                P: "GeometricTranslation","Vector3D","Vector","",3,0,0
            }} }}
            Model: 2,"boneA","LimbNode" {{ Properties70: {{
                P: "Lcl Translation","Vector3D","Vector","",{bone_a},0,0
            }} }}
            Model: 3,"boneB","LimbNode" {{ Properties70: {{
                P: "Lcl Translation","Vector3D","Vector","",{bone_b},0,0
            }} }}
            Deformer: 10,"skin","Skin" {{ SkinningType: "Linear" }}
            Deformer: 11,"clusterA","Cluster" {{
                Mode: "{mode}"
                Indexes: *2 {{ a: 0,1 }} Weights: *2 {{ a: {weight_a},{weight_a} }}
                Transform: *16 {{ a: 1,0,0,0,0,1,0,0,0,0,1,0,-5,0,0,1 }}
                TransformLink: *16 {{ a: 1,0,0,0,0,1,0,0,0,0,1,0,15,0,0,1 }}
            }}
            Deformer: 12,"clusterB","Cluster" {{
                Mode: "{mode}"
                Indexes: *2 {{ a: 0,1 }} Weights: *2 {{ a: {weight_b},{weight_b} }}
                Transform: *16 {{ a: 1,0,0,0,0,1,0,0,0,0,1,0,-10,0,0,1 }}
                TransformLink: *16 {{ a: 1,0,0,0,0,1,0,0,0,0,1,0,20,0,0,1 }}
            }}"#
            ),
            r#"C: "OO",100,1
                C: "OO",10,100 C: "OO",11,10 C: "OO",12,10
                C: "OO",2,11 C: "OO",3,12"#,
        )
    }

    #[test]
    fn skin_bind_current_geometry_offsets_and_actual_weights() {
        let bind = parse(&skin_fixture("Normalize", 15., 20., 0.25, 0.75)).unwrap();
        close(bind.triangles[0].vertices[0], [13., 0., 0.]);
        close(bind.triangles[0].vertices[1], [14., 0., 0.]);
        let moved = parse(&skin_fixture("Normalize", 17., 26., 0.5, 1.5)).unwrap();
        // Non-unit weights normalize to 1/4 and 3/4: two units and six
        // units of genuine bone motion produce five units of mesh motion.
        close(moved.triangles[0].vertices[0], [18., 0., 0.]);
        close(moved.triangles[0].vertices[1], [19., 0., 0.]);
        close(moved.triangles[0].vertices[2], [13., 1., 0.]);
        let partial = parse(&skin_fixture("TotalOne", 17., 26., 0.25, 0.25)).unwrap();
        close(partial.triangles[0].vertices[0], [15., 0., 0.]);
        close(partial.triangles[0].vertices[1], [16., 0., 0.]);
        let zero = parse(&skin_fixture("Normalize", 17., 26., 0., 0.)).unwrap();
        close(zero.triangles[0].vertices[0], [13., 0., 0.]);
        let image = crate::render::render(
            &moved,
            &crate::render::RenderOptions {
                width: 96,
                height: 64,
                transparent: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(image.pixels().filter(|p| p[3] > 0).count() > 50);
    }

    fn morph_fixture(percent: f64) -> String {
        ascii(
            &format!(
                r#"{TRIANGLE}
            Deformer: 10,"blend","BlendShape" {{}}
            Deformer: 11,"channel","BlendShapeChannel" {{
                DeformPercent: {percent} FullWeights: *2 {{ a: 50,100 }}
            }}
            Geometry: 20,"first","Shape" {{ Indexes: 0 Vertices: 0,0,2 }}
            Geometry: 21,"second","Shape" {{ Indexes: 0 Vertices: 0,0,6 }}"#
            ),
            r#"C: "OO",10,100 C: "OO",11,10 C: "OO",20,11 C: "OO",21,11"#,
        )
    }

    #[test]
    fn sparse_progressive_blend_shapes_interpolate_and_extrapolate() {
        for (percent, height) in [
            (0., 0.),
            (25., 1.),
            (50., 2.),
            (75., 4.),
            (100., 6.),
            (150., 10.),
            (-50., -2.),
        ] {
            let scene = parse(&morph_fixture(percent)).unwrap();
            close(scene.triangles[0].vertices[0], [0., 0., height]);
            close(scene.triangles[0].vertices[1], [1., 0., 0.]);
            close(scene.triangles[0].vertices[2], [0., 1., 0.]);
        }
        let negative = morph_fixture(-25.).replace("a: 50,100", "a: -50,100");
        close(
            parse(&negative).unwrap().triangles[0].vertices[0],
            [0., 0., 1.],
        );
    }

    #[test]
    fn blender_painted_target_weights_and_unused_associate_matrices() {
        let source = format!(
            r#"FBXHeaderExtension: {{ Creator: "Blender 2.79" }}
            {}"#,
            ascii(
                &format!(
                    r#"{TRIANGLE}
            Deformer: 10,"blend","BlendShape" {{}}
            Deformer: 11,"channel","BlendShapeChannel" {{ DeformPercent: 50 FullWeights: 100,50 }}
            Geometry: 20,"target","Shape" {{ Indexes: 0,1 Vertices: 0,0,2, 0,0,4 }}"#
                ),
                r#"C: "OO",10,100 C: "OO",11,10 C: "OO",20,11"#
            )
        );
        let scene = parse(&source).unwrap();
        close(scene.triangles[0].vertices[0], [0., 0., 1.]);
        close(scene.triangles[0].vertices[1], [1., 0., 1.]);
        assert!(parse(&source.replace("Blender 2.79", "Other exporter")).is_err());
        let source = skin_fixture("Normalize", 17., 26., 0.25, 0.75).replace(
            "Mode: \"Normalize\"",
            r#"Mode: "Normalize"
                TransformAssociateModel: *16 { a: 1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1 }"#,
        );
        close(
            parse(&source).unwrap().triangles[0].vertices[0],
            [18., 0., 0.],
        );
    }

    #[test]
    fn animated_morphs_and_parallel_channels_run_before_skinning() {
        let mut source = skin_fixture("Normalize", 17., 26., 0.25, 0.75);
        let insert = source.find("\n}\nConnections").unwrap();
        source.insert_str(
            insert,
            r#"
            Deformer: 30,"blend","BlendShape" {}
            Deformer: 31,"lift","BlendShapeChannel" { DeformPercent: 0 FullWeights: 100 }
            Deformer: 32,"side","BlendShapeChannel" { DeformPercent: 50 FullWeights: 100 }
            Geometry: 40,"liftShape","Shape" { Indexes: 0 Vertices: 0,0,4 }
            Geometry: 41,"sideShape","Shape" { Indexes: 0 Vertices: 2,0,0 }
            AnimationCurveNode: 50,"morphCurveNode","" {}
            AnimationCurve: 51,"morphCurve","" { KeyTime: 0,100 KeyValueFloat: 25,100 }
        "#,
        );
        let insert = source.rfind('}').unwrap();
        source.insert_str(
            insert,
            r#"
            C: "OO",30,100 C: "OO",31,30 C: "OO",32,30 C: "OO",40,31 C: "OO",41,32
            C: "OP",50,31,"DeformPercent" C: "OP",51,50,"d|DeformPercent"
        "#,
        );
        let scene = parse(&source).unwrap();
        close(scene.triangles[0].vertices[0], [19., 0., 1.]);
        close(scene.triangles[0].vertices[1], [19., 0., 0.]);
    }

    #[test]
    fn invalid_deformation_graphs_weights_and_affine_matrices_fail() {
        let valid = skin_fixture("Normalize", 17., 26., 0.25, 0.75);
        for bad in [
            valid.replace("a: 0,1", "a: 0,0"),
            valid.replace("a: 0,1", "a: 0,99"),
            valid.replace("0.25,0.25", "-0.25,0.25"),
            valid.replace("0.25,0.25", "NaN,0.25"),
            valid.replace(
                "1,0,0,0,0,1,0,0,0,0,1,0,-5,0,0,1",
                "1,0,0,1,0,1,0,0,0,0,1,0,-5,0,0,1",
            ),
            valid.replace(
                "1,0,0,0,0,1,0,0,0,0,1,0,-5,0,0,1",
                "0,0,0,0,0,0,0,0,0,0,0,0,-5,0,0,1",
            ),
            valid.replace("C: \"OO\",2,11", "C: \"OO\",999,11"),
            valid.replace(
                "SkinningType: \"Linear\"",
                "SkinningType: \"DualQuaternion\"",
            ),
            valid.replace("Mode: \"Normalize\"", "Mode: \"Additive\""),
            valid.replace("C: \"OO\",10,100", "C: \"OO\",10,999"),
            valid.replace("C: \"OO\",11,10", "C: \"OO\",11,10 C: \"OO\",11,10"),
            morph_fixture(50.).replace("a: 50,100", "a: 50,50"),
            morph_fixture(50.).replace("Indexes: 0", "Indexes: 99"),
            morph_fixture(50.).replace("Vertices: 0,0,2", "Vertices: 0,NaN,2"),
        ] {
            assert!(
                parse(&bad).is_err(),
                "unexpected accepted deformation: {bad}"
            );
        }
    }

    #[test]
    fn hierarchy_pivots_and_geometric_transforms_do_not_propagate() {
        let text = ascii(
            &format!(
                r#"{TRIANGLE}
            Model: 1, "Model::parent", "Null" {{ Properties70: {{
                P: "Lcl Translation", "Vector3D", "Vector", "A", 10,0,0
                P: "Lcl Rotation", "Vector3D", "Vector", "A", 0,0,90
            }} }}
            Model: 2, "Model::child", "Mesh" {{ Properties70: {{
                P: "Lcl Translation", "Vector3D", "Vector", "A", 2,0,0
                P: "Lcl Rotation", "Vector3D", "Vector", "A", 0,0,90
                P: "Lcl Scaling", "Vector3D", "Vector", "A", 2,3,1
                P: "RotationPivot", "Vector3D", "Vector", "", 1,0,0
                P: "GeometricTranslation", "Vector3D", "Vector", "", 0,1,0
                P: "InheritType", "enum", "", "", 1
            }} }}
            Model: 3, "Model::grandchild", "Mesh" {{}}
        "#
            ),
            r#"C: "OO",1,0
            C: "OO",2,1
            C: "OO",3,2
            C: "OO",100,2
            C: "OO",100,3"#,
        );
        let scene = parse(&text).unwrap();
        assert_eq!(scene.triangles.len(), 2);
        let vertices = scene.triangles[0].vertices;
        close(vertices[0], [11., 0., 0.]);
        close(vertices[1], [9., 0., 0.]);
        close(vertices[2], [11., -3., 0.]);
        close(scene.triangles[1].vertices[0], [11., 3., 0.]);
    }

    #[test]
    fn nonuniform_parent_scale_respects_each_inheritance_mode() {
        for (mode, y) in [(0, 2.), (1, 3.), (2, 1.)] {
            let text = ascii(
                &format!(
                    r#"{TRIANGLE}
                Model: 1, "parent", "Null" {{ Properties70: {{
                    P: "Lcl Scaling", "Vector3D", "Vector", "A", 2,3,1
                }} }}
                Model: 2, "child", "Mesh" {{ Properties70: {{
                    P: "Lcl Translation", "Vector3D", "Vector", "A", 1,0,0
                    P: "Lcl Rotation", "Vector3D", "Vector", "A", 0,0,90
                    P: "InheritType", "enum", "", "", {mode}
                }} }}"#
                ),
                r#"C: "OO",2,1
                    C: "OO",100,2"#,
            );
            let scene = parse(&text).unwrap();
            close(scene.triangles[0].vertices[0], [2., 0., 0.]);
            close(scene.triangles[0].vertices[1], [2., y, 0.]);
        }
    }

    #[test]
    fn axis_metadata_units_and_pre_post_rotations() {
        let default = Ascii::new("Objects: {}").unwrap().parse().unwrap();
        close(
            axis_basis(&default).unwrap().apply(Vec3::new(1., 2., 3.)),
            [0.01, -0.03, 0.02],
        );
        let text = ascii(
            &format!(
                r#"{TRIANGLE}
            Model: 1, "box", "Mesh" {{ Properties70: {{
                P: "PreRotation", "Vector3D", "Vector", "", 0,0,90
                P: "PostRotation", "Vector3D", "Vector", "", 0,0,90
            }} }}"#
            ),
            r#"C: "OO",100,1"#,
        );
        let scene = parse(&text).unwrap();
        close(scene.triangles[0].vertices[1], [1., 0., 0.]);
        close(
            rotation(Vec3::new(90., 90., 0.), 0.)
                .unwrap()
                .apply(Vec3::new(0., 1., 0.)),
            [1., 0., 0.],
        );
        close(
            rotation(Vec3::new(90., 90., 0.), 3.)
                .unwrap()
                .apply(Vec3::new(0., 1., 0.)),
            [0., 0., 1.],
        );
    }

    #[test]
    fn first_animation_keys_and_definition_defaults() {
        let text = format!(
            r#"Definitions: {{ ObjectType: "Model" {{
            PropertyTemplate: "FbxNode" {{ Properties70: {{
                P: "Lcl Translation", "Vector3D", "Vector", "A", 0,4,0
            }} }} }} }}
            {}"#,
            ascii(
                &format!(
                    r#"{TRIANGLE}
            Model: 1, "box", "Mesh" {{}}
            AnimationCurveNode: 2, "AnimCurveNode::T", "" {{}}
            AnimationCurve: 3, "AnimCurve::X", "" {{
                KeyTime: *2 {{ a: 0,46186158000 }}
                KeyValueFloat: *2 {{ a: 2,20 }}
            }}"#
                ),
                r#"C: "OO",100,1
            C: "OP",3,2,"d|X"
            C: "OP",2,1,"Lcl Translation""#
            )
        );
        let scene = parse(&text).unwrap();
        close(scene.triangles[0].vertices[0], [2., 4., 0.]);
        close(scene.triangles[0].vertices[1], [3., 4., 0.]);
    }

    #[test]
    fn polygon_materials_indexed_colors_and_display_layer_links() {
        let text = ascii(
            r#"Geometry: 100, "square", "Mesh" {
            Vertices: *12 { a: 0,0,0,1,0,0,1,1,0,0,1,0 }
            PolygonVertexIndex: *6 { a: 0,1,-3,0,2,-4 }
            LayerElementMaterial: 0 {
                MappingInformationType: "ByPolygon"
                ReferenceInformationType: "IndexToDirect"
                Materials: *2 { a: 0,1 }
            }
            LayerElementColor: 0 {
                MappingInformationType: "ByPolygonVertex"
                ReferenceInformationType: "IndexToDirect"
                Colors: *8 { a: 1,1,1,1,0.25,0.25,0.25,1 }
                ColorIndex: *6 { a: 0,0,0,1,1,1 }
            }
        }
        Model: 1, "square", "Mesh" {}
        DisplayLayer: 8, "layer", "" {}
        Material: 2, "red", "" { Properties70: {
            P: "DiffuseColor", "Color", "", "A", 1,0,0
        } }
        Material: 3, "green", "" { Properties70: {
            P: "DiffuseColor", "Color", "", "A", 0,1,0
        } }"#,
            r#"C: "OO",100,1
        C: "OO",2,1
        C: "OO",3,1
        C: "OO",1,8"#,
        );
        let scene = parse(&text).unwrap();
        assert_eq!(scene.triangles[0].color, Some([255, 0, 0]));
        assert_eq!(scene.triangles[1].color, Some([0, 137, 0]));
    }

    #[test]
    fn legacy_ascii_inline_geometry_and_visibility() {
        let legacy = format!(
            r#"{GLOBAL}
            Objects: {{ Model: "Model::old", "Mesh" {{
                Properties60: {{ Property: "Lcl Translation", "Vector3D", "A", 3,2,1 }}
                Vertices: 0,0,0,1,0,0,0,1,0
                PolygonVertexIndex: 0,1,-3
            }} }}
            Connections: {{ Connect: "OO","Model::old","Model::Scene" }}"#
        );
        close(
            parse(&legacy).unwrap().triangles[0].vertices[0],
            [3., 2., 1.],
        );
        let hidden = ascii(
            &format!(
                r#"{TRIANGLE}
            Model: 1, "hidden", "Mesh" {{ Properties70: {{ P: "Visibility", "double", "Number", "", 0 }} }}
        "#
            ),
            r#"C: "OO",100,1"#,
        );
        assert!(
            parse(&hidden)
                .unwrap_err()
                .to_string()
                .contains("no renderable geometry")
        );
    }

    #[test]
    fn holes_points_lines_and_concave_polygons() {
        let text = ascii(
            r#"Geometry: 100,"mixed","Mesh" {
            Vertices: *15 { a: 0,0,0, 2,0,0, 2,2,0, 1,1,0, 0,2,0 }
            PolygonVertexIndex: *11 { a: -1,0,-2,0,1,-3,0,1,2,3,-5 }
            LayerElementHole: 0 {
                MappingInformationType: "ByPolygon"
                Hole: *4 { a: 0,0,1,0 }
            }
        }"#,
            "",
        );
        let scene = parse(&text).unwrap();
        assert_eq!(scene.points.len(), 1);
        assert_eq!(scene.lines.len(), 1);
        let area = scene
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    / 2.
            })
            .sum::<f64>();
        assert!((area - 3.).abs() < 1e-12);
    }

    #[test]
    fn malformed_unknown_and_deformed_models_fail_cleanly() {
        for objects in [
            "Geometry: 1,\"nurbs\",\"NurbsSurface\" {}",
            "Deformer: 1,\"skin\",\"Skin\" {}",
            "Geometry: 1,\"bad\",\"Mesh\" { Vertices: *3 { a: 0,0 } }",
            "Geometry: 1,\"bad\",\"Mesh\" { Vertices: *3 { a: 0,0,nan } }",
            "Geometry: 1,\"bad\",\"Mesh\" { Vertices: *3 { a: 0,0,0 } PolygonVertexIndex: -2 }",
            "Geometry: 1,\"bad\",\"Mesh\" { Vertices: *3 { a: 0,0,0 } PolygonVertexIndex: 0 }",
        ] {
            assert!(parse(&ascii(objects, "")).is_err(), "{objects}");
        }
        let cycle = ascii(
            &format!("{TRIANGLE}\nModel: 1,\"a\",\"Mesh\" {{}}\nModel: 2,\"b\",\"Null\" {{}}"),
            r#"C: "OO",1,2
            C: "OO",2,1
            C: "OO",100,1"#,
        );
        assert!(parse(&cycle).unwrap_err().to_string().contains("cyclic"));
        assert!(
            parse(&ascii(
                &format!("{TRIANGLE}\nModel: 1,\"a\",\"Mesh\" {{}}"),
                r#"C: "OO",1,999"#
            ))
            .is_err()
        );
        assert!(
            Ascii::new("Objects: { Model: 1,\"unfinished\"")
                .unwrap()
                .parse()
                .is_err()
        );
    }

    fn binary_array(code: u8, length: usize, raw: &[u8], compressed: bool) -> Vec<u8> {
        let payload = if compressed {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(raw).unwrap();
            encoder.finish().unwrap()
        } else {
            raw.to_vec()
        };
        let mut bytes = vec![code];
        bytes.extend_from_slice(&(length as u32).to_le_bytes());
        bytes.extend_from_slice(&u32::from(compressed).to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);
        bytes
    }
    fn binary_value(value: &Value, compressed: bool) -> Vec<u8> {
        match value {
            Value::I64(v) => {
                let mut b = vec![b'L'];
                b.extend_from_slice(&v.to_le_bytes());
                b
            }
            Value::F64(v) => {
                let mut b = vec![b'D'];
                b.extend_from_slice(&v.to_le_bytes());
                b
            }
            Value::String(v) => {
                let mut b = vec![b'S'];
                b.extend_from_slice(&(v.len() as u32).to_le_bytes());
                b.extend_from_slice(v.as_bytes());
                b
            }
            Value::ArrF64(v) => binary_array(
                b'd',
                v.len(),
                &v.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(),
                compressed,
            ),
            Value::ArrI32(v) => binary_array(
                b'i',
                v.len(),
                &v.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(),
                compressed,
            ),
            _ => panic!("unexpected fixture value"),
        }
    }
    fn binary_node(node: &Node, bytes: &mut Vec<u8>, wide: bool, compressed: bool) {
        let offset = bytes.len();
        let n = if wide { 8 } else { 4 };
        let header = n * 3 + 1;
        bytes.resize(offset + header, 0);
        bytes[offset + header - 1] = node.name.len() as u8;
        bytes.extend_from_slice(node.name.as_bytes());
        let props = bytes.len();
        for v in &node.values {
            bytes.extend_from_slice(&binary_value(v, compressed));
        }
        let prop_bytes = bytes.len() - props;
        for child in &node.children {
            binary_node(child, bytes, wide, compressed);
        }
        bytes.resize(bytes.len() + header, 0);
        for (i, value) in [bytes.len(), node.values.len(), prop_bytes]
            .into_iter()
            .enumerate()
        {
            if wide {
                bytes[offset + i * n..offset + (i + 1) * n]
                    .copy_from_slice(&(value as u64).to_le_bytes());
            } else {
                bytes[offset + i * n..offset + (i + 1) * n]
                    .copy_from_slice(&(value as u32).to_le_bytes());
            }
        }
    }
    fn binary_fixture(version: u32, compressed: bool) -> Vec<u8> {
        let text = ascii(
            &format!(
                r#"{TRIANGLE}
            Model: 1,"cube","Mesh" {{ Properties70: {{
                P: "Lcl Translation", "Vector3D", "Vector", "A", 3,2,1
            }} }}"#
            ),
            r#"C: "OO",100,1"#,
        );
        let mut tree = Ascii::new(&text).unwrap().parse().unwrap();
        let geometry = tree
            .children
            .iter_mut()
            .find(|n| n.name == "Objects")
            .unwrap()
            .children
            .iter_mut()
            .find(|n| n.name == "Geometry")
            .unwrap();
        for node in &mut geometry.children {
            if node.name == "Vertices" {
                node.values = vec![Value::ArrF64(array(node).unwrap())];
                node.children.clear();
            }
            if node.name == "PolygonVertexIndex" {
                node.values = vec![Value::ArrI32(
                    int_array(node)
                        .unwrap()
                        .into_iter()
                        .map(|v| v as i32)
                        .collect(),
                )];
                node.children.clear();
            }
        }
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&version.to_le_bytes());
        for child in &tree.children {
            binary_node(child, &mut bytes, version >= 7500, compressed);
        }
        bytes.resize(bytes.len() + if version >= 7500 { 25 } else { 13 }, 0);
        bytes
    }

    #[test]
    fn binary_32_and_64_bit_records_with_compressed_arrays() {
        for version in [7100, 7400, 7500] {
            for compressed in [false, true] {
                let fixture = Fixture::new(&binary_fixture(version, compressed));
                let scene = load(&fixture.0).unwrap();
                assert_eq!(scene.triangles.len(), 1);
                close(scene.triangles[0].vertices[0], [3., 2., 1.]);
                close(scene.triangles[0].vertices[1], [4., 2., 1.]);
            }
        }
    }

    #[test]
    fn binary_skin_and_morph_deformations_use_real_decoded_arrays() {
        fn arrays(node: &mut Node) {
            if matches!(
                node.name.as_str(),
                "Vertices" | "Weights" | "Transform" | "TransformLink" | "FullWeights"
            ) {
                node.values = vec![Value::ArrF64(array(node).unwrap())];
                node.children.clear();
            } else if matches!(node.name.as_str(), "Indexes" | "PolygonVertexIndex") {
                node.values = vec![Value::ArrI32(
                    int_array(node)
                        .unwrap()
                        .into_iter()
                        .map(|i| i as i32)
                        .collect(),
                )];
                node.children.clear();
            }
            for child in &mut node.children {
                arrays(child);
            }
        }
        for (source, expected) in [
            (
                skin_fixture("Normalize", 17., 26., 0.25, 0.75),
                [18., 0., 0.],
            ),
            (morph_fixture(75.), [0., 0., 4.]),
        ] {
            let mut tree = Ascii::new(&source).unwrap().parse().unwrap();
            arrays(&mut tree);
            for version in [7400u32, 7500] {
                for compressed in [false, true] {
                    let mut bytes = MAGIC.to_vec();
                    bytes.extend_from_slice(&version.to_le_bytes());
                    for child in &tree.children {
                        binary_node(child, &mut bytes, version >= 7500, compressed);
                    }
                    bytes.resize(bytes.len() + if version >= 7500 { 25 } else { 13 }, 0);
                    let fixture = Fixture::new(&bytes);
                    close(load(&fixture.0).unwrap().triangles[0].vertices[0], expected);
                }
            }
        }
    }

    #[test]
    fn truncated_binary_and_huge_array_counts_are_rejected() {
        let bytes = binary_fixture(7500, true);
        for len in [23, 27, 40, bytes.len() - 30] {
            assert!(preflight_binary(&bytes[..len]).is_err());
        }
        let mut bytes = binary_fixture(7400, false);
        let marker = b"Vertices";
        let offset = bytes
            .windows(marker.len())
            .position(|p| p == marker)
            .unwrap()
            + marker.len();
        bytes[offset + 1..offset + 5].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            preflight_binary(&bytes)
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
    }
}
