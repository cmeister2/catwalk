//! Extract the exact saved PNG from qualified Lychee Mango/ASAR containers.
//!
//! Real Lychee 5.4.3 scenes qualify flat manifest version `3.1.0`. Legacy ZIP,
//! linked/unpacked resources, nested directories and other versions are outside
//! this reader's scope. Scene geometry and preparation settings are not opened.

use crate::{Result, formats};
use image::{ImageFormat, ImageReader, RgbaImage};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io::Cursor, path::Path};

/// Largest accepted UTF-8 member index before JSON allocation.
const MAX_MANIFEST: usize = 2 * 1024 * 1024;
/// Largest supported member count in the flat file index.
const MAX_MEMBERS: usize = 4096;
/// Largest JSON nesting depth admitted by the manifest preflight.
const MAX_DEPTH: usize = 8;
/// Maximum cumulative JSON values before allocating the parsed manifest.
const MAX_NODES: usize = 50_000;
/// Maximum encoded JSON string length, including delimiters and escapes.
const MAX_STRING: usize = 4096;
/// Maximum encoded PNG selected from the container.
const MAX_PREVIEW: usize = 8 * 1024 * 1024;
/// Maximum decoded image dimension on either axis.
const MAX_DIMENSION: u32 = 4096;
/// Maximum decoded image pixel count.
const MAX_PIXELS: u64 = 16_000_000;
/// Exact root member qualified as the document's saved image.
const PREVIEW: &str = "preview.png";

/// Extract a saved PNG at its original dimensions from a qualified `.lys` file.
///
/// Flat Mango/ASAR manifest version `3.1.0` is qualified by original Lychee
/// 5.4.3 scenes. Declared whole and block SHA-256 hashes are checked before
/// decoding; integrity metadata can be absent. A valid container without
/// `preview.png` returns `None`;
/// malformed, ambiguous or unsupported carriers return an error. No external
/// file is followed and no scene geometry is regenerated.
///
/// # Errors
///
/// Returns an error for file reads, malformed or unsupported Pickle/JSON
/// metadata, duplicate names, invalid or overlapping member spans, failed
/// integrity checks, PNG failures, or exceeded source, manifest and image limits.
pub fn extract(path: &Path) -> Result<Option<RgbaImage>> {
    extract_bytes(&formats::read_file(path)?)
}

/// Validate the carrier and decode only the addressed, integrity-checked PNG.
fn extract_bytes(data: &[u8]) -> Result<Option<RgbaImage>> {
    let container = Container::parse(data)?;
    let Some(png) = container.member(
        PREVIEW,
        MAX_PREVIEW,
        "Lychee saved PNG exceeds the 8 MiB limit",
    )?
    else {
        return Ok(None);
    };
    decode_png(png).map(Some)
}

/// Validated flat Mango index shared by saved-image and stored-model readers.
/// Member bodies remain borrowed from the single bounded source allocation.
pub(crate) struct Container<'a> {
    /// Original input containing every validated member span.
    data: &'a [u8],
    /// Owned bounded metadata for the flat addressed members.
    members: Map<String, Value>,
    /// Absolute byte offset at which member-relative offsets begin.
    data_start: usize,
}

