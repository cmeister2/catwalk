//! Survey point layouts: BPF, TerraScan, NASA QFIT, SBET and Optech CSD.
//! Coordinates follow each stored/PDAL axis convention; no CRS reprojection.
use crate::{
    Result,
    scene::{Point, Scene, Vec3},
};
use std::{io::Read, path::Path};
/// Maximum declared point count accepted before geometry allocation.
const MAX_POINTS: usize = 20_000_000;
/// Maximum sampled points retained in the preview scene.
const PREVIEW_POINTS: usize = 1_000_000;

/// Read a bounded survey point or trajectory file and decode its preview geometry.
///
/// # Errors
/// Returns an error for unsupported survey versions/layouts, invalid counts/records, nonfinite
/// positions or malformed compressed payloads.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    let scene = match super::extension(path)?.as_str() {
        "bpf" => bpf(&bytes)?,
        "bin" => terra(&bytes)?,
        "qi" => qfit(&bytes)?,
        "sbet" => sbet(&bytes)?,
        "csd" => csd(&bytes)?,
        _ => return Err("unsupported survey point format".into()),
    };
    scene.validate()?;
    Ok(scene)
}

/// Bounds-checked cursor for survey point or trajectory file records.
struct Reader<'a> {
    /// Borrowed complete input byte stream.
    bytes: &'a [u8],
    /// Byte position of the next unread record.
    position: usize,
    /// Whether stored words use big-endian byte order.
    big: bool,
}
impl<'a> Reader<'a> {
    /// Start a bounds-checked reader at the beginning of the supplied bytes.
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            big: false,
        }
    }
    /// Consume the requested bytes, rejecting offset overflow and truncated records.
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(count)
            .ok_or("survey offset overflow")?;
        let data = self
            .bytes
            .get(self.position..end)
            .ok_or("truncated survey point data")?;
        self.position = end;
        Ok(data)
    }
    /// Read one unsigned byte from the current record.
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    /// Read one unsigned 16-bit word from the current record.
    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?.try_into()?;
        Ok(if self.big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    }
    /// Read one unsigned 32-bit word from the current record.
    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?.try_into()?;
        Ok(if self.big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    }
    /// Read one signed 32-bit word from the current record.
    #[allow(
        clippy::cast_possible_wrap,
        reason = "Reinterpret the selected-endian u32 word as the corresponding signed two-complement record value."
    )]
    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    /// Read one finite single-precision number and widen it for geometry calculations.
    fn f32(&mut self) -> Result<f64> {
        Ok(f64::from(f32::from_bits(self.u32()?)))
    }
    /// Read one finite double-precision number from the current record.
    fn f64(&mut self) -> Result<f64> {
        let b = self.take(8)?.try_into()?;
        Ok(if self.big {
            f64::from_be_bytes(b)
        } else {
            f64::from_le_bytes(b)
        })
    }
}

/// Choose the sampling interval that keeps a preview within one million points.
fn stride(count: usize) -> Result<usize> {
    if count == 0 || count > MAX_POINTS {
        return Err("survey point count must be between 1 and 20 million".into());
    }
    Ok(count.div_ceil(PREVIEW_POINTS))
}
/// Append a selected finite survey point without exceeding the preview budget.
fn append(scene: &mut Scene, position: Vec3, color: Option<[u8; 3]>, selected: bool) -> Result<()> {
    if !position.is_finite() {
        return Err("survey point coordinates are not finite".into());
    }
    if selected {
        scene.points.push(Point { position, color });
    }
    Ok(())
}

