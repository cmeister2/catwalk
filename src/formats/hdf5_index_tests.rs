//! Independent filtered-index examples following the HDF Group's on-disk layouts.
//!
//! These bytes are assembled from the published field layouts, without the
//! upstream writer or reader's hierarchy helpers. In particular, page flags
//! are MSB first and uninitialized pages retain their physical address slots.
//! The examples use eight-byte addresses and lengths with one-byte chunk sizes.
//! Field ordering follows Appendix C of the HDF5 file-format specification and
//! the producer's `H5EAcache.c`, `H5FAcache.c`, `H5B2cache.c` and `H5VMprivate.h`:
//! <https://support.hdfgroup.org/documentation/hdf5/latest/_f_m_t4.html> and
//! <https://github.com/HDFGroup/hdf5/tree/hdf5_1_14_6/src>.

use super::chunks::Index;
use crate::Result;
use hdf5_reader::{
    checksum::jenkins_lookup3, chunk_index::ChunkEntry, messages::layout::ChunkIndexing,
};

/// Shared header location, deliberately distinct from the index block.
const HEADER: usize = 32;
/// Fixed-array data or extensible-array index block location.
const INDEX: usize = 256;
/// Undefined eight-byte address used for unallocated chunks and blocks.
const ABSENT: u64 = u64::MAX;
/// Filtered record width: address, one-byte size and four-byte filter mask.
const RECORD_BYTES: usize = 13;

/// Start a version-zero filtered-client metadata block.
fn prefix(signature: [u8; 4]) -> Vec<u8> {
    let mut bytes = signature.to_vec();
    bytes.extend_from_slice(&[0, 1]);
    bytes
}

/// Append the published lookup3 metadata checksum.
fn seal(bytes: &mut Vec<u8>) {
    bytes.extend_from_slice(&jenkins_lookup3(bytes).to_le_bytes());
}

/// Store a compact metadata block at its declared absolute address.
fn put(data: &mut [u8], address: usize, bytes: &[u8]) {
    data[address..address + bytes.len()].copy_from_slice(bytes);
}

/// Append one filtered chunk entry, preserving the distinct size and mask fields.
fn record(bytes: &mut Vec<u8>, address: Option<u64>) {
    bytes.extend_from_slice(&address.unwrap_or(ABSENT).to_le_bytes());
    bytes.push(if address.is_some() { 90 } else { 0 });
    bytes.extend_from_slice(&if address.is_some() { 7_u32 } else { 0 }.to_le_bytes());
}

