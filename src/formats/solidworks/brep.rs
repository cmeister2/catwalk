//! Restore native carrier-relative face senses from exact retained bridge records.
//!
//! The pinned neutral decoder's shell parity solve overwrites intrinsic bridge
//! senses. Retained native markers and their independently addressed surface
//! carriers establish the correct senses without changing the solved geometry.

use crate::Result;
use cadmpeg_ir::{CadIr, SourceFidelity, geometry::SolvedSurfaceGeometry, topology::Sense};
use std::{collections::HashMap, io::Read};

/// Limit the aggregate native B-rep bytes materialized during sense restoration.
const MAX_NATIVE_BYTES: usize = 64 * 1024 * 1024;
/// Limit the number of extracted framed kernel streams.
const MAX_FRAMES: usize = 1024;
/// Native bridge identity anchor from the producer-qualified record layout.
const BRIDGE_MAGIC: [u8; 8] = [0xc2, 0xbc, 0x92, 0x8f, 0x99, 0x6e, 0, 0];
/// Native compressed section identity anchor.
const ZLB_MAGIC: [u8; 16] = [
    0x23, 0x1d, 0xd5, 0x71, 0xda, 0x81, 0x48, 0xa2, 0xa8, 0x58, 0x98, 0xb2, 0x1b, 0x89, 0xef, 0x99,
];

/// One bounded source body with its exact native header removed.
struct Body {
    /// Complete decoded body bytes, retaining exact record offsets.
    bytes: Vec<u8>,
    /// First body byte after the native description and schema header.
    offset: usize,
}

/// Intrinsic native face sense and the surface attribute addressed by its bridge.
struct Bridge {
    /// Native carrier-relative face orientation before neutral graph parity.
    sense: Sense,
    /// Native surface carrier identity from the complete reference roster.
    surface: u16,
}

/// Restore every face's exact stored sense while retaining carrier chart changes.
///
/// # Errors
/// Missing provenance, ambiguous records, unsupported wrappers and limits fail.
pub(super) fn restore_face_senses(ir: &mut CadIr, fidelity: &SourceFidelity) -> Result<()> {
    if ir.model.faces.len() > 100_000 {
        return Err("SolidWorks B-rep exceeds 100,000 face orientation records".into());
    }
    let surfaces = ir
        .model
        .surfaces
        .iter()
        .map(|surface| (&surface.id, &surface.geometry))
        .collect::<HashMap<_, _>>();
    let mut streams = HashMap::new();
    let mut bytes = 0_usize;
    for face in &mut ir.model.faces {
        let provenance = fidelity
            .annotations
            .provenance
            .get(face.id.as_str())
            .ok_or("SolidWorks face has no native bridge provenance")?;
        if provenance.tag.as_deref() != Some("00_0e") {
            return Err("unsupported SolidWorks native face provenance".into());
        }
        let stream = provenance.stream();
        if !streams.contains_key(stream) {
            let bodies = retained_bodies(fidelity, stream, &mut bytes)?;
            streams.insert(stream.to_owned(), bodies);
        }
        let id: u16 = face
            .id
            .as_str()
            .strip_prefix("sldprt:brep:face#")
            .ok_or("unsupported SolidWorks qualified face identity")?
            .parse()?;
        let offset = usize::try_from(provenance.offset)?;
        let mut candidates = streams[stream].iter().filter_map(|body| {
            bridge(&body.bytes[body.offset..], offset, id).map(|sense| (body, sense))
        });
        let (_, bridge) = candidates
            .next()
            .ok_or("SolidWorks native face marker does not match its exact provenance")?;
        if candidates.next().is_some() {
            return Err(
                "SolidWorks native face orientation has ambiguous partition/deltas ownership"
                    .into(),
            );
        }
        let mut sense = bridge.sense;
        if matches!(
            surfaces
                .get(&face.surface)
                .and_then(|surface| surface.solved()),
            Some(SolvedSurfaceGeometry::Torus(_))
        ) {
            let source = fidelity
                .annotations
                .provenance
                .get(face.surface.as_str())
                .ok_or("SolidWorks torus has no native carrier provenance")?;
            if source.tag.as_deref() == Some("compact_surface") {
                if source.stream() != stream {
                    return Err(
                        "SolidWorks torus chart and face reference different source carriers"
                            .into(),
                    );
                }
                let Some(SolvedSurfaceGeometry::Torus(torus)) = surfaces
                    .get(&face.surface)
                    .and_then(|surface| surface.solved())
                else {
                    return Err("SolidWorks torus carrier identity disappeared".into());
                };
                if torus_major_reversed(
                    &streams[stream],
                    usize::try_from(source.offset)?,
                    bridge.surface,
                    torus.major_radius().get(),
                )? {
                    sense = match sense {
                        Sense::Forward => Sense::Reversed,
                        Sense::Reversed => Sense::Forward,
                    };
                }
            }
        }
        face.sense = sense;
    }
    Ok(())
}