/// Decode `TerraScan` versions and their optional per-point color/classification records.
fn terra(bytes: &[u8]) -> Result<Scene> {
    let mut input = Reader::new(bytes);
    let header = input.u32()? as usize;
    let version = input.u32()?;
    if input.u32()? != 970_401 || input.take(4)? != b"CXYZ" {
        return Err("invalid TerraScan signature".into());
    }
    let count = input.u32()? as usize;
    let step = stride(count)?;
    let units = input.i32()?;
    let origin = [input.f64()?, input.f64()?, input.f64()?];
    let time = input.u32()?;
    let color = input.u32()?;
    if units <= 0
        || origin.iter().any(|v| !v.is_finite())
        || time > 1
        || color > 1
        || header < 56
        || header > bytes.len()
    {
        return Err("invalid TerraScan header".into());
    }
    let base = match version {
        20_010_712 => 16,
        20_020_715 => 20,
        _ => return Err("unsupported TerraScan version (expected 20010712 or 20020715)".into()),
    };
    let record = base + time as usize * 4 + color as usize * 4;
    if header + count * record != bytes.len() {
        return Err("TerraScan count and record length do not match the file".into());
    }
    input.position = header;
    let mut scene = Scene::default();
    for index in 0..count {
        if version == 20_010_712 {
            input.take(4)?;
        }
        let raw = [input.i32()?, input.i32()?, input.i32()?];
        if version == 20_020_715 {
            input.take(8)?;
        }
        if time != 0 {
            input.take(4)?;
        }
        let rgb = if color != 0 {
            let c = input.take(4)?;
            Some([c[0], c[1], c[2]])
        } else {
            None
        };
        let v = std::array::from_fn::<_, 3, _>(|axis| {
            (f64::from(raw[axis]) - origin[axis]) / f64::from(units)
        });
        append(
            &mut scene,
            Vec3::new(v[0], v[1], v[2]),
            rgb,
            index.is_multiple_of(step),
        )?;
    }
    Ok(scene)
}

/// Decode endian-tagged ATM QFIT records into georeferenced point positions.
fn qfit(bytes: &[u8]) -> Result<Scene> {
    let mut input = Reader::new(bytes);
    let little = input.u32()? as usize;
    let size = if [40, 48, 56].contains(&little) {
        little
    } else {
        input.position = 0;
        input.big = true;
        let big = input.u32()? as usize;
        if ![40, 48, 56].contains(&big) {
            return Err("unsupported QFIT record length".into());
        }
        big
    };
    input.position = size;
    let marker = input.i32()?;
    let declared = input.u32()? as usize;
    // Documented second-record history marker and byte offset; headerless
    // variants may instead have their first nonnegative shot at this record.
    let offset = if (-9_000_008..=-9_000_000).contains(&marker) {
        if declared < size * 2 || declared > bytes.len() || !declared.is_multiple_of(size) {
            return Err("invalid QFIT point-data offset".into());
        }
        declared
    } else if marker >= 0 {
        size
    } else {
        return Err("invalid QFIT header record".into());
    };
    if !(bytes.len() - offset).is_multiple_of(size) {
        return Err("truncated QFIT record".into());
    }
    let count = (bytes.len() - offset) / size;
    let step = stride(count)?;
    input.position = offset;
    let mut scene = Scene::default();
    for index in 0..count {
        if input.i32()? < 0 {
            return Err("unexpected history record inside QFIT point data".into());
        }
        let latitude = f64::from(input.i32()?) / 1_000_000.;
        let longitude = f64::from(input.i32()?) / 1_000_000.;
        let elevation = f64::from(input.i32()?) / 1_000.;
        input.take(size - 16)?;
        if latitude.abs() > 90. || !(-180.0..=360.0).contains(&longitude) {
            return Err("invalid QFIT latitude or longitude".into());
        }
        append(
            &mut scene,
            Vec3::new(longitude, latitude, elevation),
            None,
            index.is_multiple_of(step),
        )?;
    }
    Ok(scene)
}

