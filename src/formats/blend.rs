//! Static Blender mesh geometry decoded from the file's own Structure DNA.
//!
//! This independent reader follows Blender's documented BHead/SDNA layout and
//! DNA mesh/attribute declarations. It never reads TEST or PreviewImage pixels.
//! Geometry requiring Blender's dependency graph is rejected explicitly.
use super::{mesh::triangulate_polygon, transform::Transform};
use crate::{
    Result,
    scene::{Line, Point, Scene, Triangle, Vec3},
};
use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Read},
    path::Path,
};

/// Maximum number of `BHead` blocks accepted from one file.
const MAX_BLOCKS: usize = 2_000_000;
/// Maximum element count for a mesh array or emitted primitive collection.
const MAX_ITEMS: usize = 5_000_000;
/// Maximum parent, collection and collection-instance recursion depth.
const MAX_DEPTH: usize = 128;
/// Shared file-size ceiling applied to decompressed Blender bytes.
#[allow(
    clippy::cast_possible_truncation,
    reason = "The shared 512 MiB limit fits usize on supported 32- and 64-bit targets."
)]
const MAX_BYTES: usize = super::MAX_INPUT_BYTES as usize;

/// Read a Blender file and emit its supported static mesh geometry.
///
/// # Errors
/// Returns an error for unreadable, oversized or malformed files, unsupported
/// compression or SDNA layouts, unresolved pointers, invalid mesh topology,
/// non-finite transforms, external libraries or geometry requiring Blender
/// dependency-graph evaluation. Empty or invalid output scenes also fail.
pub fn load(path: &Path) -> Result<Scene> {
    let encoded = super::read_file(path)?;
    let bytes = decompress(&encoded)?;
    let file = Database::parse(&bytes)?;
    let scene = file.scene()?;
    scene.validate()?;
    Ok(scene)
}

