//! Source-bounded classic TIFF and BigTIFF directory preflight before codec allocation.

use crate::Result;
use std::collections::HashSet;

/// Maximum linked image directories, including excluded reduced images and masks.
const MAX_PAGES: usize = 4096;
/// Maximum entries in any one directory before constructing the upstream decoder.
const MAX_ENTRIES: usize = 4096;
/// Maximum entries across the complete directory chain.
const MAX_TOTAL_ENTRIES: usize = 65_536;
/// Maximum bytes of any declared metadata value, matching the codec value limit.
pub(super) const MAX_VALUE_BYTES: usize = 1024 * 1024;
/// Maximum cumulative declared metadata payload, including shared spans.
const MAX_METADATA_BYTES: usize = 32 * 1024 * 1024;

/// Header byte order used by every source-bounded directory field.
#[derive(Clone, Copy)]
pub(super) struct Order {
    /// Whether TIFF integer and rational fields use little-endian encoding.
    little: bool,
}

impl Order {
    /// Decode an exact unsigned TIFF word without a temporary allocation.
    pub(super) fn integer(self, bytes: &[u8]) -> u64 {
        if self.little {
            bytes.iter().rev().fold(0, |n, &b| n * 256 + u64::from(b))
        } else {
            bytes.iter().fold(0, |n, &b| n * 256 + u64::from(b))
        }
    }
}

/// A typed metadata entry borrowed from its exact original source span.
pub(super) struct Entry<'a> {
    /// Original TIFF tag identifier.
    pub(super) tag: u16,
    /// Original TIFF storage type identifier.
    pub(super) kind: u16,
    /// Complete declared item count after bounded checked conversion.
    pub(super) count: usize,
    /// Complete source-bounded encoded value, including inline fields.
    pub(super) bytes: &'a [u8],
}

impl Entry<'_> {
    /// Require one unsigned metadata scalar of a supported integer storage width.
    pub(super) fn unsigned(&self, order: Order) -> Result<u64> {
        if self.count != 1 || !matches!(self.kind, 1 | 3 | 4 | 16) {
            return Err("TIFF required integer tag has an invalid type or count".into());
        }
        Ok(order.integer(self.bytes))
    }
}

/// One complete linked image directory; entry values remain borrowed.
pub(super) struct Directory<'a> {
    /// Unique, bounded typed entries in source order.
    entries: Vec<Entry<'a>>,
}

impl<'a> Directory<'a> {
    /// Locate an optional entry after duplicate-tag rejection.
    pub(super) fn get(&self, tag: u16) -> Option<&Entry<'a>> {
        self.entries.iter().find(|entry| entry.tag == tag)
    }

    /// Resolve one unsigned scalar or its specified missing-tag default.
    pub(super) fn unsigned(&self, order: Order, tag: u16, default: u64) -> Result<u64> {
        self.get(tag)
            .map_or(Ok(default), |entry| entry.unsigned(order))
    }
}

/// Validated byte order and complete linked directory chain.
pub(super) struct Chain<'a> {
    /// Original integer/rational byte order.
    pub(super) order: Order,
    /// All actual linked directories, never fabricated from metadata counts.
    pub(super) directories: Vec<Directory<'a>>,
}

/// Return one checked original source span.
fn span(bytes: &[u8], start: usize, count: usize) -> Result<&[u8]> {
    let end = start
        .checked_add(count)
        .ok_or("TIFF source span overflow")?;
    bytes
        .get(start..end)
        .ok_or_else(|| "truncated or out-of-range TIFF metadata span".into())
}

/// Map standard TIFF value storage identifiers to exact byte widths.
fn width(kind: u16) -> Result<usize> {
    match kind {
        1 | 2 | 6 | 7 => Ok(1),
        3 | 8 => Ok(2),
        4 | 9 | 11 | 13 => Ok(4),
        5 | 10 | 12 | 16 | 17 | 18 => Ok(8),
        _ => Err("unsupported TIFF metadata storage type".into()),
    }
}

