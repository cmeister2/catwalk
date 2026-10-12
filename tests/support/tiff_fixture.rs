//! Small authored TIFF carriers built independently of the production decoder.

use catwalk3d::Result;

/// Storage choices for the authored classic and `BigTIFF` controls.
#[derive(Clone, Copy, Default)]
pub struct Layout {
    /// Use 64-bit `BigTIFF` directory offsets and entry counts.
    pub big: bool,
    /// Store all numeric fields and scalar payloads in big-endian byte order.
    pub big_endian: bool,
}

/// One explicitly authored uncompressed grayscale page.
#[derive(Clone)]
pub struct Frame {
    /// Stored column count, before the orientation mapping.
    pub width: u32,
    /// Stored row count, before the orientation mapping.
    pub height: u32,
    /// Scalar bit width.
    pub bits: u16,
    /// TIFF sample format: unsigned 1, signed 2 or IEEE floating point 3.
    pub sample_format: u16,
    /// Exact scalar payload, already in the selected file byte order.
    pub data: Vec<u8>,
    /// TIFF orientation in the inclusive range 1–8.
    pub orientation: u16,
    /// Select `WhiteIsZero` instead of `BlackIsZero`.
    pub white_is_zero: bool,
    /// Full-resolution image 0, page 2, reduced image 1 or transparency mask 4.
    pub subfile_type: u32,
    /// Unitless 1, inches 2 or centimetres 3.
    pub resolution_unit: u16,
    /// Unsigned rational horizontal samples per resolution unit.
    pub x_resolution: [u32; 2],
    /// Unsigned rational vertical samples per resolution unit.
    pub y_resolution: [u32; 2],
}

impl Frame {
    /// Construct one default top-left, unit-spaced unsigned byte page.
    pub fn gray8(width: u32, height: u32, data: Vec<u8>) -> Self {
        Self {
            width,
            height,
            bits: 8,
            sample_format: 1,
            data,
            orientation: 1,
            white_is_zero: false,
            subfile_type: 0,
            resolution_unit: 1,
            x_resolution: [1, 1],
            y_resolution: [1, 1],
        }
    }
}

/// Write an explicitly sized integer without depending on TIFF codec helpers.
///
/// # Errors
/// Returns an error if the integer cannot fit the requested field width.
fn integer(bytes: &mut Vec<u8>, value: u64, width: usize, big_endian: bool) -> Result<()> {
    if width < 8 && value >= (1_u64 << (width * 8)) {
        return Err("authored TIFF integer exceeds its field width".into());
    }
    if big_endian {
        bytes.extend_from_slice(&value.to_be_bytes()[8 - width..]);
    } else {
        bytes.extend_from_slice(&value.to_le_bytes()[..width]);
    }
    Ok(())
}

/// Encode one single-value directory entry, including correct inline padding.
///
/// # Errors
/// Returns an error when a value does not fit its authored field representation.
fn entry(
    output: &mut Vec<u8>,
    layout: Layout,
    tag: u16,
    ty: u16,
    count: u64,
    value: &[u8],
) -> Result<()> {
    let inline = if layout.big { 8 } else { 4 };
    integer(output, u64::from(tag), 2, layout.big_endian)?;
    integer(output, u64::from(ty), 2, layout.big_endian)?;
    integer(output, count, inline, layout.big_endian)?;
    if value.len() > inline {
        return Err("authored TIFF entry does not fit its inline field".into());
    }
    output.extend_from_slice(value);
    output.resize(output.len() + inline - value.len(), 0);
    Ok(())
}

