//! Static DirectX X meshes and frames in text, binary and MSZIP containers.
use super::mesh::triangulate_polygon;
use super::transform::Transform;
use crate::scene::{Scene, Triangle, Vec3};
use crate::{Error, Result};
use flate2::{Decompress, FlushDecompress, Status};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

const MAX_ITEMS: usize = 20_000_000;
const MAX_OBJECTS: usize = 100_000;
fn error(message: impl Into<String>) -> Error {
    message.into().into()
}
#[derive(Clone, Debug, PartialEq)]
enum Token {
    Word(String),
    Number(f64),
    String,
    Symbol(char),
    Guid,
}
struct Bytes<'a> {
    bytes: &'a [u8],
    cursor: usize,
}
impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .cursor
            .checked_add(n)
            .ok_or_else(|| error("X file offset overflow"))?;
        let bytes = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| error("truncated X file record"))?;
        self.cursor = end;
        Ok(bytes)
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn float(&mut self, size: usize) -> Result<f64> {
        let n = if size == 4 {
            f32::from_le_bytes(self.take(4)?.try_into()?) as f64
        } else {
            f64::from_le_bytes(self.take(8)?.try_into()?)
        };
        if !n.is_finite() {
            return Err(error("non-finite X file value"));
        }
        Ok(n)
    }
}
fn compressed(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut data = Bytes { bytes, cursor: 0 };
    let declared = data.u32()? as usize;
    let expected = declared
        .checked_sub(16)
        .ok_or_else(|| error("X compressed file size omits its header"))?;
    if expected as u64 > super::MAX_INPUT_BYTES {
        return Err(error("X decompressed payload exceeds 512 MiB"));
    }
    let mut output = Vec::with_capacity(expected.min(1_048_576));
    while data.cursor < bytes.len() {
        let output_size = data.u16()? as usize;
        let input_size = data.u16()? as usize;
        if output_size == 0 || output_size > 32768 || input_size < 2 {
            return Err(error("invalid X MSZIP block lengths"));
        }
        let input = data.take(input_size)?;
        if &input[..2] != b"CK" {
            return Err(error("X MSZIP block lacks CK signature"));
        }
        let mut decoder = Decompress::new(false);
        if !output.is_empty() {
            let start = output.len().saturating_sub(32768);
            decoder.set_dictionary(&output[start..])?;
        }
        let mut block = vec![0; output_size];
        let status = decoder.decompress(&input[2..], &mut block, FlushDecompress::Finish)?;
        if status != Status::StreamEnd
            || decoder.total_out() != output_size as u64
            || decoder.total_in() != (input_size - 2) as u64
        {
            return Err(error("X MSZIP block did not match its declared sizes"));
        }
        if output
            .len()
            .checked_add(block.len())
            .is_none_or(|n| n > expected)
        {
            return Err(error("X MSZIP output exceeds declared size"));
        }
        output.extend_from_slice(&block);
    }
    if output.len() != expected {
        return Err(error("X MSZIP output differs from declared file size"));
    }
    Ok(output)
}
fn binary(bytes: &[u8], float_size: usize) -> Result<Vec<Token>> {
    let mut data = Bytes { bytes, cursor: 0 };
    let mut tokens = Vec::new();
    while data.cursor < bytes.len() {
        match data.u16()? {
            1 => {
                let n = data.u32()? as usize;
                let name = std::str::from_utf8(data.take(n)?)?
                    .trim_end_matches('\0')
                    .to_owned();
                tokens.push(Token::Word(name));
            }
            2 => {
                let n = data.u32()? as usize;
                data.take(n)?;
                let separator = data.u16()?;
                if separator != 20 {
                    return Err(error("X binary string lacks semicolon token"));
                }
                tokens.push(Token::String);
            }
            3 => tokens.push(Token::Number(data.u32()? as f64)),
            5 => {
                data.take(16)?;
                tokens.push(Token::Guid);
            }
            6 => {
                let n = data.u32()? as usize;
                if n > MAX_ITEMS {
                    return Err(error("too many X integer-list items"));
                }
                for _ in 0..n {
                    tokens.push(Token::Number(data.u32()? as f64));
                }
            }
            7 => {
                let n = data.u32()? as usize;
                if n > MAX_ITEMS {
                    return Err(error("too many X float-list items"));
                }
                for _ in 0..n {
                    tokens.push(Token::Number(data.float(float_size)?));
                }
            }
            id @ 10..=20 => {
                let symbol =
                    ['{', '}', '(', ')', '[', ']', '<', '>', '.', ',', ';'][(id - 10) as usize];
                if symbol != ',' && symbol != ';' {
                    tokens.push(Token::Symbol(symbol));
                }
            }
            31 => tokens.push(Token::Word("template".into())),
            id @ 40..=52 => tokens.push(Token::Word(
                [
                    "WORD", "DWORD", "FLOAT", "DOUBLE", "CHAR", "UCHAR", "SWORD", "SDWORD", "VOID",
                    "LPSTR", "UNICODE", "CSTRING", "array",
                ][(id - 40) as usize]
                    .into(),
            )),
            id => return Err(error(format!("unsupported X binary token {id}"))),
        }
        if tokens.len() > MAX_ITEMS {
            return Err(error("too many X tokens"));
        }
    }
    Ok(tokens)
}
fn text(bytes: &[u8]) -> Result<Vec<Token>> {
    let source = std::str::from_utf8(bytes)?;
    let mut chars = source.chars().peekable();
    let mut tokens = Vec::new();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() || matches!(c, ',' | ';') => {}
            '#' => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return Err(error("unterminated X file comment"));
                }
            }
            '"' => {
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '"' {
                        closed = true;
                        break;
                    }
                    if c == '\\' {
                        chars
                            .next()
                            .ok_or_else(|| error("truncated X file string"))?;
                    }
                }
                if !closed {
                    return Err(error("unterminated X file string"));
                }
                tokens.push(Token::String);
            }
            '<' => {
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '>' {
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return Err(error("unterminated X file GUID"));
                }
                tokens.push(Token::Guid);
            }
            '{' | '}' | '[' | ']' | '(' | ')' => tokens.push(Token::Symbol(c)),
            _ => {
                let mut word = String::from(c);
                while chars.peek().is_some_and(|c| {
                    !c.is_whitespace()
                        && !matches!(
                            c,
                            ',' | ';' | '{' | '}' | '[' | ']' | '(' | ')' | '<' | '>' | '"'
                        )
                }) {
                    word.push(chars.next().unwrap());
                }
                if let Ok(n) = word.parse::<f64>() {
                    if !n.is_finite() {
                        return Err(error("non-finite X file value"));
                    }
                    tokens.push(Token::Number(n));
                } else {
                    tokens.push(Token::Word(word));
                }
            }
        }
        if tokens.len() > MAX_ITEMS {
            return Err(error("too many X tokens"));
        }
    }
    Ok(tokens)
}

