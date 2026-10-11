//! LEPCC v1 decoding from Esri's published byte-stream specification.
//! Algorithms checked against https://github.com/Esri/lepcc (Apache-2.0).
use crate::scene::Vec3;
use crate::{Error, Result};
use std::collections::HashMap;
const MAX_POINTS: usize = 5_000_000;
fn error(message: impl Into<String>) -> Error {
    message.into().into()
}
struct Bytes<'a> {
    data: &'a [u8],
    offset: usize,
}
impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(n)
            .ok_or_else(|| error("LEPCC offset overflow"))?;
        let value = self
            .data
            .get(self.offset..end)
            .ok_or_else(|| error("truncated LEPCC record"))?;
        self.offset = end;
        Ok(value)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
    fn f64(&mut self) -> Result<f64> {
        let n = f64::from_le_bytes(self.take(8)?.try_into()?);
        if !n.is_finite() {
            return Err(error("non-finite LEPCC header"));
        }
        Ok(n)
    }
    fn vector(&mut self) -> Result<Vec3> {
        Ok(Vec3::new(self.f64()?, self.f64()?, self.f64()?))
    }
    fn done(&self) -> Result<()> {
        if self.offset != self.data.len() {
            return Err(error("trailing bytes after LEPCC payload"));
        }
        Ok(())
    }
}
fn checksum(data: &[u8]) -> u32 {
    let mut one = 0xffff_u32;
    let mut two = 0xffff_u32;
    for block in data.chunks(718) {
        for word in block.chunks(2) {
            one += ((word[0] as u32) << 8) + word.get(1).copied().unwrap_or(0) as u32;
            two += one;
        }
        one = (one & 0xffff) + (one >> 16);
        two = (two & 0xffff) + (two >> 16);
    }
    one = (one & 0xffff) + (one >> 16);
    two = (two & 0xffff) + (two >> 16);
    (two << 16) | one
}
fn header<'a>(data: &'a [u8], signature: &[u8; 10]) -> Result<Bytes<'a>> {
    let mut reader = Bytes { data, offset: 0 };
    if reader.take(10)? != signature || reader.u16()? != 1 {
        return Err(error("unsupported LEPCC signature/version"));
    }
    let expected = reader.u32()?;
    let size = reader.u64()?;
    if size != data.len() as u64 {
        return Err(error("LEPCC blob size differs from its data"));
    }
    if checksum(&data[16..]) != expected {
        return Err(error("LEPCC Fletcher32 checksum mismatch"));
    }
    Ok(reader)
}
fn packed(reader: &mut Bytes<'_>, count: usize, bits: usize) -> Result<Vec<u32>> {
    if count > MAX_POINTS || bits > 32 {
        return Err(error("LEPCC packed-array limit exceeded"));
    }
    let bit_count = count
        .checked_mul(bits)
        .ok_or_else(|| error("LEPCC packed count overflow"))?;
    let bytes = reader.take(bit_count.div_ceil(8))?;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let mut value = 0_u32;
        for bit in 0..bits {
            let position = index * bits + bit;
            value |= (((bytes[position / 8] >> (position % 8)) & 1) as u32) << bit;
        }
        values.push(value);
    }
    Ok(values)
}
fn bitstuff(reader: &mut Bytes<'_>) -> Result<Vec<u32>> {
    let flags = reader.u8()?;
    let width = match flags >> 6 {
        0 => 4,
        n => 3 - n as usize,
    };
    if width == 0 {
        return Err(error("invalid LEPCC count width"));
    }
    let mut count_bytes = [0; 4];
    count_bytes[..width].copy_from_slice(reader.take(width)?);
    let count = u32::from_le_bytes(count_bytes) as usize;
    let bits = (flags & 31) as usize;
    if flags & 32 == 0 {
        return packed(reader, count, bits);
    }
    let n = reader
        .u8()?
        .checked_sub(1)
        .ok_or_else(|| error("invalid LEPCC lookup size"))? as usize;
    let mut table = vec![0];
    table.extend(packed(reader, n, bits)?);
    let index_bits = (usize::BITS - n.leading_zeros()) as usize;
    packed(reader, count, index_bits)?
        .into_iter()
        .map(|i| {
            table
                .get(i as usize)
                .copied()
                .ok_or_else(|| error("LEPCC lookup index out of bounds"))
        })
        .collect()
}
fn segments(reader: &mut Bytes<'_>) -> Result<Vec<u32>> {
    let minimums = bitstuff(reader)?;
    let mut data = Vec::new();
    for minimum in minimums {
        let values = bitstuff(reader)?;
        if data
            .len()
            .checked_add(values.len())
            .is_none_or(|n| n > MAX_POINTS)
        {
            return Err(error("LEPCC segmented-array limit exceeded"));
        }
        for value in values {
            data.push(
                value
                    .checked_add(minimum)
                    .ok_or_else(|| error("LEPCC segment value overflow"))?,
            );
        }
    }
    Ok(data)
}
pub(super) fn xyz(data: &[u8]) -> Result<Vec<Vec3>> {
    let mut reader = header(data, b"LEPCC     ")?;
    let minimum = reader.vector()?;
    let maximum = reader.vector()?;
    let error_bound = reader.vector()?;
    let count = reader.u32()? as usize;
    reader.u32()?;
    if count == 0
        || count > MAX_POINTS
        || minimum.x > maximum.x
        || minimum.y > maximum.y
        || minimum.z > maximum.z
        || error_bound.x <= 0.
        || error_bound.y <= 0.
        || error_bound.z <= 0.
    {
        return Err(error("invalid LEPCC point count, extent or error bounds"));
    }
    let ys = segments(&mut reader)?;
    let rows = segments(&mut reader)?;
    let xs = segments(&mut reader)?;
    let zs = segments(&mut reader)?;
    if ys.len() != rows.len()
        || xs.len() != count
        || zs.len() != count
        || rows.iter().map(|&n| n as u64).sum::<u64>() != count as u64
    {
        return Err(error("LEPCC row/coordinate counts differ from header"));
    }
    reader.done()?;
    let mut points = Vec::with_capacity(count);
    let mut y = 0_u64;
    let mut index = 0;
    for (dy, n) in ys.into_iter().zip(rows) {
        y = y
            .checked_add(dy as u64)
            .ok_or_else(|| error("LEPCC Y delta overflow"))?;
        let mut x = 0_u64;
        for _ in 0..n {
            x = x
                .checked_add(xs[index] as u64)
                .ok_or_else(|| error("LEPCC X delta overflow"))?;
            let point = Vec3::new(
                (minimum.x + x as f64 * 2. * error_bound.x).min(maximum.x),
                (minimum.y + y as f64 * 2. * error_bound.y).min(maximum.y),
                (minimum.z + zs[index] as f64 * 2. * error_bound.z).min(maximum.z),
            );
            if !point.is_finite() {
                return Err(error("LEPCC reconstructed point overflow"));
            }
            points.push(point);
            index += 1;
        }
    }
    Ok(points)
}
struct WordBits<'a> {
    data: &'a [u8],
    position: usize,
}
impl WordBits<'_> {
    fn read(&mut self, n: usize) -> Result<u32> {
        if n > 32 {
            return Err(error("LEPCC Huffman code exceeds 32 bits"));
        }
        let mut value = 0;
        for _ in 0..n {
            let index = self.position / 32 * 4;
            let bit = self.position % 32;
            let word = u32::from_le_bytes(
                self.data
                    .get(index..index + 4)
                    .ok_or_else(|| error("truncated LEPCC Huffman word"))?
                    .try_into()?,
            );
            value = (value << 1) | ((word >> (31 - bit)) & 1);
            self.position += 1;
        }
        Ok(value)
    }
    fn consumed(&self) -> usize {
        self.position.div_ceil(32) * 4
    }
}
fn huffman(reader: &mut Bytes<'_>, count: usize) -> Result<Vec<usize>> {
    let version = reader.u32()?;
    let size = reader.u32()? as usize;
    let first = reader.u32()? as usize;
    let last = reader.u32()? as usize;
    if version < 2 || size == 0 || size > 256 || first >= last || last > size * 2 || first >= size {
        return Err(error("invalid LEPCC Huffman table header"));
    }
    let lengths = bitstuff(reader)?;
    if lengths.len() != last - first {
        return Err(error("LEPCC Huffman code-length count mismatch"));
    }
    let mut bits = WordBits {
        data: &reader.data[reader.offset..],
        position: 0,
    };
    let mut codes = HashMap::new();
    let mut max_length = 0;
    for (index, length) in (first..last).zip(lengths) {
        if length == 0 {
            continue;
        }
        let length = length as usize;
        let code = bits.read(length)?;
        if codes.insert((length, code), index % size).is_some() {
            return Err(error("duplicate LEPCC Huffman code"));
        }
        max_length = max_length.max(length);
    }
    for &(length, code) in codes.keys() {
        for prefix in 1..length {
            if codes.contains_key(&(prefix, code >> (length - prefix))) {
                return Err(error("LEPCC Huffman prefix conflict"));
            }
        }
    }
    let table_size = bits.consumed();
    reader.take(table_size)?;
    let mut bits = WordBits {
        data: &reader.data[reader.offset..],
        position: 0,
    };
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        let mut code = 0;
        let mut found = None;
        for length in 1..=max_length {
            code = (code << 1) | bits.read(1)?;
            if let Some(&symbol) = codes.get(&(length, code)) {
                found = Some(symbol);
                break;
            }
        }
        values.push(found.ok_or_else(|| error("invalid LEPCC Huffman symbol"))?);
    }
    let stream_size = bits
        .consumed()
        .checked_add(4)
        .ok_or_else(|| error("LEPCC Huffman stream overflow"))?;
    reader.take(stream_size)?;
    Ok(values)
}
pub(super) fn rgb(data: &[u8], expected: usize) -> Result<Vec<[u8; 3]>> {
    let mut reader = header(data, b"ClusterRGB")?;
    let count = reader.u32()? as usize;
    let palette_size = reader.u16()? as usize;
    let lookup = reader.u8()?;
    let method = reader.u8()?;
    if count != expected
        || count > MAX_POINTS
        || palette_size > 256
        || lookup > 2
        || (palette_size == 0 && (lookup != 0 || method != 0))
        || (palette_size > 0 && lookup == 0)
    {
        return Err(error("LEPCC RGB count/palette is invalid"));
    }
    let mut colors = Vec::with_capacity(count);
    if palette_size == 0 {
        for bytes in reader.take(count * 3)?.chunks_exact(3) {
            colors.push(bytes.try_into()?);
        }
    } else {
        let palette: Vec<[u8; 3]> = reader
            .take(palette_size * 3)?
            .chunks_exact(3)
            .map(|v| v.try_into().map_err(Into::into))
            .collect::<Result<_>>()?;
        let indices = match method {
            0 => reader.take(count)?.iter().map(|&n| n as usize).collect(),
            1 if palette_size == 1 => vec![0; count],
            2 => huffman(&mut reader, count)?,
            _ => return Err(error("unsupported LEPCC RGB palette compression")),
        };
        for index in indices {
            colors.push(
                *palette
                    .get(index)
                    .ok_or_else(|| error("LEPCC RGB palette index out of bounds"))?,
            );
        }
    }
    reader.done()?;
    Ok(colors)
}

