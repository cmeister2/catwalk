//! Scientific-container data adapters using unmodified upstream HDF5 and NetCDF APIs.
//!
//! Metadata, ordinary data layouts, filters, numeric types and CF processing
//! remain upstream-owned. Catwalk supplies only affected filtered-index reads,
//! bounded chunk assembly and the corresponding upstream array construction.

#[path = "hdf5_chunks.rs"]
/// Variable-width filtered chunk indexes.
mod chunks;

use crate::Result;
use hdf5_reader::{
    BytesStorage, Datatype, H5Type, Storage, StorageBuffer,
    filters::apply_pipeline_with_limit,
    io::Cursor,
    messages::{HdfMessage, layout::DataLayout},
    object_header::ObjectHeader,
    superblock::Superblock,
};
use ndarray::{ArrayD, IxDyn};
use netcdf_reader::{NcReadable, NcSliceInfo, NcSliceInfoElem};
use std::{ops::Deref, sync::Arc};

/// Maximum decoded array size, matching the shared scientific/Alembic byte ceiling.
const MAX_BYTES: usize = 512 * 1024 * 1024;
/// Maximum number of selected numeric samples.
const MAX_SAMPLES: usize = 16_777_216;

/// Upstream HDF5 metadata together with a shared immutable view of its source bytes.
pub(super) struct Hdf5File {
    /// Unmodified high-level metadata and ordinary data reader.
    inner: hdf5_reader::Hdf5File,
    /// File bytes shared with the upstream random-access storage.
    bytes: StorageBuffer,
    /// Public superblock metadata describing addresses and length widths.
    superblock: Superblock,
}

impl Deref for Hdf5File {
    type Target = hdf5_reader::Hdf5File;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Hdf5File {
    /// Open bounded caller-owned bytes with the scientific reader's small chunk cache.
    ///
    /// # Errors
    /// Returns an error for invalid HDF5 metadata or address ranges.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::from_vec(bytes.to_vec())
    }

    /// Transfer an existing file buffer into upstream immutable storage without copying it.
    ///
    /// # Errors
    /// Returns an error for invalid HDF5 metadata or address ranges.
    pub fn from_vec(bytes: Vec<u8>) -> Result<Self> {
        Self::from_storage(Arc::new(BytesStorage::new(bytes)))
    }

    /// Share storage with another upstream container reader without duplicating file bytes.
    fn from_storage(storage: Arc<BytesStorage>) -> Result<Self> {
        let superblock = Superblock::parse_from_storage(storage.as_ref())?;
        let bytes = storage.read_range(0, usize::try_from(storage.len())?)?;
        let inner = hdf5_reader::Hdf5File::from_storage_with_options(
            storage,
            hdf5_reader::OpenOptions {
                chunk_cache_bytes: 8 * 1024 * 1024,
                chunk_cache_slots: 257,
                ..Default::default()
            },
        )?;
        Ok(Self {
            inner,
            bytes,
            superblock,
        })
    }

    /// Open an upstream dataset and attach the adapter's immutable source context.
    ///
    /// # Errors
    /// Returns upstream lookup and dataset-metadata errors.
    pub fn dataset(&self, path: &str) -> Result<Dataset> {
        Ok(self.adapt_dataset(self.inner.dataset(path)?))
    }

    /// Attach shared source context to a dataset returned through an upstream group.
    pub fn adapt_dataset(&self, inner: hdf5_reader::Dataset) -> Dataset {
        Dataset {
            inner,
            bytes: self.bytes.clone(),
            superblock: self.superblock.clone(),
        }
    }
}

/// Upstream dataset metadata with a fallback for affected filtered-index encodings.
pub(super) struct Dataset {
    /// Resolved dataset datatype, dimensions, attributes and fill value.
    inner: hdf5_reader::Dataset,
    /// Immutable source bytes shared with the upstream file.
    bytes: StorageBuffer,
    /// File-wide address and length widths.
    superblock: Superblock,
}