#[derive(Clone, Debug)]
enum Entry {
    Object(Rc<Object>),
    Reference(String),
}
#[derive(Debug)]
struct Object {
    kind: String,
    name: Option<String>,
    numbers: Vec<f64>,
    strings: usize,
    entries: Vec<Entry>,
}
struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
    count: usize,
}
impl Parser {
    fn word(&mut self) -> Result<String> {
        match self.tokens.get(self.cursor).cloned() {
            Some(Token::Word(s)) => {
                self.cursor += 1;
                Ok(s)
            }
            _ => Err(error("expected X object name")),
        }
    }
    fn symbol(&mut self, c: char) -> Result<()> {
        if self.tokens.get(self.cursor) != Some(&Token::Symbol(c)) {
            return Err(error(format!("expected X '{c}'")));
        }
        self.cursor += 1;
        Ok(())
    }
    fn template(&mut self) -> Result<()> {
        self.word()?;
        self.symbol('{')?;
        let mut depth = 1;
        while depth != 0 {
            match self.tokens.get(self.cursor) {
                Some(Token::Symbol('{')) => depth += 1,
                Some(Token::Symbol('}')) => depth -= 1,
                None => return Err(error("truncated X template")),
                _ => {}
            }
            self.cursor += 1;
        }
        Ok(())
    }
    fn object(&mut self, depth: usize) -> Result<Rc<Object>> {
        self.count += 1;
        if depth > 256 || self.count > MAX_OBJECTS {
            return Err(error("X object/nesting limit exceeded"));
        }
        let kind = self.word()?;
        let name = if matches!(self.tokens.get(self.cursor), Some(Token::Word(_))) {
            Some(self.word()?)
        } else {
            None
        };
        if self.tokens.get(self.cursor) == Some(&Token::Guid) {
            self.cursor += 1;
        }
        self.symbol('{')?;
        let mut numbers = Vec::new();
        let mut strings = 0;
        let mut entries = Vec::new();
        while self.tokens.get(self.cursor) != Some(&Token::Symbol('}')) {
            match self.tokens.get(self.cursor).cloned() {
                Some(Token::Number(n)) => {
                    numbers.push(n);
                    self.cursor += 1;
                }
                Some(Token::String) => {
                    strings += 1;
                    self.cursor += 1;
                }
                Some(Token::Guid) => self.cursor += 1,
                Some(Token::Word(_)) => entries.push(Entry::Object(self.object(depth + 1)?)),
                Some(Token::Symbol('{')) => {
                    self.cursor += 1;
                    let name = self.word()?;
                    self.symbol('}')?;
                    entries.push(Entry::Reference(name));
                }
                _ => return Err(error("invalid X object content")),
            }
        }
        self.cursor += 1;
        Ok(Rc::new(Object {
            kind,
            name,
            numbers,
            strings,
            entries,
        }))
    }
}
struct Numbers<'a> {
    numbers: &'a [f64],
    cursor: usize,
}
impl Numbers<'_> {
    fn number(&mut self) -> Result<f64> {
        let n = *self
            .numbers
            .get(self.cursor)
            .ok_or_else(|| error("truncated X object numeric data"))?;
        self.cursor += 1;
        Ok(n)
    }
    fn count(&mut self) -> Result<usize> {
        let n = self.number()?;
        if n < 0. || n.fract() != 0. || n > MAX_ITEMS as f64 {
            return Err(error("invalid X count/index"));
        }
        Ok(n as usize)
    }
    fn dword(&mut self) -> Result<u32> {
        let n = self.number()?;
        if n < 0. || n.fract() != 0. || n > u32::MAX as f64 {
            return Err(error("invalid X DWORD"));
        }
        Ok(n as u32)
    }
    fn vector(&mut self) -> Result<Vec3> {
        Ok(Vec3::new(self.number()?, self.number()?, self.number()?))
    }
    fn done(&self) -> Result<()> {
        if self.cursor != self.numbers.len() {
            return Err(error("extra numeric data in X object"));
        }
        Ok(())
    }
}
fn rgb(values: &[f64]) -> [u8; 3] {
    std::array::from_fn(|i| (values[i] * 255.).clamp(0., 255.).round() as u8)
}
fn material(object: &Object) -> Result<[u8; 3]> {
    if object.kind != "Material" || object.numbers.len() != 11 {
        return Err(error(
            "X material needs RGBA, power, specular RGB and emissive RGB",
        ));
    }
    Ok(rgb(&object.numbers[..3]))
}
fn frame_matrix(object: &Object) -> Result<Transform> {
    if object.numbers.len() != 16 {
        return Err(error("X FrameTransformMatrix needs sixteen values"));
    }
    let matrix = Transform(std::array::from_fn(|column| {
        std::array::from_fn(|row| object.numbers[column * 4 + row])
    }));
    if matrix.0[0][3] != 0. || matrix.0[1][3] != 0. || matrix.0[2][3] != 0. || matrix.0[3][3] != 1.
    {
        return Err(error("X projective frame transforms are not supported"));
    }
    Ok(matrix)
}
struct Importer {
    names: HashMap<String, Vec<Rc<Object>>>,
    active: HashSet<usize>,
    visits: usize,
    scene: Scene,
}
impl Importer {
    fn reference(&self, name: &str, kind: Option<&str>) -> Result<Rc<Object>> {
        let matches: Vec<_> = self
            .names
            .get(name)
            .into_iter()
            .flatten()
            .filter(|object| kind.is_none_or(|k| object.kind == k))
            .collect();
        match matches.as_slice() {
            [object] => Ok((*object).clone()),
            [] => Err(error("X reference names a missing object")),
            _ => Err(error("X reference names multiple objects and is ambiguous")),
        }
    }
    fn walk_entry(&mut self, entry: &Entry, world: Transform, depth: usize) -> Result<()> {
        match entry {
            Entry::Object(object) => self.walk(object, world, depth),
            Entry::Reference(name) => {
                let object = self.reference(name, None)?;
                self.walk(&object, world, depth)
            }
        }
    }
    fn walk(&mut self, object: &Rc<Object>, world: Transform, depth: usize) -> Result<()> {
        self.visits += 1;
        if depth > 256 || self.visits > MAX_OBJECTS {
            return Err(error("X scene expansion limit exceeded"));
        }
        let key = Rc::as_ptr(object) as usize;
        if !self.active.insert(key) {
            return Err(error("cyclic X object reference"));
        }
        match object.kind.as_str() {
            "Frame" => {
                if !object.numbers.is_empty() {
                    return Err(error("X Frame has unexpected numeric values"));
                }
                let transforms: Vec<_> = object
                    .entries
                    .iter()
                    .filter_map(|e| match e {
                        Entry::Object(o) if o.kind == "FrameTransformMatrix" => Some(o),
                        _ => None,
                    })
                    .collect();
                if transforms.len() > 1 {
                    return Err(error("X Frame has multiple transforms"));
                }
                let world = if let Some(transform) = transforms.first() {
                    world.then(frame_matrix(transform)?)
                } else {
                    world
                };
                for entry in &object.entries {
                    if matches!(entry,Entry::Object(o)if o.kind=="FrameTransformMatrix") {
                        continue;
                    }
                    self.walk_entry(entry, world, depth + 1)?;
                }
            }
            "Mesh" => self.mesh(object, world)?,
            "Material" => {
                material(object)?;
            }
            "Header"
            | "AnimationSet"
            | "AnimTicksPerSecond"
            | "KeyValuePair"
            | "ObjectMatrixComment" => {}
            kind => return Err(error(format!("unsupported X scene object {kind}"))),
        }
        self.active.remove(&key);
        Ok(())
    }
    fn mesh(&mut self, object: &Object, world: Transform) -> Result<()> {
        let mut values = Numbers {
            numbers: &object.numbers,
            cursor: 0,
        };
        let count = values.count()?;
        let mut vertices = Vec::with_capacity(count.min(4096));
        for _ in 0..count {
            vertices.push(values.vector()?);
        }
        let nfaces = values.count()?;
        let mut faces = Vec::new();
        for _ in 0..nfaces {
            let count = values.count()?;
            if count < 3 {
                return Err(error("X Mesh face needs at least three vertices"));
            }
            let mut face = Vec::with_capacity(count.min(4096));
            for _ in 0..count {
                let i = values.count()?;
                if i >= vertices.len() {
                    return Err(error("X Mesh index out of bounds"));
                }
                face.push(i);
            }
            faces.push(face);
        }
        values.done()?;
        let mut materials = Vec::new();
        let mut assignments = Vec::new();
        let mut colors = HashMap::new();
        let mut material_list = false;
        for entry in &object.entries {
            let Entry::Object(child) = entry else {
                return Err(error("X Mesh contains an unsupported object reference"));
            };
            match child.kind.as_str() {
                "MeshMaterialList" => {
                    if material_list {
                        return Err(error("duplicate X MeshMaterialList"));
                    }
                    material_list = true;
                    let mut values = Numbers {
                        numbers: &child.numbers,
                        cursor: 0,
                    };
                    let count = values.count()?;
                    let n = values.count()?;
                    for _ in 0..n {
                        assignments.push(values.count()?);
                    }
                    values.done()?;
                    for entry in &child.entries {
                        let color = match entry {
                            Entry::Object(material_object) => material(material_object)?,
                            Entry::Reference(name) => {
                                material(self.reference(name, Some("Material"))?.as_ref())?
                            }
                        };
                        materials.push(color);
                    }
                    if materials.len() != count {
                        return Err(error(
                            "X MeshMaterialList material count differs from its objects",
                        ));
                    }
                    if assignments.len() == 1 && faces.len() > 1 {
                        assignments.resize(faces.len(), assignments[0]);
                    }
                    if assignments.len() != faces.len()
                        || assignments.iter().any(|&i| i >= materials.len())
                    {
                        return Err(error("X MeshMaterialList face assignments are invalid"));
                    }
                }
                "MeshVertexColors" => {
                    let mut values = Numbers {
                        numbers: &child.numbers,
                        cursor: 0,
                    };
                    let count = values.count()?;
                    for _ in 0..count {
                        let index = values.count()?;
                        let c = values.vector()?;
                        values.number()?;
                        if index >= vertices.len()
                            || colors.insert(index, rgb(&[c.x, c.y, c.z])).is_some()
                        {
                            return Err(error("invalid X MeshVertexColors index"));
                        }
                    }
                    values.done()?;
                }
                "MeshNormals" => {
                    let mut values = Numbers {
                        numbers: &child.numbers,
                        cursor: 0,
                    };
                    let count = values.count()?;
                    for _ in 0..count {
                        values.vector()?;
                    }
                    let n = values.count()?;
                    if n != faces.len() {
                        return Err(error("X MeshNormals face count differs from Mesh"));
                    }
                    for face in &faces {
                        let n = values.count()?;
                        if n != face.len() {
                            return Err(error("X MeshNormals face size differs from Mesh"));
                        }
                        for _ in 0..n {
                            if values.count()? >= count {
                                return Err(error("X MeshNormals index out of bounds"));
                            }
                        }
                    }
                    values.done()?;
                }
                "MeshTextureCoords" => {
                    let mut values = Numbers {
                        numbers: &child.numbers,
                        cursor: 0,
                    };
                    if values.count()? != vertices.len() {
                        return Err(error("X texture-coordinate count differs from Mesh"));
                    }
                    for _ in &vertices {
                        values.number()?;
                        values.number()?;
                    }
                    values.done()?;
                }
                "SkinWeights" => {
                    if child.strings != 1 {
                        return Err(error("X SkinWeights requires one bone name"));
                    }
                    let mut values = Numbers {
                        numbers: &child.numbers,
                        cursor: 0,
                    };
                    let count = values.count()?;
                    let mut indices = HashSet::new();
                    for _ in 0..count {
                        let index = values.count()?;
                        if index >= vertices.len() || !indices.insert(index) {
                            return Err(error("invalid X SkinWeights vertex index"));
                        }
                    }
                    for _ in 0..count {
                        if !(0.0..=1.0).contains(&values.number()?) {
                            return Err(error("invalid X skin weight"));
                        }
                    }
                    // Preserve the Mesh's authored bind geometry for the fixed
                    // preview; the bone offset describes joint deformation.
                    for _ in 0..16 {
                        values.number()?;
                    }
                    values.done()?;
                }
                "XSkinMeshHeader" => {
                    let mut values = Numbers {
                        numbers: &child.numbers,
                        cursor: 0,
                    };
                    for _ in 0..3 {
                        values.count()?;
                    }
                    values.done()?;
                }
                "VertexDuplicationIndices" => {
                    let mut values = Numbers {
                        numbers: &child.numbers,
                        cursor: 0,
                    };
                    let count = values.count()?;
                    let original_count = values.count()?;
                    if count != vertices.len() {
                        return Err(error("X VertexDuplicationIndices count differs from Mesh"));
                    }
                    for _ in 0..count {
                        if values.count()? >= original_count {
                            return Err(error("X duplicate-vertex index out of bounds"));
                        }
                    }
                    values.done()?;
                }
                "DeclData" | "FVFData" => {
                    let mut values = Numbers {
                        numbers: &child.numbers,
                        cursor: 0,
                    };
                    if child.kind == "DeclData" {
                        let elements = values.count()?;
                        for _ in 0..elements {
                            for _ in 0..4 {
                                values.dword()?;
                            }
                        }
                    } else {
                        values.dword()?;
                    }
                    let count = values.count()?;
                    for _ in 0..count {
                        values.dword()?;
                    }
                    values.done()?;
                }
                kind => return Err(error(format!("unsupported X Mesh child {kind}"))),
            }
        }
        for (face_index, face) in faces.iter().enumerate() {
            let base = assignments.get(face_index).map(|&i| materials[i]);
            let vertex_colors: Vec<_> =
                face.iter().filter_map(|i| colors.get(i).copied()).collect();
            let color = if vertex_colors.is_empty() {
                base
            } else {
                Some(std::array::from_fn(|i| {
                    let average = vertex_colors.iter().map(|c| c[i] as u64).sum::<u64>()
                        / vertex_colors.len() as u64;
                    ((average * base.map_or(255, |c| c[i]) as u64) / 255) as u8
                }))
            };
            let polygon: Vec<_> = face.iter().map(|&i| vertices[i]).collect();
            for triangle in triangulate_polygon(&polygon)? {
                let mut vertices = triangle.map(|p| {
                    let p = world.apply(p);
                    Vec3::new(p.x, p.z, p.y)
                });
                vertices.swap(1, 2);
                if vertices.iter().any(|p| !p.is_finite()) {
                    return Err(error("X transformed vertex is non-finite"));
                }
                self.scene.triangles.push(Triangle { vertices, color });
                if self.scene.triangles.len() > MAX_ITEMS {
                    return Err(error("too much X mesh geometry"));
                }
            }
        }
        Ok(())
    }
}
fn collect_names(object: &Rc<Object>, names: &mut HashMap<String, Vec<Rc<Object>>>) {
    if let Some(name) = &object.name {
        names.entry(name.clone()).or_default().push(object.clone());
    }
    for entry in &object.entries {
        if let Entry::Object(child) = entry {
            collect_names(child, names);
        }
    }
}
pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    parse(&bytes)
}
fn parse(bytes: &[u8]) -> Result<Scene> {
    let header = bytes
        .get(..16)
        .ok_or_else(|| error("truncated X file header"))?;
    if &header[..4] != b"xof " || !header[4..8].iter().all(u8::is_ascii_digit) {
        return Err(error("invalid X file header"));
    }
    let float_size = match &header[12..16] {
        b"0032" => 4,
        b"0064" => 8,
        _ => return Err(error("X file float size must be 32 or 64")),
    };
    let unpacked;
    let payload = if matches!(&header[8..12], b"tzip" | b"bzip") {
        unpacked = compressed(&bytes[16..])?;
        unpacked.as_slice()
    } else {
        &bytes[16..]
    };
    let tokens = match &header[8..12] {
        b"txt " | b"tzip" => text(payload)?,
        b"bin " | b"bzip" => binary(payload, float_size)?,
        _ => return Err(error("unsupported X file encoding")),
    };
    let mut parser = Parser {
        tokens,
        cursor: 0,
        count: 0,
    };
    let mut objects = Vec::new();
    while parser.cursor < parser.tokens.len() {
        if parser.tokens.get(parser.cursor) == Some(&Token::Word("template".into())) {
            parser.cursor += 1;
            parser.template()?;
        } else {
            objects.push(parser.object(0)?);
        }
    }
    let mut names = HashMap::new();
    for object in &objects {
        collect_names(object, &mut names);
    }
    let mut importer = Importer {
        names,
        active: HashSet::new(),
        visits: 0,
        scene: Scene::default(),
    };
    for object in &objects {
        importer.walk(object, Transform::IDENTITY, 0)?;
    }
    importer.scene.validate()?;
    Ok(importer.scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    const MESH: &str = "Mesh mesh { 3; 0;0;0;, 1;0;0;, 0;1;0;; 1; 3;0,1,2;; MeshMaterialList { 1; 1; 0;; Material red { 1;0;0;1;; 0; 0;0;0;; 0;0;0;; } } }";
    #[test]
    fn text_frames_materials_and_left_handed_y_up() {
        let source = format!(
            "xof 0303txt 0032\nFrame root {{ FrameTransformMatrix {{1,0,0,0,0,1,0,0,0,0,1,0,2,3,4,1;;}} {MESH} }}"
        );
        let scene = parse(source.as_bytes()).unwrap();
        assert_eq!(scene.triangles.len(), 1);
        assert_eq!(scene.triangles[0].vertices[0], Vec3::new(2., 4., 3.));
        assert_eq!(scene.triangles[0].color, Some([255, 0, 0]));
    }
    fn binary_mesh(float_size: usize) -> Vec<u8> {
        let mut bytes = format!("xof 0303bin {:04}", float_size * 8).into_bytes();
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&4_u32.to_le_bytes());
        bytes.extend_from_slice(b"Mesh");
        bytes.extend_from_slice(&10_u16.to_le_bytes());
        bytes.extend_from_slice(&3_u16.to_le_bytes());
        bytes.extend_from_slice(&3_u32.to_le_bytes());
        bytes.extend_from_slice(&7_u16.to_le_bytes());
        bytes.extend_from_slice(&9_u32.to_le_bytes());
        for n in [0_f64, 0., 0., 1., 0., 0., 0., 1., 0.] {
            if float_size == 4 {
                bytes.extend_from_slice(&(n as f32).to_le_bytes());
            } else {
                bytes.extend_from_slice(&n.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&6_u16.to_le_bytes());
        bytes.extend_from_slice(&5_u32.to_le_bytes());
        for n in [1_u32, 3, 0, 1, 2] {
            bytes.extend_from_slice(&n.to_le_bytes());
        }
        bytes.extend_from_slice(&11_u16.to_le_bytes());
        bytes
    }
    #[test]
    fn binary_32_64_and_truncation() {
        for float_size in [4, 8] {
            let bytes = binary_mesh(float_size);
            let scene = parse(&bytes).unwrap();
            assert_eq!(scene.triangles.len(), 1);
            assert!(parse(&bytes[..bytes.len() - 1]).is_err());
        }
    }
    fn zip(payload: &[u8], binary: bool) -> Vec<u8> {
        use std::io::Write;
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(payload).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut file = format!("xof 0303{}0032", if binary { "bzip" } else { "tzip" }).into_bytes();
        file.extend_from_slice(&((payload.len() + 16) as u32).to_le_bytes());
        file.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        file.extend_from_slice(&((compressed.len() + 2) as u16).to_le_bytes());
        file.extend_from_slice(b"CK");
        file.extend_from_slice(&compressed);
        file
    }
    #[test]
    fn mszip_text_and_binary() {
        let file = zip(MESH.as_bytes(), false);
        assert_eq!(parse(&file).unwrap().triangles.len(), 1);
        assert!(parse(&file[..file.len() - 1]).is_err());
        let binary = binary_mesh(4);
        assert_eq!(parse(&zip(&binary[16..], true)).unwrap().triangles.len(), 1);
    }
    #[test]
    fn mszip_rolling_dictionary_across_blocks() {
        use flate2::{Compress, Compression, FlushCompress};
        let mut payload = Vec::new();
        for _ in 0..400 {
            payload.extend_from_slice(b"// a repeated comment referring to the preceding block\n");
        }
        payload.extend_from_slice(MESH.as_bytes());
        let mut file = b"xof 0303tzip0032".to_vec();
        file.extend_from_slice(&((payload.len() + 16) as u32).to_le_bytes());
        let mut consumed = 0_usize;
        for block in payload.chunks(8192) {
            let mut encoder = Compress::new(Compression::best(), false);
            if consumed != 0 {
                encoder
                    .set_dictionary(&payload[consumed.saturating_sub(32768)..consumed])
                    .unwrap();
            }
            let mut encoded = Vec::with_capacity(block.len() + 128);
            assert_eq!(
                encoder
                    .compress_vec(block, &mut encoded, FlushCompress::Finish)
                    .unwrap(),
                Status::StreamEnd
            );
            file.extend_from_slice(&(block.len() as u16).to_le_bytes());
            file.extend_from_slice(&((encoded.len() + 2) as u16).to_le_bytes());
            file.extend_from_slice(b"CK");
            file.extend_from_slice(&encoded);
            consumed += block.len();
        }
        assert_eq!(parse(&file).unwrap().triangles.len(), 1);
    }
    #[test]
    fn rejects_invalid_mesh_indices_and_required_features() {
        assert!(
            parse(format!("xof 0303txt 0032\n{}", MESH.replace("3;0,1,2", "3;0,1,3")).as_bytes())
                .is_err()
        );
        assert!(
            parse(
                format!(
                    "xof 0303txt 0032\n{}",
                    MESH.replace("MeshMaterialList", "SkinWeights")
                )
                .as_bytes()
            )
            .is_err()
        );
        assert!(parse(b"xof 0303txt 0032\nUnknownShape {}").is_err());
    }
    #[test]
    fn named_material_references_and_templates() {
        let source = format!(
            "xof 0303txt 0032\ntemplate ignored {{<12345678-1234-1234-1234-123456789abc> DWORD count; [ ... ]}}\nMaterial red {{0;1;0;1;;0;0;0;0;;0;0;0;;}}\n{}",
            MESH.replace("Material red { 1;0;0;1;; 0; 0;0;0;; 0;0;0;; }", "{red}")
        );
        let scene = parse(source.as_bytes()).unwrap();
        assert_eq!(scene.triangles[0].color, Some([0, 255, 0]));
    }
    #[test]
    fn duplicate_authored_names_need_not_be_referenced() {
        let source = format!(
            "xof 0303txt 0032\nFrame mesh {{{MESH}}} Material mesh {{0;1;0;1;;0;0;0;0;;0;0;0;;}}"
        );
        assert_eq!(parse(source.as_bytes()).unwrap().triangles.len(), 1);
        let source = format!("{source}\nFrame another {{{{mesh}}}}");
        assert!(parse(source.as_bytes()).is_err());
    }
    #[test]
    fn authored_bind_geometry_with_optional_packed_attributes() {
        let source = format!(
            "xof 0303txt 0032\nKeyValuePair {{\"Author\";\"Example\";}}\nFrame mesh {{ ObjectMatrixComment {{1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1;;}} {} }}",
            MESH.replace("MeshMaterialList", "XSkinMeshHeader {1;3;1;} SkinWeights {\"bone\";3;0,1,2;1,1,1;1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1;;} DeclData {1;2,0,7,0;3;4294967295,0,1;} FVFData {0;1;0;} MeshMaterialList")
        );
        let scene = parse(source.as_bytes()).unwrap();
        assert_eq!(scene.triangles.len(), 1);
        assert_eq!(scene.triangles[0].color, Some([255, 0, 0]));
        assert!(parse(source.replace("3;0,1,2;1,1,1", "3;0,1,3;1,1,1").as_bytes()).is_err());
    }
}
