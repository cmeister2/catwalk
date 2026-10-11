//! Affected filtered indexes, typed/edge/sliced data, upstream string decoding and malformed metadata.
use super::*;
use hdf5_writer::{DatasetBuilder, FilterDescription, Hdf5Builder, WriteOptions};

/// Encode one authored real HDF5 file with the independent writer implementation.
fn encode(builder: Hdf5Builder) -> Vec<u8> {
    builder
        .into_plan()
        .unwrap()
        .encode(WriteOptions::default())
        .unwrap()
}

/// Add deflate followed by Fletcher32 to a chunked fixture definition.
fn compressed(dataset: DatasetBuilder) -> DatasetBuilder {
    dataset
        .filter(FilterDescription {
            id: 1,
            name: None,
            client_data: vec![6],
        })
        .filter(FilterDescription {
            id: 3,
            name: None,
            client_data: vec![],
        })
}

/// Produce a signed big-endian field with partially filled edge chunks in all three index families.
fn edge_field(maximum: Vec<u64>) -> (Vec<u8>, Vec<i32>) {
    let values: Vec<i32> = (0..297).map(|index| index * 17 - 2000).collect();
    let dataset = DatasetBuilder::typed_with_order::<i32>(
        "field",
        vec![3, 9, 11],
        hdf5_writer::ByteOrder::BigEndian,
    )
    .data(&values)
    .unwrap()
    .max_shape(maximum)
    .chunked(vec![2, 8, 8])
    .fill_value((-13i32).to_be_bytes());
    (
        Hdf5Builder::new()
            .dataset(compressed(dataset))
            .into_plan()
            .unwrap()
            .encode(WriteOptions {
                byte_order: hdf5_writer::ByteOrder::BigEndian,
                ..Default::default()
            })
            .unwrap(),
        values,
    )
}

#[test]
fn signed_endian_edge_chunks_and_strided_collapsed_slices_match_for_all_indexes() {
    let (tree, expected) = edge_field(vec![u64::MAX, u64::MAX, 11]);
    let (extensible, _) = edge_field(vec![u64::MAX, 9, 11]);
    let extensible = with_authored_extensible_index(extensible);
    let (fixed, _) = edge_field(vec![3, 9, 11]);
    for bytes in [fixed, extensible, tree] {
        let file = Hdf5File::from_bytes(&bytes).unwrap();
        assert!(
            file.inner
                .dataset("field")
                .unwrap()
                .read_array::<i32>()
                .is_err()
        );
        let dataset = file.dataset("field").unwrap();
        let actual = dataset.read_array::<i32>().unwrap();
        assert_eq!(actual.shape(), &[3, 9, 11]);
        assert_eq!(actual.iter().copied().collect::<Vec<_>>(), expected);
        let actual = dataset
            .read_slice::<i32>(&hdf5_reader::SliceInfo {
                selections: vec![
                    hdf5_reader::SliceInfoElem::Index(1),
                    hdf5_reader::SliceInfoElem::Slice {
                        start: 1,
                        end: 9,
                        step: 2,
                    },
                    hdf5_reader::SliceInfoElem::Slice {
                        start: 0,
                        end: 11,
                        step: 3,
                    },
                ],
            })
            .unwrap();
        let expected: Vec<_> = [1, 3, 5, 7]
            .into_iter()
            .flat_map(|row| [0, 3, 6, 9].map(|column| expected[99 + row * 11 + column]))
            .collect();
        assert_eq!(actual.shape(), &[4, 4]);
        assert_eq!(actual.iter().copied().collect::<Vec<_>>(), expected);
    }
}

