//! Original CFB/OLEPS thumbnail carriers, with no Inventor CAD geometry.
use catwalk::Result;
use std::{fs::File, io::Write, path::Path};

pub fn write(path: &Path, version: cfb::Version, png: &[u8]) -> Result<()> {
    let mut clipboard = u32::MAX.to_le_bytes().to_vec();
    clipboard.extend_from_slice(&3_u32.to_le_bytes());
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)?;
    clipboard.extend_from_slice(&8_u16.to_le_bytes());
    clipboard.extend_from_slice(&(image.width() as u16).to_le_bytes());
    clipboard.extend_from_slice(&(image.height() as u16).to_le_bytes());
    clipboard.extend_from_slice(&0_u16.to_le_bytes());
    clipboard.extend_from_slice(png);
    let mut section = vec![0; 16];
    section[4..8].copy_from_slice(&1_u32.to_le_bytes());
    section[8..12].copy_from_slice(&17_u32.to_le_bytes());
    section[12..16].copy_from_slice(&16_u32.to_le_bytes());
    section.extend_from_slice(&71_u32.to_le_bytes());
    section.extend_from_slice(&(clipboard.len() as u32).to_le_bytes());
    section.extend_from_slice(&clipboard);
    let size = section.len() as u32;
    section[..4].copy_from_slice(&size.to_le_bytes());
    let mut properties = vec![0; 48];
    properties[..2].copy_from_slice(&0xfffe_u16.to_le_bytes());
    properties[24..28].copy_from_slice(&1_u32.to_le_bytes());
    properties[28..44].copy_from_slice(&[
        0x39, 0xde, 0x38, 0x3d, 0x88, 0x05, 0x14, 0x4c, 0xbb, 0x37, 0x18, 0xf4, 0xd5, 0xdd, 0x31,
        0xc7,
    ]);
    properties[44..48].copy_from_slice(&48_u32.to_le_bytes());
    properties.extend_from_slice(&section);
    let mut compound = cfb::CompoundFile::create_with_version(version, File::create(path)?)?;
    compound
        .create_stream("/\u{5}CatwalkAuthoredPreview")?
        .write_all(&properties)?;
    compound.flush()?;
    Ok(())
}