impl Deref for Dataset {
    type Target = hdf5_reader::Dataset;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Dataset {
    /// Read a full numeric array, delegating every unaffected layout upstream.
    ///
    /// # Errors
    /// Returns upstream errors or bounded filtered-index decoding errors.
    pub fn read_array<T: H5Type>(&self) -> Result<ArrayD<T>> {
        if let Some(decoded) = self.repair(&hdf5_reader::SliceInfo::all(self.ndim()))? {
            self.typed(&decoded)
        } else {
            Ok(self.inner.read_array::<T>()?)
        }
    }

    /// Read stored-endian element bytes, including Alembic array/enum-wrapped POD types.
    ///
    /// # Errors
    /// Returns upstream errors or bounded filtered-index decoding errors.
    pub fn read_raw_bytes(&self) -> Result<Vec<u8>> {
        if let Some(decoded) = self.repair(&hdf5_reader::SliceInfo::all(self.ndim()))? {
            Ok(decoded.bytes)
        } else {
            Ok(self.inner.read_raw_bytes()?)
        }
    }

    /// Decode an affected scalar string through an upstream attribute object and heap resolver.
    ///
    /// # Errors
    /// Returns upstream errors for non-scalar strings, malformed data or unsupported encodings.
    pub fn read_string(&self) -> Result<String> {
        if self.num_elements()? != 1 {
            return Ok(self.inner.read_string()?);
        }
        let Some(decoded) = self.repair(&hdf5_reader::SliceInfo::all(self.ndim()))? else {
            return Ok(self.inner.read_string()?);
        };
        let header = self.header()?;
        let space = header
            .messages
            .iter()
            .find_map(|message| match message {
                HdfMessage::Dataspace(space) => Some(space.clone()),
                _ => None,
            })
            .ok_or("HDF string dataset lacks resolved dimensions")?;
        let message = hdf5_reader::messages::attribute::AttributeMessage {
            name: self.name().to_owned(),
            datatype: self.dtype().clone(),
            dataspace: space,
            raw_data: decoded.bytes,
        };
        Ok(hdf5_reader::Attribute::from_message_with_context(
            message,
            Some(&self.bytes),
            self.superblock.offset_size,
        )
        .read_string()?)
    }

    /// Read selected numeric samples, retaining first-frame and bounded-slice behavior.
    #[cfg(test)]
    fn read_slice<T: H5Type>(&self, selection: &hdf5_reader::SliceInfo) -> Result<ArrayD<T>> {
        if let Some(decoded) = self.repair(selection)? {
            self.typed(&decoded)
        } else {
            Ok(self.inner.read_slice::<T>(selection)?)
        }
    }

    /// Decode the assembled byte buffer with public upstream numeric conversion APIs.
    fn typed<T: H5Type>(&self, decoded: &Decoded) -> Result<ArrayD<T>> {
        let count = decoded.bytes.len() / decoded.element_size;
        let values = if let Some(values) = T::decode_vec(&decoded.bytes, self.dtype(), count) {
            values?
        } else {
            decoded
                .bytes
                .chunks_exact(decoded.element_size)
                .map(|bytes| T::from_bytes(bytes, self.dtype()))
                .collect::<hdf5_reader::error::Result<Vec<_>>>()?
        };
        Ok(ArrayD::from_shape_vec(IxDyn(&decoded.shape), values)?)
    }

    /// Parse and resolve object-header shared messages through public upstream APIs.
    fn header(&self) -> Result<ObjectHeader> {
        let mut header = ObjectHeader::parse_at(
            &self.bytes,
            self.address(),
            self.superblock.offset_size,
            self.superblock.length_size,
        )?;
        header.resolve_shared_messages(
            &self.bytes,
            self.superblock.offset_size,
            self.superblock.length_size,
        )?;
        Ok(header)
    }

    /// Resolve only the upstream-invalid size widths and decode intersecting chunks.
    #[allow(
        clippy::too_many_lines,
        reason = "The bounded fallback keeps metadata validation, allocation, upstream filter decoding and selected chunk assembly in one auditable data path."
    )]
    fn repair(&self, selection: &hdf5_reader::SliceInfo) -> Result<Option<Decoded>> {
        let Some(dims) = self.chunks() else {
            return Ok(None);
        };
        let header = self.header()?;
        let Some((address, kind)) = header.messages.iter().find_map(|message| {
            if let HdfMessage::DataLayout(layout) = message
                && let DataLayout::Chunked {
                    address,
                    chunk_indexing: Some(kind),
                    ..
                } = &layout.layout
            {
                return Some((*address, kind));
            }
            None
        }) else {
            return Ok(None);
        };
        if Cursor::is_undefined_offset(address, self.superblock.offset_size) {
            return Ok(None);
        }
        if dims.len() != self.ndim() || dims.is_empty() || dims.contains(&0) {
            return Err("HDF filtered chunk dimensions are invalid".into());
        }
        let index = chunks::Index {
            data: &self.bytes,
            offset_size: self.superblock.offset_size,
            length_size: self.superblock.length_size,
            shape: self.shape(),
            max_shape: self.max_dims(),
            dims: &dims,
        };
        let Some(entries) = index.entries(address, kind)? else {
            return Ok(None);
        };
        let element_size = self.inner.raw_element_size()?;
        if element_size == 0 {
            return Err("HDF numeric element size is zero".into());
        }
        let selected = Selection::new(self.shape(), selection)?;
        let output_size = selected
            .elements
            .checked_mul(element_size)
            .filter(|size| *size <= MAX_BYTES)
            .ok_or("HDF selected array exceeds 512 MiB")?;
        let chunk_elements = dims
            .iter()
            .try_fold(1usize, |total, &dim| {
                total.checked_mul(usize::try_from(dim).ok()?)
            })
            .ok_or("HDF chunk element count overflows")?;
        let chunk_bytes = chunk_elements
            .checked_mul(element_size)
            .filter(|size| *size <= MAX_BYTES)
            .ok_or("HDF chunk exceeds 512 MiB decoded limit")?;
        let filters = header
            .messages
            .iter()
            .find_map(|message| match message {
                HdfMessage::FilterPipeline(pipeline) => Some(pipeline.filters.as_slice()),
                _ => None,
            })
            .ok_or("HDF filtered index has no resolved filter pipeline")?;
        if filters.len() > 32 {
            return Err("HDF filter pipeline exceeds mask width".into());
        }
        let mut bytes = vec![0; output_size];
        if let Some(fill) = self.fill_value().and_then(|value| value.value.as_deref())
            && !fill.is_empty()
        {
            for destination in bytes.chunks_exact_mut(fill.len()) {
                destination.copy_from_slice(fill);
            }
        }
        for entry in entries {
            let Some(ranges) = selected.intersection(&entry.offsets, &dims, self.shape())? else {
                continue;
            };
            let size = if entry.size == 0 {
                chunk_bytes
            } else {
                usize::try_from(entry.size)?
            };
            let start = usize::try_from(entry.address)?;
            let end = start
                .checked_add(size)
                .ok_or("HDF encoded chunk range overflows")?;
            let encoded = self
                .bytes
                .get(start..end)
                .ok_or("HDF encoded chunk exceeds input")?;
            let decoded = apply_pipeline_with_limit(
                encoded,
                filters,
                entry.filter_mask,
                element_size,
                None,
                Some(chunk_bytes + 1),
            )?;
            if decoded.len() != chunk_bytes {
                return Err("HDF decoded chunk length mismatch".into());
            }
            selected.copy(
                &mut bytes,
                &decoded,
                element_size,
                &entry.offsets,
                &dims,
                &ranges,
            )?;
        }
        Ok(Some(Decoded {
            bytes,
            shape: selected.shape,
            element_size,
        }))
    }
}