/// Replace only a writer-emitted tree index with an independently authored inline extensible array.
fn with_authored_extensible_index(mut bytes: Vec<u8>) -> Vec<u8> {
    let file = Hdf5File::from_bytes(&bytes).unwrap();
    let dataset = file.dataset("field").unwrap();
    let address = usize::try_from(dataset.address()).unwrap();
    let mut cursor = Cursor::new(&bytes[address..]);
    assert_eq!(cursor.read_bytes(4).unwrap(), b"OHDR");
    assert_eq!(cursor.read_u8().unwrap(), 2);
    let flags = cursor.read_u8().unwrap();
    assert_eq!(flags & !3, 0);
    let length = usize::try_from(cursor.read_uvar(1usize << flags).unwrap()).unwrap();
    let end = usize::try_from(cursor.position()).unwrap() + length;
    let mut layout = None;
    while usize::try_from(cursor.position()).unwrap() < end {
        let kind = cursor.read_u8().unwrap();
        let size = usize::from(cursor.read_u16_le().unwrap());
        assert_eq!(cursor.read_u8().unwrap(), 0);
        let start = address + usize::try_from(cursor.position()).unwrap();
        cursor.skip(size).unwrap();
        if kind == 8 {
            layout = Some(start..start + size);
        }
    }
    let layout = layout.unwrap();
    let payload = &bytes[layout.clone()];
    assert_eq!(&payload[..3], &[4, 2, 0]);
    let index_type = 5 + usize::from(payload[3]) * usize::from(payload[4]);
    assert_eq!(payload[index_type], 5);
    let tree_address = Cursor::new(&payload[index_type + 7..])
        .read_u64_le()
        .unwrap();
    let mut cursor = Cursor::new(&bytes).at_offset(tree_address).unwrap();
    let tree = hdf5_reader::btree_v2::BTreeV2Header::parse(&mut cursor, 8, 8).unwrap();
    assert_eq!((tree.depth, tree.num_records_in_root), (0, 8));
    assert_eq!(tree.record_size, 39);
    let mut cursor = Cursor::new(&bytes)
        .at_offset(tree.root_node_address + 6)
        .unwrap();
    let mut records = vec![vec![0; 15]; 8];
    for _ in 0..8 {
        let record = cursor.read_bytes(15).unwrap().to_vec();
        let z = usize::try_from(cursor.read_u64_le().unwrap()).unwrap();
        let y = usize::try_from(cursor.read_u64_le().unwrap()).unwrap();
        let x = usize::try_from(cursor.read_u64_le().unwrap()).unwrap();
        records[z * 4 + y * 2 + x] = record;
    }
    let owner = u64::try_from(bytes.len()).unwrap();
    let mut header = b"EAHD\0\x01".to_vec();
    header.extend([15, 8, 8, 8, 2, 4]);
    for value in [0u64, 0, 0, 0, 8, 8] {
        header.extend(value.to_le_bytes());
    }
    header.extend((owner + 72).to_le_bytes());
    header.extend(hdf5_reader::checksum::jenkins_lookup3(&header).to_le_bytes());
    assert_eq!(header.len(), 72);
    let mut block = b"EAIB\0\x01".to_vec();
    block.extend(owner.to_le_bytes());
    for record in records {
        block.extend(record);
    }
    block.extend([0xff; 48]);
    block.extend(hdf5_reader::checksum::jenkins_lookup3(&block).to_le_bytes());
    let mut replacement = payload[..index_type].to_vec();
    replacement.extend([4, 8, 8, 2, 8, 4]);
    replacement.extend(owner.to_le_bytes());
    replacement.push(0);
    assert_eq!(replacement.len(), layout.len());
    bytes[layout].copy_from_slice(&replacement);
    let checksum_position = address + end;
    let checksum = hdf5_reader::checksum::jenkins_lookup3(&bytes[address..checksum_position]);
    bytes[checksum_position..checksum_position + 4].copy_from_slice(&checksum.to_le_bytes());
    bytes.extend(header);
    bytes.extend(block);
    refresh_eof(&mut bytes, 8);
    let actual = Hdf5File::from_bytes(&bytes)
        .unwrap()
        .dataset("field")
        .unwrap();
    assert!(actual.header().unwrap().messages.iter().any(|message| {
        matches!(message, HdfMessage::DataLayout(layout) if matches!(layout.layout, DataLayout::Chunked { chunk_indexing: Some(hdf5_reader::messages::layout::ChunkIndexing::ExtensibleArray { .. }), .. }))
    }));
    bytes
}

#[test]
fn fixed_and_variable_length_scalar_strings_reuse_upstream_decoding() {
    let text = format!("{}λ", "a".repeat(510));
    let fixed = compressed(
        DatasetBuilder::fixed_string_data("fixed", vec![1], &[&text])
            .unwrap()
            .max_shape(vec![u64::MAX])
            .chunked(vec![1]),
    );
    let variable = compressed(
        DatasetBuilder::vlen_string_data("variable", vec![1], &[&text])
            .unwrap()
            .max_shape(vec![u64::MAX])
            .chunked(vec![32]),
    );
    let bytes = encode(Hdf5Builder::new().dataset(fixed).dataset(variable));
    let file = Hdf5File::from_bytes(&bytes).unwrap();
    for name in ["fixed", "variable"] {
        assert!(file.inner.dataset(name).unwrap().read_string().is_err());
        let dataset = file.dataset(name).unwrap();
        assert_eq!(dataset.read_string().unwrap(), text);
        assert_eq!(
            dataset.read_raw_bytes().unwrap().len(),
            if name == "fixed" { 512 } else { 16 }
        );
    }
}

