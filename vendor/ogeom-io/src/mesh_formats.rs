//! Tessellated interchange: glTF 2.0, OBJ and PLY.
//!
//! Three formats, one philosophy carried over from the DXF writer: the
//! functions take bare tessellations rather than a model, so anything that
//! produces a [`Triangulation`] (a solid, one face, a healed import)
//! exports without this module knowing where it came from. glTF is written
//! as GLB, the single-file binary form, with positions, normals, indices
//! and an optional base colour per mesh; OBJ and PLY are the plain-text
//! dialects every downstream tool reads.
//!
//! # Reading is not writing backwards
//!
//! What this module writes is one of many shapes each format allows, and a
//! reader that assumed its own writer's choices would read almost nothing.
//! glTF is the sharp case: its geometry reaches the file through accessors
//! over buffer views over buffers, any of which may stride, offset, use any
//! of six component types, scale integers into fractions, or be overridden
//! piecewise by a sparse block, and the whole is placed by a node hierarchy
//! stated as a matrix or as translation, rotation and scale. All of that is
//! read. What is not read is refused by name rather than approximated.

use ogeom_core::ogeom_bail;
use ogeom_math::{Point, Vector};
use ogeom_topo::Triangulation;
use std::fmt::Write as _;

/// One mesh to export, with what the format can say about it.
#[derive(Debug, Clone)]
pub struct ExportMesh<'a> {
    /// The tessellation.
    pub mesh: &'a Triangulation,
    /// An RGBA base colour in `[0, 1]`, where the format carries one.
    pub colour: Option<[f64; 4]>,
    /// A name, where the format carries one.
    pub name: Option<String>,
}

impl<'a> ExportMesh<'a> {
    /// A bare mesh, no colour, no name.
    #[must_use]
    pub fn plain(mesh: &'a Triangulation) -> Self {
        Self {
            mesh,
            colour: None,
            name: None,
        }
    }
}

// --- glTF 2.0 (GLB) ----------------------------------------------------------