#[cfg(test)]
mod tests {
    use super::*;
    const XYZ: &[u8] = include_bytes!("../../tests/fixtures/slpk/autzen.xyz");
    const RAW: &[u8] = include_bytes!("../../tests/fixtures/slpk/rgb-raw.bin");
    const CONSTANT: &[u8] = include_bytes!("../../tests/fixtures/slpk/rgb-constant.bin");
    const HUFFMAN: &[u8] = include_bytes!("../../tests/fixtures/slpk/rgb-huffman.bin");

    #[test]
    fn esri_xyz_reference_matches_error_bounds() {
        let points = xyz(XYZ).unwrap();
        let reference = include_bytes!("../../tests/fixtures/slpk/autzen-reference.bin");
        let count = u32::from_le_bytes(reference[..4].try_into().unwrap()) as usize;
        assert_eq!(points.len(), count);
        // The reference encoder sorts by row then X; compare to original unsorted
        // double precision points with the published quantization error bounds.
        let expected: Vec<Vec3> = reference[4..4 + count * 24]
            .chunks_exact(24)
            .map(|p| {
                Vec3::new(
                    f64::from_le_bytes(p[..8].try_into().unwrap()),
                    f64::from_le_bytes(p[8..16].try_into().unwrap()),
                    f64::from_le_bytes(p[16..24].try_into().unwrap()),
                )
            })
            .collect();
        let bounds = Vec3::new(
            f64::from_le_bytes(XYZ[72..80].try_into().unwrap()),
            f64::from_le_bytes(XYZ[80..88].try_into().unwrap()),
            f64::from_le_bytes(XYZ[88..96].try_into().unwrap()),
        );
        for p in points {
            assert!(
                expected.iter().any(|q| {
                    (p.x - q.x).abs() <= bounds.x * 1.00001
                        && (p.y - q.y).abs() <= bounds.y * 1.00001
                        && (p.z - q.z).abs() <= bounds.z * 1.00001
                }),
                "no reference coordinate matches {p:?}"
            );
        }
    }