/// Locate the unique first fixed-array record using upstream-parsed layout metadata.
fn fixed_record(bytes: &[u8]) -> (usize, usize, usize) {
    let file = Hdf5File::from_bytes(bytes).unwrap();
    let dataset = file.dataset("field").unwrap();
    let header = dataset.header().unwrap();
    let index = header
        .messages
        .iter()
        .find_map(|message| match message {
            HdfMessage::DataLayout(message) => match message.layout {
                DataLayout::Chunked { address, .. } => Some(usize::try_from(address).unwrap()),
                _ => None,
            },
            _ => None,
        })
        .unwrap();
    let offset_size = usize::from(file.superblock.offset_size);
    let length_size = usize::from(file.superblock.length_size);
    let block_field = index + 8 + length_size;
    let block = usize::try_from(
        Cursor::new(&bytes[block_field..])
            .read_uvar(offset_size)
            .unwrap(),
    )
    .unwrap();
    (index, block + 6 + offset_size, offset_size)
}

/// Update the unchanged-size fixed data-block checksum after changing a sparse record.
fn refresh_fixed_checksum(bytes: &mut [u8], index: usize, record: usize, offset_size: usize) {
    let count = usize::try_from(Cursor::new(&bytes[index + 8..]).read_u64_le().unwrap()).unwrap();
    let checksum = record + count * usize::from(bytes[index + 6]);
    let value = hdf5_reader::checksum::jenkins_lookup3(&bytes[record - 6 - offset_size..checksum]);
    bytes[checksum..checksum + 4].copy_from_slice(&value.to_le_bytes());
}