/// Construct a complete linked page chain with exact source-addressed strips.
///
/// # Errors
/// Returns an error for an empty chain, excessive authored sizes, integer overflow,
/// mismatched sample payload lengths, or fields that cannot fit the selected layout.
pub fn encode(frames: &[Frame], layout: Layout) -> Result<Vec<u8>> {
    if frames.is_empty() || frames.len() > 4096 {
        return Err("authored TIFF needs between one and 4096 pages".into());
    }
    let header = if layout.big { 16_usize } else { 8 };
    let count_width = if layout.big { 8 } else { 2 };
    let offset_width = if layout.big { 8 } else { 4 };
    let entry_width = if layout.big { 20 } else { 12 };
    let directory_size = count_width + 15 * entry_width + offset_width;
    let directory_end = header + frames.len() * directory_size;
    let rational_bytes = if layout.big { 0 } else { frames.len() * 16 };
    let mut payload_offset = directory_end + rational_bytes;
    let mut output = if layout.big_endian {
        b"MM".to_vec()
    } else {
        b"II".to_vec()
    };
    integer(
        &mut output,
        if layout.big { 43 } else { 42 },
        2,
        layout.big_endian,
    )?;
    if layout.big {
        integer(&mut output, 8, 2, layout.big_endian)?;
        integer(&mut output, 0, 2, layout.big_endian)?;
    }
    integer(&mut output, header as u64, offset_width, layout.big_endian)?;
    for (index, frame) in frames.iter().enumerate() {
        let expected = u64::from(frame.width)
            .checked_mul(u64::from(frame.height))
            .and_then(|count| count.checked_mul(u64::from(frame.bits / 8)))
            .ok_or("authored TIFF sample size overflow")?;
        if frame.bits == 0 || frame.bits % 8 != 0 || expected != frame.data.len() as u64 {
            return Err("authored TIFF sample payload length disagrees with dimensions".into());
        }
        integer(&mut output, 15, count_width, layout.big_endian)?;
        let offset_type = if layout.big { 16 } else { 4 };
        let fields = [
            (254, 4, u64::from(frame.subfile_type), 4),
            (256, 4, u64::from(frame.width), 4),
            (257, 4, u64::from(frame.height), 4),
            (258, 3, u64::from(frame.bits), 2),
            (259, 3, 1, 2),
            (262, 3, u64::from(!frame.white_is_zero), 2),
            (273, offset_type, payload_offset as u64, offset_width),
            (274, 3, u64::from(frame.orientation), 2),
            (277, 3, 1, 2),
            (278, 4, u64::from(frame.height), 4),
            (279, offset_type, frame.data.len() as u64, offset_width),
        ];
        for (tag, ty, value, width) in fields {
            let mut encoded = Vec::new();
            integer(&mut encoded, value, width, layout.big_endian)?;
            entry(&mut output, layout, tag, ty, 1, &encoded)?;
        }
        resolution_entries(&mut output, frame, layout, index, directory_end)?;
        for (tag, value) in [(296, frame.resolution_unit), (339, frame.sample_format)] {
            let mut encoded = Vec::new();
            integer(&mut encoded, u64::from(value), 2, layout.big_endian)?;
            entry(&mut output, layout, tag, 3, 1, &encoded)?;
        }
        let next = if index + 1 == frames.len() {
            0
        } else {
            header + (index + 1) * directory_size
        };
        integer(&mut output, next as u64, offset_width, layout.big_endian)?;
        payload_offset = payload_offset
            .checked_add(frame.data.len())
            .ok_or("authored TIFF payload overflow")?;
    }
    if !layout.big {
        for frame in frames {
            for word in [frame.x_resolution, frame.y_resolution]
                .into_iter()
                .flatten()
            {
                integer(&mut output, u64::from(word), 4, layout.big_endian)?;
            }
        }
    }
    for frame in frames {
        output.extend_from_slice(&frame.data);
    }
    Ok(output)
}

/// Store inline `BigTIFF` rationals or classic pointers to their exact payloads.
///
/// # Errors
/// Returns an error when an authored pointer or value cannot fit its TIFF field.
fn resolution_entries(
    output: &mut Vec<u8>,
    frame: &Frame,
    layout: Layout,
    index: usize,
    directory_end: usize,
) -> Result<()> {
    for (axis, rational) in [frame.x_resolution, frame.y_resolution]
        .into_iter()
        .enumerate()
    {
        let mut encoded = Vec::new();
        if layout.big {
            for word in rational {
                integer(&mut encoded, u64::from(word), 4, layout.big_endian)?;
            }
        } else {
            integer(
                &mut encoded,
                (directory_end + index * 16 + axis * 8) as u64,
                4,
                layout.big_endian,
            )?;
        }
        entry(
            output,
            layout,
            if axis == 0 { 282 } else { 283 },
            5,
            1,
            &encoded,
        )?;
    }
    Ok(())
}
