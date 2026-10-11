//! Bounded OpenVDB scalar trees (5/4/3 topology), decoded directly in Rust.
//! Layout: OpenVDB io/Compression.h, tree/{RootNode,InternalNode,LeafNode}.h.
//! Sparse samples become a coarse density/level-set surface for a thumbnail.
use super::transform::Transform;
use crate::{
    Result,
    scene::{Scene, Vec3},
};
use std::{collections::HashMap, io::Read, path::Path};

/// Maximum scalar sample count accepted before array expansion.
const MAX_SAMPLES: usize = 5_000_000;
/// Maximum expanded scalar payload bytes.
const MAX_DECODE_BYTES: usize = 256 * 1024 * 1024;
/// Maximum sparse leaves collected before reading their value blocks.
const MAX_LEAVES: usize = 131_072;

/// Bounds-checked cursor over an `OpenVDB` byte stream.
struct Cursor<'a> {
    /// Borrowed complete VDB input bytes.
    bytes: &'a [u8],
    /// Absolute position of the next unread byte.
    pos: usize,
}
impl<'a> Cursor<'a> {
    /// Start a bounds-checked reader at the beginning of the supplied bytes.
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    /// Consume the requested bytes, rejecting offset overflow and truncated records.
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or("VDB offset overflow")?;
        let b = self.bytes.get(self.pos..end).ok_or("truncated VDB data")?;
        self.pos = end;
        Ok(b)
    }
    /// Move to an absolute file offset after checking the input bounds.
    fn seek(&mut self, n: usize) -> Result<()> {
        if n > self.bytes.len() {
            return Err("VDB offset exceeds file".into());
        }
        self.pos = n;
        Ok(())
    }
    /// Read one unsigned byte from the current record.
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    /// Read one unsigned 32-bit word from the current record.
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    /// Read one unsigned 64-bit word from the current record.
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
    /// Read one finite double-precision number from the current record.
    fn f64(&mut self) -> Result<f64> {
        finite(f64::from_le_bytes(self.take(8)?.try_into()?))
    }
    /// Read a bounded UTF-8 VDB length-prefixed name.
    fn name(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        if n > 1_048_576 {
            return Err("VDB string exceeds limit".into());
        }
        Ok(std::str::from_utf8(self.take(n)?)?.to_owned())
    }
    /// Read three signed integer tree coordinates.
    fn xyz(&mut self) -> Result<Vec3> {
        let v = self.take(12)?;
        Ok(Vec3::new(
            f64::from(i32::from_le_bytes(v[..4].try_into()?)),
            f64::from(i32::from_le_bytes(v[4..8].try_into()?)),
            f64::from(i32::from_le_bytes(v[8..12].try_into()?)),
        ))
    }
    /// Read three finite double-precision spatial coordinates.
    fn dxyz(&mut self) -> Result<Vec3> {
        Ok(Vec3::new(self.f64()?, self.f64()?, self.f64()?))
    }
    /// Retain only scalar-grid interpretation metadata while validating all record lengths.
    fn metadata(&mut self) -> Result<HashMap<String, Vec<u8>>> {
        let n = self.u32()?;
        if n > 4096 {
            return Err("VDB metadata count exceeds limit".into());
        }
        let mut m = HashMap::new();
        for _ in 0..n {
            let key = self.name()?;
            let _ty = self.name()?;
            let n = self.u32()? as usize;
            if n > 16 * 1024 * 1024 {
                return Err("VDB metadata value exceeds limit".into());
            }
            let value = self.take(n)?;
            if ["class", "name", "is_saved_as_half_float", "value_type"].contains(&key.as_str()) {
                m.insert(key, value.to_vec());
            }
        }
        Ok(m)
    }
}
/// Reject nonfinite values before using them as coordinates or scalar samples.
fn finite(v: f64) -> Result<f64> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err("non-finite VDB value".into())
    }
}

