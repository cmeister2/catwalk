//! Bounded loading of filtered HDF5 indexes whose chunk lengths use 1, 3, 5, 6 or 7 bytes.
//!
//! Upstream 0.9.1 treats these per-record lengths as file-wide length fields.
//! This adapter emits upstream `ChunkEntry` objects and leaves filters and
//! numeric decoding to upstream APIs. Index layouts follow Appendix C of
//! <https://support.hdfgroup.org/documentation/hdf5/latest/_f_m_t4.html>.
//! Layout/traversal compatibility was checked against hdf5-reader 0.9.1,
//! Copyright its contributors, licensed MIT OR Apache-2.0.

use crate::Result;
use hdf5_reader::{
    btree_v2::BTreeV2Header, checksum::jenkins_lookup3, chunk_index::ChunkEntry, io::Cursor,
    messages::layout::ChunkIndexing,
};
use std::collections::HashSet;

/// Maximum index slots visited, matching the scientific-array sample ceiling.
const MAX_SLOTS: usize = 16_777_216;
/// Maximum aggregate entry and coordinate-vector capacity before decoding geometry arrays.
const MAX_INDEX_BYTES: usize = 512 * 1024 * 1024;

/// Dimensions and address widths supplied by upstream dataset and superblock metadata.
pub(super) struct Index<'a> {
    /// Complete bounded input bytes.
    pub data: &'a [u8],
    /// File-wide address width.
    pub offset_size: u8,
    /// File-wide length width.
    pub length_size: u8,
    /// Current dataset extents in C array order.
    pub shape: &'a [u64],
    /// Declared maximum extents; `u64::MAX` marks an unlimited dataset axis.
    pub max_shape: Option<&'a [u64]>,
    /// Chunk extents in the same order.
    pub dims: &'a [u32],
}