/// Selected C-order bytes and the reduced result dimensions needed for upstream array construction.
struct Decoded {
    /// Assembled stored-endian element bytes.
    bytes: Vec<u8>,
    /// Result dimensions after index selections collapse axes.
    shape: Vec<usize>,
    /// Fixed numeric storage width.
    element_size: usize,
}

/// Validated per-axis start, step and selected sample count.
struct Axis {
    /// First selected dataset coordinate.
    start: u64,
    /// Distance between selected coordinates.
    step: u64,
    /// Number of selected samples along this axis, including collapsed axes.
    count: usize,
}

/// Bounded selection with one record per source axis and its reduced array shape.
struct Selection {
    /// Dataset axes in source C order.
    axes: Vec<Axis>,
    /// Output dimensions after individual index selections are removed.
    shape: Vec<usize>,
    /// Product of selected axis counts.
    elements: usize,
}

impl Selection {
    /// Resolve and bound every index/range before allocating data.
    fn new(shape: &[u64], selection: &hdf5_reader::SliceInfo) -> Result<Self> {
        if shape.len() != selection.selections.len() || shape.len() > 64 {
            return Err("HDF selection dimensionality is invalid".into());
        }
        let mut axes = Vec::new();
        let mut output = Vec::new();
        let mut elements = 1usize;
        for (&extent, item) in shape.iter().zip(&selection.selections) {
            let (start, step, count) = match *item {
                hdf5_reader::SliceInfoElem::Index(index) => {
                    if index >= extent {
                        return Err("HDF selection index exceeds dimension".into());
                    }
                    (index, 1, 1)
                }
                hdf5_reader::SliceInfoElem::Slice { start, end, step } => {
                    if step == 0 || start > extent {
                        return Err("invalid HDF selection range".into());
                    }
                    let count =
                        usize::try_from(end.min(extent).saturating_sub(start).div_ceil(step))?;
                    output.push(count);
                    (start, step, count)
                }
            };
            elements = elements
                .checked_mul(count)
                .filter(|count| *count <= MAX_SAMPLES)
                .ok_or("HDF selected array exceeds sample limit")?;
            axes.push(Axis { start, step, count });
        }
        Ok(Self {
            axes,
            shape: output,
            elements,
        })
    }