#[derive(Clone, Copy)]
/// Supported scalar storage types for the `OpenVDB` scalar grid.
enum Scalar {
    /// IEEE single-precision floating-point tree values.
    Float,
    /// IEEE double-precision floating-point tree values.
    Double,
    /// Signed 32-bit integer values.
    I32,
    /// Signed 64-bit integer values.
    I64,
}
impl Scalar {
    /// Return the full-width storage size of one scalar tree value.
    fn width(self) -> usize {
        match self {
            Self::Float | Self::I32 => 4,
            _ => 8,
        }
    }
    /// Read a finite full-width or half-width scalar tree value.
    #[allow(
        clippy::cast_precision_loss,
        reason = "Integer tree samples intentionally approximate to f64 scalar values for density previews."
    )]
    fn read(self, c: &mut Cursor<'_>, half: bool) -> Result<f64> {
        if half && matches!(self, Self::Float | Self::Double) {
            let b = u16::from_le_bytes(c.take(2)?.try_into()?);
            let sign = if b & 0x8000 != 0 { -1.0 } else { 1.0 };
            let e = (b >> 10) & 31;
            let fraction = f64::from(b & 1023);
            return finite(
                sign * match e {
                    0 => fraction * 2f64.powi(-24),
                    31 => f64::INFINITY,
                    _ => (1.0 + fraction / 1024.0) * 2f64.powi(i32::from(e) - 15),
                },
            );
        }
        finite(match self {
            Self::Float => f64::from(f32::from_le_bytes(c.take(4)?.try_into()?)),
            Self::Double => c.f64()?,
            Self::I32 => f64::from(i32::from_le_bytes(c.take(4)?.try_into()?)),
            Self::I64 => i64::from_le_bytes(c.take(8)?.try_into()?) as f64,
        })
    }
}

