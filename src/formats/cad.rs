//! IGES and Open CASCADE ASCII BREP topology, imported and tessellated in Rust.
//! The kernel's omission/approximation report is checked before any preview.
//! Primary implementation: <https://github.com/gilbertorconde/ogeom-rs>
use crate::{
    Result,
    scene::{Point, Scene, Triangle, Vec3},
};
use ogeom_core::Tolerances;
use ogeom_topo::{EdgeRepr, Filter, Model, NodeData, Shape, ShapeType, explore};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

/// Restore native BREP geometry carrier placements.
#[path = "cad_carriers.rs"]
mod carriers;

/// Keep independent IGES trimmed faces intact through public kernel APIs.
#[path = "iges_preview.rs"]
mod iges_preview;

/// Preserve authored analytic frames while using the upstream BREP topology reader.
#[path = "brep_frames.rs"]
mod brep_frames;

/// Standard millimetre-scale numerical tolerances for the Rust CAD kernel.
const TOLERANCES: Tolerances = Tolerances::millimetres();
/// Maximum decoded CAD exchange payload size accepted for previews.
const MAX_CAD_BYTES: usize = 16 * 1024 * 1024;
/// Maximum native topology or expanded spline records accepted during validation.
const MAX_TOPOLOGY: usize = 100_000;
/// Maximum CAD face count accepted for a preview body.
const MAX_FACES: usize = 4096;
/// Maximum source or expanded preview triangle count.
const MAX_TRIANGLES: usize = 5_000_000;

/// Load IGES or ASCII BREP geometry and tessellate its complete topology.
///
/// # Errors
///
/// File reads, UTF-8 decoding, unsupported entities, invalid topology or tessellation limits fail.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    let extension = super::extension(path)?;
    if extension == "brep" {
        load_brep_text(std::str::from_utf8(&bytes)?)
    } else {
        parse(&bytes, &extension)
    }
}

/// Import an in-memory ASCII BREP document with the same validation as file loading.
///
/// # Errors
///
/// The text is malformed, contains unsupported geometry or exceeds topology/tessellation limits.
pub(super) fn load_brep_text(text: &str) -> Result<Scene> {
    parse(text.as_bytes(), "brep")
}

/// Validate and import CAD exchange data before emitting surfaces, free wires and vertices.
///
/// # Errors
///
/// Input encoding, format records, topology, numeric bounds or tessellation fail validation.
fn parse(bytes: &[u8], extension: &str) -> Result<Scene> {
    if bytes.len() > MAX_CAD_BYTES {
        return Err("CAD input exceeds the 16 MiB preview limit".into());
    }
    let text = std::str::from_utf8(bytes)?;
    let mut scene = Scene::default();
    match extension {
        "iges" | "igs" => {
            validate_iges(text)?;
            let normalized = promote_iges_group_surfaces(text)?;
            let tolerances = iges_tolerances(&normalized)?;
            let imported = iges_preview::read(&normalized, tolerances)?;
            for warning in &imported.report.warnings {
                if !warning.contains("annotation not drawn")
                    && !warning.contains("level names no levels property")
                {
                    return Err(format!("IGES import would compromise geometry: {warning}").into());
                }
            }
            for (name, count) in &imported.report.skipped {
                let kind = name
                    .split_whitespace()
                    .nth(1)
                    .ok_or("invalid IGES skip report")?
                    .parse::<u32>()?;
                // Directions/transforms/colors and drafting metadata carry no
                // standalone preview geometry. Unconsumed curves or surfaces
                // are not discarded just because another solid imported.
                if !matches!(
                    kind,
                    123 | 124
                        | 202
                        | 206
                        | 208
                        | 210
                        | 212
                        | 214
                        | 216
                        | 218
                        | 220
                        | 222
                        | 228
                        | 314
                        | 402
                        | 406
                ) {
                    return Err(format!(
                        "IGES contains {count} unsupported/unconsumed {name} entities"
                    )
                    .into());
                }
            }
            let model = &imported.model;
            if imported.solids.len() + imported.sheets.len() > 1024 {
                return Err("IGES exceeds 1024 preview bodies".into());
            }
            for shape in imported.solids.iter().chain(&imported.sheets) {
                append_shape_with_tolerances(&mut scene, model, shape, tolerances)?;
            }
        }
        "brep" => {
            validate_brep(text)?;
            let normalized = normalize_brep_caches(text)?;
            let carriers = carriers::Metadata::parse(&normalized)?;
            let (mut model, root) = brep_frames::read(&normalized, TOLERANCES)?;
            validate_topology(&model, &root)?;
            carriers.restore(&mut model, &root, TOLERANCES)?;
            append_shape(&mut scene, &model, &root)?;
        }
        _ => return Err("unsupported CAD exchange extension".into()),
    }
    scene.validate()?;
    Ok(scene)
}

/// Append a CAD shape using the standard millimetre kernel tolerances.
///
/// # Errors
///
/// Topology is invalid, geometry cannot be evaluated or preview tessellation limits are exceeded.
pub(super) fn append_shape(scene: &mut Scene, model: &Model, root: &Shape) -> Result<()> {
    append_shape_with_tolerances(scene, model, root, TOLERANCES)
}

/// Append every supported face, free curve and isolated vertex at the requested tolerances.
///
/// # Errors
///
/// Missing topology, failed geometry evaluation or unmet bounded tessellation tolerance prevents a complete preview.
#[allow(
    clippy::too_many_lines,
    reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
)]
fn append_shape_with_tolerances(
    scene: &mut Scene,
    model: &Model,
    root: &Shape,
    tolerances: Tolerances,
) -> Result<()> {
    validate_topology(model, root)?;
    let faces = explore(model, root, Filter::OfType(ShapeType::Face))?;
    if faces.len() > MAX_FACES {
        return Err("CAD body exceeds 4096 preview faces".into());
    }
    if !faces.is_empty() {
        // Use actual analytic/curve bounds, including a curved boundary's
        // bulge, rather than fitting tessellation to its control vertices.
        let diagonal = ogeom_algo::shape_bounds(model, root, tolerances)?.diagonal();
        // Coincidence precision belongs to geometry/topology validation.
        // Amplifying authored IGES precision here would instead discard
        // visible surface detail. Keep only the default numerical floor,
        // and refuse scales at which it exceeds the promised 1% bound.
        let numerical_floor = TOLERANCES.confusion() * 100.0;
        if !diagonal.is_finite() || diagonal * 0.01 < numerical_floor {
            return Err(
                "CAD shape is too small for the bounded preview tessellation tolerance".into(),
            );
        }
        let mut accepted = None;
        for (relative, angular) in [(0.003, 0.2), (0.006, 0.3), (0.01, 0.4)] {
            let deflection = ogeom_mesh::Deflection {
                chord: (diagonal * relative).max(numerical_floor),
                angular,
                min_segments: 4,
                max_segments: 4096,
            };
            let (meshes, _) =
                ogeom_mesh::face_meshes_for(model, root, deflection, None, tolerances)?;
            if meshes.iter().all(|(_, mesh)| {
                mesh.as_ref()
                    .is_ok_and(|mesh| !mesh.triangles.is_empty() && mesh.deflection_met)
            }) {
                accepted = Some(meshes);
                break;
            }
            // A missing pcurve or an invalid face is a geometry error; an
            // approximation retry cannot repair it.
            for (_, mesh) in &meshes {
                if let Err(error) = mesh {
                    return Err(error.to_string().into());
                }
            }
        }
        let meshes = accepted.ok_or(
            "CAD face failed the bounded preview tessellation tolerance (1% of shape diameter)",
        )?;
        if meshes.len() != faces.len() {
            return Err("CAD tessellation omitted a face".into());
        }
        for (_, mesh) in meshes {
            let mesh = mesh?;
            if mesh.triangles.is_empty() || !mesh.deflection_met {
                return Err("CAD face failed the requested preview tessellation tolerance".into());
            }
            if scene.triangles.len() + mesh.triangles.len() > MAX_TRIANGLES {
                return Err("CAD preview exceeds five million triangles".into());
            }
            for indices in mesh.triangles {
                let mut vertices = [Vec3::default(); 3];
                for (vertex, index) in vertices.iter_mut().zip(indices) {
                    let p = mesh
                        .positions
                        .get(index as usize)
                        .ok_or("CAD tessellation has an invalid vertex index")?;
                    *vertex = Vec3::new(p.x, p.y, p.z);
                }
                scene.triangles.push(Triangle {
                    vertices,
                    color: None,
                });
            }
        }
    }
    // Free wires and vertices remain meaningful in a compound with faces.
    // Surface boundary edges already belong to the triangles and need no
    // overlay; every free curve is evaluated on its stored parameter range.
    let mut edges_seen = HashSet::new();
    let mut vertices_seen = HashSet::new();
    for face in &faces {
        for edge in explore(model, face, Filter::OfType(ShapeType::Edge))? {
            edges_seen.insert((edge.node(), edge.location().clone()));
        }
        for vertex in explore(model, face, Filter::OfType(ShapeType::Vertex))? {
            vertices_seen.insert((vertex.node(), vertex.location().clone()));
        }
    }
    let edges: Vec<_> = explore(model, root, Filter::OfType(ShapeType::Edge))?
        .into_iter()
        .filter(|edge| edges_seen.insert((edge.node(), edge.location().clone())))
        .collect();
    if !edges.is_empty() {
        let deflection = ogeom_algo::relative_deflection(model, root, 0.003, tolerances)?;
        for edge in edges {
            for vertex in explore(model, &edge, Filter::OfType(ShapeType::Vertex))? {
                vertices_seen.insert((vertex.node(), vertex.location().clone()));
            }
            let data = model
                .node(&edge)
                .and_then(|n| n.data().as_edge())
                .ok_or("CAD edge is missing topology")?;
            let Some(EdgeRepr::Curve3d { curve, range, .. }) = data.curve3d() else {
                if data.degenerate {
                    continue;
                }
                return Err("free CAD edge has no supported 3D curve".into());
            };
            let geometry = model
                .geometry()
                .curve(*curve)
                .ok_or("CAD edge has no geometry")?;
            let line = ogeom_mesh::discretize(geometry, *range, deflection, tolerances)?;
            if !line.deflection_met {
                return Err("CAD wire failed preview tessellation tolerance".into());
            }
            let placement = edge.transform(model.datums())?;
            for points in line.points.windows(2) {
                if scene.lines.len() >= MAX_TRIANGLES {
                    return Err("CAD preview exceeds five million segments".into());
                }
                let vertices = std::array::from_fn(|i| {
                    let p = placement.apply(points[i]);
                    Vec3::new(p.x, p.y, p.z)
                });
                scene.lines.push(crate::scene::Line {
                    vertices,
                    color: None,
                });
            }
        }
    }
    for vertex in explore(model, root, Filter::OfType(ShapeType::Vertex))? {
        if !vertices_seen.insert((vertex.node(), vertex.location().clone())) {
            continue;
        }
        if scene.points.len() >= MAX_TOPOLOGY {
            return Err("CAD preview exceeds 100000 isolated points".into());
        }
        let data = model
            .node(&vertex)
            .and_then(|n| n.data().as_vertex())
            .ok_or("CAD vertex is missing geometry")?;
        let p = vertex.transform(model.datums())?.apply(data.point);
        scene.points.push(Point {
            position: Vec3::new(p.x, p.y, p.z),
            color: None,
        });
    }
    Ok(())
}

