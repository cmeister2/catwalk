//! Mesh and point formats with no native library dependencies.
use std::collections::HashMap;
use std::path::Path;

use crate::scene::{Line, Point, Scene, Triangle, Vec3};
use crate::{Error, Result};

// Limit declared counts before allocating. This is deliberately much larger than
// a useful thumbnail, but bounds corrupt-file allocations.
/// Maximum geometry or attribute item count accepted by this importer.
const MAX_ITEMS: usize = 20_000_000;
/// Maximum vertices permitted in one polygon before triangulation.
const MAX_FACE_VERTICES: usize = 100_000;

/// Read a bounded mesh or point-cloud file and decode its preview geometry.
///
/// # Errors
/// Returns an error for unreadable input, unsupported/malformed geometry, invalid indices,
/// nonfinite coordinates or importer size limits.
pub fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let scene = match ext.as_str() {
        "stl" => stl(&bytes),
        "obj" => obj(
            text(&bytes)?,
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        ),
        "ply" => ply(&bytes),
        "off" => off(text(&bytes)?),
        "pts" => pts(text(&bytes)?),
        "ptx" => ptx(text(&bytes)?),
        "pcd" => pcd(&bytes),
        "splat" => splat(&bytes),
        _ => Err(format!("unsupported mesh format: {ext}").into()),
    }?;
    scene.validate()?;
    Ok(scene)
}

/// Decode a mesh text payload as UTF-8.
fn text(bytes: &[u8]) -> Result<&str> {
    Ok(std::str::from_utf8(bytes)?)
}
/// Convert a mesh parser diagnostic into the shared error type.
fn err(message: impl Into<String>) -> Error {
    message.into().into()
}
/// Parse and reject nonfinite mesh coordinates or material values.
fn finite(s: &str) -> Result<f64> {
    let n: f64 = s.parse()?;
    if !n.is_finite() {
        return Err(err("non-finite coordinate"));
    }
    Ok(n)
}
/// Parse a nonnegative mesh record count within the item limit.
fn count(s: &str) -> Result<usize> {
    let n: usize = s.parse()?;
    if n > MAX_ITEMS {
        return Err(err("declared item count exceeds 20 million"));
    }
    Ok(n)
}
/// Require three coordinate fields and decode one finite vertex.
fn vertex(values: &[&str]) -> Result<Vec3> {
    if values.len() < 3 {
        return Err(err("expected three coordinates"));
    }
    Ok(Vec3::new(
        finite(values[0])?,
        finite(values[1])?,
        finite(values[2])?,
    ))
}
/// Convert declared normalized or byte-range color channels to rounded RGB.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Channels are finite, clamped to the byte range and intentionally rounded before quantization."
)]
fn rgb(values: &[&str], normalized: bool) -> Result<[u8; 3]> {
    if values.len() < 3 {
        return Err(err("expected three color channels"));
    }
    let mut color = [0; 3];
    for (out, value) in color.iter_mut().zip(values) {
        let n = finite(value)?;
        let n = if normalized { n * 255.0 } else { n };
        *out = n.clamp(0.0, 255.0).round() as u8;
    }
    Ok(color)
}

/// Triangulate a simple planar polygon by projecting onto its dominant plane.
/// Convex fans alone are incorrect for common concave OBJ/OFF/PLY faces.
///
/// # Errors
/// Returns an error for too few or too many vertices, nonfinite coordinates,
/// degenerate/self-intersecting boundaries or failed triangulation.
#[allow(
    clippy::too_many_lines,
    reason = "Keep polygon validation, dominant-plane projection and triangulation consistency checks together."
)]
pub(crate) fn triangulate_polygon(vertices: &[Vec3]) -> Result<Vec<[Vec3; 3]>> {
    if vertices.len() < 3 {
        return Err(err("face has fewer than three vertices"));
    }
    if vertices.len() > MAX_FACE_VERTICES || vertices.iter().any(|p| !p.is_finite()) {
        return Err(err("invalid polygon vertices"));
    }
    let mut ids = Vec::with_capacity(vertices.len());
    for (i, p) in vertices.iter().enumerate() {
        if ids.last().is_none_or(|&j| vertices[j] != *p) {
            ids.push(i);
        }
    }
    if ids.len() > 1 && vertices[ids[0]] == vertices[*ids.last().unwrap()] {
        ids.pop();
    }
    if ids.len() < 3 {
        return Ok(Vec::new());
    }
    if ids.len() == 3 {
        return Ok(vec![[vertices[ids[0]], vertices[ids[1]], vertices[ids[2]]]]);
    }
    let mut normal = Vec3::default();
    // Shift first to avoid cancellation in models far from the origin.
    let origin = vertices[ids[0]];
    for k in 0..ids.len() {
        normal =
            normal + (vertices[ids[k]] - origin).cross(vertices[ids[(k + 1) % ids.len()]] - origin);
    }
    let axis = if normal.x.abs() >= normal.y.abs() && normal.x.abs() >= normal.z.abs() {
        0
    } else if normal.y.abs() >= normal.z.abs() {
        1
    } else {
        2
    };
    let project = |p: Vec3| -> (f64, f64) {
        let p = p - origin;
        match axis {
            0 => (p.y, p.z),
            1 => (p.x, p.z),
            _ => (p.x, p.y),
        }
    };
    let xy: Vec<_> = vertices.iter().copied().map(project).collect();
    let cross = |a: usize, b: usize, c: usize| -> f64 {
        (xy[b].0 - xy[a].0) * (xy[c].1 - xy[a].1) - (xy[b].1 - xy[a].1) * (xy[c].0 - xy[a].0)
    };
    let area: f64 = (0..ids.len())
        .map(|i| {
            let (a, b) = (xy[ids[i]], xy[ids[(i + 1) % ids.len()]]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum();
    if !area.is_finite() {
        return Err(err("polygon coordinates exceed numeric range"));
    }
    if area == 0.0 {
        return Ok(Vec::new());
    }
    let sign = area.signum();
    let scale = xy
        .iter()
        .map(|&(x, y)| x.abs().max(y.abs()))
        .fold(0.0_f64, f64::max);
    let epsilon = scale * scale * 1e-12;
    // Most imported faces are convex; avoid quadratic ear searches for them.
    if (0..ids.len())
        .all(|k| cross(ids[k], ids[(k + 1) % ids.len()], ids[(k + 2) % ids.len()]) * sign > epsilon)
    {
        return Ok((1..ids.len() - 1)
            .map(|k| [vertices[ids[0]], vertices[ids[k]], vertices[ids[k + 1]]])
            .collect());
    }
    let mut triangles = Vec::with_capacity(ids.len() - 2);
    while ids.len() > 3 {
        let mut ear = None;
        let mut collinear = None;
        for k in 0..ids.len() {
            let a = ids[(k + ids.len() - 1) % ids.len()];
            let b = ids[k];
            let c = ids[(k + 1) % ids.len()];
            let turn = cross(a, b, c) * sign;
            if turn.abs() <= epsilon {
                collinear = Some(k);
                continue;
            }
            if turn < 0.0 {
                continue;
            }
            let contains = ids.iter().any(|&p| {
                p != a
                    && p != b
                    && p != c
                    && cross(a, b, p) * sign >= -epsilon
                    && cross(b, c, p) * sign >= -epsilon
                    && cross(c, a, p) * sign >= -epsilon
            });
            if !contains {
                ear = Some((k, [vertices[a], vertices[b], vertices[c]]));
                break;
            }
        }
        if let Some((k, triangle)) = ear {
            triangles.push(triangle);
            ids.remove(k);
        } else if let Some(k) = collinear {
            ids.remove(k);
        } else {
            return Err(err(
                "cannot triangulate face: polygon is self-intersecting or invalid",
            ));
        }
    }
    if ids.len() == 3 {
        triangles.push([vertices[ids[0]], vertices[ids[1]], vertices[ids[2]]]);
    }
    Ok(triangles)
}

/// Resolve polygon indices, triangulate the face and append it within the item budget.
fn add_face(
    scene: &mut Scene,
    indices: &[usize],
    vertices: &[Vec3],
    color: Option<[u8; 3]>,
) -> Result<()> {
    let polygon = indices
        .iter()
        .map(|&i| {
            vertices
                .get(i)
                .copied()
                .ok_or_else(|| err("face vertex index out of bounds"))
        })
        .collect::<Result<Vec<_>>>()?;
    for vertices in triangulate_polygon(&polygon)? {
        scene.triangles.push(Triangle { vertices, color });
    }
    Ok(())
}

/// Decode an exact-length binary STL or structurally validated ASCII facets.
fn stl(bytes: &[u8]) -> Result<Scene> {
    // Binary headers may begin with "solid"; the count and record length take
    // precedence over that unreliable discriminator.
    if bytes.len() >= 84 {
        let n = u32::from_le_bytes(bytes[80..84].try_into()?) as usize;
        if n.checked_mul(50).and_then(|n| n.checked_add(84)) == Some(bytes.len()) {
            if n > MAX_ITEMS {
                return Err(err("too many STL triangles"));
            }
            let mut scene = Scene::default();
            for record in bytes[84..].as_chunks::<50>().0 {
                let mut vertices = [Vec3::default(); 3];
                for (i, p) in vertices.iter_mut().enumerate() {
                    let offset = 12 + i * 12;
                    let x = f32::from_le_bytes(record[offset..offset + 4].try_into()?);
                    let y = f32::from_le_bytes(record[offset + 4..offset + 8].try_into()?);
                    let z = f32::from_le_bytes(record[offset + 8..offset + 12].try_into()?);
                    *p = Vec3::from([x, y, z]);
                    if !p.is_finite() {
                        return Err(err("non-finite STL vertex"));
                    }
                }
                scene.triangles.push(Triangle {
                    vertices,
                    color: None,
                });
            }
            return Ok(scene);
        }
    }
    let source = text(bytes).map_err(|_| err("invalid or truncated binary STL"))?;
    let mut scene = Scene::default();
    let mut current = Vec::new();
    let mut in_facet = false;
    let mut saw_solid = false;
    for line in source.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        match words.first().copied() {
            Some("solid") => saw_solid = true,
            Some("facet") => {
                if in_facet {
                    return Err(err("nested STL facets"));
                }
                in_facet = true;
                current.clear();
            }
            Some("vertex") => {
                if !in_facet || current.len() >= 3 {
                    return Err(err("STL vertex outside a valid facet"));
                }
                current.push(vertex(&words[1..])?);
            }
            Some("endfacet") => {
                if !in_facet || current.len() != 3 {
                    return Err(err("STL facet requires three vertices"));
                }
                scene.triangles.push(Triangle {
                    vertices: [current[0], current[1], current[2]],
                    color: None,
                });
                in_facet = false;
            }
            _ => {}
        }
    }
    if !saw_solid || in_facet {
        return Err(err("invalid or truncated ASCII STL"));
    }
    Ok(scene)
}

/// Resolve positive or relative-negative OBJ vertex references with optional slash fields.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "The caller bounds vertex counts to MAX_ITEMS; signed OBJ references are range checked before native indexing."
)]
fn obj_vertex_index(word: &str, vertex_count: usize) -> Result<usize> {
    let index: i64 = word.split('/').next().unwrap_or("").parse()?;
    let index = match index.cmp(&0) {
        std::cmp::Ordering::Greater => index - 1,
        std::cmp::Ordering::Less => vertex_count as i64 + index,
        std::cmp::Ordering::Equal => return Err(err("OBJ indices are one-based; zero is invalid")),
    };
    if index < 0 || index as usize >= vertex_count {
        return Err(err("OBJ index out of bounds"));
    }
    Ok(index as usize)
}