/// Stored scalar-grid directory entry and its tree byte range.
struct Descriptor {
    /// Stored grid identifier.
    name: String,
    /// Declared scalar grid type.
    kind: String,
    /// Referenced grid providing shared topology, if present.
    parent: String,
    /// Absolute offset of this grid payload.
    start: usize,
    /// Absolute offset of the leaf-value block section.
    blocks: usize,
    /// Absolute end offset of this grid payload.
    end: usize,
}
#[derive(Clone, Copy)]
/// An occupied constant-value sparse tile or voxel.
struct Sample {
    /// Index-space origin of the occupied tile.
    origin: Vec3,
    /// Index-space edge length of the constant tile.
    size: f64,
    /// Finite scalar density or signed-distance value.
    value: f64,
}
/// Scalar-tree decoder state, compressed-value settings and collected samples.
struct TreeReader {
    /// Full-width scalar type of stored values.
    scalar: Scalar,
    /// Whether floating-point values are stored as binary16.
    half: bool,
    /// VDB value-compression flag bits.
    compression: u32,
    /// Value of unrepresented inactive samples.
    background: f64,
    /// Whether negative samples represent occupied level-set interior.
    level_set: bool,
    /// Aggregate expanded value bytes consumed.
    decoded: usize,
    /// Active occupied tiles collected for surface construction.
    samples: Vec<Sample>,
    /// Leaf origins waiting for their value blocks.
    leaves: Vec<Vec3>,
}
/// Test an indexed bit in a tree occupancy mask.
fn bit(mask: &[u8], i: usize) -> bool {
    mask[i / 8] & (1 << (i % 8)) != 0
}
/// Convert a packed child index into its spatial tile origin.
#[allow(
    clippy::cast_precision_loss,
    reason = "The tree topology bounds packed child indices before their exact small integer coordinates become f64."
)]
fn offset(origin: Vec3, index: usize, log: usize, size: f64) -> Vec3 {
    let mask = (1 << log) - 1;
    origin
        + Vec3::new(
            (index >> (2 * log)) as f64,
            ((index >> log) & mask) as f64,
            (index & mask) as f64,
        ) * size
}
impl TreeReader {
    /// Retain active density or level-set tiles within the sample limit.
    fn sample(&mut self, origin: Vec3, size: f64, value: f64, active: bool) -> Result<()> {
        if !(active || (self.level_set && value < 0.0)) {
            return Ok(());
        }
        if self.samples.len() >= MAX_SAMPLES {
            return Err("VDB exceeds five million sparse preview samples".into());
        }
        self.samples.push(Sample {
            origin,
            size,
            value,
        });
        Ok(())
    }
    /// Decode compressed/masked node values and reconstruct inactive background entries.
    fn values(&mut self, c: &mut Cursor<'_>, mask: &[u8], n: usize) -> Result<Vec<f64>> {
        let metadata = c.u8()?;
        if metadata > 6 {
            return Err(format!(
                "invalid VDB value-mask metadata {metadata} at byte {}",
                c.pos - 1
            )
            .into());
        }
        let mut inactive = [
            if metadata == 0 {
                self.background
            } else {
                -self.background
            },
            self.background,
        ];
        if [2, 4, 5].contains(&metadata) {
            inactive[0] = self.scalar.read(c, false)?;
        }
        if metadata == 5 {
            inactive[1] = self.scalar.read(c, false)?;
        }
        let selection = if [3, 4, 5].contains(&metadata) {
            Some(c.take(n / 8)?)
        } else {
            None
        };
        let compressed_mask = self.compression & 2 != 0 && metadata != 6;
        let count = if compressed_mask {
            mask.iter().map(|b| b.count_ones() as usize).sum()
        } else {
            n
        };
        let width = if self.half { 2 } else { self.scalar.width() };
        let length = count.checked_mul(width).ok_or("VDB sample byte overflow")?;
        self.decoded = self
            .decoded
            .checked_add(length)
            .ok_or("VDB decode byte overflow")?;
        if self.decoded > MAX_DECODE_BYTES {
            return Err("VDB exceeds 256 MiB decoded scalar budget".into());
        }
        // OpenVDB's HalfReader returns before reading a block header when no
        // active values remain. Full-width compressed arrays still have it.
        let raw = if self.half && count == 0 {
            Vec::new()
        } else if self.compression & 5 != 0 {
            let size = i64::from_le_bytes(c.take(8)?.try_into()?);
            if size <= 0 {
                let size = usize::try_from(size.checked_neg().ok_or("VDB block size overflow")?)?;
                if size != length {
                    return Err("VDB raw block has wrong length".into());
                }
                c.take(size)?.to_vec()
            } else {
                let data = c.take(usize::try_from(size)?)?;
                if self.compression & 4 != 0 {
                    blosc(data, length)?
                } else {
                    decode_stream(flate2::read::ZlibDecoder::new(data), length)?
                }
            }
        } else {
            c.take(length)?.to_vec()
        };
        let mut reader = Cursor::new(&raw);
        let mut values = Vec::with_capacity(n);
        for i in 0..n {
            values.push(if !compressed_mask || bit(mask, i) {
                self.scalar.read(&mut reader, self.half)?
            } else {
                inactive[usize::from(selection.is_some_and(|m| bit(m, i)))]
            });
        }
        if reader.pos != raw.len() {
            return Err("VDB block has excess samples".into());
        }
        Ok(values)
    }
    /// Walk one internal tree level and collect leaves or active tiles.
    fn internal(&mut self, c: &mut Cursor<'_>, origin: Vec3, log: usize) -> Result<()> {
        let n = 1 << (3 * log);
        let children = c.take(n / 8)?;
        let active = c.take(n / 8)?;
        let values = self.values(c, active, n)?;
        let size = if log == 5 { 128.0 } else { 8.0 };
        for (i, v) in values.into_iter().enumerate() {
            if !bit(children, i) {
                self.sample(offset(origin, i, log, size), size, v, bit(active, i))?;
            }
        }
        for i in 0..n {
            if bit(children, i) {
                let child = offset(origin, i, log, size);
                if log == 5 {
                    self.internal(c, child, 4)?;
                } else {
                    c.take(64)?;
                    if self.leaves.len() >= MAX_LEAVES {
                        return Err("VDB leaf count exceeds limit".into());
                    }
                    self.leaves.push(child);
                }
            }
        }
        Ok(())
    }
    /// Decode root tile topology and its child leaf-value blocks.
    fn tree(&mut self, c: &mut Cursor<'_>, blocks: usize) -> Result<()> {
        if c.u32()? != 1 {
            return Err("VDB multipass/point-data trees are not implemented".into());
        }
        self.background = self.scalar.read(c, false)?;
        let tiles = c.u32()? as usize;
        let roots = c.u32()? as usize;
        if tiles > MAX_SAMPLES || roots > 8192 {
            return Err("VDB root count exceeds limit".into());
        }
        for _ in 0..tiles {
            let origin = c.xyz()?;
            let value = self.scalar.read(c, false)?;
            let active = c.u8()? != 0;
            self.sample(origin, 4096.0, value, active)?;
        }
        for _ in 0..roots {
            let origin = c.xyz()?;
            self.internal(c, origin, 5)?;
        }
        if c.pos > blocks {
            return Err("VDB topology overlaps its leaf blocks".into());
        }
        c.seek(blocks)?;
        for origin in std::mem::take(&mut self.leaves) {
            let mask = c.take(64)?;
            let values = self.values(c, mask, 512)?;
            for (i, value) in values.into_iter().enumerate() {
                self.sample(offset(origin, i, 3, 1.0), 1.0, value, bit(mask, i))?;
            }
        }
        Ok(())
    }
}

