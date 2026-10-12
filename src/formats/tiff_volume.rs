//! Bounded scalar TIFF page stacks with explicit Cartesian sample placement.

#[path = "tiff_directory.rs"]
/// Complete source-bounded directory preflight before constructing the codec.
mod directory;
#[path = "tiff_stream.rs"]
/// Restrict each codec pixel read to its selected declared source chunk.
mod stream;

use crate::{
    Result,
    scene::{Scene, Triangle, Vec3},
};
use directory::{Directory, Order};
use std::ops::Range;
use std::path::Path;
use tiff::{
    ColorType,
    decoder::{Decoder, DecodingResult, Limits},
};

/// Maximum complete scalar frame, shared with existing volume readers.
const MAX_SAMPLES: usize = 16_777_216;
/// Maximum complete expanded f64 sample array.
const MAX_EXPANDED: usize = 128 * 1024 * 1024;
/// Maximum simultaneous source, metadata, decoding and preview storage accounting.
const MAX_STORAGE: usize = 512 * 1024 * 1024;
/// Conservative simultaneous old/new codec metadata and owned directory storage.
const METADATA_STORAGE: usize = 64 * 1024 * 1024;
/// Maximum one codec metadata-value allocation, independent of pixel dimensions.
const METADATA_ALLOCATION: usize = 32 * 1024 * 1024;
/// Maximum cumulative encoded and padded decoded chunk bytes, counting reused spans.
const MAX_DECODE_WORK: usize = 512 * 1024 * 1024;

/// A positive, canonical TIFF unsigned rational resolution.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Rational {
    /// Canonical numerator after reducing common factors.
    numerator: u32,
    /// Nonzero canonical denominator.
    denominator: u32,
}

impl Rational {
    /// Read one exact positive resolution rational, using one for absent tags.
    fn read(page: &Directory<'_>, order: Order, tag: u16) -> Result<Self> {
        let Some(entry) = page.get(tag) else {
            return Ok(Self {
                numerator: 1,
                denominator: 1,
            });
        };
        if entry.kind != 5 || entry.count != 1 {
            return Err("TIFF resolution must be one unsigned rational".into());
        }
        let numerator = u32::try_from(order.integer(&entry.bytes[..4]))?;
        let denominator = u32::try_from(order.integer(&entry.bytes[4..]))?;
        if numerator == 0 || denominator == 0 {
            return Err("TIFF resolution must be positive with a nonzero denominator".into());
        }
        let (mut a, mut b) = (numerator, denominator);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        Ok(Self {
            numerator: numerator / a,
            denominator: denominator / a,
        })
    }

    /// Convert a canonical positive rational to finite physical sample spacing.
    fn spacing(self, unit: u64) -> f64 {
        let length = match unit {
            2 => 25.4,
            3 => 10.,
            _ => return 1.,
        };
        length * f64::from(self.denominator) / f64::from(self.numerator)
    }
}

/// Geometry-compatible metadata that must agree across all selected slices.
#[derive(PartialEq, Eq)]
struct Profile {
    /// Source sample-column count, before orientation.
    width: usize,
    /// Source sample-row count, before orientation.
    height: usize,
    /// Exact scalar storage bit width.
    bits: u64,
    /// Unsigned, signed or floating TIFF sample-format discriminator.
    sample_format: u64,
    /// Black-is-zero or white-is-zero scalar photometric interpretation.
    photometric: u64,
    /// Original TIFF orientation, with absent tags defaulting to top-left.
    orientation: u64,
    /// Unitless, inches or centimetres resolution discriminator.
    unit: u64,
    /// Canonical source-column resolution.
    x_resolution: Rational,
    /// Canonical source-row resolution.
    y_resolution: Rational,
}