/// Bound topology size and validate its geometry before kernel traversal.
///
/// # Errors
///
/// Topology exceeds limits or the kernel reports invalid geometry or structure.
fn validate_topology(model: &Model, root: &Shape) -> Result<()> {
    let mut queue = vec![(root.clone(), 0usize)];
    let mut occurrences = 0usize;
    while let Some((shape, depth)) = queue.pop() {
        occurrences += 1;
        if occurrences > MAX_TOPOLOGY || depth > 64 {
            return Err("CAD topology exceeds 100000 occurrences or 64 levels".into());
        }
        let node = model
            .node(&shape)
            .ok_or("CAD topology references missing geometry")?;
        if let NodeData::Vertex(data) = node.data() {
            let p = shape.transform(model.datums())?.apply(data.point);
            if !Vec3::new(p.x, p.y, p.z).is_finite() {
                return Err("CAD topology contains non-finite vertices".into());
            }
        }
        for child in model.children_of(&shape)? {
            queue.push((child, depth + 1));
        }
    }
    Ok(())
}

/// Validate the fixed-column record envelope before a permissive geometry
/// parser can turn truncation or mismatched parameter ownership into defaults.
fn validate_iges(text: &str) -> Result<()> {
    let mut counts = [0usize; 5];
    let mut phase = 0usize;
    let mut directories: Vec<&[u8]> = Vec::new();
    let mut parameters: Vec<&[u8]> = Vec::new();
    let mut terminal = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let b = line.as_bytes();
        if b.len() < 80 || b[80..].iter().any(|c| !c.is_ascii_whitespace()) {
            return Err("IGES records must contain 80 fixed columns".into());
        }
        let index = match b[72] {
            b'S' => 0,
            b'G' => 1,
            b'D' => 2,
            b'P' => 3,
            b'T' => 4,
            _ => return Err("unknown IGES record section".into()),
        };
        if index < phase || (phase == 4) {
            return Err("IGES sections are out of order or repeated after termination".into());
        }
        phase = index;
        counts[index] += 1;
        if parse_usize(&b[73..80])? != counts[index] {
            return Err("IGES record sequence numbers are inconsistent".into());
        }
        match index {
            2 => directories.push(b),
            3 => parameters.push(b),
            4 => terminal = Some(b),
            _ => {}
        }
    }
    if counts.contains(&0)
        || !directories.len().is_multiple_of(2)
        || directories.len() / 2 > MAX_TOPOLOGY
    {
        return Err(
            "IGES requires complete S/G/D/P/T sections and bounded paired directory entries".into(),
        );
    }
    let terminal = terminal.ok_or("IGES lacks termination record")?;
    for (section, label) in [(0, b'S'), (1, b'G'), (2, b'D'), (3, b'P')] {
        let at = section * 8;
        if terminal[at] != label || parse_usize(&terminal[at + 1..at + 8])? != counts[section] {
            return Err("IGES termination counts do not match records".into());
        }
    }
    let mut used = vec![false; parameters.len()];
    for (i, pair) in directories.as_chunks::<2>().0.iter().enumerate() {
        if parse_usize(&pair[0][..8])? != parse_usize(&pair[1][..8])? {
            return Err("IGES paired directory types differ".into());
        }
        let start = parse_usize(&pair[0][8..16])?
            .checked_sub(1)
            .ok_or("IGES directory has no parameter pointer")?;
        let number = parse_usize(&pair[1][24..32])?;
        let end = start
            .checked_add(number)
            .ok_or("IGES parameter range overflow")?;
        let records = parameters
            .get(start..end)
            .ok_or("IGES directory parameter range is truncated")?;
        if records.is_empty() {
            return Err("IGES entity has no parameter records".into());
        }
        for (offset, record) in records.iter().enumerate() {
            if parse_usize(&record[64..72])? != i * 2 + 1 || used[start + offset] {
                return Err("IGES parameter ownership overlaps or differs from directory".into());
            }
            used[start + offset] = true;
        }
    }
    if used.contains(&false) {
        return Err("IGES contains orphan parameter records".into());
    }
    Ok(())
}

/// Decode a whitespace-padded decimal count from an exchange record.
///
/// # Errors
///
/// The bytes are not UTF-8 or do not represent a nonnegative machine-sized integer.
fn parse_usize(bytes: &[u8]) -> Result<usize> {
    Ok(std::str::from_utf8(bytes)?.trim().parse()?)
}

/// IGES global parameter 19 is the authored distance below which points
/// should be coincident, expressed in the units selected by parameters
/// 14/15. The kernel converts geometry to millimeters but does not consume
/// this resolution. Its scale constructor also scales linear tolerances,
/// so invert the requested millimeter confusion to retain that precision.
/// Primary specification, section 2.2.4.2.19:
/// <https://nvlpubs.nist.gov/nistpubs/Legacy/IR/nbsir86-3359.pdf>
#[allow(
    clippy::too_many_lines,
    reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
)]
fn iges_tolerances(text: &str) -> Result<Tolerances> {
    let global = text
        .lines()
        .filter(|line| line.as_bytes().get(72) == Some(&b'G'))
        .map(|line| &line[..72])
        .collect::<String>();
    let bytes = global.as_bytes();
    let delimiter = if bytes.starts_with(b"1H") {
        *bytes.get(2).ok_or("truncated IGES delimiter")?
    } else {
        b','
    };
    let second = if bytes.starts_with(b"1H") { 4 } else { 1 };
    let record = if bytes.get(second..second + 2) == Some(b"1H") {
        *bytes
            .get(second + 2)
            .ok_or("truncated IGES record delimiter")?
    } else {
        b';'
    };
    let mut fields = Vec::new();
    let mut at = 0;
    loop {
        while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        let start = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        let field = if at > start && bytes.get(at) == Some(&b'H') {
            let length = std::str::from_utf8(&bytes[start..at])?.parse::<usize>()?;
            at += 1;
            let end = at
                .checked_add(length)
                .ok_or("IGES global string length overflow")?;
            let value =
                std::str::from_utf8(bytes.get(at..end).ok_or("truncated IGES global string")?)?
                    .to_owned();
            at = end;
            while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
                at += 1;
            }
            value
        } else {
            at = start;
            while bytes
                .get(at)
                .is_some_and(|b| *b != delimiter && *b != record)
            {
                at += 1;
            }
            std::str::from_utf8(&bytes[start..at])?.trim().to_owned()
        };
        fields.push(field);
        match bytes.get(at) {
            Some(byte) if *byte == delimiter => at += 1,
            Some(byte) if *byte == record => break,
            _ => return Err("IGES global metadata lacks a valid terminator".into()),
        }
        if fields.len() > 128 {
            return Err("IGES global metadata has too many fields".into());
        }
    }
    let Some(resolution) = fields.get(18).filter(|field| !field.is_empty()) else {
        return Ok(TOLERANCES);
    };
    let resolution = resolution.replace(['d', 'D'], "E").parse::<f64>()?;
    let flag = fields
        .get(13)
        .ok_or("IGES resolution lacks units")?
        .parse::<u32>()?;
    let scale = match flag {
        1 => 25.4,
        2 => 1.,
        4 => 304.8,
        5 => 1_609_344.,
        6 => 1000.,
        7 => 1_000_000.,
        8 => 0.0254,
        9 => 0.001,
        10 => 10.,
        11 => 0.000_025_4,
        3 => match fields
            .get(14)
            .ok_or("IGES custom unit name missing")?
            .to_ascii_uppercase()
            .as_str()
        {
            "IN" | "INCH" => 25.4,
            "MM" => 1.,
            "FT" => 304.8,
            "MI" => 1_609_344.,
            "M" => 1000.,
            "KM" => 1_000_000.,
            "MIL" => 0.0254,
            "UM" => 0.001,
            "CM" => 10.,
            "UIN" => 0.000_025_4,
            _ => return Err("unsupported IGES resolution units".into()),
        },
        _ => return Err("unsupported IGES resolution units".into()),
    };
    let confusion = resolution * scale;
    if !confusion.is_finite() || confusion <= 0. || confusion > 0.1 {
        return Err(
            "IGES authored resolution must be positive and at most 0.1 millimeters for previews"
                .into(),
        );
    }
    Ok(Tolerances::with_scale(
        ogeom_core::tolerance::CONFUSION / confusion.max(TOLERANCES.confusion()),
    )?)
}