/// Keep the version-2/3 superblock's EOF and checksum correct after appending a raw chunk.
fn refresh_eof(bytes: &mut [u8], offset_size: usize) {
    assert!(matches!(bytes[8], 2 | 3));
    let eof = u64::try_from(bytes.len()).unwrap();
    let position = 12 + 2 * offset_size;
    bytes[position..position + offset_size].copy_from_slice(&eof.to_le_bytes()[..offset_size]);
    let checksum = 12 + 4 * offset_size;
    let value = hdf5_reader::checksum::jenkins_lookup3(&bytes[..checksum]);
    bytes[checksum..checksum + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn unallocated_chunks_preserve_fill_and_skipped_filters_preserve_raw_pixels() {
    let (mut bytes, values) = edge_field(vec![3, 9, 11]);
    let (index, record, offset_size) = fixed_record(&bytes);
    bytes[record..record + offset_size].fill(0xff);
    refresh_fixed_checksum(&mut bytes, index, record, offset_size);
    let dataset = Hdf5File::from_bytes(&bytes)
        .unwrap()
        .dataset("field")
        .unwrap();
    let actual = dataset.read_array::<i32>().unwrap();
    for z in 0..3 {
        for y in 0..9 {
            for x in 0..11 {
                assert_eq!(
                    actual[[z, y, x]],
                    if z < 2 && y < 8 && x < 8 {
                        -13
                    } else {
                        values[z * 99 + y * 11 + x]
                    }
                );
            }
        }
    }

    let (mut bytes, values) = edge_field(vec![3, 9, 11]);
    let (index, record, offset_size) = fixed_record(&bytes);
    let payload = u64::try_from(bytes.len()).unwrap();
    let mut raw = Vec::new();
    for z in 0..2 {
        for y in 0..8 {
            for x in 0..8 {
                raw.extend_from_slice(&values[z * 99 + y * 11 + x].to_be_bytes());
            }
        }
    }
    assert_eq!(raw.len(), 512);
    bytes.extend(raw);
    bytes[record..record + offset_size].copy_from_slice(&payload.to_le_bytes()[..offset_size]);
    bytes[record + offset_size..record + offset_size + 3]
        .copy_from_slice(&512u32.to_le_bytes()[..3]);
    bytes[record + offset_size + 3..record + offset_size + 7].copy_from_slice(&3u32.to_le_bytes());
    refresh_fixed_checksum(&mut bytes, index, record, offset_size);
    refresh_eof(&mut bytes, offset_size);
    let actual = Hdf5File::from_bytes(&bytes)
        .unwrap()
        .dataset("field")
        .unwrap()
        .read_array::<i32>()
        .unwrap();
    assert_eq!(actual.iter().copied().collect::<Vec<_>>(), values);
}

#[test]
fn invalid_filtered_width_and_corrupt_chunk_payload_return_errors() {
    let (mut bytes, _) = edge_field(vec![3, 9, 11]);
    let (index, record, offset_size) = fixed_record(&bytes);
    bytes[index + 6] = u8::try_from(offset_size + 4).unwrap();
    let checksum = index + 8 + 8 + offset_size;
    let value = hdf5_reader::checksum::jenkins_lookup3(&bytes[index..checksum]);
    bytes[checksum..checksum + 4].copy_from_slice(&value.to_le_bytes());
    assert!(
        Hdf5File::from_bytes(&bytes)
            .unwrap()
            .dataset("field")
            .unwrap()
            .read_array::<i32>()
            .unwrap_err()
            .to_string()
            .contains("width")
    );

    let (mut bytes, _) = edge_field(vec![3, 9, 11]);
    let mut cursor = Cursor::new(&bytes[record..]);
    let payload = usize::try_from(cursor.read_uvar(offset_size).unwrap()).unwrap();
    bytes[payload] ^= 0x40;
    assert!(
        Hdf5File::from_bytes(&bytes)
            .unwrap()
            .dataset("field")
            .unwrap()
            .read_array::<i32>()
            .is_err()
    );
}

#[test]
fn variable_size_widths_and_truncated_fixed_index_metadata_are_checked() {
    for width in [1u8, 3, 5, 6, 7] {
        let mut bytes = vec![0; 128];
        let mut header = b"FAHD\0\x01".to_vec();
        header.extend([8 + width + 4, 0]);
        header.extend(1u64.to_le_bytes());
        header.extend(64u64.to_le_bytes());
        let checksum = hdf5_reader::checksum::jenkins_lookup3(&header);
        header.extend(checksum.to_le_bytes());
        bytes[..header.len()].copy_from_slice(&header);
        let mut block = b"FADB\0\x01".to_vec();
        block.extend(0u64.to_le_bytes());
        block.extend(256u64.to_le_bytes());
        block.extend(&127u64.to_le_bytes()[..usize::from(width)]);
        block.extend(0x25u32.to_le_bytes());
        block.extend(hdf5_reader::checksum::jenkins_lookup3(&block).to_le_bytes());
        bytes[64..64 + block.len()].copy_from_slice(&block);
        let index = chunks::Index {
            data: &bytes,
            offset_size: 8,
            length_size: 8,
            shape: &[1],
            max_shape: None,
            dims: &[1],
        };
        let kind = hdf5_reader::messages::layout::ChunkIndexing::FixedArray {
            page_bits: 0,
            chunk_size_len: width,
        };
        let entries = index.entries(0, &kind).unwrap().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            (entries[0].address, entries[0].size, entries[0].filter_mask),
            (256, 127, 0x25)
        );
        assert_eq!(entries[0].offsets, [0]);
        let mut corrupt = bytes.clone();
        corrupt[24] ^= 1;
        assert!(
            chunks::Index {
                data: &corrupt,
                ..index
            }
            .entries(0, &kind)
            .unwrap_err()
            .to_string()
            .contains("checksum")
        );
        for length in [0, 6, 20, 64, 64 + block.len() - 1] {
            let truncated = chunks::Index {
                data: &bytes[..length],
                offset_size: 8,
                length_size: 8,
                shape: &[1],
                max_shape: None,
                dims: &[1],
            };
            assert!(truncated.entries(0, &kind).is_err());
        }
    }
}

/// Encode a one-record type-11 tree from the documented binary metadata layout.
fn authored_tree(depth: u16) -> Vec<u8> {
    let mut bytes = vec![0; 128];
    let mut header = b"BTHD\0\x0b".to_vec();
    header.extend(64u32.to_le_bytes());
    header.extend(23u16.to_le_bytes());
    header.extend(depth.to_le_bytes());
    header.extend([100, 40]);
    header.extend(64u64.to_le_bytes());
    header.extend(1u16.to_le_bytes());
    header.extend(1u64.to_le_bytes());
    header.extend(hdf5_reader::checksum::jenkins_lookup3(&header).to_le_bytes());
    bytes[..header.len()].copy_from_slice(&header);
    let mut node = if depth == 0 { b"BTLF" } else { b"BTIN" }.to_vec();
    node.extend([0, 11]);
    node.extend(256u64.to_le_bytes());
    node.extend(&127u32.to_le_bytes()[..3]);
    node.extend(0x25u32.to_le_bytes());
    node.extend(0u64.to_le_bytes());
    if depth > 0 {
        node.extend(64u64.to_le_bytes());
        node.push(1);
        node.extend(u64::MAX.to_le_bytes());
        node.push(0);
    }
    node.extend(hdf5_reader::checksum::jenkins_lookup3(&node).to_le_bytes());
    bytes[64..64 + node.len()].copy_from_slice(&node);
    bytes
}

#[test]
fn tree_checksums_cycles_and_oversized_record_counts_return_errors() {
    let bytes = authored_tree(0);
    let kind = hdf5_reader::messages::layout::ChunkIndexing::BTreeV2;
    let entries = make_index(&bytes).entries(0, &kind).unwrap().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!((entries[0].address, entries[0].size), (256, 127));
    let mut corrupt = bytes.clone();
    corrupt[93] ^= 1;
    assert!(
        make_index(&corrupt)
            .entries(0, &kind)
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );
    let cyclic = authored_tree(1);
    assert!(
        make_index(&cyclic)
            .entries(0, &kind)
            .unwrap_err()
            .to_string()
            .contains("cyclic")
    );
    let mut oversized = bytes;
    oversized[24..26].copy_from_slice(&u16::MAX.to_le_bytes());
    let checksum = hdf5_reader::checksum::jenkins_lookup3(&oversized[..34]);
    oversized[34..38].copy_from_slice(&checksum.to_le_bytes());
    assert!(
        make_index(&oversized)
            .entries(0, &kind)
            .unwrap_err()
            .to_string()
            .contains("node limits")
    );
}

/// Supply one-dimensional metadata for independently authored index-only bytes.
fn make_index(data: &[u8]) -> chunks::Index<'_> {
    chunks::Index {
        data,
        offset_size: 8,
        length_size: 8,
        shape: &[1],
        max_shape: None,
        dims: &[1],
    }
}

