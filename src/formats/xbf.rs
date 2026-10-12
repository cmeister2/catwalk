//! Static XCAF documents and standalone BREP, decoded from native OCAF/BinTools binary records.
//! Layout facts follow the primary OCCT serializers, not an ASCII alias:
//! <https://github.com/Open-Cascade-SAS/OCCT/tree/V7_9_0/src/BinTools>
//! <https://github.com/Open-Cascade-SAS/OCCT/tree/V7_9_0/src/BinLDrivers>
use super::transform::Transform;
use crate::{
    Result,
    scene::{Scene, Vec3},
};
use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    path::Path,
};

/// Maximum native binary CAD document byte length.
const MAX_BYTES: usize = 16 * 1024 * 1024;
/// Maximum native table records or expanded occurrences.
const MAX_RECORDS: usize = 100_000;
/// Maximum recursive reference, geometry or partition depth.
const MAX_DEPTH: usize = 64;
/// Maximum aggregate native geometry components.
const MAX_COMPONENTS: usize = 1_000_000;

/// Native topology occurrence, including table index, location and orientation.
#[derive(Clone, Copy, Debug)]
struct ShapeRef {
    /// Zero-based native shape-table index.
    node: usize,
    /// Native placement index; zero selects identity.
    location: usize,
    /// Native topology orientation code for this occurrence.
    orientation: u8,
}

/// One native topology record with its geometry and child references.
struct Shape {
    /// Native schema or topology kind selecting the record interpretation.
    kind: u8,
    /// Validated shape geometry encoded in ASCII BREP syntax.
    geometry: String,
    /// Native topology flags retained in the BREP translation.
    flags: u8,
    /// Native child shape occurrences with their locations and orientations.
    children: Vec<ShapeRef>,
}

/// Decoded native geometry/topology tables and quick-part offset indices.
#[derive(Default)]
struct Tables {
    /// Decoded native placement table.
    locations: Vec<Transform>,
    /// Decoded three-dimensional curves in ASCII BREP syntax.
    curves: Vec<String>,
    /// Decoded two-dimensional parameter curves in ASCII BREP syntax.
    pcurves: Vec<String>,
    /// Decoded surface records in ASCII BREP syntax.
    surfaces: Vec<String>,
    /// Decoded native topology records.
    shapes: Vec<Shape>,
    /// Quick-part byte offsets mapped to decoded shape indices.
    shape_positions: HashMap<usize, usize>,
    /// Quick-part byte offsets mapped to decoded location indices.
    location_positions: HashMap<usize, usize>,
    /// Typed quick-part byte offsets mapped to decoded geometry indices.
    geometry_positions: HashMap<(u8, usize), usize>,
}

/// Bounds-checked cursor over one binary document payload.
struct Cursor<'a> {
    /// Borrowed binary input payload.
    bytes: &'a [u8],
    /// Byte offset of the next unread native field.
    pos: usize,
}
impl<'a> Cursor<'a> {
    /// Start a bounds-checked cursor at the beginning of a binary payload.
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    /// Consume an exact byte range and advance the cursor.
    ///
    /// # Errors
    ///
    /// The range overflows or the input is truncated.
    fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(size).ok_or("XBF offset overflow")?;
        let data = self
            .bytes
            .get(self.pos..end)
            .ok_or("truncated XBF record")?;
        self.pos = end;
        Ok(data)
    }
    /// Read one byte from the binary document.
    ///
    /// # Errors
    ///
    /// The input is truncated.
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    /// Read a native boolean encoded as zero or one.
    ///
    /// # Errors
    ///
    /// The input is truncated or the flag uses another value.
    fn boolean(&mut self) -> Result<u8> {
        let value = self.byte()?;
        if value > 1 {
            return Err("invalid XBF boolean".into());
        }
        Ok(value)
    }
    /// Read a little-endian signed 32-bit value.
    ///
    /// # Errors
    ///
    /// The input is truncated.
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }
    /// Read a little-endian unsigned 64-bit value.
    ///
    /// # Errors
    ///
    /// The input is truncated.
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
    /// Read a nonnegative count bounded by the supplied limit.
    ///
    /// # Errors
    ///
    /// The input is truncated, the count is negative or it exceeds the limit.
    fn count(&mut self, limit: usize) -> Result<usize> {
        let count = usize::try_from(self.i32()?).map_err(|_| "negative XBF count")?;
        if count > limit {
            return Err("XBF count exceeds preview limit".into());
        }
        Ok(count)
    }
    /// Read a bounded nonnegative native table index.
    ///
    /// # Errors
    ///
    /// The value is truncated, negative or outside the supplied index limit.
    fn index(&mut self, limit: usize) -> Result<usize> {
        let index = self.count(limit)?;
        if index == 0 {
            return Err("XBF geometry has a null table reference".into());
        }
        Ok(index)
    }
    /// Read a finite little-endian binary64 value.
    ///
    /// # Errors
    ///
    /// The input is truncated or the value is non-finite.
    fn real(&mut self) -> Result<f64> {
        let value = f64::from_le_bytes(self.take(8)?.try_into()?);
        if !value.is_finite() {
            return Err("XBF geometry contains non-finite values".into());
        }
        Ok(value)
    }
    /// Advance to the next native four-byte attribute alignment boundary.
    ///
    /// # Errors
    ///
    /// Alignment would advance beyond the payload.
    fn align(&mut self) -> Result<()> {
        let size = (4 - self.pos % 4) % 4;
        self.take(size)?;
        Ok(())
    }
    /// Decode a bounded string using native one-byte or UTF-16 storage.
    ///
    /// # Errors
    ///
    /// Length/encoding is invalid or the string payload is truncated.
    fn string(&mut self, width: usize) -> Result<String> {
        let size = self.count(MAX_COMPONENTS)?;
        let data = self.take(size.checked_mul(width).ok_or("XBF string overflow")?)?;
        if width == 1 {
            Ok(std::str::from_utf8(data)?.trim_end_matches('\0').to_owned())
        } else {
            let data: Vec<u16> = data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            Ok(String::from_utf16(&data)?.trim_end_matches('\0').to_owned())
        }
    }
    /// Read and validate a named native geometry-table header.
    ///
    /// # Errors
    ///
    /// The name, record count or table extent is invalid or unsupported.
    fn table_header(&mut self, expected: &str) -> Result<usize> {
        while self
            .bytes
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
        let start = self.pos;
        while self.bytes.get(self.pos).is_some_and(|b| *b != b'\n') {
            self.pos += 1;
        }
        let line = std::str::from_utf8(
            self.bytes
                .get(start..self.pos)
                .ok_or("truncated XBF table")?,
        )?;
        self.take(1)?;
        let mut words = line.split_whitespace();
        if words.next() != Some(expected) {
            return Err(format!("XBF expected {expected} table").into());
        }
        let count = words
            .next()
            .ok_or("XBF table lacks a count")?
            .parse::<usize>()?;
        if count > MAX_RECORDS || words.next().is_some() {
            return Err("invalid or oversized XBF table".into());
        }
        Ok(count)
    }
    /// Read a bounded native quick-part back-reference.
    ///
    /// # Errors
    ///
    /// The reference type, offset or cursor position is invalid.
    fn reference(&mut self, kind: u8) -> Result<usize> {
        let start = self.pos.checked_sub(1).ok_or("invalid XBF reference")?;
        let delta = match kind {
            1 => u64::from(self.byte()?),
            2 => u64::from(u16::from_le_bytes(self.take(2)?.try_into()?)),
            3 => u64::from(u32::from_le_bytes(self.take(4)?.try_into()?)),
            4 => self.u64()?,
            _ => return Err("invalid XBF reference type".into()),
        };
        let delta = usize::try_from(delta)?;
        if delta == 0 {
            return Err("zero XBF backward reference".into());
        }
        start
            .checked_sub(delta)
            .ok_or_else(|| "XBF reference precedes the file".into())
    }
}

/// Append finite native real values in the ASCII BREP interchange syntax.
///
/// # Errors
///
/// The binary values are truncated/non-finite or formatting fails.
fn real_text(cursor: &mut Cursor<'_>, output: &mut String, count: usize) -> Result<()> {
    if count > MAX_COMPONENTS {
        return Err("XBF geometry exceeds one million components".into());
    }
    for _ in 0..count {
        write!(output, "{} ", cursor.real()?)?;
    }
    if output.len() > MAX_BYTES {
        return Err("decoded XBF geometry exceeds 16 MiB".into());
    }
    Ok(())
}

/// Read a supported bounded spline degree.
///
/// # Errors
///
/// The degree is truncated, invalid or exceeds the supported bound.
fn degree(cursor: &mut Cursor<'_>) -> Result<usize> {
    let degree = usize::from(u16::from_le_bytes(cursor.take(2)?.try_into()?));
    if !(1..=64).contains(&degree) {
        return Err("XBF spline degree is outside 1..64".into());
    }
    Ok(degree)
}

/// `BinTools` curve payloads have the same actual geometry as their BREP records.
fn curve(cursor: &mut Cursor<'_>, dimension: usize, depth: usize) -> Result<String> {
    if depth > MAX_DEPTH {
        return Err("XBF nested curves exceed 64 levels".into());
    }
    let kind = cursor.byte()?;
    let mut out = format!("{kind} ");
    match kind {
        1 => real_text(cursor, &mut out, dimension * 2)?,
        2..=5 => {
            real_text(cursor, &mut out, if dimension == 2 { 6 } else { 12 })?;
            real_text(cursor, &mut out, if matches!(kind, 3 | 5) { 2 } else { 1 })?;
        }
        6 | 7 => {
            let rational = cursor.boolean()?;
            let periodic = if kind == 7 { cursor.boolean()? } else { 0 };
            let degree = degree(cursor)?;
            let poles = if kind == 7 {
                cursor.count(MAX_RECORDS)?
            } else {
                degree + 1
            };
            let knots = if kind == 7 {
                cursor.count(MAX_RECORDS)?
            } else {
                2
            };
            if poles < 2 || knots < 2 {
                return Err("invalid XBF spline sizes".into());
            }
            // Exact Bezier conversion: clamped B-spline with the same poles.
            out = format!("7 {rational} {periodic} {degree} {poles} {knots} ");
            real_text(
                cursor,
                &mut out,
                poles * (dimension + usize::from(rational)),
            )?;
            if kind == 6 {
                write!(out, "0 {} 1 {} ", degree + 1, degree + 1)?;
            } else {
                for _ in 0..knots {
                    write!(out, "{} {} ", cursor.real()?, cursor.count(degree + 1)?)?;
                }
            }
        }
        8 => {
            real_text(cursor, &mut out, 2)?;
            out.push_str(&curve(cursor, dimension, depth + 1)?);
        }
        9 => {
            real_text(cursor, &mut out, if dimension == 3 { 4 } else { 1 })?;
            out.push_str(&curve(cursor, dimension, depth + 1)?);
        }
        _ => return Err(format!("unsupported XBF curve representation {kind}").into()),
    }
    Ok(out)
}

/// Translate one native surface record to validated BREP text.
///
/// # Errors
///
/// The surface schema, numeric payload or recursion/spline limits are unsupported or invalid.
fn surface(cursor: &mut Cursor<'_>, depth: usize) -> Result<String> {
    if depth > MAX_DEPTH {
        return Err("XBF nested surfaces exceed 64 levels".into());
    }
    let kind = cursor.byte()?;
    let mut out = format!("{kind} ");
    match kind {
        1..=5 => {
            real_text(cursor, &mut out, 12)?;
            real_text(
                cursor,
                &mut out,
                match kind {
                    1 => 0,
                    2 | 4 => 1,
                    _ => 2,
                },
            )?;
        }
        6 | 7 => {
            real_text(cursor, &mut out, if kind == 6 { 3 } else { 6 })?;
            out.push_str(&curve(cursor, 3, depth + 1)?);
        }
        8 | 9 => {
            let ur = cursor.boolean()?;
            let vr = cursor.boolean()?;
            let up = if kind == 9 { cursor.boolean()? } else { 0 };
            let vp = if kind == 9 { cursor.boolean()? } else { 0 };
            let ud = degree(cursor)?;
            let vd = degree(cursor)?;
            let un = if kind == 9 {
                cursor.count(MAX_RECORDS)?
            } else {
                ud + 1
            };
            let vn = if kind == 9 {
                cursor.count(MAX_RECORDS)?
            } else {
                vd + 1
            };
            let uk = if kind == 9 {
                cursor.count(MAX_RECORDS)?
            } else {
                2
            };
            let vk = if kind == 9 {
                cursor.count(MAX_RECORDS)?
            } else {
                2
            };
            let cells = un.checked_mul(vn).ok_or("XBF spline grid overflow")?;
            if cells > MAX_RECORDS || un < 2 || vn < 2 || uk < 2 || vk < 2 {
                return Err("invalid or oversized XBF surface grid".into());
            }
            out = format!("9 {ur} {vr} {up} {vp} {ud} {vd} {un} {vn} {uk} {vk} ");
            real_text(cursor, &mut out, cells * (3 + usize::from(ur | vr)))?;
            if kind == 8 {
                write!(out, "0 {} 1 {} 0 {} 1 {} ", ud + 1, ud + 1, vd + 1, vd + 1)?;
            } else {
                for (count, degree) in [(uk, ud), (vk, vd)] {
                    for _ in 0..count {
                        write!(out, "{} {} ", cursor.real()?, cursor.count(degree + 1)?)?;
                    }
                }
            }
        }
        10 => {
            real_text(cursor, &mut out, 4)?;
            out.push_str(&surface(cursor, depth + 1)?);
        }
        11 => {
            real_text(cursor, &mut out, 1)?;
            out.push_str(&surface(cursor, depth + 1)?);
        }
        _ => return Err(format!("unsupported XBF surface representation {kind}").into()),
    }
    Ok(out)
}