/// A logical group owns its subordinate surfaces, but adds no transform.
/// Promote those actual group members into the kernel's root surface set;
/// physically subordinate surfaces stay owned by their geometric entity.
fn promote_iges_group_surfaces(text: &str) -> Result<String> {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let mut directories = BTreeMap::new();
    let mut parameters: BTreeMap<usize, String> = BTreeMap::new();
    let mut global = String::new();
    for (index, line) in lines.iter().enumerate() {
        let b = line.as_bytes();
        if b.len() < 80 {
            continue;
        }
        match b[72] {
            b'D' => {
                let sequence = parse_usize(&b[73..80])?;
                if sequence % 2 == 1 {
                    directories.insert(sequence, index);
                }
            }
            b'P' => parameters
                .entry(parse_usize(&b[64..72])?)
                .or_default()
                .push_str(std::str::from_utf8(&b[..64])?),
            b'G' => global.push_str(std::str::from_utf8(&b[..72])?),
            _ => {}
        }
    }
    // Group parameter data are numeric. The kernel handles general custom
    // delimiters; this promotion additionally handles explicit one-character
    // delimiters and the conventional omitted defaults.
    let g = global.trim_start().as_bytes();
    let delimiter = if g.starts_with(b"1H") {
        *g.get(2).ok_or("truncated IGES delimiter declaration")? as char
    } else {
        ','
    };
    let second = g
        .get(if g.starts_with(b"1H") { 4 } else { 1 }..)
        .ok_or("truncated IGES global delimiters")?;
    let record_delimiter = if second.starts_with(b"1H") {
        *second.get(2).ok_or("truncated IGES record delimiter")? as char
    } else {
        ';'
    };
    let mut members = HashSet::new();
    for (&id, &index) in &directories {
        let line = &lines[index];
        if parse_usize(&line.as_bytes()[..8])? != 402 {
            continue;
        }
        let form = parse_usize(&lines[index + 1].as_bytes()[32..40])?;
        if ![1, 7, 14, 15].contains(&form) {
            continue;
        }
        let data = parameters.get(&id).ok_or("IGES group lacks parameters")?;
        let fields: Vec<_> = data.split(delimiter).collect();
        let number = fields
            .get(1)
            .ok_or("IGES group lacks member count")?
            .trim()
            .parse::<usize>()?;
        if number > MAX_TOPOLOGY || fields.len() < number + 2 {
            return Err("IGES group member list is truncated or oversized".into());
        }
        for field in &fields[2..number + 2] {
            let pointer = field
                .trim()
                .trim_end_matches(record_delimiter)
                .parse::<usize>()?;
            if !directories.contains_key(&pointer) {
                return Err("IGES group references missing directory entry".into());
            }
            members.insert(pointer);
        }
    }
    for pointer in members {
        let index = directories[&pointer];
        let line = &mut lines[index];
        let kind = parse_usize(&line.as_bytes()[..8])?;
        let status = line[64..72].replace(' ', "0").parse::<u32>()?;
        if matches!(
            kind,
            143 | 144
                | 150
                | 152
                | 154
                | 156
                | 158
                | 160
                | 162
                | 164
                | 168
                | 180
                | 184
                | 186
                | 408
                | 430
        ) && (status / 10_000) % 100 == 2
        {
            line.replace_range(66..68, "00");
        }
    }
    Ok(lines.join("\n"))
}

/// Cached OCCT polygon parameters store exactly the polygon's node count,
/// not an additional count after the `p` marker. Validate and discard caches
/// before the kernel regenerates the mesh from exact curves and surfaces.
fn normalize_brep_caches(text: &str) -> Result<String> {
    let tokens: Vec<_> = text.split_whitespace().collect();
    let section = |name: &str| -> Result<usize> {
        tokens
            .iter()
            .position(|token| *token == name)
            .ok_or_else(|| format!("BREP lacks {name} section").into())
    };
    let poly3 = section("Polygon3D")?;
    let curves2 = section("Curve2ds")?;
    let curves3 = section("Curves")?;
    let polytri = section("PolygonOnTriangulations")?;
    let surfaces = section("Surfaces")?;
    let triangles = section("Triangulations")?;
    let topology = section("TShapes")?;
    if !(curves2 < curves3
        && curves3 < poly3
        && poly3 < polytri
        && polytri < surfaces
        && surfaces < triangles
        && triangles < topology)
    {
        return Err("BREP geometry/cache sections are out of order".into());
    }
    validate_geometry_table(&tokens[curves2 + 1..curves3], 2)?;
    validate_geometry_table(&tokens[curves3 + 1..poly3], 3)?;
    validate_geometry_table(&tokens[surfaces + 1..triangles], 0)?;
    validate_cache(&tokens[poly3 + 1..polytri], 0)?;
    validate_cache(&tokens[polytri + 1..surfaces], 1)?;
    validate_cache(&tokens[triangles + 1..topology], 2)?;
    let mut rebuilt = Vec::new();
    rebuilt.extend_from_slice(&tokens[..poly3]);
    rebuilt.extend(["Polygon3D", "0", "PolygonOnTriangulations", "0"]);
    rebuilt.extend_from_slice(&tokens[surfaces..triangles]);
    rebuilt.extend(["Triangulations", "0"]);
    rebuilt.extend_from_slice(&tokens[topology..]);
    let body = rebuilt
        .iter()
        .skip(usize::from(rebuilt.first() == Some(&"DBRep_DrawableShape")))
        .map(|token| {
            // Older valid OCCT files fuse an integer index and its continuity
            // token (e.g. `44CN`). Their numeric scanner separates the two.
            let digits = token.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 && ["C0", "C1", "C2", "C3", "CN", "G1", "G2"].contains(&&token[digits..])
            {
                format!("{} {}", &token[..digits], &token[digits..])
            } else {
                (*token).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    Ok(format!("DBRep_DrawableShape\n\n{body}"))
}

/// The upstream reader has recursive trimmed/offset geometry records.
/// Walk their numeric schema before calling it, bounding nesting, spline
/// grids and multiplicities independently of the file's top-level counts,
/// retaining each record's analytic frame for the owned BREP adapter.
///
/// # Errors
///
/// Geometry is unsupported, truncated, exceeds recursion/spline limits or
/// contains invalid authored analytic coordinate frames.
#[allow(
    clippy::too_many_lines,
    reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
)]
fn validate_geometry_table(
    tokens: &[&str],
    dimension: u8,
) -> Result<Vec<Option<ogeom_math::Frame>>> {
    /// Bounds-checked cursor for recursive BREP geometry-token schemas.
    struct Cursor<'a> {
        /// Token payload for one native geometry table.
        tokens: &'a [&'a str],
        /// Index of the next unread token.
        at: usize,
    }
    impl Cursor<'_> {
        /// Consume one bounded BREP geometry count.
        ///
        /// # Errors
        ///
        /// The record is truncated, nonnumeric or above the topology limit.
        fn count(&mut self) -> Result<usize> {
            let n = self
                .tokens
                .get(self.at)
                .ok_or("truncated BREP geometry")?
                .parse::<usize>()?;
            self.at += 1;
            if n > MAX_TOPOLOGY {
                return Err("BREP geometry count exceeds 100000".into());
            }
            Ok(n)
        }
        /// Advance the geometry-token cursor without crossing its payload.
        ///
        /// # Errors
        ///
        /// The offset overflows or the payload is truncated.
        fn skip(&mut self, n: usize) -> Result<()> {
            self.at = self
                .at
                .checked_add(n)
                .ok_or("BREP geometry size overflow")?;
            if self.at > self.tokens.len() {
                return Err("truncated BREP geometry payload".into());
            }
            Ok(())
        }
        /// Consume one finite coordinate from an analytic frame.
        ///
        /// # Errors
        ///
        /// A coordinate is missing, nonnumeric or non-finite.
        fn coordinate(&mut self) -> Result<f64> {
            let value = self
                .tokens
                .get(self.at)
                .ok_or("truncated BREP analytic frame")?
                .parse::<f64>()?;
            self.at += 1;
            if !value.is_finite() {
                return Err("non-finite BREP analytic frame".into());
            }
            Ok(value)
        }
        /// Consume and normalize one authored analytic-frame direction.
        ///
        /// # Errors
        ///
        /// Coordinates are invalid or do not define a nonzero direction.
        fn direction(&mut self) -> Result<ogeom_math::Direction> {
            Ok(ogeom_math::Direction::new(
                ogeom_math::Vector::new(self.coordinate()?, self.coordinate()?, self.coordinate()?),
                TOLERANCES,
            )?)
        }
        /// Build an analytic frame from all four authored triples, including its Y axis.
        ///
        /// # Errors
        ///
        /// Coordinates, directions or the orthogonal frame are invalid.
        fn frame(&mut self) -> Result<ogeom_math::Frame> {
            let origin =
                ogeom_math::Point::new(self.coordinate()?, self.coordinate()?, self.coordinate()?);
            let normal = self.direction()?;
            let x = self.direction()?;
            let y = self.direction()?;
            Ok(ogeom_math::Frame::from_axes(
                origin, x, y, normal, TOLERANCES,
            )?)
        }
        /// Consume a BREP geometry boolean encoded as zero or one.
        ///
        /// # Errors
        ///
        /// The flag is missing, nonnumeric or outside zero/one.
        fn flag(&mut self) -> Result<usize> {
            let n = self.count()?;
            if n > 1 {
                return Err("invalid BREP geometry flag".into());
            }
            Ok(n)
        }
        /// Validate knot multiplicities and their bounded expanded count.
        ///
        /// # Errors
        ///
        /// The degree, multiplicities or expansion exceed preview limits, or data is truncated.
        fn knots(&mut self, n: usize, degree: usize) -> Result<()> {
            if degree > 64 {
                return Err("BREP spline degree exceeds 64".into());
            }
            let mut expanded = 0usize;
            for _ in 0..n {
                self.skip(1)?;
                let multiplicity = self.count()?;
                expanded = expanded
                    .checked_add(multiplicity)
                    .ok_or("BREP knot count overflow")?;
                if multiplicity > degree + 1 || expanded > MAX_TOPOLOGY {
                    return Err("BREP spline knot expansion exceeds preview limits".into());
                }
            }
            Ok(())
        }
        /// Consume one supported recursive analytic or spline geometry record.
        ///
        /// # Errors
        ///
        /// The schema is unsupported, truncated or exceeds nesting/spline limits.
        fn geometry(&mut self, dimension: u8, depth: usize) -> Result<Option<ogeom_math::Frame>> {
            if depth > 64 {
                return Err("BREP trimmed/offset geometry exceeds 64 levels".into());
            }
            let kind = self.count()?;
            let mut frame = None;
            if dimension == 0 {
                match kind {
                    1..=5 => {
                        frame = Some(self.frame()?);
                        self.skip([0, 0, 1, 2, 1, 2][kind])?;
                    }
                    6 => {
                        self.skip(3)?;
                        frame = self.geometry(3, depth + 1)?;
                    }
                    7 => {
                        self.skip(6)?;
                        frame = self.geometry(3, depth + 1)?;
                    }
                    9 => {
                        let rational = self.flag()? + self.flag()? > 0;
                        self.flag()?;
                        self.flag()?;
                        let degree_u = self.count()?;
                        let degree_v = self.count()?;
                        let poles_u = self.count()?;
                        let poles_v = self.count()?;
                        let knots_u = self.count()?;
                        let knots_v = self.count()?;
                        let cells = poles_u
                            .checked_mul(poles_v)
                            .ok_or("BREP spline grid overflow")?;
                        if cells > MAX_TOPOLOGY {
                            return Err("BREP spline grid exceeds 100000 poles".into());
                        }
                        self.skip(cells * (3 + usize::from(rational)))?;
                        self.knots(knots_u, degree_u)?;
                        self.knots(knots_v, degree_v)?;
                    }
                    10 => {
                        self.skip(4)?;
                        frame = self.geometry(0, depth + 1)?;
                    }
                    11 => {
                        self.skip(1)?;
                        frame = self.geometry(0, depth + 1)?;
                    }
                    _ => return Err(format!("unsupported BREP surface record {kind}").into()),
                }
            } else {
                let d = usize::from(dimension);
                match kind {
                    1 => self.skip(2 * d)?,
                    2..=5 if d == 3 => {
                        frame = Some(self.frame()?);
                        self.skip(usize::from(kind == 3 || kind == 5) + 1)?;
                    }
                    2 => self.skip(7)?,
                    3 => self.skip(8)?,
                    7 => {
                        let rational = self.flag()?;
                        self.flag()?;
                        let degree = self.count()?;
                        let poles = self.count()?;
                        let knots = self.count()?;
                        self.skip(poles * (d + rational))?;
                        self.knots(knots, degree)?;
                    }
                    8 => {
                        self.skip(2)?;
                        frame = self.geometry(dimension, depth + 1)?;
                    }
                    9 => {
                        self.skip(if d == 2 { 1 } else { 4 })?;
                        frame = self.geometry(dimension, depth + 1)?;
                    }
                    _ => return Err(format!("unsupported BREP curve record {kind}").into()),
                }
            }
            Ok(frame)
        }
    }
    let mut cursor = Cursor { tokens, at: 0 };
    let records = cursor.count()?;
    let mut frames = Vec::with_capacity(records);
    for _ in 0..records {
        frames.push(cursor.geometry(dimension, 0)?);
    }
    if cursor.at != tokens.len() {
        return Err("BREP geometry table has trailing or mismatched data".into());
    }
    Ok(frames)
}

