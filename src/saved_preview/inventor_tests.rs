use super::*;
use std::io::Write;

fn property_set(image: &[u8], fmtid: [u8; 16], pid: u32, format: u32) -> Vec<u8> {
    let mut clipboard = u32::MAX.to_le_bytes().to_vec();
    clipboard.extend_from_slice(&format.to_le_bytes());
    if format == 3 {
        clipboard.extend_from_slice(&[8, 0, 2, 0, 2, 0, 0, 0]);
    }
    clipboard.extend_from_slice(image);
    let mut section = vec![0; 16];
    section[4..8].copy_from_slice(&1_u32.to_le_bytes());
    section[8..12].copy_from_slice(&pid.to_le_bytes());
    section[12..16].copy_from_slice(&16_u32.to_le_bytes());
    section.extend_from_slice(&71_u32.to_le_bytes());
    section.extend_from_slice(&(clipboard.len() as u32).to_le_bytes());
    section.extend_from_slice(&clipboard);
    let section_size = section.len() as u32;
    section[..4].copy_from_slice(&section_size.to_le_bytes());
    let mut output = vec![0; 48];
    output[..2].copy_from_slice(&0xfffe_u16.to_le_bytes());
    output[24..28].copy_from_slice(&1_u32.to_le_bytes());
    output[28..44].copy_from_slice(&fmtid);
    output[44..48].copy_from_slice(&48_u32.to_le_bytes());
    output.extend_from_slice(&section);
    output
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([
            60 * x as u8,
            70 * y as u8,
            200,
            if x == 0 { 0 } else { 180 },
        ])
    });
    let mut output = Cursor::new(Vec::new());
    image
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    output.into_inner()
}

fn compound(version: cfb::Version, streams: &[(&str, &[u8])]) -> Vec<u8> {
    let mut compound =
        cfb::CompoundFile::create_with_version(version, Cursor::new(Vec::new())).unwrap();
    for &(path, data) in streams {
        let parent = Path::new(path).parent().unwrap();
        compound.create_storage_all(parent).unwrap();
        compound
            .create_stream(path)
            .unwrap()
            .write_all(data)
            .unwrap();
    }
    compound.into_inner().into_inner()
}

fn run(bytes: &[u8], suffix: &str) -> Result<Option<Vec<u8>>> {
    let path = std::env::temp_dir().join(format!(
        "catwalk-inventor-{}-{suffix}.ipt",
        std::process::id()
    ));
    std::fs::write(&path, bytes)?;
    let result = extract_png(&path);
    assert_eq!(
        std::fs::read(&path)?,
        bytes,
        "source changed during extraction"
    );
    std::fs::remove_file(path)?;
    result
}

fn pixels(png: &[u8]) -> RgbaImage {
    image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .unwrap()
        .to_rgba8()
}

#[test]
fn root_fmtid_thumbnail_is_selected_across_v3_v4_and_regular_mini_streams() {
    let image = png(3, 2);
    for (version, name) in [(cfb::Version::V3, "v3"), (cfb::Version::V4, "v4")] {
        for (padding, kind) in [(0, "mini"), (5000, "regular")] {
            let mut properties = property_set(&image, NATIVE_SUMMARY, 17, 3);
            properties.extend(vec![0; padding]);
            let bytes = compound(version, &[("/\u{5}obfuscated", &properties)]);
            let output = run(&bytes, &format!("{name}-{kind}")).unwrap().unwrap();
            assert_eq!(pixels(&output), pixels(&image));
        }
    }
}

#[test]
fn nested_distractors_and_wrong_format_or_pid_do_not_supply_preview() {
    let image = png(2, 2);
    let nested = property_set(&image, NATIVE_SUMMARY, 17, 3);
    let wrong_pid = property_set(&image, NATIVE_SUMMARY, 18, 3);
    let wrong_fmtid = property_set(&image, [0; 16], 17, 3);
    let bytes = compound(
        cfb::Version::V3,
        &[
            ("/Members/\u{5}summary", &nested),
            ("/Geometry", &image),
            ("/\u{5}other", &wrong_pid),
            ("/\u{5}wrong", &wrong_fmtid),
        ],
    );
    assert!(run(&bytes, "distractors").unwrap().is_none());
}