impl Index<'_> {
    /// Resolve affected indexes; return `None` for layouts handled unchanged upstream.
    ///
    /// # Errors
    /// Returns an error for invalid metadata/checksums, truncated records,
    /// excessive index work, cyclic trees or overflowing coordinates.
    pub fn entries(&self, address: u64, kind: &ChunkIndexing) -> Result<Option<Vec<ChunkEntry>>> {
        if self.shape.len() != self.dims.len()
            || self.shape.is_empty()
            || self.shape.len() > 64
            || self.dims.contains(&0)
        {
            return Err("HDF chunk index dimensions are invalid".into());
        }
        match kind {
            ChunkIndexing::FixedArray { .. } => self.fixed(address),
            ChunkIndexing::ExtensibleArray { .. } => self.extensible(address),
            ChunkIndexing::BTreeV2 => self.btree(address),
            _ => Ok(None),
        }
    }

    /// Create a bounds-checked cursor at an absolute address.
    fn cursor(&self, address: u64) -> Result<Cursor<'_>> {
        Ok(Cursor::new(self.data).at_offset(address)?)
    }

    /// Validate an index signature and its version-zero/client identifier prefix.
    fn prefix(cursor: &mut Cursor<'_>, signature: [u8; 4]) -> Result<u8> {
        if cursor.read_bytes(4)? != signature.as_slice() || cursor.read_u8()? != 0 {
            return Err("invalid HDF filtered chunk index signature/version".into());
        }
        Ok(cursor.read_u8()?)
    }

    /// Validate a checksum covering one metadata header before its trailing word.
    fn checksum(cursor: &mut Cursor<'_>, start: u64) -> Result<()> {
        let start = usize::try_from(start)?;
        let end = usize::try_from(cursor.position())?;
        let bytes = cursor
            .data()
            .get(start..end)
            .ok_or("HDF checksum range is invalid")?;
        let actual = jenkins_lookup3(bytes);
        if cursor.read_u32_le()? != actual {
            return Err("HDF filtered chunk index checksum mismatch".into());
        }
        Ok(())
    }

    /// Derive a filtered record's variable size width, distinguishing upstream-safe widths.
    fn size_width(&self, record_size: usize) -> Result<usize> {
        let width = record_size
            .checked_sub(usize::from(self.offset_size) + 4)
            .filter(|width| (1..=8).contains(width))
            .ok_or("invalid HDF filtered chunk-size field width")?;
        Ok(width)
    }

    /// Whether a stored address is the format's unallocated-entry sentinel.
    fn absent(&self, address: u64) -> bool {
        Cursor::is_undefined_offset(address, self.offset_size)
    }

    /// Validate the maximum extents used by fixed and extensible array slot strides.
    fn array_dimensions(&self, extensible: bool) -> Result<()> {
        let maximum = self.max_shape.unwrap_or(self.shape);
        if maximum.len() != self.shape.len()
            || maximum
                .iter()
                .zip(self.shape)
                .any(|(&maximum, &current)| maximum < current)
        {
            return Err("HDF chunk index maximum dimensions are invalid".into());
        }
        let unlimited = maximum.iter().filter(|&&extent| extent == u64::MAX).count();
        if unlimited != usize::from(extensible) {
            return Err("HDF array index has an invalid number of unlimited dimensions".into());
        }
        Ok(())
    }

    /// Invert maximum-extent slot strides, moving an unlimited axis to the slowest position.
    fn offsets(&self, linear: usize) -> Result<Vec<u64>> {
        let mut linear = u64::try_from(linear)?;
        let maximum = self.max_shape.unwrap_or(self.shape);
        let unlimited = maximum.iter().position(|&extent| extent == u64::MAX);
        let mut offsets = vec![0; self.shape.len()];
        for axis in (0..self.shape.len()).rev() {
            if Some(axis) == unlimited {
                continue;
            }
            let dim = u64::from(self.dims[axis]);
            let width = maximum[axis].div_ceil(dim);
            if width == 0 {
                return Err("HDF chunk index has an empty dimension".into());
            }
            offsets[axis] = (linear % width)
                .checked_mul(dim)
                .ok_or("HDF chunk index coordinate overflows")?;
            linear /= width;
        }
        if let Some(axis) = unlimited {
            offsets[axis] = linear
                .checked_mul(u64::from(self.dims[axis]))
                .ok_or("HDF unlimited chunk index coordinate overflows")?;
        } else if linear != 0 {
            return Err("allocated HDF chunk lies outside dataset dimensions".into());
        }
        Ok(offsets)
    }

    /// Read an array entry using the public variable-width cursor API.
    fn record(&self, cursor: &mut Cursor<'_>, width: usize) -> Result<ChunkEntry> {
        Ok(ChunkEntry {
            address: cursor.read_offset(self.offset_size)?,
            size: cursor.read_uvar(width)?,
            filter_mask: cursor.read_u32_le()?,
            offsets: Vec::new(),
        })
    }

    /// Advance over unallocated slots without allocating placeholder records.
    fn advance(linear: &mut usize, count: usize) -> Result<()> {
        *linear = linear
            .checked_add(count)
            .filter(|value| *value <= MAX_SLOTS)
            .ok_or("HDF filtered chunk index exceeds slot limit")?;
        Ok(())
    }

    /// Append allocated array records and retain their logical positions across holes.
    fn records(
        &self,
        cursor: &mut Cursor<'_>,
        width: usize,
        count: usize,
        linear: &mut usize,
        entries: &mut Vec<ChunkEntry>,
    ) -> Result<()> {
        if count > MAX_SLOTS.saturating_sub(*linear) {
            return Err("HDF filtered chunk index exceeds slot limit".into());
        }
        for _ in 0..count {
            let mut entry = self.record(cursor, width)?;
            if !self.absent(entry.address) {
                entry.offsets = self.offsets(*linear)?;
                self.append(entries, entry)?;
            }
            *linear += 1;
        }
        Ok(())
    }

    /// Reserve bounded entry/vector capacity before appending one allocated chunk.
    fn append(&self, entries: &mut Vec<ChunkEntry>, entry: ChunkEntry) -> Result<()> {
        let cost = self
            .shape
            .len()
            .checked_mul(std::mem::size_of::<u64>())
            .and_then(|size| size.checked_add(std::mem::size_of::<ChunkEntry>()))
            .ok_or("HDF chunk index allocation overflows")?;
        let maximum = (MAX_INDEX_BYTES / cost).min(MAX_SLOTS);
        if entries.len() >= maximum {
            return Err("HDF chunk index exceeds 512 MiB allocation limit".into());
        }
        if entries.len() == entries.capacity() {
            let additional = entries.capacity().max(1).min(maximum - entries.len());
            entries.try_reserve_exact(additional)?;
        }
        entries.push(entry);
        Ok(())
    }

    /// Test an MSB-first page-initialization flag without reading outside its bitmap.
    fn initialized(bitmap: &[u8], bit: usize) -> Result<bool> {
        Ok(bitmap.get(bit / 8).ok_or("HDF page bitmap is truncated")? & (0x80 >> (bit % 8)) != 0)
    }

    /// Read fixed-array pages at their physical slots, validating prefix and page checksums.
    #[allow(
        clippy::too_many_arguments,
        reason = "The page reader needs the enclosing block position, logical slot position and output budget alongside the on-disk entry parameters."
    )]
    fn pages(
        &self,
        cursor: &mut Cursor<'_>,
        block: u64,
        count: usize,
        page_bits: u8,
        width: usize,
        linear: &mut usize,
        entries: &mut Vec<ChunkEntry>,
    ) -> Result<()> {
        if count > MAX_SLOTS.saturating_sub(*linear) {
            return Err("HDF filtered chunk index exceeds slot limit".into());
        }
        let page_size = 1usize
            .checked_shl(u32::from(page_bits))
            .ok_or("HDF page size overflows")?;
        if count <= page_size {
            self.records(cursor, width, count, linear, entries)?;
            Self::checksum(cursor, block)?;
            return Ok(());
        }
        let pages = count.div_ceil(page_size);
        let bitmap = cursor.read_bytes(pages.div_ceil(8))?.to_vec();
        Self::checksum(cursor, block)?;
        let base = cursor.position();
        let stride = page_size
            .checked_mul(usize::from(self.offset_size) + width + 4)
            .and_then(|size| size.checked_add(4))
            .ok_or("HDF fixed page stride overflows")?;
        for page in 0..pages {
            let count = page_size.min(count - page * page_size);
            if Self::initialized(&bitmap, page)? {
                let address = base
                    .checked_add(u64::try_from(
                        page.checked_mul(stride)
                            .ok_or("HDF page offset overflows")?,
                    )?)
                    .ok_or("HDF page address overflows")?;
                let mut cursor = self.cursor(address)?;
                self.records(&mut cursor, width, count, linear, entries)?;
                Self::checksum(&mut cursor, address)?;
            } else {
                Self::advance(linear, count)?;
            }
        }
        Ok(())
    }

    /// Load the fixed-array header, optional initialization bitmap and allocated records.
    fn fixed(&self, address: u64) -> Result<Option<Vec<ChunkEntry>>> {
        let mut cursor = self.cursor(address)?;
        let client = Self::prefix(&mut cursor, *b"FAHD")?;
        let size = usize::from(cursor.read_u8()?);
        let page_bits = cursor.read_u8()?;
        let count = usize::try_from(cursor.read_length(self.length_size)?)?;
        let block = cursor.read_offset(self.offset_size)?;
        Self::checksum(&mut cursor, address)?;
        if client != 1 {
            return Ok(None);
        }
        let width = self.size_width(size)?;
        if matches!(width, 2 | 4 | 8) {
            return Ok(None);
        }
        self.array_dimensions(false)?;
        let mut entries = Vec::new();
        if !self.absent(block) {
            let mut cursor = self.cursor(block)?;
            if Self::prefix(&mut cursor, *b"FADB")? != client
                || cursor.read_offset(self.offset_size)? != address
            {
                return Err("HDF fixed-array block ownership mismatch".into());
            }
            self.pages(
                &mut cursor,
                block,
                count,
                page_bits,
                width,
                &mut 0,
                &mut entries,
            )?;
        }
        Ok(Some(entries))
    }

    /// Load the official extensible-array pointer table and its bounded allocated descendants.
    #[allow(
        clippy::too_many_lines,
        reason = "Header checks, fixed pointer-table sizes and direct/secondary traversal share the same creation parameters and allocation budget."
    )]
    fn extensible(&self, address: u64) -> Result<Option<Vec<ChunkEntry>>> {
        let mut cursor = self.cursor(address)?;
        let client = Self::prefix(&mut cursor, *b"EAHD")?;
        let size = usize::from(cursor.read_u8()?);
        let max_bits = cursor.read_u8()?;
        let inline = usize::from(cursor.read_u8()?);
        let min_elements = usize::from(cursor.read_u8()?);
        let min_pointers = usize::from(cursor.read_u8()?);
        let page_bits = cursor.read_u8()?;
        for _ in 0..6 {
            cursor.read_length(self.length_size)?;
        }
        let block = cursor.read_offset(self.offset_size)?;
        Self::checksum(&mut cursor, address)?;
        if client != 1 {
            return Ok(None);
        }
        let width = self.size_width(size)?;
        if matches!(width, 2 | 4 | 8) {
            return Ok(None);
        }
        self.array_dimensions(true)?;
        if !(1..=64).contains(&max_bits)
            || !min_elements.is_power_of_two()
            || min_pointers < 2
            || !min_pointers.is_power_of_two()
            || page_bits > max_bits
            || u32::from(max_bits) < min_elements.trailing_zeros()
        {
            return Err("invalid HDF extensible-array growth parameters".into());
        }
        let first_secondary = 2 * min_pointers.trailing_zeros();
        let levels = 1 + u32::from(max_bits) - min_elements.trailing_zeros();
        let page_elements = 1usize
            .checked_shl(u32::from(page_bits))
            .ok_or("HDF page capacity overflows")?;
        if first_secondary > levels
            || page_elements < inline
            || page_elements < Self::array_level(first_secondary, min_elements)?.0
        {
            return Err("invalid HDF extensible-array page capacity".into());
        }
        let mut entries = Vec::new();
        if self.absent(block) {
            return Ok(Some(entries));
        }
        let mut cursor = self.cursor(block)?;
        if Self::prefix(&mut cursor, *b"EAIB")? != client
            || cursor.read_offset(self.offset_size)? != address
        {
            return Err("HDF extensible-array index ownership mismatch".into());
        }
        self.records(&mut cursor, width, inline, &mut 0, &mut entries)?;
        let direct = (0..2 * (min_pointers - 1))
            .map(|_| cursor.read_offset(self.offset_size))
            .collect::<hdf5_reader::error::Result<Vec<_>>>()?;
        let secondary = (first_secondary..levels)
            .map(|_| cursor.read_offset(self.offset_size))
            .collect::<hdf5_reader::error::Result<Vec<_>>>()?;
        Self::checksum(&mut cursor, block)?;
        let parameters = ArrayBlocks {
            owner: address,
            width,
            max_bits,
            page_elements,
        };
        let mut direct_index = 0;
        for level in 0..first_secondary {
            let (elements, pointers, start) = Self::array_level(level, min_elements)?;
            for position in 0..pointers {
                let block = *direct
                    .get(direct_index)
                    .ok_or("invalid HDF direct array position")?;
                direct_index += 1;
                if !self.absent(block) {
                    let mut linear = Self::array_position(inline, start, position, elements)?;
                    self.array_block(
                        block,
                        elements,
                        &parameters,
                        &mut linear,
                        None,
                        &mut entries,
                    )?;
                }
            }
        }
        for (offset, &block) in secondary.iter().enumerate() {
            if self.absent(block) {
                continue;
            }
            let level = first_secondary + u32::try_from(offset)?;
            let (elements, pointers, start) = Self::array_level(level, min_elements)?;
            if pointers > MAX_SLOTS {
                return Err("HDF extensible-array pointer count exceeds slot limit".into());
            }
            let mut cursor = self.cursor(block)?;
            if Self::prefix(&mut cursor, *b"EASB")? != client
                || cursor.read_offset(self.offset_size)? != address
            {
                return Err("HDF extensible-array secondary ownership mismatch".into());
            }
            cursor.skip(usize::from(max_bits).div_ceil(8))?;
            let pages = if elements > page_elements {
                elements / page_elements
            } else {
                0
            };
            let bitmap_size = pointers
                .checked_mul(pages.div_ceil(8))
                .ok_or("HDF page bitmap size overflows")?;
            let bitmap = cursor.read_bytes(bitmap_size)?;
            let mut addresses = cursor.clone();
            cursor.skip(
                pointers
                    .checked_mul(usize::from(self.offset_size))
                    .ok_or("HDF array pointers overflow")?,
            )?;
            Self::checksum(&mut cursor, block)?;
            for position in 0..pointers {
                let block = addresses.read_offset(self.offset_size)?;
                if !self.absent(block) {
                    let mut linear = Self::array_position(inline, start, position, elements)?;
                    let flags = (pages > 0).then_some((bitmap, position * pages));
                    self.array_block(
                        block,
                        elements,
                        &parameters,
                        &mut linear,
                        flags,
                        &mut entries,
                    )?;
                }
            }
        }
        Ok(Some(entries))
    }

    /// Compute a level's block size/count and first slot from the published EA growth equations.
    fn array_level(level: u32, minimum: usize) -> Result<(usize, usize, u64)> {
        let elements = minimum
            .checked_shl(level.div_ceil(2))
            .ok_or("HDF array block size overflows")?;
        let pointers = 1usize
            .checked_shl(level / 2)
            .ok_or("HDF array pointer count overflows")?;
        let start = ((1u128 << level) - 1) * u128::try_from(minimum)?;
        Ok((elements, pointers, u64::try_from(start)?))
    }

    /// Convert one allocated block's published logical position within the bounded slot space.
    fn array_position(
        inline: usize,
        start: u64,
        position: usize,
        elements: usize,
    ) -> Result<usize> {
        let value = usize::try_from(start)?
            .checked_add(inline)
            .and_then(|value| {
                position
                    .checked_mul(elements)
                    .and_then(|offset| value.checked_add(offset))
            })
            .filter(|value| *value <= MAX_SLOTS)
            .ok_or("HDF filtered chunk index exceeds slot limit")?;
        Ok(value)
    }

    /// Decode an EA data block; paged blocks hold only a prefix followed by fixed physical page slots.
    #[allow(
        clippy::too_many_arguments,
        reason = "The data-block loader needs its owner parameters, logical slot and separate parent bitmap to preserve the HDF layout."
    )]
    fn array_block(
        &self,
        address: u64,
        count: usize,
        parameters: &ArrayBlocks,
        linear: &mut usize,
        bitmap: Option<(&[u8], usize)>,
        entries: &mut Vec<ChunkEntry>,
    ) -> Result<()> {
        if count > MAX_SLOTS.saturating_sub(*linear) {
            return Err("HDF filtered chunk index exceeds slot limit".into());
        }
        let mut cursor = self.cursor(address)?;
        if Self::prefix(&mut cursor, *b"EADB")? != 1
            || cursor.read_offset(self.offset_size)? != parameters.owner
        {
            return Err("HDF extensible-array data ownership mismatch".into());
        }
        cursor.skip(usize::from(parameters.max_bits).div_ceil(8))?;
        if count <= parameters.page_elements {
            self.records(&mut cursor, parameters.width, count, linear, entries)?;
            return Self::checksum(&mut cursor, address);
        }
        let (bitmap, first_bit) = bitmap.ok_or("HDF paged data block lacks parent bitmap")?;
        Self::checksum(&mut cursor, address)?;
        let base = cursor.position();
        let stride = parameters
            .page_elements
            .checked_mul(usize::from(self.offset_size) + parameters.width + 4)
            .and_then(|size| size.checked_add(4))
            .ok_or("HDF array page stride overflows")?;
        for page in 0..count / parameters.page_elements {
            if Self::initialized(bitmap, first_bit + page)? {
                let address = base
                    .checked_add(u64::try_from(
                        page.checked_mul(stride)
                            .ok_or("HDF page offset overflows")?,
                    )?)
                    .ok_or("HDF page address overflows")?;
                let mut cursor = self.cursor(address)?;
                self.records(
                    &mut cursor,
                    parameters.width,
                    parameters.page_elements,
                    linear,
                    entries,
                )?;
                Self::checksum(&mut cursor, address)?;
            } else {
                Self::advance(linear, parameters.page_elements)?;
            }
        }
        Ok(())
    }

    /// Use the public tree-header parser and load affected type-11 records only.
    fn btree(&self, address: u64) -> Result<Option<Vec<ChunkEntry>>> {
        let header = BTreeV2Header::parse(
            &mut self.cursor(address)?,
            self.offset_size,
            self.length_size,
        )?;
        if header.btree_type != 11 {
            return Ok(None);
        }
        let fixed = usize::from(self.offset_size) + 4 + self.dims.len() * 8;
        let width = usize::from(header.record_size)
            .checked_sub(fixed)
            .filter(|width| (1..=8).contains(width))
            .ok_or("invalid HDF tree chunk-size width")?;
        if matches!(width, 2 | 4 | 8) {
            return Ok(None);
        }
        if header.depth > 64 || header.total_records > MAX_SLOTS as u64 || header.node_size < 10 {
            return Err("HDF filtered chunk tree exceeds metadata limits".into());
        }
        let levels = self.tree_levels(&header)?;
        let mut visited = HashSet::new();
        let mut entries = Vec::new();
        let mut pending = vec![(
            header.root_node_address,
            header.depth,
            usize::from(header.num_records_in_root),
        )];
        while let Some((address, depth, count)) = pending.pop() {
            if self.absent(address) {
                continue;
            }
            if !visited.insert(address) || visited.len() > MAX_SLOTS || entries.len() > MAX_SLOTS {
                return Err("HDF filtered chunk tree is cyclic or excessive".into());
            }
            if count > MAX_SLOTS.saturating_sub(entries.len())
                || count > levels.maximum[usize::from(depth)]
                || count
                    .checked_mul(usize::from(header.record_size))
                    .and_then(|size| size.checked_add(10))
                    .is_none_or(|size| size > usize::try_from(header.node_size).unwrap_or(0))
            {
                return Err("HDF chunk tree records exceed node limits".into());
            }
            let mut cursor = self.cursor(address)?;
            let signature = if depth == 0 { b"BTLF" } else { b"BTIN" };
            if Self::prefix(&mut cursor, *signature)? != 11 {
                return Err("HDF chunk tree node type mismatch".into());
            }
            for _ in 0..count {
                let mut entry = self.record(&mut cursor, width)?;
                entry.offsets = self
                    .dims
                    .iter()
                    .map(|&dimension| {
                        Ok(cursor
                            .read_u64_le()?
                            .checked_mul(u64::from(dimension))
                            .ok_or("HDF chunk offset overflows")?)
                    })
                    .collect::<Result<Vec<_>>>()?;
                self.append(&mut entries, entry)?;
            }
            if depth > 0 {
                for _ in 0..=count {
                    let child = cursor.read_offset(self.offset_size)?;
                    let records = usize::try_from(cursor.read_uvar(levels.record_width)?)?;
                    if depth > 1 {
                        cursor.read_uvar(levels.cumulative_width[usize::from(depth - 1)])?;
                    }
                    pending.push((child, depth - 1, records));
                }
            }
            let used = cursor
                .position()
                .checked_sub(address)
                .ok_or("HDF node range overflows")?;
            if used
                .checked_add(4)
                .is_none_or(|size| size > u64::from(header.node_size))
            {
                return Err("HDF chunk tree contents exceed node size".into());
            }
            Self::checksum(&mut cursor, address)?;
        }
        Ok(Some(entries))
    }

    /// Compute each tree depth's node capacity and minimally encoded cumulative record width.
    fn tree_levels(&self, header: &BTreeV2Header) -> Result<TreeLevels> {
        let available = usize::try_from(header.node_size)? - 10;
        let record = usize::from(header.record_size);
        let leaf = available / record;
        let record_width = Self::count_width(u64::try_from(leaf)?);
        let mut maximum = vec![leaf];
        let mut cumulative_width = vec![0];
        let mut cumulative = u64::try_from(leaf)?;
        for depth in 1..=usize::from(header.depth) {
            let pointer =
                usize::from(self.offset_size) + record_width + cumulative_width[depth - 1];
            let count = available.saturating_sub(pointer) / (record + pointer);
            maximum.push(count);
            cumulative = u64::try_from(count + 1)?
                .checked_mul(cumulative)
                .and_then(|total| total.checked_add(u64::try_from(count).ok()?))
                .ok_or("HDF tree cumulative capacity overflows")?;
            cumulative_width.push(Self::count_width(cumulative));
        }
        Ok(TreeLevels {
            record_width,
            maximum,
            cumulative_width,
        })
    }

    /// Return the minimum byte width used by HDF variable-width record counts.
    fn count_width(maximum: u64) -> usize {
        usize::try_from((64 - maximum.leading_zeros()).max(1).div_ceil(8)).unwrap_or(8)
    }
}

/// Node record limits and child-pointer field widths for every bounded tree depth.
struct TreeLevels {
    /// Header-wide field width for immediate child record counts.
    record_width: usize,
    /// Maximum records fitting a node at each depth.
    maximum: Vec<usize>,
    /// Cumulative child-subtree count width at each depth; leaves omit this field.
    cumulative_width: Vec<usize>,
}

/// Extensible-array header fields shared by each child data block.
struct ArrayBlocks {
    /// Owning extensible-array header address.
    owner: u64,
    /// Variable-length filtered size width.
    width: usize,
    /// Width in bits of logical slot offsets.
    max_bits: u8,
    /// Number of records in each physical data-block page.
    page_elements: usize,
}