impl Profile {
    /// Validate one selected scalar page without invoking a pixel decoder.
    fn read(page: &Directory<'_>, order: Order) -> Result<Self> {
        let width = usize::try_from(page.unsigned(order, 256, 0)?)?;
        let height = usize::try_from(page.unsigned(order, 257, 0)?)?;
        if width == 0
            || height == 0
            || u32::try_from(width).is_err()
            || u32::try_from(height).is_err()
        {
            return Err("TIFF selected page has invalid dimensions".into());
        }
        let photometric = page.unsigned(order, 262, u64::MAX)?;
        if page.unsigned(order, 277, 1)? != 1 || !matches!(photometric, 0 | 1) {
            return Err("TIFF volumes require homogeneous single-channel grayscale pages; RGB and multichannel series are unsupported".into());
        }
        let bits = page.unsigned(order, 258, 1)?;
        let sample_format = page.unsigned(order, 339, 1)?;
        if !matches!(page.unsigned(order, 259, 1)?, 1 | 5 | 8 | 32946 | 32773) {
            return Err("unsupported TIFF scalar-volume compression; supported codecs are uncompressed, LZW, Deflate and PackBits".into());
        }
        if !matches!(bits, 8 | 16 | 32 | 64)
            || !matches!(sample_format, 1..=3)
            || (sample_format == 3 && bits == 8)
        {
            return Err("unsupported TIFF scalar sample format or bit depth".into());
        }
        if photometric == 0 && (sample_format == 2 || (sample_format == 3 && bits == 16)) {
            return Err("unsupported TIFF signed or float16 WhiteIsZero volume".into());
        }
        if page.get(338).is_some_and(|entry| entry.count != 0) {
            return Err("TIFF scalar volume cannot contain extra sample channels".into());
        }
        let orientation = page.unsigned(order, 274, 1)?;
        let unit = page.unsigned(order, 296, 1)?;
        if !matches!(orientation, 1..=8) || !matches!(unit, 1..=3) {
            return Err("unsupported TIFF orientation or resolution unit".into());
        }
        Ok(Self {
            width,
            height,
            bits,
            sample_format,
            photometric,
            orientation,
            unit,
            x_resolution: Rational::read(page, order, 282)?,
            y_resolution: Rational::read(page, order, 283)?,
        })
    }

    /// Preserve original array indices while mapping visual right/up to Cartesian X/Y.
    fn frame(&self) -> Result<(Vec3, [Vec3; 3])> {
        let sx = self.x_resolution.spacing(self.unit);
        let sy = self.y_resolution.spacing(self.unit);
        // Dimensions were checked against u32 before these exact f64 conversions.
        let w = f64::from(u32::try_from(self.width)?) - 1.;
        let h = f64::from(u32::try_from(self.height)?) - 1.;
        let (origin, column, row) = match self.orientation {
            1 => (
                Vec3::new(0., h * sy, 0.),
                Vec3::new(sx, 0., 0.),
                Vec3::new(0., -sy, 0.),
            ),
            2 => (
                Vec3::new(w * sx, h * sy, 0.),
                Vec3::new(-sx, 0., 0.),
                Vec3::new(0., -sy, 0.),
            ),
            3 => (
                Vec3::new(w * sx, 0., 0.),
                Vec3::new(-sx, 0., 0.),
                Vec3::new(0., sy, 0.),
            ),
            4 => (
                Vec3::default(),
                Vec3::new(sx, 0., 0.),
                Vec3::new(0., sy, 0.),
            ),
            5 => (
                Vec3::new(0., w * sx, 0.),
                Vec3::new(0., -sx, 0.),
                Vec3::new(sy, 0., 0.),
            ),
            6 => (
                Vec3::new(h * sy, w * sx, 0.),
                Vec3::new(0., -sx, 0.),
                Vec3::new(-sy, 0., 0.),
            ),
            7 => (
                Vec3::new(h * sy, 0., 0.),
                Vec3::new(0., sx, 0.),
                Vec3::new(-sy, 0., 0.),
            ),
            _ => (
                Vec3::default(),
                Vec3::new(0., sx, 0.),
                Vec3::new(sy, 0., 0.),
            ),
        };
        Ok((origin, [column, row, Vec3::new(0., 0., sx)]))
    }
}

