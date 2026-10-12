//! Bounded borrowed `MessagePack` decoding for qualified stored-model scene metadata.
//!
//! The public byte-transform identifier is adapted from the MIT-licensed
//! `Open-Resin-Alliance/df-plugin-lys` at `cc4e64805d2e31095b084e244ef884018c92d1ba`.
//! Its original notice is retained in `docs/licenses/DF-PLUGIN-LYS-LICENSE`.

use super::lychee_budget::Budget;
use crate::Result;
use rmpv::ValueRef;
use std::collections::BTreeSet;

/// Largest qualified encoded and decoded scene-state member.
pub(super) const MAX_SCENE: usize = 16 * 1024 * 1024;
/// Maximum nested `MessagePack` containers before either structural or value decoding.
const MAX_DEPTH: usize = 64;
/// Maximum cumulative `MessagePack` values, including keys and unknown metadata.
const MAX_NODES: usize = 1_000_000;
/// Maximum UTF-8 field/string payload admitted by the structural preflight.
const MAX_STRING: usize = 1024 * 1024;
/// Public interoperability identifier mask from the MIT-licensed import reference.
const IDENTIFIER_MASK: &[u8] = b"DragonFruitFTW";
/// Masked public identifier bytes used by qualified Mango scene-state transforms.
const IDENTIFIER: [u8; 36] = [
    0x25, 0x4a, 0x04, 0x02, 0x5e, 0x5f, 0x72, 0x44, 0x58, 0x51, 0x10, 0x76, 0x67, 0x7a, 0x70, 0x10,
    0x57, 0x5e, 0x42, 0x56, 0x27, 0x44, 0x42, 0x44, 0x41, 0x7f, 0x64, 0x67, 0x7d, 0x13, 0x52, 0x01,
    0x56, 0x0b, 0x23, 0x45,
];

/// Reverse the qualified byte transform without logging scene contents or identifiers.
///
/// # Errors
/// Returns an error for the 16 MiB scene ceiling, shared budgets or allocation failures.
pub(super) fn decode_bytes(encoded: &[u8], budget: &mut Budget) -> Result<Vec<u8>> {
    if encoded.len() > MAX_SCENE {
        return Err("Lychee scene state exceeds the 16 MiB limit".into());
    }
    budget.storage(encoded.len(), 1)?;
    budget.work(encoded.len())?;
    let mut decoded = Vec::new();
    decoded.try_reserve_exact(encoded.len())?;
    if decoded.capacity() > encoded.len() {
        budget.storage(decoded.capacity() - encoded.len(), 1)?;
    }
    for (index, byte) in encoded.iter().copied().enumerate() {
        let offset = index % IDENTIFIER.len();
        let identifier = IDENTIFIER[offset] ^ IDENTIFIER_MASK[offset % IDENTIFIER_MASK.len()];
        decoded.push(byte.wrapping_sub(identifier));
    }
    Ok(decoded)
}

/// Preflight the entire source before constructing borrowed upstream value containers.
///
/// # Errors
/// Returns an error for malformed/trailing data, duplicate string map keys,
/// invalid UTF-8, depth/node/string limits, or shared work/allocation ceilings.
pub(super) fn decode<'a>(bytes: &'a [u8], budget: &mut Budget) -> Result<ValueRef<'a>> {
    let mut preflight = Preflight {
        bytes,
        offset: 0,
        nodes: 0,
        budget,
    };
    preflight.value(0)?;
    if preflight.offset != bytes.len() {
        return Err("trailing Lychee MessagePack scene bytes".into());
    }
    // Upstream grows arrays/maps geometrically. Charge every key/value twice,
    // plus the four-slot minimum for each potential container, before decoding.
    let values = preflight
        .nodes
        .checked_mul(6)
        .ok_or("Lychee metadata allocation overflow")?;
    preflight
        .budget
        .storage(values, std::mem::size_of::<ValueRef<'_>>())?;
    let mut remaining = bytes;
    let value = rmpv::decode::read_value_ref_with_max_depth(&mut remaining, MAX_DEPTH + 4)?;
    if !remaining.is_empty() {
        return Err("trailing Lychee MessagePack scene bytes".into());
    }
    Ok(value)
}

/// Allocation-preflight cursor over complete decoded bytes and shared work counters.
struct Preflight<'a, 'b> {
    /// Single borrowed decoded source buffer.
    bytes: &'a [u8],
    /// Next unread `MessagePack` byte.
    offset: usize,
    /// Cumulative scalar/container/key values visited.
    nodes: usize,
    /// Shared native reader allocation and work accounting.
    budget: &'b mut Budget,
}

