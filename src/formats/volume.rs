//! Scalar-volume thumbnails as a bounded surface of thresholded density voxels.
//! NRRD: <https://teem.sourceforge.net/nrrd/format.html>
//! MetaImage: <https://docs.itk.org/en/latest/learn/metaio.html>
//! VTI: <https://docs.vtk.org/en/latest/vtk_file_formats/vtkxml_file_format.html>
use crate::{
    Result,
    scene::{Scene, Triangle, Vec3},
};
use base64::Engine;
use flate2::read::{MultiGzDecoder, ZlibDecoder};
use roxmltree::Node;
use std::{collections::HashMap, io::Read, path::Path};

/// Maximum scalar sample count accepted before array expansion.
const MAX_SAMPLES: usize = 16_777_216;
/// Maximum expanded scalar payload bytes.
const MAX_DECODE_BYTES: usize = 128 * 1024 * 1024;
/// Maximum volume header bytes examined before finding its payload.
const MAX_HEADER_BYTES: usize = 1024 * 1024;
/// Maximum density-preview samples along each spatial axis.
const PREVIEW_AXIS: usize = 32;

/// Read a bounded NRRD, `MetaImage` or VTK image volume and decode its preview geometry.
///
/// # Errors
/// Returns an error for unsupported scalar/volume encodings, invalid dimensions or placement,
/// nonfinite samples, malformed payloads or size limits.
pub(super) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    let volume = match super::extension(path)?.as_str() {
        "nrrd" | "nhdr" => nrrd(path, &bytes)?,
        "mhd" | "mha" => metaimage(path, &bytes)?,
        "vti" => vti(&bytes)?,
        _ => return Err("unsupported scalar volume format".into()),
    };
    volume.surface()
}

/// Decode VTK image scalars and extract a bounded density surface.
///
/// # Errors
/// Returns an error for unsupported scalar/volume encodings, invalid dimensions or placement,
/// nonfinite samples, malformed payloads or size limits.
pub(super) fn vti_scene(bytes: &[u8]) -> Result<Scene> {
    vti(bytes)?.surface()
}

#[derive(Clone, Copy, Debug)]
/// Supported scalar storage types for the NRRD, `MetaImage` or VTK image volume.
enum Scalar {
    /// Signed eight-bit integer values.
    I8,
    /// Unsigned eight-bit integer values.
    U8,
    /// Signed 16-bit integer values.
    I16,
    /// Unsigned 16-bit integer values.
    U16,
    /// Signed 32-bit integer values.
    I32,
    /// Unsigned 32-bit integer values.
    U32,
    /// Signed 64-bit integer values.
    I64,
    /// Unsigned 64-bit integer values.
    U64,
    /// IEEE single-precision floating-point values.
    F32,
    /// IEEE double-precision floating-point values.
    F64,
}

impl Scalar {
    /// Resolve NRRD, `MetaImage` and VTK scalar type aliases.
    fn parse(name: &str) -> Result<Self> {
        let name = name.trim().to_ascii_lowercase().replace(' ', "");
        Ok(match name.as_str() {
            "char" | "signedchar" | "int8" | "int8_t" | "met_char" => Self::I8,
            "uchar" | "unsignedchar" | "uint8" | "uint8_t" | "met_uchar" => Self::U8,
            "short" | "shortint" | "signedshort" | "signedshortint" | "int16" | "int16_t"
            | "met_short" => Self::I16,
            "ushort" | "unsignedshort" | "unsignedshortint" | "uint16" | "uint16_t"
            | "met_ushort" => Self::U16,
            "int" | "signedint" | "int32" | "int32_t" | "met_int" => Self::I32,
            "uint" | "unsignedint" | "uint32" | "uint32_t" | "met_uint" => Self::U32,
            "longlong" | "longlongint" | "signedlonglong" | "signedlonglongint" | "int64"
            | "int64_t" | "met_long_long" => Self::I64,
            "ulonglong"
            | "unsignedlonglong"
            | "unsignedlonglongint"
            | "uint64"
            | "uint64_t"
            | "met_ulong_long" => Self::U64,
            "float" | "float32" | "met_float" => Self::F32,
            "double" | "float64" | "met_double" => Self::F64,
            _ => return Err(format!("unsupported scalar volume type {name}").into()),
        })
    }
    /// Return the on-disk byte width of this scalar representation.
    fn width(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            _ => 8,
        }
    }
    /// Parse one ASCII scalar with its declared signedness and numeric width.
    #[allow(
        clippy::cast_precision_loss,
        reason = "Integer scalar fields intentionally approximate to f64 density samples, including 64-bit storage."
    )]
    fn ascii(self, token: &str) -> Result<f64> {
        let value = match self {
            Self::I8 => f64::from(token.parse::<i8>()?),
            Self::U8 => f64::from(token.parse::<u8>()?),
            Self::I16 => f64::from(token.parse::<i16>()?),
            Self::U16 => f64::from(token.parse::<u16>()?),
            Self::I32 => f64::from(token.parse::<i32>()?),
            Self::U32 => f64::from(token.parse::<u32>()?),
            Self::I64 => token.parse::<i64>()? as f64,
            Self::U64 => token.parse::<u64>()? as f64,
            Self::F32 => f64::from(token.parse::<f32>()?),
            Self::F64 => token.parse::<f64>()?,
        };
        finite(value)
    }
    /// Decode one endian-selected scalar and reject nonfinite values.
    #[allow(
        clippy::cast_lossless,
        clippy::cast_precision_loss,
        reason = "The shared endian macro decodes every scalar width; 64-bit integer densities intentionally approximate to f64."
    )]
    fn raw(self, bytes: &[u8], big: bool) -> Result<f64> {
        macro_rules! number {
            ($ty:ty) => {{
                let bytes = bytes.try_into().map_err(|_| "truncated scalar value")?;
                if big {
                    <$ty>::from_be_bytes(bytes) as f64
                } else {
                    <$ty>::from_le_bytes(bytes) as f64
                }
            }};
        }
        finite(match self {
            Self::I8 => f64::from(i8::from_ne_bytes([bytes[0]])),
            Self::U8 => f64::from(bytes[0]),
            Self::I16 => number!(i16),
            Self::U16 => number!(u16),
            Self::I32 => number!(i32),
            Self::U32 => number!(u32),
            Self::I64 => number!(i64),
            Self::U64 => number!(u64),
            Self::F32 => number!(f32),
            Self::F64 => number!(f64),
        })
    }
}

/// Reject nonfinite values before using them as coordinates or scalar samples.
fn finite(value: f64) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err("volume contains a non-finite number".into())
    }
}

/// Normalize one-, two- or three-dimensional volume sizes into a checked three-axis shape.
fn sizes(values: &[usize]) -> Result<[usize; 3]> {
    if !matches!(values.len(), 2 | 3) {
        return Err("volume reader requires two or three spatial dimensions".into());
    }
    let mut result = [1usize; 3];
    result[..values.len()].copy_from_slice(values);
    sample_count(result)?;
    Ok(result)
}

/// Compute the bounded product of the volume dimensions.
fn sample_count(dimensions: [usize; 3]) -> Result<usize> {
    let count = dimensions.iter().try_fold(1usize, |total, &n| {
        if n == 0 {
            return Err("volume dimensions must be positive");
        }
        total.checked_mul(n).ok_or("volume sample count overflow")
    })?;
    if count > MAX_SAMPLES {
        return Err("volume exceeds the 16 million sample limit".into());
    }
    Ok(count)
}