/// Check cached polygon or triangulation payloads before discarding them.
///
/// # Errors
///
/// Counts, flags, indices or cached parameter records are malformed or out of bounds.
fn validate_cache(tokens: &[&str], kind: u8) -> Result<()> {
    let mut at = 0;
    let number = |at: &mut usize| -> Result<usize> {
        let n = tokens
            .get(*at)
            .ok_or("truncated BREP mesh cache")?
            .parse::<usize>()?;
        *at += 1;
        if n > MAX_TRIANGLES {
            return Err("BREP cached mesh count exceeds five million".into());
        }
        Ok(n)
    };
    let skip = |at: &mut usize, count: usize, integers: bool| -> Result<()> {
        let end = at.checked_add(count).ok_or("BREP cache size overflow")?;
        for token in tokens
            .get(*at..end)
            .ok_or("truncated BREP mesh cache payload")?
        {
            if integers {
                token.parse::<u32>()?;
            } else if !token.parse::<f64>()?.is_finite() {
                return Err("non-finite BREP mesh cache".into());
            }
        }
        *at = end;
        Ok(())
    };
    let records = number(&mut at)?;
    for _ in 0..records {
        let nodes = number(&mut at)?;
        match kind {
            0 => {
                let has = number(&mut at)?;
                if has > 1 {
                    return Err("invalid BREP polygon parameter flag".into());
                }
                skip(&mut at, 1, false)?;
                skip(&mut at, nodes * 3 + nodes * has, false)?;
            }
            1 => {
                skip(&mut at, nodes, true)?;
                if tokens.get(at) == Some(&"p") {
                    at += 1;
                    skip(&mut at, 1, false)?;
                    let has = number(&mut at)?;
                    if has > 1 {
                        return Err("invalid BREP polygon parameter flag".into());
                    }
                    skip(&mut at, nodes * has, false)?;
                }
            }
            2 => {
                let triangles = number(&mut at)?;
                let has = number(&mut at)?;
                if has > 1 {
                    return Err("invalid BREP triangulation parameter flag".into());
                }
                skip(&mut at, 1, false)?;
                skip(&mut at, nodes * (3 + 2 * has), false)?;
                skip(&mut at, triangles * 3, true)?;
            }
            _ => return Err("invalid BREP cached geometry kind".into()),
        }
    }
    if at != tokens.len() {
        return Err("unsupported or trailing BREP mesh cache data".into());
    }
    Ok(())
}

/// Validate ASCII BREP headers, tables and finite numeric payloads.
///
/// # Errors
///
/// Headers, counts, topology, geometry or placement records are malformed or unsupported.
fn validate_brep(text: &str) -> Result<()> {
    let first = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or("empty BREP input")?
        .trim();
    if first != "DBRep_DrawableShape" && !first.starts_with("CASCADE Topology") {
        return Err("BREP requires Open CASCADE ASCII topology".into());
    }
    let mut tokens = text.split_whitespace().peekable();
    let mut saw_topology = false;
    let mut count = 0usize;
    while let Some(token) = tokens.next() {
        count += 1;
        if count > 2_000_000 {
            return Err("BREP exceeds two million tokens".into());
        }
        if matches!(
            token,
            "Locations"
                | "Curve2ds"
                | "Curves"
                | "Polygon3D"
                | "PolygonOnTriangulations"
                | "Surfaces"
                | "Triangulations"
                | "TShapes"
        ) {
            let n = tokens
                .next()
                .ok_or("BREP table has no count")?
                .parse::<usize>()?;
            if n > MAX_TOPOLOGY {
                return Err("BREP table exceeds 100000 records".into());
            }
            saw_topology |= token == "TShapes";
            if token == "Locations" {
                validate_locations(&mut tokens, n)?;
            }
        }
        if let Ok(value) = token.parse::<f64>()
            && !value.is_finite()
        {
            return Err("BREP contains non-finite numeric values".into());
        }
    }
    if !saw_topology {
        return Err("BREP lacks a topology table".into());
    }
    Ok(())
}