/// Compact data records followed by their own metadata checksum.
fn page(addresses: &[Option<u64>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for &address in addresses {
        record(&mut bytes, address);
    }
    seal(&mut bytes);
    bytes
}

/// Resolve a raw index using one dataset axis and two samples per chunk.
fn entries(data: &[u8], samples: u64, kind: &ChunkIndexing) -> Result<Vec<ChunkEntry>> {
    Index {
        data,
        offset_size: 8,
        length_size: 8,
        shape: &[samples],
        max_shape: matches!(kind, ChunkIndexing::ExtensibleArray { .. }).then_some(&[u64::MAX]),
        dims: &[2],
    }
    .entries(u64::try_from(HEADER)?, kind)?
    .ok_or_else(|| "independent one-byte filtered index unexpectedly delegated".into())
}

/// Compare geometry positions, physical data addresses and separately encoded record fields.
fn assert_records(actual: &[ChunkEntry], expected: &[(u64, u64)]) {
    assert_eq!(actual.len(), expected.len());
    for (entry, &(slot, address)) in actual.iter().zip(expected) {
        assert_eq!(entry.offsets, [slot * 2]);
        assert_eq!(entry.address, address);
        assert_eq!(entry.size, 90);
        assert_eq!(entry.filter_mask, 7);
    }
}

/// Fixed array with twelve slots and a four-slot physical page size.
fn fixed_sparse() -> Vec<u8> {
    let mut data = vec![0xa5; 4096];
    let mut header = prefix(*b"FAHD");
    header.extend_from_slice(&[13, 2]);
    header.extend_from_slice(&12_u64.to_le_bytes());
    header.extend_from_slice(&256_u64.to_le_bytes());
    seal(&mut header);
    put(&mut data, HEADER, &header);
    let mut block = prefix(*b"FADB");
    block.extend_from_slice(&32_u64.to_le_bytes());
    // Only physical pages1 and2 are initialized; page0 retains its address slot.
    block.push(0x60);
    seal(&mut block);
    put(&mut data, INDEX, &block);
    let stride = 4 * RECORD_BYTES + 4;
    let start = INDEX + block.len();
    put(
        &mut data,
        start + stride,
        &page(&[None, Some(16_384), None, None]),
    );
    put(
        &mut data,
        start + 2 * stride,
        &page(&[Some(16_512), None, None, Some(16_640)]),
    );
    data
}

/// Four-slot fixed pages must skip a physical uninitialized page and use MSB flags.
#[test]
fn fixed_sparse_pages_keep_physical_slots() -> Result<()> {
    let actual = entries(
        &fixed_sparse(),
        24,
        &ChunkIndexing::FixedArray {
            page_bits: 2,
            chunk_size_len: 1,
        },
    )?;
    assert_records(&actual, &[(5, 16_384), (8, 16_512), (11, 16_640)]);
    Ok(())
}

/// Build an EA header whose two direct blocks precede six secondary pointers.
fn ea_header(data: &mut [u8], maximum_set: u64, realized: u64, blocks: [u64; 4]) {
    let mut header = prefix(*b"EAHD");
    // maxbits8, inline2, minimum block2, minimum secondary pointers2, pagebits3.
    header.extend_from_slice(&[13, 8, 2, 2, 2, 3]);
    for value in blocks.into_iter().chain([maximum_set, realized]) {
        header.extend_from_slice(&value.to_le_bytes());
    }
    header.extend_from_slice(&256_u64.to_le_bytes());
    seal(&mut header);
    put(data, HEADER, &header);
}

/// Store the complete fixed-size EA index, including presently unallocated secondary levels.
fn ea_index(data: &mut [u8], inline: [Option<u64>; 2], direct: [u64; 2], secondary: [u64; 6]) {
    let mut block = prefix(*b"EAIB");
    block.extend_from_slice(&32_u64.to_le_bytes());
    for address in inline {
        record(&mut block, address);
    }
    for address in direct.into_iter().chain(secondary) {
        block.extend_from_slice(&address.to_le_bytes());
    }
    seal(&mut block);
    put(data, INDEX, &block);
}

/// Store one nonpaged EA data block; logical offsets occupy one byte for maxbits8.
fn ea_block(data: &mut [u8], address: usize, logical_offset: u8, records: &[Option<u64>]) {
    let mut block = prefix(*b"EADB");
    block.extend_from_slice(&32_u64.to_le_bytes());
    block.push(logical_offset);
    for &address in records {
        record(&mut block, address);
    }
    seal(&mut block);
    put(data, address, &block);
}

/// Store a secondary block's padded bitmap allocation and data-block addresses.
fn ea_secondary(data: &mut [u8], address: usize, offset: u8, bitmap: &[u8], blocks: &[u64]) {
    let mut block = prefix(*b"EASB");
    block.extend_from_slice(&32_u64.to_le_bytes());
    block.push(offset);
    block.extend_from_slice(bitmap);
    for &address in blocks {
        block.extend_from_slice(&address.to_le_bytes());
    }
    seal(&mut block);
    put(data, address, &block);
}

/// Keep a paged EA header's checksum before the reserved physical page slots.
fn ea_paged_prefix(data: &mut [u8], address: usize, offset: u8) -> usize {
    let mut block = prefix(*b"EADB");
    block.extend_from_slice(&32_u64.to_le_bytes());
    block.push(offset);
    seal(&mut block);
    put(data, address, &block);
    address + block.len()
}

/// Official level0 has one two-slot block; level1 has one four-slot block.
#[test]
fn extensible_direct_levels_preserve_inline_and_holes() -> Result<()> {
    let mut data = vec![0; 4096];
    ea_header(&mut data, 8, 8, [0, 0, 2, 124]);
    ea_index(&mut data, [Some(16_384), None], [512, 768], [ABSENT; 6]);
    ea_block(&mut data, 512, 0, &[None, Some(16_512)]);
    ea_block(&mut data, 768, 6, &[Some(16_640), None, None, Some(16_768)]);
    let actual = entries(&data, 16, &ea_kind())?;
    assert_records(
        &actual,
        &[(0, 16_384), (3, 16_512), (4, 16_640), (7, 16_768)],
    );
    Ok(())
}

/// Build two secondary levels separated by an unallocated four-slot data block.
fn ea_secondary_file() -> Vec<u8> {
    let mut data = vec![0; 4096];
    ea_header(&mut data, 27, 14, [2, 70, 2, 194]);
    ea_index(
        &mut data,
        [None, None],
        [ABSENT; 2],
        [512, 768, ABSENT, ABSENT, ABSENT, ABSENT],
    );
    // Level2 starts at slot8 and contains two four-slot blocks.
    ea_secondary(&mut data, 512, 6, &[], &[ABSENT, 1024]);
    ea_block(
        &mut data,
        1024,
        10,
        &[Some(16_384), None, None, Some(16_512)],
    );
    // Level3 starts at slot16 and contains two eight-slot blocks.
    ea_secondary(&mut data, 768, 14, &[], &[ABSENT, 1280]);
    ea_block(
        &mut data,
        1280,
        22,
        &[None, None, Some(16_640), None, None, None, None, None],
    );
    data
}

/// Secondary pointer levels use their authored ranges, independent of realized-slot statistics.
#[test]
fn extensible_secondary_levels_preserve_unallocated_ranges() -> Result<()> {
    let actual = entries(&ea_secondary_file(), 64, &ea_kind())?;
    assert_records(&actual, &[(12, 16_384), (15, 16_512), (26, 16_640)]);
    Ok(())
}

/// Public layout parameters corresponding exactly to the independent EA header.
fn ea_kind() -> ChunkIndexing {
    ChunkIndexing::ExtensibleArray {
        max_bits: 8,
        index_bits: 2,
        min_elements: 2,
        min_pointers: 2,
        chunk_size_len: 1,
    }
}

/// A level5 EA secondary block has four sixteen-slot blocks and eight-slot pages.
fn ea_paged_file() -> Vec<u8> {
    let mut data = vec![0xa5; 4096];
    ea_header(&mut data, 81, 34, [1, 55, 2, 470]);
    ea_index(
        &mut data,
        [None; 2],
        [ABSENT; 2],
        [ABSENT, ABSENT, ABSENT, 512, ABSENT, ABSENT],
    );
    // Four bytes are stored (one byte per data block), but H5EA's bit lookup
    // uses contiguous dblk_index*page_count+page_index positions: flags1 and2.
    ea_secondary(
        &mut data,
        512,
        62,
        &[0x60, 0, 0, 0],
        &[768, 1280, ABSENT, ABSENT],
    );
    let first = ea_paged_prefix(&mut data, 768, 62);
    let second = ea_paged_prefix(&mut data, 1280, 78);
    let stride = 8 * RECORD_BYTES + 4;
    put(
        &mut data,
        first + stride,
        &page(&[Some(16_384), None, None, None, None, None, None, None]),
    );
    put(
        &mut data,
        second,
        &page(&[Some(16_512), None, None, None, None, None, None, None]),
    );
    data
}

/// EA bitmap padding affects pointer placement; bit numbering and physical pages remain contiguous.
#[test]
fn extensible_sparse_pages_use_secondary_bitmap_and_fixed_strides() -> Result<()> {
    let actual = entries(&ea_paged_file(), 256, &ea_kind())?;
    assert_records(&actual, &[(72, 16_384), (80, 16_512)]);
    Ok(())
}

/// Wrong ownership, corrupted page metadata and truncated initialized pages fail closed.
#[test]
fn index_ownership_checksums_and_truncation_are_rejected() {
    let mut wrong_owner = ea_secondary_file();
    wrong_owner[1024 + 6..1024 + 14].copy_from_slice(&48_u64.to_le_bytes());
    assert!(entries(&wrong_owner, 64, &ea_kind()).is_err());

    let mut wrong_checksum = fixed_sparse();
    // FADB prefix is signature/version/client/owner/bitmap/checksum.
    wrong_checksum[INDEX + 15] ^= 1;
    assert!(
        entries(
            &wrong_checksum,
            24,
            &ChunkIndexing::FixedArray {
                page_bits: 2,
                chunk_size_len: 1
            }
        )
        .is_err()
    );

    let mut short = ea_paged_file();
    short.truncate(1280 + 19 + 8 * RECORD_BYTES);
    assert!(entries(&short, 256, &ea_kind()).is_err());
}

/// Check every allocated EA metadata layer rather than trusting only its header checksum.
#[test]
fn extensible_allocated_metadata_checksums_are_checked() {
    // Header, complete index, secondary block and nonpaged data block checksums.
    for position in [HEADER + 68, INDEX + 104, 512 + 31, 1024 + 67] {
        let mut data = ea_secondary_file();
        data[position] ^= 1;
        assert!(
            entries(&data, 64, &ea_kind()).is_err(),
            "corrupted EA checksum at{position} was accepted"
        );
    }
    // Paged data-block prefix and initialized page checksums are separate words.
    for position in [768 + 15, 768 + 19 + 108 + 104] {
        let mut data = ea_paged_file();
        data[position] ^= 1;
        assert!(
            entries(&data, 256, &ea_kind()).is_err(),
            "corrupted EA page checksum at{position} was accepted"
        );
    }
}

/// Append a type11 tree record with a stored chunk index rather than a sample offset.
fn tree_record(bytes: &mut Vec<u8>, slot: u64) {
    record(bytes, Some(16_384 + slot * 128));
    bytes.extend_from_slice(&slot.to_le_bytes());
}

/// Store a compact tree node whose counts have one-byte format-derived widths.
fn tree_node(data: &mut [u8], address: usize, slot: u64, children: &[(u64, Option<u8>)]) {
    let mut bytes = if children.is_empty() {
        prefix(*b"BTLF")
    } else {
        prefix(*b"BTIN")
    };
    bytes[5] = 11;
    tree_record(&mut bytes, slot);
    for &(child, subtree_count) in children {
        bytes.extend_from_slice(&child.to_le_bytes());
        bytes.push(1);
        if let Some(count) = subtree_count {
            bytes.push(count);
        }
    }
    seal(&mut bytes);
    put(data, address, &bytes);
}

/// Depth2 child counts use format-derived widths, independent of eight-byte file lengths.
#[test]
fn tree_depth_two_uses_one_byte_subtree_counts() -> Result<()> {
    let mut data = vec![0; 4096];
    let mut header = prefix(*b"BTHD");
    header[5] = 11;
    header.extend_from_slice(&128_u32.to_le_bytes());
    header.extend_from_slice(&21_u16.to_le_bytes());
    header.extend_from_slice(&2_u16.to_le_bytes());
    header.extend_from_slice(&[100, 40]);
    header.extend_from_slice(&512_u64.to_le_bytes());
    header.extend_from_slice(&1_u16.to_le_bytes());
    header.extend_from_slice(&7_u64.to_le_bytes());
    seal(&mut header);
    put(&mut data, HEADER, &header);
    // leaf maximum5; internal maximum3; maximum subtree23 => one-byte count.
    tree_node(&mut data, 512, 3, &[(768, Some(3)), (1024, Some(3))]);
    tree_node(&mut data, 768, 1, &[(1280, None), (1536, None)]);
    tree_node(&mut data, 1024, 5, &[(1792, None), (2048, None)]);
    for (address, slot) in [(1280, 0), (1536, 2), (1792, 4), (2048, 6)] {
        tree_node(&mut data, address, slot, &[]);
    }
    let mut actual = entries(&data, 14, &ChunkIndexing::BTreeV2)?;
    actual.sort_by_key(|entry| entry.offsets[0]);
    assert_records(
        &actual,
        &[
            (0, 16_384),
            (1, 16_512),
            (2, 16_640),
            (3, 16_768),
            (4, 16_896),
            (5, 17_024),
            (6, 17_152),
        ],
    );
    Ok(())
}

/// Finite array slots use maximum dimensions even before a growable axis reaches that size.
#[test]
fn fixed_slots_use_finite_maximum_dimension_strides() -> Result<()> {
    let mut data = vec![0; 4096];
    let mut header = prefix(*b"FAHD");
    header.extend_from_slice(&[13, 3]);
    header.extend_from_slice(&8_u64.to_le_bytes());
    header.extend_from_slice(&256_u64.to_le_bytes());
    seal(&mut header);
    put(&mut data, HEADER, &header);
    let mut block = prefix(*b"FADB");
    block.extend_from_slice(&32_u64.to_le_bytes());
    for address in [
        Some(16_384),
        Some(16_512),
        None,
        None,
        Some(16_640),
        Some(16_768),
        None,
        None,
    ] {
        record(&mut block, address);
    }
    seal(&mut block);
    put(&mut data, INDEX, &block);
    let actual = Index {
        data: &data,
        offset_size: 8,
        length_size: 8,
        shape: &[2, 2],
        max_shape: Some(&[2, 4]),
        dims: &[1, 1],
    }
    .entries(
        32,
        &ChunkIndexing::FixedArray {
            page_bits: 3,
            chunk_size_len: 1,
        },
    )?
    .ok_or("one-byte fixed index unexpectedly delegated")?;
    let expected = [
        ([0, 0], 16_384),
        ([0, 1], 16_512),
        ([1, 0], 16_640),
        ([1, 1], 16_768),
    ];
    assert_eq!(actual.len(), expected.len());
    for (entry, (coordinates, address)) in actual.iter().zip(expected) {
        assert_eq!(entry.offsets, coordinates);
        assert_eq!(entry.address, address);
    }
    Ok(())
}

/// An EA's nonleading unlimited axis becomes the leading stored axis without transposing output data.
#[test]
fn extensible_slots_restore_nonleading_unlimited_axis_coordinates() -> Result<()> {
    let mut data = vec![0; 4096];
    ea_header(&mut data, 6, 8, [0, 0, 2, 124]);
    ea_index(
        &mut data,
        [Some(16_384), Some(16_512)],
        [512, 768],
        [ABSENT; 6],
    );
    ea_block(&mut data, 512, 0, &[Some(16_640), Some(16_768)]);
    ea_block(&mut data, 768, 6, &[Some(16_896), Some(17_024), None, None]);
    let actual = Index {
        data: &data,
        offset_size: 8,
        length_size: 8,
        shape: &[2, 3],
        max_shape: Some(&[2, u64::MAX]),
        dims: &[1, 1],
    }
    .entries(32, &ea_kind())?
    .ok_or("one-byte extensible index unexpectedly delegated")?;
    let expected = [
        ([0, 0], 16_384),
        ([1, 0], 16_512),
        ([0, 1], 16_640),
        ([1, 1], 16_768),
        ([0, 2], 16_896),
        ([1, 2], 17_024),
    ];
    assert_eq!(actual.len(), expected.len());
    for (entry, (coordinates, address)) in actual.iter().zip(expected) {
        assert_eq!(entry.offsets, coordinates);
        assert_eq!(entry.address, address);
    }
    Ok(())
}