/// Read a finite native affine placement matrix.
///
/// # Errors
///
/// The matrix payload is truncated or contains non-finite numbers.
fn read_transform(cursor: &mut Cursor<'_>) -> Result<Transform> {
    let mut transform = Transform::IDENTITY;
    for row in 0..3 {
        for column in &mut transform.0 {
            column[row] = cursor.real()?;
        }
    }
    Ok(transform)
}

/// Invert a finite nonsingular affine placement.
///
/// # Errors
///
/// The matrix is singular or its inverse exceeds numeric range.
fn inverse(transform: Transform) -> Result<Transform> {
    let c = transform.0;
    let a = Vec3::new(c[0][0], c[0][1], c[0][2]);
    let b = Vec3::new(c[1][0], c[1][1], c[1][2]);
    let d = Vec3::new(c[2][0], c[2][1], c[2][2]);
    let determinant = a.dot(b.cross(d));
    if !determinant.is_finite() || determinant.abs() < 1e-30 {
        return Err("singular XBF location".into());
    }
    let rows = [
        b.cross(d) / determinant,
        d.cross(a) / determinant,
        a.cross(b) / determinant,
    ];
    let mut result = Transform::IDENTITY;
    for (row, r) in rows.iter().enumerate() {
        result.0[0][row] = r.x;
        result.0[1][row] = r.y;
        result.0[2][row] = r.z;
    }
    let translation = Vec3::new(c[3][0], c[3][1], c[3][2]);
    let offset = -result.apply(translation);
    result.0[3] = [offset.x, offset.y, offset.z, 1.0];
    Ok(result)
}

/// Compose a placement raised to a bounded signed integer power.
///
/// # Errors
///
/// The exponent exceeds limits or a negative power requires an invalid inverse.
fn power(mut transform: Transform, exponent: i32) -> Result<Transform> {
    if exponent.unsigned_abs() > 1024 {
        return Err("XBF location power exceeds 1024".into());
    }
    if exponent < 0 {
        transform = inverse(transform)?;
    }
    let mut result = Transform::IDENTITY;
    for _ in 0..exponent.unsigned_abs() {
        result = result.then(transform);
    }
    if result.0.iter().flatten().any(|v| !v.is_finite()) {
        return Err("XBF location composition overflow".into());
    }
    Ok(result)
}

impl Tables {
    /// Read or reuse a native quick-part location record.
    ///
    /// # Errors
    ///
    /// The placement/back-reference is invalid or exceeds nesting/table limits.
    fn location(&mut self, cursor: &mut Cursor<'_>, depth: usize) -> Result<usize> {
        if depth > MAX_DEPTH || self.locations.len() >= MAX_RECORDS {
            return Err("XBF location hierarchy exceeds preview limits".into());
        }
        let pos = cursor.pos;
        let kind = cursor.byte()?;
        if matches!(kind, 7 | 8) {
            return Ok(0);
        }
        if (1..=4).contains(&kind) {
            let reference = cursor.reference(kind)?;
            return self
                .location_positions
                .get(&reference)
                .copied()
                .ok_or_else(|| "unknown/cyclic XBF location reference".into());
        }
        let transform = match kind {
            6 => read_transform(cursor)?,
            5 => {
                let mut result = Transform::IDENTITY;
                loop {
                    let id = self.location(cursor, depth + 1)?;
                    if id == 0 {
                        break;
                    }
                    let exponent = cursor.i32()?;
                    result = power(self.locations[id - 1], exponent)?.then(result);
                }
                result
            }
            _ => return Err("unsupported XBF location representation".into()),
        };
        let id = self.locations.len() + 1;
        self.locations.push(transform);
        self.location_positions.insert(pos, id);
        Ok(id)
    }
    /// Read or reuse a native typed geometry-table record.
    ///
    /// # Errors
    ///
    /// The schema/back-reference is invalid or exceeds geometry limits.
    fn geometry(&mut self, cursor: &mut Cursor<'_>, expected: u8) -> Result<usize> {
        let pos = cursor.pos;
        let kind = cursor.byte()?;
        if kind == expected + 1 {
            return Err("XBF topology references empty curve/surface geometry".into());
        }
        if (1..=4).contains(&kind) {
            let reference = cursor.reference(kind)?;
            return self
                .geometry_positions
                .get(&(expected, reference))
                .copied()
                .ok_or_else(|| "unknown XBF geometry reference".into());
        }
        if kind != expected {
            return Err("XBF geometry record has the wrong type".into());
        }
        let text = match expected {
            9 => curve(cursor, 3, 0)?,
            11 => curve(cursor, 2, 0)?,
            13 => surface(cursor, 0)?,
            _ => return Err("unsupported XBF geometry object".into()),
        };
        let table = match expected {
            9 => &mut self.curves,
            11 => &mut self.pcurves,
            _ => &mut self.surfaces,
        };
        if table.len() >= MAX_RECORDS {
            return Err("XBF geometry table exceeds 100000 records".into());
        }
        table.push(text);
        let id = table.len();
        self.geometry_positions.insert((expected, pos), id);
        Ok(id)
    }
    /// Read or reuse quick-part topology with its native location and orientation.
    ///
    /// # Errors
    ///
    /// The topology schema, reference or bounded child hierarchy is invalid.
    fn shape(&mut self, cursor: &mut Cursor<'_>, depth: usize) -> Result<Option<ShapeRef>> {
        if depth > MAX_DEPTH || self.shapes.len() >= MAX_RECORDS {
            return Err("XBF shape hierarchy exceeds preview limits".into());
        }
        let pos = cursor.pos;
        let kind = cursor.byte()?;
        if matches!(kind, 198 | 199) {
            return Ok(None);
        }
        if (1..=4).contains(&kind) {
            let reference = cursor.reference(kind)?;
            let node = *self
                .shape_positions
                .get(&reference)
                .ok_or("unknown/cyclic XBF shape reference")?;
            let location = self.location(cursor, 0)?;
            let orientation = cursor.byte()?;
            if orientation > 3 {
                return Err("invalid XBF shape orientation".into());
            }
            return Ok(Some(ShapeRef {
                node,
                location,
                orientation,
            }));
        }
        if !(200..=231).contains(&kind) {
            return Err("invalid XBF shape tag".into());
        }
        let orientation = (kind - 200) & 3;
        let kind = (kind - 200) >> 2;
        let location = self.location(cursor, 0)?;
        let geometry = self.quick_shape_geometry(cursor, kind)?;
        let flags = cursor.byte()?;
        if flags & 128 != 0 {
            return Err("invalid XBF shape flags".into());
        }
        let mut children = Vec::new();
        while let Some(child) = self.shape(cursor, depth + 1)? {
            if children.len() >= MAX_RECORDS {
                return Err("XBF shape has too many children".into());
            }
            children.push(child);
        }
        let node = self.shapes.len();
        self.shapes.push(Shape {
            kind,
            geometry,
            flags,
            children,
        });
        self.shape_positions.insert(pos, node);
        Ok(Some(ShapeRef {
            node,
            location,
            orientation,
        }))
    }
    /// Translate quick-part shape geometry into the corresponding BREP payload.
    ///
    /// # Errors
    ///
    /// The shape/representation schema, numbers or geometry references are invalid or unsupported.
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
    )]
    fn quick_shape_geometry(&mut self, cursor: &mut Cursor<'_>, kind: u8) -> Result<String> {
        let mut out = String::new();
        match kind {
            7 => {
                real_text(cursor, &mut out, 4)?;
                loop {
                    let rep = cursor.byte()?;
                    if rep == 0 {
                        break;
                    }
                    cursor.real()?;
                    match rep {
                        1 => {
                            self.geometry(cursor, 9)?;
                        }
                        2 => {
                            self.geometry(cursor, 11)?;
                            self.geometry(cursor, 13)?;
                        }
                        3 => {
                            cursor.real()?;
                            self.geometry(cursor, 13)?;
                        }
                        _ => return Err("unsupported XBF vertex representation".into()),
                    }
                    self.location(cursor, 0)?;
                }
                out.push_str("0 0\n");
            }
            6 => {
                real_text(cursor, &mut out, 1)?;
                let flags = cursor.byte()?;
                if flags & !7 != 0 {
                    return Err("invalid XBF edge flags".into());
                }
                writeln!(
                    out,
                    "{} {} {}",
                    flags & 1,
                    (flags >> 1) & 1,
                    (flags >> 2) & 1
                )?;
                loop {
                    let rep = cursor.byte()?;
                    if rep == 0 {
                        break;
                    }
                    write!(out, "{rep} ")?;
                    match rep {
                        1 => {
                            let curve = self.geometry(cursor, 9)?;
                            let loc = self.location(cursor, 0)?;
                            self.check_representation_location(loc)?;
                            write!(out, "{curve} {loc} ")?;
                            real_text(cursor, &mut out, 2)?;
                        }
                        2 | 3 => {
                            let curve = self.geometry(cursor, 11)?;
                            write!(out, "{curve} ")?;
                            if rep == 3 {
                                let curve = self.geometry(cursor, 11)?;
                                let continuity = cursor.byte()?;
                                write!(out, "{curve} {} ", continuity_text(continuity)?)?;
                            }
                            let surface = self.geometry(cursor, 13)?;
                            let loc = self.location(cursor, 0)?;
                            self.check_representation_location(loc)?;
                            write!(out, "{surface} {loc} ")?;
                            real_text(cursor, &mut out, 2)?;
                        }
                        4 => {
                            write!(out, "{} ", continuity_text(cursor.byte()?)?)?;
                            for _ in 0..2 {
                                let s = self.geometry(cursor, 13)?;
                                let l = self.location(cursor, 0)?;
                                write!(out, "{s} {l} ")?;
                            }
                        }
                        _ => {
                            return Err(
                                "XBF polygon-only edge representations are unsupported".into()
                            );
                        }
                    }
                    out.push('\n');
                }
                out.push_str("0\n");
            }
            4 => {
                write!(out, "{} ", cursor.boolean()?)?;
                real_text(cursor, &mut out, 1)?;
                let surface = self.geometry(cursor, 13)?;
                let loc = self.location(cursor, 0)?;
                self.check_representation_location(loc)?;
                if surface == 0 {
                    return Err("XBF face has no analytic surface".into());
                }
                writeln!(out, "{surface} {loc}")?;
                match cursor.byte()? {
                    0 | 1 => {}
                    _ => return Err("XBF faces with stored triangulations are unsupported".into()),
                }
            }
            0..=3 | 5 => {}
            _ => return Err("unsupported XBF topology type".into()),
        }
        Ok(out)
    }
}

/// Map a native continuity enumeration to its BREP spelling.
///
/// # Errors
///
/// The continuity value is outside the supported enumeration.
fn continuity_text(kind: u8) -> Result<&'static str> {
    ["C0", "G1", "C1", "G2", "C2", "C3", "CN"]
        .get(usize::from(kind))
        .copied()
        .ok_or_else(|| "invalid XBF continuity".into())
}

/// Read and render a native binary XCAF/BREP document.
///
/// # Errors
///
/// File access, binary schema, geometry, document roots or tessellation fail validation.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    parse(&bytes)
}

/// Recognize the native binary topology header without reinterpreting arbitrary input as BREP.
pub(super) fn is_binary_brep(bytes: &[u8]) -> bool {
    bytes
        .trim_ascii_start()
        .starts_with(b"Open CASCADE Topology V")
}

/// Decode one standalone native binary BREP root using the shared XCAF geometry tables.
///
/// # Errors
///
/// Input size, topology version, tables, root reference, exact file extent or geometry is invalid or unsupported.
pub(super) fn load_binary_brep(bytes: &[u8]) -> Result<Scene> {
    if bytes.len() > MAX_BYTES {
        return Err("binary BREP input exceeds the 16 MiB preview limit".into());
    }
    if !is_binary_brep(bytes) {
        return Err("binary BREP lacks a native topology header".into());
    }
    let mut cursor = Cursor::new(bytes);
    let mut tables = Tables::default();
    tables.read_native_tables(&mut cursor)?;
    let root = tables
        .table_reference(&mut cursor, tables.shapes.len())?
        .ok_or("binary BREP root is null")?;
    if cursor.pos != bytes.len() {
        return Err("binary BREP contains trailing data after its root".into());
    }
    let faces = tables.containing_faces()?;
    let mut primitives = Vec::new();
    let mut occurrences = 0;
    collect_primitives(
        &tables,
        root,
        Transform::IDENTITY,
        None,
        &HashMap::new(),
        &faces,
        &mut primitives,
        &mut occurrences,
        0,
    )?;
    if primitives.is_empty() {
        return Err("binary BREP has no renderable geometry".into());
    }
    tables.render_primitives(&primitives)
}