#[test]
fn native_priority_legacy_fallback_and_blank_images_are_preserved() {
    let native_image = png(3, 1);
    let legacy_image = png(2, 2);
    let native = property_set(&native_image, NATIVE_SUMMARY, 17, 3);
    let legacy = property_set(&legacy_image, LEGACY_SUMMARY, 17, 3);
    let bytes = compound(
        cfb::Version::V3,
        &[("/\u{5}legacy", &legacy), ("/\u{5}native", &native)],
    );
    assert_eq!(
        pixels(&run(&bytes, "priority").unwrap().unwrap()),
        pixels(&native_image)
    );
    let bad_native = property_set(b"unknown", NATIVE_SUMMARY, 17, 3);
    let bytes = compound(
        cfb::Version::V3,
        &[("/\u{5}legacy", &legacy), ("/\u{5}native", &bad_native)],
    );
    assert_eq!(
        pixels(&run(&bytes, "fallback").unwrap().unwrap()),
        pixels(&legacy_image)
    );
    let mut encoded = Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(2, 2, image::Rgba([240, 240, 240, 255]))
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let blank = property_set(encoded.get_ref(), NATIVE_SUMMARY, 17, 3);
    assert!(
        run(
            &compound(cfb::Version::V3, &[("/\u{5}blank", &blank)]),
            "blank"
        )
        .unwrap()
        .is_some()
    );
}

#[test]
fn bitmap_bottom_up_top_down_and_explicit_alpha_match_pixels() {
    for (signed_height, suffix) in [(2_i32, "bottom"), (-2, "top")] {
        let mut dib = vec![0; 40];
        dib[..4].copy_from_slice(&40_u32.to_le_bytes());
        dib[4..8].copy_from_slice(&2_i32.to_le_bytes());
        dib[8..12].copy_from_slice(&signed_height.to_le_bytes());
        dib[12..14].copy_from_slice(&1_u16.to_le_bytes());
        dib[14..16].copy_from_slice(&24_u16.to_le_bytes());
        dib.extend_from_slice(&[0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0, 0, 0]);
        let properties = property_set(&dib, NATIVE_SUMMARY, 17, 8);
        let output = pixels(
            &run(
                &compound(cfb::Version::V3, &[("/\u{5}dib", &properties)]),
                suffix,
            )
            .unwrap()
            .unwrap(),
        );
        assert_eq!(
            output.get_pixel(0, if signed_height > 0 { 1 } else { 0 }).0,
            [255, 0, 0, 255]
        );
    }
    let mut dib = vec![0; 40];
    dib[..4].copy_from_slice(&40_u32.to_le_bytes());
    dib[4..8].copy_from_slice(&1_i32.to_le_bytes());
    dib[8..12].copy_from_slice(&1_i32.to_le_bytes());
    dib[12..14].copy_from_slice(&1_u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32_u16.to_le_bytes());
    dib[16..20].copy_from_slice(&6_u32.to_le_bytes());
    for value in [0xff0000_u32, 0xff00, 0xff, 0xff000000, 0x80402010] {
        dib.extend_from_slice(&value.to_le_bytes());
    }
    let properties = property_set(&dib, NATIVE_SUMMARY, 17, 8);
    let output = pixels(
        &run(
            &compound(cfb::Version::V4, &[("/\u{5}alpha", &properties)]),
            "alpha",
        )
        .unwrap()
        .unwrap(),
    );
    assert_eq!(output.get_pixel(0, 0).0, [64, 32, 16, 128]);
}

#[test]
fn malformed_checksum_unsupported_encoding_and_dimensions_have_distinct_outcomes() {
    let mut image = png(2, 2);
    image[29] ^= 1;
    let malformed = property_set(&image, NATIVE_SUMMARY, 17, 3);
    assert!(
        run(
            &compound(cfb::Version::V3, &[("/\u{5}bad", &malformed)]),
            "crc"
        )
        .is_err()
    );
    let unsupported = property_set(&png(2, 2), NATIVE_SUMMARY, 17, 99);
    assert!(
        run(
            &compound(cfb::Version::V3, &[("/\u{5}unsupported", &unsupported)]),
            "encoding"
        )
        .unwrap()
        .is_none()
    );
    let mut oversized = PNG_SIGNATURE.to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&4097_u32.to_be_bytes());
    header.extend_from_slice(&1_u32.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    append_chunk(&mut oversized, b"IHDR", &header);
    let oversized = property_set(&oversized, NATIVE_SUMMARY, 17, 3);
    assert!(
        run(
            &compound(cfb::Version::V3, &[("/\u{5}oversized", &oversized)]),
            "dimensions"
        )
        .is_err()
    );
}