/// Recover every source frame that may own one annotated active kernel record.
///
/// # Errors
/// Missing, partial or excessive retained native source carriers fail.
fn retained_bodies(
    fidelity: &SourceFidelity,
    stream: &str,
    bytes: &mut usize,
) -> Result<Vec<Body>> {
    let mut bodies = Vec::new();
    // Native table merging retains the first site stream's name. Include
    // its exact sibling deltas carrier, so updates cannot select a stale
    // partition record when frame identity is ambiguous.
    let deltas = stream
        .strip_suffix("-Partition")
        .map(|prefix| format!("{prefix}-Deltas"));
    for record in fidelity
        .retained_records()
        .values()
        .filter(|record| record.stream() == stream || deltas.as_deref() == Some(record.stream()))
    {
        if record.offset() != 0 {
            return Err("SolidWorks native kernel stream is not retained in full".into());
        }
        bodies.extend(native_bodies(
            record
                .data()
                .ok_or("SolidWorks native kernel bytes were not retained")?,
            bytes,
        )?);
    }
    if bodies.is_empty() {
        return Err("SolidWorks face has no retained native kernel carrier".into());
    }
    Ok(bodies)
}

/// Read a uniquely framed native bridge at one independently supplied offset.
fn bridge(body: &[u8], offset: usize, id: u16) -> Option<Bridge> {
    if body.get(offset..offset.checked_add(2)?)? != [0, 0x0e] {
        return None;
    }
    let mut start = offset.checked_add(2)?;
    if body.get(start) == Some(&0xff) {
        start += 1;
    }
    if u16::from_be_bytes(body.get(start..start + 2)?.try_into().ok()?) != id {
        return None;
    }
    let (marker, surface) =
        if body.get(start + 8) == Some(&1) && body.get(start + 9..start + 17)? == BRIDGE_MAGIC {
            if !(0..5).all(|index| body.get(start + 19 + index * 3) == Some(&1)) {
                return None;
            }
            (start + 32, start + 29)
        } else if body.get(start + 8..start + 16)? == BRIDGE_MAGIC {
            if (0..5).all(|index| body.get(start + 18 + index * 3) == Some(&1)) {
                (start + 31, start + 28)
            } else {
                (start + 26, start + 24)
            }
        } else {
            return None;
        };
    let sense = match body.get(marker)? {
        b'+' => Sense::Forward,
        b'-' => Sense::Reversed,
        _ => return None,
    };
    let surface = u16::from_be_bytes(body.get(surface..surface + 2)?.try_into().ok()?);
    if surface == 0 {
        return None;
    }
    Some(Bridge { sense, surface })
}

/// Recover the one carrier chart sign that the pinned analytic transfer changes.
///
/// # Errors
/// Missing, ambiguous or non-finite native torus carrier values fail.
fn torus_major_reversed(
    bodies: &[Body],
    offset: usize,
    attribute: u16,
    major: f64,
) -> Result<bool> {
    let mut matches = bodies
        .iter()
        .filter_map(|body| raw_torus_sign(&body.bytes[body.offset..], offset, attribute, major));
    let sign = matches
        .next()
        .ok_or("SolidWorks native torus chart sign is unqualified")?;
    if matches.next().is_some() {
        return Err("SolidWorks native torus chart has ambiguous frame ownership".into());
    }
    Ok(sign)
}

/// Validate the exact annotated torus carrier against its bridge and neutral radius.
///
/// Source scalar equality uses the same multiplication as the pinned decoder.
#[allow(clippy::float_cmp)]
fn raw_torus_sign(body: &[u8], offset: usize, attribute: u16, major: f64) -> Option<bool> {
    if body.get(offset..offset.checked_add(2)?) != Some(&[0, 0x36]) {
        return None;
    }
    let mut start = offset.checked_add(2)?;
    if body.get(start) == Some(&0xff) {
        start += 1;
    }
    if u16::from_be_bytes(body.get(start..start + 2)?.try_into().ok()?) != attribute {
        return None;
    }
    let mut signs = Vec::new();
    for marker in [start + 16, start + 21] {
        if marker == start + 21 && !(0..5).all(|index| body.get(start + 8 + index * 3) == Some(&1))
        {
            continue;
        }
        if !matches!(body.get(marker), Some(b'+' | b'-')) {
            continue;
        }
        let Some(values) = body.get(marker + 1..marker + 1 + 88) else {
            continue;
        };
        let numbers = values
            .as_chunks::<8>()
            .0
            .iter()
            .map(|bytes| f64::from_be_bytes(*bytes))
            .collect::<Vec<_>>();
        if numbers.iter().any(|value| !value.is_finite())
            || numbers[6].abs() * 1000. != major
            || numbers[7] <= 0.
        {
            continue;
        }
        let axis = crate::scene::Vec3::new(numbers[3], numbers[4], numbers[5]);
        let reference = crate::scene::Vec3::new(numbers[8], numbers[9], numbers[10]);
        if (axis.length() - 1.).abs() > 1e-8
            || (reference.length() - 1.).abs() > 1e-8
            || axis.dot(reference).abs() > 1e-8
        {
            continue;
        }
        signs.push(numbers[6].is_sign_negative());
    }
    (signs.len() == 1).then(|| signs[0])
}