/// Document label attributes needed for shape ownership, placement and color.
#[derive(Default)]
struct Label {
    /// Native integer label path identifying this document label.
    path: Vec<i32>,
    /// Native shape occurrences directly stored in this label.
    shapes: Vec<ShapeRef>,
    /// Native placement index; zero selects identity.
    location: Option<usize>,
    /// Whether this label declares the document shape-tool attribute.
    shape_tool: bool,
    /// Native attribute trees indexed by their supported type identifiers.
    trees: HashMap<u32, Tree>,
    /// Optional inherited RGB surface or line color.
    color: Option<[u8; 3]>,
    /// Whether this label references externally stored geometry.
    external: bool,
}
/// Native attribute-tree links used by shape and color associations.
struct Tree {
    /// Native attribute identifier used by tree links.
    id: i32,
    /// Native parent attribute identifier in the association tree.
    father: i32,
    /// Native first-child attribute identifier in the association tree.
    first: i32,
}
/// Supported binary document metadata and indexed label attributes.
struct Document {
    /// Supported native binary document format version.
    version: u32,
    /// Attribute type names in native table order.
    types: Vec<String>,
    /// Decoded document labels and their supported attributes.
    labels: Vec<Label>,
    /// Native attribute identifiers mapped to owning label indices.
    attribute_labels: HashMap<i32, usize>,
}

/// Resolve binary document attributes and native geometry into a complete scene.
///
/// # Errors
///
/// Document headers, labels, tables, references or CAD geometry are invalid, unsupported or oversized.
#[allow(
    clippy::too_many_lines,
    reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
)]
fn parse(bytes: &[u8]) -> Result<Scene> {
    if bytes.len() > MAX_BYTES {
        return Err("XBF input exceeds the 16 MiB preview limit".into());
    }
    let mut cursor = Cursor::new(bytes);
    if cursor.take(7)? != b"BINFILE" || cursor.i32()? != 0x0403_0201 {
        return Err("unsupported XBF binary signature or byte order".into());
    }
    let mut header = [0i32; 12];
    for word in &mut header {
        *word = cursor.i32()?;
    }
    cursor.count(MAX_RECORDS)?; // generic persistent object count
    let version = cursor.string(1)?.parse::<u32>()?;
    if !(6..=12).contains(&version) {
        return Err("XBF document versions outside 6..12 are unsupported".into());
    }
    cursor.string(1)?;
    cursor.string(1)?;
    cursor.string(1)?;
    cursor.string(2)?;
    cursor.string(1)?;
    cursor.string(2)?;
    let user_count = cursor.count(MAX_RECORDS)?;
    let mut types = Vec::new();
    let mut in_types = false;
    let mut xcaf = false;
    for _ in 0..user_count {
        let value = cursor.string(1)?;
        match value.as_str() {
            "START_TYPES" => in_types = true,
            "END_TYPES" => in_types = false,
            "FILE_FORMAT: BinXCAF" => xcaf = true,
            _ if in_types => types.push(value),
            _ => {}
        }
    }
    if !xcaf || in_types || types.is_empty() {
        return Err("XBF input is not a valid BinXCAF document".into());
    }
    let document_section =
        usize::try_from(header[11]).map_err(|_| "invalid XBF document offset")?;
    if document_section < cursor.pos || document_section > bytes.len() {
        return Err("invalid XBF header section bounds".into());
    }
    cursor.pos = document_section;
    let mut shape_section = None;
    {
        let name = cursor.string(1)?;
        let (offset, length, post) = if version <= 9 {
            (
                u64::try_from(cursor.i32()?)?,
                u64::try_from(cursor.i32()?)?,
                u64::try_from(cursor.i32()?)?,
            )
        } else {
            (cursor.u64()?, cursor.u64()?, cursor.u64()?)
        };
        if name == "SHAPE_SECTION_POS:" {
            if post != 0 {
                return Err("post-read XBF shape sections are unsupported".into());
            }
            let start = usize::try_from(offset)?;
            let length = usize::try_from(length)?;
            let end = start.checked_add(length).ok_or("XBF section overflow")?;
            if start < cursor.pos || end > bytes.len() {
                return Err("invalid XBF shape section bounds".into());
            }
            shape_section = Some((start, end));
        } else if name != ":" || version != 12 {
            return Err(format!("unsupported XBF document section {name}").into());
        }
    }
    if version < 12 && shape_section.is_none() {
        return Err("XBF document has no shape section".into());
    }
    let mut tables = Tables::default();
    if let Some((start, end)) = shape_section {
        let mut shape_cursor = Cursor {
            bytes: &bytes[..end],
            pos: start,
        };
        tables.read_old(&mut shape_cursor)?;
        if shape_cursor.pos != end {
            return Err("XBF shape section length disagrees with geometry".into());
        }
    }
    if cursor.i32()? != 0 {
        return Err("XBF document root label must have tag zero".into());
    }
    let mut document = Document {
        version,
        types,
        labels: Vec::new(),
        attribute_labels: HashMap::new(),
    };
    document.label(&mut cursor, &mut tables, vec![0], 0)?;
    let expected_end = shape_section.map_or(bytes.len(), |(start, _)| start);
    if cursor.pos != expected_end {
        return Err("XBF document hierarchy has trailing or missing records".into());
    }
    document.render(&mut tables)
}

impl Tables {
    /// Read native table-based geometry for supported legacy document versions.
    ///
    /// # Errors
    ///
    /// Table schemas, references, numeric data or limits are invalid or unsupported.
    fn read_old(&mut self, cursor: &mut Cursor<'_>) -> Result<()> {
        let start = cursor.pos;
        let prefix = cursor
            .bytes
            .get(start..)
            .ok_or("truncated XBF shape section")?;
        if !prefix.starts_with(b"SHAPE_SECTION") {
            return Err("XBF document lacks a native shape section".into());
        }
        cursor.take(13)?;
        self.read_native_tables(cursor)
    }

    /// Read shared native version, placement, geometry and backward-referenced topology tables.
    ///
    /// # Errors
    ///
    /// Native table schemas, numeric values, references or resource limits are invalid or unsupported.
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the bounded native table dispatch and its reference validation together."
    )]
    fn read_native_tables(&mut self, cursor: &mut Cursor<'_>) -> Result<()> {
        while cursor
            .bytes
            .get(cursor.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            cursor.pos += 1;
        }
        let start = cursor.pos;
        while cursor.bytes.get(cursor.pos).is_some_and(|b| *b != b'\n') {
            cursor.pos += 1;
        }
        let version = std::str::from_utf8(
            cursor
                .bytes
                .get(start..cursor.pos)
                .ok_or("truncated XBF shape version")?,
        )?;
        let version = match version.trim() {
            "Open CASCADE Topology V1 (c)" => 1,
            "Open CASCADE Topology V2 (c)" => 2,
            "Open CASCADE Topology V3 (c)" => 3,
            "Open CASCADE Topology V4, (c) Open Cascade" => 4,
            _ => return Err("unsupported XBF BinTools topology version".into()),
        };
        cursor.take(1)?;
        let count = cursor.table_header("Locations")?;
        for _ in 0..count {
            let transform = match cursor.byte()? {
                1 => read_transform(cursor)?,
                2 => {
                    let mut result = Transform::IDENTITY;
                    loop {
                        let id = cursor.count(self.locations.len())?;
                        if id == 0 {
                            break;
                        }
                        result = result.then(power(self.locations[id - 1], cursor.i32()?)?);
                    }
                    result
                }
                _ => return Err("unsupported XBF table location".into()),
            };
            self.locations.push(transform);
        }
        let count = cursor.table_header("Curve2ds")?;
        for _ in 0..count {
            self.pcurves.push(curve(cursor, 2, 0)?);
        }
        let count = cursor.table_header("Curves")?;
        for _ in 0..count {
            self.curves.push(curve(cursor, 3, 0)?);
        }
        if cursor.table_header("Polygon3D")? != 0
            || cursor.table_header("PolygonOnTriangulations")? != 0
        {
            return Err("XBF table polygon representations are unsupported".into());
        }
        let count = cursor.table_header("Surfaces")?;
        for _ in 0..count {
            self.surfaces.push(surface(cursor, 0)?);
        }
        if cursor.table_header("Triangulations")? != 0 {
            return Err("XBF stored triangulations are unsupported".into());
        }
        let total = cursor.table_header("TShapes")?;
        for _ in 0..total {
            let kind = cursor.byte()?;
            let geometry = self.old_shape_geometry(cursor, kind, version)?;
            let mut flags = 0;
            for bit in 0..7 {
                flags |= cursor.boolean()? << bit;
            }
            let mut children = Vec::new();
            while let Some(reference) = self.table_reference(cursor, total)? {
                if children.len() >= MAX_RECORDS {
                    return Err("XBF shape has too many children".into());
                }
                children.push(reference);
            }
            self.shapes.push(Shape {
                kind,
                geometry,
                flags,
                children,
            });
        }
        Ok(())
    }

    /// Decode one native occurrence or null marker using reverse table indices.
    ///
    /// # Errors
    ///
    /// Orientation, shape/location indices or backward-reference ordering is invalid or truncated.
    fn table_reference(&self, cursor: &mut Cursor<'_>, total: usize) -> Result<Option<ShapeRef>> {
        let orientation = cursor.byte()?;
        if orientation == b'*' {
            return Ok(None);
        }
        if orientation > 3 {
            return Err("invalid native BREP table orientation".into());
        }
        let reverse_id = cursor.count(total)?;
        if reverse_id == 0 {
            return Err("zero native BREP topology reference".into());
        }
        let node = total - reverse_id;
        if node >= self.shapes.len() {
            return Err("forward/cyclic native BREP topology reference".into());
        }
        let location = cursor.count(self.locations.len())?;
        Ok(Some(ShapeRef {
            node,
            location,
            orientation,
        }))
    }

    /// Validate backward topology ordering and mark records containing faces.
    ///
    /// # Errors
    ///
    /// A child shape refers forward or forms a cycle.
    fn containing_faces(&self) -> Result<Vec<bool>> {
        let mut faces = Vec::with_capacity(self.shapes.len());
        for (index, shape) in self.shapes.iter().enumerate() {
            let mut contains = shape.kind == 4;
            for child in &shape.children {
                if child.node >= index {
                    return Err("forward or cyclic native BREP shape topology".into());
                }
                contains |= faces[child.node];
            }
            faces.push(contains);
        }
        Ok(faces)
    }
    /// Translate legacy table-based shape representations to BREP syntax.
    ///
    /// # Errors
    ///
    /// Native representation data or geometry/location references are invalid or unsupported.
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
    )]
    fn old_shape_geometry(
        &self,
        cursor: &mut Cursor<'_>,
        kind: u8,
        version: u32,
    ) -> Result<String> {
        let mut out = String::new();
        match kind {
            7 => {
                real_text(cursor, &mut out, 4)?;
                loop {
                    // Historical V1/V2 vertices put the parameter before its
                    // kind byte. OCCT also accepts modern kind-first records
                    // for those versions using this same lookahead.
                    let parameter_first = version <= 2
                        && cursor
                            .bytes
                            .get(cursor.pos + 8)
                            .is_some_and(|kind| (1..=3).contains(kind));
                    let rep = if parameter_first {
                        cursor.real()?;
                        cursor.byte()?
                    } else {
                        let rep = cursor.byte()?;
                        if rep != 0 {
                            cursor.real()?;
                        }
                        rep
                    };
                    if rep == 0 {
                        break;
                    }
                    match rep {
                        1 => {
                            cursor.count(self.curves.len())?;
                        }
                        2 => {
                            cursor.count(self.pcurves.len())?;
                            cursor.count(self.surfaces.len())?;
                        }
                        3 => {
                            cursor.real()?;
                            cursor.count(self.surfaces.len())?;
                        }
                        _ => return Err("unsupported XBF table vertex representation".into()),
                    }
                    cursor.count(self.locations.len())?;
                }
                out.push_str("0 0\n");
            }
            6 => {
                real_text(cursor, &mut out, 1)?;
                writeln!(
                    out,
                    "{} {} {}",
                    cursor.boolean()?,
                    cursor.boolean()?,
                    cursor.boolean()?
                )?;
                loop {
                    let rep = cursor.byte()?;
                    if rep == 0 {
                        break;
                    }
                    write!(out, "{rep} ")?;
                    match rep {
                        1 => {
                            let curve = cursor.index(self.curves.len())?;
                            let location = cursor.count(self.locations.len())?;
                            self.check_representation_location(location)?;
                            write!(out, "{curve} {location} ")?;
                            real_text(cursor, &mut out, 2)?;
                        }
                        2 | 3 => {
                            write!(out, "{} ", cursor.index(self.pcurves.len())?)?;
                            if rep == 3 {
                                write!(
                                    out,
                                    "{} {} ",
                                    cursor.index(self.pcurves.len())?,
                                    continuity_text(cursor.byte()?)?
                                )?;
                            }
                            let surface = cursor.index(self.surfaces.len())?;
                            let location = cursor.count(self.locations.len())?;
                            self.check_representation_location(location)?;
                            write!(out, "{surface} {location} ")?;
                            real_text(cursor, &mut out, 2)?;
                            if matches!(version, 2 | 3) {
                                for _ in 0..4 {
                                    cursor.real()?;
                                }
                            }
                        }
                        4 => {
                            write!(out, "{} ", continuity_text(cursor.byte()?)?)?;
                            for _ in 0..2 {
                                write!(
                                    out,
                                    "{} {} ",
                                    cursor.count(self.surfaces.len())?,
                                    cursor.count(self.locations.len())?
                                )?;
                            }
                        }
                        _ => return Err("unsupported XBF table edge representation".into()),
                    }
                    out.push('\n');
                }
                out.push_str("0\n");
            }
            4 => {
                write!(out, "{} ", cursor.boolean()?)?;
                real_text(cursor, &mut out, 1)?;
                let surface = cursor.index(self.surfaces.len())?;
                let location = cursor.count(self.locations.len())?;
                self.check_representation_location(location)?;
                if surface == 0 {
                    return Err("XBF face has no analytic surface".into());
                }
                writeln!(out, "{surface} {location}")?;
                match cursor.byte()? {
                    0 | 1 => {}
                    _ => return Err("XBF face triangulation references are unsupported".into()),
                }
            }
            0..=3 | 5 => {}
            _ => return Err("unsupported XBF table topology type".into()),
        }
        Ok(out)
    }
    /// Decode a stored shape reference and its placement/orientation.
    ///
    /// # Errors
    ///
    /// The encoded shape, location or orientation is invalid or missing.
    fn stored_reference(&self, cursor: &mut Cursor<'_>) -> Result<ShapeRef> {
        cursor.align()?;
        let index = cursor.count(self.shapes.len())?;
        if index == 0 {
            return Err("XBF named shape has a null shape reference".into());
        }
        let location = cursor.count(self.locations.len())?;
        let orientation = match cursor.byte()? {
            b'F' => 0,
            b'R' => 1,
            b'I' => 2,
            b'E' => 3,
            _ => return Err("invalid XBF named shape orientation".into()),
        };
        Ok(ShapeRef {
            node: index - 1,
            location,
            orientation,
        })
    }
    /// Resolve a native location table index, including identity index zero.
    ///
    /// # Errors
    ///
    /// The location index is out of range.
    fn location_transform(&self, index: usize) -> Result<Transform> {
        if index == 0 {
            Ok(Transform::IDENTITY)
        } else {
            self.locations
                .get(index - 1)
                .copied()
                .ok_or_else(|| "invalid XBF location index".into())
        }
    }
    /// Check a representation's native location reference.
    ///
    /// # Errors
    ///
    /// The location does not resolve to a known placement.
    fn check_representation_location(&self, index: usize) -> Result<()> {
        self.location_transform(index)?;
        Ok(())
    }
}