/// Decode fixed-size SBET navigation records as longitude, latitude and altitude points.
fn sbet(bytes: &[u8]) -> Result<Scene> {
    const RECORD: usize = 17 * 8;
    if !bytes.len().is_multiple_of(RECORD) {
        return Err("SBET requires complete 17-double records".into());
    }
    let count = bytes.len() / RECORD;
    let step = stride(count)?;
    let mut input = Reader::new(bytes);
    let mut scene = Scene::default();
    for index in 0..count {
        let values: Vec<f64> = (0..17).map(|_| input.f64()).collect::<Result<_>>()?;
        if values.iter().any(|v| !v.is_finite())
            || values[1].abs() > std::f64::consts::FRAC_PI_2
            || values[2].abs() > std::f64::consts::TAU
        {
            return Err("invalid SBET trajectory record".into());
        }
        append(
            &mut scene,
            Vec3::new(values[2].to_degrees(), values[1].to_degrees(), values[3]),
            None,
            index.is_multiple_of(step),
        )?;
    }
    Ok(scene)
}

/// Apply the survey boresight rotations to a measured direction vector.
fn rotate(v: Vec3, angles: [f64; 3]) -> Vec3 {
    // Optech orientation is Rz(-heading) * Rx(pitch) * Ry(roll).
    let [roll, pitch, heading] = angles;
    let (s, c) = roll.sin_cos();
    let v = Vec3::new(c * v.x + s * v.z, v.y, -s * v.x + c * v.z);
    let (s, c) = pitch.sin_cos();
    let v = Vec3::new(v.x, c * v.y - s * v.z, s * v.y + c * v.z);
    let (s, c) = heading.sin_cos();
    Vec3::new(c * v.x + s * v.y, -s * v.x + c * v.y, v.z)
}