impl<'a> Preflight<'a, '_> {
    /// Take an exact source-bounded span without allocating a copy.
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or("Lychee MessagePack span overflow")?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or("truncated Lychee MessagePack scene")?;
        self.offset = end;
        Ok(bytes)
    }

    /// Read one byte after checking that it is present.
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    /// Read a network-endian declared length of one, two or four bytes.
    fn length(&mut self, width: usize) -> Result<usize> {
        let value = self
            .take(width)?
            .iter()
            .fold(0_u64, |value, &byte| value * 256 + u64::from(byte));
        Ok(usize::try_from(value)?)
    }

    /// Require bounded valid UTF-8 and retain a borrowed map key where requested.
    fn string(&mut self, length: usize) -> Result<&'a str> {
        if length > MAX_STRING {
            return Err("Lychee MessagePack string exceeds the 1 MiB limit".into());
        }
        Ok(std::str::from_utf8(self.take(length)?)?)
    }

    /// Preflight one typed value and return its borrowed text for string map keys.
    fn value(&mut self, depth: usize) -> Result<Option<&'a str>> {
        self.nodes += 1;
        if self.nodes > MAX_NODES || depth > MAX_DEPTH {
            return Err("Lychee MessagePack node or depth limit exceeded".into());
        }
        self.budget.work(1)?;
        let tag = self.byte()?;
        match tag {
            0x00..=0x7f | 0xc0 | 0xc2..=0xc3 | 0xe0..=0xff => {}
            0x80..=0x8f => self.map(usize::from(tag & 15), depth)?,
            0x90..=0x9f => self.array(usize::from(tag & 15), depth)?,
            0xa0..=0xbf => return self.string(usize::from(tag & 31)).map(Some),
            0xcc | 0xd0 => {
                self.take(1)?;
            }
            0xcd | 0xd1 => {
                self.take(2)?;
            }
            0xca | 0xce | 0xd2 => {
                self.take(4)?;
            }
            0xcb | 0xcf | 0xd3 => {
                self.take(8)?;
            }
            0xc4..=0xc6 => {
                let width = 1 << (tag - 0xc4);
                let length = self.length(width)?;
                self.take(length)?;
            }
            0xd9..=0xdb => {
                let width = 1 << (tag - 0xd9);
                let length = self.length(width)?;
                return self.string(length).map(Some);
            }
            0xdc..=0xdd => {
                let width = if tag == 0xdc { 2 } else { 4 };
                let length = self.length(width)?;
                self.array(length, depth)?;
            }
            0xde..=0xdf => {
                let width = if tag == 0xde { 2 } else { 4 };
                let length = self.length(width)?;
                self.map(length, depth)?;
            }
            0xc7..=0xc9 => {
                let width = 1 << (tag - 0xc7);
                let length = self.length(width)?;
                self.take(
                    length
                        .checked_add(1)
                        .ok_or("Lychee extension span overflow")?,
                )?;
            }
            0xd4..=0xd8 => {
                self.take((1 << (tag - 0xd4)) + 1)?;
            }
            0xc1 => return Err("invalid Lychee MessagePack reserved marker".into()),
        }
        Ok(None)
    }

    /// Visit every array member only after its declared count fits source/node bounds.
    fn array(&mut self, count: usize, depth: usize) -> Result<()> {
        self.count(count)?;
        for _ in 0..count {
            self.value(depth + 1)?;
        }
        Ok(())
    }

    /// Validate unique UTF-8 map keys with bounded set capacity before upstream allocation.
    fn map(&mut self, count: usize, depth: usize) -> Result<()> {
        self.count(
            count
                .checked_mul(2)
                .ok_or("Lychee MessagePack map count overflow")?,
        )?;
        self.budget.storage(count, 128)?;
        let mut keys = BTreeSet::new();
        for _ in 0..count {
            if let Some(key) = self.value(depth + 1)?
                && !keys.insert(key)
            {
                return Err("duplicate Lychee MessagePack map key".into());
            }
            self.value(depth + 1)?;
        }
        Ok(())
    }

    /// A declared value count must fit both remaining one-byte markers and the node ceiling.
    fn count(&self, count: usize) -> Result<()> {
        if count > self.bytes.len() - self.offset || count > MAX_NODES - self.nodes {
            return Err("Lychee MessagePack declared count exceeds source or node limits".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_counts_duplicate_keys_trailing_bytes_and_depth_fail_before_value_allocation() {
        for bytes in [
            [0xdd, 0xff, 0xff, 0xff, 0xff].as_slice(),
            &[0xdf, 0xff, 0xff, 0xff, 0xff],
            &[0x82, 0xa1, b'a', 1, 0xa1, b'a', 2],
            &[0xa1, 0xff],
            &[0x90, 0],
            &[0xc1],
            &[0xd4, 0],
        ] {
            assert!(decode(bytes, &mut Budget::default()).is_err());
        }
        let mut nested = vec![0x91; MAX_DEPTH + 1];
        nested.push(0);
        assert!(
            decode(&nested, &mut Budget::default())
                .unwrap_err()
                .to_string()
                .contains("depth")
        );
        assert!(decode(&[0x81, 0xa1, b'a', 0x92, 1, 2], &mut Budget::default()).is_ok());
        assert!(decode(&[0x81, 1, 2], &mut Budget::default()).is_ok());
        assert!(decode(&[0x81, 0xa1, b'a', 0xd4, 4, 9], &mut Budget::default()).is_ok());
    }

    #[test]
    fn allocation_and_work_preflight_fail_before_upstream_value_construction() {
        let mut budget = Budget::default();
        budget.storage(512, 1024 * 1024).unwrap();
        assert!(
            decode(&[0x90], &mut budget)
                .unwrap_err()
                .to_string()
                .contains("allocation")
        );
        let mut budget = Budget::default();
        budget.work(50_000_000).unwrap();
        assert!(
            decode(&[0x90], &mut budget)
                .unwrap_err()
                .to_string()
                .contains("work")
        );
        let mut budget = Budget::default();
        budget.storage(512, 1024 * 1024).unwrap();
        assert!(
            decode_bytes(&[0], &mut budget)
                .unwrap_err()
                .to_string()
                .contains("allocation")
        );
    }

    #[test]
    fn every_truncated_prefix_and_complete_scalar_arrays_are_checked() {
        let bytes = [
            0x81, 0xa1, b'x', 0x93, 0xc3, 0xcd, 0, 4, 0xd9, 3, b'a', b'b', b'c',
        ];
        for end in 0..bytes.len() {
            assert!(
                decode(&bytes[..end], &mut Budget::default()).is_err(),
                "prefix {end}"
            );
        }
        assert!(decode(&bytes, &mut Budget::default()).is_ok());
    }
}
