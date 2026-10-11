//! Research-only derived fixture writer. Removes PID17 `VT_CF` from qualified summary sections.
use serde_json::json;
use sha2::{Digest, Sha256};
/// Fallible result used only by the fixture-derivation example.
type ResearchResult<T> = Result<T, Box<dyn std::error::Error>>;
use std::{
    env, fs,
    io::{Read, Write},
    path::Path,
};

/// Qualified Inventor native summary-information property-set identifier in stored byte order.
const NATIVE: [u8; 16] = [
    0x39, 0xde, 0x38, 0x3d, 0x88, 0x05, 0x14, 0x4c, 0xbb, 0x37, 0x18, 0xf4, 0xd5, 0xdd, 0x31, 0xc7,
];
/// Legacy OLE summary-information property-set identifier in stored byte order.
const LEGACY: [u8; 16] = [
    0xe0, 0x85, 0x9f, 0xf2, 0xf9, 0x4f, 0x68, 0x10, 0xab, 0x91, 0x08, 0x00, 0x2b, 0x27, 0xb3, 0xd9,
];
/// Read a little-endian property-table word from a checked four-byte source slice.
fn u32_at(data: &[u8], offset: usize) -> ResearchResult<usize> {
    let end = offset.checked_add(4).ok_or("property offset overflow")?;
    let value = u32::from_le_bytes(
        data.get(offset..end)
            .ok_or("truncated property set")?
            .try_into()?,
    );
    Ok(usize::try_from(value)?)
}

/// Remove the qualified clipboard preview while preserving every other property payload.
fn strip_section(original: &[u8]) -> ResearchResult<(Vec<u8>, usize)> {
    let size = original.len();
    let properties = u32_at(original, 4)?;
    if properties > 10_000 {
        return Err("invalid property count".into());
    }
    let table_end = 8 + 8 * properties;
    if table_end > size {
        return Err("invalid property count".into());
    }
    let mut entries = Vec::new();
    for property in 0..properties {
        entries.push((
            u32_at(original, 8 + property * 8)?,
            u32_at(original, 12 + property * 8)?,
        ));
    }
    let mut sorted = entries.iter().map(|entry| entry.1).collect::<Vec<_>>();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() != entries.len() {
        return Err("duplicate property offset".into());
    }
    let mut retained = Vec::new();
    let mut removed = 0;
    for (pid, offset) in entries {
        if offset < table_end || offset >= size {
            return Err("invalid property offset".into());
        }
        let end = sorted
            .iter()
            .copied()
            .find(|value| *value > offset)
            .unwrap_or(size);
        let payload = original.get(offset..end).ok_or("invalid property extent")?;
        if pid == 17 && u32_at(payload, 0)? == 0x47 {
            removed += 1;
        } else {
            retained.push((pid, payload));
        }
    }
    if removed == 0 {
        return Ok((original.to_vec(), 0));
    }
    let mut output = vec![0; 8 + 8 * retained.len()];
    output[4..8].copy_from_slice(&u32::try_from(retained.len())?.to_le_bytes());
    for (property, (pid, payload)) in retained.iter().enumerate() {
        let offset = output.len();
        output[8 + 8 * property..12 + 8 * property]
            .copy_from_slice(&u32::try_from(*pid)?.to_le_bytes());
        output[12 + 8 * property..16 + 8 * property]
            .copy_from_slice(&u32::try_from(offset)?.to_le_bytes());
        output.extend_from_slice(payload);
    }
    let size = output.len();
    output[..4].copy_from_slice(&u32::try_from(size)?.to_le_bytes());
    Ok((output, removed))
}