impl<'a> Container<'a> {
    /// Validate framing, unique flat names and every source-bounded member span once.
    ///
    /// # Errors
    /// Returns an error for malformed or unsupported JSON/Pickle metadata, names,
    /// directory/link fields, overlapping spans, or exceeded manifest limits.
    pub(crate) fn parse(data: &'a [u8]) -> Result<Self> {
        let manifest = manifest(data)?;
        let root = object(&manifest, "Lychee manifest must be a JSON object")?;
        allowed_fields(root, &["version", "mangoFiles"])?;
        if root.get("version").and_then(Value::as_str) != Some("3.1.0") {
            return Err("unqualified Lychee manifest version (expected 3.1.0)".into());
        }
        let members = object(
            root.get("mangoFiles")
                .ok_or("Lychee manifest has no mangoFiles index")?,
            "Lychee mangoFiles index must be a flat JSON object",
        )?;
        if members.len() > MAX_MEMBERS {
            return Err("Lychee member index exceeds the 4096-entry limit".into());
        }
        let data_start = 8 + usize::try_from(u32_le(data, 4)?)?;
        let mut spans = Vec::with_capacity(members.len());
        for (name, member) in members {
            validate_name(name)?;
            let metadata = object(member, "Lychee member metadata must be an object")?;
            allowed_fields(metadata, &["size", "offset", "integrity"])?;
            let span = member_span(metadata, data_start, data.len())?;
            if span.start != span.end {
                spans.push(span);
            }
        }
        spans.sort_unstable_by_key(|span| span.start);
        if spans.windows(2).any(|pair| pair[0].end > pair[1].start) {
            return Err("Lychee member spans overlap".into());
        }
        let Value::Object(mut root) = manifest else {
            return Err("Lychee manifest must be a JSON object".into());
        };
        let Some(Value::Object(members)) = root.remove("mangoFiles") else {
            return Err("Lychee mangoFiles index must be a flat JSON object".into());
        };
        Ok(Self {
            data,
            members,
            data_start,
        })
    }

    /// Address one required-name member and verify any declared whole/block digests.
    /// Optional integrity metadata remains optional in both reader paths.
    ///
    /// # Errors
    /// Returns an error for invalid metadata, a selected member above `maximum`,
    /// unsupported or incomplete integrity metadata, or failed SHA-256 checks.
    pub(crate) fn member(
        &self,
        name: &str,
        maximum: usize,
        size_error: &str,
    ) -> Result<Option<&'a [u8]>> {
        let Some(member) = self.members.get(name) else {
            return Ok(None);
        };
        let metadata = object(member, "Lychee member metadata must be an object")?;
        let span = member_span(metadata, self.data_start, self.data.len())?;
        let bytes = &self.data[span];
        if bytes.len() > maximum {
            return Err(size_error.into());
        }
        verify_integrity(bytes, metadata, maximum)?;
        Ok(Some(bytes))
    }
}

/// Read one checked little-endian word from the fixed Pickle prefix.
fn u32_le(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        data.get(offset..offset + 4)
            .ok_or("truncated Lychee Pickle framing")?
            .try_into()?,
    ))
}

/// Validate canonical Pickle lengths and zero padding before parsing bounded JSON.
fn manifest(data: &[u8]) -> Result<Value> {
    let size_pickle = u32_le(data, 0)?;
    if size_pickle != 4 {
        return Err("unqualified Lychee Mango/ASAR Pickle framing".into());
    }
    let header_size = usize::try_from(u32_le(data, 4)?)?;
    let payload_size = usize::try_from(u32_le(data, 8)?)?;
    let json_size = usize::try_from(u32_le(data, 12)?)?;
    if json_size == 0 || json_size > MAX_MANIFEST {
        return Err("Lychee manifest exceeds the nonempty 2 MiB limit".into());
    }
    let padded_json_size = json_size
        .checked_add(3)
        .ok_or("Lychee JSON length overflow")?
        & !3;
    if header_size != padded_json_size + 8 || payload_size != padded_json_size + 4 {
        return Err("invalid Lychee Pickle lengths or alignment".into());
    }
    let json = data
        .get(16..16 + json_size)
        .ok_or("truncated Lychee JSON manifest")?;
    let padding = data
        .get(16 + json_size..8 + header_size)
        .ok_or("truncated Lychee Pickle padding")?;
    if padding.iter().any(|&byte| byte != 0) {
        return Err("Lychee Pickle padding must be zero".into());
    }
    JsonPreflight::new(json).check()?;
    Ok(serde_json::from_slice(json)?)
}

/// Require an object without echoing private scene or author metadata in errors.
fn object<'a>(value: &'a Value, error: &str) -> Result<&'a Map<String, Value>> {
    value.as_object().ok_or_else(|| error.into())
}