/// Decode supported affine grid maps into world-space placement.
#[allow(
    clippy::float_cmp,
    reason = "A supported affine grid map requires the stored homogeneous final entry to be exactly one."
)]
fn transform(c: &mut Cursor<'_>) -> Result<Transform> {
    let name = c.name()?;
    let mut result = Transform::IDENTITY;
    match name.as_str() {
        "ScaleMap" | "UniformScaleMap" | "ScaleTranslateMap" | "UniformScaleTranslateMap" => {
            if name.contains("Translate") {
                let v = c.dxyz()?;
                result.0[3][..3].copy_from_slice(&[v.x, v.y, v.z]);
            }
            let v = c.dxyz()?;
            result.0[0][0] = v.x;
            result.0[1][1] = v.y;
            result.0[2][2] = v.z;
            for _ in 0..4 {
                let _ = c.dxyz()?;
            }
        }
        "TranslationMap" => {
            let v = c.dxyz()?;
            result.0[3][..3].copy_from_slice(&[v.x, v.y, v.z]);
        }
        "AffineMap" | "UnitaryMap" => {
            for column in &mut result.0 {
                for value in column {
                    *value = c.f64()?;
                }
            }
            if result.0[0][3] != 0.0
                || result.0[1][3] != 0.0
                || result.0[2][3] != 0.0
                || result.0[3][3] != 1.0
            {
                return Err("VDB affine map is not affine".into());
            }
        }
        _ => return Err(format!("unsupported VDB transform {name}").into()),
    }
    Ok(result)
}

/// Read a bounded `OpenVDB` scalar grid and decode its preview geometry.
///
/// # Errors
/// Returns an error for unsupported grid/tree layouts, invalid offsets, malformed compression,
/// nonfinite samples or resource limits.
pub(super) fn load(path: &Path) -> Result<Scene> {
    decode(&super::read_file(path)?)
}
/// Validate scalar grid descriptors and decode supported VDB tree/compression layouts.
fn decode(bytes: &[u8]) -> Result<Scene> {
    let mut c = Cursor::new(bytes);
    if c.u64()? != 0x0000_0000_5644_4220 {
        return Err("invalid OpenVDB magic".into());
    }
    let version = c.u32()?;
    if !(222..=224).contains(&version) {
        return Err("OpenVDB preview supports file versions 222–224".into());
    }
    let _major = c.u32()?;
    let _minor = c.u32()?;
    if c.u8()? != 1 {
        return Err("VDB archives without grid offsets are not implemented".into());
    }
    c.take(36)?;
    c.metadata()?;
    let count = c.u32()? as usize;
    if count == 0 || count > 256 {
        return Err("VDB grid count is empty or exceeds limit".into());
    }
    let mut descriptors = Vec::new();
    let mut names = std::collections::HashSet::new();
    for _ in 0..count {
        let name = c.name()?;
        let kind = c.name()?;
        let parent = c.name()?;
        let start = usize::try_from(c.u64()?)?;
        let blocks = usize::try_from(c.u64()?)?;
        let end = usize::try_from(c.u64()?)?;
        if start < c.pos
            || start > blocks
            || blocks > end
            || end > bytes.len()
            || !names.insert(name.clone())
        {
            return Err("invalid VDB descriptor offsets or duplicate name".into());
        }
        descriptors.push(Descriptor {
            name,
            kind,
            parent,
            start,
            blocks,
            end,
        });
        c.seek(end)?;
    }
    // Prefer a density field, otherwise the first supported scalar field.
    descriptors.sort_by_key(|d| usize::from(!d.name.to_ascii_lowercase().contains("density")));
    let descriptor=descriptors.into_iter().find(|d|["Tree_float_5_4_3","Tree_double_5_4_3","Tree_int32_5_4_3","Tree_int64_5_4_3"].iter().any(|kind|d.kind==*kind||d.kind==format!("{kind}_HalfFloat"))).ok_or("VDB contains no supported scalar 5/4/3 tree; vector/mask/point grids are not implemented")?;
    if !descriptor.parent.is_empty() {
        return Err("instanced VDB trees are not implemented".into());
    }
    c.seek(descriptor.start)?;
    let compression = c.u32()?;
    if compression & !7 != 0 || compression & 5 == 5 {
        return Err("invalid VDB compression flags".into());
    }
    let metadata = c.metadata()?;
    let map = transform(&mut c)?;
    let level_set = metadata.get("class").is_some_and(|v| v == b"level set");
    let half = descriptor.kind.ends_with("_HalfFloat")
        || metadata
            .get("is_saved_as_half_float")
            .is_some_and(|v| v == &[1]);
    let scalar = if descriptor.kind.starts_with("Tree_float_") {
        Scalar::Float
    } else if descriptor.kind.starts_with("Tree_double_") {
        Scalar::Double
    } else if descriptor.kind.starts_with("Tree_int32_") {
        Scalar::I32
    } else {
        Scalar::I64
    };
    if half && !matches!(scalar, Scalar::Float | Scalar::Double) {
        return Err("VDB half encoding requires a floating-point grid".into());
    }
    let mut reader = TreeReader {
        scalar,
        half,
        compression,
        background: 0.0,
        level_set,
        decoded: 0,
        samples: Vec::new(),
        leaves: Vec::new(),
    };
    reader.tree(&mut c, descriptor.blocks)?;
    if c.pos > descriptor.end {
        return Err("VDB leaf blocks exceed their grid".into());
    }
    surface(reader.samples, level_set, map)
}