/// Validate BREP placement matrices and bounded composition powers.
///
/// # Errors
///
/// Locations are truncated, non-finite, invalidly referenced or exceed composition limits.
fn validate_locations<'a>(
    tokens: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
    count: usize,
) -> Result<()> {
    for current in 0..count {
        match tokens.next().ok_or("truncated BREP location")? {
            "1" => {
                for _ in 0..12 {
                    let value = tokens
                        .next()
                        .ok_or("truncated BREP transform")?
                        .parse::<f64>()?;
                    if !value.is_finite() {
                        return Err("BREP transform is non-finite".into());
                    }
                }
            }
            "2" => {
                let mut links = 0usize;
                loop {
                    let index = tokens
                        .next()
                        .ok_or("truncated BREP location composition")?
                        .parse::<usize>()?;
                    if index == 0 {
                        break;
                    }
                    links += 1;
                    let power = tokens
                        .next()
                        .ok_or("truncated BREP location power")?
                        .parse::<i64>()?;
                    if index > current || links > 64 || power.unsigned_abs() > 64 {
                        return Err(
                            "BREP composed location exceeds reference/depth/power limits".into(),
                        );
                    }
                }
            }
            _ => return Err("unsupported BREP location representation".into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogeom_math::{Frame, Point as CadPoint, Vector};
    use std::fmt::Write as _;

    fn body(kind: &str) -> (Model, Shape) {
        let mut model = Model::new();
        let frame = Frame::WORLD.with_origin(CadPoint::new(7., -2., 5.));
        let built = match kind {
            "box" => ogeom_algo::make_box(&mut model, frame, (2., 3., 4.), TOLERANCES),
            "cylinder" => ogeom_algo::make_cylinder(&mut model, frame, 2., 3., TOLERANCES),
            "sphere" => ogeom_algo::make_sphere(&mut model, frame, 2., TOLERANCES),
            _ => panic!("unknown fixture"),
        }
        .unwrap();
        (model, built.shape)
    }
    fn area(scene: &Scene) -> f64 {
        scene
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.vertices;
                (b - a).cross(c - a).length() * 0.5
            })
            .sum()
    }
    fn text_of(kind: &str, iges: bool) -> String {
        let (model, root) = body(kind);
        if iges {
            let mut document = ogeom_doc::Document::over(model);
            document.add_part("translated", root);
            ogeom_io::write_iges(&document, TOLERANCES).unwrap()
        } else {
            ogeom_io::brep::write(&model, &root, TOLERANCES).unwrap()
        }
    }

    #[test]
    fn brep_planar_topology_preserves_real_boundaries_and_placement() {
        let text = text_of("box", false);
        let scene = load_brep_text(&text).unwrap();
        assert_eq!(scene.triangles.len(), 12);
        assert!((area(&scene) - 52.).abs() < 1e-8);
        let bounds = scene.positions().fold(
            (
                Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
                Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
            ),
            |(low, high), p| {
                (
                    Vec3::new(low.x.min(p.x), low.y.min(p.y), low.z.min(p.z)),
                    Vec3::new(high.x.max(p.x), high.y.max(p.y), high.z.max(p.z)),
                )
            },
        );
        assert_eq!(bounds.0, Vec3::new(7., -2., 5.));
        assert_eq!(bounds.1, Vec3::new(9., 1., 9.));
        let without_header = text.split_once("CASCADE Topology").unwrap().1;
        assert_eq!(
            load_brep_text(&format!("CASCADE Topology{without_header}"))
                .unwrap()
                .triangles
                .len(),
            12
        );
    }

    #[test]
    fn brep_compound_preserves_free_wires_and_isolated_vertices() {
        let (mut model, solid) = body("box");
        let wire = ogeom_algo::make_polygon(
            &mut model,
            &[
                CadPoint::new(-2., 0., 0.),
                CadPoint::new(-1., 2., 0.),
                CadPoint::new(0., 1., 0.),
            ],
            false,
            TOLERANCES,
        )
        .unwrap()
        .shape;
        let vertex = ogeom_algo::make_vertex(&mut model, CadPoint::new(-3., 4., 1.)).shape;
        let root = ogeom_algo::make_compound(&mut model, &[solid, wire, vertex])
            .unwrap()
            .shape;
        let text = ogeom_io::brep::write(&model, &root, TOLERANCES).unwrap();
        let scene = load_brep_text(&text).unwrap();
        assert_eq!(scene.triangles.len(), 12);
        assert!((area(&scene) - 52.).abs() < 1e-8);
        assert_eq!(scene.lines.len(), 2);
        assert_eq!(scene.points.len(), 1);
        assert_eq!(scene.points[0].position, Vec3::new(-3., 4., 1.));
        assert!(
            scene
                .lines
                .iter()
                .all(|line| line.vertices.iter().all(|p| p.x <= 1e-9))
        );
    }

    #[test]
    fn cad_curved_surfaces_are_tessellated_rather_than_control_points() {
        for kind in ["cylinder", "sphere"] {
            let expected = if kind == "cylinder" {
                20. * std::f64::consts::PI
            } else {
                16. * std::f64::consts::PI
            };
            for iges in [false, true] {
                let text = text_of(kind, iges);
                let scene = parse(text.as_bytes(), if iges { "iges" } else { "brep" }).unwrap();
                assert!(scene.triangles.len() > 100, "{kind} iges={iges}");
                assert!(
                    (area(&scene) - expected).abs() / expected < 0.025,
                    "{kind} iges={iges}, area={}",
                    area(&scene)
                );
                assert!(scene.points.is_empty());
            }
        }
    }

    #[test]
    fn coarse_authored_precision_preserves_small_curved_preview_detail() {
        let mut model = Model::new();
        let radius = 0.25;
        let root = ogeom_algo::make_sphere(&mut model, Frame::WORLD, radius, TOLERANCES)
            .unwrap()
            .shape;
        let authored = Tolerances::with_scale(ogeom_core::tolerance::CONFUSION / 0.001).unwrap();
        let mut scene = Scene::default();
        append_shape_with_tolerances(&mut scene, &model, &root, authored).unwrap();
        let maximum_sag = scene
            .triangles
            .iter()
            .map(|triangle| {
                let center =
                    (triangle.vertices[0] + triangle.vertices[1] + triangle.vertices[2]) / 3.;
                (radius - center.length()).abs()
            })
            .fold(0., f64::max);
        assert!(
            (area(&scene) - 4. * std::f64::consts::PI * radius * radius).abs() / area(&scene)
                < 0.025
        );
        assert!(
            maximum_sag <= 0.5 * 3f64.sqrt() * 0.003,
            "spherical triangle sag {maximum_sag}"
        );
        let mut tiny_model = Model::new();
        let tiny = ogeom_algo::make_box(
            &mut tiny_model,
            Frame::WORLD,
            (0.0001, 0.0001, 0.0001),
            TOLERANCES,
        )
        .unwrap()
        .shape;
        assert!(append_shape(&mut Scene::default(), &tiny_model, &tiny).is_err());
    }

    #[test]
    fn iges_manifold_and_trimmed_sheet_geometry_are_preserved() {
        for extension in ["iges", "igs"] {
            let text = text_of("box", true);
            let scene = parse(text.as_bytes(), extension).unwrap();
            assert_eq!(scene.triangles.len(), 12);
            assert!((area(&scene) - 52.).abs() < 1e-7);
        }
        let mut model = Model::new();
        let wire = ogeom_algo::make_polygon(
            &mut model,
            &[
                CadPoint::new(0., 0., 0.),
                CadPoint::new(10., 0., 0.),
                CadPoint::new(10., 6., 0.),
            ],
            false,
            TOLERANCES,
        )
        .unwrap()
        .shape;
        let sheet = ogeom_algo::make_prism(&mut model, &wire, Vector::new(0., 0., 5.), TOLERANCES)
            .unwrap()
            .shape;
        let mut document = ogeom_doc::Document::over(model);
        document.add_part("open_l", sheet);
        let text = ogeom_io::write_iges(&document, TOLERANCES).unwrap();
        let scene = parse(text.as_bytes(), "iges").unwrap();
        assert!((area(&scene) - 80.).abs() < 1e-6);
    }

    #[test]
    fn iges_logical_groups_preserve_owned_solids_without_promoting_physical_children() {
        let text = text_of("box", true);
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let mut counts = [0usize; 4];
        for line in &lines {
            if let Some(index) = b"SGDP"
                .iter()
                .position(|section| *section == line.as_bytes()[72])
            {
                counts[index] += 1;
            }
        }
        let index = lines
            .iter()
            .position(|line| line.as_bytes()[72] == b'D' && line[..8].trim() == "186")
            .unwrap();
        let member = lines[index][73..80].trim().parse::<usize>().unwrap();
        lines[index].replace_range(66..68, "02");
        let directory = counts[2] + 1;
        let parameter = counts[3] + 1;
        let fields = |values: [usize; 9]| {
            values.iter().fold(String::new(), |mut text, value| {
                let _ = write!(text, "{value:>8}");
                text
            })
        };
        let first = format!(
            "{}D{directory:>7}",
            fields([402, parameter, 0, 0, 0, 0, 0, 0, 0])
        );
        let second = format!(
            "{}D{:>7}",
            fields([402, 0, 0, 1, 7, 0, 0, 0, 0]),
            directory + 1
        );
        let parameters = format!(
            "{:<64}{directory:>8}P{parameter:>7}",
            format!("402,1,{member};")
        );
        let pindex = lines
            .iter()
            .position(|line| line.as_bytes()[72] == b'P')
            .unwrap();
        lines.splice(pindex..pindex, [first, second]);
        let tindex = lines
            .iter()
            .position(|line| line.as_bytes()[72] == b'T')
            .unwrap();
        lines.insert(tindex, parameters);
        let terminal = format!(
            "S{:>7}G{:>7}D{:>7}P{:>7}",
            counts[0],
            counts[1],
            counts[2] + 2,
            counts[3] + 1
        );
        *lines.last_mut().unwrap() = format!("{terminal:<72}T{:>7}", 1);
        let source = lines.join("\n");
        let scene = parse(source.as_bytes(), "iges").unwrap();
        assert_eq!(scene.triangles.len(), 12);
        assert!((area(&scene) - 52.).abs() < 1e-7);
        lines[index].replace_range(66..68, "01");
        let normalized = promote_iges_group_surfaces(&lines.join("\n")).unwrap();
        assert_eq!(&normalized.lines().nth(index).unwrap()[66..68], "01");
        lines[index].replace_range(66..68, "02");
        let group = lines
            .iter_mut()
            .find(|line| line.as_bytes()[72] == b'P' && line[..64].starts_with("402,"))
            .unwrap();
        group.replace_range(..64, &format!("{:<64}", "402,1,999999;"));
        assert!(parse(lines.join("\n").as_bytes(), "iges").is_err());
    }

    #[test]
    fn malformed_iges_record_ownership_and_truncation_fail() {
        let text = text_of("box", true);
        let lines: Vec<_> = text.lines().collect();
        assert!(parse(lines[..lines.len() - 1].join("\n").as_bytes(), "iges").is_err());
        let mut broken: Vec<String> = lines.iter().map(std::string::ToString::to_string).collect();
        let parameter = broken
            .iter_mut()
            .find(|line| line.as_bytes()[72] == b'P')
            .unwrap();
        parameter.replace_range(64..72, "       0");
        assert!(parse(broken.join("\n").as_bytes(), "iges").is_err());
        let mut broken: Vec<String> = lines.iter().map(std::string::ToString::to_string).collect();
        broken.last_mut().unwrap().replace_range(25..32, "9999999");
        assert!(parse(broken.join("\n").as_bytes(), "iges").is_err());
    }

    #[test]
    fn malformed_brep_counts_nonfinite_and_location_powers_fail() {
        let text = text_of("box", false);
        assert!(load_brep_text(&text.replace("Locations 0", "Locations 99999999999")).is_err());
        assert!(
            load_brep_text(&text.replace("Locations 0", "Locations 1\n2 1 -9223372036854775808 0"))
                .is_err()
        );
        assert!(
            load_brep_text(
                &text.replace("Locations 0", "Locations 1\n1 NaN 0 0 0 0 1 0 0 0 0 1 0")
            )
            .is_err()
        );
        assert!(load_brep_text(&text[..text.len() / 2]).is_err());
        let nested = format!("1 {}1 0 0 1 0", "8 0 1 ".repeat(66));
        assert!(
            validate_geometry_table(&nested.split_whitespace().collect::<Vec<_>>(), 2).is_err()
        );
        let huge_grid = "1 9 0 0 0 0 1 1 500 500 2 2";
        assert!(
            validate_geometry_table(&huge_grid.split_whitespace().collect::<Vec<_>>(), 0).is_err()
        );
    }

    #[test]
    fn scaled_brep_carriers_fail_but_occurrence_placements_survive() {
        let source = text_of("cylinder", false)
            .replace("Locations 0", "Locations 1\n1 2 0 0 10 0 2 0 0 0 0 2 0");
        let normalized = normalize_brep_caches(&source).unwrap();
        let tokens: Vec<_> = normalized.split_whitespace().map(str::to_owned).collect();
        for wanted in [0, 1, 2, 3] {
            let mut altered = tokens.clone();
            let mut selected = None;
            for (at, word) in tokens.iter().enumerate() {
                if wanted == 0 && word == "Fa" {
                    selected = Some(at + 4);
                    break;
                }
                if wanted > 0 && word == "Ed" {
                    let mut cursor = at + 5;
                    loop {
                        let kind = tokens[cursor].parse::<u32>().unwrap();
                        cursor += 1;
                        if kind == 0 {
                            break;
                        }
                        if kind == wanted {
                            selected = Some(
                                cursor
                                    + match kind {
                                        1 => 1,
                                        2 => 2,
                                        3 => 4,
                                        _ => unreachable!(),
                                    },
                            );
                            break;
                        }
                        cursor += match kind {
                            1 | 7 => 4,
                            2 | 4 => 5,
                            3 => 7,
                            5 => 2,
                            6 => 3,
                            _ => panic!("unexpected representation"),
                        };
                    }
                    if selected.is_some() {
                        break;
                    }
                }
            }
            altered[selected.expect("fixture contains each carrier representation")] =
                "1".to_owned();
            let altered = format!("DBRep_DrawableShape\n\n{}", altered[1..].join(" "));
            let error = load_brep_text(&altered).unwrap_err().to_string();
            assert!(
                error.contains("scaled BREP carrier locations are unsupported"),
                "{wanted}: {error}"
            );
        }
        let (mut model, root) = body("box");
        let root = ogeom_algo::transformed(
            &mut model,
            &root,
            ogeom_math::Transform::translation(Vector::new(10., 3., -1.)),
        )
        .unwrap()
        .shape;
        let root = ogeom_algo::make_compound(&mut model, &[root])
            .unwrap()
            .shape;
        let mut placed = ogeom_io::brep::write(&model, &root, TOLERANCES).unwrap();
        assert!(!placed.contains("Locations 0"));
        // Write the standard single-kind marker followed by its 3x4
        // matrix explicitly: the upstream writer currently repeats that
        // marker on every matrix row, which its own reader cannot consume.
        let start = placed.find("Locations").unwrap();
        let end = placed.find("Curve2ds").unwrap();
        placed.replace_range(start..end, "Locations 1\n1\n1 0 0 10\n0 1 0 3\n0 0 1 -1\n");
        let scene = load_brep_text(&placed).unwrap();
        assert!((area(&scene) - 52.).abs() < 1e-8);
        let low = scene.positions().fold(
            Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            |a, p| Vec3::new(a.x.min(p.x), a.y.min(p.y), a.z.min(p.z)),
        );
        let high = scene.positions().fold(
            Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
            |a, p| Vec3::new(a.x.max(p.x), a.y.max(p.y), a.z.max(p.z)),
        );
        assert_eq!(low, Vec3::new(17., 1., 4.));
        assert_eq!(high, Vec3::new(19., 4., 8.));
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "Keep all geometry qualification assertions beside the authored fixture construction."
    )]
    fn rigid_brep_carriers_share_geometry_and_preserve_occurrence_bounds() {
        for mirrored in [false, true] {
            let (mut model, root) = body("box");
            let sign = if mirrored { -1. } else { 1. };
            let carrier = ogeom_math::Transform::from_parts(
                ogeom_math::Matrix3::new(if mirrored {
                    [[1., 0., 0.], [0., -1., 0.], [0., 0., -1.]]
                } else {
                    [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]
                }),
                sign,
                Vector::new(10., 3., -1.),
                1e-9,
            )
            .unwrap();
            let location = ogeom_topo::Location::of(model.add_datum(carrier));
            let repeated = ogeom_topo::Location::of(model.add_datum(carrier));
            let nodes: Vec<_> = model.nodes().map(|(id, _)| Shape::of(id)).collect();
            for node in nodes {
                let mut data = model.node(&node).unwrap().data().clone();
                match &mut data {
                    NodeData::Vertex(vertex) => vertex.point = carrier.apply(vertex.point),
                    NodeData::Face(face) => face.location = location.clone(),
                    NodeData::Edge(edge) => {
                        for representation in &mut edge.representations {
                            match representation {
                                EdgeRepr::Curve3d { location: at, .. } => *at = location.clone(),
                                EdgeRepr::PCurve { location: at, .. }
                                | EdgeRepr::Seam { location: at, .. } => *at = repeated.clone(),
                                _ => {}
                            }
                        }
                        edge.assert_same_parameter(false);
                    }
                    NodeData::Container => {}
                }
                *model.node_mut(&node).unwrap().data_mut() = data;
            }
            let root = ogeom_algo::transformed(
                &mut model,
                &root,
                ogeom_math::Transform::translation(Vector::new(2., -1., 3.)),
            )
            .unwrap()
            .shape;
            let root = ogeom_algo::make_compound(&mut model, &[root])
                .unwrap()
                .shape;
            let mut source = ogeom_io::brep::write(&model, &root, TOLERANCES).unwrap();
            assert!(source.contains("Locations 3"));
            let start = source.find("Locations").unwrap();
            let end = source.find("Curve2ds").unwrap();
            let matrix = format!("1\n{sign} 0 0 10\n0 1 0 3\n0 0 1 -1\n");
            source.replace_range(
                start..end,
                &format!("Locations 3\n{matrix}{matrix}1\n1 0 0 2\n0 1 0 -1\n0 0 1 3\n"),
            );
            // Its 3D-curve writer also emits zero carrier indices; supply
            // the genuine native indices explicitly in this fixture.
            let mut words: Vec<String> = source.split_whitespace().map(str::to_owned).collect();
            for at in 0..words.len() {
                if words[at] != "Ed" {
                    continue;
                }
                let mut cursor = at + 5;
                loop {
                    let kind = words[cursor].parse::<u32>().unwrap();
                    cursor += 1;
                    match kind {
                        0 => break,
                        1 => {
                            words[cursor + 1] = "2".to_owned();
                            cursor += 4;
                        }
                        2 | 4 => cursor += 5,
                        3 => cursor += 7,
                        5 => cursor += 2,
                        6 => cursor += 3,
                        7 => cursor += 4,
                        _ => panic!("unexpected native carrier"),
                    }
                }
            }
            source = format!("DBRep_DrawableShape\n\n{}", words[1..].join(" "));
            let normalized = normalize_brep_caches(&source).unwrap();
            let metadata = carriers::Metadata::parse(&normalized).unwrap();
            let (mut imported, shape) = ogeom_io::brep::read(&normalized, TOLERANCES).unwrap();
            metadata.restore(&mut imported, &shape, TOLERANCES).unwrap();
            for face in explore(&imported, &shape, Filter::OfType(ShapeType::Face)).unwrap() {
                let surface = imported
                    .node(&face)
                    .unwrap()
                    .data()
                    .as_face()
                    .unwrap()
                    .surface;
                for edge in explore(&imported, &face, Filter::OfType(ShapeType::Edge)).unwrap() {
                    let data = imported.node(&edge).unwrap().data().as_edge().unwrap();
                    assert!(data.representations.iter().any(|repr|matches!(repr,EdgeRepr::PCurve {surface:on,..}|EdgeRepr::Seam {surface:on,..} if *on==surface)));
                }
            }
            let scene = load_brep_text(&source).unwrap();
            assert_eq!(scene.triangles.len(), 12);
            assert!((area(&scene) - 52.).abs() < 1e-8);
            let low = scene.positions().fold(
                Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
                |a, p| Vec3::new(a.x.min(p.x), a.y.min(p.y), a.z.min(p.z)),
            );
            let high = scene.positions().fold(
                Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
                |a, p| Vec3::new(a.x.max(p.x), a.y.max(p.y), a.z.max(p.z)),
            );
            assert_eq!(low, Vec3::new(if mirrored { 3. } else { 19. }, 0., 7.));
            assert_eq!(high, Vec3::new(if mirrored { 5. } else { 21. }, 3., 11.));
        }
    }

    #[test]
    fn brep_omitted_planar_pcurves_are_exact_and_reject_off_plane_edges() {
        for malformed in [false, true] {
            let (mut model, root) = body("box");
            let edges: Vec<_> = model
                .nodes()
                .filter(|(_, node)| node.kind() == ShapeType::Edge)
                .map(|(id, _)| Shape::of(id))
                .collect();
            for edge in edges {
                let mut data = model.node(&edge).unwrap().data().as_edge().unwrap().clone();
                data.representations
                    .retain(|r| matches!(r, EdgeRepr::Curve3d { .. }));
                if malformed {
                    let EdgeRepr::Curve3d { curve, .. } = &mut data.representations[0] else {
                        unreachable!()
                    };
                    let geometry = model.geometry().curve(*curve).unwrap();
                    let changed = ogeom_geom::Transformable::transformed(
                        geometry,
                        &ogeom_math::Transform::translation(Vector::new(0.1, 0.2, 0.3)),
                        TOLERANCES,
                    )
                    .unwrap();
                    *curve = model.geometry_mut().add_curve(changed);
                }
                *model.node_mut(&edge).unwrap().data_mut() = NodeData::Edge(Box::new(data));
            }
            let source = ogeom_io::brep::write(&model, &root, TOLERANCES).unwrap();
            let result = load_brep_text(&source);
            if malformed {
                assert!(result.unwrap_err().to_string().contains("off-plane"));
            } else {
                let scene = result.unwrap();
                assert_eq!(scene.triangles.len(), 12);
                assert!((area(&scene) - 52.).abs() < 1e-8);
                let low = scene.positions().fold(
                    Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
                    |a, p| Vec3::new(a.x.min(p.x), a.y.min(p.y), a.z.min(p.z)),
                );
                let high = scene.positions().fold(
                    Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
                    |a, p| Vec3::new(a.x.max(p.x), a.y.max(p.y), a.z.max(p.z)),
                );
                assert_eq!(low, Vec3::new(7., -2., 5.));
                assert_eq!(high, Vec3::new(9., 1., 9.));
            }
        }
        let (mut model, root) = body("cylinder");
        let edges: Vec<_> = model
            .nodes()
            .filter(|(_, n)| n.kind() == ShapeType::Edge)
            .map(|(id, _)| Shape::of(id))
            .collect();
        for edge in edges {
            if let NodeData::Edge(data) = model.node_mut(&edge).unwrap().data_mut() {
                data.representations
                    .retain(|r| matches!(r, EdgeRepr::Curve3d { .. }));
            }
        }
        assert!(
            load_brep_text(&ogeom_io::brep::write(&model, &root, TOLERANCES).unwrap()).is_err()
        );
    }

    #[test]
    fn brep_pcurve_carriers_are_relative_to_edge_occurrences() {
        let (mut model, root) = body("box");
        let shift = ogeom_math::Transform::translation(Vector::new(10., 3., -1.));
        let outward = ogeom_topo::Location::of(model.add_datum(shift));
        let inward = ogeom_topo::Location::of(model.add_datum(shift.inverse().unwrap()));
        let nodes: Vec<_> = model.nodes().map(|(id, _)| Shape::of(id)).collect();
        for shape in nodes {
            let node = model.node(&shape).unwrap();
            let kind = node.kind();
            let mut data = node.data().clone();
            let children = match kind {
                ShapeType::Wire => node
                    .children()
                    .iter()
                    .map(|s| s.located(outward.clone()))
                    .collect(),
                ShapeType::Edge => node
                    .children()
                    .iter()
                    .map(|s| s.located(inward.clone()))
                    .collect(),
                _ => node.children().to_vec(),
            };
            if let NodeData::Edge(edge) = &mut data {
                for repr in &mut edge.representations {
                    match repr {
                        EdgeRepr::Curve3d { location, .. }
                        | EdgeRepr::PCurve { location, .. }
                        | EdgeRepr::Seam { location, .. } => *location = inward.clone(),
                        _ => panic!("unsupported generated representation"),
                    }
                }
                edge.assert_same_parameter(false);
            }
            *model.node_mut(&shape).unwrap() = ogeom_topo::TShape::new(kind, data, children);
        }
        let mut source = ogeom_io::brep::write(&model, &root, TOLERANCES).unwrap();
        let start = source.find("Locations").unwrap();
        let end = source.find("Curve2ds").unwrap();
        // Native order is established by the first edge's inward carrier;
        // the independently stored child edge occurrence is outward.
        source.replace_range(
            start..end,
            "Locations 2\n1\n1 0 0 -10\n0 1 0 -3\n0 0 1 1\n1\n1 0 0 10\n0 1 0 3\n0 0 1 -1\n",
        );
        let mut words: Vec<String> = source.split_whitespace().map(str::to_owned).collect();
        for at in 0..words.len() {
            if words[at] != "Ed" {
                continue;
            }
            let mut cursor = at + 5;
            loop {
                let kind = words[cursor].parse::<u32>().unwrap();
                cursor += 1;
                match kind {
                    0 => break,
                    1 => {
                        words[cursor + 1] = "1".into();
                        cursor += 4;
                    }
                    2 | 4 => cursor += 5,
                    3 => cursor += 7,
                    5 => cursor += 2,
                    6 => cursor += 3,
                    7 => cursor += 4,
                    _ => panic!("bad repr"),
                }
            }
        }
        source = format!("DBRep_DrawableShape\n\n{}", words[1..].join(" "));
        let scene = load_brep_text(&source).unwrap();
        assert_eq!(scene.triangles.len(), 12);
        assert!((area(&scene) - 52.).abs() < 1e-8);
        let low = scene.positions().fold(
            Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            |a, p| Vec3::new(a.x.min(p.x), a.y.min(p.y), a.z.min(p.z)),
        );
        let high = scene.positions().fold(
            Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
            |a, p| Vec3::new(a.x.max(p.x), a.y.max(p.y), a.z.max(p.z)),
        );
        assert_eq!(low, Vec3::new(7., -2., 5.));
        assert_eq!(high, Vec3::new(9., 1., 9.));
    }

    #[test]
    fn brep_left_handed_analytic_charts_preserve_authored_axes() {
        use ogeom_geom::{Curve3d, Surface};
        let frame = Frame::from_axes(
            CadPoint::new(7., -2., 5.),
            ogeom_math::Direction::X,
            ogeom_math::Direction::Y,
            ogeom_math::Direction::Z.reversed(),
            TOLERANCES,
        )
        .unwrap();
        let mut model = Model::new();
        // The primitive builder deliberately canonicalizes cylinders to a
        // right-handed frame. Construct the authored analytic chart itself
        // instead, with a rectangular trim that the BREP file preserves.
        let cylinder = ogeom_geom::CylinderSurface::new(
            ogeom_math::Cylinder::new(frame, 2., TOLERANCES).unwrap(),
            (0., 3.),
        )
        .unwrap();
        let geometry = ogeom_geom::TrimmedSurface::new(
            cylinder.into(),
            (0., std::f64::consts::TAU),
            (0., 3.),
            TOLERANCES,
        )
        .unwrap();
        let surface = model.geometry_mut().add_surface(geometry.into());
        let face = model
            .add_face(
                ogeom_topo::FaceData::new(surface, ogeom_topo::Location::identity()),
                &[],
            )
            .unwrap();
        let curve = model.geometry_mut().add_curve(
            ogeom_geom::CircleCurve::new(ogeom_math::Circle::new(frame, 2., TOLERANCES).unwrap())
                .into(),
        );
        let vertex = model.add_point(CadPoint::new(9., -2., 5.));
        let edge = model
            .add_edge(
                ogeom_topo::EdgeData::on_curve(
                    curve,
                    ogeom_topo::Location::identity(),
                    (0., std::f64::consts::TAU),
                ),
                &[vertex.clone(), vertex],
            )
            .unwrap();
        let root = model.add_compound(&[face, edge]).unwrap();
        let source = ogeom_io::brep::write(&model, &root, TOLERANCES).unwrap();
        let (imported, shape) = brep_frames::read(&source, TOLERANCES).unwrap();
        let surface = explore(&imported, &shape, Filter::OfType(ShapeType::Face))
            .unwrap()
            .into_iter()
            .find_map(|f| {
                let id = imported.node(&f)?.data().as_face()?.surface;
                let geometry = imported.geometry().surface(id)?;
                matches!(geometry, ogeom_geom::SurfaceGeometry::Trimmed(trim) if matches!(trim.basis(),ogeom_geom::SurfaceGeometry::Cylinder(_))).then_some(geometry)
            })
            .unwrap();
        let point = surface
            .point_at(std::f64::consts::FRAC_PI_2, 1., TOLERANCES)
            .unwrap();
        assert!(point.distance(CadPoint::new(7., 0., 4.)) < 1e-10);
        let circle = explore(&imported, &shape, Filter::OfType(ShapeType::Edge))
            .unwrap()
            .into_iter()
            .find_map(|e| {
                let EdgeRepr::Curve3d { curve, .. } =
                    imported.node(&e)?.data().as_edge()?.curve3d()?
                else {
                    return None;
                };
                let geometry = imported.geometry().curve(*curve)?;
                matches!(geometry, ogeom_geom::Curve::Circle(_)).then_some(geometry)
            })
            .unwrap();
        assert!(
            (circle
                .point_at(std::f64::consts::FRAC_PI_2, TOLERANCES)
                .unwrap()
                .y
                - 0.)
                .abs()
                < 1e-10
        );
        let scene = load_brep_text(&source).unwrap();
        let expected = 12. * std::f64::consts::PI;
        assert!((area(&scene) - expected).abs() / expected < 0.025);
        let low = scene.positions().fold(
            Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            |a, p| Vec3::new(a.x.min(p.x), a.y.min(p.y), a.z.min(p.z)),
        );
        let high = scene.positions().fold(
            Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
            |a, p| Vec3::new(a.x.max(p.x), a.y.max(p.y), a.z.max(p.z)),
        );
        assert!((low - Vec3::new(5., -4., 2.)).length() < 0.02);
        assert!((high - Vec3::new(9., 0., 5.)).length() < 0.02);
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "Keep all geometry qualification assertions beside the authored fixture construction."
    )]
    fn iges_unsewn_preview_retains_every_face_and_accounts_only_owned_alternatives() {
        let mut model = Model::new();
        let wire = ogeom_algo::make_polygon(
            &mut model,
            &[
                CadPoint::new(0., 0., 0.),
                CadPoint::new(10., 0., 0.),
                CadPoint::new(10., 6., 0.),
            ],
            false,
            TOLERANCES,
        )
        .unwrap()
        .shape;
        let sheet = ogeom_algo::make_prism(&mut model, &wire, Vector::new(0., 0., 5.), TOLERANCES)
            .unwrap()
            .shape;
        let mut document = ogeom_doc::Document::over(model);
        document.add_part("unsewn_l", sheet);
        let source = ogeom_io::write_iges(&document, TOLERANCES).unwrap();
        let authored_faces = source
            .lines()
            .filter(|l| l.as_bytes()[72] == b'D' && l[..8].trim() == "144")
            .count()
            / 2;
        assert_eq!(authored_faces, 2);
        let imported = iges_preview::read(&source, TOLERANCES).unwrap();
        assert_eq!(imported.report.warnings, [] as [std::string::String; 0]);
        let model = &imported.model;
        let imported_faces: usize = imported
            .sheets
            .iter()
            .map(|s| {
                explore(model, s, Filter::OfType(ShapeType::Face))
                    .unwrap()
                    .len()
            })
            .sum();
        assert_eq!(imported_faces, authored_faces);
        let mut rendered_faces = 0;
        for shape in &imported.sheets {
            let (meshes, _) = ogeom_mesh::face_meshes_for(
                model,
                shape,
                ogeom_mesh::Deflection {
                    chord: 0.01,
                    angular: 0.2,
                    min_segments: 4,
                    max_segments: 4096,
                },
                None,
                TOLERANCES,
            )
            .unwrap();
            assert!(meshes.iter().all(|(_, m)| {
                m.as_ref()
                    .is_ok_and(|m| !m.triangles.is_empty() && m.deflection_met)
            }));
            rendered_faces += meshes.len();
        }
        assert_eq!(rendered_faces, authored_faces);
        let scene = parse(source.as_bytes(), "iges").unwrap();
        assert_eq!(scene.triangles.len(), 4);
        assert!((area(&scene) - 80.).abs() < 1e-8);
        let low = scene.positions().fold(
            Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            |a, p| Vec3::new(a.x.min(p.x), a.y.min(p.y), a.z.min(p.z)),
        );
        let high = scene.positions().fold(
            Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
            |a, p| Vec3::new(a.x.max(p.x), a.y.max(p.y), a.z.max(p.z)),
        );
        assert_eq!(low, Vec3::new(0., 0., 0.));
        assert_eq!(high, Vec3::new(10., 6., 5.));
        let with_alternate = |owned: bool, missing: bool| {
            let mut lines: Vec<String> = source.lines().map(str::to_owned).collect();
            let directory_count = lines.iter().filter(|l| l.as_bytes()[72] == b'D').count();
            let parameter_count = lines.iter().filter(|l| l.as_bytes()[72] == b'P').count();
            let new_directory = directory_count + 1;
            let new_parameter = parameter_count + 1;
            let at = lines
                .iter()
                .position(|l| l.as_bytes()[72] == b'D' && l[..8].trim() == "110")
                .unwrap();
            let original = lines[at][73..80].trim().parse::<usize>().unwrap();
            let mut first = lines[at].clone();
            let mut second = lines[at + 1].clone();
            first.replace_range(8..16, &format!("{new_parameter:>8}"));
            first.replace_range(64..72, "00010000");
            first.replace_range(73..80, &format!("{new_directory:>7}"));
            second.replace_range(73..80, &format!("{:>7}", new_directory + 1));
            let mut parameter = lines
                .iter()
                .find(|l| {
                    l.as_bytes()[72] == b'P'
                        && l[64..72].trim().parse::<usize>().unwrap() == original
                })
                .unwrap()
                .clone();
            parameter.replace_range(64..72, &format!("{new_directory:>8}"));
            parameter.replace_range(73..80, &format!("{new_parameter:>7}"));
            if owned {
                let boundary = lines
                    .iter_mut()
                    .find(|l| l.as_bytes()[72] == b'P' && l[..64].trim_start().starts_with("142,"))
                    .unwrap();
                let mut fields: Vec<String> = boundary[..64]
                    .trim()
                    .split(',')
                    .map(str::to_owned)
                    .collect();
                fields[3] = if missing {
                    "999999".into()
                } else {
                    new_directory.to_string()
                };
                let data = fields.join(",");
                assert!(data.len() <= 64);
                boundary.replace_range(..64, &format!("{data:<64}"));
            }
            let pindex = lines.iter().position(|l| l.as_bytes()[72] == b'P').unwrap();
            lines.splice(pindex..pindex, [first, second]);
            let tindex = lines.iter().position(|l| l.as_bytes()[72] == b'T').unwrap();
            lines.insert(tindex, parameter);
            let mut terminal = lines.last().unwrap().clone();
            terminal.replace_range(17..24, &format!("{:>7}", directory_count + 2));
            terminal.replace_range(25..32, &format!("{:>7}", parameter_count + 1));
            *lines.last_mut().unwrap() = terminal;
            lines.join("\n")
        };
        let owned = parse(with_alternate(true, false).as_bytes(), "iges").unwrap();
        assert_eq!(owned.triangles.len(), 4);
        assert!((area(&owned) - 80.).abs() < 1e-8);
        assert!(
            parse(with_alternate(false, false).as_bytes(), "iges")
                .unwrap_err()
                .to_string()
                .contains("unsupported/unconsumed")
        );
        assert!(parse(with_alternate(true, true).as_bytes(), "iges").is_err());
    }

    #[test]
    fn iges_authored_resolution_respects_hollerith_units_and_limits() {
        let mut fields = vec!["1H,".to_owned(), "1H;".to_owned(), "8HA,B;prod".to_owned()];
        fields.extend(
            [
                "0H", "0H", "0H", "32", "308", "15", "308", "15", "0H", "1", "2", "2HMM", "1",
                "0.01", "0H", "0.0001", "1000",
            ]
            .map(str::to_owned),
        );
        let deck = |fields: &[String], delimiter: char, record: char| {
            let text = format!("{}{record}", fields.join(&delimiter.to_string()));
            text.as_bytes()
                .chunks(72)
                .enumerate()
                .map(|(i, chunk)| {
                    format!("{:<72}G{:>7}", std::str::from_utf8(chunk).unwrap(), i + 1)
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(
            (iges_tolerances(&deck(&fields, ',', ';'))
                .unwrap()
                .confusion()
                - 0.0001)
                .abs()
                < 1e-12
        );
        fields[13] = "6".to_owned();
        fields[14] = "1HM".to_owned();
        fields[18] = "1D-6".to_owned();
        assert!(
            (iges_tolerances(&deck(&fields, ',', ';'))
                .unwrap()
                .confusion()
                - 0.001)
                .abs()
                < 1e-12
        );
        fields[13] = "3".to_owned();
        fields[14] = "4HINCH".to_owned();
        fields[18] = "0.0001".to_owned();
        fields[0] = "1H|".to_owned();
        fields[1] = "1H^".to_owned();
        assert!(
            (iges_tolerances(&deck(&fields, '|', '^'))
                .unwrap()
                .confusion()
                - 0.00254)
                .abs()
                < 1e-12
        );
        fields[13] = "2".to_owned();
        fields[14] = "2HMM".to_owned();
        for invalid in ["0", "-1", "NaN", "1e100", "0.2"] {
            fields[18] = invalid.to_owned();
            assert!(
                iges_tolerances(&deck(&fields, '|', '^')).is_err(),
                "{invalid}"
            );
        }
        fields[18] = String::new();
        assert_eq!(
            iges_tolerances(&deck(&fields, '|', '^')).unwrap(),
            TOLERANCES
        );
        let original = text_of("box", true);
        assert!((area(&parse(original.as_bytes(), "iges").unwrap()) - 52.).abs() < 1e-7);
        let mut lines: Vec<_> = original.lines().map(str::to_owned).collect();
        let indices: Vec<_> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.as_bytes()[72] == b'G')
            .map(|(i, _)| i)
            .collect();
        let global = indices
            .iter()
            .map(|index| &lines[*index][..72])
            .collect::<String>();
        assert!(global.contains(",2,2HMM,"));
        let global = global.replace(",2,2HMM,", ",6,1HM,");
        for (i, index) in indices.into_iter().enumerate() {
            let start = i * 72;
            let chunk = &global[start..(start + 72).min(global.len())];
            lines[index] = format!("{chunk:<72}G{:>7}", i + 1);
        }
        let meters = lines.join("\n");
        assert!((iges_tolerances(&meters).unwrap().confusion() - 1e-4).abs() < 1e-12);
        let scene = parse(meters.as_bytes(), "iges").unwrap();
        assert!((area(&scene) - 52_000_000.).abs() < 1e-3);
        assert!(scene.positions().all(|p| p.x >= 7000. - 1e-8
            && p.x <= 9000. + 1e-8
            && p.y >= -2000. - 1e-8
            && p.y <= 1000. + 1e-8
            && p.z >= 5000. - 1e-8
            && p.z <= 9000. + 1e-8));
    }

    #[test]
    fn brep_cached_polygon_parameters_and_legacy_continuities_are_validated() {
        let text = text_of("box", false)
            .replace(
                "PolygonOnTriangulations 0",
                "PolygonOnTriangulations 1\n2 1 2\np 0.1 1 0 1",
            )
            .replace(
                "Triangulations 0",
                "Triangulations 1\n3 1 1 0.1\n0 0 0 1 0 0 0 1 0\n0 0 1 0 0 1\n1 2 3",
            );
        let scene = load_brep_text(&text).unwrap();
        assert_eq!(scene.triangles.len(), 12);
        assert!((area(&scene) - 52.).abs() < 1e-8);
        let uncached = load_brep_text(&text_of("box", false)).unwrap();
        for (actual, expected) in scene.triangles.iter().zip(&uncached.triangles) {
            assert_eq!(actual.vertices, expected.vertices);
        }
        assert!(load_brep_text(&text.replace("p 0.1 1 0 1", "p 0.1 1 0")).is_err());
        let cylinder = text_of("cylinder", false);
        assert!(cylinder.contains(" C0"));
        let legacy = cylinder.replace(" C0", "C0");
        let original = load_brep_text(&cylinder).unwrap();
        let scene = load_brep_text(&legacy).unwrap();
        assert_eq!(scene.triangles.len(), original.triangles.len());
        assert_eq!(area(&scene), area(&original));
    }
}
