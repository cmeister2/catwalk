//! Bounded VTK XML binary payload handling, following vtkXMLWriter's stream layout.
use crate::Result;
use base64::Engine;
use flate2::read::ZlibDecoder;
use roxmltree::Node;
use std::cell::Cell;
use std::io::Read;

/// Maximum decoded VTK XML payload bytes.
#[allow(
    clippy::cast_possible_truncation,
    reason = "The shared 512 MiB input cap fits usize on every supported 32- or 64-bit target."
)]
const MAX_BYTES: usize = super::super::MAX_INPUT_BYTES as usize;
/// Maximum zlib blocks accepted in one VTK XML array.
const MAX_BLOCKS: usize = 1_000_000;

#[derive(Clone, Copy)]
/// Supported scalar storage types for the VTK XML numeric array.
pub(super) enum Scalar {
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
    /// Resolve the supported VTK XML and legacy numeric type names.
    ///
    /// # Errors
    /// Returns an error if the name does not identify a supported integer or floating-point
    /// storage type.
    pub(super) fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "Int8" | "char" | "signed_char" => Self::I8,
            "UInt8" | "unsigned_char" => Self::U8,
            "Int16" | "short" => Self::I16,
            "UInt16" | "unsigned_short" => Self::U16,
            "Int32" | "int" | "vtkidtype" => Self::I32,
            "UInt32" | "unsigned_int" => Self::U32,
            "Int64" | "long_long" | "vtktypeint64" => Self::I64,
            "UInt64" | "unsigned_long_long" | "vtktypeuint64" => Self::U64,
            "Float32" | "float" => Self::F32,
            "Float64" | "double" => Self::F64,
            _ => return Err(format!("unsupported VTK numeric type {name}").into()),
        })
    }
    /// Return the on-disk byte width of this scalar representation.
    pub(super) fn width(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::I64 | Self::U64 | Self::F64 => 8,
        }
    }
    /// Decode an endian-selected integer word without routing connectivity through floating point.
    ///
    /// # Errors
    /// Returns an error for an invalid word width, a floating-point connectivity type, or
    /// an unsigned 64-bit value outside the signed index range.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "Payload decoding selects the declared word width; narrowing preserves the stored signed integer bit pattern."
    )]
    pub(super) fn integer(self, data: &[u8], big: bool) -> Result<i64> {
        let word = unsigned(data, big)?;
        Ok(match self {
            Self::I8 => i64::from(word as i8),
            Self::U8 => i64::from(word as u8),
            Self::I16 => i64::from(word as i16),
            Self::U16 => i64::from(word as u16),
            Self::I32 => i64::from(word as i32),
            Self::U32 => i64::from(word as u32),
            Self::I64 => word as i64,
            Self::U64 => i64::try_from(word)?,
            Self::F32 | Self::F64 => return Err("VTK connectivity needs an integer type".into()),
        })
    }
    /// Decode a finite numeric word for coordinates or scalar data.
    ///
    /// # Errors
    /// Returns an error for an invalid word width, an integer conversion outside the
    /// supported range, or a nonfinite decoded value.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "Numeric scalar samples intentionally use f64; the Float32 decoder selects a four-byte source word."
    )]
    pub(super) fn float(self, data: &[u8], big: bool) -> Result<f64> {
        let value = match self {
            Self::F32 => f64::from(f32::from_bits(unsigned(data, big)? as u32)),
            Self::F64 => f64::from_bits(unsigned(data, big)?),
            Self::U64 => unsigned(data, big)? as f64,
            _ => self.integer(data, big)? as f64,
        };
        if !value.is_finite() {
            return Err("VTK coordinate is not finite".into());
        }
        Ok(value)
    }
}

/// Assemble a one-, two-, four- or eight-byte unsigned word in the selected byte order.
fn unsigned(data: &[u8], big: bool) -> Result<u64> {
    if !matches!(data.len(), 1 | 2 | 4 | 8) {
        return Err("invalid VTK numeric word size".into());
    }
    Ok(if big {
        data.iter().fold(0u64, |n, &b| (n << 8) | u64::from(b))
    } else {
        data.iter()
            .rev()
            .fold(0u64, |n, &b| (n << 8) | u64::from(b))
    })
}