/// Average available indexed vertex RGB values without allocating an intermediate palette.
#[allow(
    clippy::cast_possible_truncation,
    reason = "A mean of available byte channels is guaranteed to remain within the byte range."
)]
fn indexed_color(indices: &[usize], colors: &[Option<[u8; 3]>]) -> Option<[u8; 3]> {
    let mut sum = [0_u64; 3];
    let mut count = 0_u64;
    for color in indices
        .iter()
        .filter_map(|&i| colors.get(i).copied().flatten())
    {
        count += 1;
        for (channel, value) in color.into_iter().enumerate() {
            sum[channel] += u64::from(value);
        }
    }
    (count != 0).then(|| sum.map(|value| (value / count) as u8))
}

/// Decode OBJ polygons, polylines, points, vertex colors and adjacent MTL diffuse colors.
#[allow(
    clippy::too_many_lines,
    reason = "Keep OBJ directives together because they share evolving vertex, material and index state."
)]
fn obj(source: &str, directory: &Path) -> Result<Scene> {
    let mut vertices = Vec::new();
    let mut colors = Vec::new();
    let mut materials = HashMap::new();
    let mut material = None;
    let mut scene = Scene::default();
    // OBJ continuations are allowed; build logical lines before tokenizing.
    let source = source.replace("\\\r\n", " ").replace("\\\n", " ");
    for (line_no, line) in source.lines().enumerate() {
        let words: Vec<_> = line
            .split('#')
            .next()
            .unwrap_or("")
            .split_whitespace()
            .collect();
        let result = (|| -> Result<()> {
            match words.first().copied() {
                Some("v") => {
                    let mut p = vertex(&words[1..])?;
                    if words.len() == 5 || words.len() == 8 {
                        let w = finite(words[4])?;
                        if w == 0.0 {
                            return Err(err("OBJ homogeneous vertex has zero weight"));
                        }
                        p = p / w;
                    }
                    vertices.push(p);
                    let color = if words.len() == 7 {
                        Some(rgb(&words[4..7], true)?)
                    } else if words.len() >= 8 {
                        Some(rgb(&words[5..8], true)?)
                    } else {
                        None
                    };
                    colors.push(color);
                }
                Some("f") => {
                    let indices = words[1..]
                        .iter()
                        .map(|w| obj_vertex_index(w, vertices.len()))
                        .collect::<Result<Vec<_>>>()?;
                    let color = material
                        .as_ref()
                        .and_then(|name| materials.get(name))
                        .copied()
                        .or_else(|| indexed_color(&indices, &colors));
                    add_face(&mut scene, &indices, &vertices, color)?;
                }
                Some("l") => {
                    if words.len() < 3 {
                        return Err(err("OBJ polyline requires at least two vertices"));
                    }
                    let indices = words[1..]
                        .iter()
                        .map(|word| obj_vertex_index(word, vertices.len()))
                        .collect::<Result<Vec<_>>>()?;
                    if scene.lines.len().saturating_add(indices.len() - 1) > MAX_ITEMS {
                        return Err(err("too much OBJ line geometry"));
                    }
                    let material_color = material
                        .as_ref()
                        .and_then(|name| materials.get(name))
                        .copied();
                    scene.lines.extend(indices.windows(2).map(|edge| Line {
                        vertices: [vertices[edge[0]], vertices[edge[1]]],
                        color: material_color.or_else(|| indexed_color(edge, &colors)),
                    }));
                }
                Some("p") => {
                    if words.len() < 2 {
                        return Err(err("OBJ point element requires a vertex"));
                    }
                    for word in &words[1..] {
                        let index = obj_vertex_index(word, vertices.len())?;
                        scene.points.push(Point {
                            position: vertices[index],
                            color: colors[index],
                        });
                    }
                }
                Some("curv" | "curv2" | "surf") => {
                    return Err(err("OBJ free-form curves and surfaces are not implemented"));
                }
                Some("usemtl") => material = words.get(1).map(std::string::ToString::to_string),
                Some("mtllib") => {
                    for name in &words[1..] {
                        // A missing optional appearance file should not hide geometry.
                        if let Ok(source) = super::relative_resource(directory, name)
                            .and_then(|path| super::read_file(&path))
                            .and_then(|bytes| String::from_utf8(bytes).map_err(Error::from))
                        {
                            let mut name = None;
                            for line in source.lines() {
                                let words: Vec<_> = line
                                    .split('#')
                                    .next()
                                    .unwrap_or("")
                                    .split_whitespace()
                                    .collect();
                                if words.first() == Some(&"newmtl") {
                                    name = words.get(1).map(std::string::ToString::to_string);
                                } else if words.first() == Some(&"Kd")
                                    && words.len() >= 4
                                    && let (Some(name), Ok(color)) =
                                        (name.as_ref(), rgb(&words[1..4], true))
                                {
                                    materials.insert(name.clone(), color);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            if vertices.len() > MAX_ITEMS
                || scene.triangles.len() > MAX_ITEMS
                || scene.points.len() > MAX_ITEMS
            {
                return Err(err("too much OBJ geometry"));
            }
            Ok(())
        })();
        result.map_err(|e| err(format!("OBJ line {}: {e}", line_no + 1)))?;
    }
    if scene.triangles.is_empty() && scene.points.is_empty() && scene.lines.is_empty() {
        scene.points = vertices
            .into_iter()
            .zip(colors)
            .map(|(position, color)| Point { position, color })
            .collect();
    }
    Ok(scene)
}

/// Decode OFF or COFF polygon meshes with checked counts and optional vertex/face colors.
#[allow(
    clippy::cast_possible_truncation,
    reason = "A mean of byte vertex colors remains within the byte range before conversion."
)]
#[allow(
    clippy::too_many_lines,
    reason = "Keep OFF/COFF header, vertex and face-count validation in one record traversal."
)]
fn off(source: &str) -> Result<Scene> {
    let lines: Vec<Vec<&str>> = source
        .lines()
        .map(|line| {
            line.split('#')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .collect()
        })
        .filter(|line: &Vec<_>| !line.is_empty())
        .collect();
    let first = lines.first().ok_or_else(|| err("empty OFF file"))?;
    let tag = first[0];
    if !matches!(tag, "OFF" | "COFF" | "NOFF" | "CNOFF" | "STOFF") {
        return Err(err("unsupported OFF header"));
    }
    let (counts, mut cursor) = if first.len() >= 4 {
        (&first[1..], 1)
    } else {
        (
            lines
                .get(1)
                .ok_or_else(|| err("missing OFF counts"))?
                .as_slice(),
            2,
        )
    };
    if counts.len() < 3 {
        return Err(err("invalid OFF counts"));
    }
    let (nv, nf) = (count(counts[0])?, count(counts[1])?);
    let mut vertices = Vec::with_capacity(nv.min(lines.len()));
    let mut colors = Vec::with_capacity(nv.min(lines.len()));
    for _ in 0..nv {
        let values = lines
            .get(cursor)
            .ok_or_else(|| err("truncated OFF vertices"))?;
        cursor += 1;
        vertices.push(vertex(values)?);
        let color_start = if tag.contains('N') { 6 } else { 3 };
        let color = if tag.starts_with('C') {
            let values = values
                .get(color_start..color_start + 3)
                .ok_or_else(|| err("missing OFF vertex colors"))?;
            let normalized = values
                .iter()
                .any(|v| v.contains('.') || v.contains('e') || v.contains('E'))
                && values
                    .iter()
                    .all(|v| v.parse::<f64>().is_ok_and(|n| n <= 1.0));
            Some(rgb(values, normalized)?)
        } else {
            None
        };
        colors.push(color);
    }
    let mut scene = Scene::default();
    for _ in 0..nf {
        let values = lines
            .get(cursor)
            .ok_or_else(|| err("truncated OFF faces"))?;
        cursor += 1;
        let n = count(values[0])?;
        let indices = values
            .get(1..n + 1)
            .ok_or_else(|| err("truncated OFF face indices"))?
            .iter()
            .map(|s| s.parse::<usize>().map_err(Error::from))
            .collect::<Result<Vec<_>>>()?;
        let color = if let Some(channels) = values.get(n + 1..n + 4) {
            let normalized = channels.iter().any(|v| v.contains('.'))
                && channels
                    .iter()
                    .all(|v| v.parse::<f64>().is_ok_and(|n| n <= 1.0));
            Some(rgb(channels, normalized)?)
        } else {
            None
        };
        let color = color.or_else(|| {
            let cs: Vec<_> = indices
                .iter()
                .filter_map(|&i| colors.get(i).copied().flatten())
                .collect();
            if cs.is_empty() {
                None
            } else {
                Some(std::array::from_fn(|i| {
                    (cs.iter().map(|c| u64::from(c[i])).sum::<u64>() / cs.len() as u64) as u8
                }))
            }
        });
        add_face(&mut scene, &indices, &vertices, color)?;
    }
    if scene.triangles.is_empty() {
        scene.points = vertices
            .into_iter()
            .zip(colors)
            .map(|(position, color)| Point { position, color })
            .collect();
    }
    Ok(scene)
}

#[derive(Clone, Copy, Debug)]
/// Supported scalar storage types for the mesh or point-cloud file.
enum ScalarType {
    /// Signed eight-bit integer values.
    I8,
    /// Unsigned eight-bit integer values.
    U8,
    /// Signed 16-bit integer values.
    I16,
    /// Unsigned 16-bit integer values.
    U16,
    /// Signed 32-bit integer values.
    I32,
    /// Unsigned 32-bit integer values.
    U32,
    /// IEEE single-precision floating-point values.
    F32,
    /// IEEE double-precision floating-point values.
    F64,
}
impl ScalarType {
    /// Resolve the supported PLY scalar type names and aliases.
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "char" | "int8" => Self::I8,
            "uchar" | "uint8" => Self::U8,
            "short" | "int16" => Self::I16,
            "ushort" | "uint16" => Self::U16,
            "int" | "int32" => Self::I32,
            "uint" | "uint32" => Self::U32,
            "float" | "float32" => Self::F32,
            "double" | "float64" => Self::F64,
            _ => return Err(err(format!("unsupported PLY scalar type: {s}"))),
        })
    }
    /// Return the declared PLY scalar byte width.
    fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }
    /// Identify PLY floating-point storage instead of integer connectivity.
    fn floating(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }
}
#[derive(Debug)]
/// One PLY scalar or counted-list property declaration.
struct PlyProperty {
    /// Property identifier from the PLY header.
    name: String,
    /// Scalar type of a value or list item.
    item: ScalarType,
    /// Scalar type of a list length; absent for a scalar property.
    count: Option<ScalarType>,
}
#[derive(Debug)]
/// Count and ordered properties for one PLY element kind.
struct PlyElement {
    /// Element identifier such as vertex, face or edge.
    name: String,
    /// Declared number of records for this element.
    count: usize,
    /// Properties in their declared payload order.
    properties: Vec<PlyProperty>,
}
/// State for ASCII or endian-selected binary PLY scalar reads.
enum PlyValues<'a> {
    /// Whitespace-delimited ASCII scalar values.
    Ascii(std::str::SplitWhitespace<'a>),
    /// Byte-addressed binary scalar values with a selected byte order.
    Binary {
        /// Borrowed complete binary scalar payload.
        data: &'a [u8],
        /// Byte position of the next scalar value.
        cursor: usize,
        /// Whether binary words use little-endian byte order.
        little: bool,
    },
}
impl PlyValues<'_> {
    /// Read one PLY ASCII or endian-selected binary scalar.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "The declared PLY word width bounds packed values; narrowing preserves signed/IEEE bit patterns before f64 decoding."
    )]
    fn value(&mut self, kind: ScalarType) -> Result<f64> {
        match self {
            Self::Ascii(tokens) => {
                let token = tokens
                    .next()
                    .ok_or_else(|| err("truncated ASCII PLY data"))?;
                Ok(match kind {
                    ScalarType::I8 => f64::from(token.parse::<i8>()?),
                    ScalarType::U8 => f64::from(token.parse::<u8>()?),
                    ScalarType::I16 => f64::from(token.parse::<i16>()?),
                    ScalarType::U16 => f64::from(token.parse::<u16>()?),
                    ScalarType::I32 => f64::from(token.parse::<i32>()?),
                    ScalarType::U32 => f64::from(token.parse::<u32>()?),
                    ScalarType::F32 => f64::from(token.parse::<f32>()?),
                    ScalarType::F64 => token.parse::<f64>()?,
                })
            }
            Self::Binary {
                data,
                cursor,
                little,
            } => {
                let end = cursor
                    .checked_add(kind.size())
                    .ok_or_else(|| err("PLY offset overflow"))?;
                let bytes = data
                    .get(*cursor..end)
                    .ok_or_else(|| err("truncated binary PLY data"))?;
                *cursor = end;
                let n = match kind.size() {
                    1 => u64::from(bytes[0]),
                    2 => u64::from(if *little {
                        u16::from_le_bytes(bytes.try_into()?)
                    } else {
                        u16::from_be_bytes(bytes.try_into()?)
                    }),
                    4 => u64::from(if *little {
                        u32::from_le_bytes(bytes.try_into()?)
                    } else {
                        u32::from_be_bytes(bytes.try_into()?)
                    }),
                    8 => {
                        if *little {
                            u64::from_le_bytes(bytes.try_into()?)
                        } else {
                            u64::from_be_bytes(bytes.try_into()?)
                        }
                    }
                    _ => unreachable!(),
                };
                Ok(match kind {
                    ScalarType::I8 => f64::from(n as i8),
                    ScalarType::U8 | ScalarType::U16 | ScalarType::U32 => n as f64,
                    ScalarType::I16 => f64::from(n as i16),
                    ScalarType::I32 => f64::from(n as i32),
                    ScalarType::F32 => f64::from(f32::from_bits(n as u32)),
                    ScalarType::F64 => f64::from_bits(n),
                })
            }
        }
    }
    /// Read a bounded nonnegative PLY list length.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        reason = "PLY list lengths are integral, nonnegative and bounded to MAX_FACE_VERTICES before native conversion."
    )]
    fn list_count(&mut self, kind: ScalarType) -> Result<usize> {
        let value = self.value(kind)?;
        if value < 0.0 || value > MAX_ITEMS as f64 || kind.floating() {
            return Err(err("invalid PLY list count"));
        }
        Ok(value as usize)
    }
}