#[test]
fn native_png_16bit_precision_survives_metadata_normalization() {
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, 2, 1);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Sixteen);
        encoder
            .add_text_chunk("Comment".into(), "unneeded".into())
            .unwrap();
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[0x12, 0x34, 0xab, 0xcd])
            .unwrap();
    }
    let properties = property_set(&output, NATIVE_SUMMARY, 17, 3);
    let normalized = run(
        &compound(cfb::Version::V3, &[("/\u{5}precision", &properties)]),
        "precision",
    )
    .unwrap()
    .unwrap();
    assert_eq!(normalized[24], 16);
    assert!(!normalized.windows(4).any(|value| value == b"tEXt"));
    let mut decoder = png::Decoder::new(Cursor::new(normalized))
        .read_info()
        .unwrap();
    let mut pixels = vec![0; decoder.output_buffer_size().unwrap()];
    decoder.next_frame(&mut pixels).unwrap();
    assert_eq!(pixels, [0x12, 0x34, 0xab, 0xcd]);
}

#[test]
fn png_stream_truncation_palette_indices_and_chunk_order_are_rejected() {
    let mut header = Vec::new();
    header.extend_from_slice(&1_u32.to_be_bytes());
    header.extend_from_slice(&1_u32.to_be_bytes());
    header.extend_from_slice(&[8, 3, 0, 0, 0]);
    let mut stream = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    stream.write_all(&[0, 1]).unwrap();
    let compressed = stream.finish().unwrap();
    let make = |payload: &[u8], palette: bool| {
        let mut output = PNG_SIGNATURE.to_vec();
        append_chunk(&mut output, b"IHDR", &header);
        if palette {
            append_chunk(&mut output, b"PLTE", &[255, 0, 0]);
        }
        append_chunk(&mut output, b"IDAT", payload);
        append_chunk(&mut output, b"IEND", &[]);
        output
    };
    assert!(normalize_png(&make(&compressed, true), Instant::now()).is_err());
    assert!(normalize_png(&make(&compressed, false), Instant::now()).is_err());
    let mut stream = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    stream.write_all(&[0, 0]).unwrap();
    let compressed = stream.finish().unwrap();
    assert!(
        normalize_png(
            &make(&compressed[..compressed.len() - 2], true),
            Instant::now()
        )
        .is_err()
    );
    assert!(
        normalize_png(&make(&compressed, true), Instant::now())
            .unwrap()
            .is_some()
    );
}

#[test]
fn indexed_bitmap_depths_and_high_color_tables_preserve_exact_colors() {
    for depth in [1_u16, 4, 8, 16, 24, 32] {
        let mut dib = vec![0; 40];
        dib[..4].copy_from_slice(&40_u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1_i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1_i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1_u16.to_le_bytes());
        dib[14..16].copy_from_slice(&depth.to_le_bytes());
        if depth <= 8 {
            dib[32..36].copy_from_slice(&1_u32.to_le_bytes());
            dib.extend_from_slice(&[0, 0, 255, 0]);
            dib.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            // A high-color optimization palette precedes actual pixel bytes.
            dib[32..36].copy_from_slice(&1_u32.to_le_bytes());
            dib.extend_from_slice(&[17, 19, 23, 0]);
            if depth == 16 {
                dib.extend_from_slice(&[0, 0x7c, 0, 0]);
            } else {
                dib.extend_from_slice(&[0, 0, 255, 0]);
            }
        }
        let properties = property_set(&dib, NATIVE_SUMMARY, 17, 8);
        let image = pixels(
            &run(
                &compound(cfb::Version::V3, &[("/\u{5}indexed", &properties)]),
                &format!("depth-{depth}"),
            )
            .unwrap()
            .unwrap(),
        );
        assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 255], "depth {depth}");
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&58_u32.to_le_bytes());
        bmp.extend_from_slice(&dib);
        let properties = property_set(&bmp, NATIVE_SUMMARY, 17, 3);
        let bmp_image = pixels(
            &run(
                &compound(cfb::Version::V4, &[("/\u{5}bmp", &properties)]),
                &format!("bmp-{depth}"),
            )
            .unwrap()
            .unwrap(),
        );
        assert_eq!(image, bmp_image);
    }
}