/// Raw appended bytes are outside XML's character rules. Parse only the XML envelope.
///
/// # Errors
/// Returns an error for invalid UTF-8/XML tags, an unsupported appended-data encoding, or
/// missing, misplaced or unclosed raw-payload markers.
pub(super) fn xml_envelope(bytes: &[u8]) -> Result<(String, Option<&[u8]>)> {
    let start = bytes.windows(13).position(|s| s == b"<AppendedData");
    if let Some(start) = start {
        let open_end = bytes[start..]
            .iter()
            .position(|&b| b == b'>')
            .map(|n| n + start + 1)
            .ok_or("truncated VTK AppendedData tag")?;
        let tag = std::str::from_utf8(&bytes[start..open_end])?;
        let opening = format!("<Root>{tag}</AppendedData></Root>");
        let doc = roxmltree::Document::parse(&opening)?;
        let encoding = doc
            .root_element()
            .first_element_child()
            .and_then(|n| n.attribute("encoding"));
        if encoding == Some("raw") {
            let close = bytes
                .windows(15)
                .rposition(|s| s == b"</AppendedData>")
                .ok_or("missing VTK AppendedData closing tag")?;
            let marker = bytes
                .get(open_end..close)
                .ok_or("VTK AppendedData closing tag precedes its opening tag")?
                .iter()
                .position(|b| !b.is_ascii_whitespace())
                .map(|n| n + open_end)
                .ok_or("VTK appended data is missing its underscore")?;
            if bytes[marker] != b'_' {
                return Err("VTK appended data is missing its underscore".into());
            }
            let mut text = std::str::from_utf8(&bytes[..open_end])?.to_owned();
            text.push('_');
            text.push_str(std::str::from_utf8(&bytes[close..])?);
            return Ok((text, Some(&bytes[marker + 1..close])));
        }
    }
    Ok((
        std::str::from_utf8(bytes)?
            .trim_start_matches('\u{feff}')
            .to_owned(),
        None,
    ))
}