/// Remove only PID 17 with clipboard type from qualified summary sections, preserving other payloads.
fn strip(data: &[u8]) -> ResearchResult<Option<(Vec<u8>, usize)>> {
    if data.len() < 28 || data[..2] != [0xfe, 0xff] {
        return Ok(None);
    }
    let count = u32_at(data, 24)?;
    if count == 0 || count > 16 {
        return Ok(None);
    }
    let mut sections = Vec::new();
    let mut removed = 0;
    for index in 0..count {
        let descriptor = 28 + index * 20;
        let id: [u8; 16] = data
            .get(descriptor..descriptor + 16)
            .ok_or("truncated section descriptor")?
            .try_into()?;
        let start = u32_at(data, descriptor + 16)?;
        let size = u32_at(data, start)?;
        let end = start.checked_add(size).ok_or("section length overflow")?;
        let original = data.get(start..end).ok_or("invalid section length")?;
        let section = if id == NATIVE || id == LEGACY {
            let (section, count) = strip_section(original)?;
            removed += count;
            section
        } else {
            original.to_vec()
        };
        sections.push((descriptor, start, section));
    }
    if removed == 0 {
        return Ok(None);
    }
    sections.sort_by_key(|section| section.1);
    let first = sections[0].1;
    if first < 28 + count * 20 {
        return Err("section overlaps header".into());
    }
    let mut output = data[..first].to_vec();
    for (descriptor, _, section) in sections {
        let start = output.len();
        output[descriptor + 16..descriptor + 20]
            .copy_from_slice(&u32::try_from(start)?.to_le_bytes());
        output.extend_from_slice(&section);
    }
    Ok(Some((output, removed)))
}

/// Copy a native document to a new destination, remove its preview property and print stream hashes.
fn main() -> ResearchResult<()> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let [input, output] = args.as_slice() else {
        return Err("usage: remove-thumbnails INPUT OUTPUT".into());
    };
    if Path::new(input) == Path::new(output) {
        return Err("source must be preserved".into());
    }
    let mut source = fs::File::open(input)?;
    let mut destination = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    std::io::copy(&mut source, &mut destination)?;
    drop(destination);
    let mut file = cfb::open_rw(output)?;
    let names = file
        .walk()
        .filter(cfb::Entry::is_stream)
        .map(|e| e.path().to_owned())
        .collect::<Vec<_>>();
    let mut audit = Vec::new();
    for path in names {
        let mut bytes = Vec::new();
        file.open_stream(&path)?.read_to_end(&mut bytes)?;
        let before = format!("{:x}", Sha256::digest(&bytes));
        let changed = if path
            .file_name()
            .is_some_and(|p| p.to_string_lossy().starts_with('\u{5}'))
        {
            strip(&bytes)?
        } else {
            None
        };
        let (after, removed) = if let Some((new, count)) = changed {
            file.create_stream(&path)?.write_all(&new)?;
            (format!("{:x}", Sha256::digest(&new)), count)
        } else {
            (before.clone(), 0)
        };
        audit.push(json!({"stream":path.to_string_lossy(),"before_sha256":before,"after_sha256":after,"removed_pid17_vt_cf":removed}));
    }
    file.flush()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"source":input,"derived":output,"streams":audit}))?
    );
    Ok(())
}

#[cfg(test)]
/// Checked property-table decoding and exact preservation of unrelated property payloads.
mod tests {
    use super::{NATIVE, strip, u32_at};

    /// Build one native summary section with independent clipboard and text property payloads.
    fn summary() -> Vec<u8> {
        let mut data = vec![0; 48];
        data[..2].copy_from_slice(&[0xfe, 0xff]);
        data[24..28].copy_from_slice(&1_u32.to_le_bytes());
        data[28..44].copy_from_slice(&NATIVE);
        data[44..48].copy_from_slice(&48_u32.to_le_bytes());
        for value in [
            40_u32,
            2,
            17,
            24,
            2,
            32,
            0x47,
            0x0403_0201,
            0x1f,
            0x0607_0809,
        ] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        data
    }

    #[test]
    fn removing_clipboard_preview_preserves_the_other_property_payload_exactly() {
        let original = summary();
        let (derived, removed) = strip(&original).unwrap().unwrap();
        assert_eq!(removed, 1);
        assert_eq!(&derived[..48], &original[..48]);
        assert_eq!(u32_at(&derived, 48).unwrap(), 24);
        assert_eq!(u32_at(&derived, 52).unwrap(), 1);
        assert_eq!(u32_at(&derived, 56).unwrap(), 2);
        assert_eq!(u32_at(&derived, 60).unwrap(), 16);
        assert_eq!(&derived[64..], &original[80..]);
    }

    #[test]
    fn malformed_property_extents_and_offset_overflow_return_errors() {
        assert!(u32_at(&[], usize::MAX).is_err());
        let mut oversized_property = summary();
        oversized_property[68..72].copy_from_slice(&95_u32.to_le_bytes());
        assert!(strip(&oversized_property).is_err());
        let mut oversized_section = summary();
        oversized_section[48..52].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(strip(&oversized_section).is_err());
    }
}