impl Document {
    /// Read one document label and its supported typed attributes recursively.
    ///
    /// # Errors
    ///
    /// Attributes, labels, references or nesting limits are malformed or unsupported.
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
    )]
    fn label(
        &mut self,
        cursor: &mut Cursor<'_>,
        tables: &mut Tables,
        path: Vec<i32>,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH || self.labels.len() >= MAX_RECORDS {
            return Err("XBF document hierarchy exceeds preview limits".into());
        }
        let label_end = if self.version == 12 {
            let position = cursor.pos;
            let size = usize::try_from(cursor.u64()?)?;
            let end = position
                .checked_add(size)
                .ok_or("XBF label size overflow")?;
            if size < 16 || end > cursor.bytes.len() {
                return Err("invalid XBF label size".into());
            }
            Some(end)
        } else {
            None
        };
        let index = self.labels.len();
        self.labels.push(Label {
            path,
            ..Default::default()
        });
        loop {
            let kind = cursor.i32()?;
            if kind == -1 {
                break;
            }
            let kind = usize::try_from(kind).map_err(|_| "invalid XBF attribute type")?;
            let name = self
                .types
                .get(kind.checked_sub(1).ok_or("zero XBF attribute type")?)
                .ok_or("unknown XBF attribute type")?
                .clone();
            let id = cursor.i32()?;
            if id == 0 || id == i32::MIN || self.attribute_labels.insert(id.abs(), index).is_some()
            {
                return Err("invalid or duplicate XBF attribute ID".into());
            }
            let size = cursor.count(MAX_BYTES)?;
            let data = cursor.take(size)?;
            let mut value = Cursor::new(data);
            let mut direct_end = None;
            if id < 0 {
                if self.version != 12 {
                    return Err("inline XBF attributes require document version 12".into());
                }
                let start = cursor.pos;
                let size = usize::try_from(cursor.u64()?)?;
                let end = start
                    .checked_add(size)
                    .ok_or("XBF inline attribute overflow")?;
                if size < 8 || end > cursor.bytes.len() {
                    return Err("invalid XBF inline attribute length".into());
                }
                direct_end = Some(end);
            }
            match name.as_str() {
                "XCAFDoc_ShapeTool" => self.labels[index].shape_tool = true,
                "TNaming_NamedShape" => {
                    let count = value.count(MAX_RECORDS)?;
                    value.i32()?;
                    let evolution = value.byte()?;
                    if !b"PGMDSR".contains(&evolution) {
                        return Err("unsupported XBF shape evolution".into());
                    }
                    for _ in 0..count {
                        if evolution != b'P' {
                            if direct_end.is_some() {
                                tables.shape(cursor, 0)?;
                            } else {
                                tables.stored_reference(&mut value)?;
                            }
                        }
                        if evolution != b'D' {
                            let shape = if direct_end.is_some() {
                                tables.shape(cursor, 0)?.ok_or("null XBF named shape")?
                            } else {
                                tables.stored_reference(&mut value)?
                            };
                            self.labels[index].shapes.push(shape);
                        }
                    }
                    if value.pos != data.len() {
                        return Err("XBF named-shape payload has trailing data".into());
                    }
                }
                "XCAFDoc_Location" => {
                    let location = if direct_end.is_some() {
                        tables.location(cursor, 0)?
                    } else {
                        let first = value.count(tables.locations.len())?;
                        // The v6+ attribute repeats the remaining location
                        // chain for legacy compatibility. The first table
                        // entry already stores the exact complete transform.
                        while value.pos < data.len() {
                            value.count(tables.locations.len())?;
                        }
                        first
                    };
                    self.labels[index].location = Some(location);
                }
                "TDataStd_TreeNode" => {
                    let father = value.i32()?;
                    value.i32()?;
                    value.i32()?;
                    let first = value.i32()?;
                    let guid = value.take(16)?;
                    let prefix = u32::from_le_bytes(guid[..4].try_into()?);
                    if value.pos != data.len() {
                        return Err("invalid XBF tree attribute length".into());
                    }
                    self.labels[index].trees.insert(
                        prefix,
                        Tree {
                            id: id.abs(),
                            father,
                            first,
                        },
                    );
                }
                "XCAFDoc_Color" => {
                    let mut color = [0u8; 3];
                    for component in &mut color {
                        let value = value.real()?;
                        if !(0.0..=1.0).contains(&value) {
                            return Err("XBF color is outside 0..1".into());
                        }
                        *component = linear_color(value);
                    }
                    if value.pos < data.len() {
                        let alpha = f32::from_le_bytes(value.take(4)?.try_into()?);
                        if !alpha.is_finite() || !(0.0..=1.0).contains(&alpha) {
                            return Err("invalid XBF color alpha".into());
                        }
                    }
                    if value.pos != data.len() {
                        return Err("invalid XBF color payload length".into());
                    }
                    self.labels[index].color = Some(color);
                }
                "TDataStd_UAttribute"
                    if data.len() == 16
                        && u32::from_le_bytes(data[..4].try_into()?) == 0x6b89_6b01 =>
                {
                    self.labels[index].external = true;
                }
                _ if direct_end.is_some() => {
                    return Err(format!("unsupported direct XBF attribute {name}").into());
                }
                _ => {}
            }
            if let Some(end) = direct_end
                && cursor.pos != end
            {
                return Err("XBF inline geometry size mismatch".into());
            }
        }
        let mut tags = HashSet::new();
        loop {
            let tag = cursor.i32()?;
            if tag == -2 {
                break;
            }
            if tag <= 0 || !tags.insert(tag) {
                return Err("invalid or duplicate XBF child label tag".into());
            }
            let mut path = self.labels[index].path.clone();
            path.push(tag);
            self.label(cursor, tables, path, depth + 1)?;
        }
        if label_end.is_some_and(|end| cursor.pos != end) {
            return Err("XBF label size disagrees with its hierarchy".into());
        }
        Ok(())
    }

    /// Resolve a shape label or assembly reference to concrete native shapes.
    ///
    /// # Errors
    ///
    /// Label/tree references are missing, cyclic or exceed nesting limits.
    fn resolve(&self, index: usize, tables: &mut Tables, depth: usize) -> Result<Vec<ShapeRef>> {
        if depth > MAX_DEPTH {
            return Err("cyclic or overly deep XBF assembly reference".into());
        }
        let label = &self.labels[index];
        if label.external {
            return Err("XBF external shape references are unsupported".into());
        }
        if let Some(tree) = label.trees.get(&0x5b89_6afe).filter(|tree| tree.father > 0) {
            let parent = *self
                .attribute_labels
                .get(&tree.father)
                .ok_or("XBF assembly reference target does not exist")?;
            let target_tree = self.labels[parent]
                .trees
                .get(&0x5b89_6afe)
                .ok_or("XBF assembly reference does not target a shape tree")?;
            if target_tree.id != tree.father {
                return Err("XBF assembly link has the wrong attribute type".into());
            }
            let location = label
                .location
                .ok_or("XBF assembly reference has no placement")?;
            let transform = tables.location_transform(location)?;
            let mut shapes = self.resolve(parent, tables, depth + 1)?;
            for shape in &mut shapes {
                let transform = transform.then(tables.location_transform(shape.location)?);
                tables.locations.push(transform);
                shape.location = tables.locations.len();
            }
            Ok(shapes)
        } else {
            Ok(label.shapes.clone())
        }
    }

    /// Resolve a label's inherited diffuse color.
    ///
    /// # Errors
    ///
    /// The label/color-tree references are invalid or cyclic.
    fn diffuse(&self, index: usize) -> Result<Option<[u8; 3]>> {
        let label = &self.labels[index];
        for kind in [0xefd2_12e5, 0xefd2_12e4, 0xefd2_12e6] {
            if let Some(tree) = label.trees.get(&kind).filter(|tree| tree.father > 0) {
                let parent = *self
                    .attribute_labels
                    .get(&tree.father)
                    .ok_or("XBF color reference target does not exist")?;
                let target_tree = self.labels[parent]
                    .trees
                    .get(&kind)
                    .ok_or("XBF color reference has the wrong tree type")?;
                if target_tree.id != tree.father {
                    return Err("invalid XBF color reference attribute".into());
                }
                return self.labels[parent]
                    .color
                    .ok_or_else(|| "XBF color target has no RGB value".into())
                    .map(Some);
            }
        }
        Ok(None)
    }

    /// Render document roots with inherited color and native occurrence placements.
    ///
    /// # Errors
    ///
    /// Required shapes, roots, placements or CAD tessellation cannot produce a complete bounded scene.
    fn render(&self, tables: &mut Tables) -> Result<Scene> {
        let tools: Vec<_> = self
            .labels
            .iter()
            .filter(|label| label.shape_tool)
            .collect();
        if tools.len() != 1 {
            return Err("XBF document must have exactly one shape tool".into());
        }
        let base = &tools[0].path;
        let mut colors = HashMap::new();
        for (index, label) in self.labels.iter().enumerate() {
            if let Some(color) = self.diffuse(index)? {
                for shape in &label.shapes {
                    colors.insert(shape.node, color);
                }
            }
        }
        let faces = tables.containing_faces()?;
        let mut primitives = Vec::new();
        let mut occurrences = 0;
        for (index, label) in self.labels.iter().enumerate() {
            if label.path.len() != base.len() + 1 || !label.path.starts_with(base) {
                continue;
            }
            if label
                .trees
                .get(&0x5b89_6afe)
                .is_some_and(|tree| tree.first > 0)
            {
                continue;
            }
            let color = self.diffuse(index)?;
            for shape in self.resolve(index, tables, 0)? {
                collect_primitives(
                    tables,
                    shape,
                    Transform::IDENTITY,
                    color,
                    &colors,
                    &faces,
                    &mut primitives,
                    &mut occurrences,
                    0,
                )?;
            }
        }
        if primitives.is_empty() {
            return Err("XBF document has no free renderable shapes".into());
        }
        tables.render_primitives(&primitives)
    }
}

