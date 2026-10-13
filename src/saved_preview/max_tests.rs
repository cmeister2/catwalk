use super::*;
use std::io::{Cursor, Write};

fn clipboard() -> Vec<u8> {
    let mut dib = vec![0; 40];
    dib[0..4].copy_from_slice(&40_u32.to_le_bytes());
    dib[4..8].copy_from_slice(&3_u32.to_le_bytes());
    dib[8..12].copy_from_slice(&2_u32.to_le_bytes());
    dib[12..14].copy_from_slice(&1_u16.to_le_bytes());
    dib[14..16].copy_from_slice(&24_u16.to_le_bytes());
    dib[20..24].copy_from_slice(&24_u32.to_le_bytes());
    // Bottom row then top row; padding bytes are deliberately nonzero.
    dib.extend_from_slice(&[255, 255, 255, 0, 0, 0, 30, 20, 10, 91, 92, 93]);
    dib.extend_from_slice(&[0, 0, 255, 0, 255, 0, 255, 0, 0, 81, 82, 83]);
    let mut record = vec![0; 28];
    record[..4].copy_from_slice(&46_u32.to_le_bytes());
    record[4..6].copy_from_slice(&0x0f43_u16.to_le_bytes());
    record[6..10].copy_from_slice(&0x00cc_0020_u32.to_le_bytes());
    for offset in [12, 20] {
        record[offset..offset + 2].copy_from_slice(&2_u16.to_le_bytes());
    }
    for offset in [14, 22] {
        record[offset..offset + 2].copy_from_slice(&3_u16.to_le_bytes());
    }
    record.extend_from_slice(&dib);
    let mut output = vec![0; 34];
    output[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    output[4..8].copy_from_slice(&3_u32.to_le_bytes());
    output[8..10].copy_from_slice(&1_u16.to_le_bytes());
    output[10..12].copy_from_slice(&3_u16.to_le_bytes());
    output[12..14].copy_from_slice(&2_u16.to_le_bytes());
    output[16..18].copy_from_slice(&1_u16.to_le_bytes());
    output[18..20].copy_from_slice(&9_u16.to_le_bytes());
    output[20..22].copy_from_slice(&0x300_u16.to_le_bytes());
    output[22..26].copy_from_slice(&58_u32.to_le_bytes());
    output[28..32].copy_from_slice(&46_u32.to_le_bytes());
    output.extend_from_slice(&record);
    output.extend_from_slice(&[3, 0, 0, 0, 0, 0]);
    output
}

fn properties(clipboard: &[u8]) -> Vec<u8> {
    let mut output = vec![0; 72];
    output[..2].copy_from_slice(&0xfffe_u16.to_le_bytes());
    output[24..28].copy_from_slice(&1_u32.to_le_bytes());
    output[28..44].copy_from_slice(&SUMMARY_FMTID);
    output[44..48].copy_from_slice(&48_u32.to_le_bytes());
    output[52..56].copy_from_slice(&1_u32.to_le_bytes());
    output[56..60].copy_from_slice(&17_u32.to_le_bytes());
    output[60..64].copy_from_slice(&16_u32.to_le_bytes());
    output[64..68].copy_from_slice(&71_u32.to_le_bytes());
    output[68..72].copy_from_slice(&u32::try_from(clipboard.len()).unwrap().to_le_bytes());
    output.extend_from_slice(clipboard);
    let size = u32::try_from(output.len() - 48).unwrap();
    output[48..52].copy_from_slice(&size.to_le_bytes());
    output
}

fn compound(version: cfb::Version, streams: &[(&str, &[u8])]) -> Vec<u8> {
    let mut file =
        cfb::CompoundFile::create_with_version(version, Cursor::new(Vec::new())).unwrap();
    for &(path, data) in streams {
        file.create_storage_all(Path::new(path).parent().unwrap())
            .unwrap();
        file.create_stream(path).unwrap().write_all(data).unwrap();
    }
    file.into_inner().into_inner()
}

fn run(data: &[u8], name: &str) -> Result<Option<RgbaImage>> {
    let path = std::env::temp_dir().join(format!("catwalk-max-{}-{name}.max", std::process::id()));
    std::fs::write(&path, data)?;
    let result = extract(&path);
    assert_eq!(std::fs::read(&path)?, data, "extraction modified source");
    std::fs::remove_file(path)?;
    result
}

#[test]
fn addressed_carrier_preserves_bgr_orientation_and_padding() {
    let image = metafile_thumbnail(&clipboard(), Instant::now())
        .unwrap()
        .unwrap();
    assert_eq!(image.dimensions(), (3, 2));
    assert_eq!(
        image.as_raw(),
        &[
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255, 10,
            20, 30, 255
        ]
    );
}

#[test]
fn root_stream_and_scene_identity_are_required_across_cfb_versions() {
    for (version, name) in [(cfb::Version::V3, "v3"), (cfb::Version::V4, "v4")] {
        for (padding, kind) in [(0, "mini"), (5000, "regular")] {
            let mut summary = properties(&clipboard());
            summary.extend(vec![0; padding]);
            let data = compound(
                version,
                &[
                    ("/Scene", b""),
                    ("/ClassData", b""),
                    ("/ClassDirectory3", b""),
                    ("/\u{5}SummaryInformation", &summary),
                    ("/decoy", b"\x89PNG\r\n\x1a\n"),
                ],
            );
            assert_eq!(
                run(&data, &format!("{name}-{kind}"))
                    .unwrap()
                    .unwrap()
                    .dimensions(),
                (3, 2)
            );
        }
    }
    let summary = properties(&clipboard());
    for (name, streams) in [
        (
            "renamed-ole",
            vec![("/\u{5}SummaryInformation", summary.as_slice())],
        ),
        (
            "nested-summary",
            vec![
                ("/Scene", b"".as_slice()),
                ("/ClassData", b""),
                ("/ClassDirectory3", b""),
                ("/nested/\u{5}SummaryInformation", summary.as_slice()),
            ],
        ),
        (
            "unaddressed",
            vec![
                ("/Scene", b"".as_slice()),
                ("/ClassData", b""),
                ("/ClassDirectory3", b""),
                ("/decoy", summary.as_slice()),
            ],
        ),
    ] {
        assert!(
            run(&compound(cfb::Version::V3, &streams), name)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn wrong_property_identity_and_missing_pid_do_not_supply_images() {
    for offset in [28, 56, 64] {
        let mut data = properties(&clipboard());
        data[offset] ^= 1;
        assert!(property_thumbnail(&data, Instant::now()).unwrap().is_none());
    }
}

#[test]
fn malformed_property_ranges_and_overlapping_values_fail() {
    let original = properties(&clipboard());
    for (offset, value) in [
        (44, 47),
        (48, u32::MAX),
        (52, u32::MAX),
        (60, 8),
        (68, u32::MAX),
    ] {
        let mut data = original.clone();
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(
            property_thumbnail(&data, Instant::now()).is_err(),
            "offset {offset}"
        );
    }
    let mut data = original.clone();
    data.splice(
        64..64,
        [17_u32.to_le_bytes(), 24_u32.to_le_bytes()].concat(),
    );
    data[52..56].copy_from_slice(&2_u32.to_le_bytes());
    data[60..64].copy_from_slice(&24_u32.to_le_bytes());
    let length = u32::try_from(data.len() - 48).unwrap();
    data[48..52].copy_from_slice(&length.to_le_bytes());
    assert!(property_thumbnail(&data, Instant::now()).is_err());
    data[64..68].copy_from_slice(&18_u32.to_le_bytes());
    assert!(property_thumbnail(&data, Instant::now()).is_err());
    // A following addressed property bounds PID 17 even when bytes remain later in the section.
    data[68..72].copy_from_slice(&28_u32.to_le_bytes());
    assert!(property_thumbnail(&data, Instant::now()).is_err());
}

#[test]
fn malformed_metafile_extents_and_bitmap_sizes_fail() {
    let original = clipboard();
    for (offset, value) in [
        (22, u32::MAX),
        (28, 47),
        (34, u32::MAX),
        (34, 2),
        (34 + 28 + 4, 4),
        (34 + 28 + 20, 0),
    ] {
        let mut data = original.clone();
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(
            metafile_thumbnail(&data, Instant::now()).is_err(),
            "offset {offset}"
        );
    }
    for length in [4, 15, 30, original.len() - 1] {
        assert!(metafile_thumbnail(&original[..length], Instant::now()).is_err());
    }
    let mut trailing = original.clone();
    trailing.extend_from_slice(&[0, 0]);
    assert!(metafile_thumbnail(&trailing, Instant::now()).is_err());
}

#[test]
fn unsupported_drawing_operations_are_never_discarded_to_extract_pixels() {
    let original = clipboard();
    for offset in [4, 8, 16, 20, 38, 40, 44, 50, 58, 62] {
        let mut data = original.clone();
        data[offset] ^= 1;
        assert!(
            metafile_thumbnail(&data, Instant::now()).unwrap().is_none(),
            "offset {offset}"
        );
    }
    let mut huge = original.clone();
    huge[10..12].copy_from_slice(&4097_u16.to_le_bytes());
    huge[48..50].copy_from_slice(&4097_u16.to_le_bytes());
    huge[56..58].copy_from_slice(&4097_u16.to_le_bytes());
    assert!(metafile_thumbnail(&huge, Instant::now()).is_err());
}

#[test]
fn property_count_is_bounded_before_identity_table_allocation() {
    let mut data = properties(&clipboard());
    data.resize(48 + 8 + 4097 * 8, 0);
    let size = u32::try_from(data.len() - 48).unwrap();
    data[48..52].copy_from_slice(&size.to_le_bytes());
    data[52..56].copy_from_slice(&4097_u32.to_le_bytes());
    let error = property_thumbnail(&data, Instant::now())
        .unwrap_err()
        .to_string();
    assert!(error.contains("property count"), "{error}");
}

#[test]
fn compound_summary_name_case_uses_original_selected_chain() {
    let summary = properties(&clipboard());
    let data = compound(
        cfb::Version::V3,
        &[
            ("/scene", b""),
            ("/CLASSDATA", b""),
            ("/ClassDirectory3", b""),
            ("/\u{5}sUMMARYiNFORMATION", &summary),
        ],
    );
    assert_eq!(
        run(&data, "mixed-case").unwrap().unwrap().dimensions(),
        (3, 2)
    );
}