/// Reject fields representing unqualified directory, link or external-resource nodes.
fn allowed_fields(object: &Map<String, Value>, allowed: &[&str]) -> Result<()> {
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("unsupported Lychee manifest or member fields".into());
    }
    Ok(())
}

/// Admit only flat ASCII member names without path aliases or preview case aliases.
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 256
        || matches!(name, "." | "..")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        || (name != PREVIEW && name.eq_ignore_ascii_case(PREVIEW))
    {
        return Err("unsupported or ambiguous Lychee flat member name".into());
    }
    Ok(())
}

/// Resolve decimal-string offsets and integer sizes with checked source-bounded arithmetic.
fn member_span(
    metadata: &Map<String, Value>,
    data_start: usize,
    source_size: usize,
) -> Result<std::ops::Range<usize>> {
    let offset = metadata
        .get("offset")
        .and_then(Value::as_str)
        .filter(|offset| !offset.is_empty() && offset.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or("Lychee member offset must be a decimal string")?
        .parse::<u64>()?;
    let size = metadata
        .get("size")
        .and_then(Value::as_u64)
        .ok_or("Lychee member size must be a nonnegative integer")?;
    let start = u64::try_from(data_start)?
        .checked_add(offset)
        .ok_or("Lychee member offset overflow")?;
    let end = start
        .checked_add(size)
        .ok_or("Lychee member span overflow")?;
    if end > u64::try_from(source_size)? {
        return Err("Lychee member span exceeds the source file".into());
    }
    Ok(usize::try_from(start)?..usize::try_from(end)?)
}

/// Decode a declared SHA-256 digest without allocating or accepting ambiguous encodings.
fn digest(value: &Value) -> Result<[u8; 32]> {
    let text = value
        .as_str()
        .filter(|text| text.len() == 64)
        .ok_or("Lychee SHA-256 digest must contain 64 hexadecimal digits")?;
    let mut bytes = [0; 32];
    for (slot, pair) in bytes.iter_mut().zip(text.as_bytes().as_chunks::<2>().0) {
        let high = char::from(pair[0])
            .to_digit(16)
            .ok_or("invalid Lychee SHA-256 digest")?;
        let low = char::from(pair[1])
            .to_digit(16)
            .ok_or("invalid Lychee SHA-256 digest")?;
        *slot = u8::try_from(high * 16 + low)?;
    }
    Ok(bytes)
}

/// Check the selected member's whole digest and every declared contiguous block digest.
fn verify_integrity(png: &[u8], metadata: &Map<String, Value>, maximum_block: usize) -> Result<()> {
    let Some(declared) = metadata.get("integrity") else {
        return Ok(());
    };
    let integrity = object(declared, "Lychee preview integrity must be an object")?;
    allowed_fields(integrity, &["algorithm", "hash", "blockSize", "blocks"])?;
    if integrity.get("algorithm").and_then(Value::as_str) != Some("SHA256") {
        return Err("unsupported Lychee preview integrity algorithm".into());
    }
    let expected = digest(
        integrity
            .get("hash")
            .ok_or("Lychee preview is missing its SHA-256 digest")?,
    )?;
    if Sha256::digest(png).as_slice() != expected {
        return Err("Lychee preview SHA-256 integrity check failed".into());
    }
    if !integrity.contains_key("blockSize") && !integrity.contains_key("blocks") {
        return Ok(());
    }
    let maximum_block = u64::try_from(maximum_block)?;
    let block_size = integrity
        .get("blockSize")
        .and_then(Value::as_u64)
        .filter(|&size| size > 0 && size <= maximum_block)
        .ok_or("invalid Lychee preview integrity block size")?;
    let block_size = usize::try_from(block_size)?;
    let blocks = integrity
        .get("blocks")
        .and_then(Value::as_array)
        .ok_or("Lychee preview integrity blocks must be an array")?;
    if blocks.len() != png.len().div_ceil(block_size) {
        return Err("Lychee preview integrity block count does not match its size".into());
    }
    for (block, expected) in png.chunks(block_size).zip(blocks) {
        if Sha256::digest(block).as_slice() != digest(expected)? {
            return Err("Lychee preview block SHA-256 integrity check failed".into());
        }
    }
    Ok(())
}

/// Preflight PNG dimensions before bounded decoding, preserving the original RGBA pixels.
fn decode_png(png: &[u8]) -> Result<RgbaImage> {
    if png.len() < 33
        || !png.starts_with(b"\x89PNG\r\n\x1a\n")
        || png[8..16] != [0, 0, 0, 13, b'I', b'H', b'D', b'R']
    {
        return Err("Lychee preview.png is not a PNG with a valid IHDR carrier".into());
    }
    let width = u32::from_be_bytes(png[16..20].try_into()?);
    let height = u32::from_be_bytes(png[20..24].try_into()?);
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err("Lychee saved PNG exceeds the dimension or pixel limit".into());
    }
    let mut reader = ImageReader::with_format(Cursor::new(png), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode()?.to_rgba8())
}

