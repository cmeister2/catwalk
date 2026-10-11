//! DXF group-pair decoding and static model-space drawing geometry.
use super::mesh::triangulate_polygon;
use super::transform::Transform;
use crate::scene::{Line, Point, Scene, Triangle, Vec3};
use crate::{Error, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

const MAX_ITEMS: usize = 20_000_000;
const SIGNATURE: &[u8] = b"AutoCAD Binary DXF\r\n\x1a\0";
#[path = "dxf_spline.rs"]
mod spline;
fn error(message: impl Into<String>) -> Error {
    message.into().into()
}
#[derive(Clone, Debug)]
struct Pair {
    code: u16,
    value: String,
}
#[derive(Clone, Debug)]
struct Entity {
    kind: String,
    pairs: Vec<Pair>,
    vertices: Vec<Entity>,
}
impl Entity {
    fn value(&self, code: u16) -> Option<&str> {
        self.pairs
            .iter()
            .find(|p| p.code == code)
            .map(|p| p.value.as_str())
    }
    fn number(&self, code: u16, default: f64) -> Result<f64> {
        let n = self
            .value(code)
            .map(str::parse::<f64>)
            .transpose()?
            .unwrap_or(default);
        if !n.is_finite() {
            return Err(error("non-finite DXF geometry value"));
        }
        Ok(n)
    }
    fn integer(&self, code: u16, default: i64) -> Result<i64> {
        Ok(self
            .value(code)
            .map(str::parse::<i64>)
            .transpose()?
            .unwrap_or(default))
    }
    fn vector(&self, code: u16, default: Vec3) -> Result<Vec3> {
        Ok(Vec3::new(
            self.number(code, default.x)?,
            self.number(code + 10, default.y)?,
            self.number(code + 20, default.z)?,
        ))
    }
    fn all(&self, code: u16) -> impl Iterator<Item = &str> {
        self.pairs
            .iter()
            .filter(move |p| p.code == code)
            .map(|p| p.value.as_str())
    }
}
fn string(bytes: &[u8]) -> String {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .unwrap_or_else(|_| bytes.iter().map(|&b| char::from(b)).collect())
}
fn ascii(bytes: &[u8]) -> Result<Vec<Pair>> {
    let source = string(bytes);
    let mut lines = source.lines();
    let mut pairs = Vec::new();
    while let Some(code) = lines.find(|line| !line.trim().is_empty()) {
        let code: u16 = code.trim().parse()?;
        let value = lines
            .next()
            .ok_or_else(|| error("truncated ASCII DXF group pair"))?
            .trim()
            .to_owned();
        pairs.push(Pair { code, value });
        if pairs.len() > MAX_ITEMS {
            return Err(error("too many DXF group pairs"));
        }
    }
    Ok(pairs)
}
struct Binary<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Binary<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(n)
            .ok_or_else(|| error("DXF offset overflow"))?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| error("truncated binary DXF group pair"))?;
        self.offset = end;
        Ok(value)
    }
    fn text(&mut self) -> Result<String> {
        let rest = &self.bytes[self.offset..];
        let n = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| error("unterminated binary DXF string"))?;
        let value = string(self.take(n)?);
        self.take(1)?;
        Ok(value)
    }
}
fn binary(bytes: &[u8], modern: bool) -> Result<Vec<Pair>> {
    let mut reader = Binary {
        bytes,
        offset: SIGNATURE.len(),
    };
    let mut pairs = Vec::new();
    while reader.offset < bytes.len() {
        let code = if modern {
            u16::from_le_bytes(reader.take(2)?.try_into()?)
        } else {
            let n = reader.take(1)?[0];
            if n == 255 {
                u16::from_le_bytes(reader.take(2)?.try_into()?)
            } else {
                n as u16
            }
        };
        let value = match code {
            0..=9
            | 100
            | 102
            | 105
            | 300..=309
            | 320..=369
            | 390..=399
            | 410..=419
            | 430..=439
            | 470..=481
            | 999
            | 1000..=1003
            | 1005..=1009 => reader.text()?,
            10..=59 | 110..=149 | 210..=239 | 460..=469 | 1010..=1059 => {
                f64::from_le_bytes(reader.take(8)?.try_into()?).to_string()
            }
            60..=79 | 170..=179 | 270..=289 | 370..=389 | 400..=409 | 1060..=1070 => {
                i16::from_le_bytes(reader.take(2)?.try_into()?).to_string()
            }
            90..=99 | 420..=429 | 440..=459 | 1071 => {
                i32::from_le_bytes(reader.take(4)?.try_into()?).to_string()
            }
            160..=169 => i64::from_le_bytes(reader.take(8)?.try_into()?).to_string(),
            290..=299 => reader.take(1)?[0].to_string(),
            310..=319 | 1004 => {
                let n = reader.take(1)?[0] as usize;
                reader.take(n)?;
                String::new()
            }
            _ => return Err(error(format!("unsupported binary DXF group code {code}"))),
        };
        pairs.push(Pair { code, value });
        if pairs.len() > MAX_ITEMS {
            return Err(error("too many DXF group pairs"));
        }
    }
    if pairs
        .first()
        .is_none_or(|p| p.code != 0 || p.value != "SECTION")
    {
        return Err(error("binary DXF does not begin with SECTION"));
    }
    Ok(pairs)
}
fn records(pairs: Vec<Pair>) -> Result<Vec<Entity>> {
    let mut records: Vec<Entity> = Vec::new();
    for pair in pairs {
        if pair.code == 999 {
            continue;
        }
        if pair.code == 0 {
            records.push(Entity {
                kind: pair.value.to_ascii_uppercase(),
                pairs: Vec::new(),
                vertices: Vec::new(),
            });
        } else {
            records
                .last_mut()
                .ok_or_else(|| error("DXF group appears before its record"))?
                .pairs
                .push(pair);
        }
    }
    if records.last().is_none_or(|e| e.kind != "EOF") {
        return Err(error("DXF lacks EOF record"));
    }
    Ok(records)
}
#[derive(Clone)]
struct Block {
    base: Vec3,
    flags: i64,
    entities: Vec<Entity>,
}
#[derive(Clone, Copy)]
struct Layer {
    color: Option<[u8; 3]>,
    hidden: bool,
}
fn palette(index: i64) -> Result<Option<[u8; 3]>> {
    Ok(match index {
        0 | 256 => None,
        1 => Some([255, 0, 0]),
        2 => Some([255, 255, 0]),
        3 => Some([0, 255, 0]),
        4 => Some([0, 255, 255]),
        5 => Some([0, 0, 255]),
        6 => Some([255, 0, 255]),
        7 => Some([40, 40, 40]),
        8 => Some([128, 128, 128]),
        9 => Some([192, 192, 192]),
        10..=249 => {
            let n = (index - 10) as usize;
            let hue = (n / 10) as f64 / 4.;
            let saturation = if n.is_multiple_of(2) { 1. } else { 0.5 };
            let brightness = [255., 165., 127., 76., 38.][(n % 10) / 2];
            let chroma = brightness * saturation;
            let x = chroma * (1. - (hue % 2. - 1.).abs());
            let rgb = match hue as usize {
                0 => [chroma, x, 0.],
                1 => [x, chroma, 0.],
                2 => [0., chroma, x],
                3 => [0., x, chroma],
                4 => [x, 0., chroma],
                _ => [chroma, 0., x],
            };
            Some(rgb.map(|n| (n + brightness - chroma).round() as u8))
        }
        250..=255 => {
            Some([[51; 3], [80; 3], [105; 3], [130; 3], [190; 3], [255; 3]][(index - 250) as usize])
        }
        _ => return Err(error("invalid DXF indexed color")),
    })
}
fn direct_color(entity: &Entity) -> Result<Option<[u8; 3]>> {
    if let Some(value) = entity.value(420) {
        let n: u32 = value.parse()?;
        if n > 0x00ff_ffff {
            return Err(error("DXF true color exceeds 24 bits"));
        }
        Ok(Some([(n >> 16) as u8, (n >> 8) as u8, n as u8]))
    } else {
        palette(
            entity
                .integer(62, 256)?
                .checked_abs()
                .ok_or_else(|| error("DXF indexed color overflow"))?,
        )
    }
}
fn collect_entities(records: &[Entity], cursor: &mut usize, end: &str) -> Result<Vec<Entity>> {
    let mut entities = Vec::new();
    while let Some(record) = records.get(*cursor) {
        if record.kind == end {
            *cursor += 1;
            return Ok(entities);
        }
        let mut record = record.clone();
        *cursor += 1;
        if record.kind == "POLYLINE" {
            while let Some(vertex) = records.get(*cursor) {
                *cursor += 1;
                if vertex.kind == "SEQEND" {
                    break;
                }
                if vertex.kind != "VERTEX" {
                    return Err(error("DXF POLYLINE is missing SEQEND"));
                }
                record.vertices.push(vertex.clone());
            }
            if records
                .get(cursor.saturating_sub(1))
                .is_none_or(|r| r.kind != "SEQEND")
            {
                return Err(error("truncated DXF POLYLINE"));
            }
        }
        entities.push(record);
    }
    Err(error(format!("DXF section lacks {end}")))
}
fn ocs(entity: &Entity) -> Result<Transform> {
    let n = entity.vector(210, Vec3::new(0., 0., 1.))?;
    if n.length() == 0. {
        return Err(error("DXF extrusion direction is zero"));
    }
    let n = n.normalized();
    let x = if n.x.abs() < 1. / 64. && n.y.abs() < 1. / 64. {
        Vec3::new(0., 1., 0.).cross(n)
    } else {
        Vec3::new(0., 0., 1.).cross(n)
    }
    .normalized();
    let y = n.cross(x);
    let mut m = Transform::IDENTITY.0;
    for (column, v) in [x, y, n].into_iter().enumerate() {
        m[column][0] = v.x;
        m[column][1] = v.y;
        m[column][2] = v.z;
    }
    Ok(Transform(m))
}
fn translation(v: Vec3) -> Transform {
    let mut m = Transform::IDENTITY.0;
    m[3][0] = v.x;
    m[3][1] = v.y;
    m[3][2] = v.z;
    Transform(m)
}
fn scale(v: Vec3) -> Transform {
    let mut m = Transform::IDENTITY.0;
    m[0][0] = v.x;
    m[1][1] = v.y;
    m[2][2] = v.z;
    Transform(m)
}
fn rotate(angle: f64) -> Transform {
    let (s, c) = angle.to_radians().sin_cos();
    let mut m = Transform::IDENTITY.0;
    m[0][0] = c;
    m[0][1] = s;
    m[1][0] = -s;
    m[1][1] = c;
    Transform(m)
}