/// VTK XML array decoding metadata and cumulative payload accounting.
pub(super) struct XmlData<'a> {
    /// Whether binary words use big-endian byte order.
    big: bool,
    /// Byte width of VTK length/compression header words.
    header: usize,
    /// Declared supported compressor name, if any.
    compressor: Option<&'a str>,
    /// Borrowed appended numeric payload, raw or base64.
    appended: Option<&'a [u8]>,
    /// Whether appended offsets address encoded rather than raw bytes.
    encoded: bool,
    /// Declared appended-array offsets used to determine slice boundaries.
    offsets: Vec<usize>,
    /// Expanded array bytes charged across this XML document.
    decoded: Cell<usize>,
}
impl<'a> XmlData<'a> {
    /// Read XML byte-order, header-width, compressor and appended-data metadata.
    ///
    /// # Errors
    /// Returns an error for an unsupported byte order, header width or compressor,
    /// inconsistent appended-data metadata, or invalid array offsets.
    pub(super) fn new(root: Node<'a, '_>, raw: Option<&'a [u8]>) -> Result<Self> {
        let big = match root.attribute("byte_order").unwrap_or("LittleEndian") {
            "LittleEndian" => false,
            "BigEndian" => true,
            _ => return Err("invalid VTK XML byte order".into()),
        };
        let header = match root.attribute("header_type").unwrap_or("UInt32") {
            "UInt32" => 4,
            "UInt64" => 8,
            _ => return Err("unsupported VTK XML header type".into()),
        };
        let compressor = root.attribute("compressor").filter(|s| !s.is_empty());
        if compressor
            .is_some_and(|s| !matches!(s, "vtkZLibDataCompressor" | "vtkLZ4DataCompressor"))
        {
            return Err(
                "VTK XML compressor is not implemented (zlib and LZ4 are supported)".into(),
            );
        }
        let mut appended_nodes = root.children().filter(|n| n.has_tag_name("AppendedData"));
        let appended_node = appended_nodes.next();
        if appended_nodes.next().is_some() {
            return Err("duplicate VTK AppendedData section".into());
        }
        let (appended, encoded) = if let Some(node) = appended_node {
            match node.attribute("encoding").unwrap_or("base64") {
                "base64" => {
                    if raw.is_some() {
                        return Err("VTK raw envelope encoding disagrees with XML".into());
                    }
                    let text = node
                        .text()
                        .unwrap_or("")
                        .trim_start()
                        .strip_prefix('_')
                        .ok_or("VTK appended data is missing its underscore")?;
                    (Some(text.as_bytes()), true)
                }
                "raw" => (
                    Some(raw.ok_or("VTK raw appended payload is missing")?),
                    false,
                ),
                _ => return Err("unsupported VTK appended encoding".into()),
            }
        } else {
            if raw.is_some() {
                return Err("VTK raw envelope has no AppendedData element".into());
            }
            (None, false)
        };
        let mut offsets = Vec::new();
        for array in root
            .descendants()
            .filter(|n| n.has_tag_name("DataArray") && n.attribute("format") == Some("appended"))
        {
            offsets.push(
                array
                    .attribute("offset")
                    .ok_or("VTK appended offset is missing")?
                    .parse::<usize>()?,
            );
            if offsets.len() > super::MAX_ITEMS {
                return Err("VTK array count exceeds limit".into());
            }
        }
        offsets.sort_unstable();
        offsets.dedup();
        if offsets
            .last()
            .is_some_and(|&n| appended.is_none_or(|data| n > data.len()))
        {
            return Err("VTK appended offset is out of bounds".into());
        }
        Ok(Self {
            big,
            header,
            compressor,
            appended,
            encoded,
            offsets,
            decoded: Cell::new(0),
        })
    }
    /// Resolve inline or appended bytes and enforce tuple sizes and per-array limits.
    fn payload(&self, node: Node<'_, '_>, limit: usize) -> Result<(Scalar, Vec<u8>)> {
        let ty = Scalar::parse(
            node.attribute("type")
                .ok_or("VTK binary array type is missing")?,
        )?;
        let maximum = limit
            .checked_mul(ty.width())
            .ok_or("VTK array length overflow")?
            .min(MAX_BYTES);
        let (data, encoded) = match node.attribute("format") {
            Some("binary") => (node.text().unwrap_or("").as_bytes(), true),
            Some("appended") => {
                let data = self.appended.ok_or("VTK appended array has no payload")?;
                let offset = node
                    .attribute("offset")
                    .ok_or("VTK appended offset is missing")?
                    .parse::<usize>()?;
                let next = self
                    .offsets
                    .get(self.offsets.partition_point(|&n| n <= offset))
                    .copied()
                    .unwrap_or(data.len());
                (
                    data.get(offset..next)
                        .ok_or("VTK appended offset is out of bounds")?,
                    self.encoded,
                )
            }
            _ => return Err("unsupported VTK array encoding".into()),
        };
        let decoded;
        let data = if encoded {
            decoded = decode_base64(data)?;
            decoded.as_slice()
        } else {
            data
        };
        let (payload, consumed) = binary(data, maximum, self.header, self.big, self.compressor)?;
        if !data[consumed..].iter().all(u8::is_ascii_whitespace) {
            return Err("VTK binary array contains trailing data".into());
        }
        if payload.len() % ty.width() != 0 {
            return Err("VTK binary array has a partial numeric word".into());
        }
        let total = self
            .decoded
            .get()
            .checked_add(payload.len())
            .ok_or("VTK decoded byte count overflow")?;
        if total > MAX_BYTES {
            return Err("VTK decoded arrays exceed aggregate 512 MiB limit".into());
        }
        self.decoded.set(total);
        Ok((ty, payload))
    }
    /// Decode the declared components of a finite XML numeric array.
    ///
    /// # Errors
    /// Returns an error for malformed scalar/component declarations, invalid
    /// ASCII/base64/binary data, nonfinite samples, inconsistent payload sizes or
    /// decoded-byte limits.
    pub(super) fn floats(&self, node: Node<'_, '_>, limit: usize) -> Result<Vec<f64>> {
        reject_time(node)?;
        if node.attribute("format").unwrap_or("ascii") == "ascii" {
            let mut values = Vec::new();
            for token in node.text().unwrap_or("").split_whitespace() {
                if values.len() == limit {
                    return Err("VTK coordinate array exceeds count limit".into());
                }
                let n: f64 = token.parse()?;
                if !n.is_finite() {
                    return Err("VTK coordinate is not finite".into());
                }
                values.push(n);
            }
            return Ok(values);
        }
        let (ty, data) = self.payload(node, limit)?;
        data.chunks_exact(ty.width())
            .map(|word| ty.float(word, self.big))
            .collect()
    }
    /// Decode XML connectivity words while preserving integer range checks.
    ///
    /// # Errors
    /// Returns an error for floating-point connectivity, malformed scalar declarations or
    /// integer payloads, invalid component counts, decompression failures or byte limits.
    pub(super) fn integers(&self, node: Node<'_, '_>, limit: usize) -> Result<Vec<i64>> {
        reject_time(node)?;
        if node.attribute("format").unwrap_or("ascii") == "ascii" {
            let mut values = Vec::new();
            for token in node.text().unwrap_or("").split_whitespace() {
                if values.len() == limit {
                    return Err("VTK integer array exceeds count limit".into());
                }
                values.push(token.parse()?);
            }
            return Ok(values);
        }
        let (ty, data) = self.payload(node, limit)?;
        data.chunks_exact(ty.width())
            .map(|word| ty.integer(word, self.big))
            .collect()
    }
    /// Convert decoded XML integer values into nonnegative native indices.
    ///
    /// # Errors
    /// Returns an error if numeric array decoding fails, an index is negative, or a decoded
    /// index cannot fit the native index type.
    pub(super) fn indices(&self, node: Node<'_, '_>, limit: usize) -> Result<Vec<usize>> {
        self.integers(node, limit)?
            .into_iter()
            .map(|n| {
                let n = usize::try_from(n)?;
                if n > super::MAX_ITEMS {
                    return Err("VTK index exceeds geometry limit".into());
                }
                Ok(n)
            })
            .collect()
    }
}
/// Reject arrays with time-step selection that the static preview cannot interpret.
fn reject_time(node: Node<'_, '_>) -> Result<()> {
    if node.attribute("TimeStep").is_some() {
        return Err("VTK time-dependent geometry arrays are not implemented".into());
    }
    Ok(())
}