/// Bound manifest structure and reject decoded duplicate JSON keys before `Value` loses them.
struct JsonPreflight<'a> {
    /// UTF-8 JSON bytes already bounded by the Pickle reader.
    bytes: &'a [u8],
    /// Next unread byte in the structural preflight.
    position: usize,
    /// Cumulative object, array and scalar values encountered.
    nodes: usize,
}

impl<'a> JsonPreflight<'a> {
    /// Start a bounded manifest structural preflight.
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            nodes: 0,
        }
    }

    /// Require one complete value without trailing bytes or duplicate object keys.
    fn check(mut self) -> Result<()> {
        self.value(0)?;
        self.whitespace();
        if self.position != self.bytes.len() {
            return Err("trailing Lychee manifest JSON data".into());
        }
        Ok(())
    }

    /// Skip only the four JSON whitespace bytes.
    fn whitespace(&mut self) {
        while self
            .bytes
            .get(self.position)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        {
            self.position += 1;
        }
    }

    /// Consume one required punctuation byte after JSON whitespace.
    fn punctuation(&mut self, expected: u8) -> Result<()> {
        self.whitespace();
        if self.bytes.get(self.position) != Some(&expected) {
            return Err("invalid Lychee manifest JSON structure".into());
        }
        self.position += 1;
        Ok(())
    }

    /// Locate one bounded quoted string, deferring escape validity to `serde_json`.
    fn string(&mut self) -> Result<&'a [u8]> {
        self.whitespace();
        let start = self.position;
        self.punctuation(b'"')?;
        loop {
            if self.position - start >= MAX_STRING {
                return Err("Lychee manifest string exceeds the 4096-byte limit".into());
            }
            match self.bytes.get(self.position) {
                Some(b'"') => {
                    self.position += 1;
                    return Ok(&self.bytes[start..self.position]);
                }
                Some(b'\\') => self.position += 2,
                Some(_) => self.position += 1,
                None => return Err("truncated Lychee manifest JSON string".into()),
            }
        }
    }

    /// Walk one JSON value under depth/node caps and detect duplicates in every object.
    fn value(&mut self, depth: usize) -> Result<()> {
        self.nodes += 1;
        if depth > MAX_DEPTH || self.nodes > MAX_NODES {
            return Err("Lychee manifest exceeds the JSON depth or node limit".into());
        }
        self.whitespace();
        match self.bytes.get(self.position) {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(|_| ()),
            Some(_) => {
                let start = self.position;
                while self.bytes.get(self.position).is_some_and(|byte| {
                    !matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n')
                }) {
                    self.position += 1;
                }
                if start == self.position {
                    return Err("invalid Lychee manifest JSON scalar".into());
                }
                Ok(())
            }
            None => Err("truncated Lychee manifest JSON value".into()),
        }
    }

    /// Check one object, comparing decoded keys so escaped aliases are also duplicates.
    fn object(&mut self, depth: usize) -> Result<()> {
        self.punctuation(b'{')?;
        self.whitespace();
        if self.bytes.get(self.position) == Some(&b'}') {
            self.position += 1;
            return Ok(());
        }
        let mut keys = BTreeSet::new();
        loop {
            let key: String = serde_json::from_slice(self.string()?)?;
            if !keys.insert(key) {
                return Err("duplicate Lychee manifest JSON key".into());
            }
            self.punctuation(b':')?;
            self.value(depth + 1)?;
            self.whitespace();
            if self.bytes.get(self.position) == Some(&b'}') {
                self.position += 1;
                return Ok(());
            }
            self.punctuation(b',')?;
        }
    }

    /// Check an array while charging every element to the shared node limit.
    fn array(&mut self, depth: usize) -> Result<()> {
        self.punctuation(b'[')?;
        self.whitespace();
        if self.bytes.get(self.position) == Some(&b']') {
            self.position += 1;
            return Ok(());
        }
        loop {
            self.value(depth + 1)?;
            self.whitespace();
            if self.bytes.get(self.position) == Some(&b']') {
                self.position += 1;
                return Ok(());
            }
            self.punctuation(b',')?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    use serde_json::json;
    use std::fmt::Write;

    /// Encode a digest using the manifest's literal lowercase hexadecimal protocol.
    fn hex(data: &[u8]) -> String {
        let mut text = String::with_capacity(64);
        for byte in Sha256::digest(data) {
            write!(text, "{byte:02x}").unwrap();
        }
        text
    }

    /// Build an owned small PNG with varying opaque and transparent pixel values.
    fn png() -> Vec<u8> {
        let image = RgbaImage::from_fn(2, 2, |x, y| match (x, y) {
            (0, 0) => Rgba([20, 40, 60, 255]),
            (1, 0) => Rgba([80, 100, 120, 128]),
            (0, 1) => Rgba([140, 160, 180, 0]),
            _ => Rgba([200, 220, 240, 255]),
        });
        let mut encoded = Vec::new();
        crate::render::write_png(&image, &mut encoded).unwrap();
        encoded
    }

    /// Frame supplied JSON and payload using canonical size/header Pickles and zero padding.
    fn container(json: &[u8], payload: &[u8]) -> Vec<u8> {
        let padded = (json.len() + 3) & !3;
        let mut data = Vec::new();
        for word in [4, padded + 8, padded + 4, json.len()] {
            data.extend_from_slice(&u32::try_from(word).unwrap().to_le_bytes());
        }
        data.extend_from_slice(json);
        data.resize(16 + padded, 0);
        data.extend_from_slice(payload);
        data
    }

    /// Serialize a neutral root manifest and its supplied addressed member bytes.
    fn indexed(members: &Value, payload: &[u8]) -> Vec<u8> {
        container(
            &serde_json::to_vec(&json!({"version":"3.1.0", "mangoFiles":members})).unwrap(),
            payload,
        )
    }

    /// Construct selected-member metadata, including all literal whole/block digests.
    fn metadata(data: &[u8], block_size: usize) -> Value {
        json!({"offset":"0", "size":data.len(), "integrity":{
            "algorithm":"SHA256", "hash":hex(data), "blockSize":block_size,
            "blocks":data.chunks(block_size).map(hex).collect::<Vec<_>>()
        }})
    }

    /// Assert a specific public failure class rather than any incidental parse error.
    fn error(data: &[u8], expected: &str) {
        let error = extract_bytes(data).unwrap_err().to_string();
        assert!(
            error.contains(expected),
            "expected {expected:?}; got {error}"
        );
    }

    #[test]
    fn exact_address_preserves_pixels_and_checks_each_integrity_block() {
        let png = png();
        let unrelated = b"neutral unrelated bytes";
        let mut payload = unrelated.to_vec();
        payload.extend_from_slice(&png);
        let mut preview = metadata(&png, 17);
        preview["offset"] = json!(unrelated.len().to_string());
        let source = indexed(
            &json!({"note.bin":{"offset":"0","size":unrelated.len()}, "preview.png":preview}),
            &payload,
        );
        let original = source.clone();
        let actual = extract_bytes(&source).unwrap().unwrap();
        assert_eq!(actual.dimensions(), (2, 2));
        assert_eq!(*actual.get_pixel(0, 0), Rgba([20, 40, 60, 255]));
        assert_eq!(*actual.get_pixel(1, 0), Rgba([80, 100, 120, 128]));
        assert_eq!(*actual.get_pixel(0, 1), Rgba([140, 160, 180, 0]));
        assert_eq!(*actual.get_pixel(1, 1), Rgba([200, 220, 240, 255]));
        assert_eq!(source, original);
    }

    #[test]
    fn absent_addressed_preview_ignores_unrelated_png_signatures() {
        let png = png();
        assert!(
            extract_bytes(&indexed(&json!({"note.png":metadata(&png, 4096)}), &png))
                .unwrap()
                .is_none()
        );
        assert!(extract_bytes(&indexed(&json!({}), &png)).unwrap().is_none());
    }

    #[test]
    fn malformed_selected_png_errors_instead_of_reporting_absence() {
        let not_png = b"neutral non-image payload";
        error(
            &indexed(&json!({"preview.png":metadata(not_png, 4096)}), not_png),
            "valid IHDR",
        );
        let mut png = png();
        let mut offset = 8;
        loop {
            let size = usize::try_from(u32::from_be_bytes(
                png[offset..offset + 4].try_into().unwrap(),
            ))
            .unwrap();
            if &png[offset + 4..offset + 8] == b"IDAT" {
                png[offset + 8 + size] ^= 1;
                break;
            }
            offset += 12 + size;
        }
        error(
            &indexed(&json!({"preview.png":metadata(&png, 4096)}), &png),
            "CRC",
        );
    }

    #[test]
    fn every_truncated_pickle_prefix_and_payload_is_rejected() {
        let source = indexed(&json!({}), &[]);
        for length in 0..source.len() {
            assert!(extract_bytes(&source[..length]).is_err(), "length {length}");
        }
        let png = png();
        let mut source = indexed(&json!({"preview.png":metadata(&png, 4096)}), &png);
        source.pop();
        error(&source, "span exceeds");
    }

    #[test]
    fn pickle_word_lengths_alignment_and_zero_padding_are_mandatory() {
        let json = br#"{"version":"3.1.0","mangoFiles":{}}"#;
        assert_ne!(json.len() % 4, 0);
        let source = container(json, &[]);
        for offset in [0, 4, 8, 12] {
            let mut malformed = source.clone();
            malformed[offset] ^= 1;
            assert!(extract_bytes(&malformed).is_err(), "word offset {offset}");
        }
        let mut nonzero_padding = source.clone();
        nonzero_padding[16 + json.len()] = 1;
        error(&nonzero_padding, "padding must be zero");
        let mut excessive = source;
        excessive[12..16].copy_from_slice(&u32::try_from(MAX_MANIFEST + 1).unwrap().to_le_bytes());
        error(&excessive, "2 MiB");
    }

    #[test]
    fn duplicate_decoded_keys_and_member_aliases_are_rejected() {
        for json in [
            br#"{"version":"3.1.0","version":"3.1.0","mangoFiles":{}}"#.as_slice(),
            br#"{"version":"3.1.0","mangoFiles":{"preview.png":{"offset":"0","size":0},"preview.png":{"offset":"0","size":0}}}"#,
            br#"{"version":"3.1.0","mangoFiles":{"preview.png":{"offset":"0","size":0},"\u0070review.png":{"offset":"0","size":0}}}"#,
            br#"{"version":"3.1.0","mangoFiles":{"mesh.bin":{"offset":"0","size":0,"size":1}}}"#,
        ] {
            error(&container(json, &[]), "duplicate");
        }
        for name in [
            "Preview.png",
            "./preview.png",
            "nested/preview.png",
            "../preview.png",
            "/preview.png",
            "preview.png ",
            "..",
            "C:\\preview.png",
        ] {
            let mut members = Map::new();
            members.insert(name.into(), json!({"offset":"0", "size":0}));
            error(&indexed(&Value::Object(members), &[]), "member name");
        }
    }

    #[test]
    fn linked_unpacked_nested_and_unknown_manifest_carriers_are_rejected() {
        for field in ["link", "unpacked", "files", "external"] {
            let mut member = json!({"offset":"0", "size":0});
            member[field] = json!(true);
            error(&indexed(&json!({"preview.png":member}), &[]), "unsupported");
        }
        for manifest in [
            json!({"version":"3.2.0", "mangoFiles":{}}),
            json!({"version":3, "mangoFiles":{}}),
            json!({"version":"3.1.0", "files":{}}),
            json!({"version":"3.1.0", "mangoFiles":[]}),
            json!({"version":"3.1.0", "mangoFiles":{}, "external":true}),
        ] {
            assert!(
                extract_bytes(&container(&serde_json::to_vec(&manifest).unwrap(), &[])).is_err()
            );
        }
        error(b"PK\x03\x04legacy ZIP .lys carrier", "Pickle");
    }

    #[test]
    fn all_member_spans_are_checked_even_without_a_saved_preview() {
        for member in [
            json!({"size":0}),
            json!({"offset":"0"}),
            json!({"offset":0,"size":0}),
            json!({"offset":"-1","size":0}),
            json!({"offset":"+1","size":0}),
            json!({"offset":"1.5","size":0}),
            json!({"offset":"0","size":-1}),
            json!({"offset":"0","size":0.5}),
            json!({"offset":"18446744073709551615","size":0}),
            json!({"offset":"0","size":18_446_744_073_709_551_615_u64}),
            json!({"offset":"18446744073709551616","size":0}),
            json!({"offset":"1","size":0}),
        ] {
            assert!(extract_bytes(&indexed(&json!({"mesh.bin":member}), &[])).is_err());
        }
        error(
            &indexed(
                &json!({"one.bin":{"offset":"0","size":2}, "two.bin":{"offset":"1","size":1}}),
                b"xx",
            ),
            "overlap",
        );
    }

    #[test]
    fn wrong_whole_and_block_hashes_and_unqualified_integrity_are_rejected() {
        let png = png();
        let valid = metadata(&png, 17);
        let mut wrong_hash = valid.clone();
        wrong_hash["integrity"]["hash"] = json!("00".repeat(32));
        error(
            &indexed(&json!({"preview.png":wrong_hash}), &png),
            "SHA-256 integrity",
        );
        let mut wrong_block = valid.clone();
        wrong_block["integrity"]["blocks"][0] = json!("00".repeat(32));
        error(
            &indexed(&json!({"preview.png":wrong_block}), &png),
            "block SHA-256",
        );
        for (field, value) in [
            ("algorithm", json!("SHA1")),
            ("hash", json!("z".repeat(64))),
            ("hash", json!("0".repeat(63))),
            ("blockSize", json!(0)),
            ("blockSize", json!(MAX_PREVIEW + 1)),
            ("blocks", json!([])),
            ("blocks", json!("not an array")),
        ] {
            let mut preview = valid.clone();
            preview["integrity"][field] = value;
            assert!(extract_bytes(&indexed(&json!({"preview.png":preview}), &png)).is_err());
        }
        let mut absent_hash = valid;
        absent_hash["integrity"]
            .as_object_mut()
            .unwrap()
            .remove("hash");
        error(
            &indexed(&json!({"preview.png":absent_hash}), &png),
            "missing its SHA-256",
        );
    }

    #[test]
    fn valid_png_without_optional_integrity_metadata_preserves_pixels() {
        let png = png();
        let source = indexed(
            &json!({"preview.png":{"offset":"0", "size":png.len()}}),
            &png,
        );
        let actual = extract_bytes(&source).unwrap().unwrap();
        assert_eq!(
            actual,
            image::load_from_memory_with_format(&png, ImageFormat::Png)
                .unwrap()
                .into_rgba8()
        );
        let corrupt = indexed(&json!({"preview.png":{"offset":"0", "size":4}}), b"nope");
        error(&corrupt, "valid IHDR");
    }

    #[test]
    fn whole_only_integrity_is_supported_and_partial_block_metadata_errors() {
        let png = png();
        let mut preview = metadata(&png, 4096);
        preview["integrity"]
            .as_object_mut()
            .unwrap()
            .remove("blocks");
        error(
            &indexed(&json!({"preview.png":preview.clone()}), &png),
            "blocks must be an array",
        );
        preview["integrity"]
            .as_object_mut()
            .unwrap()
            .remove("blockSize");
        assert!(
            extract_bytes(&indexed(&json!({"preview.png":preview.clone()}), &png))
                .unwrap()
                .is_some()
        );
        preview["integrity"]["blocks"] = json!([hex(&png)]);
        error(
            &indexed(&json!({"preview.png":preview}), &png),
            "block size",
        );
    }

    #[test]
    fn decoded_dimensions_and_pixels_are_bounded_before_png_allocation() {
        for (width, height) in [(0, 2), (2, 0), (4097, 2), (2, 4097), (4096, 4096)] {
            let mut png = png();
            png[16..20].copy_from_slice(&u32::to_be_bytes(width));
            png[20..24].copy_from_slice(&u32::to_be_bytes(height));
            error(
                &indexed(&json!({"preview.png":metadata(&png, 4096)}), &png),
                "dimension or pixel",
            );
        }
        let oversized = vec![0; MAX_PREVIEW + 1];
        error(
            &indexed(
                &json!({"preview.png":{"offset":"0", "size":oversized.len()}}),
                &oversized,
            ),
            "8 MiB",
        );
    }

    #[test]
    fn manifest_depth_node_string_and_member_counts_are_bounded() {
        let mut deep = json!(0);
        for _ in 0..=MAX_DEPTH {
            deep = json!([deep]);
        }
        error(
            &container(&serde_json::to_vec(&deep).unwrap(), &[]),
            "depth or node",
        );
        error(
            &container(&serde_json::to_vec(&vec![0; MAX_NODES]).unwrap(), &[]),
            "depth or node",
        );
        error(
            &container(&serde_json::to_vec(&"x".repeat(MAX_STRING)).unwrap(), &[]),
            "string exceeds",
        );
        let members: Map<String, Value> = (0..=MAX_MEMBERS)
            .map(|index| (format!("m{index}.bin"), json!({"offset":"0","size":0})))
            .collect();
        error(&indexed(&Value::Object(members), &[]), "4096-entry");
    }

    #[test]
    fn malformed_json_escapes_scalars_and_trailing_data_are_rejected() {
        for json in [
            br#"{"version":"3.1.0","mangoFiles":{},}"#.as_slice(),
            br#"{"version":"3.1.0","mangoFiles":{},"x":"\q"}"#,
            br#"{"version":"3.1.0","mangoFiles":{"x":NaN}}"#,
            br#"{"version":"3.1.0","mangoFiles":{}} {}"#,
            br#"{"version":"3.1.0","mangoFiles":{"x":[0,]}}"#,
        ] {
            assert!(extract_bytes(&container(json, &[])).is_err());
        }
        let escaped =
            br#"{"version":"3.1.0","mangoFiles":{"note\u002epng":{"offset":"0","size":0}}}"#;
        assert!(extract_bytes(&container(escaped, &[])).unwrap().is_none());
    }
}