#[test]
fn affected_netcdf_slice_masks_stored_values_before_unpacking() {
    use netcdf_writer::{NcAttrValue, NcFileBuilder, NcWriteFormat, NcWriteOptions};
    let mut builder = NcFileBuilder::new();
    let mut dimensions = Vec::new();
    for (name, length) in [("time", 2), ("z", 4), ("y", 8), ("x", 8)] {
        dimensions.push(builder.add_dimension(name, length).unwrap());
    }
    let variable = builder.add_variable::<i16>("density", &dimensions).unwrap();
    for (name, values) in [
        ("valid_range", vec![-60., 60.]),
        ("scale_factor", vec![0.5]),
        ("add_offset", vec![10.]),
    ] {
        builder
            .add_variable_attribute(variable, name, NcAttrValue::Doubles(values))
            .unwrap();
    }
    builder
        .set_variable_fill_value(variable, -32767i16)
        .unwrap();
    let mut stored: Vec<i16> = (0..512).map(|index| index % 180 - 90).collect();
    stored[1] = -32767;
    stored[64 + 2 * 8 + 3] = -32767;
    builder.write_variable(variable, &stored).unwrap();
    builder
        .set_variable_chunking(variable, vec![1, 4, 8, 8])
        .unwrap();
    builder
        .set_variable_deflate(variable, Some(6), true)
        .unwrap();
    let (_, bytes) = builder
        .to_vec(NcWriteOptions {
            format: NcWriteFormat::Nc4,
        })
        .unwrap();
    let file =
        NcFile::from_bytes_with_options(&bytes, netcdf_reader::NcOpenOptions::default()).unwrap();
    let selection = NcSliceInfo {
        selections: vec![
            NcSliceInfoElem::Index(0),
            NcSliceInfoElem::Slice {
                start: 1,
                end: 4,
                step: 2,
            },
            NcSliceInfoElem::Slice {
                start: 2,
                end: 8,
                step: 3,
            },
            NcSliceInfoElem::Slice {
                start: 1,
                end: 8,
                step: 2,
            },
        ],
    };
    assert!(
        file.inner
            .read_variable_slice_unpacked_masked("density", &selection)
            .is_err()
    );
    let actual = file
        .read_variable_slice_unpacked_masked("density", &selection)
        .unwrap();
    assert_eq!(actual.shape(), &[2, 2, 4]);
    let expected: Vec<_> = [1, 3]
        .into_iter()
        .flat_map(|z| {
            [2, 5]
                .into_iter()
                .flat_map(move |y| [1, 3, 5, 7].map(move |x| z * 64 + y * 8 + x))
        })
        .map(|index| stored[index])
        .collect();
    for (&actual, &stored) in actual.iter().zip(&expected) {
        if !(-60..=60).contains(&stored) || stored == -32767 {
            assert!(actual.is_nan());
        } else {
            assert_eq!(actual, f64::from(stored) * 0.5 + 10.);
        }
    }
}