/// Decode CSD echoes, instrument placement and ranges into spatial points.
fn csd(bytes: &[u8]) -> Result<Scene> {
    let mut input = Reader::new(bytes);
    if input.take(4)? != b"CSD\0" {
        return Err("invalid Optech CSD signature".into());
    }
    input.take(96)?;
    let version = input.f32()?;
    let header = input.u16()? as usize;
    input.u16()?; // GPS week.
    input.take(16)?; // Acquisition time range.
    let count = input.u32()? as usize;
    stride(count)?;
    let step = count.div_ceil(PREVIEW_POINTS / 4);
    let strips = input.u16()?;
    input.take(1024)?;
    let mis = [input.f64()?, input.f64()?, input.f64()?];
    let imu = [input.f64()?, input.f64()?, input.f64()?];
    let boresight = std::array::from_fn::<_, 3, _>(|i| mis[i] + imu[i]);
    if !version.is_finite()
        || version <= 0.
        || header < 2048
        || strips > 256
        || boresight.iter().any(|v| !v.is_finite())
    {
        return Err("invalid Optech CSD header".into());
    }
    if header + count * 69 != bytes.len() {
        return Err("Optech CSD count and 69-byte record length do not match file".into());
    }
    input.position = header;
    let mut scene = Scene::default();
    for index in 0..count {
        input.f64()?; // GPS time.
        let returns = input.u8()? as usize;
        let range = [input.f32()?, input.f32()?, input.f32()?, input.f32()?];
        input.take(8)?; // Intensities.
        let scan = input.f32()?;
        let attitude = [input.f32()?, input.f32()?, input.f32()?];
        let latitude = input.f64()?;
        let longitude = input.f64()?;
        let platform_height = input.f32()?;
        if returns == 0 {
            continue;
        }
        if latitude.abs() >= std::f64::consts::FRAC_PI_2
            || !Vec3::new(latitude, longitude, platform_height).is_finite()
            || !scan.is_finite()
            || attitude.iter().any(|v| !v.is_finite())
        {
            return Err("invalid Optech CSD platform coordinates".into());
        }
        let flattening = 1. / 298.257_223_563;
        let e2 = 2. * flattening - flattening * flattening;
        let w = (1. - e2 * latitude.sin().powi(2)).sqrt();
        let east_radius = 6_378_137. / w * latitude.cos();
        let north_radius = 6_378_137. * (1. - e2) / w.powi(3);
        let longitude = (longitude + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
            - std::f64::consts::PI;
        for distance in range.into_iter().take(returns.min(4)) {
            if !distance.is_finite() || distance < 0. {
                return Err("invalid Optech CSD echo range".into());
            }
            let sensor = Vec3::new(distance * scan.sin(), 0., -distance * scan.cos());
            let displacement = rotate(rotate(sensor, boresight), attitude);
            let position = Vec3::new(
                (longitude + displacement.x / east_radius).to_degrees(),
                (latitude + displacement.y / north_radius).to_degrees(),
                platform_height + displacement.z,
            );
            append(&mut scene, position, None, index.is_multiple_of(step))?;
        }
    }
    Ok(scene)
}

/// Decode BPF dimension layouts, optional zlib payloads and homogeneous coordinate transforms.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "BPF dimensions/counts are checked against layout limits; finite RGB channels are clamped before byte conversion."
)]
#[allow(
    clippy::too_many_lines,
    reason = "Keep BPF versions, layout metadata and compressed-payload interpretation in one cursor traversal."
)]
fn bpf(bytes: &[u8]) -> Result<Scene> {
    let mut input = Reader::new(bytes);
    let v3 = bytes.starts_with(b"BPF!");
    let mut matrix = [0.; 16];
    for index in 0..4 {
        matrix[index * 5] = 1.;
    }
    let mut offsets = Vec::new();
    let mut labels = Vec::new();
    let (header, count, dimensions, layout, compression) = if v3 {
        input.take(4)?;
        if input.take(4)? != b"0003" {
            return Err("unsupported BPF version".into());
        }
        let header = input.u32()? as usize;
        let dimensions = input.u8()? as usize;
        let layout = input.u8()?;
        let compression = input.u8()?;
        input.u8()?;
        let count = input.u32()? as usize;
        input.take(12)?;
        for value in &mut matrix {
            *value = input.f64()?;
        }
        input.take(16)?;
        (header, count, dimensions, layout, compression)
    } else {
        let header = input.u32()? as usize;
        let version = input.u32()?;
        let count = input.u32()? as usize;
        let extra = input.u32()? as usize;
        input.take(12)?;
        if extra > 252 || !(1..=2).contains(&version) {
            return Err("unsupported BPF legacy version/dimensions".into());
        }
        for _ in 0..3 {
            offsets.push(input.f64()?);
        }
        input.take(48)?;
        labels.extend(["X".to_owned(), "Y".to_owned(), "Z".to_owned()]);
        (header, count, extra + 3, (version - 1) as u8, 0)
    };
    let step = stride(count)?;
    if !(3..=255).contains(&dimensions) || layout > 2 || ![0, 3].contains(&compression) {
        return Err(
            "BPF requires XYZ dimensions and uncompressed/zlib data in a supported layout".into(),
        );
    }
    let static_count = offsets.len();
    for _ in static_count..dimensions {
        offsets.push(input.f64()?);
    }
    input.take((dimensions - static_count) * 16)?; // Min/max metadata.
    for _ in static_count..dimensions {
        let raw = input.take(32)?;
        let len = raw
            .iter()
            .position(|sample| *sample == 0)
            .unwrap_or(raw.len());
        labels.push(std::str::from_utf8(&raw[..len])?.trim().to_owned());
    }
    if header < input.position
        || header > bytes.len()
        || offsets
            .iter()
            .chain(matrix.iter())
            .any(|sample| !sample.is_finite())
    {
        return Err("invalid BPF header length or transform".into());
    }
    let expected = count
        .checked_mul(dimensions)
        .and_then(|sample| sample.checked_mul(4))
        .ok_or("BPF data size overflow")?;
    if expected as u64 > super::MAX_INPUT_BYTES {
        return Err("BPF decoded point data exceeds 512 MiB".into());
    }
    let data = if compression == 0 {
        if bytes.len() - header != expected {
            return Err("BPF point data length does not match its header".into());
        }
        bytes[header..].to_vec()
    } else {
        input.position = header;
        let mut data = Vec::new();
        while data.len() < expected {
            let expanded_length = input.u32()? as usize;
            let encoded = input.u32()? as usize;
            if expanded_length == 0 || expanded_length > expected - data.len() || encoded == 0 {
                return Err("invalid BPF zlib block length".into());
            }
            let block = input.take(encoded)?;
            let mut decoder = flate2::read::ZlibDecoder::new(block);
            let mut out = Vec::new();
            decoder
                .by_ref()
                .take(expanded_length as u64 + 1)
                .read_to_end(&mut out)?;
            if out.len() != expanded_length || decoder.total_in() != encoded as u64 {
                return Err("BPF zlib block length/checksum mismatch".into());
            }
            data.extend(out);
        }
        if input.position != bytes.len() {
            return Err("unexpected bytes after BPF compressed data".into());
        }
        data
    };
    let axis = |name: &str| -> Result<usize> {
        let matches: Vec<_> = labels
            .iter()
            .enumerate()
            .filter(|(_, label)| label.eq_ignore_ascii_case(name))
            .map(|(index, _)| index)
            .collect();
        if matches.len() != 1 {
            return Err("BPF requires unique X/Y/Z dimensions".into());
        }
        Ok(matches[0])
    };
    let xyz = [axis("X")?, axis("Y")?, axis("Z")?];
    let rgb = ["Red", "Green", "Blue"].map(|name| {
        labels
            .iter()
            .position(|label| label.eq_ignore_ascii_case(name))
    });
    let value = |point: usize, dimension: usize| -> f64 {
        let packed: [u8; 4] = match layout {
            0 => data[(dimension * count + point) * 4..(dimension * count + point + 1) * 4]
                .try_into()
                .unwrap(),
            1 => data
                [(point * dimensions + dimension) * 4..(point * dimensions + dimension + 1) * 4]
                .try_into()
                .unwrap(),
            _ => std::array::from_fn(|byte| data[dimension * count * 4 + byte * count + point]),
        };
        f64::from(f32::from_le_bytes(packed)) + offsets[dimension]
    };
    let mut scene = Scene::default();
    let mut colors = Vec::new();
    let mut max_color = 0.0f64;
    for point in 0..count {
        let [x_coordinate, y_coordinate, z_coordinate] =
            xyz.map(|dimension| value(point, dimension));
        let denominator = matrix[12] * x_coordinate
            + matrix[13] * y_coordinate
            + matrix[14] * z_coordinate
            + matrix[15];
        if denominator == 0. {
            return Err("BPF transform has a zero homogeneous denominator".into());
        }
        let sample = std::array::from_fn::<_, 3, _>(|row| {
            (matrix[row * 4] * x_coordinate
                + matrix[row * 4 + 1] * y_coordinate
                + matrix[row * 4 + 2] * z_coordinate
                + matrix[row * 4 + 3])
                / denominator
        });
        let selected = point.is_multiple_of(step);
        append(
            &mut scene,
            Vec3::new(sample[0], sample[1], sample[2]),
            None,
            selected,
        )?;
        if selected {
            let color = if let [
                Some(red_dimension),
                Some(green_dimension),
                Some(blue_dimension),
            ] = rgb
            {
                let components = [
                    value(point, red_dimension),
                    value(point, green_dimension),
                    value(point, blue_dimension),
                ];
                if components
                    .iter()
                    .any(|sample| !sample.is_finite() || *sample < 0. || *sample > 65535.)
                {
                    return Err("invalid BPF RGB component".into());
                }
                for sample in components {
                    max_color = max_color.max(sample);
                }
                Some(components)
            } else {
                None
            };
            colors.push(color);
        }
    }
    let divisor = if max_color > 255. { 257. } else { 1. };
    for (point, color) in scene.points.iter_mut().zip(colors) {
        point.color = color.map(|rgb| rgb.map(|sample| (sample / divisor).round() as u8));
    }
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn terra_file(version: u32) -> Vec<u8> {
        let mut b = Vec::new();
        for v in [56, version, 970_401] {
            b.extend(v.to_le_bytes());
        }
        b.extend(b"CXYZ");
        b.extend(2u32.to_le_bytes());
        b.extend(100i32.to_le_bytes());
        for v in [1000f64, -2000., 3000.] {
            b.extend(v.to_le_bytes());
        }
        b.extend(1u32.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        for coordinates in [[1100i32, -1800, 3300], [1200, -1700, 3400]] {
            if version == 20_010_712 {
                b.extend([1, 2, 3, 0]);
            }
            for v in coordinates {
                b.extend(v.to_le_bytes());
            }
            if version == 20_020_715 {
                b.extend([0; 8]);
            }
            b.extend(1234u32.to_le_bytes());
            b.extend([255, 128, 0, 255]);
        }
        b
    }
    #[test]
    fn terra_versions_offsets_and_optional_fields() {
        for version in [20_010_712, 20_020_715] {
            let b = terra_file(version);
            let s = terra(&b).unwrap();
            assert_eq!(s.points[0].position, Vec3::new(1., 2., 3.));
            assert_eq!(s.points[1].position, Vec3::new(2., 3., 4.));
            assert_eq!(s.points[0].color, Some([255, 128, 0]));
            assert!(terra(&b[..b.len() - 1]).is_err());
        }
    }
    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "The authored QFIT record sizes are small positive constants fitting every tested signed/unsigned width."
    )]
    fn qfit_endian_lengths_and_header_offsets() {
        for size in [40usize, 48, 56] {
            for big in [false, true] {
                let mut b = vec![0; size * 4];
                let values = [
                    (0, size as i32),
                    (size, -9_000_000),
                    (size + 4, (size * 2) as i32),
                    (size * 2, 100),
                    (size * 2 + 4, 51_000_000),
                    (size * 2 + 8, -120_000_000),
                    (size * 2 + 12, 4500),
                    (size * 3, 200),
                    (size * 3 + 4, 51_000_001),
                    (size * 3 + 8, -120_000_001),
                    (size * 3 + 12, 4600),
                ];
                for (offset, v) in values {
                    b[offset..offset + 4].copy_from_slice(&if big {
                        v.to_be_bytes()
                    } else {
                        v.to_le_bytes()
                    });
                }
                let s = qfit(&b).unwrap();
                assert_eq!(s.points.len(), 2);
                assert_eq!(s.points[0].position, Vec3::new(-120., 51., 4.5));
                assert!(qfit(&b[..b.len() - 1]).is_err());
            }
        }
    }
    #[test]
    fn sbet_coordinates_and_invalid_records() {
        let mut b = Vec::new();
        for v in [
            1f64, 0.5, -1., 10., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0.,
        ] {
            b.extend(v.to_le_bytes());
        }
        let s = sbet(&b).unwrap();
        assert_eq!(
            s.points[0].position,
            Vec3::new(-1f64.to_degrees(), 0.5f64.to_degrees(), 10.)
        );
        assert!(sbet(&b[..b.len() - 1]).is_err());
        b[24..32].copy_from_slice(&f64::NAN.to_le_bytes());
        assert!(sbet(&b).is_err());
    }
    #[test]
    fn csd_echoes_boresight_and_range_geometry() {
        let mut b = vec![0; 2048];
        b[..4].copy_from_slice(b"CSD\0");
        b[100..104].copy_from_slice(&5.201f32.to_le_bytes());
        b[104..106].copy_from_slice(&2048u16.to_le_bytes());
        b[124..128].copy_from_slice(&1u32.to_le_bytes());
        b.extend(0f64.to_le_bytes());
        b.push(2);
        for v in [10f32, 20., 0., 0.] {
            b.extend(v.to_le_bytes());
        }
        b.extend([0; 8]);
        for v in [0f32, 0., 0., 0.] {
            b.extend(v.to_le_bytes());
        }
        b.extend(0f64.to_le_bytes());
        b.extend(0f64.to_le_bytes());
        b.extend(100f32.to_le_bytes());
        let s = csd(&b).unwrap();
        assert_eq!(s.points[0].position, Vec3::new(0., 0., 90.));
        assert_eq!(s.points[1].position, Vec3::new(0., 0., 80.));
        // A 90-degree roll moves the range displacement along east, rather than altitude.
        b[1154..1162].copy_from_slice(&std::f64::consts::FRAC_PI_2.to_le_bytes());
        let s = csd(&b).unwrap();
        assert!((s.points[0].position.z - 100.).abs() < 1e-10);
        assert!(
            (s.points[0].position.x + 10. / 6_378_137. * 180. / std::f64::consts::PI).abs() < 1e-10
        );
        assert!(csd(&b[..b.len() - 1]).is_err());
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "The authored BPF fixture uses three points and a few dimensions with bounded positive values."
    )]
    fn bpf_file(layout: u8, compressed: bool) -> Vec<u8> {
        let dimensions = 3;
        let count = 2;
        let mut b = b"BPF!0003".to_vec();
        b.extend((176u32 + 56 * dimensions).to_le_bytes());
        b.extend([dimensions as u8, layout, if compressed { 3 } else { 0 }, 0]);
        b.extend((count as u32).to_le_bytes());
        b.extend([0; 12]);
        for r in 0..4 {
            for c in 0..4 {
                let v: f64 = if r == c {
                    1.
                } else if r == 0 && c == 3 {
                    10.
                } else {
                    0.
                };
                b.extend(v.to_le_bytes());
            }
        }
        b.extend([0; 16]);
        for v in [100f64, 200., 300.] {
            b.extend(v.to_le_bytes());
        }
        b.extend([0; 48]);
        for label in *b"XYZ" {
            b.push(label);
            b.extend([0; 31]);
        }
        let values = [[1f32, 2., 3.], [4., 5., 6.]];
        let mut data = Vec::new();
        match layout {
            0 => {
                for d in 0..3 {
                    for v in values {
                        data.extend(v[d].to_le_bytes());
                    }
                }
            }
            1 => {
                for v in values {
                    for component in v {
                        data.extend(component.to_le_bytes());
                    }
                }
            }
            _ => {
                for d in 0..3 {
                    for byte in 0..4 {
                        for v in values {
                            data.push(v[d].to_le_bytes()[byte]);
                        }
                    }
                }
            }
        }
        if compressed {
            let mut writer =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            writer.write_all(&data).unwrap();
            let out = writer.finish().unwrap();
            b.extend((data.len() as u32).to_le_bytes());
            b.extend((out.len() as u32).to_le_bytes());
            b.extend(out);
        } else {
            b.extend(data);
        }
        b
    }
    #[test]
    fn bpf_layouts_zlib_and_homogeneous_transform() {
        for layout in 0..3 {
            for compressed in [false, true] {
                let b = bpf_file(layout, compressed);
                let s = bpf(&b).unwrap();
                assert_eq!(s.points[0].position, Vec3::new(111., 202., 303.));
                assert_eq!(s.points[1].position, Vec3::new(114., 205., 306.));
                assert!(bpf(&b[..b.len() - 1]).is_err());
            }
        }
        let mut b = bpf_file(0, false);
        b[32..40].copy_from_slice(&2f64.to_le_bytes());
        b[40..48].copy_from_slice(&1f64.to_le_bytes());
        assert_eq!(
            bpf(&b).unwrap().points[0].position.x,
            2. * 101. + 202. + 10.
        );
        b[152..160].copy_from_slice(&0f64.to_le_bytes());
        assert!(bpf(&b).is_err());
    }
}