/// Rasterize sparse samples into a bounded density or level-set preview surface.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "Preview indices are clamped to the 32-cubed lattice and nonnegative before native indexing."
)]
fn surface(samples: Vec<Sample>, level_set: bool, map: Transform) -> Result<Scene> {
    if samples.is_empty() {
        return Err("VDB scalar grid has no active geometry".into());
    }
    let mut min = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for sample in &samples {
        let a = sample.origin - Vec3::new(0.5, 0.5, 0.5);
        let b = a + Vec3::new(sample.size, sample.size, sample.size);
        min = Vec3::new(min.x.min(a.x), min.y.min(a.y), min.z.min(a.z));
        max = Vec3::new(max.x.max(b.x), max.y.max(b.y), max.z.max(b.z));
    }
    let a = [min.x, min.y, min.z];
    let extent = [max.x - min.x, max.y - min.y, max.z - min.z];
    let dimensions = extent.map(|n| (n.ceil() as usize).clamp(1, 32));
    let step = std::array::from_fn::<_, 3, _>(|i| extent[i] / dimensions[i] as f64);
    let mut values = vec![0.0f64; dimensions.iter().product()];
    for sample in samples {
        let value = if level_set {
            if sample.value <= 0.0 { 1.0 } else { 0.0 }
        } else {
            sample.value.max(0.0)
        };
        if value == 0.0 {
            continue;
        }
        let low = [
            sample.origin.x - 0.5,
            sample.origin.y - 0.5,
            sample.origin.z - 0.5,
        ];
        let first = std::array::from_fn::<_, 3, _>(|i| {
            (((low[i] - a[i]) / step[i]).floor() as usize).min(dimensions[i] - 1)
        });
        let last = std::array::from_fn::<_, 3, _>(|i| {
            (((low[i] + sample.size - a[i]) / step[i]).ceil() as usize)
                .clamp(first[i] + 1, dimensions[i])
        });
        for z in first[2]..last[2] {
            for y in first[1]..last[1] {
                for x in first[0]..last[0] {
                    let i = (z * dimensions[1] + y) * dimensions[0] + x;
                    values[i] = values[i].max(value);
                }
            }
        }
    }
    let origin = map.apply(min);
    let axes = [
        Vec3::new(step[0], 0., 0.),
        Vec3::new(0., step[1], 0.),
        Vec3::new(0., 0., step[2]),
    ]
    .map(|v| map.apply(min + v) - origin);
    // Volume coordinates describe sample centers; move half a coarse voxel inward.
    super::volume::density_scene(
        dimensions,
        origin + (axes[0] + axes[1] + axes[2]) * 0.5,
        axes,
        values,
    )
}

/// Expand exactly the declared compressed bytes within the VDB decoded-byte budget.
fn decode_stream(reader: impl Read, length: usize) -> Result<Vec<u8>> {
    let mut data = Vec::with_capacity(length);
    reader.take(length as u64 + 1).read_to_end(&mut data)?;
    if data.len() != length {
        return Err("VDB decompressed block has wrong length".into());
    }
    Ok(data)
}