    /// Find selected sample-index intervals overlapping one allocated chunk.
    fn intersection(
        &self,
        offsets: &[u64],
        dims: &[u32],
        shape: &[u64],
    ) -> Result<Option<Vec<(usize, usize)>>> {
        if offsets.len() != self.axes.len() {
            return Err("HDF chunk offset rank is invalid".into());
        }
        let mut ranges = Vec::new();
        for (((axis, &offset), &dimension), &extent) in
            self.axes.iter().zip(offsets).zip(dims).zip(shape)
        {
            if offset >= extent || offset % u64::from(dimension) != 0 {
                return Err("HDF allocated chunk offset exceeds dataset".into());
            }
            let end = offset
                .checked_add(u64::from(dimension))
                .ok_or("HDF chunk extent overflows")?
                .min(extent);
            let first = usize::try_from(offset.saturating_sub(axis.start).div_ceil(axis.step))?
                .min(axis.count);
            let last = usize::try_from(end.saturating_sub(axis.start).div_ceil(axis.step))?
                .min(axis.count);
            if first >= last {
                return Ok(None);
            }
            ranges.push((first, last));
        }
        Ok(Some(ranges))
    }

    /// Copy intersecting selected elements while clipping padded chunk-edge samples.
    fn copy(
        &self,
        destination: &mut [u8],
        source: &[u8],
        element_size: usize,
        offsets: &[u64],
        dims: &[u32],
        ranges: &[(usize, usize)],
    ) -> Result<()> {
        let count = ranges
            .iter()
            .try_fold(1usize, |n, &(start, end)| n.checked_mul(end - start))
            .ok_or("HDF chunk selection overflows")?;
        for linear in 0..count {
            let mut remainder = linear;
            let mut source_index = 0usize;
            let mut destination_index = 0usize;
            let mut source_stride = 1usize;
            let mut destination_stride = 1usize;
            for axis in (0..ranges.len()).rev() {
                let (start, end) = ranges[axis];
                let index = start + remainder % (end - start);
                remainder /= end - start;
                let global = self.axes[axis].start + u64::try_from(index)? * self.axes[axis].step;
                source_index += usize::try_from(global - offsets[axis])? * source_stride;
                destination_index += index * destination_stride;
                source_stride *= usize::try_from(dims[axis])?;
                destination_stride *= self.axes[axis].count;
            }
            let input = source
                .get(source_index * element_size..(source_index + 1) * element_size)
                .ok_or("HDF selected chunk element is out of range")?;
            let output = destination
                .get_mut(destination_index * element_size..(destination_index + 1) * element_size)
                .ok_or("HDF selected array element is out of range")?;
            output.copy_from_slice(input);
        }
        Ok(())
    }
}

/// Upstream `NetCDF` metadata plus an optional shared HDF5 data adapter.
pub(super) struct NcFile {
    /// Classic or `NetCDF4` metadata and unaffected reads.
    inner: netcdf_reader::NcFile,
    /// Numeric fallback for HDF5-backed variables only.
    hdf: Option<Hdf5File>,
}