/// Decode a qualified scalar TIFF stack, retaining ordinary first-page 2D inputs.
///
/// Actual linked directories define slice order. Explicit full-image subtype-zero
/// pages take precedence; otherwise ordinary/page-tagged directories are selected.
/// Reduced-resolution images and masks do not become scalar slices. The source
/// array stays X-fastest; orientation maps its axes into Cartesian visual right/up
/// and Z-up. Inches/centimetres are converted to millimetres; Z spacing uses the
/// original column spacing as the qualified VTK stack heuristic.
/// The caller supplies the input buffer's full reserved capacity as its source
/// allocation charge, including unused capacity retained after reading.
///
/// # Errors
/// Returns an error for malformed/bounded directory chains, unsupported scalar
/// stacks, inconsistent slice metadata, nonfinite samples, allocation/codec or
/// aggregate limits, invalid density surfaces, or a single selected full image
/// that is not the original first directory. The latter cannot use the existing
/// first-page raster fallback without selecting a reduced image incorrectly.
pub(crate) fn scene(bytes: &[u8], source_storage: usize) -> Result<Option<Scene>> {
    if source_storage < bytes.len() {
        return Err("TIFF source allocation charge is smaller than its input length".into());
    }
    if source_storage
        .checked_add(METADATA_STORAGE)
        .is_none_or(|n| n > MAX_STORAGE)
    {
        return Err("TIFF source and metadata exceed the 512 MiB aggregate limit".into());
    }
    let chain = directory::preflight(bytes)?;
    if chain.directories.len() == 1 {
        metadata_axes(&chain.directories[0], 1)?;
        return Ok(None);
    }
    let mut full = Vec::new();
    let mut ordinary = Vec::new();
    for (index, page) in chain.directories.iter().enumerate() {
        match page.unsigned(chain.order, 254, 2)? {
            0 => full.push(index),
            2 => ordinary.push(index),
            1 | 3..=7 => {}
            _ => return Err("unsupported TIFF subfile type".into()),
        }
    }
    let selected = if full.is_empty() { ordinary } else { full };
    if selected.is_empty() {
        return Err("TIFF contains only reduced images or masks".into());
    }
    for &index in &selected {
        metadata_axes(&chain.directories[index], selected.len())?;
    }
    if selected.len() == 1 {
        if selected[0] != 0 {
            return Err(
                "TIFF single selected full-resolution image is not the first directory".into(),
            );
        }
        return Ok(None);
    }
    let profile = Profile::read(&chain.directories[selected[0]], chain.order)?;
    for &index in &selected[1..] {
        if Profile::read(&chain.directories[index], chain.order)? != profile {
            return Err("TIFF selected scalar pages have inconsistent dimensions, types, orientation or resolution".into());
        }
    }
    let values = scalar_values(bytes, source_storage, &chain, &selected, &profile)?;
    let (origin, axes) = profile.frame()?;
    super::volume::density_scene(
        [profile.width, profile.height, selected.len()],
        origin,
        axes,
        values,
    )
    .map(Some)
}

/// Load a scalar TIFF stack, charging the primary buffer's actual reserved capacity.
///
/// # Errors
/// Returns an error for unreadable inputs, malformed/unsupported stacks, resource
/// limits or a single-page raster input without scalar volume geometry.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    scene(&bytes, bytes.capacity())?
        .ok_or_else(|| "TIFF geometry requires a supported multipage scalar stack".into())
}

/// Preflight complete scalar/codec storage, then decode the selected original pages.
fn scalar_values(
    bytes: &[u8],
    source_storage: usize,
    chain: &directory::Chain<'_>,
    selected: &[usize],
    profile: &Profile,
) -> Result<Vec<f64>> {
    let plan = decode_plan(bytes, source_storage, chain, selected, profile)?;
    let mut values = Vec::new();
    values.try_reserve_exact(plan.total)?;
    if values
        .capacity()
        .checked_mul(8)
        .and_then(|capacity| plan.storage.checked_add(capacity - plan.total * 8))
        .is_none_or(|n| n > MAX_STORAGE)
    {
        return Err("TIFF reserved scalar capacity exceeds aggregate allocation limit".into());
    }
    let (source, guard) = stream::source(bytes);
    let mut decoder = Decoder::new(source)?.with_limits(plan.limits);
    let mut selected_index = 0;
    for index in 0..chain.directories.len() {
        if selected.get(selected_index) == Some(&index) {
            if decoder.dimensions()?
                != (
                    u32::try_from(profile.width)?,
                    u32::try_from(profile.height)?,
                )
                || decoder.colortype()? != ColorType::Gray(u8::try_from(profile.bits)?)
            {
                return Err("TIFF codec metadata differs from the preflighted scalar frame".into());
            }
            let ranges = chunks(&chain.directories[index], chain.order, bytes.len())?
                .collect::<Result<Vec<_>>>()?;
            guard.activate(ranges)?;
            let page_result = decoder.read_image();
            guard.clear();
            append(page_result?, plan.per_page, &mut values)?;
            selected_index += 1;
        }
        if index + 1 < chain.directories.len() {
            decoder.next_image()?;
        }
    }
    Ok(values)
}