/// Decode VTK base64 segments while bounding aggregate output bytes.
fn decode_base64(text: &[u8]) -> Result<Vec<u8>> {
    if text.len() > MAX_BYTES.div_ceil(3) * 4 + MAX_BLOCKS * 16 {
        return Err("VTK base64 payload exceeds byte limit".into());
    }
    let text: Vec<_> = text
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    let mut result = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'=' {
            i += 1;
            if text.get(i) == Some(&b'=') {
                i += 1;
            }
            result.extend(base64::engine::general_purpose::STANDARD.decode(&text[start..i])?);
            if result.len() > MAX_BYTES {
                return Err("VTK base64 decoded payload exceeds byte limit".into());
            }
            start = i;
        } else {
            i += 1;
        }
    }
    if start < text.len() {
        result.extend(base64::engine::general_purpose::STANDARD.decode(&text[start..])?);
    }
    if result.len() > MAX_BYTES {
        return Err("VTK base64 decoded payload exceeds byte limit".into());
    }
    Ok(result)
}
/// Expand VTK length-prefixed or multiblock zlib array payloads and validate their declared sizes.
fn binary(
    data: &[u8],
    limit: usize,
    header: usize,
    big: bool,
    compressor: Option<&str>,
) -> Result<(Vec<u8>, usize)> {
    let word = |offset: usize| -> Result<usize> {
        let end = offset
            .checked_add(header)
            .ok_or("VTK header offset overflow")?;
        Ok(usize::try_from(unsigned(
            data.get(offset..end).ok_or("truncated VTK binary header")?,
            big,
        )?)?)
    };
    if compressor.is_none() {
        let count = word(0)?;
        if count > limit {
            return Err("VTK binary array exceeds declared geometry count limit".into());
        }
        let end = header
            .checked_add(count)
            .ok_or("VTK binary length overflow")?;
        return Ok((
            data.get(header..end)
                .ok_or("truncated VTK binary payload")?
                .to_vec(),
            end,
        ));
    }
    let blocks = word(0)?;
    let size = word(header)?;
    let last = word(2 * header)?;
    if blocks > MAX_BLOCKS
        || (blocks != 0 && size == 0)
        || last > size
        || (blocks == 0 && last != 0)
    {
        return Err("invalid VTK compressed block header".into());
    }
    let total = if blocks == 0 {
        0
    } else {
        (blocks - 1)
            .checked_mul(size)
            .and_then(|n| n.checked_add(if last == 0 { size } else { last }))
            .ok_or("VTK decompressed length overflow")?
    };
    if total > limit {
        return Err("VTK decompressed array exceeds geometry count limit".into());
    }
    let mut offset = (blocks + 3)
        .checked_mul(header)
        .ok_or("VTK block header size overflow")?;
    if offset > data.len() {
        return Err("truncated VTK compressed header".into());
    }
    let mut result = Vec::with_capacity(total);
    for i in 0..blocks {
        let length = word((i + 3) * header)?;
        let end = offset
            .checked_add(length)
            .ok_or("VTK compressed block size overflow")?;
        let block = data
            .get(offset..end)
            .ok_or("truncated VTK compressed block")?;
        let count = if i + 1 == blocks && last != 0 {
            last
        } else {
            size
        };
        match compressor {
            Some("vtkZLibDataCompressor") => {
                let mut decoder = ZlibDecoder::new(block);
                let before = result.len();
                decoder
                    .by_ref()
                    .take(count as u64 + 1)
                    .read_to_end(&mut result)?;
                if result.len() - before != count || decoder.total_in() != block.len() as u64 {
                    return Err("VTK compressed block length disagrees with header".into());
                }
            }
            Some("vtkLZ4DataCompressor") => {
                let before = result.len();
                result.resize(before + count, 0);
                let decoded = lz4_flex::block::decompress_into(block, &mut result[before..])?;
                if decoded != count {
                    return Err("VTK LZ4 block length disagrees with header".into());
                }
            }
            _ => return Err("unsupported VTK compressor".into()),
        }
        offset = end;
    }
    Ok((result, offset))
}