/// Blosc1 chunks: bounded blocks and split streams, shuffle, BloscLZ/LZ4/zlib/Zstd.
#[allow(
    clippy::too_many_lines,
    reason = "Keep Blosc block-table, compression and shuffle-layout checks together with output accounting."
)]
fn blosc(data: &[u8], length: usize) -> Result<Vec<u8>> {
    let mut c = Cursor::new(data);
    let version = c.u8()?;
    let _codec_version = c.u8()?;
    let flags = c.u8()?;
    let width = c.u8()? as usize;
    let n = c.u32()? as usize;
    let block = c.u32()? as usize;
    let compressed = c.u32()? as usize;
    if ![1, 2].contains(&version)
        || width == 0
        || n != length
        || compressed != data.len()
        || n > MAX_DECODE_BYTES
    {
        return Err("invalid or unsupported Blosc1 header".into());
    }
    if flags & 2 != 0 {
        if compressed != 16 + n {
            return Err("invalid Blosc memcpy length".into());
        }
        return Ok(c.take(n)?.to_vec());
    }
    if n == 0 {
        return Ok(Vec::new());
    }
    if block == 0 || block > MAX_DECODE_BYTES {
        return Err("invalid Blosc block size".into());
    }
    let blocks = n.div_ceil(block);
    if blocks > 1_048_576 {
        return Err("Blosc block count exceeds limit".into());
    }
    let mut starts = Vec::with_capacity(blocks);
    for _ in 0..blocks {
        starts.push(c.u32()? as usize);
    }
    let table_end = c.pos;
    let mut ordered = starts.clone();
    ordered.sort_unstable();
    if ordered.windows(2).any(|v| v[0] == v[1]) {
        return Err("duplicate Blosc block offset".into());
    }
    let ends: HashMap<usize, usize> = ordered
        .iter()
        .copied()
        .zip(
            ordered
                .iter()
                .copied()
                .skip(1)
                .chain(std::iter::once(data.len())),
        )
        .collect();
    let mut output = vec![0; n];
    for (i, &start) in starts.iter().enumerate() {
        if start < table_end || start >= data.len() {
            return Err("Blosc block offset is out of bounds".into());
        }
        // Block offsets are allowed in arbitrary order (parallel C writers).
        let end = ends[&start];
        let mut source = Cursor::new(&data[start..end]);
        let target_len = (n - i * block).min(block);
        let streams = if flags & 16 == 0 && target_len == block {
            width
        } else {
            1
        };
        if !target_len.is_multiple_of(streams) {
            return Err("Blosc split streams do not divide block".into());
        }
        let stream_len = target_len / streams;
        let mut decoded = vec![0; target_len];
        for target in decoded.chunks_mut(stream_len.max(1)) {
            let size = source.u32()? as usize;
            let content = source.take(size)?;
            if size == stream_len {
                target.copy_from_slice(content);
            } else {
                let actual = match flags >> 5 {
                    0 => blosclz(content, target)?,
                    1 => lz4_flex::block::decompress_into(content, target)?,
                    3 => {
                        let bytes =
                            decode_stream(flate2::read::ZlibDecoder::new(content), stream_len)?;
                        target.copy_from_slice(&bytes);
                        stream_len
                    }
                    4 => {
                        let bytes = decode_stream(
                            ruzstd::decoding::StreamingDecoder::new(content)?,
                            stream_len,
                        )?;
                        target.copy_from_slice(&bytes);
                        stream_len
                    }
                    codec => return Err(format!("Blosc codec {codec} is not implemented").into()),
                };
                if actual != stream_len {
                    return Err("Blosc stream has wrong decoded length".into());
                }
            }
        }
        if source.pos != source.bytes.len() {
            return Err("Blosc block has trailing stream bytes".into());
        }
        let target = &mut output[i * block..i * block + target_len];
        if flags & 4 != 0 {
            bitunshuffle(&decoded, target, width);
        } else if flags & 1 != 0 && width > 1 {
            let elements = target_len / width;
            for j in 0..elements {
                for byte in 0..width {
                    target[j * width + byte] = decoded[byte * elements + j];
                }
            }
            target[elements * width..].copy_from_slice(&decoded[elements * width..]);
        } else {
            target.copy_from_slice(&decoded);
        }
    }
    Ok(output)
}

/// Undo Blosc bit-plane shuffling for fixed-width scalar elements.
fn bitunshuffle(source: &[u8], target: &mut [u8], width: usize) {
    let elements = (source.len() / width) / 8 * 8;
    let plane_len = elements / 8;
    for byte in 0..width {
        for bit_index in 0..8 {
            for group in 0..plane_len {
                let bits = source[(byte * 8 + bit_index) * plane_len + group];
                for j in 0..8 {
                    target[(group * 8 + j) * width + byte] |= ((bits >> j) & 1) << bit_index;
                }
            }
        }
    }
    target[elements * width..].copy_from_slice(&source[elements * width..]);
}