/// Compute the expected scalar byte length within the decoded-volume limit.
fn payload_size(ty: Scalar, count: usize) -> Result<usize> {
    let bytes = count
        .checked_mul(ty.width())
        .ok_or("volume byte count overflow")?;
    if bytes > MAX_DECODE_BYTES {
        return Err("volume exceeds the 128 MiB decoded payload limit".into());
    }
    Ok(bytes)
}

/// Decode an exact-length raw scalar payload with the selected byte order.
fn raw_values(bytes: &[u8], ty: Scalar, count: usize, big: bool) -> Result<Vec<f64>> {
    let expected = payload_size(ty, count)?;
    let data = bytes
        .get(..expected)
        .ok_or("truncated volume scalar payload")?;
    data.chunks_exact(ty.width())
        .map(|chunk| ty.raw(chunk, big))
        .collect()
}

/// Parse the declared ASCII samples and optionally require exact token consumption.
fn ascii_values(bytes: &[u8], ty: Scalar, count: usize, exact: bool) -> Result<Vec<f64>> {
    payload_size(ty, count)?;
    let mut tokens = std::str::from_utf8(bytes)?.split_whitespace();
    let values = (0..count)
        .map(|_| ty.ascii(tokens.next().ok_or("truncated ASCII scalar payload")?))
        .collect::<Result<_>>()?;
    if exact && tokens.next().is_some() {
        return Err("ASCII scalar count disagrees with dimensions".into());
    }
    Ok(values)
}

/// Read exactly the expected decompressed payload without accepting excess output.
fn unpack<R: Read>(reader: R, expected: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(expected as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() != expected {
        return Err("decompressed volume size disagrees with dimensions".into());
    }
    Ok(bytes)
}

/// Parse whitespace-separated finite header numbers.
fn numbers(text: &str) -> Result<Vec<f64>> {
    text.split_whitespace()
        .map(|s| finite(s.parse()?))
        .collect()
}

/// Require exactly three finite header components for a spatial vector.
fn vector(values: &[f64]) -> Result<Vec3> {
    match values {
        [x, y] => Ok(Vec3::new(*x, *y, 0.)),
        [x, y, z] => Ok(Vec3::new(*x, *y, *z)),
        _ => Err("volume vector needs two or three components".into()),
    }
}

/// Decode row- or column-major direction metadata and reject singular axes.
fn direction_matrix(values: &[f64], dimensions: usize, columns: bool) -> Result<[Vec3; 3]> {
    if values.len() != dimensions * dimensions {
        return Err("volume direction matrix has the wrong size".into());
    }
    let mut result = [Vec3::default(); 3];
    for axis in 0..dimensions {
        let mut v = [0f64; 3];
        for component in 0..dimensions {
            v[component] = values[if columns {
                axis * dimensions + component
            } else {
                component * dimensions + axis
            }];
        }
        result[axis] = Vec3::new(v[0], v[1], v[2]);
    }
    if dimensions == 2 {
        result[2] = result[0].cross(result[1]).normalized();
    }
    Ok(result)
}

/// Return the Cartesian unit axes used when no orientation is supplied.
fn default_axes() -> [Vec3; 3] {
    [
        Vec3::new(1., 0., 0.),
        Vec3::new(0., 1., 0.),
        Vec3::new(0., 0., 1.),
    ]
}

/// Finite scalar lattice with an origin and oriented spatial axes.
struct Volume {
    /// Number of scalar samples along each axis.
    dimensions: [usize; 3],
    /// World-space lattice origin.
    origin: Vec3,
    /// World-space step vectors, including orientation and spacing.
    axes: [Vec3; 3],
    /// Scalar samples in X-fastest lattice order.
    values: Vec<f64>,
}

/// Extract a bounded density surface from regularly spaced scalar samples.
///
/// # Errors
/// Returns an error for empty/oversized dimensions, a mismatched or nonfinite scalar array,
/// invalid spatial axes/origin, or degenerate surface geometry.
pub(super) fn density_scene(
    dimensions: [usize; 3],
    origin: Vec3,
    axes: [Vec3; 3],
    values: Vec<f64>,
) -> Result<Scene> {
    Volume {
        dimensions,
        origin,
        axes,
        values,
    }
    .surface()
}

/// Extract a density surface using explicit nonuniform coordinates on each axis.
///
/// # Errors
/// Returns an error for invalid scalar dimensions/values, nonfinite or nonmonotonic coordinate
/// axes, inconsistent axis lengths, or invalid surface geometry.
pub(super) fn density_rectilinear_scene(
    coordinates: &[Vec<f64>; 3],
    values: Vec<f64>,
) -> Result<Scene> {
    let dimensions = coordinates.each_ref().map(std::vec::Vec::len);
    Volume {
        dimensions,
        origin: Vec3::new(0., 0., 0.),
        axes: default_axes(),
        values,
    }
    .surface_with_coordinates(Some(coordinates))
}

impl Volume {
    /// Reduce scalar samples to a bounded preview lattice and extract occupied boundary faces.
    fn surface(self) -> Result<Scene> {
        self.surface_with_coordinates(None)
    }

    /// Max-pool density while retaining occupied cavities and explicit axis coordinates.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        reason = "Max-pool bin indices are bounded by validated dimensions and PREVIEW_AXIS before native/f64 conversion."
    )]
    #[allow(
        clippy::too_many_lines,
        reason = "Keep max-pooling, coordinate resampling and exposed-face construction together to preserve cavity boundaries."
    )]
    fn surface_with_coordinates(self, coordinates: Option<&[Vec<f64>; 3]>) -> Result<Scene> {
        if self.values.len() != sample_count(self.dimensions)? {
            return Err("volume scalar count disagrees with dimensions".into());
        }
        if !self.origin.is_finite()
            || self
                .axes
                .iter()
                .any(|axis| !axis.is_finite() || axis.length() == 0.)
        {
            return Err("volume geometry has invalid origin or directions".into());
        }
        let mut determinant = self.axes[0]
            .normalized()
            .dot(self.axes[1].normalized().cross(self.axes[2].normalized()));
        if let Some(coordinates) = coordinates {
            for axis in coordinates {
                if axis.iter().any(|v| !v.is_finite()) {
                    return Err("rectilinear volume coordinates must be finite".into());
                }
                if axis.len() > 1 {
                    let increasing = axis[1] > axis[0];
                    if axis.windows(2).any(|w| {
                        if increasing {
                            w[1] <= w[0]
                        } else {
                            w[1] >= w[0]
                        }
                    }) {
                        return Err(
                            "rectilinear volume coordinates must be strictly monotonic".into()
                        );
                    }
                    if !increasing {
                        determinant = -determinant;
                    }
                }
            }
        }
        if !determinant.is_finite() || determinant.abs() < 1e-12 {
            return Err("volume directions form a singular transform".into());
        }
        let (mut minimum, mut maximum) = (f64::INFINITY, f64::NEG_INFINITY);
        for &value in &self.values {
            finite(value)?;
            minimum = minimum.min(value);
            maximum = maximum.max(value);
        }
        if maximum == 0. && minimum == 0. {
            return Err("volume contains no nonzero density".into());
        }
        let threshold = if maximum > 0. && minimum < 0. {
            maximum * 0.5
        } else {
            minimum * 0.5 + maximum * 0.5
        };
        let strides = self.dimensions.map(|n| n.div_ceil(PREVIEW_AXIS));
        let coarse =
            std::array::from_fn::<_, 3, _>(|axis| self.dimensions[axis].div_ceil(strides[axis]));
        let index = |p: [usize; 3], dims: [usize; 3]| p[0] + dims[0] * (p[1] + dims[1] * p[2]);
        let mut occupied = vec![false; coarse.iter().product()];
        for z in 0..self.dimensions[2] {
            for y in 0..self.dimensions[1] {
                for x in 0..self.dimensions[0] {
                    if self.values[index([x, y, z], self.dimensions)] >= threshold {
                        occupied[index([x / strides[0], y / strides[1], z / strides[2]], coarse)] =
                            true;
                    }
                }
            }
        }
        let world = |p: [f64; 3]| {
            let p = if let Some(coordinates) = coordinates {
                std::array::from_fn(|axis| {
                    let values = &coordinates[axis];
                    if values.len() == 1 {
                        return values[0] + p[axis];
                    }
                    let lower = (p[axis].floor().max(0.) as usize).min(values.len() - 2);
                    values[lower] + (p[axis] - lower as f64) * (values[lower + 1] - values[lower])
                })
            } else {
                p
            };
            self.origin + self.axes[0] * p[0] + self.axes[1] * p[1] + self.axes[2] * p[2]
        };
        let mut scene = Scene::default();
        for z in 0..coarse[2] {
            for y in 0..coarse[1] {
                for x in 0..coarse[0] {
                    let p = [x, y, z];
                    if !occupied[index(p, coarse)] {
                        continue;
                    }
                    let low = std::array::from_fn::<_, 3, _>(|axis| {
                        (p[axis] * strides[axis]) as f64 - 0.5
                    });
                    let high = std::array::from_fn::<_, 3, _>(|axis| {
                        ((p[axis] + 1) * strides[axis]).min(self.dimensions[axis]) as f64 - 0.5
                    });
                    for axis in 0..3 {
                        for upper in [false, true] {
                            let neighbor = if upper {
                                p[axis].checked_add(1).filter(|&n| n < coarse[axis])
                            } else {
                                p[axis].checked_sub(1)
                            };
                            if let Some(next) = neighbor {
                                let mut other = p;
                                other[axis] = next;
                                if occupied[index(other, coarse)] {
                                    continue;
                                }
                            }
                            let other: Vec<_> = (0..3).filter(|&a| a != axis).collect();
                            let mut corners = Vec::new();
                            for (a, b) in
                                [(false, false), (true, false), (true, true), (false, true)]
                            {
                                let mut v = low;
                                v[axis] = if upper { high[axis] } else { low[axis] };
                                v[other[0]] = if a { high[other[0]] } else { low[other[0]] };
                                v[other[1]] = if b { high[other[1]] } else { low[other[1]] };
                                corners.push(world(v));
                            }
                            if (upper == (axis == 1)) != (determinant < 0.) {
                                corners.reverse();
                            }
                            for indices in [[0, 1, 2], [0, 2, 3]] {
                                scene.triangles.push(Triangle {
                                    vertices: indices.map(|i| corners[i]),
                                    color: None,
                                });
                            }
                        }
                    }
                }
            }
        }
        scene.validate()?;
        Ok(scene)
    }
}