/// Fully preflighted scalar output and codec allocation limits.
struct DecodePlan {
    /// Exact source samples in each selected page.
    per_page: usize,
    /// Exact scalar samples in the entire selected volume.
    total: usize,
    /// Conservative simultaneous source, metadata, pixel and preview allocation.
    storage: usize,
    /// Codec limits that preserve both pixel and bounded metadata allocations.
    limits: Limits,
}

/// Bound simultaneous allocation and cumulative encoded/padded decode work.
fn decode_plan(
    bytes: &[u8],
    source_storage: usize,
    chain: &directory::Chain<'_>,
    selected: &[usize],
    profile: &Profile,
) -> Result<DecodePlan> {
    let per_page = profile
        .width
        .checked_mul(profile.height)
        .ok_or("TIFF page sample count overflow")?;
    let total = per_page
        .checked_mul(selected.len())
        .ok_or("TIFF volume sample count overflow")?;
    let expanded = total
        .checked_mul(8)
        .ok_or("TIFF expanded sample size overflow")?;
    if total > MAX_SAMPLES || expanded > MAX_EXPANDED {
        return Err("TIFF volume exceeds scalar sample or 128 MiB expanded limits".into());
    }
    let pixels = per_page
        .checked_mul(usize::try_from(profile.bits / 8)?)
        .ok_or("TIFF page buffer size overflow")?;
    let mut intermediate = pixels;
    let mut work = 0_usize;
    for &index in selected {
        let budget = chunk_storage(&chain.directories[index], chain.order, profile, bytes)?;
        intermediate = intermediate.max(budget.storage);
        work = work
            .checked_add(budget.work)
            .ok_or("TIFF cumulative decode work overflow")?;
        if work > MAX_DECODE_WORK {
            return Err(
                "TIFF cumulative encoded and padded decode work exceeds the 512 MiB limit".into(),
            );
        }
    }
    if intermediate > MAX_EXPANDED {
        return Err("TIFF compressed or padded tile buffer exceeds the 128 MiB limit".into());
    }
    let coarse =
        [profile.width, profile.height, selected.len()].map(|n| n.div_ceil(n.div_ceil(32)));
    let geometry = coarse
        .into_iter()
        .try_fold(24 * std::mem::size_of::<Triangle>(), |n, size| {
            n.checked_mul(size).ok_or("TIFF preview storage overflow")
        })?;
    let storage = [
        source_storage,
        METADATA_STORAGE,
        expanded,
        pixels,
        intermediate,
        intermediate,
        geometry,
    ]
    .into_iter()
    .try_fold(0_usize, |n, size| {
        n.checked_add(size)
            .ok_or("TIFF aggregate allocation overflow")
    })?;
    if storage > MAX_STORAGE {
        return Err("TIFF aggregate source, decode and preview allocation limit exceeded".into());
    }
    let mut limits = Limits::default();
    // The codec uses this cap for metadata Value arrays as well as decoded
    // pixels; owned scalar dimensions independently bound the pixel output.
    limits.decoding_buffer_size = pixels.max(METADATA_ALLOCATION);
    limits.intermediate_buffer_size = intermediate;
    limits.ifd_value_size = directory::MAX_VALUE_BYTES;
    Ok(DecodePlan {
        per_page,
        total,
        storage,
        limits,
    })
}