struct Importer {
    blocks: HashMap<String, Block>,
    layers: HashMap<String, Layer>,
    active: HashSet<String>,
    scene: Scene,
    visits: usize,
}
impl Importer {
    fn limit(&self) -> Result<()> {
        if self.scene.triangles.len() + self.scene.lines.len() + self.scene.points.len() > MAX_ITEMS
        {
            return Err(error("too much DXF geometry"));
        }
        Ok(())
    }
    fn face(&mut self, vertices: &[Vec3], world: Transform, color: Option<[u8; 3]>) -> Result<()> {
        for vertices in triangulate_polygon(vertices)? {
            let vertices = vertices.map(|p| world.apply(p));
            if vertices.iter().any(|p| !p.is_finite()) {
                return Err(error("DXF transform overflow"));
            }
            self.scene.triangles.push(Triangle { vertices, color });
            self.limit()?;
        }
        Ok(())
    }
    fn stroke(
        &mut self,
        points: &[Vec3],
        world: Transform,
        color: Option<[u8; 3]>,
        extrusion: Vec3,
    ) -> Result<()> {
        for pair in points.windows(2) {
            if extrusion != Vec3::default() {
                self.face(
                    &[pair[0], pair[1], pair[1] + extrusion, pair[0] + extrusion],
                    world,
                    color,
                )?;
            } else {
                let vertices = [world.apply(pair[0]), world.apply(pair[1])];
                if vertices.iter().any(|p| !p.is_finite()) {
                    return Err(error("DXF transform overflow"));
                }
                if vertices[0] != vertices[1] {
                    self.scene.lines.push(Line { vertices, color });
                    self.limit()?;
                }
            }
        }
        Ok(())
    }
    fn walk(
        &mut self,
        entity: &Entity,
        world: Transform,
        inherited_color: Option<[u8; 3]>,
        inherited_layer: &str,
        depth: usize,
    ) -> Result<()> {
        self.visits += 1;
        if depth > 128 || self.visits > MAX_ITEMS {
            return Err(error("DXF block/instance limit exceeded"));
        }
        let own_layer = entity.value(8).unwrap_or("0");
        let layer = if own_layer == "0" {
            inherited_layer
        } else {
            own_layer
        };
        if entity.integer(67, 0)? != 0
            || entity.integer(60, 0)? != 0
            || entity.integer(62, 256)? < 0
            || self.layers.get(layer).is_some_and(|l| l.hidden)
        {
            return Ok(());
        }
        let color = if entity.integer(62, 256)? == 0 && entity.value(420).is_none() {
            inherited_color
        } else {
            direct_color(entity)?
                .or_else(|| self.layers.get(layer).and_then(|l| l.color))
                .or(inherited_color)
        };
        let thickness = entity.number(39, 0.)?;
        match entity.kind.as_str() {
            "SPLINE" => {
                if thickness != 0. {
                    return Err(error(
                        "DXF SPLINE thickness is not defined by the supported schema",
                    ));
                }
                for points in spline::sample(entity)? {
                    // DXF spline control points are WCS coordinates, even when a planar normal exists.
                    self.stroke(&points, world, color, Vec3::default())?;
                }
            }
            "3DFACE" => {
                let a = entity.vector(10, Vec3::default())?;
                let b = entity.vector(11, Vec3::default())?;
                let c = entity.vector(12, Vec3::default())?;
                let d = entity.vector(13, c)?;
                if c == d {
                    self.face(&[a, b, c], world, color)?;
                } else {
                    self.face(&[a, b, c, d], world, color)?;
                }
            }
            "SOLID" | "TRACE" => {
                let a = entity.vector(10, Vec3::default())?;
                let b = entity.vector(11, Vec3::default())?;
                let c = entity.vector(12, Vec3::default())?;
                let d = entity.vector(13, c)?;
                let vertices = if c == d {
                    vec![a, b, c]
                } else {
                    vec![a, b, d, c]
                };
                let world = world.then(ocs(entity)?);
                self.face(&vertices, world, color)?;
                if thickness != 0. {
                    let top: Vec<_> = vertices
                        .iter()
                        .map(|p| *p + Vec3::new(0., 0., thickness))
                        .collect();
                    self.face(&top, world, color)?;
                    for i in 0..vertices.len() {
                        self.face(
                            &[
                                vertices[i],
                                vertices[(i + 1) % vertices.len()],
                                top[(i + 1) % vertices.len()],
                                top[i],
                            ],
                            world,
                            color,
                        )?;
                    }
                }
            }
            "LINE" => {
                let n = entity.vector(210, Vec3::new(0., 0., 1.))?.normalized() * thickness;
                self.stroke(
                    &[
                        entity.vector(10, Vec3::default())?,
                        entity.vector(11, Vec3::default())?,
                    ],
                    world,
                    color,
                    n,
                )?;
            }
            "POINT" => {
                let position = world.apply(entity.vector(10, Vec3::default())?);
                if !position.is_finite() {
                    return Err(error("DXF transform overflow"));
                }
                self.scene.points.push(Point { position, color });
                self.limit()?;
            }
            "CIRCLE" | "ARC" => {
                let center = entity.vector(10, Vec3::default())?;
                let radius = entity.number(40, 0.)?;
                if radius <= 0. {
                    return Err(error("DXF circle radius must be positive"));
                }
                let start = if entity.kind == "CIRCLE" {
                    0.
                } else {
                    entity.number(50, 0.)?
                };
                let sweep = if entity.kind == "CIRCLE" {
                    360.
                } else {
                    let end = entity.number(51, 0.)?;
                    let sweep = (end - start).rem_euclid(360.);
                    if sweep == 0. { 360. } else { sweep }
                };
                let n = ((sweep / 360. * 128.).ceil() as usize).max(1);
                let points: Vec<_> = (0..=n)
                    .map(|i| {
                        let angle = (start + sweep * i as f64 / n as f64).to_radians();
                        center + Vec3::new(radius * angle.cos(), radius * angle.sin(), 0.)
                    })
                    .collect();
                self.stroke(
                    &points,
                    world.then(ocs(entity)?),
                    color,
                    Vec3::new(0., 0., thickness),
                )?;
            }
            "ELLIPSE" => {
                let center = entity.vector(10, Vec3::default())?;
                let major = entity.vector(11, Vec3::default())?;
                let normal = entity.vector(210, Vec3::new(0., 0., 1.))?;
                let ratio = entity.number(40, 1.)?;
                if major.length() == 0. || normal.length() == 0. || ratio <= 0. || ratio > 1. {
                    return Err(error("invalid DXF ellipse axes"));
                }
                let minor = normal.normalized().cross(major) * ratio;
                let start = entity.number(41, 0.)?;
                let end = entity.number(42, std::f64::consts::TAU)?;
                let mut sweep = (end - start).rem_euclid(std::f64::consts::TAU);
                if sweep == 0. {
                    sweep = std::f64::consts::TAU;
                }
                let n = ((sweep / std::f64::consts::TAU * 128.).ceil() as usize).max(1);
                let points: Vec<_> = (0..=n)
                    .map(|i| {
                        let a = start + sweep * i as f64 / n as f64;
                        center + major * a.cos() + minor * a.sin()
                    })
                    .collect();
                self.stroke(&points, world, color, normal.normalized() * thickness)?;
            }
            "LWPOLYLINE" => {
                let elevation = entity.number(38, 0.)?;
                let mut vertices = Vec::new();
                let mut bulges = Vec::new();
                for pair in &entity.pairs {
                    match pair.code {
                        10 => {
                            let x: f64 = pair.value.parse()?;
                            vertices.push(Vec3::new(x, 0., elevation));
                            bulges.push(0.);
                        }
                        20 => {
                            let vertex = vertices
                                .last_mut()
                                .ok_or_else(|| error("DXF polyline Y appears before X"))?;
                            vertex.y = pair.value.parse()?;
                        }
                        42 => {
                            *bulges
                                .last_mut()
                                .ok_or_else(|| error("DXF bulge appears before a vertex"))? =
                                pair.value.parse()?
                        }
                        40 | 41 if pair.value.parse::<f64>()? != 0. => {
                            return Err(error("DXF polyline widths are not supported"));
                        }
                        _ => {}
                    }
                }
                if entity.integer(90, vertices.len() as i64)? != vertices.len() as i64 {
                    return Err(error("DXF LWPOLYLINE count differs from its vertices"));
                }
                if entity.number(43, 0.)? != 0. {
                    return Err(error("DXF polyline widths are not supported"));
                }
                self.polyline(
                    &vertices,
                    &bulges,
                    entity.integer(70, 0)? & 1 != 0,
                    world.then(ocs(entity)?),
                    color,
                    Vec3::new(0., 0., thickness),
                )?;
            }
            "POLYLINE" => {
                let flags = entity.integer(70, 0)?;
                if flags & 6 != 0 || entity.integer(75, 0)? != 0 {
                    return Err(error(
                        "DXF fitted polygon meshes/polylines are not supported",
                    ));
                }
                if flags & 64 != 0 {
                    let mut positions = Vec::new();
                    for vertex in &entity.vertices {
                        if vertex.integer(70, 0)? & 64 != 0 {
                            positions.push(vertex.vector(10, Vec3::default())?);
                        }
                    }
                    for face in &entity.vertices {
                        if face.integer(70, 0)? & 128 != 0 && face.integer(70, 0)? & 64 == 0 {
                            let mut polygon = Vec::new();
                            for code in 71..=74 {
                                let index = face.integer(code, 0)?;
                                if index == 0 {
                                    break;
                                }
                                let index = index
                                    .checked_abs()
                                    .ok_or_else(|| error("DXF polyface index overflow"))?
                                    as usize;
                                polygon.push(*positions.get(index - 1).ok_or_else(|| {
                                    error("DXF polyface vertex index out of bounds")
                                })?);
                            }
                            self.face(&polygon, world, color)?;
                        }
                    }
                } else if flags & 16 != 0 {
                    let m = usize::try_from(entity.integer(71, 0)?)
                        .map_err(|_| error("invalid DXF mesh dimension"))?;
                    let n = usize::try_from(entity.integer(72, 0)?)
                        .map_err(|_| error("invalid DXF mesh dimension"))?;
                    if m < 2 || n < 2 || m.checked_mul(n) != Some(entity.vertices.len()) {
                        return Err(error("DXF polygon mesh dimensions differ from vertices"));
                    }
                    let positions = entity
                        .vertices
                        .iter()
                        .map(|v| v.vector(10, Vec3::default()))
                        .collect::<Result<Vec<_>>>()?;
                    let rows = if flags & 1 != 0 { m } else { m - 1 };
                    let cols = if flags & 32 != 0 { n } else { n - 1 };
                    for i in 0..rows {
                        for j in 0..cols {
                            self.face(
                                &[
                                    positions[i * n + j],
                                    positions[((i + 1) % m) * n + j],
                                    positions[((i + 1) % m) * n + (j + 1) % n],
                                    positions[i * n + (j + 1) % n],
                                ],
                                world,
                                color,
                            )?;
                        }
                    }
                } else {
                    let spatial = flags & 8 != 0;
                    let elevation = entity.number(30, 0.)?;
                    let positions = entity
                        .vertices
                        .iter()
                        .map(|v| {
                            let mut p = v.vector(10, Vec3::default())?;
                            if !spatial {
                                p.z = elevation;
                            }
                            Ok(p)
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let bulges = entity
                        .vertices
                        .iter()
                        .map(|v| v.number(42, 0.))
                        .collect::<Result<Vec<_>>>()?;
                    if entity.number(40, 0.)? != 0. || entity.number(41, 0.)? != 0. {
                        return Err(error("DXF polyline widths are not supported"));
                    }
                    let world = if spatial {
                        world
                    } else {
                        world.then(ocs(entity)?)
                    };
                    let extrusion = if spatial {
                        entity.vector(210, Vec3::new(0., 0., 1.))?.normalized() * thickness
                    } else {
                        Vec3::new(0., 0., thickness)
                    };
                    self.polyline(&positions, &bulges, flags & 1 != 0, world, color, extrusion)?;
                }
            }
            "MESH" => {
                if entity.integer(71, 2)? != 2 {
                    return Err(error("unsupported DXF MESH version"));
                }
                if !(0..=1).contains(&entity.integer(72, 0)?) {
                    return Err(error("invalid DXF MESH blend-crease flag"));
                }
                if entity.integer(91, 0)? != 0 {
                    return Err(error("DXF subdivided MESH requires subdivision evaluation"));
                }
                let mut positions = Vec::new();
                for pair in &entity.pairs {
                    match pair.code {
                        10 => positions.push(Vec3::new(pair.value.parse()?, 0., 0.)),
                        20 => {
                            positions
                                .last_mut()
                                .ok_or_else(|| error("DXF mesh Y appears before X"))?
                                .y = pair.value.parse()?
                        }
                        30 => {
                            positions
                                .last_mut()
                                .ok_or_else(|| error("DXF mesh Z appears before X"))?
                                .z = pair.value.parse()?
                        }
                        _ => {}
                    }
                }
                if entity.integer(92, positions.len() as i64)? != positions.len() as i64 {
                    return Err(error("DXF MESH vertex count differs from data"));
                }
                let data = entity
                    .all(90)
                    .map(str::parse::<usize>)
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let face_size = usize::try_from(entity.integer(93, -1)?)?;
                let faces = data
                    .get(..face_size)
                    .ok_or_else(|| error("truncated DXF MESH face list"))?;
                let edge_count = usize::try_from(entity.integer(94, 0)?)?;
                let edge_end = edge_count
                    .checked_mul(2)
                    .and_then(|n| face_size.checked_add(n))
                    .ok_or_else(|| error("DXF MESH edge-list overflow"))?;
                let edges = data
                    .get(face_size..edge_end)
                    .ok_or_else(|| error("truncated DXF MESH edge list"))?;
                if edges.iter().any(|&i| i >= positions.len()) {
                    return Err(error("DXF MESH edge index out of bounds"));
                }
                let remainder = &data[edge_end..];
                if !remainder.is_empty() && remainder != [0] {
                    return Err(error(
                        "DXF MESH sub-entity property overrides are not supported",
                    ));
                }
                let crease_count = usize::try_from(entity.integer(95, 0)?)?;
                if crease_count > edge_count || entity.all(140).count() != crease_count {
                    return Err(error("DXF MESH crease count differs from edge data"));
                }
                for value in entity.all(140) {
                    if !value.parse::<f64>()?.is_finite() {
                        return Err(error("non-finite DXF MESH crease value"));
                    }
                }
                let mut cursor = 0;
                while cursor < faces.len() {
                    let n = faces[cursor];
                    cursor += 1;
                    let end = cursor
                        .checked_add(n)
                        .ok_or_else(|| error("DXF face offset overflow"))?;
                    let indices = faces
                        .get(cursor..end)
                        .ok_or_else(|| error("truncated DXF MESH face"))?;
                    let polygon = indices
                        .iter()
                        .map(|&i| {
                            positions
                                .get(i)
                                .copied()
                                .ok_or_else(|| error("DXF MESH index out of bounds"))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    self.face(&polygon, world, color)?;
                    cursor = end;
                }
            }
            "INSERT" => {
                let name = entity
                    .value(2)
                    .ok_or_else(|| error("DXF INSERT lacks block name"))?;
                let block = self
                    .blocks
                    .get(name)
                    .cloned()
                    .ok_or_else(|| error("DXF INSERT references a missing block"))?;
                if block.flags & 28 != 0 {
                    return Err(error("DXF external reference blocks are not supported"));
                }
                if !self.active.insert(name.to_owned()) {
                    return Err(error("cyclic DXF block INSERT"));
                }
                let cols = entity.integer(70, 1)?;
                let rows = entity.integer(71, 1)?;
                if cols < 1 || rows < 1 || cols.checked_mul(rows).is_none_or(|n| n > 100_000) {
                    return Err(error("DXF INSERT array exceeds limits"));
                }
                let base = world
                    .then(ocs(entity)?)
                    .then(translation(entity.vector(10, Vec3::default())?))
                    .then(rotate(entity.number(50, 0.)?));
                let scale = scale(Vec3::new(
                    entity.number(41, 1.)?,
                    entity.number(42, 1.)?,
                    entity.number(43, 1.)?,
                ));
                for row in 0..rows {
                    for col in 0..cols {
                        let transform = base
                            .then(translation(Vec3::new(
                                col as f64 * entity.number(44, 0.)?,
                                row as f64 * entity.number(45, 0.)?,
                                0.,
                            )))
                            .then(scale)
                            .then(translation(-block.base));
                        for child in &block.entities {
                            self.walk(child, transform, color, layer, depth + 1)?;
                        }
                    }
                }
                self.active.remove(name);
            }
            "SEQEND"
            | "TEXT"
            | "MTEXT"
            | "ATTRIB"
            | "ATTDEF"
            | "DIMENSION"
            | "ARC_DIMENSION"
            | "LARGE_RADIAL_DIMENSION"
            | "LEADER"
            | "MULTILEADER"
            | "TOLERANCE"
            | "ACAD_TABLE"
            | "VIEWPORT" => {
                // Drafting labels, measurement/leader annotations and viewport definitions
                // do not participate in the model-geometry thumbnail or its framing.
            }
            kind => {
                return Err(error(format!(
                    "unsupported DXF entity {kind}; geometry cannot be omitted"
                )));
            }
        }
        Ok(())
    }
    fn polyline(
        &mut self,
        vertices: &[Vec3],
        bulges: &[f64],
        closed: bool,
        world: Transform,
        color: Option<[u8; 3]>,
        extrusion: Vec3,
    ) -> Result<()> {
        if vertices.iter().any(|p| !p.is_finite()) || bulges.iter().any(|b| !b.is_finite()) {
            return Err(error("non-finite DXF polyline vertex"));
        }
        if vertices.len() < 2 {
            return Err(error("DXF polyline needs at least two vertices"));
        }
        let segments = if closed {
            vertices.len()
        } else {
            vertices.len() - 1
        };
        for i in 0..segments {
            let (a, b) = (vertices[i], vertices[(i + 1) % vertices.len()]);
            let bulge = bulges[i];
            if bulge == 0. {
                self.stroke(&[a, b], world, color, extrusion)?;
            } else {
                if (a.z - b.z).abs() > 1e-12 {
                    return Err(error("DXF bulge requires planar XY vertices"));
                }
                let chord = b - a;
                let length = chord.length();
                if length == 0. {
                    return Err(error("DXF bulged segment has zero length"));
                }
                let theta = 4. * bulge.atan();
                let radius = length * (1. + bulge * bulge) / (4. * bulge.abs());
                let midpoint = (a + b) / 2.;
                let center = midpoint
                    + Vec3::new(-chord.y, chord.x, 0.) * ((1. - bulge * bulge) / (4. * bulge));
                let start = (a.y - center.y).atan2(a.x - center.x);
                let n = ((theta.abs() / std::f64::consts::TAU * 128.).ceil() as usize).max(1);
                let points: Vec<_> = (0..=n)
                    .map(|k| {
                        let t = start + theta * k as f64 / n as f64;
                        Vec3::new(
                            center.x + radius * t.cos(),
                            center.y + radius * t.sin(),
                            a.z,
                        )
                    })
                    .collect();
                self.stroke(&points, world, color, extrusion)?;
            }
        }
        Ok(())
    }
}

pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    parse(&bytes)
}
fn parse(bytes: &[u8]) -> Result<Scene> {
    let pairs = if bytes.starts_with(SIGNATURE) {
        binary(bytes, true).or_else(|_| binary(bytes, false))?
    } else {
        ascii(bytes)?
    };
    let records = records(pairs)?;
    let mut blocks = HashMap::new();
    let mut layers = HashMap::new();
    let mut entities = Vec::new();
    let mut cursor = 0;
    while let Some(record) = records.get(cursor) {
        cursor += 1;
        match record.kind.as_str() {
            "SECTION" => match record.value(2).unwrap_or("") {
                "ENTITIES" => entities.extend(collect_entities(&records, &mut cursor, "ENDSEC")?),
                "BLOCKS" => {
                    while let Some(block) = records.get(cursor) {
                        cursor += 1;
                        if block.kind == "ENDSEC" {
                            break;
                        }
                        if block.kind != "BLOCK" {
                            return Err(error("unexpected record in DXF BLOCKS"));
                        }
                        let name = block
                            .value(2)
                            .ok_or_else(|| error("DXF BLOCK lacks name"))?
                            .to_owned();
                        let entities = collect_entities(&records, &mut cursor, "ENDBLK")?;
                        let value = Block {
                            base: block.vector(10, Vec3::default())?,
                            flags: block.integer(70, 0)?,
                            entities,
                        };
                        if blocks.insert(name, value).is_some() {
                            return Err(error("duplicate DXF block name"));
                        }
                    }
                }
                "TABLES" => {
                    while let Some(table) = records.get(cursor) {
                        cursor += 1;
                        if table.kind == "ENDSEC" {
                            break;
                        }
                        if table.kind == "LAYER" {
                            let name = table
                                .value(2)
                                .ok_or_else(|| error("DXF LAYER lacks name"))?
                                .to_owned();
                            layers.insert(
                                name,
                                Layer {
                                    color: direct_color(table)?,
                                    hidden: table.integer(62, 7)? < 0
                                        || table.integer(70, 0)? & 1 != 0,
                                },
                            );
                        }
                    }
                }
                _ => {
                    while let Some(record) = records.get(cursor) {
                        cursor += 1;
                        if record.kind == "ENDSEC" {
                            break;
                        }
                    }
                }
            },
            "EOF" => break,
            _ => return Err(error("DXF record outside a SECTION")),
        }
    }
    let mut importer = Importer {
        blocks,
        layers,
        active: HashSet::new(),
        scene: Scene::default(),
        visits: 0,
    };
    for entity in entities {
        importer.walk(&entity, Transform::IDENTITY, None, "0", 0)?;
    }
    importer.scene.validate()?;
    Ok(importer.scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn drawing(entities: &str) -> Vec<u8> {
        format!("0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n").into_bytes()
    }
    #[test]
    fn faces_and_lines() {
        let scene=parse(&drawing("0\n3DFACE\n10\n0\n20\n0\n30\n0\n11\n1\n21\n0\n31\n0\n12\n0\n22\n1\n32\n0\n13\n0\n23\n1\n33\n0\n420\n16711680\n0\nLINE\n10\n0\n11\n2\n21\n3\n")).unwrap();
        assert_eq!(scene.triangles.len(), 1);
        assert_eq!(scene.lines.len(), 1);
        assert_eq!(scene.triangles[0].color, Some([255, 0, 0]));
        assert_eq!(scene.lines[0].vertices[1], Vec3::new(2., 3., 0.));
    }
    #[test]
    fn bulges_trace_semicircle() {
        let scene = parse(&drawing(
            "0\nLWPOLYLINE\n90\n2\n10\n-1\n20\n0\n42\n1\n10\n1\n20\n0\n",
        ))
        .unwrap();
        assert_eq!(scene.lines.len(), 64);
        assert!(scene.positions().all(|p| (p.length() - 1.).abs() < 1e-12));
        assert!(scene.positions().any(|p| p.y < -0.99));
    }
    #[test]
    fn nested_block_base_scale_rotation() {
        let source = "0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n2\ntriangle\n10\n1\n0\nLINE\n10\n1\n11\n2\n0\nENDBLK\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nINSERT\n2\ntriangle\n10\n10\n20\n20\n41\n2\n42\n2\n50\n90\n0\nENDSEC\n0\nEOF\n";
        let scene = parse(source.as_bytes()).unwrap();
        assert_eq!(scene.lines[0].vertices[0], Vec3::new(10., 20., 0.));
        let end = scene.lines[0].vertices[1];
        assert!((end.x - 10.).abs() < 1e-12 && (end.y - 22.).abs() < 1e-12);
    }
    #[test]
    fn binary_group_pairs_match_ascii() {
        let ascii = drawing("0\nLINE\n10\n1\n20\n2\n30\n3\n11\n4\n21\n5\n31\n6\n");
        let pairs = super::ascii(&ascii).unwrap();
        for modern in [true, false] {
            let mut data = SIGNATURE.to_vec();
            for pair in &pairs {
                if modern {
                    data.extend_from_slice(&pair.code.to_le_bytes());
                } else {
                    data.push(pair.code as u8);
                }
                if pair.code == 0 || pair.code == 2 {
                    data.extend_from_slice(pair.value.as_bytes());
                    data.push(0);
                } else {
                    data.extend_from_slice(&pair.value.parse::<f64>().unwrap().to_le_bytes());
                }
            }
            let scene = parse(&data).unwrap();
            assert_eq!(scene.lines[0].vertices[1], Vec3::new(4., 5., 6.));
            assert!(parse(&data[..data.len() - 1]).is_err());
        }
    }
    #[test]
    fn rejects_unsupported_and_cyclic_geometry() {
        assert!(parse(&drawing("0\n3DSOLID\n")).is_err());
        assert!(parse(&drawing("0\nSPLINE\n")).is_err());
        assert!(parse(b"0\nSECTION\n2\nENTITIES\n0\nLINE\n10\n").is_err());
        let source = "0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n2\nloop\n0\nINSERT\n2\nloop\n0\nENDBLK\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nINSERT\n2\nloop\n0\nENDSEC\n0\nEOF\n";
        assert!(parse(source.as_bytes()).is_err());
    }
    #[test]
    fn modern_mesh_edge_records_and_subdivision() {
        let entity = "0\nMESH\n71\n2\n72\n1\n91\n0\n92\n3\n10\n0\n20\n0\n30\n0\n10\n1\n20\n0\n30\n0\n10\n0\n20\n1\n30\n0\n93\n4\n90\n3\n90\n0\n90\n1\n90\n2\n94\n1\n90\n0\n90\n1\n95\n1\n140\n0.5\n90\n0\n";
        let scene = parse(&drawing(entity)).unwrap();
        assert_eq!(scene.triangles.len(), 1);
        assert!(parse(&drawing(&entity.replace("91\n0", "91\n1"))).is_err());
        assert!(parse(&drawing(&entity.replace("93\n4", "93\n5"))).is_err());
        assert!(parse(&drawing("0\nPOINT\n62\n-9223372036854775808\n")).is_err());
    }
}