/// Write meshes as a GLB: glTF 2.0's single-file binary form.
///
/// One buffer, one node per mesh under one scene; positions and normals as
/// `f32` vectors, indices as `u32`, and a metallic-roughness material with
/// the base colour where one was given. Empty meshes are skipped; a node
/// with nothing to draw is not something a viewer should be handed.
#[must_use]
pub fn write_glb(meshes: &[ExportMesh<'_>]) -> Vec<u8> {
    let mut binary: Vec<u8> = Vec::new();
    let mut accessors = String::new();
    let mut buffer_views = String::new();
    let mut mesh_json = String::new();
    let mut node_json = String::new();
    let mut material_json = String::new();
    let mut accessor_count = 0_usize;
    let mut view_count = 0_usize;
    let mut mesh_count = 0_usize;
    let mut material_count = 0_usize;

    for export in meshes {
        let mesh = export.mesh;
        if mesh.triangles.is_empty() || mesh.positions.is_empty() {
            continue;
        }
        let comma = |s: &mut String| {
            if !s.is_empty() {
                s.push(',');
            }
        };

        // Positions.
        let position_offset = binary.len();
        let (mut low, mut high) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for p in &mesh.positions {
            for (k, v) in [p.x, p.y, p.z].into_iter().enumerate() {
                low[k] = low[k].min(v);
                high[k] = high[k].max(v);
                #[allow(clippy::cast_possible_truncation)]
                binary.extend_from_slice(&(v as f32).to_le_bytes());
            }
        }
        comma(&mut buffer_views);
        let _ = write!(
            buffer_views,
            r#"{{"buffer":0,"byteOffset":{position_offset},"byteLength":{}}}"#,
            binary.len() - position_offset
        );
        let position_view = view_count;
        view_count += 1;
        comma(&mut accessors);
        #[allow(clippy::cast_possible_truncation)]
        let (lo, hi) = (low.map(|v| v as f32), high.map(|v| v as f32));
        let _ = write!(
            accessors,
            r#"{{"bufferView":{position_view},"componentType":5126,"count":{},"type":"VEC3","min":[{},{},{}],"max":[{},{},{}]}}"#,
            mesh.positions.len(),
            lo[0],
            lo[1],
            lo[2],
            hi[0],
            hi[1],
            hi[2],
        );
        let position_accessor = accessor_count;
        accessor_count += 1;

        // Normals, normalized as glTF requires.
        let normal_offset = binary.len();
        for n in &mesh.normals {
            let magnitude = n.magnitude();
            let unit = if magnitude > 0.0 {
                [n.x / magnitude, n.y / magnitude, n.z / magnitude]
            } else {
                [0.0, 0.0, 1.0]
            };
            for v in unit {
                #[allow(clippy::cast_possible_truncation)]
                binary.extend_from_slice(&(v as f32).to_le_bytes());
            }
        }
        comma(&mut buffer_views);
        let _ = write!(
            buffer_views,
            r#"{{"buffer":0,"byteOffset":{normal_offset},"byteLength":{}}}"#,
            binary.len() - normal_offset
        );
        let normal_view = view_count;
        view_count += 1;
        comma(&mut accessors);
        let _ = write!(
            accessors,
            r#"{{"bufferView":{normal_view},"componentType":5126,"count":{},"type":"VEC3"}}"#,
            mesh.normals.len(),
        );
        let normal_accessor = accessor_count;
        accessor_count += 1;

        // Indices.
        let index_offset = binary.len();
        for t in &mesh.triangles {
            for &k in t {
                binary.extend_from_slice(&k.to_le_bytes());
            }
        }
        comma(&mut buffer_views);
        let _ = write!(
            buffer_views,
            r#"{{"buffer":0,"byteOffset":{index_offset},"byteLength":{}}}"#,
            binary.len() - index_offset
        );
        let index_view = view_count;
        view_count += 1;
        comma(&mut accessors);
        let _ = write!(
            accessors,
            r#"{{"bufferView":{index_view},"componentType":5125,"count":{},"type":"SCALAR"}}"#,
            mesh.triangles.len() * 3,
        );
        let index_accessor = accessor_count;
        accessor_count += 1;

        // Material, when a colour was given.
        let material = export.colour.map(|[r, g, b, a]| {
            comma(&mut material_json);
            let _ = write!(
                material_json,
                r#"{{"pbrMetallicRoughness":{{"baseColorFactor":[{r},{g},{b},{a}],"metallicFactor":0.1,"roughnessFactor":0.8}}}}"#,
            );
            material_count += 1;
            material_count - 1
        });

        comma(&mut mesh_json);
        let material_field = material.map_or(String::new(), |m| format!(r#","material":{m}"#));
        let _ = write!(
            mesh_json,
            r#"{{"primitives":[{{"attributes":{{"POSITION":{position_accessor},"NORMAL":{normal_accessor}}},"indices":{index_accessor}{material_field}}}]}}"#,
        );
        comma(&mut node_json);
        let name_field = export.name.as_ref().map_or(String::new(), |n| {
            format!(r#","name":"{}""#, escape_json(n))
        });
        let _ = write!(node_json, r#"{{"mesh":{mesh_count}{name_field}}}"#);
        mesh_count += 1;
    }

    let node_indices: Vec<String> = (0..mesh_count).map(|i| i.to_string()).collect();
    let json = format!(
        r#"{{"asset":{{"version":"2.0","generator":"ogeom"}},"scene":0,"scenes":[{{"nodes":[{}]}}],"nodes":[{node_json}],"meshes":[{mesh_json}],"materials":[{material_json}],"accessors":[{accessors}],"bufferViews":[{buffer_views}],"buffers":[{{"byteLength":{}}}]}}"#,
        node_indices.join(","),
        binary.len(),
    );
    // A colour-free file carries no materials array worth having.
    let json = json.replace(r#","materials":[],"#, ",");

    // GLB framing: 4-byte alignment, JSON padded with spaces, binary with
    // zeros.
    let mut json_bytes = json.into_bytes();
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    while !binary.len().is_multiple_of(4) {
        binary.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + binary.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&0x4654_6C67_u32.to_le_bytes()); // "glTF"
    out.extend_from_slice(&2_u32.to_le_bytes());
    out.extend_from_slice(&u32::try_from(total).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(json_bytes.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(&0x4E4F_534A_u32.to_le_bytes()); // "JSON"
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(
        &u32::try_from(binary.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(&0x004E_4942_u32.to_le_bytes()); // "BIN\0"
    out.extend_from_slice(&binary);
    out
}

fn escape_json(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            c if c.is_control() => format!("\\u{:04x}", c as u32).chars().collect(),
            c => vec![c],
        })
        .collect()
}

// --- OBJ ---------------------------------------------------------------------

/// Write meshes as Wavefront OBJ.
///
/// `v`/`vn`/`f` records, one `o` group per named mesh, indices one-based
/// and shared across the file as the format demands.
#[must_use]
pub fn write_obj(meshes: &[ExportMesh<'_>]) -> String {
    let mut out = String::from("# ogeom\n");
    let mut base = 1_usize;
    for (i, export) in meshes.iter().enumerate() {
        let mesh = export.mesh;
        if mesh.triangles.is_empty() {
            continue;
        }
        let name = export.name.clone().unwrap_or_else(|| format!("mesh-{i}"));
        let _ = writeln!(out, "o {name}");
        for p in &mesh.positions {
            let _ = writeln!(out, "v {} {} {}", p.x, p.y, p.z);
        }
        for n in &mesh.normals {
            let _ = writeln!(out, "vn {} {} {}", n.x, n.y, n.z);
        }
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|k| k as usize + base);
            let _ = writeln!(out, "f {a}//{a} {b}//{b} {c}//{c}");
        }
        base += mesh.positions.len();
    }
    out
}

// --- PLY ---------------------------------------------------------------------

/// Write one mesh as ASCII PLY.
///
/// Vertices with normals, faces as index lists; a colour, when given, is
/// carried per vertex as the `uchar` triple the format convention expects.
#[must_use]
pub fn write_ply(export: &ExportMesh<'_>) -> String {
    let mesh = export.mesh;
    let colour = export.colour.map(|[r, g, b, _]| {
        [
            (r.clamp(0.0, 1.0) * 255.0).round(),
            (g.clamp(0.0, 1.0) * 255.0).round(),
            (b.clamp(0.0, 1.0) * 255.0).round(),
        ]
    });
    let mut out = String::from("ply\nformat ascii 1.0\ncomment ogeom\n");
    let _ = writeln!(out, "element vertex {}", mesh.positions.len());
    out.push_str(
        "property float x\nproperty float y\nproperty float z\n\
         property float nx\nproperty float ny\nproperty float nz\n",
    );
    if colour.is_some() {
        out.push_str("property uchar red\nproperty uchar green\nproperty uchar blue\n");
    }
    let _ = writeln!(out, "element face {}", mesh.triangles.len());
    out.push_str("property list uchar uint vertex_indices\nend_header\n");
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        let _ = write!(out, "{} {} {} {} {} {}", p.x, p.y, p.z, n.x, n.y, n.z);
        if let Some([r, g, b]) = colour {
            let _ = write!(out, " {r} {g} {b}");
        }
        out.push('\n');
    }
    for t in &mesh.triangles {
        let _ = writeln!(out, "3 {} {} {}", t[0], t[1], t[2]);
    }
    out
}

// --- reading -----------------------------------------------------------------

/// Read an OBJ into a triangulation.
///
/// Vertices, faces and vertex normals; everything else (materials, groups,
/// texture coordinates, smoothing) is a statement about *rendering* a mesh
/// rather than about the mesh, and is skipped rather than half-honoured. A
/// face of more than three vertices is fanned from its first, which is what
/// a convex polygon means and what OBJ writers emit.
///
/// Indices may be negative, which in OBJ counts back from the end, and may
/// carry the `v/vt/vn` triple, of which the first field is the position.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) if an
/// index names a vertex that is not there, or a coordinate does not parse.
pub fn read_obj(text: &str) -> ogeom_core::OgeomResult<Triangulation> {
    let mut positions: Vec<ogeom_math::Point> = Vec::new();
    let mut normals: Vec<ogeom_math::Vector> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut normal_of: Vec<Option<usize>> = Vec::new();

    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("v") => {
                let coords = triple(&mut fields, "a vertex")?;
                positions.push(ogeom_math::Point::new(coords[0], coords[1], coords[2]));
                normal_of.push(None);
            }
            Some("vn") => {
                let coords = triple(&mut fields, "a normal")?;
                normals.push(ogeom_math::Vector::new(coords[0], coords[1], coords[2]));
            }
            Some("f") => {
                let mut corners: Vec<(usize, Option<usize>)> = Vec::new();
                for field in fields {
                    corners.push(corner(field, positions.len(), normals.len())?);
                }
                if corners.len() < 3 {
                    ogeom_core::ogeom_bail!(
                        Construction,
                        "a face of {} corners is not a face",
                        corners.len()
                    );
                }
                for i in 1..corners.len() - 1 {
                    let fan = [corners[0], corners[i], corners[i + 1]];
                    let mut indices = [0_u32; 3];
                    for (k, (vertex, normal)) in fan.iter().enumerate() {
                        if let Some(n) = normal {
                            normal_of[*vertex] = Some(*n);
                        }
                        indices[k] = u32::try_from(*vertex).unwrap_or(u32::MAX);
                    }
                    triangles.push(indices);
                }
            }
            _ => {}
        }
    }
    if positions.is_empty() {
        ogeom_core::ogeom_bail!(Construction, "the file carries no vertices");
    }
    // A vertex whose normal the file did not give takes the average of the
    // triangles it belongs to, the same answer the writer would have had.
    let mut resolved = vec![ogeom_math::Vector::ZERO; positions.len()];
    for (i, held) in normal_of.iter().enumerate() {
        if let Some(n) = held.and_then(|n| normals.get(n)) {
            resolved[i] = *n;
        }
    }
    for triangle in &triangles {
        let [a, b, c] = triangle.map(|i| positions[i as usize]);
        let face = (b - a).cross(c - a);
        for index in triangle {
            let slot = &mut resolved[*index as usize];
            if held_is_unset(slot) {
                *slot += face;
            }
        }
    }
    let normals = resolved
        .into_iter()
        .map(|n| {
            let m = n.magnitude();
            if m > 0.0 {
                n / m
            } else {
                ogeom_math::Vector::Z
            }
        })
        .collect::<Vec<_>>();
    let parameters = vec![(0.0, 0.0); positions.len()];
    Ok(Triangulation {
        positions,
        normals,
        parameters,
        triangles,
        deflection_met: true,
    })
}

/// Whether a normal slot is still waiting to be accumulated into.
///
/// A normal the file gave is a unit vector; a slot nobody has filled starts
/// at zero and grows by the faces around it. Telling them apart by length is
/// enough, and it means a file that gives *some* normals keeps them while
/// the rest are worked out.
fn held_is_unset(slot: &ogeom_math::Vector) -> bool {
    !(0.999..=1.001).contains(&slot.magnitude())
}

/// Three floats off an iterator.
fn triple<'a>(
    fields: &mut impl Iterator<Item = &'a str>,
    what: &str,
) -> ogeom_core::OgeomResult<[f64; 3]> {
    let mut out = [0.0; 3];
    for slot in &mut out {
        let Some(field) = fields.next() else {
            ogeom_core::ogeom_bail!(Construction, "{what} needs three coordinates");
        };
        let Ok(value) = field.parse::<f64>() else {
            ogeom_core::ogeom_bail!(
                Construction,
                "{what} carries {field}, which is not a number"
            );
        };
        *slot = value;
    }
    Ok(out)
}

/// One OBJ face corner: `v`, `v/vt`, `v//vn` or `v/vt/vn`, one-based, and
/// negative counting back from the end.
fn corner(
    field: &str,
    vertices: usize,
    normals: usize,
) -> ogeom_core::OgeomResult<(usize, Option<usize>)> {
    let mut parts = field.split('/');
    let resolve = |text: &str, count: usize| -> Option<usize> {
        let index = text.parse::<isize>().ok()?;
        let zero = if index > 0 {
            usize::try_from(index).ok()?.checked_sub(1)?
        } else if index < 0 {
            count.checked_sub(usize::try_from(-index).ok()?)?
        } else {
            return None;
        };
        (zero < count).then_some(zero)
    };
    let Some(vertex) = parts.next().and_then(|t| resolve(t, vertices)) else {
        ogeom_core::ogeom_bail!(
            Construction,
            "a face names vertex {field}, which is not there"
        );
    };
    let _texture = parts.next();
    let normal = parts.next().and_then(|t| resolve(t, normals));
    Ok((vertex, normal))
}

/// Read an ASCII PLY into a triangulation.
///
/// The element order the header declares, the properties it names, and the
/// faces' own vertex lists. Binary PLY is refused by name: its header says
/// `format binary_little_endian` and this reads `format ascii`, which is
/// what every writer here emits and what the format's own text dialect is.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) if the
/// header is not a PLY header, the format is binary, the body does not
/// match what the header promised, or a face names a vertex that is not
/// there.
pub fn read_ply(text: &str) -> ogeom_core::OgeomResult<Triangulation> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("ply") {
        ogeom_core::ogeom_bail!(Construction, "this is not a PLY file");
    }
    // The header: element counts and, per element, its properties in order.
    let mut counts: Vec<(String, usize, Vec<String>)> = Vec::new();
    for line in lines.by_ref() {
        let line = line.trim();
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("format") => {
                let kind = fields.next().unwrap_or("");
                if kind != "ascii" {
                    ogeom_core::ogeom_bail!(
                        Construction,
                        "this PLY is {kind}; the text dialect is what is read here"
                    );
                }
            }
            Some("element") => {
                let name = fields.next().unwrap_or("").to_string();
                let count = fields
                    .next()
                    .and_then(|t| t.parse::<usize>().ok())
                    .unwrap_or(0);
                counts.push((name, count, Vec::new()));
            }
            Some("property") => {
                if let Some((_, _, properties)) = counts.last_mut() {
                    let last = line.split_whitespace().last().unwrap_or("");
                    properties.push(last.to_string());
                }
            }
            Some("end_header") => break,
            _ => {}
        }
    }

    let mut positions: Vec<ogeom_math::Point> = Vec::new();
    let mut normals: Vec<ogeom_math::Vector> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut body = lines.filter(|l| !l.trim().is_empty());
    for (name, count, properties) in &counts {
        for _ in 0..*count {
            let Some(line) = body.next() else {
                ogeom_core::ogeom_bail!(
                    Construction,
                    "the header promised {count} of {name} and the body ran out"
                );
            };
            let fields: Vec<&str> = line.split_whitespace().collect();
            if name == "vertex" {
                let read = |what: &str| -> Option<f64> {
                    let at = properties.iter().position(|p| p == what)?;
                    fields.get(at)?.parse::<f64>().ok()
                };
                let (Some(x), Some(y), Some(z)) = (read("x"), read("y"), read("z")) else {
                    ogeom_core::ogeom_bail!(Construction, "a vertex is missing a coordinate");
                };
                positions.push(ogeom_math::Point::new(x, y, z));
                normals.push(match (read("nx"), read("ny"), read("nz")) {
                    (Some(a), Some(b), Some(c)) => ogeom_math::Vector::new(a, b, c),
                    _ => ogeom_math::Vector::ZERO,
                });
            } else if name == "face" {
                let Some(count) = fields.first().and_then(|t| t.parse::<usize>().ok()) else {
                    ogeom_core::ogeom_bail!(Construction, "a face does not say how many corners");
                };
                let corners: Vec<u32> = fields
                    .iter()
                    .skip(1)
                    .take(count)
                    .filter_map(|t| t.parse::<u32>().ok())
                    .collect();
                if corners.len() != count {
                    ogeom_core::ogeom_bail!(
                        Construction,
                        "a face of {count} corners lists {} of them",
                        corners.len()
                    );
                }
                for i in 1..corners.len().saturating_sub(1) {
                    triangles.push([corners[0], corners[i], corners[i + 1]]);
                }
            }
        }
    }
    if positions.is_empty() {
        ogeom_core::ogeom_bail!(Construction, "the file carries no vertices");
    }
    if let Some(past) = triangles
        .iter()
        .flatten()
        .find(|&&v| v as usize >= positions.len())
    {
        ogeom_core::ogeom_bail!(
            Construction,
            "a face names vertex {past}, past the {} the file has",
            positions.len()
        );
    }
    // Normals the file did not give come from the triangles, as in OBJ.
    let mut resolved = normals;
    resolved.resize(positions.len(), ogeom_math::Vector::ZERO);
    for triangle in &triangles {
        let [a, b, c] = triangle.map(|i| positions[i as usize]);
        let face = (b - a).cross(c - a);
        for index in triangle {
            let slot = &mut resolved[*index as usize];
            if held_is_unset(slot) {
                *slot += face;
            }
        }
    }
    let normals = resolved
        .into_iter()
        .map(|n| {
            let m = n.magnitude();
            if m > 0.0 {
                n / m
            } else {
                ogeom_math::Vector::Z
            }
        })
        .collect::<Vec<_>>();
    let parameters = vec![(0.0, 0.0); positions.len()];
    Ok(Triangulation {
        positions,
        normals,
        parameters,
        triangles,
        deflection_met: true,
    })
}

// --- VRML --------------------------------------------------------------------

/// Write meshes as VRML 97.
///
/// One `Shape` per mesh, each an `IndexedFaceSet` over its own coordinates,
/// with a material where a colour was given. Written rather than read: VRML
/// is a scene-description language with a great deal in it that is not
/// geometry, and a reader that took only the geometry would be claiming
/// more than it did.
#[must_use]
pub fn write_vrml(meshes: &[ExportMesh<'_>]) -> String {
    let mut out = String::from("#VRML V2.0 utf8\n\n");
    for export in meshes {
        if export.mesh.triangles.is_empty() {
            continue;
        }
        if let Some(name) = &export.name {
            let _ = writeln!(out, "# {name}");
        }
        out.push_str("Shape {\n");
        if let Some([r, g, b, a]) = export.colour {
            out.push_str("  appearance Appearance {\n    material Material {\n");
            let _ = writeln!(out, "      diffuseColor {r} {g} {b}");
            if a < 1.0 {
                let _ = writeln!(out, "      transparency {}", 1.0 - a);
            }
            out.push_str("    }\n  }\n");
        }
        out.push_str("  geometry IndexedFaceSet {\n    coord Coordinate {\n      point [\n");
        for p in &export.mesh.positions {
            let _ = writeln!(out, "        {} {} {},", p.x, p.y, p.z);
        }
        out.push_str("      ]\n    }\n    coordIndex [\n");
        for [a, b, c] in &export.mesh.triangles {
            let _ = writeln!(out, "      {a} {b} {c} -1,");
        }
        out.push_str("    ]\n    solid TRUE\n  }\n}\n\n");
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use ogeom_core::Tolerances;
    use ogeom_math::Frame;
    use ogeom_topo::Model;

    const T: Tolerances = Tolerances::millimetres();

    fn box_mesh() -> Triangulation {
        let mut model = Model::new();
        let solid = ogeom_algo::make_box(&mut model, Frame::WORLD, (2.0, 3.0, 4.0), T)
            .unwrap()
            .shape;
        ogeom_mesh::triangulate(&model, &solid, ogeom_mesh::Deflection::default(), T).unwrap()
    }

    #[test]
    fn a_glb_frames_its_chunks_and_counts_its_geometry() {
        let mesh = box_mesh();
        let glb = write_glb(&[ExportMesh {
            mesh: &mesh,
            colour: Some([0.8, 0.2, 0.1, 1.0]),
            name: Some("box".into()),
        }]);
        // Magic, version, and a total length that matches the file.
        assert_eq!(&glb[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(glb[4..8].try_into().unwrap()), 2);
        assert_eq!(
            u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize,
            glb.len()
        );
        // The JSON chunk parses far enough to carry the structure.
        let json_length = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        assert_eq!(&glb[16..20], b"JSON");
        let json = core::str::from_utf8(&glb[20..20 + json_length]).unwrap();
        assert!(json.contains(r#""version":"2.0""#));
        assert!(json.contains(r#""POSITION":0"#));
        assert!(json.contains(r#""indices":2"#));
        assert!(json.contains("baseColorFactor"));
        assert!(json.contains(r#""name":"box""#));
        assert!(json.contains(r#""min":["#), "POSITION carries bounds");
        // The binary chunk holds what the accessors promise: positions,
        // normals, indices.
        let expected =
            mesh.positions.len() * 12 + mesh.normals.len() * 12 + mesh.triangles.len() * 12;
        let padded = expected + (4 - expected % 4) % 4;
        let binary_length =
            u32::from_le_bytes(glb[20 + json_length..24 + json_length].try_into().unwrap());
        assert_eq!(binary_length as usize, padded);
    }

    #[test]
    fn an_obj_counts_its_records_and_shares_its_index_space() {
        let mesh = box_mesh();
        let one = ExportMesh::plain(&mesh);
        let text = write_obj(&[one.clone(), one]);
        assert_eq!(
            text.lines().filter(|l| l.starts_with("v ")).count(),
            mesh.positions.len() * 2
        );
        assert_eq!(
            text.lines().filter(|l| l.starts_with("f ")).count(),
            mesh.triangles.len() * 2
        );
        // The second mesh's faces reference the second mesh's vertices.
        let last_face = text.lines().rev().find(|l| l.starts_with("f ")).unwrap();
        let first_index: usize = last_face
            .split_whitespace()
            .nth(1)
            .unwrap()
            .split('/')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(first_index > mesh.positions.len());
    }

    #[test]
    fn a_ply_declares_what_it_carries() {
        let mesh = box_mesh();
        let text = write_ply(&ExportMesh {
            mesh: &mesh,
            colour: Some([0.0, 0.5, 1.0, 1.0]),
            name: None,
        });
        assert!(text.starts_with("ply\nformat ascii 1.0\n"));
        assert!(text.contains(&format!("element vertex {}", mesh.positions.len())));
        assert!(text.contains(&format!("element face {}", mesh.triangles.len())));
        assert!(text.contains("property uchar red"));
        let body_faces = text
            .lines()
            .skip_while(|l| *l != "end_header")
            .filter(|l| l.starts_with("3 "))
            .count();
        assert_eq!(body_faces, mesh.triangles.len());
    }
}

// --- glTF 2.0, reading -------------------------------------------------------

/// One mesh read back out of a glTF document.
///
/// The owning counterpart of [`ExportMesh`]: what the file said, with the
/// node transform that placed it already applied to the geometry, so the
/// positions are where the scene puts them.
#[derive(Debug, Clone)]
pub struct ImportedMesh {
    /// The tessellation, in scene coordinates.
    pub mesh: Triangulation,
    /// The base colour of the primitive's material, where it had one.
    pub colour: Option<[f64; 4]>,
    /// The node's name, where it had one.
    pub name: Option<String>,
}

/// Read a GLB: glTF 2.0's single-file binary form.
///
/// The whole indirection is honoured, because a writer chooses it and a
/// reader does not get to assume: a primitive names accessors, an accessor
/// names a buffer view and a component type, a view names a buffer and may
/// stride over it, and any of them may be replaced piecewise by a *sparse*
/// block. Every component type the standard defines is read (signed and
/// unsigned bytes, shorts, unsigned ints and floats), with `normalized`
/// honoured where it is set, which is the difference between a colour of
/// `255` and a colour of `1`.
///
/// The scene's node hierarchy is walked and each node's transform composed,
/// stated as a matrix or as translation, rotation and scale; the geometry
/// comes back placed. Normals are carried through the transform's inverse
/// transpose, which is what keeps them normal to a scaled surface.
///
/// What is *not* read is refused rather than approximated: a primitive whose
/// mode is not triangles, a buffer that points at an external file this
/// function cannot open, and a `KHR_draco_mesh_compression` payload, each by
/// name.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) for a
/// file whose framing, JSON or indices do not hold together, and for the
/// refusals above.
pub fn read_glb(bytes: &[u8]) -> ogeom_core::OgeomResult<Vec<ImportedMesh>> {
    let (json, binary) = split_glb(bytes)?;
    let document = crate::json::parse(&json)?;
    read_gltf_document(&document, binary.as_deref())
}

/// Read a `.gltf`: the JSON form, whose buffers are data URIs.
///
/// A buffer with an external `uri` is refused by name: this function is handed
/// bytes, not a directory, and quietly producing a mesh with no positions
/// would be worse than saying so.
///
/// # Errors
///
/// As [`read_glb`].
pub fn read_gltf(text: &str) -> ogeom_core::OgeomResult<Vec<ImportedMesh>> {
    let document = crate::json::parse(text)?;
    read_gltf_document(&document, None)
}

/// The JSON chunk and the binary chunk of a GLB.
fn split_glb(bytes: &[u8]) -> ogeom_core::OgeomResult<(String, Option<Vec<u8>>)> {
    let word = |at: usize| -> ogeom_core::OgeomResult<u32> {
        let Some(slice) = bytes.get(at..at + 4) else {
            ogeom_bail!(Construction, "the file ends inside its own header");
        };
        Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
    };
    if word(0)? != 0x4654_6C67 {
        ogeom_bail!(
            Construction,
            "this is not a GLB: the first four bytes are not `glTF`"
        );
    }
    let version = word(4)?;
    if version != 2 {
        ogeom_bail!(Construction, "GLB version {version} is not glTF 2.0");
    }
    let mut json = None;
    let mut binary = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let length = word(at)? as usize;
        let kind = word(at + 4)?;
        let start = at + 8;
        let Some(chunk) = bytes.get(start..start + length) else {
            ogeom_bail!(
                Construction,
                "a chunk claims {length} bytes it does not have"
            );
        };
        match kind {
            0x4E4F_534A => {
                let Ok(text) = core::str::from_utf8(chunk) else {
                    ogeom_bail!(Construction, "the JSON chunk is not UTF-8");
                };
                json = Some(text.trim_end_matches(['\0', ' ']).to_owned());
            }
            0x004E_4942 => binary = Some(chunk.to_vec()),
            // The standard says an unknown chunk is skipped, not refused.
            _ => {}
        }
        at = start + length.next_multiple_of(4);
    }
    let Some(json) = json else {
        ogeom_bail!(Construction, "the GLB carries no JSON chunk");
    };
    Ok((json, binary))
}

/// One buffer's bytes, from the binary chunk or a data URI.
fn buffers(
    document: &crate::json::Json,
    binary: Option<&[u8]>,
) -> ogeom_core::OgeomResult<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    for buffer in document.get("buffers").map(|b| b.items()).unwrap_or(&[]) {
        match buffer.get("uri").and_then(crate::json::Json::text) {
            None => {
                // The GLB's own binary chunk: only the first buffer may take
                // it, which is what "the buffer with no uri" means.
                let Some(chunk) = binary else {
                    ogeom_bail!(
                        Construction,
                        "a buffer with no uri wants the GLB's binary chunk, and \
                         there is none"
                    );
                };
                out.push(chunk.to_vec());
            }
            Some(uri) if uri.starts_with("data:") => {
                let Some((_, payload)) = uri.split_once("base64,") else {
                    ogeom_bail!(
                        Construction,
                        "a data uri that is not base64 is not something this reads"
                    );
                };
                out.push(from_base64(payload)?);
            }
            Some(uri) => ogeom_bail!(
                Construction,
                "the buffer points at `{uri}`, an external file this reader is \
                 not given; hand it a GLB or a document with data uris"
            ),
        }
    }
    Ok(out)
}

/// Standard base64, padding tolerated and whitespace ignored.
fn from_base64(text: &str) -> ogeom_core::OgeomResult<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0_u32;
    let mut held = 0_u32;
    for c in text.bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let Some(v) = value(c) else {
            ogeom_bail!(Construction, "`{}` is not base64", c as char);
        };
        acc = (acc << 6) | v;
        held += 6;
        if held >= 8 {
            held -= 8;
            #[allow(clippy::cast_possible_truncation, reason = "masked to a byte")]
            out.push(((acc >> held) & 0xFF) as u8);
        }
    }
    Ok(out)
}

/// The whole document: buffers, then every mesh the scene's nodes place.
fn read_gltf_document(
    document: &crate::json::Json,
    binary: Option<&[u8]>,
) -> ogeom_core::OgeomResult<Vec<ImportedMesh>> {
    use crate::json::Json;
    let buffers = buffers(document, binary)?;
    let views = document.get("bufferViews").map_or(&[][..], Json::items);
    let accessors = document.get("accessors").map_or(&[][..], Json::items);
    let meshes = document.get("meshes").map_or(&[][..], Json::items);
    let nodes = document.get("nodes").map_or(&[][..], Json::items);
    let materials = document.get("materials").map_or(&[][..], Json::items);

    // The nodes the scene names, or (for a document with no scene at all)
    // every node, which is what a reader can honestly do with one.
    let scene = document
        .index_at("scene")
        .and_then(|i| document.get("scenes")?.items().get(i))
        .and_then(|s| s.get("nodes"))
        .map(Json::items);
    let roots: Vec<usize> = match scene {
        Some(list) => list.iter().filter_map(Json::index).collect(),
        None => (0..nodes.len()).collect(),
    };

    let mut out = Vec::new();
    let mut pending: Vec<(usize, Placement)> = roots
        .into_iter()
        .rev()
        .map(|i| (i, Placement::IDENTITY))
        .collect();
    let mut seen = vec![false; nodes.len()];
    while let Some((index, parent)) = pending.pop() {
        let Some(node) = nodes.get(index) else {
            ogeom_bail!(Construction, "node {index} is not in this document");
        };
        // A cycle in the node graph is a broken document, and walking it
        // forever is not a better answer than saying so.
        if seen.get(index).copied().unwrap_or(false) {
            ogeom_bail!(Construction, "node {index} is its own descendant");
        }
        if let Some(flag) = seen.get_mut(index) {
            *flag = true;
        }
        let here = parent.then(node_transform(node)?);
        for child in node.get("children").map_or(&[][..], Json::items) {
            let Some(child) = child.index() else {
                ogeom_bail!(Construction, "a child index is not an index");
            };
            pending.push((child, here));
        }
        let Some(mesh_index) = node.index_at("mesh") else {
            continue;
        };
        let Some(mesh) = meshes.get(mesh_index) else {
            ogeom_bail!(Construction, "mesh {mesh_index} is not in this document");
        };
        let name = node
            .get("name")
            .and_then(Json::text)
            .or_else(|| mesh.get("name").and_then(Json::text))
            .map(str::to_owned);
        for primitive in mesh.get("primitives").map_or(&[][..], Json::items) {
            let built = read_primitive(primitive, accessors, views, &buffers, materials, here)?;
            if let Some((mesh, colour)) = built {
                out.push(ImportedMesh {
                    mesh,
                    colour,
                    name: name.clone(),
                });
            }
        }
    }
    Ok(out)
}

/// A node's placement: three basis columns and a translation.
///
/// Not the kernel's own `Transform`, and deliberately: a glTF node may scale
/// unevenly, which is not a placement at all: it carries a circle to an
/// ellipse. What comes out of a glTF file is a *mesh*, where an uneven scale
/// is nothing worse than three multiplications, so the reader carries the
/// affine map plainly and applies it to points and normals.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Placement {
    pub(crate) columns: [Vector; 3],
    pub(crate) translation: Vector,
}

impl Placement {
    pub(crate) const IDENTITY: Self = Self {
        columns: [
            Vector::new(1.0, 0.0, 0.0),
            Vector::new(0.0, 1.0, 0.0),
            Vector::new(0.0, 0.0, 1.0),
        ],
        translation: Vector::new(0.0, 0.0, 0.0),
    };

    /// `self` after `inner`: the child's own map applied first.
    pub(crate) fn then(self, inner: Self) -> Self {
        let map = |v: Vector| self.columns[0] * v.x + self.columns[1] * v.y + self.columns[2] * v.z;
        Self {
            columns: inner.columns.map(map),
            translation: map(inner.translation) + self.translation,
        }
    }

    pub(crate) fn point(self, p: Point) -> Point {
        Point::ORIGIN
            + self.columns[0] * p.x
            + self.columns[1] * p.y
            + self.columns[2] * p.z
            + self.translation
    }

    /// Whether the map is a reflection: it turns a triangle's winding
    /// against its normal.
    pub(crate) fn mirrors(self) -> bool {
        let [a, b, c] = self.columns;
        a.cross(b).dot(c) < 0.0
    }

    /// A normal carried through: the inverse transpose, which is what keeps a
    /// normal normal to a surface an uneven scale has stretched.
    ///
    /// Built from the cofactors, which *is* the inverse transpose times the
    /// determinant. A normal is renormalized anyway, so only the
    /// determinant's sign matters, and the singular case does not divide.
    fn normal(self, n: Vector) -> Vector {
        let [a, b, c] = self.columns;
        let cofactors = [b.cross(c), c.cross(a), a.cross(b)];
        let out = cofactors[0] * n.x + cofactors[1] * n.y + cofactors[2] * n.z;
        let magnitude = out.magnitude();
        if magnitude == 0.0 {
            n
        } else if self.mirrors() {
            -out / magnitude
        } else {
            out / magnitude
        }
    }
}

/// A node's own transform: a matrix, or translation, rotation and scale.
fn node_transform(node: &crate::json::Json) -> ogeom_core::OgeomResult<Placement> {
    use crate::json::Json;
    let triple = |value: &Json, what: &str| -> ogeom_core::OgeomResult<[f64; 3]> {
        let v: Vec<f64> = value.items().iter().filter_map(Json::number).collect();
        let [x, y, z] = v[..] else {
            ogeom_bail!(Construction, "a node {what} has three numbers");
        };
        Ok([x, y, z])
    };
    if let Some(matrix) = node.get("matrix") {
        let values: Vec<f64> = matrix.items().iter().filter_map(Json::number).collect();
        if values.len() != 16 {
            ogeom_bail!(Construction, "a node matrix has sixteen numbers");
        }
        // Column-major, as the standard states it.
        return Ok(Placement {
            columns: [
                Vector::new(values[0], values[1], values[2]),
                Vector::new(values[4], values[5], values[6]),
                Vector::new(values[8], values[9], values[10]),
            ],
            translation: Vector::new(values[12], values[13], values[14]),
        });
    }
    // Scale first, then rotate, then translate: the order the standard sets.
    let mut placement = Placement::IDENTITY;
    if let Some(scale) = node.get("scale") {
        let [x, y, z] = triple(scale, "scale")?;
        placement = Placement {
            columns: [
                Vector::new(x, 0.0, 0.0),
                Vector::new(0.0, y, 0.0),
                Vector::new(0.0, 0.0, z),
            ],
            translation: Vector::new(0.0, 0.0, 0.0),
        }
        .then(placement);
    }
    if let Some(rotation) = node.get("rotation") {
        let q: Vec<f64> = rotation.items().iter().filter_map(Json::number).collect();
        let [x, y, z, w] = q[..] else {
            ogeom_bail!(Construction, "a node rotation is a quaternion of four");
        };
        placement = quaternion_placement(x, y, z, w).then(placement);
    }
    if let Some(translation) = node.get("translation") {
        let [x, y, z] = triple(translation, "translation")?;
        placement = Placement {
            translation: Vector::new(x, y, z),
            ..Placement::IDENTITY
        }
        .then(placement);
    }
    Ok(placement)
}

/// The rotation a glTF quaternion `(x, y, z, w)` names.
fn quaternion_placement(x: f64, y: f64, z: f64, w: f64) -> Placement {
    let n = x.mul_add(x, y.mul_add(y, z.mul_add(z, w * w))).sqrt();
    let (x, y, z, w) = if n > 0.0 {
        (x / n, y / n, z / n, w / n)
    } else {
        (0.0, 0.0, 0.0, 1.0)
    };
    Placement {
        columns: [
            Vector::new(
                2.0f64.mul_add(-y.mul_add(y, z * z), 1.0),
                2.0 * x.mul_add(y, z * w),
                2.0 * x.mul_add(z, -(y * w)),
            ),
            Vector::new(
                2.0 * x.mul_add(y, -(z * w)),
                2.0f64.mul_add(-x.mul_add(x, z * z), 1.0),
                2.0 * y.mul_add(z, x * w),
            ),
            Vector::new(
                2.0 * x.mul_add(z, y * w),
                2.0 * y.mul_add(z, -(x * w)),
                2.0f64.mul_add(-x.mul_add(x, y * y), 1.0),
            ),
        ],
        translation: Vector::new(0.0, 0.0, 0.0),
    }
}

/// One primitive as a triangulation, placed by its node.
///
/// `None` where the primitive draws nothing (no positions, or no triangles
/// once the indices are read), which is a thing a document may legitimately
/// contain and not a thing to hand on as a mesh.
fn read_primitive(
    primitive: &crate::json::Json,
    accessors: &[crate::json::Json],
    views: &[crate::json::Json],
    buffers: &[Vec<u8>],
    materials: &[crate::json::Json],
    placement: Placement,
) -> ogeom_core::OgeomResult<Option<(Triangulation, Option<[f64; 4]>)>> {
    use crate::json::Json;
    if primitive
        .get("extensions")
        .and_then(|e| e.get("KHR_draco_mesh_compression"))
        .is_some()
    {
        ogeom_bail!(
            Construction,
            "this primitive's geometry is Draco-compressed, which is a codec \
             this reader does not carry"
        );
    }
    // The default mode is 4, triangles; anything else draws something a
    // triangulation is not, and fanning a strip here would be inventing.
    let mode = primitive.index_at("mode").unwrap_or(4);
    if mode != 4 {
        ogeom_bail!(
            Construction,
            "primitive mode {mode} is not triangles; only triangle meshes read \
             back as a triangulation"
        );
    }
    let Some(attributes) = primitive.get("attributes") else {
        return Ok(None);
    };
    let Some(position_index) = attributes.index_at("POSITION") else {
        return Ok(None);
    };
    let positions_raw = read_accessor(position_index, accessors, views, buffers)?;
    if positions_raw.components != 3 {
        ogeom_bail!(Construction, "POSITION is a VEC3");
    }
    let positions: Vec<Point> = positions_raw
        .values
        .as_chunks::<3>()
        .0
        .iter()
        .map(|v| placement.point(Point::new(v[0], v[1], v[2])))
        .collect();

    let normals: Vec<Vector> = match attributes.index_at("NORMAL") {
        None => Vec::new(),
        Some(index) => {
            let raw = read_accessor(index, accessors, views, buffers)?;
            if raw.components != 3 {
                ogeom_bail!(Construction, "NORMAL is a VEC3");
            }
            if raw.count != positions.len() {
                ogeom_bail!(
                    Construction,
                    "the primitive has {} positions and {} normals",
                    positions.len(),
                    raw.count
                );
            }
            raw.values
                .as_chunks::<3>()
                .0
                .iter()
                .map(|v| placement.normal(Vector::new(v[0], v[1], v[2])))
                .collect()
        }
    };

    let indices: Vec<u32> = match primitive.index_at("indices") {
        // A primitive with no indices draws its vertices in order, three at a
        // time, which the standard says and a reader has to honour.
        None => (0..u32::try_from(positions.len()).unwrap_or(u32::MAX)).collect(),
        Some(index) => {
            let raw = read_accessor(index, accessors, views, buffers)?;
            if raw.components != 1 {
                ogeom_bail!(Construction, "an index accessor is a SCALAR");
            }
            let mut out = Vec::with_capacity(raw.values.len());
            for v in raw.values {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "range-checked against the vertex count below"
                )]
                let k = v as u32;
                if f64::from(k) != v || (k as usize) >= positions.len() {
                    ogeom_bail!(
                        Construction,
                        "index {v} names no vertex among {}",
                        positions.len()
                    );
                }
                out.push(k);
            }
            out
        }
    };
    // A mirroring node turns the winding inside out; turn it back.
    let mirrored = placement.mirrors();
    let triangles: Vec<[u32; 3]> = indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| {
            if mirrored {
                [c[0], c[2], c[1]]
            } else {
                [c[0], c[1], c[2]]
            }
        })
        .collect();
    if triangles.is_empty() {
        return Ok(None);
    }

    let colour = primitive
        .index_at("material")
        .and_then(|i| materials.get(i))
        .and_then(|m| m.get("pbrMetallicRoughness"))
        .and_then(|p| p.get("baseColorFactor"))
        .and_then(|f| {
            let v: Vec<f64> = f.items().iter().filter_map(Json::number).collect();
            <[f64; 4]>::try_from(v).ok()
        });

    // A mesh with no normals given gets none invented from nothing: the
    // triangles' own are the only honest answer, and they are what a viewer
    // would have computed anyway.
    let normals = if normals.is_empty() {
        normals_from_triangles(&positions, &triangles)
    } else {
        normals
    };
    Ok(Some((
        Triangulation {
            parameters: vec![(0.0, 0.0); positions.len()],
            positions,
            normals,
            triangles,
            // The file says nothing about what deflection it was built at, so
            // nothing is claimed about one.
            deflection_met: false,
        },
        colour,
    )))
}

/// Area-weighted vertex normals, for a file that gave none.
pub(crate) fn normals_from_triangles(positions: &[Point], triangles: &[[u32; 3]]) -> Vec<Vector> {
    let mut out = vec![Vector::new(0.0, 0.0, 0.0); positions.len()];
    for [a, b, c] in triangles {
        let (pa, pb, pc) = (
            positions[*a as usize],
            positions[*b as usize],
            positions[*c as usize],
        );
        // Not normalized: the cross product's length is twice the triangle's
        // area, which is exactly the weight a vertex normal wants.
        let n = (pb - pa).cross(pc - pa);
        for &k in &[*a, *b, *c] {
            out[k as usize] += n;
        }
    }
    for n in &mut out {
        let magnitude = n.magnitude();
        *n = if magnitude > 0.0 {
            *n / magnitude
        } else {
            Vector::new(0.0, 0.0, 1.0)
        };
    }
    out
}

/// One accessor's values, flattened.
struct AccessorValues {
    /// `count * components` numbers, in order.
    values: Vec<f64>,
    /// How many numbers each element holds.
    components: usize,
    /// How many elements there are.
    count: usize,
}

/// Read an accessor: its buffer view, its component type, its stride, and the
/// sparse block that overrides part of it.
///
/// An accessor with no buffer view is all zeros, which the standard says and
/// which is exactly what a sparse accessor over nothing means.
fn read_accessor(
    index: usize,
    accessors: &[crate::json::Json],
    views: &[crate::json::Json],
    buffers: &[Vec<u8>],
) -> ogeom_core::OgeomResult<AccessorValues> {
    let Some(accessor) = accessors.get(index) else {
        ogeom_bail!(Construction, "accessor {index} is not in this document");
    };
    let Some(kind) = accessor.get("type").and_then(crate::json::Json::text) else {
        ogeom_bail!(Construction, "accessor {index} states no type");
    };
    let components = match kind {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" | "MAT2" => 4,
        "MAT3" => 9,
        "MAT4" => 16,
        other => ogeom_bail!(Construction, "`{other}` is not an accessor type"),
    };
    let Some(component_type) = accessor.index_at("componentType") else {
        ogeom_bail!(Construction, "accessor {index} states no componentType");
    };
    let normalized = accessor.get("normalized") == Some(&crate::json::Json::Bool(true));
    let count = accessor.index_at("count").unwrap_or(0);
    // Every element the document holds data for takes at least a byte of
    // its buffers per component, so no honest accessor asks for more
    // numbers than the buffers have bytes; one that does would size an
    // allocation from a number in the file.
    let held: usize = buffers.iter().map(Vec::len).sum();
    let Some(numbers) = count
        .checked_mul(components)
        .filter(|n| *n <= held.max(1 << 16))
    else {
        ogeom_bail!(
            Construction,
            "accessor {index} asks for {count} elements of {components} from {held} bytes"
        );
    };
    let mut values = vec![0.0; numbers];

    if let Some(view_index) = accessor.index_at("bufferView") {
        let offset = accessor.index_at("byteOffset").unwrap_or(0);
        read_into(
            &mut values,
            view_index,
            offset,
            component_type,
            components,
            count,
            normalized,
            views,
            buffers,
        )?;
    }

    // The sparse block: a run of indices, and the elements to put at them.
    // It comes *after* the dense read, because that is what "sparse" means:
    // a document may give a base and then override part of it.
    if let Some(sparse) = accessor.get("sparse") {
        let sparse_count = sparse.index_at("count").unwrap_or(0).min(count);
        let Some(indices) = sparse.get("indices") else {
            ogeom_bail!(Construction, "a sparse accessor names its indices");
        };
        let Some(sparse_values) = sparse.get("values") else {
            ogeom_bail!(Construction, "a sparse accessor names its values");
        };
        let Some(index_type) = indices.index_at("componentType") else {
            ogeom_bail!(Construction, "a sparse index has a componentType");
        };
        let Some(index_view) = indices.index_at("bufferView") else {
            ogeom_bail!(Construction, "a sparse index has a bufferView");
        };
        let mut which = vec![0.0; sparse_count];
        read_into(
            &mut which,
            index_view,
            indices.index_at("byteOffset").unwrap_or(0),
            index_type,
            1,
            sparse_count,
            false,
            views,
            buffers,
        )?;
        let Some(value_view) = sparse_values.index_at("bufferView") else {
            ogeom_bail!(Construction, "a sparse value block has a bufferView");
        };
        let mut replacement = vec![0.0; sparse_count * components];
        read_into(
            &mut replacement,
            value_view,
            sparse_values.index_at("byteOffset").unwrap_or(0),
            component_type,
            components,
            sparse_count,
            normalized,
            views,
            buffers,
        )?;
        for (slot, target) in which.iter().enumerate() {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "range-checked against the element count below"
            )]
            let at = *target as usize;
            if f64::from(u32::try_from(at).unwrap_or(u32::MAX)) != *target || at >= count {
                ogeom_bail!(
                    Construction,
                    "a sparse index names element {target} of {count}"
                );
            }
            for k in 0..components {
                values[at * components + k] = replacement[slot * components + k];
            }
        }
    }

    Ok(AccessorValues {
        values,
        components,
        count,
    })
}

/// Read `count` elements of `components` numbers out of a buffer view.
#[allow(clippy::too_many_arguments)]
fn read_into(
    out: &mut [f64],
    view_index: usize,
    accessor_offset: usize,
    component_type: usize,
    components: usize,
    count: usize,
    normalized: bool,
    views: &[crate::json::Json],
    buffers: &[Vec<u8>],
) -> ogeom_core::OgeomResult<()> {
    let Some(view) = views.get(view_index) else {
        ogeom_bail!(
            Construction,
            "buffer view {view_index} is not in this document"
        );
    };
    let buffer_index = view.index_at("buffer").unwrap_or(0);
    let Some(buffer) = buffers.get(buffer_index) else {
        ogeom_bail!(
            Construction,
            "buffer {buffer_index} is not in this document"
        );
    };
    let view_offset = view.index_at("byteOffset").unwrap_or(0);
    let width = match component_type {
        5120 | 5121 => 1,
        5122 | 5123 => 2,
        5125 | 5126 => 4,
        other => ogeom_bail!(Construction, "{other} is not a glTF component type"),
    };
    // A view may stride over the buffer, which is how a writer interleaves
    // several attributes; with no stride the elements are packed.
    let element = components * width;
    let stride = view.index_at("byteStride").unwrap_or(element);
    if stride < element {
        ogeom_bail!(
            Construction,
            "a stride of {stride} is shorter than the {element} bytes an \
             element occupies"
        );
    }
    for i in 0..count {
        let base = view_offset + accessor_offset + i * stride;
        for k in 0..components {
            let at = base + k * width;
            let Some(slice) = buffer.get(at..at + width) else {
                ogeom_bail!(
                    Construction,
                    "the buffer ends at {} where the accessor wants byte {at}",
                    buffer.len()
                );
            };
            out[i * components + k] = component_value(component_type, slice, normalized);
        }
    }
    Ok(())
}

/// One component, as the number it stands for.
///
/// `normalized` is what the standard means by an integer standing in for a
/// fraction: unsigned types map to `[0, 1]` and signed to `[-1, 1]`, with the
/// signed floor clamped, which is the mapping the standard states exactly.
fn component_value(component_type: usize, bytes: &[u8], normalized: bool) -> f64 {
    match component_type {
        5120 => {
            let v = f64::from(bytes[0] as i8);
            if normalized { (v / 127.0).max(-1.0) } else { v }
        }
        5121 => {
            let v = f64::from(bytes[0]);
            if normalized { v / 255.0 } else { v }
        }
        5122 => {
            let v = f64::from(i16::from_le_bytes([bytes[0], bytes[1]]));
            if normalized {
                (v / 32767.0).max(-1.0)
            } else {
                v
            }
        }
        5123 => {
            let v = f64::from(u16::from_le_bytes([bytes[0], bytes[1]]));
            if normalized { v / 65535.0 } else { v }
        }
        5125 => f64::from(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        _ => f64::from(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
    }
}