/// Preflight every directory and value span before allowing codec construction.
///
/// # Errors
/// Returns an error for invalid headers, unsupported types, duplicate tags,
/// cyclic/truncated chains, checked arithmetic, allocation failures or limits.
pub(super) fn preflight(bytes: &[u8]) -> Result<Chain<'_>> {
    let signature = span(bytes, 0, 4)?;
    let order = Order {
        little: match &signature[..2] {
            b"II" => true,
            b"MM" => false,
            _ => return Err("invalid TIFF byte-order signature".into()),
        },
    };
    let big = match order.integer(&signature[2..]) {
        42 => false,
        43 => {
            if order.integer(span(bytes, 4, 2)?) != 8 || order.integer(span(bytes, 6, 2)?) != 0 {
                return Err("unsupported BigTIFF offset width or reserved header".into());
            }
            true
        }
        _ => return Err("invalid TIFF version signature".into()),
    };
    let pointer_width = if big { 8 } else { 4 };
    let count_width = if big { 8 } else { 2 };
    let entry_width = if big { 20 } else { 12 };
    let mut next = order.integer(span(bytes, if big { 8 } else { 4 }, pointer_width)?);
    let mut directories = Vec::new();
    let mut seen = HashSet::new();
    let mut total_entries = 0_usize;
    let mut metadata_bytes = 0_usize;
    while next != 0 {
        if directories.len() >= MAX_PAGES || !seen.insert(next) {
            return Err("TIFF directory cycle or page limit exceeded".into());
        }
        let offset = usize::try_from(next)?;
        let count = usize::try_from(order.integer(span(bytes, offset, count_width)?))?;
        total_entries = total_entries
            .checked_add(count)
            .ok_or("TIFF directory count overflow")?;
        if count > MAX_ENTRIES || total_entries > MAX_TOTAL_ENTRIES {
            return Err("TIFF directory entry count limit exceeded".into());
        }
        let table_start = offset
            .checked_add(count_width)
            .ok_or("TIFF directory offset overflow")?;
        let table_bytes = count
            .checked_mul(entry_width)
            .ok_or("TIFF directory size overflow")?;
        let table = span(bytes, table_start, table_bytes)?;
        let next_start = table_start
            .checked_add(table_bytes)
            .ok_or("TIFF next-directory offset overflow")?;
        next = order.integer(span(bytes, next_start, pointer_width)?);
        let mut entries = Vec::new();
        entries.try_reserve_exact(count)?;
        let mut tags = HashSet::new();
        tags.try_reserve(count)?;
        for record in table.chunks_exact(entry_width) {
            let tag = u16::try_from(order.integer(&record[..2]))?;
            let kind = u16::try_from(order.integer(&record[2..4]))?;
            if !tags.insert(tag) {
                return Err("duplicate TIFF directory tag".into());
            }
            let value_count = usize::try_from(order.integer(&record[4..if big { 12 } else { 8 }]))?;
            let size = value_count
                .checked_mul(width(kind)?)
                .ok_or("TIFF metadata value size overflow")?;
            metadata_bytes = metadata_bytes
                .checked_add(size)
                .ok_or("TIFF metadata allocation overflow")?;
            if size > MAX_VALUE_BYTES || metadata_bytes > MAX_METADATA_BYTES {
                return Err("TIFF metadata value or cumulative payload limit exceeded".into());
            }
            let inline = &record[if big { 12 } else { 8 }..];
            let data = if size <= pointer_width {
                &inline[..size]
            } else {
                span(bytes, usize::try_from(order.integer(inline))?, size)?
            };
            entries.push(Entry {
                tag,
                kind,
                count: value_count,
                bytes: data,
            });
        }
        directories.try_reserve(1)?;
        directories.push(Directory { entries });
    }
    if directories.is_empty() {
        return Err("TIFF contains no actual image directory".into());
    }
    Ok(Chain { order, directories })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frame owned classic-directory records without using a TIFF decoder/writer.
    fn classic(records: &[(u16, u16, u32, u32)], next: u32) -> Vec<u8> {
        let mut bytes = b"II\x2a\0\x08\0\0\0".to_vec();
        bytes.extend(u16::try_from(records.len()).unwrap().to_le_bytes());
        for &(tag, kind, count, value) in records {
            bytes.extend(tag.to_le_bytes());
            bytes.extend(kind.to_le_bytes());
            bytes.extend(count.to_le_bytes());
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(next.to_le_bytes());
        bytes
    }

    #[test]
    fn huge_counts_cycles_and_duplicate_tags_fail_before_codec_construction() {
        let classic_count = b"II\x2a\0\x08\0\0\0\xff\xff";
        assert!(
            preflight(classic_count)
                .err()
                .unwrap()
                .to_string()
                .contains("count limit")
        );
        let mut big = b"II\x2b\0\x08\0\0\0\x10\0\0\0\0\0\0\0".to_vec();
        big.extend(u64::MAX.to_le_bytes());
        assert!(
            preflight(&big)
                .err()
                .unwrap()
                .to_string()
                .contains("count limit")
        );
        assert!(
            preflight(&classic(&[], 8))
                .err()
                .unwrap()
                .to_string()
                .contains("cycle")
        );
        assert!(
            preflight(&classic(&[(256, 4, 1, 1), (256, 4, 1, 2)], 0))
                .err()
                .unwrap()
                .to_string()
                .contains("duplicate")
        );
    }

    #[test]
    fn complete_source_spans_value_limits_and_every_truncated_prefix_are_checked() {
        let valid = classic(&[(256, 4, 1, 1)], 0);
        assert!(preflight(&valid).is_ok());
        for length in 0..valid.len() {
            assert!(preflight(&valid[..length]).is_err(), "prefix {length}");
        }
        assert!(preflight(&classic(&[(256, 4, 2, u32::MAX)], 0)).is_err());
        assert!(
            preflight(&classic(&[(270, 2, u32::MAX, 0)], 0))
                .err()
                .unwrap()
                .to_string()
                .contains("payload limit")
        );
        assert!(
            preflight(&classic(&[(256, 65_535, 1, 0)], 0))
                .err()
                .unwrap()
                .to_string()
                .contains("storage type")
        );
    }
}