impl Tables {
    /// Restore native carrier geometry and tessellate only selected shape occurrences.
    ///
    /// # Errors
    ///
    /// Conversion, carrier placements, topology, tessellation or the resulting scene is invalid or excessive.
    fn render_primitives(&mut self, primitives: &[Primitive]) -> Result<Scene> {
        let text = self.brep(primitives)?;
        let (mut model, root) = super::cad::read_brep_with_authored_frames(
            &text,
            ogeom_core::Tolerances::millimetres(),
        )?;
        let children = model.children_of(&root)?;
        if children.len() != primitives.len() + self.shapes.len() {
            return Err("XBF topology conversion omitted a primitive".into());
        }
        self.restore_representation_locations(&mut model, &children[..self.shapes.len()])?;
        super::cad::restore_brep_wire_order(&mut model)?;
        let mut scene = Scene::default();
        for (shape, (_, _, color)) in children
            .into_iter()
            .skip(self.shapes.len())
            .zip(primitives.iter().copied())
        {
            let first_triangle = scene.triangles.len();
            let first_point = scene.points.len();
            let first_line = scene.lines.len();
            super::cad::append_shape(&mut scene, &model, &shape)?;
            for triangle in &mut scene.triangles[first_triangle..] {
                triangle.color = color;
            }
            for point in &mut scene.points[first_point..] {
                point.color = color;
            }
            for line in &mut scene.lines[first_line..] {
                line.color = color;
            }
        }
        scene.validate()?;
        Ok(scene)
    }
}

/// Encode one clamped linear RGB component as an sRGB byte.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Validated zero-to-one color factors are intentionally rounded to bounded RGB bytes."
)]
fn linear_color(value: f64) -> u8 {
    let value = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

/// A renderable native shape occurrence with world placement and inherited RGB color.
type Primitive = (ShapeRef, Transform, Option<[u8; 3]>);

/// Collect face, free-edge and vertex occurrences with their world placement and color.
///
/// # Errors
///
/// Shape/location references are invalid or occurrence, nesting or primitive limits are exceeded.
#[allow(
    clippy::too_many_arguments,
    reason = "The bounded shape walk carries distinct placement, color, ownership, and resource state through recursion."
)]
fn collect_primitives(
    tables: &Tables,
    reference: ShapeRef,
    parent: Transform,
    color: Option<[u8; 3]>,
    colors: &HashMap<usize, [u8; 3]>,
    faces: &[bool],
    output: &mut Vec<Primitive>,
    occurrences: &mut usize,
    depth: usize,
) -> Result<()> {
    *occurrences += 1;
    if *occurrences > MAX_RECORDS || depth > MAX_DEPTH {
        return Err("XBF instances exceed 100000 occurrences or 64 levels".into());
    }
    let shape = tables
        .shapes
        .get(reference.node)
        .ok_or("XBF shape index is out of range")?;
    let world = parent.then(tables.location_transform(reference.location)?);
    let color = colors.get(&reference.node).copied().or(color);
    if shape.kind == 4 || (shape.kind == 6 && !faces[reference.node]) || shape.kind == 7 {
        if output.len() >= 4096 {
            return Err("XBF exceeds 4096 preview primitives".into());
        }
        output.push((
            ShapeRef {
                location: 0,
                ..reference
            },
            world,
            color,
        ));
        return Ok(());
    }
    // Stopping at a face already retains its boundaries exactly once. Other
    // compound children may be independent free wires or isolated vertices.
    for child in &shape.children {
        collect_primitives(
            tables,
            ShapeRef {
                orientation: compose_orientation(reference.orientation, child.orientation)?,
                ..*child
            },
            world,
            color,
            colors,
            faces,
            output,
            occurrences,
            depth + 1,
        )?;
    }
    Ok(())
}

/// Compose native occurrence orientations as OCCT's `TopoDS_Iterator` does.
/// Its `TopAbs::Compose(parent, child)` implementation indexes the child row
/// and parent column, including the asymmetric internal/external cases.
///
/// # Errors
///
/// Either orientation lies outside the four native enumeration values.
fn compose_orientation(parent: u8, child: u8) -> Result<u8> {
    [[0, 1, 2, 3], [1, 0, 2, 3], [2, 2, 2, 2], [3, 3, 3, 3]]
        .get(usize::from(child))
        .and_then(|row| row.get(usize::from(parent)))
        .copied()
        .ok_or_else(|| "invalid native BREP occurrence orientation".into())
}