/// Read at most the remaining decompression budget, rejecting an extra byte.
fn bounded(reader: impl Read, remaining: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(remaining as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > remaining {
        return Err("decompressed Blender file exceeds 512 MiB".into());
    }
    Ok(bytes)
}
/// Decode raw, multi-member gzip or bounded Zstandard Blender data, including seek tables.
fn decompress(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.starts_with(b"BLENDER") {
        return Ok(bytes.to_vec());
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return bounded(flate2::read::MultiGzDecoder::new(bytes), MAX_BYTES);
    }
    if !bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        return Err("input is not a raw, gzip or Zstandard Blender file".into());
    }
    let mut cursor = Cursor::new(bytes);
    let mut output = Vec::new();
    let mut frames = 0;
    while cursor.position() < bytes.len() as u64 {
        frames += 1;
        if frames > 4096 {
            return Err("too many Blender Zstandard frames".into());
        }
        let before = cursor.position();
        let offset = usize::try_from(before)?;
        let magic = Endian::Little.uint(slice(bytes, offset, 4)?)?;
        if magic & 0xffff_fff0 == 0x184d_2a50 {
            let length = usize::try_from(Endian::Little.uint(slice(bytes, offset + 4, 4)?)?)?;
            let end = offset
                .checked_add(8)
                .and_then(|v| v.checked_add(length))
                .ok_or("Blender seek table length overflow")?;
            slice(bytes, offset, end - offset)?;
            cursor.set_position(end as u64);
            continue;
        }
        let mut decoder =
            ruzstd::decoding::StreamingDecoder::new_with_max_window_size(cursor, 64 * 1024 * 1024)?;
        output.extend(bounded(&mut decoder, MAX_BYTES - output.len())?);
        cursor = decoder.into_inner();
        if cursor.position() <= before {
            return Err("Blender Zstandard decoder made no progress".into());
        }
    }
    Ok(output)
}
/// Borrow a checked byte range, rejecting offset overflow and truncated payloads.
fn slice(bytes: &[u8], offset: usize, length: usize) -> Result<&[u8]> {
    let end = offset
        .checked_add(length)
        .ok_or("Blender offset overflow")?;
    bytes
        .get(offset..end)
        .ok_or_else(|| "truncated Blender data".into())
}
/// Byte order declared by the Blender file header.
#[derive(Clone, Copy)]
enum Endian {
    /// Least-significant byte first.
    Little,
    /// Most-significant byte first.
    Big,
}
impl Endian {
    /// Decode an unsigned one-, two-, four- or eight-byte word in the file byte order.
    fn uint(self, bytes: &[u8]) -> Result<u64> {
        if !matches!(bytes.len(), 1 | 2 | 4 | 8) {
            return Err("unsupported Blender integer size".into());
        }
        let mut out = [0; 8];
        Ok(match self {
            Self::Little => {
                out[..bytes.len()].copy_from_slice(bytes);
                u64::from_le_bytes(out)
            }
            Self::Big => {
                out[8 - bytes.len()..].copy_from_slice(bytes);
                u64::from_be_bytes(out)
            }
        })
    }
    /// Decode a signed word by sign-extending its encoded bit pattern.
    fn int(self, bytes: &[u8]) -> Result<i64> {
        let unsigned = self.uint(bytes)?;
        let shift = 64 - bytes.len() * 8;
        Ok((unsigned << shift).cast_signed() >> shift)
    }
    /// Decode a finite float32 coordinate or matrix coefficient as f64.
    fn float(self, bytes: &[u8]) -> Result<f64> {
        if bytes.len() != 4 {
            return Err("Blender coordinate is not a 32-bit float".into());
        }
        let value = f64::from(f32::from_bits(u32::try_from(self.uint(bytes)?)?));
        if !value.is_finite() {
            return Err("non-finite Blender coordinate or transform".into());
        }
        Ok(value)
    }
}
/// One SDNA field declaration with its computed position in the enclosing record.
struct Field {
    /// Field identifier after pointer and array syntax is removed.
    name: String,
    /// SDNA base type name used to resolve embedded structures.
    type_name: String,
    /// Byte offset from the start of the enclosing structure.
    offset: usize,
    /// Byte extent including every declared array element.
    length: usize,
    /// Whether the declaration stores a saved pointer rather than an embedded value.
    pointer: bool,
}
/// An SDNA structure layout used to validate typed block and field access.
struct Schema {
    /// SDNA structure type name.
    name: String,
    /// Structure byte size from the TLEN table.
    size: usize,
    /// Fields in declaration order with computed byte offsets.
    fields: Vec<Field>,
    /// Total declared field extent, checked against the TLEN size before record access.
    used: usize,
}
/// A `BHead` payload and its saved address, schema and local-data ownership.
#[derive(Clone, Copy)]
struct Block<'a> {
    /// Four-byte `BHead` identifier, including DATA or a datablock type.
    code: [u8; 4],
    /// Original saved memory address used to resolve pointers.
    address: u64,
    /// Index of the block element type in the SDNA structure table.
    schema: usize,
    /// Number of encoded elements claimed by the `BHead` header.
    count: usize,
    /// Borrowed block payload after its checked header.
    data: &'a [u8],
    /// Index of the preceding non-DATA block that scopes local DATA pointers.
    owner: usize,
}
/// A single typed SDNA record borrowed from a validated block payload.
#[derive(Clone, Copy)]
struct Record<'a> {
    /// Index of the record structure in the SDNA table.
    schema: usize,
    /// Borrowed bytes for this one structure value.
    data: &'a [u8],
    /// Datablock owner used when resolving nested local pointers.
    owner: usize,
}
/// Parsed Blender blocks, SDNA layouts and scoped saved-pointer lookup tables.
struct Database<'a> {
    /// File-declared byte order for numeric fields.
    endian: Endian,
    /// Encoded pointer width, either four or eight bytes.
    pointer_size: usize,
    /// Parsed and indexed SDNA structure layouts.
    schemas: Vec<Schema>,
    /// Unique SDNA structure names mapped to schema indices.
    types: HashMap<String, usize>,
    /// `BHead` blocks in source order.
    blocks: Vec<Block<'a>>,
    /// Global datablock addresses mapped to block indices.
    ids: HashMap<u64, usize>,
    /// DATA addresses mapped by their owning datablock and saved address.
    local: HashMap<(usize, u64), usize>,
}
/// Geometry attribute identifiers for legacy `CustomData` and modern attribute storage.
#[derive(Clone, Copy)]
struct AttributeSpec {
    /// Legacy Mesh `CustomData` field for this geometry domain.
    domain: &'static str,
    /// Named geometry attribute identifier.
    name: &'static str,
    /// `CustomData` layer type used for legacy named attributes.
    legacy_type: i64,
    /// Modern attribute data-type identifier.
    modern_type: i64,
    /// Modern attribute geometry-domain identifier.
    modern_domain: i64,
    /// Encoded bytes per attribute sample.
    stride: usize,
}
/// Packed float32 vertex positions in the point domain.
const POSITION: AttributeSpec = AttributeSpec {
    domain: "vdata",
    name: "position",
    legacy_type: 48,
    modern_type: 7,
    modern_domain: 0,
    stride: 12,
};
/// Packed signed vertex indices in the face-corner domain.
const CORNER_VERTEX: AttributeSpec = AttributeSpec {
    domain: "ldata",
    name: ".corner_vert",
    legacy_type: 11,
    modern_type: 3,
    modern_domain: 3,
    stride: 4,
};
/// Packed pairs of signed vertex indices in the edge domain.
const EDGE_VERTICES: AttributeSpec = AttributeSpec {
    domain: "edata",
    name: ".edge_verts",
    legacy_type: 46,
    modern_type: 4,
    modern_domain: 1,
    stride: 8,
};
/// Accumulated static geometry and transform caches during object instancing.
struct GeometryState {
    /// Geometry emitted by completed mesh and instance visits.
    scene: Scene,
    /// Cached world matrices for saved object addresses.
    matrices: HashMap<u64, Transform>,
    /// Collection addresses currently on the instance recursion stack.
    active_instances: HashSet<u64>,
    /// Number of object instances visited, including empty and skipped objects.
    emitted: usize,
}
/// Bounded cursor over the Structure DNA section in its declared byte order.
struct DnaCursor<'a> {
    /// Borrowed Structure DNA section bytes.
    data: &'a [u8],
    /// Next unread byte, always within the checked section.
    offset: usize,
    /// File byte order for section counts and indices.
    endian: Endian,
}
impl<'a> DnaCursor<'a> {
    /// Consume an exact byte count after checking the SDNA payload boundary.
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let out = slice(self.data, self.offset, n)?;
        self.offset += n;
        Ok(out)
    }
    /// Consume and verify a required SDNA section marker.
    fn tag(&mut self, tag: &[u8]) -> Result<()> {
        if self.take(tag.len())? != tag {
            return Err("invalid Blender SDNA section".into());
        }
        Ok(())
    }
    /// Consume an unsigned SDNA word that fits the host index type.
    fn integer(&mut self, bytes: usize) -> Result<usize> {
        usize::try_from(self.endian.uint(self.take(bytes)?)?).map_err(Into::into)
    }
    /// Read an SDNA table length bounded to 100,000 entries.
    fn count(&mut self) -> Result<usize> {
        let n = self.integer(4)?;
        if n > 100_000 {
            return Err("Blender SDNA table exceeds 100,000 entries".into());
        }
        Ok(n)
    }
    /// Consume padding up to the next four-byte SDNA section boundary.
    fn align(&mut self) -> Result<()> {
        let pad = (4 - self.offset % 4) % 4;
        self.take(pad)?;
        Ok(())
    }
    /// Decode a bounded table of terminated UTF-8 SDNA names and its alignment padding.
    fn strings(&mut self) -> Result<Vec<String>> {
        let count = self.count()?;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            let rest = &self.data[self.offset..];
            let n = rest
                .iter()
                .position(|b| *b == 0)
                .ok_or("unterminated Blender SDNA string")?;
            if n > 1024 {
                return Err("Blender SDNA name exceeds 1,024 bytes".into());
            }
            values.push(std::str::from_utf8(self.take(n)?)?.to_owned());
            self.take(1)?;
        }
        self.align()?;
        Ok(values)
    }
}
/// Separate an SDNA field name, pointer flag and checked array-element product.
fn declaration(raw: &str) -> Result<(String, usize, bool)> {
    let pointer = raw.contains('*');
    let name: String = raw
        .chars()
        .take_while(|c| *c != '[')
        .filter(|c| !matches!(c, '*' | '(' | ')'))
        .collect();
    let mut elements = 1usize;
    let mut remaining = raw;
    while let Some(open) = remaining.find('[') {
        remaining = &remaining[open + 1..];
        let close = remaining.find(']').ok_or("invalid Blender SDNA array")?;
        let n: usize = remaining[..close].parse()?;
        elements = elements
            .checked_mul(n)
            .ok_or("Blender SDNA array overflow")?;
        if elements > MAX_ITEMS {
            return Err("Blender SDNA array exceeds the item limit".into());
        }
        remaining = &remaining[close + 1..];
    }
    Ok((name, elements, pointer))
}
/// Decode SDNA names, type sizes and structures into ordered field layouts.
fn schemas(data: &[u8], endian: Endian, pointer: usize) -> Result<Vec<Schema>> {
    let mut cursor = DnaCursor {
        data,
        offset: 0,
        endian,
    };
    cursor.tag(b"SDNANAME")?;
    let names = cursor.strings()?;
    cursor.tag(b"TYPE")?;
    let types = cursor.strings()?;
    cursor.tag(b"TLEN")?;
    let lengths: Vec<usize> = (0..types.len())
        .map(|_| cursor.integer(2))
        .collect::<Result<_>>()?;
    cursor.align()?;
    cursor.tag(b"STRC")?;
    let count = cursor.count()?;
    let mut result = Vec::with_capacity(count);
    let mut total_fields = 0usize;
    for _ in 0..count {
        let type_index = cursor.integer(2)?;
        let name = types
            .get(type_index)
            .ok_or("invalid Blender SDNA type index")?
            .clone();
        let field_count = cursor.integer(2)?;
        total_fields += field_count;
        if total_fields > 1_000_000 {
            return Err("Blender SDNA exceeds one million fields".into());
        }
        let mut fields = Vec::with_capacity(field_count);
        let mut used = 0usize;
        for _ in 0..field_count {
            let field_type = cursor.integer(2)?;
            let field_name = cursor.integer(2)?;
            let type_name = types
                .get(field_type)
                .ok_or("invalid Blender SDNA field type")?
                .clone();
            let raw = names
                .get(field_name)
                .ok_or("invalid Blender SDNA field name")?;
            let (name, elements, is_pointer) = declaration(raw)?;
            let size = if is_pointer {
                pointer
            } else {
                lengths[field_type]
            };
            let length = size
                .checked_mul(elements)
                .ok_or("Blender SDNA field size overflow")?;
            fields.push(Field {
                name,
                type_name,
                offset: used,
                length,
                pointer: is_pointer,
            });
            used = used
                .checked_add(length)
                .ok_or("Blender SDNA struct size overflow")?;
        }
        result.push(Schema {
            name,
            size: lengths[type_index],
            fields,
            used,
        });
    }
    if cursor.offset != data.len() {
        return Err("unexpected trailing Blender SDNA bytes".into());
    }
    Ok(result)
}
impl<'a> Database<'a> {
    /// Parse file and `BHead` headers, decode SDNA and build unambiguous scoped address maps.
    #[allow(
        clippy::too_many_lines,
        reason = "Keep versioned BHead parsing, SDNA decoding and scoped pointer indexing together so their validation order remains explicit."
    )]
    fn parse(data: &'a [u8]) -> Result<Self> {
        if !data.starts_with(b"BLENDER") {
            return Err("invalid Blender file signature".into());
        }
        let large = data.get(7..9) == Some(b"17");
        let (pointer_size, endian, mut offset, block_size) = if large {
            if data.get(9..13) != Some(b"-01v") {
                return Err("unsupported Blender low-level file format".into());
            }
            if !slice(data, 13, 4)?.iter().all(u8::is_ascii_digit) {
                return Err("invalid Blender version".into());
            }
            (8, Endian::Little, 17, 32)
        } else {
            let pointer = match data.get(7) {
                Some(b'-') => 8,
                Some(b'_') => 4,
                _ => return Err("invalid Blender pointer size".into()),
            };
            let endian = match data.get(8) {
                Some(b'v') => Endian::Little,
                Some(b'V') => Endian::Big,
                _ => return Err("invalid Blender byte order".into()),
            };
            if !slice(data, 9, 3)?.iter().all(u8::is_ascii_digit) {
                return Err("invalid Blender version".into());
            }
            (pointer, endian, 12, 16 + pointer)
        };
        let mut blocks = Vec::new();
        let mut dna = None;
        let mut ended = false;
        let mut owner = 0;
        while offset < data.len() {
            if blocks.len() >= MAX_BLOCKS {
                return Err("Blender block count exceeds two million".into());
            }
            let code: [u8; 4] = slice(data, offset, 4)?.try_into()?;
            if code == *b"ENDB" {
                // Older Blender writers use an eight-byte terminal header.
                let remaining = data.len() - offset;
                if remaining != 8 && remaining != block_size {
                    return Err("invalid Blender terminal block".into());
                }
                if endian.uint(slice(data, offset + 4, 4)?)? != 0 {
                    return Err("invalid Blender terminal block".into());
                }
                ended = true;
                break;
            }
            let header = slice(data, offset, block_size)?;
            let (length, schema, count) = if large {
                (
                    endian.int(&header[16..24])?,
                    endian.int(&header[4..8])?,
                    endian.int(&header[24..32])?,
                )
            } else {
                (
                    endian.int(&header[4..8])?,
                    endian.int(&header[8 + pointer_size..12 + pointer_size])?,
                    endian.int(&header[12 + pointer_size..16 + pointer_size])?,
                )
            };
            let length = usize::try_from(length).map_err(|_| "negative Blender block length")?;
            let schema = usize::try_from(schema).map_err(|_| "negative Blender SDNA index")?;
            let count = usize::try_from(count).map_err(|_| "negative Blender block count")?;
            if count > MAX_ITEMS {
                return Err("Blender block exceeds five million elements".into());
            }
            let address = endian.uint(&header[8..8 + pointer_size])?;
            offset += block_size;
            let payload = slice(data, offset, length)?;
            offset += length;
            if code != *b"DATA" {
                owner = blocks.len();
            }
            if code == *b"DNA1" && dna.replace(payload).is_some() {
                return Err("duplicate Blender SDNA block".into());
            }
            blocks.push(Block {
                code,
                address,
                schema,
                count,
                data: payload,
                owner,
            });
        }
        if !ended {
            return Err("Blender file has no terminal block".into());
        }
        let schemas = schemas(
            dna.ok_or("Blender file has no Structure DNA")?,
            endian,
            pointer_size,
        )?;
        let mut types = HashMap::new();
        for (index, schema) in schemas.iter().enumerate() {
            if types.insert(schema.name.clone(), index).is_some() {
                return Err("duplicate Blender SDNA struct type".into());
            }
        }
        let mut ids = HashMap::new();
        let mut local = HashMap::new();
        for (index, block) in blocks.iter().enumerate() {
            if block.address == 0 || matches!(&block.code, b"REND" | b"TEST" | b"DNA1" | b"GLOB") {
                continue;
            }
            let duplicate = if block.code == *b"DATA" {
                local.insert((block.owner, block.address), index).is_some()
            } else {
                ids.insert(block.address, index).is_some()
            };
            if duplicate {
                return Err("duplicate Blender block address within its data owner".into());
            }
        }
        Ok(Self {
            endian,
            pointer_size,
            schemas,
            types,
            blocks,
            ids,
            local,
        })
    }
    /// Retrieve a schema whose computed field extent equals its declared structure size.
    fn schema(&self, index: usize) -> Result<&Schema> {
        let schema = self
            .schemas
            .get(index)
            .ok_or("invalid Blender SDNA index")?;
        if schema.size == 0 || schema.used != schema.size {
            return Err(format!("unsupported Blender SDNA layout for {}", schema.name).into());
        }
        Ok(schema)
    }
    /// Find a named field in a record after validating its enclosing schema.
    fn field(&self, record: Record<'a>, name: &str) -> Result<&Field> {
        self.schema(record.schema)?
            .fields
            .iter()
            .find(|f| f.name == name)
            .ok_or_else(|| {
                format!(
                    "Blender {} has no {name} field",
                    self.schemas[record.schema].name
                )
                .into()
            })
    }
    /// Check whether a validated record schema includes a version-specific field.
    fn has(&self, record: Record<'a>, name: &str) -> bool {
        self.schemas[record.schema]
            .fields
            .iter()
            .any(|f| f.name == name)
    }
    /// Borrow the checked byte extent of a named record field.
    fn bytes(&self, record: Record<'a>, name: &str) -> Result<&'a [u8]> {
        let f = self.field(record, name)?;
        slice(record.data, f.offset, f.length)
    }
    /// Read a signed non-pointer field from a typed record.
    fn integer(&self, record: Record<'a>, name: &str) -> Result<i64> {
        if self.field(record, name)?.pointer {
            return Err("Blender integer field is a pointer".into());
        }
        self.endian.int(self.bytes(record, name)?)
    }
    /// Read a version-specific integer field, using zero when it is absent.
    fn optional_integer(&self, record: Record<'a>, name: &str) -> Result<i64> {
        if self.has(record, name) {
            self.integer(record, name)
        } else {
            Ok(0)
        }
    }
    /// Read a pointer field after checking its declaration and encoded pointer width.
    fn pointer(&self, record: Record<'a>, name: &str) -> Result<u64> {
        let field = self.field(record, name)?;
        if !field.pointer || field.length != self.pointer_size {
            return Err("invalid Blender pointer field".into());
        }
        self.endian.uint(self.bytes(record, name)?)
    }
    /// Read a version-specific pointer field, using the null address when absent.
    fn optional_pointer(&self, record: Record<'a>, name: &str) -> Result<u64> {
        if self.has(record, name) {
            self.pointer(record, name)
        } else {
            Ok(0)
        }
    }
    /// Borrow a non-pointer nested structure while retaining local-data ownership.
    fn embedded(&self, record: Record<'a>, name: &str) -> Result<Record<'a>> {
        let field = self.field(record, name)?;
        if field.pointer {
            return Err("Blender embedded struct field is a pointer".into());
        }
        let schema = *self
            .types
            .get(&field.type_name)
            .ok_or("missing Blender nested struct DNA")?;
        Ok(Record {
            schema,
            data: self.bytes(record, name)?,
            owner: record.owner,
        })
    }
    /// Resolve a saved pointer in its local owner before falling back to global datablocks.
    fn block(&self, address: u64, owner: usize) -> Result<Block<'a>> {
        let index = self
            .local
            .get(&(owner, address))
            .or_else(|| self.ids.get(&address))
            .ok_or_else(|| format!("unresolved Blender pointer {address:#x}"))?;
        Ok(self.blocks[*index])
    }
    /// Resolve and validate one block of the expected SDNA structure type.
    fn record(&self, address: u64, owner: usize, expected: &str) -> Result<Record<'a>> {
        let block = self.block(address, owner)?;
        let schema = self.schema(block.schema)?;
        if schema.name != expected || block.count != 1 || block.data.len() != schema.size {
            return Err(format!("invalid Blender {expected} datablock").into());
        }
        Ok(Record {
            schema: block.schema,
            data: block.data,
            owner: block.owner,
        })
    }
    /// Resolve an exactly sized array of records with the expected SDNA structure type.
    fn array(
        &self,
        address: u64,
        owner: usize,
        expected: &str,
        count: usize,
    ) -> Result<Vec<Record<'a>>> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let block = self.block(address, owner)?;
        let schema = self.schema(block.schema)?;
        if schema.name != expected
            || block.count != count
            || block.data.len()
                != schema
                    .size
                    .checked_mul(count)
                    .ok_or("Blender array size overflow")?
        {
            return Err(format!("invalid Blender {expected} array").into());
        }
        Ok(block
            .data
            .chunks_exact(schema.size)
            .map(|data| Record {
                schema: block.schema,
                data,
                owner: block.owner,
            })
            .collect())
    }
    /// Read the first available version-specific count field and enforce the geometry budget.
    fn count(&self, record: Record<'a>, names: &[&str]) -> Result<usize> {
        let selected = names
            .iter()
            .find(|n| self.has(record, n))
            .ok_or("Blender geometry count field is absent")?;
        let count = usize::try_from(self.integer(record, selected)?)
            .map_err(|_| "negative Blender geometry count")?;
        if count > MAX_ITEMS {
            return Err("Blender geometry exceeds five million elements".into());
        }
        Ok(count)
    }
    /// Follow a bounded, acyclic `ListBase` chain and verify its declared final address.
    fn list(&self, record: Record<'a>, name: &str, expected: &str) -> Result<Vec<Record<'a>>> {
        let list = self.embedded(record, name)?;
        let mut address = self.pointer(list, "first")?;
        let tail = self.pointer(list, "last")?;
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        let mut final_address = 0;
        while address != 0 {
            if result.len() > 100_000 || !seen.insert(address) {
                return Err("cyclic or oversized Blender linked list".into());
            }
            let item = self.record(address, record.owner, expected)?;
            final_address = address;
            address = self.pointer(item, "next")?;
            result.push(item);
        }
        if tail != final_address {
            return Err("inconsistent Blender linked-list tail".into());
        }
        Ok(result)
    }
    /// Check both `ListBase` endpoints, treating an absent optional list as empty.
    fn list_empty(&self, record: Record<'a>, name: &str) -> Result<bool> {
        if !self.has(record, name) {
            return Ok(true);
        }
        let list = self.embedded(record, name)?;
        Ok(self.pointer(list, "first")? == 0 && self.pointer(list, "last")? == 0)
    }
    /// Reject datablocks whose ID refers to an external Blender library.
    fn local_id(&self, record: Record<'a>) -> Result<()> {
        let id = self.embedded(record, "id")?;
        if self.optional_pointer(id, "lib")? != 0 {
            return Err("external Blender library geometry is not supported".into());
        }
        Ok(())
    }
    /// Select the active scene and emit its render-visible static objects once per placement.
    fn scene(&self) -> Result<Scene> {
        let mut current = 0;
        for block in &self.blocks {
            if block.code == *b"GLOB" {
                self.schema(block.schema)?;
                let global = Record {
                    schema: block.schema,
                    data: block.data,
                    owner: block.owner,
                };
                current = self.optional_pointer(global, "curscene")?;
            }
        }
        if current == 0 {
            let mut scenes = self.blocks.iter().filter(|b| {
                self.schemas
                    .get(b.schema)
                    .is_some_and(|s| s.name == "Scene")
                    && b.code != *b"DATA"
            });
            current = scenes.next().ok_or("Blender file has no scene")?.address;
            if scenes.next().is_some() {
                return Err("Blender active scene is ambiguous".into());
            }
        }
        let scene = self.record(current, 0, "Scene")?;
        self.local_id(scene)?;
        if self.optional_pointer(scene, "set")? != 0 {
            return Err("Blender background scenes are not supported".into());
        }
        let mut objects = Vec::new();
        let collection = self.optional_pointer(scene, "master_collection")?;
        if collection != 0 {
            self.collection(
                collection,
                scene.owner,
                &mut HashSet::new(),
                &mut HashSet::new(),
                &mut objects,
                0,
            )?;
        } else {
            for base in self.list(scene, "base", "Base")? {
                objects.push(self.pointer(base, "object")?);
            }
        }
        let mut state = GeometryState {
            scene: Scene::default(),
            matrices: HashMap::new(),
            active_instances: HashSet::new(),
            emitted: 0,
        };
        let mut seen = HashSet::new();
        for address in objects {
            if seen.insert(address) {
                self.emit_object(address, Transform::IDENTITY, &mut state, 0)?;
            }
        }
        state.scene.validate()?;
        Ok(state.scene)
    }
    /// Emit a mesh or collection instance with visibility, evaluation and recursion checks.
    fn emit_object(
        &self,
        address: u64,
        placement: Transform,
        state: &mut GeometryState,
        depth: usize,
    ) -> Result<()> {
        state.emitted += 1;
        if depth > MAX_DEPTH || state.emitted > 100_000 {
            return Err("Blender scene exceeds object instance limits".into());
        }
        let object = self.record(address, 0, "Object")?;
        self.local_id(object)?;
        let visibility = self.optional_integer(object, "visibility_flag")?
            | self.optional_integer(object, "restrictflag")?;
        if visibility & 4 != 0 {
            return Ok(());
        }
        let kind = self.integer(object, "type")?;
        if matches!(kind, 10..=13 | 22 | 25) {
            return Ok(());
        }
        let flags = self.optional_integer(object, "transflag")?;
        if flags & ((1 << 4) | (1 << 9) | (1 << 11)) != 0 {
            return Err(
                "Blender generated vertex, face or particle instances require evaluation".into(),
            );
        }
        if flags & (1 << 8) != 0 {
            if kind != 0 {
                return Err(
                    "Blender collection instances on geometric objects are not supported".into(),
                );
            }
            let collection_address = if self.has(object, "instance_collection") {
                self.pointer(object, "instance_collection")?
            } else {
                self.pointer(object, "dup_group")?
            };
            if !state.active_instances.insert(collection_address) {
                return Err("cyclic Blender collection instancing".into());
            }
            let collection = self.record(collection_address, object.owner, "Collection")?;
            let offset_name = if self.has(collection, "instance_offset") {
                "instance_offset"
            } else {
                "dupli_ofs"
            };
            let offset = self.floats(collection, offset_name, 3)?;
            let mut translation = Transform::IDENTITY;
            translation.0[3][..3].copy_from_slice(&[-offset[0], -offset[1], -offset[2]]);
            let transform = placement
                .then(self.object_transform(
                    address,
                    &mut state.matrices,
                    &mut HashSet::new(),
                    0,
                )?)
                .then(translation);
            let mut objects = Vec::new();
            self.collection(
                collection_address,
                object.owner,
                &mut HashSet::new(),
                &mut HashSet::new(),
                &mut objects,
                0,
            )?;
            let mut seen = HashSet::new();
            for child in objects {
                if seen.insert(child) {
                    self.emit_object(child, transform, state, depth + 1)?;
                }
            }
            state.active_instances.remove(&collection_address);
            return Ok(());
        }
        if kind == 0 {
            return Ok(());
        }
        if kind != 1 {
            return Err(format!(
                "Blender geometric object type {kind} is not supported; convert it to a mesh"
            )
            .into());
        }
        for name in ["modifiers", "constraints", "particlesystem", "effect"] {
            if !self.list_empty(object, name)? {
                return Err(
                    format!("Blender mesh {name} require dependency-graph evaluation").into(),
                );
            }
        }
        let transform = placement.then(self.object_transform(
            address,
            &mut state.matrices,
            &mut HashSet::new(),
            0,
        )?);
        let mesh = self.record(self.pointer(object, "data")?, object.owner, "Mesh")?;
        self.mesh(mesh, transform, &mut state.scene)
    }
    /// Gather visible collection object links while rejecting cycles and excessive nesting.
    fn collection(
        &self,
        address: u64,
        owner: usize,
        active: &mut HashSet<u64>,
        visited: &mut HashSet<u64>,
        objects: &mut Vec<u64>,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH || !active.insert(address) {
            return Err("cyclic or deeply nested Blender collections".into());
        }
        if !visited.insert(address) {
            active.remove(&address);
            return Ok(());
        }
        let collection = self.record(address, owner, "Collection")?;
        self.local_id(collection)?;
        if self.optional_integer(collection, "flag")? & 8 == 0 {
            for link in self.list(collection, "gobject", "CollectionObject")? {
                objects.push(self.pointer(link, "ob")?);
            }
            for link in self.list(collection, "children", "CollectionChild")? {
                self.collection(
                    self.pointer(link, "collection")?,
                    collection.owner,
                    active,
                    visited,
                    objects,
                    depth + 1,
                )?;
            }
        }
        if objects.len() > 100_000 {
            return Err("Blender scene exceeds 100,000 object links".into());
        }
        active.remove(&address);
        Ok(())
    }
    /// Decode an exactly sized SDNA float array and reject non-finite entries.
    fn floats(&self, record: Record<'a>, name: &str, count: usize) -> Result<Vec<f64>> {
        let bytes = self.bytes(record, name)?;
        if bytes.len() != count * 4 || self.field(record, name)?.type_name != "float" {
            return Err("invalid Blender float array".into());
        }
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| self.endian.float(b))
            .collect()
    }
    /// Decode a column-major object matrix and require an exact affine homogeneous row.
    #[allow(
        clippy::float_cmp,
        reason = "The stored homogeneous row must encode an exact affine transform, rather than a nearly affine projective matrix."
    )]
    fn matrix(&self, record: Record<'a>, name: &str) -> Result<Transform> {
        let values = self.floats(record, name, 16)?;
        let matrix = Transform(std::array::from_fn(|c| {
            std::array::from_fn(|r| values[c * 4 + r])
        }));
        if matrix.0[0][3] != 0.
            || matrix.0[1][3] != 0.
            || matrix.0[2][3] != 0.
            || matrix.0[3][3] != 1.
        {
            return Err("non-affine Blender object transform".into());
        }
        Ok(matrix)
    }
    /// Resolve and cache world placement from saved matrices or supported static TRS and parents.
    fn object_transform(
        &self,
        address: u64,
        cache: &mut HashMap<u64, Transform>,
        active: &mut HashSet<u64>,
        depth: usize,
    ) -> Result<Transform> {
        if let Some(matrix) = cache.get(&address) {
            return Ok(*matrix);
        }
        if depth > MAX_DEPTH || !active.insert(address) {
            return Err("cyclic or deeply nested Blender object parents".into());
        }
        let object = self.record(address, 0, "Object")?;
        self.local_id(object)?;
        let transform = if self.has(object, "obmat") {
            self.matrix(object, "obmat")?
        } else {
            if self.optional_pointer(object, "adt")? != 0
                || self.optional_pointer(object, "ipo")? != 0
                || !self.list_empty(object, "constraints")?
            {
                return Err(
                    "Blender animated or constrained object transforms require evaluation".into(),
                );
            }
            let mut position = self.floats(object, "loc", 3)?;
            let delta_position = self.floats(object, "dloc", 3)?;
            for i in 0..3 {
                position[i] += delta_position[i];
            }
            let scale_name = if self.has(object, "scale") {
                "scale"
            } else {
                "size"
            };
            let mut scale = self.floats(object, scale_name, 3)?;
            let delta_scale = self.floats(object, "dscale", 3)?;
            for i in 0..3 {
                scale[i] *= delta_scale[i];
            }
            let mode = self.integer(object, "rotmode")?;
            let rotation = match mode {
                1 => {
                    let angles = self.floats(object, "rot", 3)?;
                    let delta = self.floats(object, "drot", 3)?;
                    euler_xyz(&delta).then(euler_xyz(&angles))
                }
                0 => quaternion(&self.floats(object, "dquat", 4)?)?.then(quaternion(&self.floats(object, "quat", 4)?)?),
                _ => return Err(format!("Blender rotation mode {mode} is not yet supported without a saved object matrix").into()),
            };
            let mut local = rotation;
            for (column, factor) in local.0[..3].iter_mut().zip(scale) {
                for value in &mut column[..3] {
                    *value *= factor;
                }
            }
            local.0[3][..3].copy_from_slice(&position);
            let parent = self.pointer(object, "parent")?;
            if parent == 0 {
                local
            } else {
                if self.integer(object, "partype")? & 15 != 0 {
                    return Err("Blender bone or vertex parenting requires evaluation".into());
                }
                self.object_transform(parent, cache, active, depth + 1)?
                    .then(self.matrix(object, "parentinv")?)
                    .then(local)
            }
        };
        active.remove(&address);
        cache.insert(address, transform);
        Ok(transform)
    }
    /// Find a legacy geometry array from its direct field or matching `CustomData` layer.
    fn geometry_pointer(
        &self,
        mesh: Record<'a>,
        direct: &str,
        domain: &str,
        kind: i64,
    ) -> Result<u64> {
        let pointer = self.optional_pointer(mesh, direct)?;
        if pointer != 0 {
            return Ok(pointer);
        }
        if !self.has(mesh, domain) {
            return Ok(0);
        }
        let custom = self.embedded(mesh, domain)?;
        let count = self.count(custom, &["totlayer"])?;
        for layer in self.array(
            self.pointer(custom, "layers")?,
            mesh.owner,
            "CustomDataLayer",
            count,
        )? {
            if self.integer(layer, "type")? == kind {
                return self.pointer(layer, "data");
            }
        }
        Ok(0)
    }
    /// Resolve a packed attribute array and verify its exact domain count and stride.
    fn raw_array(
        &self,
        pointer: u64,
        owner: usize,
        count: usize,
        stride: usize,
    ) -> Result<&'a [u8]> {
        if count == 0 {
            return Ok(&[]);
        }
        let bytes = self.block(pointer, owner)?.data;
        let expected = count
            .checked_mul(stride)
            .ok_or("Blender attribute array overflow")?;
        if bytes.len() != expected {
            return Err("Blender attribute array size differs from its geometry domain".into());
        }
        Ok(bytes)
    }
    /// Find a named geometry attribute in modern or legacy storage with checked type and domain.
    fn attribute(
        &self,
        mesh: Record<'a>,
        spec: AttributeSpec,
        count: usize,
    ) -> Result<Option<&'a [u8]>> {
        let AttributeSpec {
            domain,
            name,
            legacy_type,
            modern_type,
            modern_domain,
            stride,
        } = spec;
        if self.has(mesh, "attribute_storage") {
            let storage = self.embedded(mesh, "attribute_storage")?;
            let n = self.count(storage, &["dna_attributes_num"])?;
            for attribute in self.array(
                self.pointer(storage, "dna_attributes")?,
                mesh.owner,
                "Attribute",
                n,
            )? {
                let text = self
                    .block(self.pointer(attribute, "name")?, mesh.owner)?
                    .data;
                let end = text
                    .iter()
                    .position(|b| *b == 0)
                    .ok_or("unterminated Blender attribute name")?;
                if &text[..end] != name.as_bytes() {
                    continue;
                }
                if self.integer(attribute, "data_type")? != modern_type
                    || self.integer(attribute, "domain")? != modern_domain
                    || self.integer(attribute, "storage_type")? != 0
                {
                    return Err(format!("unsupported Blender {name} attribute storage").into());
                }
                let array = self.record(
                    self.pointer(attribute, "data")?,
                    mesh.owner,
                    "AttributeArray",
                )?;
                if self.count(array, &["size"])? != count
                    || self.optional_integer(array, "is_single")? != 0
                {
                    return Err("Blender geometry attribute is not a full array".into());
                }
                return Ok(Some(self.raw_array(
                    self.pointer(array, "data")?,
                    mesh.owner,
                    count,
                    stride,
                )?));
            }
        }
        if self.has(mesh, domain) {
            let custom = self.embedded(mesh, domain)?;
            let n = self.count(custom, &["totlayer"])?;
            for layer in self.array(
                self.pointer(custom, "layers")?,
                mesh.owner,
                "CustomDataLayer",
                n,
            )? {
                let text = self.bytes(layer, "name")?;
                let end = text.iter().position(|b| *b == 0).unwrap_or(text.len());
                if &text[..end] == name.as_bytes() && self.integer(layer, "type")? == legacy_type {
                    return Ok(Some(self.raw_array(
                        self.pointer(layer, "data")?,
                        mesh.owner,
                        count,
                        stride,
                    )?));
                }
            }
        }
        Ok(None)
    }
    /// Decode mesh positions, faces and edges, triangulating polygons and retaining loose geometry.
    #[allow(
        clippy::too_many_lines,
        reason = "Legacy and modern arrays share topology and loose-geometry accounting; retain their common validation and emission order."
    )]
    fn mesh(&self, mesh: Record<'a>, transform: Transform, scene: &mut Scene) -> Result<()> {
        self.local_id(mesh)?;
        if self.optional_pointer(mesh, "key")? != 0
            || self.optional_pointer(mesh, "adt")? != 0
            || self.optional_pointer(mesh, "mr")? != 0
        {
            return Err("Blender shape keys, animated meshes or multiresolution geometry require evaluation".into());
        }
        let vertex_count = self.count(mesh, &["verts_num", "totvert"])?;
        let face_count = if self.has(mesh, "totpoly") || self.has(mesh, "faces_num") {
            self.count(mesh, &["faces_num", "totpoly"])?
        } else {
            0
        };
        let edge_count = self.count(mesh, &["edges_num", "totedge"])?;
        let old_vertices = self.geometry_pointer(mesh, "mvert", "vdata", 0)?;
        let vertices: Vec<Vec3> = if old_vertices != 0 {
            self.array(old_vertices, mesh.owner, "MVert", vertex_count)?
                .into_iter()
                .map(|vertex| {
                    let name = if self.has(vertex, "co") {
                        "co"
                    } else {
                        "co_legacy"
                    };
                    let v = self.floats(vertex, name, 3)?;
                    Ok(Vec3::new(v[0], v[1], v[2]))
                })
                .collect::<Result<_>>()?
        } else {
            let raw = self
                .attribute(mesh, POSITION, vertex_count)?
                .ok_or("Blender mesh has no vertex positions")?;
            raw.as_chunks::<12>()
                .0
                .iter()
                .map(|v| {
                    Ok(Vec3::new(
                        self.endian.float(&v[..4])?,
                        self.endian.float(&v[4..8])?,
                        self.endian.float(&v[8..])?,
                    ))
                })
                .collect::<Result<_>>()?
        };
        let mut used_vertices = vec![false; vertex_count];
        let mut face_edges = HashSet::new();
        let mut polygons = Vec::new();
        if face_count > 0 {
            let corner_count = self.count(mesh, &["corners_num", "totloop"])?;
            let loops = self.geometry_pointer(mesh, "mloop", "ldata", 26)?;
            let corners: Vec<usize> = if loops != 0 {
                self.array(loops, mesh.owner, "MLoop", corner_count)?
                    .into_iter()
                    .map(|r| usize::try_from(self.integer(r, "v")?).map_err(Into::into))
                    .collect::<Result<_>>()?
            } else {
                let bytes = self
                    .attribute(mesh, CORNER_VERTEX, corner_count)?
                    .ok_or("Blender mesh has no face corner indices")?;
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| usize::try_from(self.endian.int(b)?).map_err(Into::into))
                    .collect::<Result<_>>()?
            };
            let polys = self.geometry_pointer(mesh, "mpoly", "pdata", 25)?;
            if polys != 0 {
                for poly in self.array(polys, mesh.owner, "MPoly", face_count)? {
                    let start = self.count(poly, &["loopstart"])?;
                    let n = self.count(poly, &["totloop"])?;
                    polygons.push(slice_indices(&corners, start, n)?.to_vec());
                }
            } else {
                let offsets_name = if self.has(mesh, "face_offset_indices") {
                    "face_offset_indices"
                } else {
                    "poly_offset_indices"
                };
                let bytes = self.raw_array(
                    self.pointer(mesh, offsets_name)?,
                    mesh.owner,
                    face_count + 1,
                    4,
                )?;
                let offsets: Vec<usize> = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| usize::try_from(self.endian.int(b)?).map_err(Into::into))
                    .collect::<Result<_>>()?;
                if offsets[0] != 0 || offsets[face_count] != corner_count {
                    return Err("invalid Blender face offset endpoints".into());
                }
                for pair in offsets.windows(2) {
                    let n = pair[1]
                        .checked_sub(pair[0])
                        .ok_or("descending Blender face offsets")?;
                    polygons.push(slice_indices(&corners, pair[0], n)?.to_vec());
                }
            }
        } else {
            let count = self.count(mesh, &["totface_legacy", "totface"])?;
            let faces = self.geometry_pointer(mesh, "mface", "fdata", 4)?;
            for face in self.array(faces, mesh.owner, "MFace", count)? {
                let mut ids: Vec<usize> = ["v1", "v2", "v3", "v4"]
                    .iter()
                    .map(|n| usize::try_from(self.integer(face, n)?).map_err(Into::into))
                    .collect::<Result<_>>()?;
                if ids[3] == 0 {
                    ids.pop();
                }
                polygons.push(ids);
            }
        }
        let mut corner_total = 0usize;
        for polygon in polygons {
            corner_total = corner_total
                .checked_add(polygon.len())
                .ok_or("Blender face corner count overflow")?;
            if corner_total > MAX_ITEMS || polygon.len() < 3 {
                return Err("invalid or oversized Blender polygon topology".into());
            }
            let mut positions = Vec::with_capacity(polygon.len());
            for &index in &polygon {
                let position = *vertices
                    .get(index)
                    .ok_or("Blender face vertex index out of bounds")?;
                used_vertices[index] = true;
                positions.push(transform.apply(position));
            }
            for (a, b) in polygon
                .iter()
                .zip(polygon.iter().cycle().skip(1))
                .take(polygon.len())
            {
                face_edges.insert(((*a).min(*b), (*a).max(*b)));
            }
            let triangles = triangulate_polygon(&positions)?;
            if scene.triangles.len() + triangles.len() > MAX_ITEMS {
                return Err("Blender scene exceeds five million triangles".into());
            }
            scene
                .triangles
                .extend(triangles.into_iter().map(|vertices| Triangle {
                    vertices,
                    color: None,
                }));
        }
        let old_edges = self.geometry_pointer(mesh, "medge", "edata", 3)?;
        let edges: Vec<[usize; 2]> = if old_edges != 0 {
            self.array(old_edges, mesh.owner, "MEdge", edge_count)?
                .into_iter()
                .map(|edge| {
                    Ok([
                        usize::try_from(self.integer(edge, "v1")?)?,
                        usize::try_from(self.integer(edge, "v2")?)?,
                    ])
                })
                .collect::<Result<_>>()?
        } else if edge_count > 0 {
            let bytes = self
                .attribute(mesh, EDGE_VERTICES, edge_count)?
                .ok_or("Blender mesh has no edge indices")?;
            bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|b| {
                    Ok([
                        usize::try_from(self.endian.int(&b[..4])?)?,
                        usize::try_from(self.endian.int(&b[4..])?)?,
                    ])
                })
                .collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        for [a, b] in edges {
            let va = *vertices.get(a).ok_or("Blender edge index out of bounds")?;
            let vb = *vertices.get(b).ok_or("Blender edge index out of bounds")?;
            used_vertices[a] = true;
            used_vertices[b] = true;
            if !face_edges.contains(&(a.min(b), a.max(b))) {
                if scene.lines.len() >= MAX_ITEMS {
                    return Err("Blender scene exceeds five million loose edges".into());
                }
                scene.lines.push(Line {
                    vertices: [transform.apply(va), transform.apply(vb)],
                    color: None,
                });
            }
        }
        for (position, used) in vertices.into_iter().zip(used_vertices) {
            if !used {
                if scene.points.len() >= MAX_ITEMS {
                    return Err("Blender scene exceeds five million loose vertices".into());
                }
                scene.points.push(Point {
                    position: transform.apply(position),
                    color: None,
                });
            }
        }
        Ok(())
    }
}
/// Borrow a checked face-corner range without allowing index addition overflow.
fn slice_indices(indices: &[usize], start: usize, count: usize) -> Result<&[usize]> {
    let end = start
        .checked_add(count)
        .ok_or("Blender face range overflow")?;
    indices
        .get(start..end)
        .ok_or_else(|| "Blender face range exceeds corner array".into())
}
/// Build a rotation matrix from three XYZ Euler angles in radians.
fn euler_xyz(angles: &[f64]) -> Transform {
    let (sx, cx) = angles[0].sin_cos();
    let (sy, cy) = angles[1].sin_cos();
    let (sz, cz) = angles[2].sin_cos();
    Transform([
        [cy * cz, cy * sz, -sy, 0.],
        [sx * sy * cz - cx * sz, sx * sy * sz + cx * cz, sx * cy, 0.],
        [cx * sy * cz + sx * sz, cx * sy * sz - sx * cz, cx * cy, 0.],
        [0., 0., 0., 1.],
    ])
}
/// Normalize a nonzero Blender WXYZ quaternion and build its rotation matrix.
fn quaternion(values: &[f64]) -> Result<Transform> {
    let length = values.iter().map(|v| v * v).sum::<f64>().sqrt();
    if length <= f64::EPSILON {
        return Err("zero Blender rotation quaternion".into());
    }
    let [w, x, y, z] = std::array::from_fn(|i| values[i] / length);
    Ok(Transform([
        [
            1. - 2. * (y * y + z * z),
            2. * (x * y + w * z),
            2. * (x * z - w * y),
            0.,
        ],
        [
            2. * (x * y - w * z),
            1. - 2. * (x * x + z * z),
            2. * (y * z + w * x),
            0.,
        ],
        [
            2. * (x * z + w * y),
            2. * (y * z - w * x),
            1. - 2. * (x * x + y * y),
            0.,
        ],
        [0., 0., 0., 1.],
    ]))
}

