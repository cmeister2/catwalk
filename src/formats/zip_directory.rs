//! Bounded physical ZIP directory validation shared by native containers.

use super::MAX_INPUT_BYTES;
use crate::Result;
use std::{
    collections::HashSet,
    io::{Read, Seek, SeekFrom},
};

/// Read a fixed-width raw ZIP directory field.
///
/// # Errors
///
/// The requested byte range is truncated or cannot represent the fixed-size field.
fn zip_number<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N]> {
    Ok(bytes
        .get(offset..)
        .and_then(|tail| tail.get(..N))
        .ok_or("truncated ZIP directory field")?
        .try_into()?)
}

/// Decode a little-endian 16-bit ZIP directory field.
///
/// # Errors
///
/// The requested field is truncated.
pub(super) fn zip_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(zip_number(bytes, offset)?))
}

/// Decode a little-endian 32-bit ZIP directory field.
///
/// # Errors
///
/// The requested field is truncated.
pub(super) fn zip_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(zip_number(bytes, offset)?))
}

/// Decode a little-endian 64-bit ZIP directory field.
///
/// # Errors
///
/// The requested field is truncated.
pub(super) fn zip_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(zip_number(bytes, offset)?))
}

/// Seek and read an exact raw package byte range.
///
/// # Errors
///
/// Seeking or reading fails, including a truncated requested range.
fn zip_read(reader: &mut (impl Read + Seek), offset: u64, length: usize) -> Result<Vec<u8>> {
    reader.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// Locate a valid ZIP64 end record, including prefixed/extensible archives.
///
/// # Errors
///
/// The end record is missing, inconsistent, truncated or unreadable.
fn zip64_end(
    reader: &mut (impl Read + Seek),
    locator: u64,
    declared: u64,
) -> Result<(u64, Vec<u8>)> {
    // The usual record has 56 bytes. Also accept an extensible record at the
    // locator's declared offset, including archives with a leading prefix.
    for offset in [declared, locator.saturating_sub(56)] {
        if offset.checked_add(56).is_none_or(|end| end > locator) {
            continue;
        }
        let header = zip_read(reader, offset, 56)?;
        if &header[..4] == b"PK\x06\x06" {
            let size = zip_u64(&header, 4)?;
            if size >= 44
                && offset.checked_add(size).and_then(|n| n.checked_add(12)) == Some(locator)
            {
                return Ok((offset, header));
            }
        }
    }
    // A ZIP64 record with both an archive prefix and extensible data cannot
    // be located directly from the relative locator offset. Scan backwards
    // in bounded chunks, retaining overlap for a complete fixed header.
    let mut end = locator;
    while end > declared {
        let start = end.saturating_sub(65_536).max(declared);
        let bytes = zip_read(reader, start, usize::try_from(end - start)?)?;
        for (index, header) in bytes.windows(56).enumerate().rev() {
            if &header[..4] != b"PK\x06\x06" {
                continue;
            }
            let offset = start + index as u64;
            let size = zip_u64(header, 4)?;
            if size >= 44
                && offset.checked_add(size).and_then(|n| n.checked_add(12)) == Some(locator)
            {
                return Ok((offset, header.to_vec()));
            }
        }
        if start == declared {
            break;
        }
        end = start + 55;
    }
    Err("invalid ZIP64 end-of-directory record".into())
}

/// Inspect the raw central directory before `ZipArchive` coalesces identical
/// names. PKWARE APPNOTE sections 4.3.12--4.3.16 define these fixed headers;
/// compressed entry data and ZIP64 per-entry extras remain the ZIP reader's
/// responsibility. Count and name checks apply to every physical record.
///
/// # Errors
/// Returns an error for unreadable, malformed, multidisk or oversized archives,
/// duplicate physical member names, or counts above the caller's limit.
#[allow(
    clippy::too_many_lines,
    reason = "Keep the format record dispatch, validation and its resource checks together without changing geometry evaluation."
)]
pub(super) fn validate(
    reader: &mut (impl Read + Seek),
    length: u64,
    max_entries: usize,
) -> Result<usize> {
    if !(22..=MAX_INPUT_BYTES).contains(&length) {
        return Err("invalid or oversized ZIP package".into());
    }
    let tail_length = length.min(65_557);
    let tail_start = length - tail_length;
    let tail = zip_read(reader, tail_start, usize::try_from(tail_length)?)?;
    let (index, eocd) = tail
        .windows(22)
        .enumerate()
        .rev()
        .find(|(index, record)| {
            &record[..4] == b"PK\x05\x06"
                && index + 22 + usize::from(u16::from_le_bytes([record[20], record[21]]))
                    == tail.len()
        })
        .ok_or("ZIP end-of-directory record is missing")?;
    let eocd_start = tail_start + index as u64;
    let disk = zip_u16(eocd, 4)?;
    let directory_disk = zip_u16(eocd, 6)?;
    let disk_entries = zip_u16(eocd, 8)?;
    let entries = zip_u16(eocd, 10)?;
    let size = zip_u32(eocd, 12)?;
    let offset = zip_u32(eocd, 16)?;
    let needs_zip64 = [disk, directory_disk, disk_entries, entries].contains(&u16::MAX)
        || [size, offset].contains(&u32::MAX);
    let locator = if eocd_start >= 20 {
        zip_read(reader, eocd_start - 20, 20)?
    } else {
        Vec::new()
    };
    let (entries, size, offset, directory_end) = if locator.starts_with(b"PK\x06\x07") {
        if zip_u32(&locator, 4)? != 0 || zip_u32(&locator, 16)? != 1 {
            return Err("multi-disk ZIP packages are unsupported".into());
        }
        let declared = zip_u64(&locator, 8)?;
        let (end_start, record) = zip64_end(reader, eocd_start - 20, declared)?;
        if declared > end_start
            || zip_u32(&record, 16)? != 0
            || zip_u32(&record, 20)? != 0
            || zip_u64(&record, 24)? != zip_u64(&record, 32)?
        {
            return Err("invalid or multi-disk ZIP64 directory".into());
        }
        let full_entries = zip_u64(&record, 32)?;
        let full_size = zip_u64(&record, 40)?;
        let full_offset = zip_u64(&record, 48)?;
        for (legacy, sentinel, actual) in [
            (u64::from(disk), u64::from(u16::MAX), 0),
            (u64::from(directory_disk), u64::from(u16::MAX), 0),
            (u64::from(disk_entries), u64::from(u16::MAX), full_entries),
            (u64::from(entries), u64::from(u16::MAX), full_entries),
            (u64::from(size), u64::from(u32::MAX), full_size),
            (u64::from(offset), u64::from(u32::MAX), full_offset),
        ] {
            if legacy != sentinel && legacy != actual {
                return Err("inconsistent ZIP and ZIP64 directory fields".into());
            }
        }
        (full_entries, full_size, full_offset, end_start)
    } else {
        if needs_zip64 {
            return Err("ZIP64 locator is missing".into());
        }
        if disk != 0 || directory_disk != 0 || disk_entries != entries {
            return Err("multi-disk ZIP packages are unsupported".into());
        }
        (
            u64::from(entries),
            u64::from(size),
            u64::from(offset),
            eocd_start,
        )
    };
    if entries > u64::try_from(max_entries)? {
        return Err("ZIP archive has too many archive entries".into());
    }
    let directory_start = directory_end
        .checked_sub(size)
        .filter(|&start| start >= offset)
        .ok_or("invalid ZIP central-directory extent")?;
    let mut position = directory_start;
    let mut names = HashSet::new();
    for _ in 0..entries {
        if position
            .checked_add(46)
            .is_none_or(|end| end > directory_end)
        {
            return Err("truncated ZIP central-directory header".into());
        }
        let header = zip_read(reader, position, 46)?;
        if !header.starts_with(b"PK\x01\x02") {
            return Err("invalid ZIP central-directory header".into());
        }
        let name_length = u64::from(zip_u16(&header, 28)?);
        let extra_length = u64::from(zip_u16(&header, 30)?);
        let comment_length = u64::from(zip_u16(&header, 32)?);
        let next = position
            .checked_add(46 + name_length + extra_length + comment_length)
            .filter(|&end| end <= directory_end)
            .ok_or("truncated ZIP central-directory entry")?;
        let name = zip_read(reader, position + 46, usize::try_from(name_length)?)?;
        if !names.insert(name) {
            return Err("duplicate ZIP archive part name".into());
        }
        position = next;
    }
    if position != directory_end {
        // A central-directory digital-signature record is optional and is
        // metadata only. Its bounded payload is not a model or an entry.
        if position
            .checked_add(6)
            .is_none_or(|end| end > directory_end)
        {
            return Err("ZIP directory size or entry count is inconsistent".into());
        }
        let signature = zip_read(reader, position, 6)?;
        if !signature.starts_with(b"PK\x05\x05")
            || position + 6 + u64::from(zip_u16(&signature, 4)?) != directory_end
        {
            return Err("ZIP directory size or entry count is inconsistent".into());
        }
    }
    Ok(usize::try_from(entries)?)
}