impl Tables {
    /// The ASCII kernel reader preserves topology placements but resets
    /// edge/surface representation placements. Restore those exact native
    /// records through the public typed geometry API before tessellation.
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
    )]
    fn restore_representation_locations(
        &self,
        model: &mut ogeom_topo::Model,
        nodes: &[ogeom_topo::Shape],
    ) -> Result<()> {
        use ogeom_geom::Transformable;
        let tolerance = ogeom_core::Tolerances::millimetres();
        let mut transforms = vec![ogeom_math::Transform::IDENTITY];
        for transform in &self.locations {
            let rows =
                std::array::from_fn(|row| std::array::from_fn(|column| transform.0[column][row]));
            let scale = ogeom_math::Matrix3::new(rows).determinant().cbrt();
            if !scale.is_finite() || scale == 0.0 {
                return Err("singular XBF geometry location".into());
            }
            let normalized =
                std::array::from_fn(|row| std::array::from_fn(|column| rows[row][column] / scale));
            transforms.push(ogeom_math::Transform::from_parts(
                ogeom_math::Matrix3::new(normalized),
                scale,
                ogeom_math::Vector::new(transform.0[3][0], transform.0[3][1], transform.0[3][2]),
                1e-9,
            )?);
        }
        // The tessellator evaluates carrier geometry in its stored coordinate
        // frame. Materialize each actual rigid carrier placement once and
        // share that transformed geometry between its face and pcurves.
        let mut surfaces = HashMap::new();
        let mut curves = HashMap::new();
        let placed_surface = |model: &mut ogeom_topo::Model,
                              surfaces: &mut HashMap<
            (ogeom_topo::SurfaceId, usize),
            ogeom_topo::SurfaceId,
        >,
                              id: ogeom_topo::SurfaceId,
                              index: usize|
         -> Result<ogeom_topo::SurfaceId> {
            let transform = transforms
                .get(index)
                .ok_or("invalid XBF surface location")?;
            if (transform.scale_factor().abs() - 1.0).abs() > 1e-9 {
                return Err("scaled XBF surface representation locations are unsupported".into());
            }
            if transform.kind() == ogeom_math::TransformKind::Identity {
                return Ok(id);
            }
            if let Some(placed) = surfaces.get(&(id, index)) {
                return Ok(*placed);
            }
            let surface = model
                .geometry()
                .surface(id)
                .ok_or("XBF carrier surface is missing")?
                .transformed(transform, tolerance)?;
            let placed = model.geometry_mut().add_surface(surface);
            surfaces.insert((id, index), placed);
            Ok(placed)
        };
        for (shape, node) in self.shapes.iter().zip(nodes) {
            let mut tokens = shape.geometry.split_whitespace();
            let mut data = model
                .node(node)
                .ok_or("converted XBF node is missing")?
                .data()
                .clone();
            match &mut data {
                ogeom_topo::NodeData::Face(face) => {
                    let index = tokens
                        .nth(3)
                        .ok_or("XBF face has no carrier placement")?
                        .parse::<usize>()?;
                    face.surface = placed_surface(model, &mut surfaces, face.surface, index)?;
                    face.location = ogeom_topo::Location::identity();
                }
                ogeom_topo::NodeData::Edge(edge) => {
                    for _ in 0..4 {
                        tokens.next().ok_or("truncated XBF edge geometry")?;
                    }
                    let mut index = 0;
                    loop {
                        let kind = tokens
                            .next()
                            .ok_or("truncated XBF edge representations")?
                            .parse::<u8>()?;
                        if kind == 0 {
                            break;
                        }
                        let (before, after) = match kind {
                            1 => (1, 2),
                            2 => (2, 2),
                            3 => (4, 2),
                            4 => {
                                for _ in 0..5 {
                                    tokens.next().ok_or("truncated XBF continuity")?;
                                }
                                continue;
                            }
                            _ => return Err("unsupported converted XBF edge representation".into()),
                        };
                        for _ in 0..before {
                            tokens.next().ok_or("truncated XBF edge placement")?;
                        }
                        let native_location = tokens
                            .next()
                            .ok_or("missing XBF edge placement")?
                            .parse::<usize>()?;
                        let transform = transforms
                            .get(native_location)
                            .ok_or("invalid XBF edge placement")?;
                        if (transform.scale_factor().abs() - 1.0).abs() > 1e-9 {
                            return Err(
                                "scaled XBF curve representation locations are unsupported".into(),
                            );
                        }
                        let representation = edge
                            .representations
                            .get_mut(index)
                            .ok_or("XBF conversion lost an edge representation")?;
                        match representation {
                            ogeom_topo::EdgeRepr::Curve3d {
                                curve, location, ..
                            } => {
                                if transform.kind() != ogeom_math::TransformKind::Identity {
                                    let key = (*curve, native_location);
                                    let placed = if let Some(id) = curves.get(&key) {
                                        *id
                                    } else {
                                        let geometry = model
                                            .geometry()
                                            .curve(*curve)
                                            .ok_or("XBF edge curve is missing")?
                                            .transformed(transform, tolerance)?;
                                        let id = model.geometry_mut().add_curve(geometry);
                                        curves.insert(key, id);
                                        id
                                    };
                                    *curve = placed;
                                }
                                *location = ogeom_topo::Location::identity();
                            }
                            ogeom_topo::EdgeRepr::PCurve {
                                surface, location, ..
                            }
                            | ogeom_topo::EdgeRepr::Seam {
                                surface, location, ..
                            } => {
                                *surface = placed_surface(
                                    model,
                                    &mut surfaces,
                                    *surface,
                                    native_location,
                                )?;
                                *location = ogeom_topo::Location::identity();
                            }
                            _ => return Err("XBF conversion changed an edge representation".into()),
                        }
                        index += 1;
                        for _ in 0..after {
                            tokens.next().ok_or("truncated XBF edge range")?;
                        }
                    }
                    if index != edge.representations.len() {
                        return Err("XBF conversion lost edge representations".into());
                    }
                    edge.assert_same_parameter(false);
                }
                _ => {}
            }
            *model
                .node_mut(node)
                .ok_or("converted XBF node is missing")?
                .data_mut() = data;
        }
        Ok(())
    }
    /// Serialize collected native topology and placements as ASCII BREP.
    ///
    /// # Errors
    ///
    /// Required topology references, geometry or formatting are invalid.
    fn brep(&mut self, primitives: &[Primitive]) -> Result<String> {
        let mut locations = self.locations.clone();
        let mut root_children = Vec::new();
        for (shape, world, _) in primitives {
            locations.push(*world);
            root_children.push(ShapeRef {
                location: locations.len(),
                ..*shape
            });
        }
        let mut out =
            String::from("DBRep_DrawableShape\n\nCASCADE Topology V1, (c) Matra-Datavision\n");
        writeln!(out, "Locations {}", locations.len())?;
        for location in locations {
            out.push_str("1\n");
            for row in 0..3 {
                for column in &location.0 {
                    write!(out, "{} ", column[row])?;
                }
                out.push('\n');
            }
        }
        writeln!(out, "Curve2ds {}", self.pcurves.len())?;
        for curve in &self.pcurves {
            writeln!(out, "{curve}")?;
        }
        writeln!(out, "Curves {}", self.curves.len())?;
        for curve in &self.curves {
            writeln!(out, "{curve}")?;
        }
        out.push_str("Polygon3D 0\nPolygonOnTriangulations 0\n");
        writeln!(out, "Surfaces {}", self.surfaces.len())?;
        for surface in &self.surfaces {
            writeln!(out, "{surface}")?;
        }
        out.push_str("Triangulations 0\n");
        let total = self.shapes.len() + 1;
        writeln!(out, "TShapes {total}")?;
        for shape in &self.shapes {
            writeln!(out, "{}\n{}", shape_name(shape.kind)?, shape.geometry)?;
            for bit in 0..7 {
                write!(out, "{}", (shape.flags >> bit) & 1)?;
            }
            out.push('\n');
            for child in &shape.children {
                write_reference(&mut out, *child, total)?;
            }
            out.push_str("*\n");
            if out.len() > MAX_BYTES {
                return Err("converted XBF topology exceeds 16 MiB".into());
            }
        }
        out.push_str("Co\n0001000\n");
        // Native-node references make the BREP conversion's identities
        // explicit, allowing representation placements to be restored
        // without relying on kernel arena allocation or iteration order.
        for node in 0..self.shapes.len() {
            write_reference(
                &mut out,
                ShapeRef {
                    node,
                    location: 0,
                    orientation: 0,
                },
                total,
            )?;
        }
        for child in root_children {
            write_reference(&mut out, child, total)?;
        }
        out.push_str("*\n+1 0\n");
        if out.len() > MAX_BYTES {
            return Err("converted XBF topology exceeds 16 MiB".into());
        }
        Ok(out)
    }
}
/// Resolve a native topology kind to its ASCII BREP name.
///
/// # Errors
///
/// The topology kind is outside the supported enumeration.
fn shape_name(kind: u8) -> Result<&'static str> {
    ["Co", "CS", "So", "Sh", "Fa", "Wi", "Ed", "Ve"]
        .get(usize::from(kind))
        .copied()
        .ok_or_else(|| "unsupported XBF shape type".into())
}
/// Write a native shape occurrence in ASCII BREP reference syntax.
///
/// # Errors
///
/// The referenced shape index is invalid or formatting fails.
fn write_reference(out: &mut String, shape: ShapeRef, total: usize) -> Result<()> {
    let orientation = ["+", "-", "i", "e"]
        .get(usize::from(shape.orientation))
        .ok_or("invalid XBF shape orientation")?;
    let number = total
        .checked_sub(shape.node)
        .ok_or("invalid XBF shape index")?;
    write!(out, "{orientation}{number} {} ", shape.location)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Emit a primary native table header for one of its four supported versions.
    fn native_header(version: u32) -> Vec<u8> {
        let line = match version {
            1 => "Open CASCADE Topology V1 (c)",
            2 => "Open CASCADE Topology V2 (c)",
            3 => "Open CASCADE Topology V3 (c)",
            4 => "Open CASCADE Topology V4, (c) Open Cascade",
            _ => unreachable!(),
        };
        format!("\n{line}\n").into_bytes()
    }

    /// Serialize direct native placement records in mathematical row order.
    fn native_locations(out: &mut Vec<u8>, placements: &[Transform]) {
        out.extend(format!("Locations {}\n", placements.len()).as_bytes());
        for placement in placements {
            out.push(1);
            for row in 0..3 {
                for column in &placement.0 {
                    real(out, column[row]);
                }
            }
        }
    }

    /// Write shape flags, independently authored reverse references and the null terminator.
    fn native_children(out: &mut Vec<u8>, children: &[(u8, i32, i32)]) {
        out.extend([0, 0, 0, 1, 0, 0, 0]);
        for &(orientation, reverse_id, location) in children {
            out.push(orientation);
            integer(out, reverse_id);
            integer(out, location);
        }
        out.push(b'*');
    }

    /// Build two native points under a compound with independent child/root placements.
    fn native_points(
        version: u32,
        placements: &[Transform],
        child_location: i32,
        root_location: i32,
    ) -> Vec<u8> {
        let mut out = native_header(version);
        native_locations(&mut out, placements);
        out.extend(b"Curve2ds 0\nCurves 0\nPolygon3D 0\nPolygonOnTriangulations 0\nSurfaces 0\nTriangulations 0\nTShapes 3\n");
        for point in [[1., 2., 3.], [4., 5., 6.]] {
            out.push(7);
            real(&mut out, 1e-7);
            reals(&mut out, &point);
            out.push(0);
            native_children(&mut out, &[]);
        }
        out.push(0);
        native_children(&mut out, &[(0, 3, child_location), (0, 2, child_location)]);
        out.push(0);
        integer(&mut out, 1);
        integer(&mut out, root_location);
        out
    }

    /// Build a wound triangle, an independent free edge and one isolated point in native V4.
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the independently authored native geometry and reverse topology indices together."
    )]
    fn native_mixed(
        carrier: Transform,
        root: Transform,
        root_orientation: u8,
        face_orientation: u8,
    ) -> Vec<u8> {
        let points = [
            Vec3::new(0., 0., 0.),
            Vec3::new(2., 0., 0.),
            Vec3::new(0., 1., 0.),
            Vec3::new(4., 0., 0.),
            Vec3::new(5., 0., 0.),
            Vec3::new(8., 4., 3.),
        ];
        let endpoints = [(0_usize, 1_usize), (1, 2), (2, 0), (3, 4)];
        let mut out = native_header(4);
        native_locations(&mut out, &[carrier, root]);
        out.extend(b"Curve2ds 3\n");
        for &(start, end) in &endpoints[..3] {
            let vector = (points[end] - points[start]).normalized();
            out.push(1);
            reals(
                &mut out,
                &[points[start].x, points[start].y, vector.x, vector.y],
            );
        }
        out.extend(b"Curves 4\n");
        for &(start, end) in &endpoints {
            let vector = (points[end] - points[start]).normalized();
            out.push(1);
            reals(
                &mut out,
                &[
                    points[start].x,
                    points[start].y,
                    points[start].z,
                    vector.x,
                    vector.y,
                    vector.z,
                ],
            );
        }
        out.extend(b"Polygon3D 0\nPolygonOnTriangulations 0\nSurfaces 1\n");
        out.push(1);
        reals(&mut out, &[0., 0., 0., 0., 0., 1., 1., 0., 0., 0., 1., 0.]);
        out.extend(b"Triangulations 0\nTShapes 13\n");
        for point in points {
            let point = carrier.apply(point);
            out.push(7);
            reals(&mut out, &[1e-7, point.x, point.y, point.z]);
            out.push(0);
            native_children(&mut out, &[]);
        }
        for (index, &(start, end)) in endpoints.iter().enumerate() {
            let length = (points[end] - points[start]).length();
            let id = i32::try_from(index + 1).unwrap();
            out.push(6);
            real(&mut out, 1e-7);
            out.extend([1, 1, 0, 1]);
            integer(&mut out, id);
            integer(&mut out, 1);
            reals(&mut out, &[0., length]);
            if index < 3 {
                out.push(2);
                integer(&mut out, id);
                integer(&mut out, 1);
                integer(&mut out, 1);
                reals(&mut out, &[0., length]);
            }
            out.push(0);
            native_children(
                &mut out,
                &[
                    (0, 13 - i32::try_from(start).unwrap(), 0),
                    (1, 13 - i32::try_from(end).unwrap(), 0),
                ],
            );
        }
        out.push(5);
        native_children(&mut out, &[(0, 7, 0), (0, 6, 0), (0, 5, 0)]);
        out.extend([4, 0]);
        real(&mut out, 1e-7);
        integer(&mut out, 1);
        integer(&mut out, 1);
        out.push(0);
        native_children(&mut out, &[(0, 3, 0)]);
        out.push(0);
        native_children(&mut out, &[(face_orientation, 2, 0), (0, 4, 0), (0, 8, 0)]);
        out.push(root_orientation);
        integer(&mut out, 1);
        integer(&mut out, 2);
        out
    }

    /// Author an indirect plane with asymmetric UV trim and independently placed vertices.
    fn native_indirect_plane(placed: bool) -> Vec<u8> {
        let carrier = if placed {
            Transform([
                [0., 1., 0., 0.],
                [-1., 0., 0., 0.],
                [0., 0., 1., 0.],
                [10., 3., -1., 1.],
            ])
        } else {
            Transform::IDENTITY
        };
        let points = if placed {
            [[12., 10., 4.], [12., 13., 4.], [8., 10., 4.]]
        } else {
            [[7., -2., 5.], [10., -2., 5.], [7., 2., 5.]]
        };
        let mut out = native_header(4);
        native_locations(&mut out, &[carrier]);
        out.extend(b"Curve2ds 3\n");
        for line in [[0., 0., 1., 0.], [3., 0., -0.6, 0.8], [0., 4., 0., -1.]] {
            out.push(1);
            reals(&mut out, &line);
        }
        out.extend(b"Curves 3\n");
        for line in [
            [7., -2., 5., 1., 0., 0.],
            [10., -2., 5., -0.6, 0.8, 0.],
            [7., 2., 5., 0., -1., 0.],
        ] {
            out.push(1);
            reals(&mut out, &line);
        }
        out.extend(b"Polygon3D 0\nPolygonOnTriangulations 0\nSurfaces 1\n");
        out.push(1);
        // Valid gp_Ax3: X=+X, Y=+Y, Z=-Z is an indirect surface chart.
        reals(
            &mut out,
            &[7., -2., 5., 0., 0., -1., 1., 0., 0., 0., 1., 0.],
        );
        out.extend(b"Triangulations 0\nTShapes 8\n");
        for point in points {
            out.push(7);
            real(&mut out, 1e-7);
            reals(&mut out, &point);
            out.push(0);
            native_children(&mut out, &[]);
        }
        for (index, (length, endpoints)) in [(3., [8, 7]), (5., [7, 6]), (4., [6, 8])]
            .into_iter()
            .enumerate()
        {
            let curve = i32::try_from(index + 1).unwrap();
            out.push(6);
            real(&mut out, 1e-7);
            out.extend([1, 1, 0, 1]);
            integer(&mut out, curve);
            integer(&mut out, 1);
            reals(&mut out, &[0., length]);
            out.push(2);
            integer(&mut out, curve);
            integer(&mut out, 1);
            integer(&mut out, 1);
            reals(&mut out, &[0., length]);
            out.push(0);
            native_children(&mut out, &[(0, endpoints[0], 0), (1, endpoints[1], 0)]);
        }
        out.push(5);
        native_children(&mut out, &[(0, 5, 0), (0, 4, 0), (0, 3, 0)]);
        out.extend([4, 0]);
        real(&mut out, 1e-7);
        integer(&mut out, 1);
        integer(&mut out, 1);
        out.push(0);
        native_children(&mut out, &[(0, 2, 0)]);
        out.push(0);
        integer(&mut out, 1);
        integer(&mut out, 0);
        out
    }

    /// Independently author the same triangular trim in native ASCII table syntax.
    fn ascii_indirect_plane(placed: bool) -> String {
        let matrix = if placed {
            "0 -1 0 10\n1 0 0 3\n0 0 1 -1"
        } else {
            "1 0 0 0\n0 1 0 0\n0 0 1 0"
        };
        let points = if placed {
            ["12 10 4", "12 13 4", "8 10 4"]
        } else {
            ["7 -2 5", "10 -2 5", "7 2 5"]
        };
        format!(
            "DBRep_DrawableShape\n\nCASCADE Topology V1, (c) Matra-Datavision\n\
             Locations 1\n1\n{matrix}\n\
             Curve2ds 3\n1 0 0 1 0\n1 3 0 -0.6 0.8\n1 0 4 0 -1\n\
             Curves 3\n1 7 -2 5 1 0 0\n1 10 -2 5 -0.6 0.8 0\n1 7 2 5 0 -1 0\n\
             Polygon3D 0\nPolygonOnTriangulations 0\n\
             Surfaces 1\n1 7 -2 5 0 0 -1 1 0 0 0 1 0\n\
             Triangulations 0\nTShapes 8\n\
             Ve\n1e-7 {}\n0 0\n0001000\n*\n\
             Ve\n1e-7 {}\n0 0\n0001000\n*\n\
             Ve\n1e-7 {}\n0 0\n0001000\n*\n\
             Ed\n1e-7 1 1 0\n1 1 1 0 3\n2 1 1 1 0 3\n0\n0001000\n+8 0 -7 0 *\n\
             Ed\n1e-7 1 1 0\n1 2 1 0 5\n2 2 1 1 0 5\n0\n0001000\n+7 0 -6 0 *\n\
             Ed\n1e-7 1 1 0\n1 3 1 0 4\n2 3 1 1 0 4\n0\n0001000\n+6 0 -8 0 *\n\
             Wi\n0001000\n+5 0 +4 0 +3 0 *\n\
             Fa\n0 1e-7 1 1\n0001000\n+2 0 *\n+1 0\n",
            points[0], points[1], points[2]
        )
    }

    #[test]
    fn binary_indirect_plane_keeps_literal_chart_trim_and_ascii_render() {
        use ogeom_geom::Surface;
        let tolerances = ogeom_core::Tolerances::millimetres();
        for placed in [false, true] {
            let bytes = native_indirect_plane(placed);
            let ascii = ascii_indirect_plane(placed);
            let mut cursor = Cursor::new(&bytes);
            let mut tables = Tables::default();
            tables.read_native_tables(&mut cursor).unwrap();
            let root = tables
                .table_reference(&mut cursor, tables.shapes.len())
                .unwrap()
                .unwrap();
            let converted = tables.brep(&[(root, Transform::IDENTITY, None)]).unwrap();
            // Check each decoder against a literal chart point, before carrier restoration.
            for source in [&converted, &ascii] {
                let (model, _) =
                    super::super::cad::read_brep_with_authored_frames(source, tolerances).unwrap();
                let (_, surface) = model.geometry().surfaces().next().unwrap();
                let point = surface.point_at(1., 2., tolerances).unwrap();
                close(Vec3::new(point.x, point.y, point.z), Vec3::new(8., 0., 5.));
            }
            let binary_scene = load_binary_brep(&bytes).unwrap();
            let ascii_scene = super::super::cad::load_brep_text(&ascii).unwrap();
            let expected = if placed {
                [
                    Vec3::new(12., 10., 4.),
                    Vec3::new(12., 13., 4.),
                    Vec3::new(8., 10., 4.),
                ]
            } else {
                [
                    Vec3::new(7., -2., 5.),
                    Vec3::new(10., -2., 5.),
                    Vec3::new(7., 2., 5.),
                ]
            };
            for scene in [&binary_scene, &ascii_scene] {
                assert_eq!(scene.triangles.len(), 1);
                assert!(scene.lines.is_empty() && scene.points.is_empty());
                let vertices = scene.triangles[0].vertices;
                for literal in expected {
                    assert!(
                        vertices
                            .iter()
                            .any(|point| (*point - literal).length() < 1e-9)
                    );
                }
                let [a, b, c] = vertices;
                assert!(((b - a).cross(c - a).length() / 2. - 6.).abs() < 1e-9);
                let (low, high) = bounds(scene);
                let (minimum, maximum) = if placed {
                    (Vec3::new(8., 10., 4.), Vec3::new(12., 13., 4.))
                } else {
                    (Vec3::new(7., -2., 5.), Vec3::new(10., 2., 5.))
                };
                close(low, minimum);
                close(high, maximum);
            }
            let options = crate::render::RenderOptions {
                width: 64,
                height: 64,
                transparent: true,
                ..Default::default()
            };
            assert_eq!(
                crate::render::render(&binary_scene, &options).unwrap(),
                crate::render::render(&ascii_scene, &options).unwrap()
            );
        }
    }

    #[test]
    fn binary_direct_conic_retains_right_handed_ax2_orientation() {
        use ogeom_geom::Curve3d;
        let mut bytes = native_header(4);
        native_locations(&mut bytes, &[]);
        bytes.extend(b"Curve2ds 0\nCurves 1\n");
        bytes.push(2);
        // gp_Ax2 is direct: Z=-Z, X=+X requires Y=-Y, unlike the plane above.
        reals(
            &mut bytes,
            &[7., -2., 5., 0., 0., -1., 1., 0., 0., 0., -1., 0., 2.],
        );
        bytes.extend(
            b"Polygon3D 0\nPolygonOnTriangulations 0\nSurfaces 0\nTriangulations 0\nTShapes 3\n",
        );
        for point in [[9., -2., 5.], [7., -4., 5.]] {
            bytes.push(7);
            real(&mut bytes, 1e-7);
            reals(&mut bytes, &point);
            bytes.push(0);
            native_children(&mut bytes, &[]);
        }
        bytes.push(6);
        real(&mut bytes, 1e-7);
        bytes.extend([1, 1, 0, 1]);
        integer(&mut bytes, 1);
        integer(&mut bytes, 0);
        reals(&mut bytes, &[0., std::f64::consts::FRAC_PI_2]);
        bytes.push(0);
        native_children(&mut bytes, &[(0, 3, 0), (1, 2, 0)]);
        bytes.push(0);
        integer(&mut bytes, 1);
        integer(&mut bytes, 0);

        let mut cursor = Cursor::new(&bytes);
        let mut tables = Tables::default();
        tables.read_native_tables(&mut cursor).unwrap();
        let root = tables
            .table_reference(&mut cursor, tables.shapes.len())
            .unwrap()
            .unwrap();
        let text = tables.brep(&[(root, Transform::IDENTITY, None)]).unwrap();
        let tolerances = ogeom_core::Tolerances::millimetres();
        let (model, _) =
            super::super::cad::read_brep_with_authored_frames(&text, tolerances).unwrap();
        let (_, curve) = model.geometry().curves().next().unwrap();
        let point = curve
            .point_at(std::f64::consts::FRAC_PI_2, tolerances)
            .unwrap();
        close(Vec3::new(point.x, point.y, point.z), Vec3::new(7., -4., 5.));
        let scene = load_binary_brep(&bytes).unwrap();
        assert!(scene.triangles.is_empty() && scene.points.is_empty());
        assert!(!scene.lines.is_empty());
        close(
            scene.lines.first().unwrap().vertices[0],
            Vec3::new(9., -2., 5.),
        );
        close(
            scene.lines.last().unwrap().vertices[1],
            Vec3::new(7., -4., 5.),
        );
        assert!(scene.positions().all(|point| point.y <= -2. + 1e-9));
    }

    #[test]
    fn standalone_native_versions_preserve_points_and_nested_root_placements() {
        for version in 1..=4 {
            let scene = load_binary_brep(&native_points(version, &[], 0, 0)).unwrap();
            assert_eq!(scene.points.len(), 2);
            let (low, high) = bounds(&scene);
            close(low, Vec3::new(1., 2., 3.));
            close(high, Vec3::new(4., 5., 6.));
            assert!(scene.points.iter().all(|point| point.color.is_none()));
        }
        let child = Transform([
            [1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [4., -2., 3., 1.],
        ]);
        let root = Transform([
            [0., 2., 0., 0.],
            [-2., 0., 0., 0.],
            [0., 0., 2., 0.],
            [10., 20., 30., 1.],
        ]);
        let scene = load_binary_brep(&native_points(4, &[child, root], 1, 2)).unwrap();
        close(scene.points[0].position, Vec3::new(10., 30., 42.));
        close(scene.points[1].position, Vec3::new(4., 36., 48.));

        // The standalone root selects one occurrence, rather than every table record.
        let mut one_point = native_points(4, &[], 0, 0);
        let index = one_point.len() - 8;
        one_point[index..index + 4].copy_from_slice(&2_i32.to_le_bytes());
        let scene = load_binary_brep(&one_point).unwrap();
        assert_eq!(scene.points.len(), 1);
        close(scene.points[0].position, Vec3::new(4., 5., 6.));
    }

    #[test]
    fn standalone_compounds_preserve_faces_free_wires_points_and_carriers() {
        let carrier = Transform([
            [-1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [7., 5., 2., 1.],
        ]);
        let root = Transform([
            [2., 0., 0., 0.],
            [0., 2., 0., 0.],
            [0., 0., 2., 0.],
            [10., -2., 3., 1.],
        ]);
        let scene = load_binary_brep(&native_mixed(carrier, root, 0, 0)).unwrap();
        assert_eq!(scene.triangles.len(), 1);
        assert_eq!(scene.lines.len(), 1);
        assert_eq!(scene.points.len(), 1);
        let (low, high) = bounds(&scene);
        close(low, Vec3::new(8., 8., 7.));
        close(high, Vec3::new(24., 16., 13.));
        close(scene.points[0].position, Vec3::new(8., 16., 13.));
        let triangle = &scene.triangles[0];
        let area = (triangle.vertices[1] - triangle.vertices[0])
            .cross(triangle.vertices[2] - triangle.vertices[0])
            .length()
            * 0.5;
        assert!((area - 4.).abs() < 1e-6);
        let options = crate::render::RenderOptions {
            width: 96,
            height: 64,
            transparent: true,
            ..Default::default()
        };
        let image = crate::render::render(&scene, &options).unwrap();
        assert!(image.pixels().filter(|pixel| pixel[3] > 0).count() > 50);
    }

    #[test]
    fn reversed_native_compound_root_preserves_authored_face_winding() {
        let load = |parent, child| {
            load_binary_brep(&native_mixed(
                Transform::IDENTITY,
                Transform::IDENTITY,
                parent,
                child,
            ))
            .unwrap()
        };
        let signed_area = |scene: &Scene| {
            let points = scene.triangles[0].vertices;
            (points[1] - points[0]).cross(points[2] - points[0]).z * 0.5
        };
        assert!((signed_area(&load(0, 0)) - 1.).abs() < 1e-6);
        assert!((signed_area(&load(1, 0)) + 1.).abs() < 1e-6);
        assert!((signed_area(&load(1, 1)) - 1.).abs() < 1e-6);
        // Literal results follow the runtime TopAbs::Compose called by
        // TopoDS_Iterator, including the two asymmetric cross cases.
        for (parent, child, expected) in [
            (0, 0, 0),
            (0, 1, 1),
            (0, 2, 2),
            (0, 3, 3),
            (1, 0, 1),
            (1, 1, 0),
            (1, 2, 2),
            (1, 3, 3),
            (2, 0, 2),
            (2, 1, 2),
            (2, 2, 2),
            (2, 3, 3),
            (3, 0, 3),
            (3, 1, 3),
            (3, 2, 2),
            (3, 3, 3),
        ] {
            assert_eq!(compose_orientation(parent, child).unwrap(), expected);
        }
        assert!(compose_orientation(4, 0).is_err());
        assert!(compose_orientation(0, 4).is_err());
    }

    #[test]
    fn standalone_native_root_and_extent_validation_precedes_geometry() {
        let valid = native_points(4, &[], 0, 0);
        for size in [
            0,
            1,
            32,
            65,
            valid.len() / 2,
            valid.len() - 9,
            valid.len() - 1,
        ] {
            assert!(load_binary_brep(&valid[..size]).is_err(), "size {size}");
        }
        for orientation in [4, b'*', b'F'] {
            let mut bad = valid.clone();
            let root = bad.len() - 9;
            bad[root] = orientation;
            assert!(load_binary_brep(&bad).is_err());
        }
        for reverse_id in [-1_i32, 0, 4] {
            let mut bad = valid.clone();
            let start = bad.len() - 8;
            bad[start..start + 4].copy_from_slice(&reverse_id.to_le_bytes());
            assert!(load_binary_brep(&bad).is_err());
        }
        for location in [-1_i32, 1] {
            let mut bad = valid.clone();
            let start = bad.len() - 4;
            bad[start..].copy_from_slice(&location.to_le_bytes());
            assert!(load_binary_brep(&bad).is_err());
        }
        let mut trailing = valid.clone();
        trailing.push(b'\n');
        assert!(
            load_binary_brep(&trailing)
                .unwrap_err()
                .to_string()
                .contains("trailing")
        );
        assert!(!is_binary_brep(b"arbitrary binary\xff"));
        assert!(!is_binary_brep(b"CASCADE Topology V3, (c) Open Cascade\n"));
    }

    #[test]
    fn standalone_native_limits_and_unsupported_tables_fail_before_allocation() {
        let oversized = vec![0; MAX_BYTES + 1];
        assert!(
            load_binary_brep(&oversized)
                .unwrap_err()
                .to_string()
                .contains("16 MiB")
        );
        let version = b"Open CASCADE Topology V5 (c)\nLocations 0\n";
        assert!(
            load_binary_brep(version)
                .unwrap_err()
                .to_string()
                .contains("version")
        );
        for table in ["Locations", "Curve2ds", "Curves", "Surfaces", "TShapes"] {
            let preceding = match table {
                "Locations" => "",
                "Curve2ds" => "Locations 0\n",
                "Curves" => "Locations 0\nCurve2ds 0\n",
                "Surfaces" => {
                    "Locations 0\nCurve2ds 0\nCurves 0\nPolygon3D 0\nPolygonOnTriangulations 0\n"
                }
                _ => {
                    "Locations 0\nCurve2ds 0\nCurves 0\nPolygon3D 0\nPolygonOnTriangulations 0\nSurfaces 0\nTriangulations 0\n"
                }
            };
            let mut bad = native_header(4);
            bad.extend(format!("{preceding}{table} {}\n", MAX_RECORDS + 1).as_bytes());
            assert!(
                load_binary_brep(&bad)
                    .unwrap_err()
                    .to_string()
                    .contains("oversized")
            );
        }
        for cached in ["Polygon3D", "PolygonOnTriangulations", "Triangulations"] {
            let mut bad = native_header(4);
            bad.extend(b"Locations 0\nCurve2ds 0\nCurves 0\n");
            if cached != "Polygon3D" {
                bad.extend(b"Polygon3D 0\n");
            }
            if cached == "Triangulations" {
                bad.extend(b"PolygonOnTriangulations 0\nSurfaces 0\n");
            }
            bad.extend(format!("{cached} 1\n").as_bytes());
            assert!(
                load_binary_brep(&bad)
                    .unwrap_err()
                    .to_string()
                    .contains("unsupported")
            );
        }
    }

    #[test]
    fn legacy_native_vertex_parameters_accept_primary_historical_and_modern_orders() {
        let tables = Tables {
            curves: vec!["1 0 0 0 1 0 0".into()],
            ..Default::default()
        };
        for version in 1..=4 {
            for parameter_first in [false, true] {
                if version > 2 && parameter_first {
                    continue;
                }
                let mut bytes = Vec::new();
                reals(&mut bytes, &[1e-7, 1., 2., 3.]);
                if parameter_first {
                    real(&mut bytes, 0.25);
                    bytes.push(1);
                } else {
                    bytes.push(1);
                    real(&mut bytes, 0.25);
                }
                integer(&mut bytes, 1);
                integer(&mut bytes, 0);
                bytes.push(0);
                let mut cursor = Cursor::new(&bytes);
                let geometry = tables.old_shape_geometry(&mut cursor, 7, version).unwrap();
                assert_eq!(cursor.pos, bytes.len());
                assert_eq!(geometry, "0.0000001 1 2 3 0 0\n");
            }
        }
    }

    fn integer(out: &mut Vec<u8>, value: i32) {
        out.extend(value.to_le_bytes());
    }
    fn real(out: &mut Vec<u8>, value: f64) {
        out.extend(value.to_le_bytes());
    }
    fn reals(out: &mut Vec<u8>, values: &[f64]) {
        for value in values {
            real(out, *value);
        }
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "Small authored test payload lengths fit their native field widths; preserve the exact binary fixture encoding."
    )]
    fn string(out: &mut Vec<u8>, value: &str) {
        integer(out, value.len() as i32);
        out.extend(value.as_bytes());
    }
    fn transform(out: &mut Vec<u8>, value: Transform) {
        out.push(6);
        for row in 0..3 {
            for column in &value.0 {
                real(out, column[row]);
            }
        }
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "Small authored test payload lengths fit their native field widths; preserve the exact binary fixture encoding."
    )]
    fn reference(out: &mut Vec<u8>, offset: usize) {
        let delta = out.len() - offset;
        out.push(3);
        out.extend((delta as u32).to_le_bytes());
    }
    fn vertex(out: &mut Vec<u8>, point: Vec3, orientation: u8) {
        out.extend([228 + orientation, 7]);
        reals(out, &[1e-7, point.x, point.y, point.z]);
        out.extend([0, 8, 199]);
    }
    fn triangle(carrier: Transform, root: Transform) -> Vec<u8> {
        let mut out = vec![200];
        transform(&mut out, root);
        out.push(8);
        out.extend([216, 7, 0]);
        real(&mut out, 1e-7);
        let surface = out.len();
        out.extend([13, 1]);
        reals(&mut out, &[0., 0., 0., 0., 0., 1., 1., 0., 0., 0., 1., 0.]);
        let placement = out.len();
        transform(&mut out, carrier);
        out.extend([0, 8, 220, 7, 8]);
        let points = [
            Vec3::new(0., 0., 0.),
            Vec3::new(2., 0., 0.),
            Vec3::new(0., 1., 0.),
        ];
        for edge in 0..3 {
            let start = points[edge];
            let end = points[(edge + 1) % 3];
            let delta = end - start;
            let length = delta.length();
            let direction = delta / length;
            out.extend([224, 7]);
            real(&mut out, 1e-7);
            out.extend([3, 1, 9, 1]);
            reals(
                &mut out,
                &[start.x, start.y, 0., direction.x, direction.y, 0.],
            );
            reference(&mut out, placement);
            reals(&mut out, &[0., length]);
            out.extend([2, 11, 1]);
            reals(&mut out, &[start.x, start.y, direction.x, direction.y]);
            reference(&mut out, surface);
            reference(&mut out, placement);
            reals(&mut out, &[0., length]);
            out.extend([0, 8]);
            vertex(&mut out, carrier.apply(start), 0);
            vertex(&mut out, carrier.apply(end), 1);
            out.push(199);
        }
        out.extend([199, 199, 199]);
        out
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "Small authored test payload lengths fit their native field widths; preserve the exact binary fixture encoding."
    )]
    fn attribute(kind: i32, id: i32, data: Vec<u8>, direct: Option<Vec<u8>>) -> Vec<u8> {
        let mut out = Vec::new();
        integer(&mut out, kind);
        integer(&mut out, id);
        integer(&mut out, data.len() as i32);
        out.extend(data);
        if let Some(data) = direct {
            out.extend(((data.len() + 8) as u64).to_le_bytes());
            out.extend(data);
        }
        out
    }
    fn tree(id: i32, father: i32, first: i32) -> Vec<u8> {
        let mut data = Vec::new();
        for value in [father, -1, -1, first] {
            integer(&mut data, value);
        }
        data.extend(0xefd2_12e4u32.to_le_bytes());
        data.extend([
            0xfd, 0x6d, 0xd4, 0x11, 0xb9, 0xc8, 0, 0x60, 0xb0, 0xee, 0x28, 0x1b,
        ]);
        attribute(3, id, data, None)
    }
    fn label(attributes: Vec<Vec<u8>>, children: Vec<(i32, Vec<u8>)>, quick: bool) -> Vec<u8> {
        let mut out = if quick { vec![0; 8] } else { Vec::new() };
        for attr in attributes {
            out.extend(attr);
        }
        integer(&mut out, -1);
        for (tag, child) in children {
            integer(&mut out, tag);
            out.extend(child);
        }
        integer(&mut out, -2);
        if quick {
            let size = out.len() as u64;
            out[..8].copy_from_slice(&size.to_le_bytes());
        }
        out
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "Small authored test payload lengths fit their native field widths; preserve the exact binary fixture encoding."
    )]
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
    )]
    fn document(geometry: Vec<u8>, version: u32) -> Vec<u8> {
        let quick = version == 12;
        let mut named = Vec::new();
        integer(&mut named, 1);
        integer(&mut named, 1);
        named.push(b'P');
        let direct = if quick {
            Some(geometry.clone())
        } else {
            named.extend([0, 0, 0]);
            integer(&mut named, 3);
            integer(&mut named, 0);
            named.push(b'F');
            None
        };
        let shape = label(
            vec![
                attribute(1, if quick { -2 } else { 2 }, named, direct),
                tree(3, 4, -1),
            ],
            vec![],
            quick,
        );
        let mut color = Vec::new();
        reals(&mut color, &[0.25, 0.5, 1.0]);
        let color = label(
            vec![tree(4, -1, 3), attribute(4, 5, color, None)],
            vec![],
            quick,
        );
        let body = label(
            vec![],
            vec![(
                1,
                label(
                    vec![],
                    vec![
                        (
                            1,
                            label(vec![attribute(2, 1, vec![], None)], vec![(1, shape)], quick),
                        ),
                        (2, label(vec![], vec![(1, color)], quick)),
                    ],
                    quick,
                ),
            )],
            quick,
        );
        let mut out = b"BINFILE".to_vec();
        out.resize(59, 0);
        integer(&mut out, 1);
        string(&mut out, &version.to_string());
        for _ in 0..3 {
            string(&mut out, "");
        }
        integer(&mut out, 0);
        string(&mut out, "");
        integer(&mut out, 0);
        let info = [
            "FILE_FORMAT: BinXCAF",
            "START_TYPES",
            "TNaming_NamedShape",
            "XCAFDoc_ShapeTool",
            "TDataStd_TreeNode",
            "XCAFDoc_Color",
            "END_TYPES",
        ];
        integer(&mut out, info.len() as i32);
        for item in info {
            string(&mut out, item);
        }
        let end_info = out.len() as i32;
        integer(&mut out, 0);
        let start_doc = out.len() as i32;
        let header = [
            0x0403_0201,
            7,
            end_info,
            end_info,
            start_doc,
            -1,
            -1,
            -1,
            -1,
            -1,
            -1,
            start_doc,
            start_doc,
        ];
        for (index, value) in header.iter().enumerate() {
            out[7 + index * 4..11 + index * 4].copy_from_slice(&value.to_le_bytes());
        }
        if quick {
            integer(&mut out, 4);
            out.extend(b":\0\0\0");
            out.extend([0; 24]);
        } else {
            integer(&mut out, 20);
            out.extend(b"SHAPE_SECTION_POS:\0\0");
            let offset = out.len() + 24 + 4 + body.len();
            out.extend((offset as u64).to_le_bytes());
            out.extend((geometry.len() as u64).to_le_bytes());
            out.extend(0u64.to_le_bytes());
        }
        integer(&mut out, 0);
        out.extend(body);
        if !quick {
            out.extend(geometry);
        }
        out
    }
    fn bounds(scene: &Scene) -> (Vec3, Vec3) {
        let mut low = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
        let mut high = -low;
        for p in scene
            .triangles
            .iter()
            .flat_map(|t| t.vertices)
            .chain(scene.points.iter().map(|p| p.position))
            .chain(scene.lines.iter().flat_map(|l| l.vertices))
        {
            low = Vec3::new(low.x.min(p.x), low.y.min(p.y), low.z.min(p.z));
            high = Vec3::new(high.x.max(p.x), high.y.max(p.y), high.z.max(p.z));
        }
        (low, high)
    }
    fn close(actual: Vec3, expected: Vec3) {
        assert!(
            (actual - expected).length() < 1e-6,
            "{actual:?} differs from {expected:?}"
        );
    }
    #[test]
    fn modern_native_topology_has_actual_area_placement_color_and_thumbnail() {
        let root = Transform([
            [0., 1., 0., 0.],
            [-1., 0., 0., 0.],
            [0., 0., 1., 0.],
            [10., -2., 3., 1.],
        ]);
        let scene = parse(&document(triangle(Transform::IDENTITY, root), 12)).unwrap();
        let (low, high) = bounds(&scene);
        close(low, Vec3::new(9., -2., 3.));
        close(high, Vec3::new(10., 0., 3.));
        let area: f64 = scene
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    * 0.5
            })
            .sum();
        assert!((area - 1.).abs() < 1e-6);
        assert!(
            scene
                .triangles
                .iter()
                .all(|t| t.color == Some([137, 188, 255]))
        );
        let options = crate::render::RenderOptions {
            width: 96,
            height: 64,
            transparent: true,
            ..Default::default()
        };
        let image = crate::render::render(&scene, &options).unwrap();
        assert!(image.pixels().filter(|p| p[3] > 0).count() > 100);
    }
    #[test]
    fn carrier_locations_are_applied_to_surfaces_and_edge_representations() {
        let carrier = Transform([
            [1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [7., 5., 2., 1.],
        ]);
        let scene = parse(&document(triangle(carrier, Transform::IDENTITY), 12)).unwrap();
        let (low, high) = bounds(&scene);
        close(low, Vec3::new(7., 5., 2.));
        close(high, Vec3::new(9., 6., 2.));
        assert!(!scene.triangles.is_empty());
    }
    #[test]
    fn mirrored_carriers_and_scaled_instances_keep_geometry() {
        let mirrored = Transform([
            [-1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [7., 5., 2., 1.],
        ]);
        let scene = parse(&document(triangle(mirrored, Transform::IDENTITY), 12)).unwrap();
        let (low, high) = bounds(&scene);
        close(low, Vec3::new(5., 5., 2.));
        close(high, Vec3::new(7., 6., 2.));
        let scale = Transform([
            [2., 0., 0., 0.],
            [0., 2., 0., 0.],
            [0., 0., 2., 0.],
            [0., 0., 0., 1.],
        ]);
        let scene = parse(&document(triangle(Transform::IDENTITY, scale), 12)).unwrap();
        let (low, high) = bounds(&scene);
        close(low, Vec3::default());
        close(high, Vec3::new(4., 2., 0.));
        let area: f64 = scene
            .triangles
            .iter()
            .map(|t| {
                (t.vertices[1] - t.vertices[0])
                    .cross(t.vertices[2] - t.vertices[0])
                    .length()
                    * 0.5
            })
            .sum();
        assert!((area - 4.).abs() < 1e-6);
        let error = parse(&document(triangle(scale, Transform::IDENTITY), 12))
            .unwrap_err()
            .to_string();
        assert!(error.contains("scaled XBF"));
    }
    #[test]
    fn legacy_table_archive_uses_document_roots_and_real_vertex_geometry() {
        let mut archive=b"SHAPE_SECTION\nOpen CASCADE Topology V3 (c)\nLocations 0\nCurve2ds 0\nCurves 0\nPolygon3D 0\nPolygonOnTriangulations 0\nSurfaces 0\nTriangulations 0\nTShapes 3\n".to_vec();
        for point in [[1., 2., 3.], [4., 5., 6.]] {
            archive.push(7);
            real(&mut archive, 1e-7);
            reals(&mut archive, &point);
            archive.extend([0, 0, 0, 0, 1, 0, 0, 0, b'*']);
        }
        archive.push(0);
        archive.extend([0, 0, 0, 1, 0, 0, 0]);
        for index in [3, 2] {
            archive.push(0);
            integer(&mut archive, index);
            integer(&mut archive, 0);
        }
        archive.push(b'*');
        let scene = parse(&document(archive, 10)).unwrap();
        assert_eq!(scene.points.len(), 2);
        let (low, high) = bounds(&scene);
        close(low, Vec3::new(1., 2., 3.));
        close(high, Vec3::new(4., 5., 6.));
        assert!(
            scene
                .points
                .iter()
                .all(|p| p.color == Some([137, 188, 255]))
        );
    }
    #[test]
    fn composed_locations_preserve_order_negative_powers_and_mirroring() {
        let translation = Transform([
            [1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [4., -2., 3., 1.],
        ]);
        let mirror = Transform([
            [-1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [0., 0., 0., 1.],
        ]);
        close(
            power(translation, -1)
                .unwrap()
                .apply(Vec3::new(4., -2., 3.)),
            Vec3::default(),
        );
        let mut bytes = vec![5];
        transform(&mut bytes, translation);
        integer(&mut bytes, 1);
        transform(&mut bytes, mirror);
        integer(&mut bytes, 1);
        bytes.push(8);
        let mut tables = Tables::default();
        let location = tables.location(&mut Cursor::new(&bytes), 0).unwrap();
        close(
            tables
                .location_transform(location)
                .unwrap()
                .apply(Vec3::new(1., 2., 3.)),
            Vec3::new(-5., 0., 6.),
        );
        assert!(power(translation, i32::MIN).is_err());
    }
    #[test]
    fn malformed_binary_documents_and_geometry_fail_without_panics() {
        let valid = document(triangle(Transform::IDENTITY, Transform::IDENTITY), 12);
        for size in [0, 7, 12, 58, 100, valid.len() / 2, valid.len() - 1] {
            assert!(parse(&valid[..size]).is_err());
        }
        let mut bad = valid.clone();
        bad[7] = 4;
        assert!(parse(&bad).is_err());
        let mut bad = valid;
        bad[55..59].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(parse(&bad).is_err());
        let mut tables = Tables::default();
        assert!(tables.shape(&mut Cursor::new(&[1, 1, 7, 0]), 0).is_err());
        assert!(tables.geometry(&mut Cursor::new(&[10]), 9).is_err());
        let mut values = vec![1];
        real(&mut values, f64::NAN);
        assert!(curve(&mut Cursor::new(&values), 3, 0).is_err());
        let mut values = vec![7, 0, 0];
        values.extend(65u16.to_le_bytes());
        assert!(curve(&mut Cursor::new(&values), 3, 0).is_err());
    }
}