impl Deref for NcFile {
    type Target = netcdf_reader::NcFile;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl NcFile {
    /// Open classic or `NetCDF4` bytes with a shared bounded metadata/cache configuration.
    ///
    /// # Errors
    /// Returns upstream container and metadata errors.
    pub fn from_bytes_with_options(
        bytes: &[u8],
        options: netcdf_reader::NcOpenOptions,
    ) -> Result<Self> {
        let storage = Arc::new(BytesStorage::new(bytes.to_vec()));
        let inner = netcdf_reader::NcFile::from_storage_with_options(storage.clone(), options)?;
        let hdf = if bytes.starts_with(&hdf5_reader::superblock::HDF5_MAGIC) {
            Some(Hdf5File::from_storage(storage)?)
        } else {
            None
        };
        Ok(Self { inner, hdf })
    }

    /// Read typed data without converting integer connectivity through floating point.
    ///
    /// # Errors
    /// Returns upstream or bounded HDF5 numeric data errors.
    pub fn read_variable<T: NcReadable>(&self, name: &str) -> Result<ArrayD<T>> {
        self.variable(name)?;
        if let Some(file) = &self.hdf {
            let dataset = file.dataset(name)?;
            if let Some(decoded) = dataset.repair(&hdf5_reader::SliceInfo::all(dataset.ndim()))? {
                return dataset.typed(&decoded);
            }
        }
        Ok(self.inner.read_variable::<T>(name)?)
    }

    /// Promote stored numeric types using upstream typed arrays.
    ///
    /// # Errors
    /// Returns an error for unsupported numeric datatypes or failed data reads.
    pub fn read_variable_as_f64(&self, name: &str) -> Result<ArrayD<f64>> {
        self.variable(name)?;
        if let Some(file) = &self.hdf
            && let Some(values) = numeric(&file.dataset(name)?, None)?
        {
            return Ok(values);
        }
        Ok(self.inner.read_variable_as_f64(name)?)
    }

    /// Apply upstream CF unpacking after an owned affected-index read.
    ///
    /// # Errors
    /// Returns variable lookup, datatype and bounded data-reading errors.
    pub fn read_variable_unpacked(&self, name: &str) -> Result<ArrayD<f64>> {
        let mut values = self.read_variable_as_f64(name)?;
        if let Some(parameters) =
            netcdf_reader::unpack::UnpackParams::from_variable(self.variable(name)?)
        {
            parameters.apply(&mut values);
        }
        Ok(values)
    }

    /// Read a slice and apply upstream masking before upstream scale/offset unpacking.
    ///
    /// # Errors
    /// Returns invalid selection, metadata, datatype and bounded data-reading errors.
    pub fn read_variable_slice_unpacked_masked(
        &self,
        name: &str,
        selection: &NcSliceInfo,
    ) -> Result<ArrayD<f64>> {
        let variable = self.variable(name)?;
        if let Some(file) = &self.hdf {
            let hdf_selection = hdf5_reader::SliceInfo {
                selections: selection
                    .selections
                    .iter()
                    .map(|selection| match *selection {
                        NcSliceInfoElem::Index(index) => hdf5_reader::SliceInfoElem::Index(index),
                        NcSliceInfoElem::Slice { start, end, step } => {
                            hdf5_reader::SliceInfoElem::Slice { start, end, step }
                        }
                    })
                    .collect(),
            };
            if let Some(mut values) = numeric(&file.dataset(name)?, Some(&hdf_selection))? {
                if let Some(parameters) = netcdf_reader::masked::MaskParams::from_variable(variable)
                {
                    parameters.apply(&mut values);
                }
                if let Some(parameters) =
                    netcdf_reader::unpack::UnpackParams::from_variable(variable)
                {
                    parameters.apply(&mut values);
                }
                return Ok(values);
            }
        }
        Ok(self
            .inner
            .read_variable_slice_unpacked_masked(name, selection)?)
    }
}

/// Read supported numeric storage with exact upstream decoding followed by f64 promotion.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    reason = "All numeric widths share one dispatch; 64-bit geometry scalars intentionally approximate to f64."
)]
fn numeric(
    dataset: &Dataset,
    selection: Option<&hdf5_reader::SliceInfo>,
) -> Result<Option<ArrayD<f64>>> {
    let full = hdf5_reader::SliceInfo::all(dataset.ndim());
    let Some(decoded) = dataset.repair(selection.unwrap_or(&full))? else {
        return Ok(None);
    };
    macro_rules! read {
        ($type:ty) => {{
            let values = dataset.typed::<$type>(&decoded)?;
            Ok(Some(values.mapv(|value| value as f64)))
        }};
    }
    match dataset.dtype() {
        Datatype::FloatingPoint { size: 4, .. } => read!(f32),
        Datatype::FloatingPoint { size: 8, .. } => read!(f64),
        Datatype::FixedPoint {
            size: 1,
            signed: true,
            ..
        } => read!(i8),
        Datatype::FixedPoint {
            size: 1,
            signed: false,
            ..
        } => read!(u8),
        Datatype::FixedPoint {
            size: 2,
            signed: true,
            ..
        } => read!(i16),
        Datatype::FixedPoint {
            size: 2,
            signed: false,
            ..
        } => read!(u16),
        Datatype::FixedPoint {
            size: 4,
            signed: true,
            ..
        } => read!(i32),
        Datatype::FixedPoint {
            size: 4,
            signed: false,
            ..
        } => read!(u32),
        Datatype::FixedPoint {
            size: 8,
            signed: true,
            ..
        } => read!(i64),
        Datatype::FixedPoint {
            size: 8,
            signed: false,
            ..
        } => read!(u64),
        _ => Err("HDF scientific array has an unsupported numeric datatype".into()),
    }
}

#[cfg(test)]
#[path = "hdf5_owned_tests.rs"]
/// Regressions exercising unmodified upstream errors and the owned affected-layout path.
mod tests;

#[cfg(test)]
#[path = "hdf5_index_tests.rs"]
/// Independently authored HDF Group hierarchy and physical-page metadata regressions.
mod index_tests;
