//! Bounded little-endian chunk framing used by native 3ds Max scene streams.

use crate::Result;
use std::time::{Duration, Instant};

/// Maximum chunk nesting admitted before following another container.
const MAX_DEPTH: usize = 64;
/// Maximum total number of chunks across both selected streams.
const MAX_CHUNKS: usize = 1_000_000;

/// Borrowed native chunk with its exact payload and optional nested children.
pub(super) struct Chunk<'a> {
    /// Scoped chunk identifier, or class-directory index for scene objects.
    pub id: u16,
    /// Exact serialized payload excluding framing.
    pub data: &'a [u8],
    /// Child chunks; absent for a data chunk.
    pub children: Option<Vec<Chunk<'a>>>,
}

impl<'a> Chunk<'a> {
    /// Select a unique child by its scoped identifier.
    pub fn child(&self, id: u16) -> Result<Option<&Chunk<'a>>> {
        let mut matches = self.list()?.iter().filter(|chunk| chunk.id == id);
        let first = matches.next();
        if matches.next().is_some() {
            return Err(format!("duplicate 3ds Max chunk {id:04x}").into());
        }
        Ok(first)
    }

    /// Require a child carrying the requested native field.
    pub fn required(&self, id: u16) -> Result<&Chunk<'a>> {
        self.child(id)?
            .ok_or_else(|| format!("missing 3ds Max chunk {id:04x}").into())
    }

    /// Require a container rather than interpreting an opaque payload as children.
    pub fn list(&self) -> Result<&[Chunk<'a>]> {
        self.children
            .as_deref()
            .ok_or_else(|| "3ds Max expected a container chunk".into())
    }
}

/// Deadline and cumulative chunk work shared by selected native streams.
pub(super) struct Budget {
    /// Initial monotonic decode time.
    started: Instant,
    /// Number of framed chunks already admitted.
    chunks: usize,
}

impl Budget {
    /// Start the import deadline before reading compound-file streams.
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            chunks: 0,
        }
    }

    /// Refuse work after the native import deadline.
    pub fn check(&self) -> Result<()> {
        if self.started.elapsed() > Duration::from_secs(30) {
            return Err("3ds Max geometry import exceeded its 30 second limit".into());
        }
        Ok(())
    }

    /// Frame an entire scoped chunk stream with exact end accounting.
    pub fn parse<'a>(&mut self, bytes: &'a [u8], depth: usize) -> Result<Vec<Chunk<'a>>> {
        self.check()?;
        if depth > MAX_DEPTH {
            return Err("3ds Max chunk nesting exceeds its limit".into());
        }
        let mut chunks = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            self.check()?;
            self.chunks += 1;
            if self.chunks > MAX_CHUNKS {
                return Err("3ds Max chunk count exceeds its limit".into());
            }
            let id = u16_at(bytes, offset)?;
            let short_size = u32_at(bytes, offset + 2)?;
            let (header, size, container) = if short_size == 0 {
                let size = u64::from_le_bytes(slice(bytes, offset + 6, 8)?.try_into()?);
                (
                    14,
                    usize::try_from(size & 0x7fff_ffff_ffff_ffff)?,
                    size >> 63 != 0,
                )
            } else {
                (
                    6,
                    usize::try_from(short_size & 0x7fff_ffff)?,
                    short_size >> 31 != 0,
                )
            };
            if size < header {
                return Err("invalid 3ds Max chunk extent".into());
            }
            let data = slice(bytes, offset + header, size - header)?;
            let children = if container {
                Some(self.parse(data, depth + 1)?)
            } else {
                None
            };
            chunks.push(Chunk { id, data, children });
            offset = offset
                .checked_add(size)
                .ok_or("3ds Max chunk offset overflow")?;
        }
        Ok(chunks)
    }
}

/// Return a checked binary extent without integer wraparound.
pub(super) fn slice(data: &[u8], offset: usize, size: usize) -> Result<&[u8]> {
    data.get(offset..offset.checked_add(size).ok_or("3ds Max offset overflow")?)
        .ok_or_else(|| "truncated 3ds Max native data".into())
}

/// Read one unaligned little-endian native word.
pub(super) fn u16_at(data: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(slice(data, offset, 2)?.try_into()?))
}

/// Read one unaligned little-endian native integer.
pub(super) fn u32_at(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(slice(data, offset, 4)?.try_into()?))
}

/// Read a finite native coordinate or controller value in double precision.
pub(super) fn float_at(data: &[u8], offset: usize) -> Result<f64> {
    let value = f64::from(f32::from_le_bytes(slice(data, offset, 4)?.try_into()?));
    if !value.is_finite() {
        return Err("non-finite 3ds Max geometry or transform value".into());
    }
    Ok(value)
}