#[test]
fn malformed_properties_masks_and_container_limits_fail_without_changing_source() {
    let mut properties = property_set(&png(2, 2), NATIVE_SUMMARY, 17, 3);
    properties[60..64].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(
        run(
            &compound(cfb::Version::V3, &[("/\u{5}offset", &properties)]),
            "offset"
        )
        .is_err()
    );
    let properties = property_set(&png(2, 2), NATIVE_SUMMARY, 17, 3);
    let mut data = compound(cfb::Version::V3, &[("/\u{5}source", &properties)]);
    data[64..68].copy_from_slice(&8193_u32.to_le_bytes());
    assert!(
        run(&data, "mini-limit")
            .unwrap_err()
            .to_string()
            .contains("miniFAT")
    );
    assert!(check_time(Instant::now() - Duration::from_secs(6)).is_err());
    let mut dib = vec![0; 40];
    dib[..4].copy_from_slice(&40_u32.to_le_bytes());
    dib[4..8].copy_from_slice(&1_i32.to_le_bytes());
    dib[8..12].copy_from_slice(&1_i32.to_le_bytes());
    dib[12..14].copy_from_slice(&1_u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32_u16.to_le_bytes());
    dib[16..20].copy_from_slice(&3_u32.to_le_bytes());
    for value in [0xff0000_u32, 0xff0000, 0xff, 0] {
        dib.extend_from_slice(&value.to_le_bytes());
    }
    assert!(bitmap::normalize(&dib, false, Instant::now()).is_err());
}

#[test]
fn original_selected_fat_links_and_declared_lengths_are_validated_before_repair() {
    let mut properties = property_set(&png(2, 2), NATIVE_SUMMARY, 17, 3);
    properties.resize(5000, 0);
    let valid = compound(cfb::Version::V3, &[("/\u{5}summary", &properties)]);
    assert!(run(&valid, "valid-chain").unwrap().is_some());
    let fat_start = (u32_le(&valid, 76).unwrap() as usize + 1) * 512;
    let directory_start = (u32_le(&valid, 48).unwrap() as usize + 1) * 512;
    let stream_entry = directory_start + 128;
    let mut id = u32_le(&valid, stream_entry + 116).unwrap();
    loop {
        let next = u32_le(&valid, fat_start + id as usize * 4).unwrap();
        if next == u32::MAX - 1 {
            break;
        }
        id = next;
    }
    let mut outside = valid.clone();
    let invalid = (outside.len() / 512 + 2) as u32;
    outside[fat_start + id as usize * 4..fat_start + id as usize * 4 + 4]
        .copy_from_slice(&invalid.to_le_bytes());
    assert!(
        run(&outside, "outside-chain")
            .unwrap_err()
            .to_string()
            .contains("outside")
    );
    let mut wrong_size = valid.clone();
    wrong_size[stream_entry + 120..stream_entry + 124].copy_from_slice(&4096_u32.to_le_bytes());
    assert!(
        run(&wrong_size, "wrong-chain-size")
            .unwrap_err()
            .to_string()
            .contains("declared size")
    );
    for length in 0..4 {
        let properties = property_set(&vec![0; length], NATIVE_SUMMARY, 17, 3);
        assert!(
            run(
                &compound(cfb::Version::V3, &[("/\u{5}truncated", &properties)]),
                &format!("short-{length}")
            )
            .is_err()
        );
    }
    let properties = property_set(&[0; 4], NATIVE_SUMMARY, 17, 3);
    assert!(
        run(
            &compound(cfb::Version::V3, &[("/\u{5}unknown", &properties)]),
            "complete-unknown"
        )
        .unwrap()
        .is_none()
    );
    let mut other = property_set(&png(2, 2), [0; 16], 17, 3);
    other.resize(128, 0);
    let valid = compound(cfb::Version::V3, &[("/\u{5}other", &other)]);
    assert!(run(&valid, "unknown-valid-mini").unwrap().is_none());
    let directory = (u32_le(&valid, 48).unwrap() as usize + 1) * 512;
    let mut inconsistent = valid;
    inconsistent[directory + 128 + 120..directory + 128 + 124]
        .copy_from_slice(&64_u32.to_le_bytes());
    assert!(
        run(&inconsistent, "unknown-invalid-mini")
            .unwrap_err()
            .to_string()
            .contains("declared size")
    );
}