/// Require a named volume header field.
fn required<'a>(fields: &'a HashMap<String, String>, name: &str) -> Result<&'a str> {
    fields
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("missing volume header {name}").into())
}

/// Store a unique normalized header field and reject duplicates.
fn insert(fields: &mut HashMap<String, String>, name: &str, value: &str) -> Result<()> {
    if fields
        .insert(name.to_owned(), value.trim().to_owned())
        .is_some()
    {
        return Err(format!("duplicate volume header field {name}").into());
    }
    Ok(())
}

/// Read an adjacent payload after validating its relative path and byte budget.
fn detached(path: &Path, filename: &str, header_len: usize) -> Result<Vec<u8>> {
    let filename = filename.trim().trim_matches('"');
    if filename.starts_with("LIST") || filename.contains('%') {
        return Err("volume filename lists and patterns are not implemented".into());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let resource = super::relative_resource(parent, filename)?;
    let data = super::read_file(&resource)?;
    if header_len as u64 + data.len() as u64 > super::MAX_INPUT_BYTES {
        return Err("volume header and payload exceed the aggregate input limit".into());
    }
    Ok(data)
}

/// Parse parenthesized NRRD axis vectors.
fn paren_vectors(text: &str) -> Result<Vec<Vec3>> {
    let mut text = text.trim();
    let mut vectors = Vec::new();
    while !text.is_empty() {
        let rest = text
            .strip_prefix('(')
            .ok_or("volume spatial direction needs a parenthesized vector")?;
        let end = rest.find(')').ok_or("unterminated volume spatial vector")?;
        let values = rest[..end]
            .split(',')
            .map(|s| finite(s.trim().parse()?))
            .collect::<Result<Vec<_>>>()?;
        vectors.push(vector(&values)?);
        text = rest[end + 1..].trim_start();
    }
    Ok(vectors)
}

/// Apply declared line and byte skips without leaving the payload bounds.
fn skip_payload(
    data: &[u8],
    lines: usize,
    skip: i64,
    expected: usize,
    backwards: bool,
) -> Result<&[u8]> {
    if skip == -1 && backwards {
        return data
            .get(
                data.len()
                    .checked_sub(expected)
                    .ok_or("truncated volume payload")?..,
            )
            .ok_or_else(|| "invalid volume payload offset".into());
    }
    if skip < 0 {
        return Err("negative volume byte skip requires uncompressed binary data".into());
    }
    let mut data = data;
    for _ in 0..lines {
        data = data
            .get(
                data.iter()
                    .position(|&b| b == b'\n')
                    .ok_or("truncated volume skipped lines")?
                    + 1..,
            )
            .ok_or("invalid volume line skip")?;
    }
    data.get(usize::try_from(skip)?..)
        .ok_or_else(|| "volume byte skip exceeds payload size".into())
}

/// Decode NRRD metadata, scalar encodings, detached data and spatial axis vectors.
#[allow(
    clippy::too_many_lines,
    reason = "Keep NRRD header, detached-payload and encoding checks together with the declared scalar layout."
)]
fn nrrd(path: &Path, bytes: &[u8]) -> Result<Volume> {
    let mut offset = 0usize;
    let mut fields = HashMap::new();
    let mut first = true;
    let mut separator = false;
    while offset < bytes.len() {
        let end = bytes[offset..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |n| offset + n);
        if end > MAX_HEADER_BYTES {
            return Err("NRRD header exceeds the 1 MiB limit".into());
        }
        let line = std::str::from_utf8(&bytes[offset..end])?.trim_end_matches('\r');
        offset = (end + 1).min(bytes.len());
        if first {
            if !matches!(
                line,
                "NRRD0001" | "NRRD0002" | "NRRD0003" | "NRRD0004" | "NRRD0005" | "NRRD00.01"
            ) {
                return Err("unsupported or missing NRRD magic".into());
            }
            first = false;
            continue;
        }
        if line.is_empty() {
            separator = true;
            break;
        }
        if line.starts_with('#') || line.contains(":=") {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or("invalid NRRD header field")?;
        let name = match name.trim().to_ascii_lowercase().as_str() {
            "datafile" => "data file".to_owned(),
            "lineskip" => "line skip".to_owned(),
            "byteskip" => "byte skip".to_owned(),
            other => other.to_owned(),
        };
        insert(&mut fields, &name, value)?;
    }
    let dimension: usize = required(&fields, "dimension")?.parse()?;
    let dims = required(&fields, "sizes")?
        .split_whitespace()
        .map(str::parse)
        .collect::<std::result::Result<Vec<usize>, _>>()?;
    if dims.len() != dimension {
        return Err("NRRD dimension disagrees with sizes".into());
    }
    let dimensions = sizes(&dims)?;
    if let Some(kinds) = fields.get("kinds")
        && (kinds.split_whitespace().count() != dimension
            || kinds
                .split_whitespace()
                .any(|s| !matches!(s.to_ascii_lowercase().as_str(), "domain" | "space")))
    {
        return Err("NRRD non-spatial axes are not implemented".into());
    }
    for field in ["axis mins", "axis maxs"] {
        if let Some(values) = fields.get(field)
            && values
                .split_whitespace()
                .any(|s| s.parse::<f64>().map_or(true, |v| !v.is_nan()))
        {
            return Err(
                "NRRD axis min/max placement is not implemented; use space origin/directions"
                    .into(),
            );
        }
    }
    let mut axes = default_axes();
    if let Some(directions) = fields.get("space directions") {
        let values = paren_vectors(directions)?;
        if values.len() != dimension {
            return Err("NRRD spatial direction count disagrees with dimension".into());
        }
        axes[..dimension].copy_from_slice(&values);
        if dimension == 2 {
            axes[2] = axes[0].cross(axes[1]).normalized();
        }
    } else if let Some(spacings) = fields.get("spacings") {
        let spacings = numbers(spacings)?;
        if spacings.len() != dimension {
            return Err("NRRD spacing count disagrees with dimension".into());
        }
        for axis in 0..dimension {
            axes[axis] = axes[axis] * spacings[axis];
        }
    }
    let origin = fields
        .get("space origin")
        .map(|text| -> Result<Vec3> {
            let values = paren_vectors(text)?;
            if values.len() != 1 {
                return Err("invalid NRRD space origin".into());
            }
            Ok(values[0])
        })
        .transpose()?
        .unwrap_or_default();
    let ty = Scalar::parse(required(&fields, "type")?)?;
    let count = sample_count(dimensions)?;
    let expected = payload_size(ty, count)?;
    let encoding = required(&fields, "encoding")?.to_ascii_lowercase();
    let ascii = matches!(encoding.as_str(), "ascii" | "text" | "txt");
    let endian = fields.get("endian").map(|s| s.to_ascii_lowercase());
    let big = match endian.as_deref() {
        Some("big") => true,
        Some("little") => false,
        None if ascii || ty.width() == 1 => false,
        None => return Err("NRRD binary multibyte scalars require endian".into()),
        _ => return Err("invalid NRRD endian".into()),
    };
    let external;
    let data = if let Some(filename) = fields.get("data file") {
        external = detached(path, filename, bytes.len())?;
        external.as_slice()
    } else {
        if !separator {
            return Err("attached NRRD is missing its header separator".into());
        }
        &bytes[offset..]
    };
    let lines: usize = fields
        .get("line skip")
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(0);
    let skip: i64 = fields
        .get("byte skip")
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(0);
    if matches!(encoding.as_str(), "gzip" | "gz") && skip != 0 {
        return Err("NRRD byte skipping inside gzip data is not implemented".into());
    }
    let data = skip_payload(data, lines, skip, expected, encoding == "raw")?;
    let values = match encoding.as_str() {
        "raw" => raw_values(data, ty, count, big)?,
        "ascii" | "text" | "txt" => ascii_values(data, ty, count, false)?,
        "gzip" | "gz" => raw_values(
            &unpack(MultiGzDecoder::new(data), expected)?,
            ty,
            count,
            big,
        )?,
        "hex" => {
            let hex: Vec<_> = data
                .iter()
                .copied()
                .filter(|b| !b.is_ascii_whitespace())
                .take(expected * 2 + 1)
                .collect();
            if hex.len() < expected * 2 {
                return Err("truncated NRRD hex payload".into());
            }
            let raw = hex[..expected * 2]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    let text = std::str::from_utf8(pair)?;
                    Ok(u8::from_str_radix(text, 16)?)
                })
                .collect::<Result<Vec<u8>>>()?;
            raw_values(&raw, ty, count, big)?
        }
        _ => return Err(format!("unsupported NRRD encoding {encoding}").into()),
    };
    Ok(Volume {
        dimensions,
        origin,
        axes,
        values,
    })
}