/// Authored binary-layout checks and retained-file transform qualification.
#[cfg(test)]
mod tests {
    use super::*;
    /// Append the selected byte width of an authored integer in the requested byte order.
    fn number(output: &mut Vec<u8>, value: u64, bytes: usize, endian: Endian) {
        match endian {
            Endian::Little => output.extend_from_slice(&value.to_le_bytes()[..bytes]),
            Endian::Big => output.extend_from_slice(&value.to_be_bytes()[8 - bytes..]),
        }
    }
    /// Append an authored `BHead` header and payload in the selected file layout.
    fn block(
        output: &mut Vec<u8>,
        code: [u8; 4],
        address: u64,
        schema: usize,
        count: usize,
        data: &[u8],
        layout: (usize, Endian, bool),
    ) {
        let (pointer, endian, large) = layout;
        output.extend_from_slice(&code);
        if large {
            number(output, schema as u64, 4, endian);
            number(output, address, 8, endian);
            number(output, data.len() as u64, 8, endian);
            number(output, count as u64, 8, endian);
        } else {
            number(output, data.len() as u64, 4, endian);
            number(output, address, pointer, endian);
            number(output, schema as u64, 4, endian);
            number(output, count as u64, 4, endian);
        }
        output.extend_from_slice(data);
    }
    /// Construct a complete static triangle file with matching SDNA and configurable `BHead` layout.
    #[allow(
        clippy::too_many_lines,
        reason = "Construct one internally consistent authored SDNA and its matching BHead payloads for every tested layout."
    )]
    fn authored(pointer: usize, endian: Endian, large: bool) -> Vec<u8> {
        let definitions = [
            ("ID", vec![("void", "*lib")]),
            ("ListBase", vec![("void", "*first"), ("void", "*last")]),
            (
                "Scene",
                vec![("ID", "id"), ("Scene", "*set"), ("ListBase", "base")],
            ),
            (
                "Base",
                vec![("Base", "*next"), ("Base", "*prev"), ("Object", "*object")],
            ),
            (
                "Object",
                vec![
                    ("ID", "id"),
                    ("int", "type"),
                    ("int", "_pad"),
                    ("void", "*data"),
                    ("float", "obmat[4][4]"),
                ],
            ),
            (
                "Mesh",
                vec![
                    ("ID", "id"),
                    ("int", "totvert"),
                    ("int", "totedge"),
                    ("int", "totface"),
                    ("int", "_pad"),
                    ("MVert", "*mvert"),
                    ("MEdge", "*medge"),
                    ("MFace", "*mface"),
                ],
            ),
            ("MVert", vec![("float", "co[3]")]),
            (
                "MFace",
                vec![("int", "v1"), ("int", "v2"), ("int", "v3"), ("int", "v4")],
            ),
        ];
        let mut types = vec!["void", "int", "float", "MEdge"];
        types.extend(definitions.iter().map(|d| d.0));
        let lengths = [
            0,
            4,
            4,
            8,
            pointer,
            2 * pointer,
            4 * pointer,
            3 * pointer,
            2 * pointer + 72,
            4 * pointer + 16,
            12,
            16,
        ];
        let mut names = Vec::new();
        for (_, fields) in &definitions {
            for (_, name) in fields {
                if !names.contains(name) {
                    names.push(*name);
                }
            }
        }
        let mut dna = b"SDNANAME".to_vec();
        number(&mut dna, names.len() as u64, 4, endian);
        for name in &names {
            dna.extend_from_slice(name.as_bytes());
            dna.push(0);
        }
        while !dna.len().is_multiple_of(4) {
            dna.push(0);
        }
        dna.extend_from_slice(b"TYPE");
        number(&mut dna, types.len() as u64, 4, endian);
        for name in &types {
            dna.extend_from_slice(name.as_bytes());
            dna.push(0);
        }
        while !dna.len().is_multiple_of(4) {
            dna.push(0);
        }
        dna.extend_from_slice(b"TLEN");
        for n in lengths {
            number(&mut dna, n as u64, 2, endian);
        }
        while !dna.len().is_multiple_of(4) {
            dna.push(0);
        }
        dna.extend_from_slice(b"STRC");
        number(&mut dna, definitions.len() as u64, 4, endian);
        for (name, fields) in &definitions {
            number(
                &mut dna,
                types.iter().position(|n| n == name).unwrap() as u64,
                2,
                endian,
            );
            number(&mut dna, fields.len() as u64, 2, endian);
            for (kind, name) in fields {
                number(
                    &mut dna,
                    types.iter().position(|n| n == kind).unwrap() as u64,
                    2,
                    endian,
                );
                number(
                    &mut dna,
                    names.iter().position(|n| n == name).unwrap() as u64,
                    2,
                    endian,
                );
            }
        }
        let mut output = if large {
            b"BLENDER17-01v0502".to_vec()
        } else {
            let mut header = b"BLENDER".to_vec();
            header.push(if pointer == 8 { b'-' } else { b'_' });
            header.push(if matches!(endian, Endian::Little) {
                b'v'
            } else {
                b'V'
            });
            header.extend_from_slice(b"252");
            header
        };
        let layout = (pointer, endian, large);
        // TEST can contain arbitrary unrelated bytes; the geometry reader does
        // not decode image data or require a valid saved-thumbnail carrier.
        block(&mut output, *b"TEST", 0, 0, 1, b"not image pixels", layout);
        let mut scene = Vec::new();
        for n in [0, 0, 2000, 2000] {
            number(&mut scene, n, pointer, endian);
        }
        block(&mut output, *b"SC\0\0", 1000, 2, 1, &scene, layout);
        let mut base = Vec::new();
        for n in [0, 0, 3000] {
            number(&mut base, n, pointer, endian);
        }
        block(&mut output, *b"DATA", 2000, 3, 1, &base, layout);
        let mut object = Vec::new();
        number(&mut object, 0, pointer, endian);
        number(&mut object, 1, 4, endian);
        number(&mut object, 0, 4, endian);
        number(&mut object, 4000, pointer, endian);
        for value in [
            2.0_f32, 0., 0., 0., 0., 3., 0., 0., 0., 0., 4., 0., 10., 20., 30., 1.,
        ] {
            number(&mut object, u64::from(value.to_bits()), 4, endian);
        }
        block(&mut output, *b"OB\0\0", 3000, 4, 1, &object, layout);
        let mut mesh = Vec::new();
        number(&mut mesh, 0, pointer, endian);
        for n in [3, 0, 1, 0] {
            number(&mut mesh, n, 4, endian);
        }
        for n in [5000, 0, 6000] {
            number(&mut mesh, n, pointer, endian);
        }
        block(&mut output, *b"ME\0\0", 4000, 5, 1, &mesh, layout);
        let mut vertices = Vec::new();
        for value in [0.0_f32, 0., 0., 1., 0., 0., 0., 1., 0.] {
            number(&mut vertices, u64::from(value.to_bits()), 4, endian);
        }
        block(&mut output, *b"DATA", 5000, 6, 3, &vertices, layout);
        let mut face = Vec::new();
        for n in [0, 1, 2, 0] {
            number(&mut face, n, 4, endian);
        }
        block(&mut output, *b"DATA", 6000, 7, 1, &face, layout);
        block(&mut output, *b"DNA1", 0, 0, 1, &dna, layout);
        block(&mut output, *b"ENDB", 0, 0, 0, &[], layout);
        output
    }
    #[test]
    fn sdna_decodes_pointer_width_byte_order_and_bhead_generation() {
        for layout in [
            (4, Endian::Little, false),
            (8, Endian::Little, false),
            (4, Endian::Big, false),
            (8, Endian::Big, false),
            (8, Endian::Little, true),
        ] {
            let bytes = authored(layout.0, layout.1, layout.2);
            let scene = Database::parse(&bytes).unwrap().scene().unwrap();
            assert_eq!(scene.triangles.len(), 1);
            assert_eq!(
                scene.triangles[0].vertices,
                [
                    Vec3::new(10., 20., 30.),
                    Vec3::new(12., 20., 30.),
                    Vec3::new(10., 23., 30.)
                ]
            );
            assert!(scene.lines.is_empty() && scene.points.is_empty());
        }
    }
    #[test]
    fn corrupt_counts_and_pointer_topology_fail_without_partial_geometry() {
        let source = authored(8, Endian::Little, false);
        let file = Database::parse(&source).unwrap();
        let mesh = file.record(4000, 0, "Mesh").unwrap();
        let base = file.record(2000, 1, "Base").unwrap();
        let offset = |record: Record<'_>, name: &str| {
            record.data.as_ptr() as usize - source.as_ptr() as usize
                + file.field(record, name).unwrap().offset
        };
        for (start, replacement, message) in [
            (
                offset(mesh, "totvert"),
                (-1_i32).to_le_bytes().to_vec(),
                "negative",
            ),
            (
                offset(mesh, "totvert"),
                i32::try_from(MAX_ITEMS + 1).unwrap().to_le_bytes().to_vec(),
                "five million",
            ),
            (
                offset(mesh, "mvert"),
                9999_u64.to_le_bytes().to_vec(),
                "unresolved",
            ),
            (
                offset(base, "next"),
                2000_u64.to_le_bytes().to_vec(),
                "cyclic",
            ),
        ] {
            let mut bytes = source.clone();
            bytes[start..start + replacement.len()].copy_from_slice(&replacement);
            let error = Database::parse(&bytes)
                .unwrap()
                .scene()
                .unwrap_err()
                .to_string();
            assert!(error.contains(message), "{error}");
        }
        let face = file.block(6000, mesh.owner).unwrap();
        let start = face.data.as_ptr() as usize - source.as_ptr() as usize;
        let mut bytes = source.clone();
        bytes[start..start + 4].copy_from_slice(&8_i32.to_le_bytes());
        assert!(
            Database::parse(&bytes)
                .unwrap()
                .scene()
                .unwrap_err()
                .to_string()
                .contains("out of bounds")
        );
    }
    #[test]
    fn invalid_global_block_schema_is_an_error_before_optional_field_access() {
        let mut bytes = authored(8, Endian::Little, false);
        let invalid_schema = Database::parse(&bytes).unwrap().schemas.len();
        // Replace the authored terminal block with GLOB followed by ENDB.
        // GLOB participates in active-scene selection without pointer lookup.
        bytes.truncate(bytes.len() - 24);
        let layout = (8, Endian::Little, false);
        block(&mut bytes, *b"GLOB", 0, invalid_schema, 1, &[0; 8], layout);
        block(&mut bytes, *b"ENDB", 0, 0, 0, &[], layout);
        let error = Database::parse(&bytes)
            .unwrap()
            .scene()
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid Blender SDNA index"), "{error}");
    }

    #[test]
    fn truncation_bad_dna_and_nonfinite_geometry_are_errors() {
        let source = authored(8, Endian::Little, false);
        for end in [0, 7, 11, 16, 30, source.len() - 1, source.len() - 24] {
            assert!(Database::parse(&source[..end]).is_err());
        }
        let file = Database::parse(&source).unwrap();
        let vertex = file.block(5000, file.ids[&4000]).unwrap();
        let start = vertex.data.as_ptr() as usize - source.as_ptr() as usize;
        let mut bytes = source.clone();
        bytes[start..start + 4].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
        assert!(
            Database::parse(&bytes)
                .unwrap()
                .scene()
                .unwrap_err()
                .to_string()
                .contains("non-finite")
        );
        let start = source.windows(8).position(|b| b == b"SDNANAME").unwrap();
        let mut bytes = source;
        bytes[start] = b'X';
        assert!(Database::parse(&bytes).is_err());
    }
    #[test]
    fn modern_rotation_scale_delta_and_parent_matrices_are_composed() {
        let source = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/native_geometry/blend/hierarchy-502-no-thumbnail.blend"),
        )
        .unwrap();
        let file = Database::parse(&source).unwrap();
        let address = file
            .blocks
            .iter()
            .find(|b| {
                if file
                    .schemas
                    .get(b.schema)
                    .is_none_or(|s| s.name != "Object")
                    || b.code == *b"DATA"
                {
                    return false;
                }
                let record = Record {
                    schema: b.schema,
                    data: b.data,
                    owner: b.owner,
                };
                let id = file.embedded(record, "id").unwrap();
                let name = file.bytes(id, "name").unwrap();
                name.split(|b| *b == 0).next() == Some(b"OBCube".as_slice())
            })
            .unwrap()
            .address;
        let object = file.record(address, 0, "Object").unwrap();
        let mut bytes = source.clone();
        for (name, values) in [
            ("loc", [3.0_f32, -2., 1.]),
            ("dloc", [1., 0., 0.]),
            ("size", [2., 3., 4.]),
            ("dscale", [1., 2., 1.]),
            ("rot", [0., 0., std::f32::consts::FRAC_PI_2]),
            ("drot", [0., 0., 0.]),
        ] {
            let start = object.data.as_ptr() as usize - source.as_ptr() as usize
                + file.field(object, name).unwrap().offset;
            for (i, value) in values.into_iter().enumerate() {
                bytes[start + i * 4..start + (i + 1) * 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        let file = Database::parse(&bytes).unwrap();
        let matrix = file
            .object_transform(address, &mut HashMap::new(), &mut HashSet::new(), 0)
            .unwrap();
        for (point, expected) in [
            (Vec3::new(0., 0., 0.), Vec3::new(4., -2., 1.)),
            (Vec3::new(1., 0., 0.), Vec3::new(4., 0., 1.)),
            (Vec3::new(0., 1., 0.), Vec3::new(-2., -2., 1.)),
            (Vec3::new(0., 0., 1.), Vec3::new(4., -2., 5.)),
        ] {
            assert!((matrix.apply(point) - expected).length() < 1e-6);
        }
        // The real Sphere is parented to an empty with a nonidentity parent
        // inverse. Its measured world bounds in the integration test catch
        // either dropping or double-applying that parent's translation.
    }
    #[test]
    fn real_zstandard_frames_and_thumbnail_removal_preserve_geometry() {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/saved-previews/blend_dwg/blend-extras.blend");
        let decoded = decompress(&std::fs::read(source).unwrap()).unwrap();
        assert!(decoded.starts_with(b"BLENDER17-01v0502"));
        let error = Database::parse(&decoded)
            .unwrap()
            .scene()
            .unwrap_err()
            .to_string();
        assert!(error.contains("modifiers"), "{error}");
    }
}
