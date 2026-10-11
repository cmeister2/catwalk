//! First-frame DICOM image previews with audited Rust pixel codecs.
use crate::Result;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_pixeldata::{ConvertOptions, PixelDecoder, VoiLutOption};
use image::DynamicImage;
use std::{io::Cursor, path::Path};

pub(crate) fn load(path: &Path) -> Result<DynamicImage> {
    decode(&super::read_file(path)?)
}

fn decode(bytes: &[u8]) -> Result<DynamicImage> {
    let mut object = dicom_object::from_reader(Cursor::new(bytes))?;
    let rows = object.element_by_name("Rows")?.to_int::<u64>()?;
    let columns = object.element_by_name("Columns")?.to_int::<u64>()?;
    let samples = object.element_by_name("SamplesPerPixel")?.to_int::<u64>()?;
    let frames = match object.element_by_name("NumberOfFrames") {
        Ok(element) => element.to_int::<u64>()?,
        Err(_) => 1,
    };
    let allocated = object.element_by_name("BitsAllocated")?.to_int::<u16>()?;
    let count = rows
        .checked_mul(columns)
        .and_then(|v| v.checked_mul(samples))
        .ok_or("DICOM image dimensions overflow")?;
    if rows == 0
        || columns == 0
        || frames == 0
        || count > 16_777_216
        || ![1, 3].contains(&samples)
        || ![8, 16].contains(&allocated)
    {
        return Err(
            "DICOM preview requires up to 16 million scalar/RGB samples with 8/16-bit allocation"
                .into(),
        );
    }
    if object.meta().transfer_syntax() == "1.2.840.10008.1.2.5" {
        let fragments = object
            .element_by_name("PixelData")?
            .value()
            .fragments()
            .ok_or("DICOM RLE has no pixel fragments")?;
        let raw = rle_frame(
            fragments.first().ok_or("DICOM RLE has no first frame")?,
            rows as usize * columns as usize,
            samples as usize,
            allocated as usize / 8,
        )?;
        object.put(DataElement::new(
            Tag(0x7fe0, 0x10),
            VR::OB,
            PrimitiveValue::U8(raw.into()),
        ));
        object.put(DataElement::new(
            Tag(0x0028, 0x0008),
            VR::IS,
            PrimitiveValue::from("1"),
        ));
        object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.1".into();
    }
    // Decode the selected frame only; a multi-frame file need not allocate an
    // entire study merely to show one representative image.
    let decoded = object.decode_pixel_data_frame(0)?;
    if decoded.rows() as u64 != rows
        || decoded.columns() as u64 != columns
        || decoded.samples_per_pixel() as u64 != samples
    {
        return Err("DICOM decoded dimensions do not match image metadata".into());
    }
    let options = ConvertOptions::new()
        .with_voi_lut(VoiLutOption::Default)
        .force_8bit();
    Ok(decoded.to_dynamic_image_with_options(0, &options)?)
}