/// Parse the exact `MetaImage` boolean spellings.
fn boolean(text: &str) -> Result<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("invalid MetaImage boolean".into()),
    }
}

/// Decode `MetaImage` local/detached scalars, byte order, compression and column-major placement.
#[allow(
    clippy::too_many_lines,
    reason = "Keep MetaImage header termination and local/detached scalar layout validation together."
)]
fn metaimage(path: &Path, bytes: &[u8]) -> Result<Volume> {
    let mut fields = HashMap::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = bytes[offset..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |n| offset + n);
        if end > MAX_HEADER_BYTES {
            return Err("MetaImage header exceeds the 1 MiB limit".into());
        }
        let line = std::str::from_utf8(&bytes[offset..end])?.trim();
        offset = (end + 1).min(bytes.len());
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .ok_or("invalid MetaImage header field")?;
        let name = match name.trim().to_ascii_lowercase().as_str() {
            "position" | "origin" => "offset".to_owned(),
            "orientation" | "rotation" => "transformmatrix".to_owned(),
            "elementbyteordermsb" => "binarydatabyteordermsb".to_owned(),
            name => name.to_owned(),
        };
        let done = name == "elementdatafile";
        insert(&mut fields, &name, value)?;
        if done {
            break;
        }
    }
    if fields
        .get("objecttype")
        .is_some_and(|s| !s.eq_ignore_ascii_case("image"))
    {
        return Err("MetaImage reader only accepts Image objects".into());
    }
    let dimension: usize = required(&fields, "ndims")?.parse()?;
    let dims = required(&fields, "dimsize")?
        .split_whitespace()
        .map(str::parse)
        .collect::<std::result::Result<Vec<usize>, _>>()?;
    if dims.len() != dimension {
        return Err("MetaImage dimension disagrees with DimSize".into());
    }
    let dimensions = sizes(&dims)?;
    if fields
        .get("elementnumberofchannels")
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(1)
        != 1
    {
        return Err("MetaImage vector/color channels are not implemented".into());
    }
    let mut axes = if let Some(matrix) = fields.get("transformmatrix") {
        direction_matrix(&numbers(matrix)?, dimension, true)?
    } else {
        default_axes()
    };
    if let Some(spacing) = fields
        .get("elementspacing")
        .or_else(|| fields.get("elementsize"))
    {
        let values = numbers(spacing)?;
        if values.len() != dimension {
            return Err("MetaImage spacing count disagrees with dimension".into());
        }
        for axis in 0..dimension {
            axes[axis] = axes[axis] * values[axis];
        }
    }
    let origin = fields
        .get("offset")
        .map(|s| {
            let values = numbers(s)?;
            if values.len() != dimension {
                return Err("MetaImage origin count disagrees with dimension".into());
            }
            vector(&values)
        })
        .transpose()?
        .unwrap_or_default();
    let ty = Scalar::parse(required(&fields, "elementtype")?)?;
    let count = sample_count(dimensions)?;
    let expected = payload_size(ty, count)?;
    let binary = fields
        .get("binarydata")
        .map(|s| boolean(s))
        .transpose()?
        .unwrap_or(true);
    let compressed = fields
        .get("compresseddata")
        .map(|s| boolean(s))
        .transpose()?
        .unwrap_or(false);
    if compressed && !binary {
        return Err("compressed MetaImage requires binary data".into());
    }
    let big = fields
        .get("binarydatabyteordermsb")
        .map(|s| boolean(s))
        .transpose()?
        .unwrap_or(cfg!(target_endian = "big"));
    let filename = required(&fields, "elementdatafile")?;
    let external;
    let data = if filename.eq_ignore_ascii_case("LOCAL") {
        &bytes[offset..]
    } else {
        external = detached(path, filename, bytes.len())?;
        external.as_slice()
    };
    let skip: i64 = fields
        .get("headersize")
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(0);
    let mut data = skip_payload(data, 0, skip, expected, binary && !compressed)?;
    if compressed && let Some(size) = fields.get("compresseddatasize") {
        let size: usize = size.parse()?;
        data = data
            .get(..size)
            .ok_or("truncated MetaImage compressed payload")?;
    }
    let values = if compressed {
        raw_values(&unpack(ZlibDecoder::new(data), expected)?, ty, count, big)?
    } else if binary {
        raw_values(data, ty, count, big)?
    } else {
        ascii_values(data, ty, count, false)?
    };
    Ok(Volume {
        dimensions,
        origin,
        axes,
        values,
    })
}