// Raw BloscLZ layout follows the C-Blosc BSD-licensed protocol. All offsets and
// lengths are checked before copying, including an overlapping final match.
/// Expand bounded `BloscLZ` literals and overlapping backward matches.
fn blosclz(input: &[u8], output: &mut [u8]) -> Result<usize> {
    let mut c = Cursor::new(input);
    let mut control = c.u8()? & 31;
    let mut pos = 0usize;
    loop {
        if control < 32 {
            let n = control as usize + 1;
            let bytes = c.take(n)?;
            let target = output
                .get_mut(pos..pos + n)
                .ok_or("BloscLZ literal exceeds output")?;
            target.copy_from_slice(bytes);
            pos += n;
        } else {
            let mut length = (control >> 5) as usize - 1;
            let high = (control as usize & 31) * 256;
            if length == 6 {
                loop {
                    let code = c.u8()? as usize;
                    length = length.checked_add(code).ok_or("BloscLZ length overflow")?;
                    if length > output.len() {
                        return Err("BloscLZ length exceeds output".into());
                    }
                    if code != 255 {
                        break;
                    }
                }
            }
            let code = c.u8()? as usize;
            let distance = if code == 255 && high == 31 * 256 {
                let high = c.u8()? as usize;
                let low = c.u8()? as usize;
                (high << 8) + low + 8191 + 1
            } else {
                high + code + 1
            };
            length += 3;
            if distance > pos || length > output.len() - pos {
                return Err("invalid BloscLZ match distance/length".into());
            }
            for i in 0..length {
                output[pos + i] = output[pos - distance + i];
            }
            pos += length;
        }
        if c.pos == input.len() {
            break;
        }
        control = c.u8()?;
    }
    Ok(pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "The authored sparse grid contains tiny bounded coordinates and byte-sized compression blocks."
    )]
    fn field(half: bool) -> Vec<u8> {
        use std::io::Write;
        let name = |bytes: &mut Vec<u8>, value: &str| {
            bytes.extend((value.len() as u32).to_le_bytes());
            bytes.extend(value.as_bytes());
        };
        let mut topology = Vec::new();
        topology.extend(1u32.to_le_bytes());
        topology.extend(0f32.to_le_bytes());
        topology.extend(0u32.to_le_bytes());
        topology.extend(1u32.to_le_bytes());
        topology.extend([0u8; 12]);
        for dimension in [32768, 4096] {
            let mut children = vec![0u8; dimension / 8];
            children[0] = 1;
            topology.extend(children);
            topology.extend(vec![0; dimension / 8]);
            topology.push(0);
            if !half {
                topology.extend(0i64.to_le_bytes());
            }
        }
        topology.extend([255u8; 64]);
        let mut payload = vec![255u8; 64];
        payload.push(6);
        let mut values = Vec::new();
        for x in 0..8 {
            for y in 0..8 {
                for z in 0..8 {
                    let occupied = [x, y, z].iter().any(|&i| i == 0 || i == 7);
                    if half {
                        values.extend(if occupied { 0x3c00u16 } else { 0 }.to_le_bytes());
                    } else {
                        values.extend(if occupied { 1f32 } else { 0. }.to_le_bytes());
                    }
                }
            }
        }
        let mut zip = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        zip.write_all(&values).unwrap();
        let zip = zip.finish().unwrap();
        payload.extend((zip.len() as i64).to_le_bytes());
        payload.extend(zip);
        let mut grid = Vec::new();
        grid.extend(3u32.to_le_bytes());
        grid.extend(0u32.to_le_bytes());
        name(&mut grid, "ScaleTranslateMap");
        for value in [
            12.,
            -3.,
            7.,
            2.,
            -3.,
            5.,
            2.,
            3.,
            5.,
            0.5,
            -1. / 3.,
            0.2,
            0.25,
            1. / 9.,
            0.04,
            0.25,
            -1. / 6.,
            0.1,
        ] {
            grid.extend(f64::to_le_bytes(value));
        }
        let mut file = Vec::new();
        file.extend(0x5644_4220_u64.to_le_bytes());
        for n in [223u32, 12, 1] {
            file.extend(n.to_le_bytes());
        }
        file.push(1);
        file.extend([b'0'; 36]);
        file.extend(0u32.to_le_bytes());
        file.extend(1u32.to_le_bytes());
        name(&mut file, "density");
        name(
            &mut file,
            if half {
                "Tree_float_5_4_3_HalfFloat"
            } else {
                "Tree_float_5_4_3"
            },
        );
        name(&mut file, "");
        let start = file.len() + 24;
        let block = start + grid.len() + topology.len();
        let end = block + payload.len();
        for p in [start, block, end] {
            file.extend((p as u64).to_le_bytes());
        }
        file.extend(grid);
        file.extend(topology);
        file.extend(payload);
        file
    }

    #[test]
    fn full_and_half_sparse_fields_keep_cavities_and_reflected_placement() {
        let a = decode(&field(false)).unwrap();
        let b = decode(&field(true)).unwrap();
        assert_eq!(a.triangles.len(), 1200); // 6 outer and 6 inner walls, preserving voxel sampling.
        assert_eq!(
            a.triangles.iter().map(|t| t.vertices).collect::<Vec<_>>(),
            b.triangles.iter().map(|t| t.vertices).collect::<Vec<_>>()
        );
        let image = crate::render::render(
            &a,
            &crate::render::RenderOptions {
                width: 64,
                height: 64,
                transparent: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(image.pixels().filter(|p| p[3] > 0).count() > 100);
        let mut bytes = field(true);
        bytes.pop();
        assert!(decode(&bytes).is_err());
        let mut reader = TreeReader {
            scalar: Scalar::Float,
            half: true,
            compression: 6,
            background: 3.,
            level_set: true,
            decoded: 0,
            samples: Vec::new(),
            leaves: Vec::new(),
        };
        let mut c = Cursor::new(&[1, 99]);
        assert_eq!(reader.values(&mut c, &[0], 8).unwrap(), [-3.; 8]);
        assert_eq!(c.pos, 1); // Empty half arrays have no compressed length prefix.
    }
    #[test]
    fn blosclz_final_overlapping_match_is_copied() {
        let mut output = [0; 8];
        assert_eq!(blosclz(&[0, b'a', 0x80, 0], &mut output).unwrap(), 7);
        assert_eq!(&output[..7], b"aaaaaaa");
        assert!(blosclz(&[0, b'a', 0xe0, 255], &mut output).is_err());
    }
    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "The authored shuffle test uses eight nonnegative integer samples, exactly representable in every conversion."
    )]
    fn bit_shuffle_and_byte_shuffle_decode() {
        let expected = (0..64).map(|n| (n * 7) as u8).collect::<Vec<_>>();
        let mut shuffled = vec![0; 64];
        for byte in 0..4 {
            for bit_index in 0..8 {
                for group in 0..2 {
                    for j in 0..8 {
                        shuffled[(byte * 8 + bit_index) * 2 + group] |=
                            ((expected[(group * 8 + j) * 4 + byte] >> bit_index) & 1) << j;
                    }
                }
            }
        }
        let mut decoded = vec![0; 64];
        bitunshuffle(&shuffled, &mut decoded, 4);
        assert_eq!(decoded, expected);
        let mut frame = vec![2, 1, 1 | 16 | (1 << 5), 4];
        for n in [64u32, 64, 88, 20, 64] {
            frame.extend(n.to_le_bytes());
        }
        for byte in 0..4 {
            for i in 0..16 {
                frame.push(expected[i * 4 + byte]);
            }
        }
        assert_eq!(
            blosc(&frame, 64).unwrap(),
            (0..64).map(|n| (n * 7) as u8).collect::<Vec<_>>()
        );
        assert!(blosc(&frame, 65).is_err());
        assert!(blosc(&frame[..frame.len() - 1], 64).is_err());
    }
    #[test]
    fn scalar_masks_restore_negative_background() {
        let mut bytes = vec![1];
        bytes.extend(2f32.to_le_bytes());
        let mut reader = TreeReader {
            scalar: Scalar::Float,
            half: false,
            compression: 2,
            background: 3.,
            level_set: true,
            decoded: 0,
            samples: Vec::new(),
            leaves: Vec::new(),
        };
        let values = reader.values(&mut Cursor::new(&bytes), &[1], 8).unwrap();
        assert_eq!(values, [2., -3., -3., -3., -3., -3., -3., -3.]);
        assert!(decode(&[0; 100]).is_err());
        assert!(decode(b" \x42\x44\x56\0\0\0\0").is_err());
    }
}