/// Admit complete framed source streams before inspecting any geometric record.
///
/// # Errors
/// Invalid lengths, compression, native headers and materialization limits fail.
fn native_bodies(source: &[u8], bytes: &mut usize) -> Result<Vec<Body>> {
    let mut payloads = Vec::new();
    if source.starts_with(b"PS\0\0") {
        charge(bytes, source.len())?;
        payloads.push(source.to_vec());
    } else {
        let mut cursor = 0_usize;
        while cursor < source.len() {
            if source[cursor..].iter().all(|byte| *byte == 0) {
                break;
            }
            let length = usize::try_from(u32::from_le_bytes(
                source
                    .get(cursor..cursor + 4)
                    .ok_or("truncated SolidWorks native section length")?
                    .try_into()?,
            ))?;
            let end = cursor
                .checked_add(4)
                .and_then(|value| value.checked_add(length))
                .filter(|end| *end <= source.len())
                .ok_or("SolidWorks native section length escapes its source stream")?;
            if source.get(cursor + 4..cursor + 20) != Some(ZLB_MAGIC.as_slice()) {
                return Err("unsupported SolidWorks native B-rep section wrapper".into());
            }
            let mut at = cursor + 20;
            let mut payload = Vec::new();
            while at < end && !source[at..end].iter().all(|byte| *byte == 0) {
                let header = source
                    .get(at..at + 8)
                    .filter(|_| at + 8 <= end)
                    .ok_or("truncated SolidWorks native compressed frame")?;
                let expanded = usize::try_from(u32::from_le_bytes(header[..4].try_into()?))?;
                let compressed = usize::try_from(u32::from_le_bytes(header[4..].try_into()?))?;
                let next = at
                    .checked_add(8)
                    .and_then(|value| value.checked_add(compressed))
                    .filter(|value| *value <= end)
                    .ok_or("SolidWorks native compressed frame escapes its section")?;
                charge(bytes, expanded)?;
                let mut decoder = flate2::read::ZlibDecoder::new(&source[at + 8..next]);
                let before = payload.len();
                (&mut decoder)
                    .take(u64::try_from(expanded)? + 1)
                    .read_to_end(&mut payload)?;
                if payload.len() - before != expanded
                    || decoder.total_in() != u64::try_from(compressed)?
                {
                    return Err("SolidWorks native B-rep inflation length is inconsistent".into());
                }
                at = next;
            }
            payloads.push(payload);
            if payloads.len() > MAX_FRAMES {
                return Err("SolidWorks native B-rep exceeds 1024 kernel frames".into());
            }
            cursor = end;
        }
    }
    payloads
        .into_iter()
        .map(|bytes| {
            let offset = body_offset(&bytes)?;
            Ok(Body { bytes, offset })
        })
        .collect()
}

/// Identify the validated length-prefixed native schema in the bounded header.
///
/// # Errors
/// Missing/truncated or ambiguous schema headers fail.
fn body_offset(source: &[u8]) -> Result<usize> {
    if !source.starts_with(b"PS\0\0") {
        return Err("SolidWorks restored carrier has no native PS header".into());
    }
    let description = usize::from(u16::from_be_bytes(
        source
            .get(4..6)
            .ok_or("truncated SolidWorks kernel description length")?
            .try_into()?,
    ));
    let first = 6_usize
        .checked_add(description)
        .filter(|first| *first <= source.len())
        .ok_or("SolidWorks kernel description escapes its frame")?;
    let last = first.saturating_add(64).min(source.len());
    let mut schemas = Vec::new();
    for at in first + 1..last.saturating_sub(3) {
        if source.get(at..at + 4) != Some(b"SCH_") {
            continue;
        }
        let length = usize::from(source[at - 1]);
        if length < 4 {
            continue;
        }
        let Some(schema) = source.get(at..at + length).filter(|_| at + length <= last) else {
            continue;
        };
        if schema
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            schemas.push(at + length);
        }
    }
    if schemas.len() != 1 {
        return Err("SolidWorks native kernel schema location is ambiguous".into());
    }
    Ok(schemas[0])
}

/// Charge expanded native bytes before copying or decompressing retained carriers.
///
/// # Errors
/// Aggregate byte overflow or the independent 64 MiB ceiling fails.
fn charge(bytes: &mut usize, amount: usize) -> Result<()> {
    *bytes = bytes
        .checked_add(amount)
        .ok_or("SolidWorks native carrier byte count overflow")?;
    if *bytes > MAX_NATIVE_BYTES {
        return Err("SolidWorks native face orientation carriers exceed 64 MiB".into());
    }
    Ok(())
}