    #[test]
    fn esri_raw_constant_and_huffman_color_encodings() {
        let expected: Vec<_> = (0..17_u8).map(|i| [i * 13, i * 7, i * 9]).collect();
        assert_eq!(rgb(RAW, 17).unwrap(), expected);
        assert_eq!(rgb(CONSTANT, 1024).unwrap(), vec![[30, 100, 200]; 1024]);
        let expected: Vec<_> = (0..1024)
            .map(|i| {
                if i % 17 == 0 {
                    [255, 0, 0]
                } else if i % 29 == 0 {
                    [0, 255, 0]
                } else {
                    [0, 0, 255]
                }
            })
            .collect();
        assert_eq!(rgb(HUFFMAN, 1024).unwrap(), expected);
    }

    #[test]
    fn corrupt_and_truncated_streams_fail() {
        for stream in [XYZ, RAW, CONSTANT, HUFFMAN] {
            for length in [0, 10, 20, stream.len() - 1] {
                assert!(xyz(&stream[..length]).is_err());
                assert!(rgb(&stream[..length], 17).is_err());
            }
            let mut bad = stream.to_vec();
            *bad.last_mut().unwrap() ^= 1;
            assert!(xyz(&bad).is_err());
            assert!(rgb(&bad, 17).is_err());
        }
        assert!(rgb(RAW, 16).is_err());
    }
}