/// Require a direct VTK image XML child.
fn xml_child<'a, 'b>(node: Node<'a, 'b>, name: &str) -> Result<Node<'a, 'b>> {
    node.children()
        .find(|n| n.has_tag_name(name))
        .ok_or_else(|| format!("missing VTI {name}").into())
}

/// Read ordered VTK image extents with exactly six integer values.
fn xml_extent(node: Node<'_, '_>, name: &str) -> Result<[i64; 6]> {
    let values = node
        .attribute(name)
        .ok_or("missing VTI extent")?
        .split_whitespace()
        .map(str::parse)
        .collect::<std::result::Result<Vec<i64>, _>>()?;
    values
        .try_into()
        .map_err(|_| "VTI extent requires six integers".into())
}

/// Decode bounded inline/appended VTK base64 segments.
fn base64_bytes(text: &str) -> Result<Vec<u8>> {
    if text.len() > (MAX_DECODE_BYTES + 8 * MAX_HEADER_BYTES).div_ceil(3) * 4 {
        return Err("VTI binary array exceeds decoded payload limit".into());
    }
    let bytes: Vec<_> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut result = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'=' {
            i += 1;
            if bytes.get(i) == Some(&b'=') {
                i += 1;
            }
            result.extend(base64::engine::general_purpose::STANDARD.decode(&bytes[start..i])?);
            start = i;
        } else {
            i += 1;
        }
    }
    if start < bytes.len() {
        result.extend(base64::engine::general_purpose::STANDARD.decode(&bytes[start..])?);
    }
    Ok(result)
}

/// Expand VTK image numeric payloads with size headers and optional multiblock zlib.
fn vtk_binary(
    data: &[u8],
    expected: usize,
    header: usize,
    big: bool,
    compressor: Option<&str>,
) -> Result<Vec<u8>> {
    let word = |offset: usize| -> Result<usize> {
        let bytes = data
            .get(offset..offset + header)
            .ok_or("truncated VTI binary header")?;
        let n = if header == 8 {
            let a = bytes.try_into()?;
            if big {
                u64::from_be_bytes(a)
            } else {
                u64::from_le_bytes(a)
            }
        } else {
            let a = bytes.try_into()?;
            if big {
                u64::from(u32::from_be_bytes(a))
            } else {
                u64::from(u32::from_le_bytes(a))
            }
        };
        Ok(usize::try_from(n)?)
    };
    if compressor.is_none() {
        if word(0)? != expected {
            return Err("VTI binary length disagrees with dimensions".into());
        }
        return Ok(data
            .get(header..header + expected)
            .ok_or("truncated VTI binary scalar payload")?
            .to_vec());
    }
    if compressor != Some("vtkZLibDataCompressor") {
        return Err("unsupported VTI compressor".into());
    }
    let blocks = word(0)?;
    let size = word(header)?;
    let last = word(header * 2)?;
    if blocks == 0 || blocks > MAX_HEADER_BYTES || size == 0 || last > size {
        return Err("invalid VTI compression block header".into());
    }
    let total = (blocks - 1)
        .checked_mul(size)
        .and_then(|n| n.checked_add(if last == 0 { size } else { last }))
        .ok_or("VTI decompressed size overflow")?;
    if total != expected {
        return Err("VTI decompressed size disagrees with dimensions".into());
    }
    let mut offset = (blocks + 3)
        .checked_mul(header)
        .ok_or("VTI header size overflow")?;
    let mut result = Vec::new();
    for block in 0..blocks {
        let length = word((block + 3) * header)?;
        let end = offset
            .checked_add(length)
            .ok_or("VTI compressed block size overflow")?;
        let data = data
            .get(offset..end)
            .ok_or("truncated VTI compressed block")?;
        let count = if block + 1 == blocks && last != 0 {
            last
        } else {
            size
        };
        let mut reader = ZlibDecoder::new(data);
        result.extend(unpack(&mut reader, count)?);
        if reader.total_in() != data.len() as u64 {
            return Err("VTI compressed block contains trailing bytes".into());
        }
        offset = end;
    }
    Ok(result)
}