/// Reject declared nonspatial/unaddressed axes without creating metadata-only slices.
fn metadata_axes(page: &Directory<'_>, pages: usize) -> Result<()> {
    if pages > 1 && page.get(330).is_some_and(|entry| entry.count != 0) {
        return Err("TIFF SubIFD pyramids are unsupported as scalar volumes".into());
    }
    let Some(entry) = page.get(270) else {
        return Ok(());
    };
    if entry.kind != 2 {
        return Err("TIFF image description must use ASCII storage".into());
    }
    let text = std::str::from_utf8(entry.bytes)?.trim_end_matches('\0');
    let lower = text.to_ascii_lowercase();
    if lower.contains("<ome") || lower.contains("openmicroscopy.org/schemas/ome") {
        if pages > 1 {
            return Err(
                "TIFF OME channel/time hyperstacks are unsupported as scalar volumes".into(),
            );
        }
        let options = roxmltree::ParsingOptions {
            nodes_limit: 4096,
            ..roxmltree::ParsingOptions::default()
        };
        let document = roxmltree::Document::parse_with_options(text, options)?;
        for pixel in document
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "Pixels")
        {
            for name in ["SizeZ", "SizeT", "SizeC"] {
                if let Some(value) = pixel.attribute(name) {
                    let size = value.parse::<usize>()?;
                    if size != 1 {
                        return Err("TIFF OME metadata declares unsupported unaddressed planes or nonspatial axes".into());
                    }
                }
            }
        }
    }
    if text
        .lines()
        .filter_map(|line| line.split_once('='))
        .any(|(name, _)| name.trim().eq_ignore_ascii_case("imagej"))
    {
        let mut fields = std::collections::HashSet::new();
        for (name, value) in text.lines().filter_map(|line| line.split_once('=')) {
            let name = name.trim().to_ascii_lowercase();
            if !matches!(name.as_str(), "images" | "channels" | "frames" | "slices") {
                continue;
            }
            if !fields.insert(name.clone()) {
                return Err("duplicate TIFF ImageJ axis metadata".into());
            }
            let count = value.trim().parse::<usize>()?;
            if count == 0
                || (matches!(name.as_str(), "channels" | "frames") && count != 1)
                || (matches!(name.as_str(), "images" | "slices") && count != pages)
            {
                return Err("TIFF ImageJ metadata declares nonspatial axes or inconsistent unaddressed planes".into());
            }
        }
    }
    Ok(())
}

/// Append one exact scalar page, rejecting mismatched counts and every nonfinite sample.
fn append(decoded: DecodingResult, expected: usize, output: &mut Vec<f64>) -> Result<()> {
    macro_rules! scalars {
        ($values:expr, $convert:expr) => {{
            if $values.len() != expected {
                return Err("TIFF decoded scalar page length differs from its dimensions".into());
            }
            for value in $values {
                let scalar: f64 = ($convert)(value);
                if !scalar.is_finite() {
                    return Err("TIFF volume contains a nonfinite scalar sample".into());
                }
                output.push(scalar);
            }
        }};
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "Density previews approximate wide integer samples to f64, matching existing volume readers."
    )]
    match decoded {
        DecodingResult::U8(v) => scalars!(v, f64::from),
        DecodingResult::U16(v) => scalars!(v, f64::from),
        DecodingResult::U32(v) => scalars!(v, f64::from),
        DecodingResult::U64(v) => scalars!(v, |n| n as f64),
        DecodingResult::I8(v) => scalars!(v, f64::from),
        DecodingResult::I16(v) => scalars!(v, f64::from),
        DecodingResult::I32(v) => scalars!(v, f64::from),
        DecodingResult::I64(v) => scalars!(v, |n| n as f64),
        DecodingResult::F16(v) => scalars!(v, f64::from),
        DecodingResult::F32(v) => scalars!(v, f64::from),
        DecodingResult::F64(v) => scalars!(v, |n| n),
    }
    Ok(())
}

