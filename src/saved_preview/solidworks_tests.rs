use super::*;
use std::{
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

struct Fixture(std::path::PathBuf);

impl Fixture {
    fn new(data: &[u8]) -> Self {
        let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "catwalk-solidworks-{}-{id}.sldprt",
            std::process::id()
        ));
        std::fs::write(&path, data).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn compound(version: cfb::Version, streams: &[(&str, &[u8])]) -> Vec<u8> {
    let mut compound =
        cfb::CompoundFile::create_with_version(version, Cursor::new(Vec::new())).unwrap();
    compound
        .create_stream("ISolidWorksInformation")
        .unwrap()
        .write_all(&[0xfe, 0xff])
        .unwrap();
    for (name, data) in streams {
        if let Some(parent) = Path::new(name)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            compound.create_storage_all(parent).unwrap();
        }
        compound
            .create_stream(name)
            .unwrap()
            .write_all(data)
            .unwrap();
    }
    compound.into_inner().into_inner()
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = RgbaImage::from_pixel(width, height, image::Rgba([200, 40, 80, 180]));
    let mut bytes = Vec::new();
    crate::render::write_png(&image, &mut bytes).unwrap();
    bytes
}

fn dib() -> Vec<u8> {
    let mut header = vec![0; 40];
    header[..4].copy_from_slice(&40u32.to_le_bytes());
    header[4..8].copy_from_slice(&2i32.to_le_bytes());
    header[8..12].copy_from_slice(&1i32.to_le_bytes());
    header[12..14].copy_from_slice(&1u16.to_le_bytes());
    header[14..16].copy_from_slice(&24u16.to_le_bytes());
    header.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
    let mut bytes = u32::try_from(header.len()).unwrap().to_le_bytes().to_vec();
    bytes.extend(header);
    bytes
}

#[test]
fn root_png_precedes_dib_and_configuration_images_in_both_cfb_versions() {
    for version in [cfb::Version::V3, cfb::Version::V4] {
        let encoded = png(3, 2);
        let other = png(8, 8);
        let fixture = Fixture::new(&compound(
            version,
            &[
                ("PreviewPNG", &encoded),
                ("Preview", &dib()),
                ("Config-0-PreviewPNG", &other),
                ("Other/PreviewPNG", &other),
            ],
        ));
        let image = extract(&fixture.0).unwrap().unwrap();
        assert_eq!(image.dimensions(), (3, 2));
        assert!(image.pixels().all(|pixel| pixel.0 == [200, 40, 80, 180]));
    }
}

#[test]
fn dib_fallback_recovers_absent_or_corrupt_png_and_checks_length() {
    for streams in [
        vec![("Preview", dib())],
        vec![("PreviewPNG", b"bad PNG".to_vec()), ("Preview", dib())],
    ] {
        let borrowed: Vec<_> = streams
            .iter()
            .map(|(name, data)| (*name, data.as_slice()))
            .collect();
        let fixture = Fixture::new(&compound(cfb::Version::V3, &borrowed));
        let image = extract(&fixture.0).unwrap().unwrap();
        assert_eq!(image.dimensions(), (2, 1));
        assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(1, 0).0, [0, 255, 0, 255]);
    }
    let mut invalid = dib();
    invalid[0] ^= 1;
    assert!(
        decode_dib(&invalid)
            .unwrap_err()
            .to_string()
            .contains("length")
    );
}

#[test]
fn authored_stream_case_follows_compound_storage_lookup() {
    for (name, data, dimensions) in [
        ("pReVieWPNG", png(3, 2), (3, 2)),
        ("pReVieW", dib(), (2, 1)),
    ] {
        let fixture = Fixture::new(&compound(cfb::Version::V3, &[(name, &data)]));
        assert_eq!(
            extract(&fixture.0).unwrap().unwrap().dimensions(),
            dimensions
        );
    }
}

#[test]
fn unrelated_images_and_empty_carriers_do_not_become_document_previews() {
    let other = png(2, 2);
    for streams in [
        vec![
            ("Other/PreviewPNG", other.as_slice()),
            ("Config-0-PreviewPNG", other.as_slice()),
        ],
        vec![("PreviewPNG", &[][..])],
    ] {
        let fixture = Fixture::new(&compound(cfb::Version::V3, &streams));
        assert!(extract(&fixture.0).unwrap().is_none());
    }
    let mut unrelated = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    unrelated
        .create_stream("PreviewPNG")
        .unwrap()
        .write_all(&other)
        .unwrap();
    let fixture = Fixture::new(&unrelated.into_inner().into_inner());
    assert!(
        extract(&fixture.0)
            .unwrap_err()
            .to_string()
            .contains("SolidWorks document information")
    );
    let fixture = Fixture::new(&compound(
        cfb::Version::V3,
        &[("PreviewPNG", b"bad PNG"), ("Config-0-PreviewPNG", &other)],
    ));
    assert!(extract(&fixture.0).is_err());
}

#[test]
fn selected_images_obey_axis_and_encoded_stream_limits() {
    assert!(decode_png(&png(4097, 1)).is_err());
    let oversized = vec![0; usize::try_from(MAX_STREAM + 1).unwrap()];
    let fixture = Fixture::new(&compound(cfb::Version::V3, &[("PreviewPNG", &oversized)]));
    assert!(
        extract(&fixture.0)
            .unwrap_err()
            .to_string()
            .contains("8 MiB")
    );
}

#[test]
fn original_selected_mini_fat_cycles_are_rejected() {
    let mut data = compound(cfb::Version::V3, &[("PreviewPNG", &png(2, 2))]);
    let name: Vec<_> = "PreviewPNG\0"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let entry = data
        .as_chunks::<128>()
        .0
        .iter()
        .position(|entry| entry.starts_with(&name))
        .unwrap()
        * 128;
    let start = u32::from_le_bytes(data[entry + 116..entry + 120].try_into().unwrap());
    let mini_fat = u32::from_le_bytes(data[60..64].try_into().unwrap());
    let offset =
        (usize::try_from(mini_fat).unwrap() + 1) * 512 + usize::try_from(start).unwrap() * 4;
    data[offset..offset + 4].copy_from_slice(&start.to_le_bytes());
    let fixture = Fixture::new(&data);
    assert!(
        extract(&fixture.0)
            .unwrap_err()
            .to_string()
            .contains("cyclic")
    );
}

#[test]
fn modern_and_truncated_containers_fail_explicitly() {
    let mut data = vec![0; 512];
    data[8..14].copy_from_slice(&[0x14, 0, 6, 0, 8, 0]);
    let fixture = Fixture::new(&data);
    assert!(
        extract(&fixture.0)
            .unwrap_err()
            .to_string()
            .contains("compressed-section")
    );
    let fixture = Fixture::new(&data[..64]);
    assert!(extract(&fixture.0).is_err());
}