/// Decode static VTK `ImageData` point/cell scalars and their extent, origin and direction.
#[allow(
    clippy::cast_precision_loss,
    reason = "VTK image extents are checked before their lattice offsets become f64 coordinates."
)]
#[allow(
    clippy::too_many_lines,
    reason = "Keep VTK image extents, scalar association and binary-array validation together."
)]
fn vti(bytes: &[u8]) -> Result<Volume> {
    let doc = roxmltree::Document::parse(std::str::from_utf8(bytes)?)?;
    let root = doc.root_element();
    if !root.has_tag_name("VTKFile") || root.attribute("type") != Some("ImageData") {
        return Err("expected a VTI ImageData document".into());
    }
    let grid = xml_child(root, "ImageData")?;
    let extent = xml_extent(grid, "WholeExtent")?;
    let mut dims = [0usize; 3];
    for axis in 0..3 {
        let n = extent[axis * 2 + 1]
            .checked_sub(extent[axis * 2])
            .and_then(|n| n.checked_add(1))
            .ok_or("VTI extent overflow")?;
        dims[axis] = usize::try_from(n)?;
    }
    sample_count(dims)?;
    let mut pieces = grid.children().filter(|n| n.has_tag_name("Piece"));
    let piece = pieces.next().ok_or("VTI has no piece")?;
    if pieces.next().is_some() || xml_extent(piece, "Extent")? != extent {
        return Err("VTI reader requires one piece covering WholeExtent".into());
    }
    let mut axes = if let Some(matrix) = grid.attribute("Direction") {
        direction_matrix(&numbers(matrix)?, 3, false)?
    } else {
        default_axes()
    };
    let spacing = numbers(grid.attribute("Spacing").unwrap_or("1 1 1"))?;
    if spacing.len() != 3 {
        return Err("VTI spacing requires three values".into());
    }
    for axis in 0..3 {
        axes[axis] = axes[axis] * spacing[axis];
    }
    let origin_values = numbers(grid.attribute("Origin").unwrap_or("0 0 0"))?;
    if origin_values.len() != 3 {
        return Err("VTI origin requires three values".into());
    }
    let mut origin = vector(&origin_values)?;
    for axis in 0..3 {
        origin = origin + axes[axis] * extent[axis * 2] as f64;
    }
    let mut selection = None;
    for (tag, cell) in [("PointData", false), ("CellData", true)] {
        let Some(parent) = piece.children().find(|n| n.has_tag_name(tag)) else {
            continue;
        };
        let name = parent.attribute("Scalars");
        let array = parent.children().find(|n| {
            n.has_tag_name("DataArray")
                && name.map_or(
                    n.attribute("NumberOfComponents").unwrap_or("1") == "1",
                    |s| n.attribute("Name") == Some(s),
                )
        });
        if name.is_some() && array.is_none() {
            return Err("VTI active scalar array is missing".into());
        }
        if let Some(array) = array {
            selection = Some((array, cell));
            break;
        }
    }
    let (array, cell) = selection.ok_or("VTI has no scalar density array")?;
    if array.attribute("NumberOfComponents").unwrap_or("1") != "1"
        || array.attribute("TimeStep").is_some()
    {
        return Err("VTI vector/time scalar arrays are not implemented".into());
    }
    for ghost in piece
        .descendants()
        .filter(|n| n.has_tag_name("DataArray") && n.attribute("Name") == Some("vtkGhostType"))
    {
        if ghost.attribute("format").unwrap_or("ascii") != "ascii"
            || ghost
                .text()
                .unwrap_or("")
                .split_whitespace()
                .any(|s| s != "0")
        {
            return Err("VTI ghost or blanked voxels are not implemented".into());
        }
    }
    if cell {
        for axis in 0..3 {
            if dims[axis] > 1 {
                dims[axis] -= 1;
                origin = origin + axes[axis] * 0.5;
            }
        }
    }
    let count = sample_count(dims)?;
    let ty = Scalar::parse(
        array
            .attribute("type")
            .ok_or("VTI scalar type is missing")?,
    )?;
    let expected = payload_size(ty, count)?;
    let big = match root.attribute("byte_order").unwrap_or("LittleEndian") {
        "LittleEndian" => false,
        "BigEndian" => true,
        _ => return Err("invalid VTI byte order".into()),
    };
    let values = match array.attribute("format").unwrap_or("ascii") {
        "ascii" => ascii_values(array.text().unwrap_or("").as_bytes(), ty, count, true)?,
        format @ ("binary" | "appended") => {
            let text = if format == "binary" {
                array.text().unwrap_or("")
            } else {
                let appended = xml_child(root, "AppendedData")?;
                if appended.attribute("encoding").unwrap_or("base64") != "base64" {
                    return Err("VTI raw appended bytes are not implemented".into());
                }
                let text = appended
                    .text()
                    .unwrap_or("")
                    .trim_start()
                    .strip_prefix('_')
                    .ok_or("VTI appended data is missing its underscore")?;
                let offset: usize = array
                    .attribute("offset")
                    .ok_or("VTI appended array offset is missing")?
                    .parse()?;
                let next = root
                    .descendants()
                    .filter(|n| {
                        n.has_tag_name("DataArray") && n.attribute("format") == Some("appended")
                    })
                    .filter_map(|n| n.attribute("offset"))
                    .map(str::parse::<usize>)
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .into_iter()
                    .filter(|&n| n > offset)
                    .min()
                    .unwrap_or(text.len());
                text.get(offset..next)
                    .ok_or("VTI appended offset is out of bounds")?
            };
            let header = match root.attribute("header_type").unwrap_or("UInt32") {
                "UInt32" => 4,
                "UInt64" => 8,
                _ => return Err("unsupported VTI header word type".into()),
            };
            let raw = vtk_binary(
                &base64_bytes(text)?,
                expected,
                header,
                big,
                root.attribute("compressor"),
            )?;
            raw_values(&raw, ty, count, big)?
        }
        _ => return Err("unsupported VTI scalar encoding".into()),
    };
    Ok(Volume {
        dimensions: dims,
        origin,
        axes,
        values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{
        Compression,
        write::{GzEncoder, ZlibEncoder},
    };
    use std::io::Write;

    fn density() -> Vec<u8> {
        let mut v = vec![0; 27];
        v[13] = 10;
        v
    }
    fn bounds(scene: &Scene) -> [(f64, f64); 3] {
        std::array::from_fn(|axis| {
            scene
                .positions()
                .map(|p| [p.x, p.y, p.z][axis])
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                    (a.min(v), b.max(v))
                })
        })
    }
    fn compressed(raw: &[u8], gzip: bool) -> Vec<u8> {
        if gzip {
            let mut e = GzEncoder::new(Vec::new(), Compression::fast());
            e.write_all(raw).unwrap();
            e.finish().unwrap()
        } else {
            let mut e = ZlibEncoder::new(Vec::new(), Compression::fast());
            e.write_all(raw).unwrap();
            e.finish().unwrap()
        }
    }
    fn nrrd_bytes(encoding: &str, ty: &str, endian: &str, payload: &[u8]) -> Vec<u8> {
        let header = format!(
            "NRRD0005\ntype: {ty}\ndimension: 3\nsizes: 3 3 3\nencoding: {encoding}\nendian: {endian}\nspace directions: (0,2,0) (-3,0,0) (0,0,-4)\nspace origin: (10,20,30)\n\n"
        );
        [header.as_bytes(), payload].concat()
    }
    fn vti_document(association: &str, array: &str, attributes: &str, appended: &str) -> Vec<u8> {
        format!(r#"<VTKFile type="ImageData" {attributes}><ImageData WholeExtent="0 2 0 2 0 2" Origin="10 20 30" Spacing="2 3 4" Direction="0 -1 0 1 0 0 0 0 -1"><Piece Extent="0 2 0 2 0 2"><{association} Scalars="density">{array}</{association}></Piece></ImageData>{appended}</VTKFile>"#).into_bytes()
    }

    #[test]
    fn nonuniform_density_emits_content_faces_with_reflections_and_spacing() {
        let values = density().into_iter().map(f64::from).collect();
        let scene = Volume {
            dimensions: [3; 3],
            origin: Vec3::new(10., 20., 30.),
            axes: [
                Vec3::new(0., 2., 0.),
                Vec3::new(-3., 0., 0.),
                Vec3::new(0., 0., -4.),
            ],
            values,
        }
        .surface()
        .unwrap();
        assert_eq!(scene.triangles.len(), 12);
        assert_eq!(bounds(&scene), [(5.5, 8.5), (21., 23.), (24., 28.)]);
        let center = Vec3::new(7., 22., 26.);
        let mut area = 0.;
        for t in &scene.triangles {
            let normal = (t.vertices[1] - t.vertices[0]).cross(t.vertices[2] - t.vertices[0]);
            assert!(normal.dot((t.vertices[0] + t.vertices[1] + t.vertices[2]) / 3. - center) > 0.);
            area += normal.length() * 0.5;
        }
        assert_eq!(area, 52.);
        assert!(crate::render::render(&scene, &crate::render::RenderOptions::default()).is_ok());
    }

    #[test]
    fn max_pooling_preserves_a_small_bright_feature_and_bounds_output() {
        let mut values = vec![0.; 65];
        values[64] = 1.;
        let scene = Volume {
            dimensions: [65, 1, 1],
            origin: Vec3::default(),
            axes: default_axes(),
            values,
        }
        .surface()
        .unwrap();
        assert_eq!(scene.triangles.len(), 12);
        assert_eq!(bounds(&scene), [(62.5, 64.5), (-0.5, 0.5), (-0.5, 0.5)]);
        let values = (0..32usize.pow(3))
            .map(|n| {
                if (n % 32 + n / 32 % 32 + n / 1024) % 2 == 0 {
                    1.
                } else {
                    0.
                }
            })
            .collect();
        let scene = Volume {
            dimensions: [32; 3],
            origin: Vec3::default(),
            axes: default_axes(),
            values,
        }
        .surface()
        .unwrap();
        assert!(scene.triangles.len() < 400_000);
        assert!(
            Volume {
                dimensions: [1; 3],
                origin: Vec3::default(),
                axes: default_axes(),
                values: vec![0.]
            }
            .surface()
            .is_err()
        );
    }

    #[test]
    fn hollow_density_shell_contains_both_outer_and_inner_surface_geometry() {
        let mut values = Vec::new();
        for z in 0..32 {
            for y in 0..32 {
                for x in 0..32 {
                    let squared = (f64::from(x) - 16.).powi(2)
                        + (f64::from(y) - 16.).powi(2)
                        + (f64::from(z) - 16.).powi(2);
                    values.push(if (25. ..=81.).contains(&squared) {
                        100.
                    } else {
                        -1000.
                    });
                }
            }
        }
        let scene = Volume {
            dimensions: [32; 3],
            origin: Vec3::default(),
            axes: default_axes(),
            values,
        }
        .surface()
        .unwrap();
        assert_eq!(bounds(&scene), [(6.5, 25.5); 3]);
        assert!(scene.triangles.len() > 1000 && scene.triangles.len() < 10000);
        let center = Vec3::new(16., 16., 16.);
        let mut inner = 0usize;
        let mut outer = 0usize;
        for triangle in &scene.triangles {
            let centroid =
                (triangle.vertices[0] + triangle.vertices[1] + triangle.vertices[2]) / 3.;
            let radius = (centroid - center).length();
            assert!(radius > 4. && radius < 10.);
            if radius < 6. {
                inner += 1;
            } else {
                outer += 1;
            }
        }
        assert!(inner > 100 && outer > 100);
        let thumbnail = crate::render::render(
            &scene,
            &crate::render::RenderOptions {
                width: 128,
                height: 128,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(thumbnail.pixels().filter(|p| p[3] > 0).count() > 1000);
    }

    #[test]
    #[allow(
        clippy::cast_precision_loss,
        reason = "This test intentionally compares parser output with the same documented i64-to-f64 scalar approximation."
    )]
    fn scalar_types_respect_binary_endianness_and_ascii_ranges() {
        for (ty, big, bytes, expected) in [
            (Scalar::I8, false, vec![0xfe], -2.),
            (Scalar::U8, true, vec![255], 255.),
            (Scalar::I16, true, (-1234i16).to_be_bytes().to_vec(), -1234.),
            (Scalar::U16, false, 50000u16.to_le_bytes().to_vec(), 50000.),
            (
                Scalar::I32,
                false,
                (-1_234_567i32).to_le_bytes().to_vec(),
                -1_234_567.,
            ),
            (
                Scalar::U32,
                true,
                4_000_000_000u32.to_be_bytes().to_vec(),
                4_000_000_000.,
            ),
            (
                Scalar::I64,
                true,
                i64::MIN.to_be_bytes().to_vec(),
                i64::MIN as f64,
            ),
            (
                Scalar::U64,
                false,
                u64::MAX.to_le_bytes().to_vec(),
                u64::MAX as f64,
            ),
            (Scalar::F32, true, (-2.5f32).to_be_bytes().to_vec(), -2.5),
            (Scalar::F64, false, 3.25f64.to_le_bytes().to_vec(), 3.25),
        ] {
            assert_eq!(raw_values(&bytes, ty, 1, big).unwrap(), vec![expected]);
        }
        assert!(ascii_values(b"256", Scalar::U8, 1, true).is_err());
        assert!(raw_values(&f32::NAN.to_le_bytes(), Scalar::F32, 1, false).is_err());
        assert!(raw_values(&[0], Scalar::U16, 1, false).is_err());
    }

    #[test]
    fn nrrd_raw_ascii_hex_and_gzip_preserve_real_density() {
        use std::fmt::Write as _;
        let raw = density()
            .iter()
            .flat_map(|&v| u16::from(v).to_be_bytes())
            .collect::<Vec<_>>();
        let ascii = density()
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        let mut hex = String::new();
        for value in &raw {
            write!(hex, "{value:02x} ").unwrap();
        }
        for (encoding, ty, payload) in [
            ("raw", "ushort", raw.clone()),
            ("gzip", "ushort", compressed(&raw, true)),
            ("ascii", "uint16", ascii.into_bytes()),
            ("hex", "unsigned short", hex.into_bytes()),
        ] {
            let scene = nrrd(
                Path::new("unused.nrrd"),
                &nrrd_bytes(encoding, ty, "big", &payload),
            )
            .unwrap()
            .surface()
            .unwrap();
            assert_eq!(scene.triangles.len(), 12);
            assert_eq!(bounds(&scene), [(5.5, 8.5), (21., 23.), (24., 28.)]);
        }
    }

    #[test]
    fn metaimage_local_zlib_big_endian_and_column_major_orientation() {
        let raw = density()
            .iter()
            .flat_map(|&v| u16::from(v).to_be_bytes())
            .collect::<Vec<_>>();
        for compressed_data in [false, true] {
            let payload = if compressed_data {
                compressed(&raw, false)
            } else {
                raw.clone()
            };
            let header = format!(
                "ObjectType = Image\nNDims = 3\nDimSize = 3 3 3\nElementType = MET_USHORT\nElementSpacing = 2 3 4\nTransformMatrix = 0 1 0 -1 0 0 0 0 -1\nOffset = 10 20 30\nBinaryDataByteOrderMSB = True\nCompressedData = {compressed_data}\nElementDataFile = LOCAL\n"
            );
            let scene = metaimage(
                Path::new("unused.mha"),
                &[header.as_bytes(), &payload].concat(),
            )
            .unwrap()
            .surface()
            .unwrap();
            assert_eq!(bounds(&scene), [(5.5, 8.5), (21., 23.), (24., 28.)]);
        }
        let text = "NDims = 2\nDimSize = 2 2\nElementType = MET_FLOAT\nBinaryData = False\nElementDataFile = LOCAL\n0 0 0 1";
        let scene = metaimage(Path::new("unused.mha"), text.as_bytes())
            .unwrap()
            .surface()
            .unwrap();
        assert_eq!(bounds(&scene), [(0.5, 1.5), (0.5, 1.5), (-0.5, 0.5)]);
    }

    #[test]
    fn vti_ascii_point_and_cell_density_apply_extent_and_direction() {
        let values = density()
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        let array = format!(
            r#"<DataArray type="UInt8" Name="density" format="ascii">{values}</DataArray>"#
        );
        let scene = vti(&vti_document("PointData", &array, "", ""))
            .unwrap()
            .surface()
            .unwrap();
        assert_eq!(bounds(&scene), [(5.5, 8.5), (21., 23.), (24., 28.)]);
        let shifted = String::from_utf8(vti_document("PointData", &array, "", ""))
            .unwrap()
            .replace("0 2 0 2 0 2", "-1 1 -1 1 -1 1");
        assert_eq!(
            bounds(&vti(shifted.as_bytes()).unwrap().surface().unwrap()),
            [(8.5, 11.5), (19., 21.), (28., 32.)]
        );
        let cell = String::from_utf8(vti_document("CellData", &array, "", ""))
            .unwrap()
            .replace("0 2 0 2 0 2", "0 3 0 3 0 3");
        assert_eq!(
            bounds(&vti(cell.as_bytes()).unwrap().surface().unwrap()),
            [(4., 7.), (22., 24.), (22., 26.)]
        );
    }

    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "The authored VTK array payload fits every tested 32- or 64-bit header width."
    )]
    fn vti_inline_binary_and_multiblock_zlib_headers_decode() {
        let raw = density()
            .iter()
            .flat_map(|&v| u16::from(v).to_be_bytes())
            .collect::<Vec<_>>();
        let plain = [(raw.len() as u64).to_be_bytes().as_slice(), &raw].concat();
        let encoded = base64::engine::general_purpose::STANDARD.encode(plain);
        let array = format!(
            r#"<DataArray type="UInt16" Name="density" format="binary">{encoded}</DataArray>"#
        );
        let scene = vti(&vti_document(
            "PointData",
            &array,
            r#"byte_order="BigEndian" header_type="UInt64""#,
            "",
        ))
        .unwrap()
        .surface()
        .unwrap();
        assert_eq!(scene.triangles.len(), 12);
        let parts = [compressed(&raw[..32], false), compressed(&raw[32..], false)];
        let header = [2u32, 32, 22, parts[0].len() as u32, parts[1].len() as u32]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        let encoded = base64::engine::general_purpose::STANDARD.encode(header)
            + &base64::engine::general_purpose::STANDARD.encode(parts.concat());
        let array = format!(
            r#"<DataArray type="UInt16" Name="density" format="binary">{encoded}</DataArray>"#
        );
        let scene = vti(&vti_document(
            "PointData",
            &array,
            r#"byte_order="BigEndian" compressor="vtkZLibDataCompressor""#,
            "",
        ))
        .unwrap()
        .surface()
        .unwrap();
        assert_eq!(bounds(&scene), [(5.5, 8.5), (21., 23.), (24., 28.)]);
    }

    #[test]
    fn vti_appended_offsets_count_encoded_stream_bytes() {
        let prefix = base64::engine::general_purpose::STANDARD
            .encode([3u32.to_le_bytes().as_slice(), &[1, 2, 3]].concat());
        let payload = base64::engine::general_purpose::STANDARD
            .encode([27u32.to_le_bytes().as_slice(), &density()].concat());
        let array = format!(
            r#"<DataArray type="UInt8" Name="ignored" format="appended" offset="0"/><DataArray type="UInt8" Name="density" format="appended" offset="{}"/>"#,
            prefix.len()
        );
        let appended =
            format!(r#"<AppendedData encoding="base64">_{prefix}{payload}</AppendedData>"#);
        let doc = vti_document("PointData", &array, "", &appended);
        assert_eq!(vti(&doc).unwrap().surface().unwrap().triangles.len(), 12);
        let bad = String::from_utf8(doc)
            .unwrap()
            .replace(&format!("offset=\"{}\"", prefix.len()), "offset=\"999999\"");
        assert!(vti(bad.as_bytes()).is_err());
    }

    #[test]
    fn detached_payloads_skip_headers_and_reject_traversal() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "catwalk-volume-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("field.raw"),
            [b"junk\nextra".as_slice(), &density()].concat(),
        )
        .unwrap();
        let nrrd = "NRRD0005\ntype: uint8\ndimension: 3\nsizes: 3 3 3\nencoding: raw\nline skip: 1\nbyte skip: 5\ndata file: field.raw\n";
        let path = dir.join("field.nhdr");
        std::fs::write(&path, nrrd).unwrap();
        assert_eq!(load(&path).unwrap().triangles.len(), 12);
        let header = "NDims = 3\nDimSize = 3 3 3\nElementType = MET_UCHAR\nHeaderSize = -1\nElementDataFile = field.raw\n";
        let path = dir.join("field.mhd");
        std::fs::write(&path, header).unwrap();
        assert_eq!(load(&path).unwrap().triangles.len(), 12);
        for filename in [
            "../field.raw",
            "/tmp/field.raw",
            "https://example.com/field.raw",
            "LIST",
            "slice%03d.raw 1 3 1",
        ] {
            std::fs::write(&path, header.replace("field.raw", filename)).unwrap();
            assert!(load(&path).is_err());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_nonfinite_oversized_and_unsupported_volumes_fail() {
        let valid = nrrd_bytes("raw", "uint8", "little", &density());
        let header = std::str::from_utf8(&valid[..valid.len() - 27]).unwrap();
        for bad in [
            header.replace("sizes: 3 3 3", "sizes: 0 3 3"),
            header.replace("sizes: 3 3 3", "sizes: 9999999999 9999999999 3"),
            header.replace("dimension: 3", "dimension: 4"),
            header.replace("encoding: raw", "encoding: bzip2"),
            header.replace("type: uint8", "type: block"),
            header.replace("endian: little", "endian: invalid"),
            header.replace("(0,2,0)", "(0,0,0)"),
            header.replace("(10,20,30)", "(NaN,20,30)"),
            header.replace("dimension: 3", "dimension: 3\ndimension: 3"),
        ] {
            assert!(
                nrrd(
                    Path::new("invalid.nrrd"),
                    &[bad.as_bytes(), &density()].concat()
                )
                .and_then(Volume::surface)
                .is_err()
            );
        }
        let ascii = density()
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        let array = format!(r#"<DataArray type="UInt8" Name="density">{ascii}</DataArray>"#);
        let doc = String::from_utf8(vti_document("PointData", &array, "", "")).unwrap();
        for bad in [
            doc.replace("Scalars=\"density\"", "Scalars=\"missing\""),
            doc.replace("type=\"UInt8\"", "type=\"Float64\"")
                .replace(">0 0 0 ", ">NaN 0 0 "),
            doc.replace("Spacing=\"2 3 4\"", "Spacing=\"0 3 4\""),
            doc.replace("0 2 0 2 0 2", "2 0 0 2 0 2"),
            doc.replace(
                "</ImageData>",
                "<Piece Extent=\"0 2 0 2 0 2\"/></ImageData>",
            ),
        ] {
            assert!(vti(bad.as_bytes()).and_then(Volume::surface).is_err());
        }
        assert!(vtk_binary(&[0; 16], 27, 4, false, Some("vtkZLibDataCompressor")).is_err());
        assert!(vtk_binary(&[0; 16], 27, 4, false, Some("vtkLZMADataCompressor")).is_err());
        assert!(metaimage(Path::new("invalid.mha"),b"NDims = 3\nDimSize = 3 3 3\nElementType = MET_UCHAR\nElementDataFile = LOCAL\nshort").is_err());
    }
}