/// Iterate exact source-bounded declared strip/tile spans without allocating them.
fn chunks<'a>(
    page: &'a Directory<'_>,
    order: Order,
    source_size: usize,
) -> Result<impl Iterator<Item = Result<Range<usize>>> + 'a> {
    let tiled = page.get(324).is_some() || page.get(325).is_some();
    if tiled && (page.get(273).is_some() || page.get(279).is_some()) {
        return Err("TIFF selected page has ambiguous strip and tile storage".into());
    }
    let (offset_tag, count_tag) = if tiled { (324, 325) } else { (273, 279) };
    let offsets = page
        .get(offset_tag)
        .ok_or("TIFF selected page is missing chunk offsets")?;
    let counts = page
        .get(count_tag)
        .ok_or("TIFF selected page is missing chunk byte counts")?;
    if offsets.count == 0
        || offsets.count != counts.count
        || !matches!(offsets.kind, 3 | 4 | 16)
        || !matches!(counts.kind, 3 | 4 | 16)
    {
        return Err("TIFF selected chunk arrays have invalid counts or types".into());
    }
    let offset_width = offsets.bytes.len() / offsets.count;
    let count_width = counts.bytes.len() / counts.count;
    Ok(offsets
        .bytes
        .chunks_exact(offset_width)
        .zip(counts.bytes.chunks_exact(count_width))
        .map(move |(offset, count)| {
            let start = usize::try_from(order.integer(offset))?;
            let length = usize::try_from(order.integer(count))?;
            let end = start
                .checked_add(length)
                .ok_or("TIFF encoded chunk span overflow")?;
            if length == 0 || end > source_size {
                return Err("TIFF selected encoded chunk span exceeds the source".into());
            }
            Ok(start..end)
        }))
}

/// Simultaneous codec storage and cumulative source/decompression work for one page.
struct ChunkBudget {
    /// Largest encoded or padded decoded chunk.
    storage: usize,
    /// Sum of all declared encoded and padded decoded chunk bytes, including aliases.
    work: usize,
}

/// Bound selected strip/tile storage and total expanded work before codec allocation.
fn chunk_storage(
    page: &Directory<'_>,
    order: Order,
    profile: &Profile,
    bytes: &[u8],
) -> Result<ChunkBudget> {
    let mut maximum = 0;
    let mut encoded = 0_usize;
    let mut count = 0_usize;
    for range in chunks(page, order, bytes.len())? {
        let length = range?.len();
        maximum = maximum.max(length);
        encoded = encoded
            .checked_add(length)
            .ok_or("TIFF encoded work overflow")?;
        count += 1;
    }
    let tiled = page.get(324).is_some() || page.get(325).is_some();
    let (columns, rows) = if tiled {
        (
            usize::try_from(page.unsigned(order, 322, 0)?)?,
            usize::try_from(page.unsigned(order, 323, 0)?)?,
        )
    } else {
        (
            profile.width,
            usize::try_from(page.unsigned(order, 278, u64::from(u32::MAX))?)?.min(profile.height),
        )
    };
    if columns == 0
        || rows == 0
        || page.unsigned(order, 32997, 1)? != 1
        || page.unsigned(order, 32998, 1)? != 1
    {
        return Err("unsupported TIFF chunk dimensions or single-directory depth encoding".into());
    }
    let expanded = columns
        .checked_mul(rows)
        .and_then(|n| n.checked_mul(usize::try_from(profile.bits / 8).ok()?))
        .ok_or("TIFF padded chunk buffer overflow")?;
    let work = expanded
        .checked_mul(count)
        .and_then(|decoded| decoded.checked_add(encoded))
        .ok_or("TIFF expanded chunk work overflow")?;
    Ok(ChunkBudget {
        storage: maximum.max(expanded),
        work,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_source_capacity_is_charged_before_metadata_and_pixel_allocation() {
        let bytes = b"II\x2a\0\x08\0\0\0\0\0\0\0\0\0";
        assert!(scene(bytes, bytes.len()).unwrap().is_none());
        assert!(scene(bytes, 0).unwrap_err().to_string().contains("charge"));
        assert!(
            scene(bytes, MAX_STORAGE)
                .unwrap_err()
                .to_string()
                .contains("aggregate")
        );
    }
}
