use super::{MAX_DECODED, Result, bytes, check_time, dimensions, u16_le, u32_le};
use std::{io::Cursor, time::Instant};

pub(super) fn normalize(data: &[u8], bmp: bool, started: Instant) -> Result<Option<Vec<u8>>> {
    let base = if bmp { 14 } else { 0 };
    if bmp && u32_le(data, 2)? as usize != data.len() {
        return Err("invalid Inventor BMP size".into());
    }
    let header_size = u32_le(data, base)? as usize;
    if ![40, 108, 124].contains(&header_size) {
        return Ok(None);
    }
    let header = bytes(data, base, header_size)?;
    let signed_width = i32::from_le_bytes(bytes(header, 4, 4)?.try_into()?);
    let signed_height = i32::from_le_bytes(bytes(header, 8, 4)?.try_into()?);
    if signed_width <= 0 {
        return Err("invalid Inventor bitmap width".into());
    }
    let width = signed_width as usize;
    let height = signed_height.unsigned_abs() as usize;
    dimensions(width, height)?;
    let depth = usize::from(u16_le(header, 14)?);
    let compression = u32_le(header, 16)?;
    if u16_le(header, 12)? != 1
        || ![1, 4, 8, 16, 24, 32].contains(&depth)
        || ![0, 3, 6].contains(&compression)
        || (compression != 0 && ![16, 32].contains(&depth))
    {
        return Ok(None);
    }
    let mut pixel_offset = base + header_size;
    let mut masks = Vec::new();
    if compression != 0 {
        let count = if compression == 6 { 4 } else { 3 };
        let values = if header_size == 40 {
            let values = bytes(data, pixel_offset, count * 4)?;
            pixel_offset += count * 4;
            values
        } else {
            bytes(header, 40, 16)?
        };
        for mask in values.as_chunks::<4>().0.iter() {
            masks.push(u32::from_le_bytes(*mask));
        }
    } else if depth == 16 {
        masks = vec![0x7c00, 0x03e0, 0x001f];
    } else if depth == 32 {
        masks = vec![0x00ff0000, 0x0000ff00, 0x000000ff];
    }
    let channels = mask_channels(masks, depth)?;
    let mut palette = Vec::new();
    let mut colors = u32_le(header, 32)? as usize;
    if depth <= 8 {
        if colors == 0 {
            colors = 1 << depth;
        }
        if colors == 0 || colors > 1 << depth {
            return Err("invalid Inventor bitmap palette size".into());
        }
        for color in bytes(data, pixel_offset, colors * 4)?
            .as_chunks::<4>()
            .0
            .iter()
        {
            palette.push([color[2], color[1], color[0], 255]);
        }
    } else if colors > data.len().saturating_sub(pixel_offset) / 4 {
        return Err("invalid Inventor bitmap color table".into());
    }
    pixel_offset += colors * 4;
    if bmp {
        let declared = u32_le(data, 10)? as usize;
        if declared < pixel_offset {
            return Err("invalid Inventor BMP pixel offset".into());
        }
        pixel_offset = declared;
    }
    let stride = (width * depth).div_ceil(32) * 4;
    bytes(data, pixel_offset, stride * height)?;
    if (width * 4 + 1) * height > MAX_DECODED {
        return Err("Inventor decoded bitmap exceeds the 64 MiB limit".into());
    }
    let mut image = image::RgbaImage::new(width as u32, height as u32);
    for y in 0..height {
        check_time(started)?;
        let source_y = if signed_height > 0 { height - y - 1 } else { y };
        let row = bytes(data, pixel_offset + source_y * stride, stride)?;
        for x in 0..width {
            if x & 255 == 0 {
                check_time(started)?;
            }
            let rgba = if depth <= 8 {
                let bit = x * depth;
                let index = (row[bit / 8] >> (8 - depth - bit % 8)) & ((1_u16 << depth) - 1) as u8;
                *palette
                    .get(usize::from(index))
                    .ok_or("invalid Inventor bitmap palette index")?
            } else if depth == 24 {
                [row[x * 3 + 2], row[x * 3 + 1], row[x * 3], 255]
            } else {
                let pixel = if depth == 16 {
                    u32::from(u16_le(row, x * 2)?)
                } else {
                    u32_le(row, x * 4)?
                };
                let mut rgba = [255; 4];
                for (index, &(mask, shift, maximum)) in channels.iter().enumerate() {
                    if mask != 0 {
                        rgba[index] =
                            (((u64::from(pixel & mask) >> shift) * 255) / u64::from(maximum)) as u8;
                    }
                }
                rgba
            };
            image.put_pixel(x as u32, y as u32, image::Rgba(rgba));
        }
    }
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, image::ImageFormat::Png)?;
    check_time(started)?;
    Ok(Some(output.into_inner()))
}

fn mask_channels(mut masks: Vec<u32>, depth: usize) -> Result<Vec<(u32, u32, u32)>> {
    if masks.is_empty() {
        return Ok(Vec::new());
    }
    if masks.len() == 3 {
        masks.push(0);
    }
    let mut used = 0;
    let mut output = Vec::new();
    for (index, mask) in masks.into_iter().enumerate() {
        if mask == 0 {
            if index != 3 {
                return Err("invalid Inventor bitmap color mask".into());
            }
            output.push((0, 0, 0));
            continue;
        }
        let shift = mask.trailing_zeros();
        let maximum = mask >> shift;
        if (depth < 32 && mask >> depth != 0)
            || mask & used != 0
            || maximum & maximum.wrapping_add(1) != 0
        {
            return Err("invalid Inventor bitmap color mask".into());
        }
        used |= mask;
        output.push((mask, shift, maximum));
    }
    Ok(output)
}