fn rle_frame(
    fragment: &[u8],
    pixels: usize,
    samples: usize,
    bytes_per_sample: usize,
) -> Result<Vec<u8>> {
    if fragment.len() < 64 {
        return Err("truncated DICOM RLE header".into());
    }
    let number = u32::from_le_bytes(fragment[..4].try_into()?) as usize;
    if number != samples * bytes_per_sample || number > 15 {
        return Err("DICOM RLE segment count differs from pixel layout".into());
    }
    let mut offsets: Vec<usize> = fragment[4..4 + number * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b) as usize)
        .collect();
    offsets.push(fragment.len());
    if offsets[0] < 64 || offsets.windows(2).any(|p| p[0] >= p[1]) {
        return Err("invalid DICOM RLE segment offsets".into());
    }
    let mut raw = vec![0u8; pixels * samples * bytes_per_sample];
    for segment in 0..number {
        let data = fragment
            .get(offsets[segment]..offsets[segment + 1])
            .ok_or("truncated DICOM RLE segment")?;
        let mut decoded = Vec::new();
        let mut cursor = 0;
        while decoded.len() < pixels {
            let control = *data.get(cursor).ok_or("truncated DICOM RLE run")? as i8;
            cursor += 1;
            if control == -128 {
                continue;
            }
            let run = if control >= 0 {
                control as usize + 1
            } else {
                (1i16 - control as i16) as usize
            };
            if run > pixels - decoded.len() {
                return Err("DICOM RLE run exceeds its scalar plane".into());
            }
            if control >= 0 {
                let end = cursor.checked_add(run).ok_or("DICOM RLE run overflow")?;
                decoded.extend(
                    data.get(cursor..end)
                        .ok_or("truncated DICOM RLE literal run")?,
                );
                cursor = end;
            } else {
                let value = *data.get(cursor).ok_or("truncated DICOM RLE repeat run")?;
                cursor += 1;
                decoded.resize(decoded.len() + run, value);
            }
        }
        let component = segment / bytes_per_sample;
        let byte = bytes_per_sample - 1 - segment % bytes_per_sample;
        for (pixel, value) in decoded.into_iter().enumerate() {
            raw[(pixel * samples + component) * bytes_per_sample + byte] = value;
        }
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn element(out: &mut Vec<u8>, group: u16, tag: u16, vr: &[u8; 2], mut data: Vec<u8>) {
        if !data.len().is_multiple_of(2) {
            data.push(if vr == b"UI" { 0 } else { b' ' });
        }
        out.extend(group.to_le_bytes());
        out.extend(tag.to_le_bytes());
        out.extend(vr);
        if [b"OB", b"OW", b"SQ", b"UN", b"UT"].contains(&vr) {
            out.extend([0, 0]);
            out.extend((data.len() as u32).to_le_bytes());
        } else {
            out.extend((data.len() as u16).to_le_bytes());
        }
        out.extend(data);
    }
    fn fixture(rgb: bool, frames: u8, rle: bool) -> Vec<u8> {
        let mut meta = Vec::new();
        for (tag, vr, value) in [
            (1, b"OB", vec![0, 1]),
            (2, b"UI", b"1.2.840.10008.5.1.4.1.1.7".to_vec()),
            (3, b"UI", b"1.2.3.4.5".to_vec()),
            (
                0x10,
                b"UI",
                if rle {
                    b"1.2.840.10008.1.2.5".to_vec()
                } else {
                    b"1.2.840.10008.1.2.1".to_vec()
                },
            ),
            (0x12, b"UI", b"1.2.3.4".to_vec()),
        ] {
            element(&mut meta, 2, tag, vr, value);
        }
        let mut out = vec![0; 128];
        out.extend(b"DICM");
        element(
            &mut out,
            2,
            0,
            b"UL",
            (meta.len() as u32).to_le_bytes().to_vec(),
        );
        out.extend(meta);
        element(
            &mut out,
            8,
            0x16,
            b"UI",
            b"1.2.840.10008.5.1.4.1.1.7".to_vec(),
        );
        element(&mut out, 8, 0x18, b"UI", b"1.2.3.4.5".to_vec());
        element(
            &mut out,
            0x28,
            2,
            b"US",
            (if rgb { 3u16 } else { 1u16 }).to_le_bytes().to_vec(),
        );
        element(
            &mut out,
            0x28,
            4,
            b"CS",
            if rgb {
                b"RGB".to_vec()
            } else {
                b"MONOCHROME2".to_vec()
            },
        );
        if rgb {
            element(&mut out, 0x28, 6, b"US", 0u16.to_le_bytes().to_vec());
        }
        element(&mut out, 0x28, 8, b"IS", frames.to_string().into_bytes());
        for (tag, value) in [
            (0x10, 2u16),
            (0x11, 3),
            (0x100, 8),
            (0x101, 8),
            (0x102, 7),
            (0x103, 0),
        ] {
            element(&mut out, 0x28, tag, b"US", value.to_le_bytes().to_vec());
        }
        let pixels = if rgb {
            vec![
                255, 0, 0, 0, 255, 0, 0, 0, 255, 128, 128, 128, 0, 0, 0, 255, 255, 255,
            ]
        } else {
            vec![0, 32, 64, 128, 192, 255]
        };
        if rle {
            out.extend(0x7fe0u16.to_le_bytes());
            out.extend(0x10u16.to_le_bytes());
            out.extend(b"OB\0\0");
            out.extend(u32::MAX.to_le_bytes());
            out.extend(0xfffeu16.to_le_bytes());
            out.extend(0xe000u16.to_le_bytes());
            out.extend(4u32.to_le_bytes());
            out.extend(0u32.to_le_bytes());
            let mut encoded = 1u32.to_le_bytes().to_vec();
            encoded.extend(64u32.to_le_bytes());
            encoded.extend([0; 56]);
            encoded.push((pixels.len() - 1) as u8);
            encoded.extend(pixels);
            if !encoded.len().is_multiple_of(2) {
                encoded.push(0);
            }
            out.extend(0xfffeu16.to_le_bytes());
            out.extend(0xe000u16.to_le_bytes());
            out.extend((encoded.len() as u32).to_le_bytes());
            out.extend(encoded);
            out.extend(0xfffeu16.to_le_bytes());
            out.extend(0xe0ddu16.to_le_bytes());
            out.extend(0u32.to_le_bytes());
        } else {
            let mut all = pixels;
            for _ in 1..frames {
                all.extend(vec![255; if rgb { 18 } else { 6 }]);
            }
            element(&mut out, 0x7fe0, 0x10, b"OB", all);
        }
        out
    }
    #[test]
    fn native_rle_and_first_frame_pixels() {
        for rle in [false, true] {
            let image = decode(&fixture(false, 1, rle)).unwrap().to_luma8();
            assert_eq!(image.dimensions(), (3, 2));
            assert_eq!(image.as_raw(), &vec![0, 32, 64, 128, 192, 255]);
        }
        let image = decode(&fixture(false, 2, false)).unwrap().to_luma8();
        assert_eq!(image.as_raw(), &vec![0, 32, 64, 128, 192, 255]);
        let image = decode(&fixture(true, 1, false)).unwrap().to_rgb8();
        assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0]);
        assert_eq!(image.get_pixel(1, 0).0, [0, 255, 0]);
    }
    #[test]
    fn truncation_and_missing_image_data_fail() {
        assert!(decode(b"not dicom").is_err());
        let mut bytes = fixture(false, 1, false);
        bytes.truncate(bytes.len() - 4);
        assert!(decode(&bytes).is_err());
        let mut bytes = fixture(false, 1, true);
        bytes.truncate(bytes.len() - 30);
        assert!(decode(&bytes).is_err());
    }
}