/// Decode PLY polygons, standard edge records and colored point fallback.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "PLY counts and indices are range checked; color channels are clamped and intentionally rounded into bytes."
)]
#[allow(
    clippy::too_many_lines,
    reason = "Keep ordered PLY element/property decoding together to preserve cross-element edge and face references."
)]
fn ply(bytes: &[u8]) -> Result<Scene> {
    let mut cursor = 0;
    let mut format = None;
    let mut elements: Vec<PlyElement> = Vec::new();
    let mut first = true;
    loop {
        let remaining = bytes
            .get(cursor..)
            .ok_or_else(|| err("invalid PLY header"))?;
        let len = remaining
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| err("missing PLY end_header"))?;
        let line = text(&remaining[..len])?.trim();
        cursor += len + 1;
        if cursor > 1_048_576 {
            return Err(err("PLY header exceeds one megabyte"));
        }
        if first {
            if line != "ply" {
                return Err(err("missing PLY magic"));
            }
            first = false;
            continue;
        }
        let words: Vec<_> = line.split_whitespace().collect();
        match words.first().copied() {
            Some("format") => {
                if words.len() != 3 || words[2] != "1.0" || format.is_some() {
                    return Err(err("invalid PLY format declaration"));
                }
                format = Some(words[1]);
            }
            Some("element") => {
                if words.len() != 3 {
                    return Err(err("invalid PLY element declaration"));
                }
                elements.push(PlyElement {
                    name: words[1].into(),
                    count: count(words[2])?,
                    properties: Vec::new(),
                });
            }
            Some("property") => {
                let element = elements
                    .last_mut()
                    .ok_or_else(|| err("PLY property without element"))?;
                let property = match words.as_slice() {
                    ["property", kind, name] => PlyProperty {
                        name: (*name).into(),
                        item: ScalarType::parse(kind)?,
                        count: None,
                    },
                    ["property", "list", length, kind, name] => {
                        let length = ScalarType::parse(length)?;
                        if length.floating() {
                            return Err(err("PLY list count must be an integer"));
                        }
                        PlyProperty {
                            name: (*name).into(),
                            item: ScalarType::parse(kind)?,
                            count: Some(length),
                        }
                    }
                    _ => return Err(err("invalid PLY property declaration")),
                };
                element.properties.push(property);
            }
            Some("end_header") => break,
            Some("comment" | "obj_info") | None => {}
            _ => return Err(err("unrecognized PLY header directive")),
        }
    }
    let mut values = match format {
        Some("ascii") => PlyValues::Ascii(text(&bytes[cursor..])?.split_whitespace()),
        Some("binary_little_endian") => PlyValues::Binary {
            data: &bytes[cursor..],
            cursor: 0,
            little: true,
        },
        Some("binary_big_endian") => PlyValues::Binary {
            data: &bytes[cursor..],
            cursor: 0,
            little: false,
        },
        _ => return Err(err("unsupported or missing PLY encoding")),
    };
    let mut vertices = Vec::new();
    let mut colors = Vec::new();
    let mut faces = Vec::new();
    let mut edges = Vec::new();
    for element in elements {
        if element.name == "vertex" {
            for axis in ["x", "y", "z"] {
                if !element
                    .properties
                    .iter()
                    .any(|p| p.name == axis && p.count.is_none())
                {
                    return Err(err(format!("PLY vertex lacks {axis} coordinate")));
                }
            }
        }
        if element.properties.is_empty() && element.count != 0 {
            return Err(err("PLY element has no properties"));
        }
        if element.name == "edge" && element.count != 0 {
            for endpoint in ["vertex1", "vertex2"] {
                let mut properties = element
                    .properties
                    .iter()
                    .filter(|property| property.name == endpoint);
                let property = properties.next();
                if properties.next().is_some()
                    || property
                        .is_none_or(|property| property.count.is_some() || property.item.floating())
                {
                    return Err(err(format!("PLY edge requires one integer {endpoint}")));
                }
            }
        }
        for _ in 0..element.count {
            let mut position = Vec3::default();
            let mut color = [None; 3];
            let mut indices = None;
            let mut endpoints = [None; 2];
            for property in &element.properties {
                if let Some(kind) = property.count {
                    let n = values.list_count(kind)?;
                    if element.name == "face"
                        && matches!(property.name.as_str(), "vertex_indices" | "vertex_index")
                    {
                        if property.item.floating() {
                            return Err(err("PLY face indices must be integers"));
                        }
                        let mut list = Vec::with_capacity(n.min(4096));
                        for _ in 0..n {
                            let value = values.value(property.item)?;
                            if value < 0.0 || value > MAX_ITEMS as f64 {
                                return Err(err("invalid PLY face index"));
                            }
                            list.push(value as usize);
                        }
                        indices = Some(list);
                    } else {
                        for _ in 0..n {
                            values.value(property.item)?;
                        }
                    }
                } else {
                    let value = values.value(property.item)?;
                    if element.name == "vertex" {
                        match property.name.as_str() {
                            "x" => position.x = value,
                            "y" => position.y = value,
                            "z" => position.z = value,
                            _ => {}
                        }
                    }
                    if element.name == "edge" {
                        let endpoint = match property.name.as_str() {
                            "vertex1" => Some(0),
                            "vertex2" => Some(1),
                            _ => None,
                        };
                        if let Some(endpoint) = endpoint {
                            if property.item.floating() || value < 0.0 || value > MAX_ITEMS as f64 {
                                return Err(err("invalid PLY edge index"));
                            }
                            endpoints[endpoint] = Some(value as usize);
                        }
                    }
                    let channel = match property.name.as_str() {
                        "red" | "r" | "diffuse_red" => Some(0),
                        "green" | "g" | "diffuse_green" => Some(1),
                        "blue" | "b" | "diffuse_blue" => Some(2),
                        _ => None,
                    };
                    if let Some(channel) = channel
                        .filter(|_| matches!(element.name.as_str(), "vertex" | "face" | "edge"))
                    {
                        if !value.is_finite() {
                            return Err(err("non-finite PLY color"));
                        }
                        color[channel] = Some(
                            (if property.item.floating() {
                                value * 255.0
                            } else {
                                value
                            })
                            .clamp(0.0, 255.0)
                            .round() as u8,
                        );
                    }
                }
            }
            let color = match color {
                [Some(r), Some(g), Some(b)] => Some([r, g, b]),
                _ => None,
            };
            if element.name == "vertex" {
                if !position.is_finite() {
                    return Err(err("non-finite PLY vertex"));
                }
                vertices.push(position);
                colors.push(color);
            } else if element.name == "face" {
                faces.push((
                    indices.ok_or_else(|| err("PLY face lacks vertex indices"))?,
                    color,
                ));
            } else if element.name == "edge" {
                let [Some(first), Some(second)] = endpoints else {
                    return Err(err("PLY edge lacks vertex indices"));
                };
                edges.push(([first, second], color));
            }
        }
    }
    let mut scene = Scene::default();
    for (indices, color) in faces {
        let color = color.or_else(|| indexed_color(&indices, &colors));
        add_face(&mut scene, &indices, &vertices, color)?;
    }
    for (indices, color) in edges {
        let [Some(first), Some(second)] = indices.map(|index| vertices.get(index).copied()) else {
            return Err(err("PLY edge references a missing vertex"));
        };
        scene.lines.push(Line {
            vertices: [first, second],
            color: color.or_else(|| indexed_color(&indices, &colors)),
        });
    }
    if scene.triangles.is_empty() && scene.lines.is_empty() {
        scene.points = vertices
            .into_iter()
            .zip(colors)
            .map(|(position, color)| Point { position, color })
            .collect();
    }
    Ok(scene)
}

/// Split nonempty noncomment point-cloud lines into whitespace fields.
fn point_lines(source: &str) -> Vec<Vec<&str>> {
    source
        .lines()
        .map(|line| {
            line.split('#')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .collect()
        })
        .filter(|values: &Vec<_>| !values.is_empty())
        .collect()
}
/// Decode optional-count PTS records with finite positions and optional RGB.
fn pts(source: &str) -> Result<Scene> {
    let mut lines = point_lines(source);
    let expected = if lines.first().is_some_and(|line| line.len() == 1) {
        Some(count(lines.remove(0)[0])?)
    } else {
        None
    };
    if expected.is_some_and(|n| n != lines.len()) {
        return Err(err("PTS point count does not match the data"));
    }
    if lines.len() > MAX_ITEMS {
        return Err(err("too many PTS points"));
    }
    let points = lines
        .into_iter()
        .map(|values| {
            let position = vertex(&values)?;
            let color = if values.len() >= 7 {
                Some(rgb(&values[4..7], false)?)
            } else if values.len() == 6 {
                Some(rgb(&values[3..6], false)?)
            } else {
                None
            };
            Ok(Point { position, color })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Scene {
        points,
        ..Scene::default()
    })
}
/// Decode gridded PTX scans and apply their scanner transforms.
fn ptx(source: &str) -> Result<Scene> {
    let lines = point_lines(source);
    let mut cursor = 0;
    let mut scene = Scene::default();
    while cursor < lines.len() {
        let next = |cursor: &mut usize| -> Result<&[&str]> {
            let line = lines
                .get(*cursor)
                .ok_or_else(|| err("truncated PTX scan"))?;
            *cursor += 1;
            Ok(line)
        };
        let cols = next(&mut cursor)?;
        let rows = next(&mut cursor)?;
        if cols.len() != 1 || rows.len() != 1 {
            return Err(err("invalid PTX dimensions"));
        }
        let n = count(cols[0])?
            .checked_mul(count(rows[0])?)
            .filter(|&n| n <= MAX_ITEMS)
            .ok_or_else(|| err("too many PTX points"))?;
        // Scanner position and three axes are metadata; the following matrix
        // maps scan-local coordinates into the model's coordinate system.
        for _ in 0..4 {
            let _ = vertex(next(&mut cursor)?)?;
        }
        let mut transform = [[0.0; 4]; 4];
        for row in &mut transform {
            let values = next(&mut cursor)?;
            if values.len() != 4 {
                return Err(err("PTX transform requires four columns"));
            }
            for (out, value) in row.iter_mut().zip(values) {
                *out = finite(value)?;
            }
        }
        for _ in 0..n {
            let values = next(&mut cursor)?;
            let local = vertex(values)?;
            if local == Vec3::default() {
                continue;
            } // PTX missing-return sentinel.
            let components = [local.x, local.y, local.z, 1.0];
            let transformed: [f64; 4] =
                std::array::from_fn(|j| (0..4).map(|i| components[i] * transform[i][j]).sum());
            if transformed[3] == 0.0 {
                return Err(err("PTX transform maps a point to infinity"));
            }
            let position =
                Vec3::new(transformed[0], transformed[1], transformed[2]) / transformed[3];
            if !position.is_finite() {
                return Err(err("non-finite transformed PTX coordinate"));
            }
            let color = if values.len() >= 7 {
                Some(rgb(&values[4..7], false)?)
            } else {
                None
            };
            scene.points.push(Point { position, color });
            if scene.points.len() > MAX_ITEMS {
                return Err(err("too many PTX points"));
            }
        }
    }
    Ok(scene)
}

/// Sample fixed-layout Gaussian-splat centers and stored RGB values.
fn splat(bytes: &[u8]) -> Result<Scene> {
    if !bytes.len().is_multiple_of(32) || bytes.len() / 32 > MAX_ITEMS {
        return Err(err("invalid splat file length (expected 32-byte records)"));
    }
    let mut points = Vec::with_capacity(bytes.len() / 32);
    for record in bytes.as_chunks::<32>().0 {
        let position = Vec3::from(std::array::from_fn(|i| {
            let j = i * 4;
            f32::from_le_bytes([record[j], record[j + 1], record[j + 2], record[j + 3]])
        }));
        if !position.is_finite() {
            return Err(err("non-finite splat position"));
        }
        // The CPU preview represents Gaussian centers as colored points.
        points.push(Point {
            position,
            color: Some([record[24], record[25], record[26]]),
        });
    }
    Ok(Scene {
        points,
        ..Scene::default()
    })
}

#[derive(Debug)]
/// PCD field layout in interleaved and field-major payloads.
struct PcdField {
    /// Named semantic from the PCD FIELDS header.
    name: String,
    /// Signed, unsigned or floating-point TYPE code.
    kind: char,
    /// Stored byte width of each component.
    size: usize,
    /// Components per point record for this field.
    count: usize,
    /// Byte offset in an interleaved point record.
    offset: usize,
    /// Byte offset of this field in a decompressed field-major payload.
    block_offset: usize,
}
/// Decode one binary PCD field while retaining packed integer color bits.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    reason = "PCD word widths are validated; casts preserve packed signed/IEEE bit patterns and intentionally approximate 64-bit scalar values."
)]
fn pcd_numeric(bytes: &[u8], kind: char) -> Result<(f64, u64)> {
    let bits = match bytes.len() {
        1 => u64::from(bytes[0]),
        2 => u64::from(u16::from_le_bytes(bytes.try_into()?)),
        4 => u64::from(u32::from_le_bytes(bytes.try_into()?)),
        8 => u64::from_le_bytes(bytes.try_into()?),
        _ => return Err(err("unsupported PCD field size")),
    };
    let value = match (kind, bytes.len()) {
        ('F', 4) => f64::from(f32::from_bits(bits as u32)),
        ('F', 8) => f64::from_bits(bits),
        ('I', 1) => f64::from(bits as i8),
        ('I', 2) => f64::from(bits as i16),
        ('I', 4) => f64::from(bits as i32),
        ('I', 8) => bits as i64 as f64,
        ('U', _) => bits as f64,
        _ => return Err(err("unsupported PCD field type")),
    };
    Ok((value, bits))
}
/// Decode an ASCII PCD field with its declared type, width and raw packed bits.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "Signed PCD values retain their two-complement packed color bits; scalar previews intentionally use f64."
)]
fn pcd_ascii(token: &str, field: &PcdField) -> Result<(f64, u64)> {
    Ok(match (field.kind, field.size) {
        ('F', 4) => {
            let n = token.parse::<f32>()?;
            (f64::from(n), u64::from(n.to_bits()))
        }
        ('F', 8) => {
            let n = token.parse::<f64>()?;
            (n, n.to_bits())
        }
        ('I', 1) => {
            let n = token.parse::<i8>()?;
            (f64::from(n), u64::from(n as u8))
        }
        ('I', 2) => {
            let n = token.parse::<i16>()?;
            (f64::from(n), u64::from(n as u16))
        }
        ('I', 4) => {
            let n = token.parse::<i32>()?;
            (f64::from(n), u64::from(n as u32))
        }
        ('I', 8) => {
            let n = token.parse::<i64>()?;
            (n as f64, n as u64)
        }
        ('U', 1) => {
            let n = token.parse::<u8>()?;
            (f64::from(n), u64::from(n))
        }
        ('U', 2) => {
            let n = token.parse::<u16>()?;
            (f64::from(n), u64::from(n))
        }
        ('U', 4) => {
            let n = token.parse::<u32>()?;
            (f64::from(n), u64::from(n))
        }
        ('U', 8) => {
            let n = token.parse::<u64>()?;
            (n as f64, n)
        }
        _ => return Err(err("unsupported PCD field type or size")),
    })
}
/// Expand bounded PCD LZF literals and overlapping backward copies.
fn lzf(input: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(expected.min(1_048_576));
    let mut cursor = 0;
    while cursor < input.len() {
        let control = input[cursor] as usize;
        cursor += 1;
        if control < 32 {
            let n = control + 1;
            let end = cursor
                .checked_add(n)
                .ok_or_else(|| err("LZF literal overflow"))?;
            let literal = input
                .get(cursor..end)
                .ok_or_else(|| err("truncated LZF literal"))?;
            if output.len().checked_add(n).is_none_or(|n| n > expected) {
                return Err(err("LZF output exceeds declared size"));
            }
            output.extend_from_slice(literal);
            cursor = end;
        } else {
            let mut n = control >> 5;
            if n == 7 {
                n += *input
                    .get(cursor)
                    .ok_or_else(|| err("truncated LZF length"))? as usize;
                cursor += 1;
            }
            let low = *input
                .get(cursor)
                .ok_or_else(|| err("truncated LZF offset"))? as usize;
            cursor += 1;
            let distance = ((control & 31) << 8) + low + 1;
            n += 2;
            let start = output
                .len()
                .checked_sub(distance)
                .ok_or_else(|| err("invalid LZF backward reference"))?;
            if output.len().checked_add(n).is_none_or(|n| n > expected) {
                return Err(err("LZF output exceeds declared size"));
            }
            for i in 0..n {
                let byte = output[start + i];
                output.push(byte);
            }
        }
    }
    if output.len() != expected {
        return Err(err("LZF output size does not match PCD header"));
    }
    Ok(output)
}
/// Decode PCD ASCII, binary or field-major LZF-compressed records using named fields.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "PCD counts and field layouts are checked; packed RGB extracts low bytes and normalized colors intentionally quantize to bytes."
)]
#[allow(
    clippy::too_many_lines,
    reason = "Keep PCD header/layout validation and encoding dispatch together with the same field-offset table."
)]
fn pcd(bytes: &[u8]) -> Result<Scene> {
    let mut header: HashMap<String, Vec<String>> = HashMap::new();
    let mut cursor = 0;
    let encoding = loop {
        let remaining = bytes
            .get(cursor..)
            .ok_or_else(|| err("invalid PCD header"))?;
        let len = remaining
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| err("missing PCD DATA line"))?;
        let line = text(&remaining[..len])?
            .split('#')
            .next()
            .unwrap_or("")
            .trim();
        cursor += len + 1;
        if cursor > 1_048_576 {
            return Err(err("PCD header exceeds one megabyte"));
        }
        let mut words = line.split_whitespace();
        let Some(key) = words.next() else {
            continue;
        };
        let key = key.to_ascii_uppercase();
        let values: Vec<String> = words.map(str::to_owned).collect();
        if key == "DATA" {
            if values.len() != 1 {
                return Err(err("invalid PCD DATA declaration"));
            }
            break values[0].to_ascii_lowercase();
        }
        if header.insert(key.clone(), values).is_some() {
            return Err(err(format!("duplicate PCD {key} declaration")));
        }
    };
    let names = header
        .get("FIELDS")
        .or_else(|| header.get("FIELD"))
        .ok_or_else(|| err("missing PCD FIELDS"))?;
    let sizes = header.get("SIZE").ok_or_else(|| err("missing PCD SIZE"))?;
    let kinds = header.get("TYPE").ok_or_else(|| err("missing PCD TYPE"))?;
    let counts = header.get("COUNT");
    if names.is_empty()
        || names.len() != sizes.len()
        || names.len() != kinds.len()
        || counts.is_some_and(|c| c.len() != names.len())
    {
        return Err(err("PCD field declarations have different lengths"));
    }
    let single_count = |key: &str| -> Result<Option<usize>> {
        header
            .get(key)
            .map(|v| {
                if v.len() != 1 {
                    return Err(err(format!("invalid PCD {key}")));
                }
                count(&v[0])
            })
            .transpose()
    };
    let shape = single_count("WIDTH")?
        .zip(single_count("HEIGHT")?)
        .map(|(w, h)| {
            w.checked_mul(h)
                .filter(|&n| n <= MAX_ITEMS)
                .ok_or_else(|| err("too many PCD points"))
        })
        .transpose()?;
    let n = single_count("POINTS")?
        .or(shape)
        .ok_or_else(|| err("PCD needs POINTS or WIDTH and HEIGHT"))?;
    if shape.is_some_and(|s| s != n) {
        return Err(err("PCD POINTS differs from WIDTH times HEIGHT"));
    }
    let mut fields = Vec::new();
    let mut stride = 0_usize;
    let mut block_offset = 0_usize;
    for i in 0..names.len() {
        let size: usize = sizes[i].parse()?;
        let kind = kinds[i]
            .chars()
            .next()
            .ok_or_else(|| err("empty PCD field type"))?
            .to_ascii_uppercase();
        if kinds[i].len() != 1 || !matches!((kind, size), ('F', 4 | 8) | ('I' | 'U', 1 | 2 | 4 | 8))
        {
            return Err(err("unsupported PCD field type or size"));
        }
        let field_count = if let Some(counts) = counts {
            count(&counts[i])?
        } else {
            1
        };
        if field_count == 0 {
            return Err(err("PCD field count must be positive"));
        }
        let width = size
            .checked_mul(field_count)
            .ok_or_else(|| err("PCD field width overflow"))?;
        fields.push(PcdField {
            name: names[i].to_ascii_lowercase(),
            kind,
            size,
            count: field_count,
            offset: stride,
            block_offset,
        });
        stride = stride
            .checked_add(width)
            .ok_or_else(|| err("PCD point size overflow"))?;
        block_offset = block_offset
            .checked_add(
                width
                    .checked_mul(n)
                    .ok_or_else(|| err("PCD data size overflow"))?,
            )
            .ok_or_else(|| err("PCD data size overflow"))?;
    }
    for axis in ["x", "y", "z"] {
        if fields
            .iter()
            .filter(|f| f.name == axis && f.count == 1)
            .count()
            != 1
        {
            return Err(err(format!("PCD needs one scalar {axis} field")));
        }
    }
    let expected = stride
        .checked_mul(n)
        .ok_or_else(|| err("PCD data size overflow"))?;
    if expected > 512 * 1024 * 1024 {
        return Err(err("PCD payload exceeds 512 megabytes"));
    }
    let compressed;
    let payload = match encoding.as_str() {
        "ascii" => &bytes[cursor..],
        "binary" => {
            let data = bytes
                .get(
                    cursor
                        ..cursor
                            .checked_add(expected)
                            .ok_or_else(|| err("PCD offset overflow"))?,
                )
                .ok_or_else(|| err("truncated binary PCD payload"))?;
            if bytes.len() != cursor + expected {
                return Err(err("binary PCD payload length does not match header"));
            }
            data
        }
        "binary_compressed" => {
            let sizes = bytes
                .get(cursor..cursor + 8)
                .ok_or_else(|| err("truncated compressed PCD prefix"))?;
            let input_len = u32::from_le_bytes(sizes[0..4].try_into()?) as usize;
            let output_len = u32::from_le_bytes(sizes[4..8].try_into()?) as usize;
            if output_len != expected {
                return Err(err("compressed PCD length differs from header"));
            }
            let data = bytes
                .get(cursor + 8..)
                .ok_or_else(|| err("truncated compressed PCD payload"))?;
            if data.len() != input_len {
                return Err(err("compressed PCD input length differs from prefix"));
            }
            compressed = lzf(data, expected)?;
            compressed.as_slice()
        }
        _ => return Err(err(format!("unsupported PCD encoding: {encoding}"))),
    };
    let mut tokens = if encoding == "ascii" {
        Some(text(payload)?.split_whitespace())
    } else {
        None
    };
    let mut scene = Scene::default();
    for point_index in 0..n {
        let mut position = Vec3::default();
        let mut color = None;
        let mut channels = [None; 3];
        for field in &fields {
            for component in 0..field.count {
                let (value, bits) = if let Some(tokens) = &mut tokens {
                    pcd_ascii(
                        tokens
                            .next()
                            .ok_or_else(|| err("truncated ASCII PCD payload"))?,
                        field,
                    )?
                } else {
                    let offset = if encoding == "binary_compressed" {
                        field.block_offset
                            + point_index * field.size * field.count
                            + component * field.size
                    } else {
                        point_index * stride + field.offset + component * field.size
                    };
                    pcd_numeric(
                        payload
                            .get(offset..offset + field.size)
                            .ok_or_else(|| err("truncated PCD field"))?,
                        field.kind,
                    )?
                };
                if component != 0 {
                    continue;
                }
                match field.name.as_str() {
                    "x" => position.x = value,
                    "y" => position.y = value,
                    "z" => position.z = value,
                    "rgb" | "rgba" => {
                        color = Some([(bits >> 16) as u8, (bits >> 8) as u8, bits as u8]);
                    }
                    "r" | "red" => channels[0] = Some(value),
                    "g" | "green" => channels[1] = Some(value),
                    "b" | "blue" => channels[2] = Some(value),
                    _ => {}
                }
            }
        }
        if let [Some(r), Some(g), Some(b)] = channels {
            if ![r, g, b].iter().all(|v| v.is_finite()) {
                return Err(err("non-finite PCD color"));
            }
            color = Some([r, g, b].map(|v| v.clamp(0.0, 255.0).round() as u8));
        }
        // Organized PCD clouds use NaN coordinates for missing sensor returns.
        if position.is_finite() {
            scene.points.push(Point { position, color });
        }
    }
    if tokens.is_some_and(|mut t| t.next().is_some()) {
        return Err(err("extra values after ASCII PCD points"));
    }
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle_area(t: [Vec3; 3]) -> f64 {
        (t[1] - t[0]).cross(t[2] - t[0]).length() / 2.0
    }

    #[test]
    fn concave_faces_cover_only_the_polygon() {
        let polygon = [
            Vec3::new(0., 0., 0.),
            Vec3::new(2., 0., 0.),
            Vec3::new(2., 2., 0.),
            Vec3::new(1., 1., 0.),
            Vec3::new(0., 2., 0.),
        ];
        let triangles = triangulate_polygon(&polygon).unwrap();
        assert_eq!(triangles.len(), 3);
        assert!((triangles.iter().copied().map(triangle_area).sum::<f64>() - 3.0).abs() < 1e-12);
        let reversed: Vec<_> = polygon.into_iter().rev().collect();
        assert!(
            (triangulate_polygon(&reversed)
                .unwrap()
                .into_iter()
                .map(triangle_area)
                .sum::<f64>()
                - 3.0)
                .abs()
                < 1e-12
        );
    }

    #[test]
    fn binary_stl_may_have_solid_header() {
        let mut data = vec![0; 84];
        data[..5].copy_from_slice(b"solid");
        data[80..84].copy_from_slice(&1_u32.to_le_bytes());
        let mut triangle = [0; 50];
        triangle[24..28].copy_from_slice(&1_f32.to_le_bytes()); // vertex 1 x
        triangle[40..44].copy_from_slice(&1_f32.to_le_bytes()); // vertex 2 y
        data.extend_from_slice(&triangle);
        let scene = stl(&data).unwrap();
        assert_eq!(scene.triangles.len(), 1);
        assert_eq!(scene.triangles[0].vertices[1], Vec3::new(1., 0., 0.));
        assert_eq!(scene.triangles[0].vertices[2], Vec3::new(0., 1., 0.));
        assert!(stl(&data[..data.len() - 1]).is_err());
    }

    #[test]
    fn ascii_stl_rejects_incomplete_facets() {
        assert!(stl(b"solid a\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nendloop\nendfacet\nendsolid a").is_err());
        let scene = stl(b"solid a\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid a").unwrap();
        assert_eq!(scene.triangles.len(), 1);
    }

    #[test]
    fn obj_negative_indices_and_vertex_colors() {
        let source = "v 0 0 0 1 0 0\nv 1 0 0 1 0 0\nv 0 1 0 1 0 0\nf -3/-1 -2/-1 -1/-1\n";
        let scene = obj(source, Path::new(".")).unwrap();
        assert_eq!(scene.triangles.len(), 1);
        assert_eq!(scene.triangles[0].color, Some([255, 0, 0]));
        assert!(obj("v 0 0 0\nf 0 1 1", Path::new(".")).is_err());
        assert!(obj("v 0 0 0\nf -2 1 1", Path::new(".")).is_err());
    }

    #[test]
    fn obj_polylines_keep_segments_colors_and_explicit_geometry() {
        let vertices = "v 0 0 0 1 0 0\nv 2 0 0 0 1 0\nv 2 1 1 0 0 1\nv 1000 1000 1000\n";
        let source = format!("{vertices}vt 0 0\nvt 1 0\nvt 1 1\nl 1/1 2/2 3/3\n");
        let scene = obj(&source, Path::new(".")).unwrap();
        assert!(scene.points.is_empty());
        assert!(scene.triangles.is_empty());
        assert_eq!(scene.lines.len(), 2);
        assert_eq!(
            scene.lines[0].vertices,
            [Vec3::new(0., 0., 0.), Vec3::new(2., 0., 0.)]
        );
        assert_eq!(
            scene.lines[1].vertices,
            [Vec3::new(2., 0., 0.), Vec3::new(2., 1., 1.)]
        );
        assert_eq!(scene.lines[0].color, Some([127, 127, 0]));
        assert_eq!(scene.lines[1].color, Some([0, 127, 127]));
        assert!(scene.positions().all(|v| v.x < 1000.));
        let mixed = obj(
            &format!("{vertices}l -4 -3 -2\nf 1 2 3\np -4\n"),
            Path::new("."),
        )
        .unwrap();
        assert_eq!(mixed.lines.len(), 2);
        assert_eq!(mixed.triangles.len(), 1);
        assert_eq!(mixed.points.len(), 1);
        assert_eq!(mixed.lines[1].vertices, scene.lines[1].vertices);
    }

    #[test]
    fn obj_lines_use_materials_and_reject_invalid_or_unimplemented_geometry() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("catwalk-obj-unit-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(
            directory.join("obj-palette.mtl"),
            "newmtl face0\nKd 0.9019607843137255 0.4117647058823529 0.21568627450980393\n",
        )
        .unwrap();
        let loaded = obj(
            "mtllib obj-palette.mtl\nv 0 0 0 0 1 0\nv 1 0 0 0 1 0\nusemtl face0\nl 1 2\n",
            &directory,
        );
        std::fs::remove_dir_all(&directory).unwrap();
        let scene = loaded.unwrap();
        assert_eq!(scene.lines[0].color, Some([230, 105, 55]));
        for element in [
            "l",
            "l 1",
            "l 0 1",
            "l -3 -1",
            "l 1 3",
            "p 0",
            "curv 0 1 1 2",
            "surf 0 1 0 1 1 2",
        ] {
            let source = format!("v 0 0 0\nv 1 0 0\n{element}\n");
            assert!(obj(&source, Path::new(".")).is_err(), "{element}");
        }
    }

    #[test]
    fn off_counts_colors_and_truncation() {
        let scene =
            off("COFF\n3 1 0\n0 0 0 255 0 0 255\n1 0 0 255 0 0 255\n0 1 0 255 0 0 255\n3 0 1 2\n")
                .unwrap();
        assert_eq!(scene.triangles[0].color, Some([255, 0, 0]));
        assert!(off("OFF\n3 1 0\n0 0 0\n1 0 0\n").is_err());
        assert!(off("OFF\n3 1 0\n0 0 0\n1 0 0\n0 1 0\n3 0 1 3\n").is_err());
    }

    #[test]
    fn ply_ascii_skips_unknown_lists() {
        let data = b"ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nproperty list uchar int tags\nelement face 1\nproperty list uchar int vertex_indices\nproperty float confidence\nend_header\n0 0 0 2 5 6\n1 0 0 0\n0 1 0 1 9\n3 0 1 2 0.5\n";
        assert_eq!(ply(data).unwrap().triangles.len(), 1);
        let bad = String::from_utf8(data.to_vec())
            .unwrap()
            .replace("3 0 1 2 0.5", "3 0 1 4 0.5");
        assert!(ply(bad.as_bytes()).is_err());
    }

    #[test]
    fn ply_binary_both_endiannesses_and_truncation() {
        for little in [true, false] {
            let format = if little {
                "binary_little_endian"
            } else {
                "binary_big_endian"
            };
            let mut bytes = format!("ply\r\nformat {format} 1.0\r\nelement vertex 3\r\nproperty double x\r\nproperty double y\r\nproperty double z\r\nproperty uchar red\r\nproperty uchar green\r\nproperty uchar blue\r\nelement face 1\r\nproperty list uchar int vertex_indices\r\nend_header\r\n").into_bytes();
            for p in [[0_f64, 0., 0.], [1., 0., 0.], [0., 1., 0.]] {
                for c in p {
                    bytes.extend_from_slice(&if little {
                        c.to_le_bytes()
                    } else {
                        c.to_be_bytes()
                    });
                }
                bytes.extend_from_slice(&[10, 20, 30]);
            }
            bytes.push(3);
            for index in 0_i32..3 {
                bytes.extend_from_slice(&if little {
                    index.to_le_bytes()
                } else {
                    index.to_be_bytes()
                });
            }
            let scene = ply(&bytes).unwrap();
            assert_eq!(scene.triangles.len(), 1);
            assert_eq!(scene.triangles[0].color, Some([10, 20, 30]));
            assert!(ply(&bytes[..bytes.len() - 1]).is_err());
        }
    }

    #[test]
    fn ply_edges_preserve_wire_geometry_before_vertices_and_with_faces() {
        let source = "ply\nformat ascii 1.0\nelement edge 2\nproperty int vertex1\nproperty int vertex2\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nend_header\n0 1 230 105 55\n1 2 55 150 220\n0 0 0\n2 0 0\n2 1 1\n";
        let scene = ply(source.as_bytes()).unwrap();
        assert!(scene.points.is_empty());
        assert_eq!(scene.lines.len(), 2);
        assert_eq!(
            scene.lines[0].vertices,
            [Vec3::new(0., 0., 0.), Vec3::new(2., 0., 0.)]
        );
        assert_eq!(
            scene.lines[1].vertices,
            [Vec3::new(2., 0., 0.), Vec3::new(2., 1., 1.)]
        );
        assert_eq!(scene.lines[1].color, Some([55, 150, 220]));
        let mixed = source.replace(
            "end_header",
            "element face 1\nproperty list uchar int vertex_indices\nend_header",
        ) + "3 0 1 2\n";
        let scene = ply(mixed.as_bytes()).unwrap();
        assert_eq!(scene.triangles.len(), 1);
        assert_eq!(scene.lines.len(), 2);
        assert!(scene.points.is_empty());
        for invalid in [
            source.replace("0 1 230", "0 3 230"),
            source.replace("0 1 230", "-1 1 230"),
            source.replace("property int vertex1", "property float vertex1"),
            source.replace("property int vertex1", "property int absent"),
            source.replace(
                "property int vertex1",
                "property int vertex1\nproperty list uchar int vertex1",
            ),
        ] {
            assert!(ply(invalid.as_bytes()).is_err());
        }
    }

    #[test]
    fn ply_binary_edges_use_vertex_colors_in_both_byte_orders() {
        for little in [true, false] {
            let format = if little {
                "binary_little_endian"
            } else {
                "binary_big_endian"
            };
            let mut bytes = format!("ply\nformat {format} 1.0\nelement vertex 2\nproperty double x\nproperty double y\nproperty double z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nelement edge 1\nproperty int vertex1\nproperty uint vertex2\nend_header\n").into_bytes();
            for (vertex, color) in [([0_f64, 0., 0.], [255, 0, 0]), ([2., 1., 1.], [0, 0, 255])] {
                for value in vertex {
                    bytes.extend(if little {
                        value.to_le_bytes()
                    } else {
                        value.to_be_bytes()
                    });
                }
                bytes.extend(color);
            }
            for index in [0_i32, 1] {
                bytes.extend(if little {
                    index.to_le_bytes()
                } else {
                    index.to_be_bytes()
                });
            }
            let scene = ply(&bytes).unwrap();
            assert_eq!(scene.lines.len(), 1);
            assert!(scene.points.is_empty());
            assert_eq!(
                scene.lines[0].vertices,
                [Vec3::new(0., 0., 0.), Vec3::new(2., 1., 1.)]
            );
            assert_eq!(scene.lines[0].color, Some([127, 0, 127]));
            assert!(ply(&bytes[..bytes.len() - 1]).is_err());
        }
    }

    #[test]
    fn pts_declared_count_and_color() {
        let scene = pts("2\n0 0 0 0.5 10 20 30\n1 2 3 0.9 40 50 60\n").unwrap();
        assert_eq!(scene.points.len(), 2);
        assert_eq!(scene.points[1].color, Some([40, 50, 60]));
        assert!(pts("2\n0 0 0\n").is_err());
        assert!(pts("nan 0 0\n").is_err());
    }

    #[test]
    fn ptx_applies_transform_and_skips_missing_return() {
        let scene = ptx("2\n1\n0 0 0\n1 0 0\n0 1 0\n0 0 1\n1 0 0 0\n0 1 0 0\n0 0 1 0\n10 20 30 1\n1 2 3 0.5 255 0 0\n0 0 0 0\n").unwrap();
        assert_eq!(scene.points.len(), 1);
        assert_eq!(scene.points[0].position, Vec3::new(11., 22., 33.));
    }

    fn pcd_header(encoding: &str) -> Vec<u8> {
        format!("# .PCD v0.7\nVERSION 0.7\nFIELDS x y z rgb\nSIZE 4 4 4 4\nTYPE F F F U\nCOUNT 1 1 1 1\nWIDTH 2\nHEIGHT 1\nPOINTS 2\nDATA {encoding}\n").into_bytes()
    }

    #[test]
    fn pcd_ascii_binary_and_compressed_field_major() {
        let mut ascii = pcd_header("ascii");
        ascii.extend_from_slice(b"1 2 3 16711680\n4 5 6 65280\n");
        let expected = pcd(&ascii).unwrap();
        assert_eq!(expected.points[0].color, Some([255, 0, 0]));
        let mut binary = pcd_header("binary");
        for (p, color) in [
            ([1_f32, 2., 3.], 0x00ff_0000_u32),
            ([4., 5., 6.], 0x0000_ff00),
        ] {
            for x in p {
                binary.extend_from_slice(&x.to_le_bytes());
            }
            binary.extend_from_slice(&color.to_le_bytes());
        }
        assert_eq!(
            pcd(&binary).unwrap().points[1].position,
            expected.points[1].position
        );
        assert!(pcd(&binary[..binary.len() - 1]).is_err());
        let mut field_major = Vec::new();
        for axis in [[1_f32, 4.], [2., 5.], [3., 6.]] {
            for x in axis {
                field_major.extend_from_slice(&x.to_le_bytes());
            }
        }
        for color in [0x00ff_0000_u32, 0x0000_ff00] {
            field_major.extend_from_slice(&color.to_le_bytes());
        }
        // LZF supports a literal run of up to 32 bytes.
        let mut compressed = pcd_header("binary_compressed");
        compressed.extend_from_slice(&33_u32.to_le_bytes());
        compressed.extend_from_slice(&32_u32.to_le_bytes());
        compressed.push(31);
        compressed.extend_from_slice(&field_major);
        let scene = pcd(&compressed).unwrap();
        assert_eq!(scene.points[1].position, expected.points[1].position);
        assert_eq!(scene.points[1].color, Some([0, 255, 0]));
    }

    #[test]
    fn lzf_overlapping_reference_and_invalid_reference() {
        assert_eq!(
            lzf(&[2, b'a', b'b', b'c', 0x80, 2], 9).unwrap(),
            b"abcabcabc"
        );
        assert!(lzf(&[0x80, 2], 6).is_err());
        assert!(lzf(&[31, b'a'], 32).is_err());
        assert!(lzf(&[0, b'a'], 0).is_err());
    }

    #[test]
    fn splat_centers_and_colors() {
        let mut record = [0; 32];
        record[..4].copy_from_slice(&2_f32.to_le_bytes());
        record[24..27].copy_from_slice(&[10, 20, 30]);
        let scene = splat(&record).unwrap();
        assert_eq!(scene.points[0].position, Vec3::new(2., 0., 0.));
        assert_eq!(scene.points[0].color, Some([10, 20, 30]));
        assert!(splat(&record[..31]).is_err());
    }
}
