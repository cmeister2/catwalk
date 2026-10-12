//! Borrowed TIFF input with independently declared chunk boundaries during decoding.

use crate::Result;
use std::{
    cell::RefCell,
    io::{self, Read, Seek, SeekFrom},
    ops::Range,
    rc::Rc,
};

/// Maximum chunk count represented by one bounded one-MiB SHORT metadata array.
const MAX_CHUNKS: usize = 524_288;

/// One active page's sorted original encoded spans and selected codec cursor span.
struct Page {
    /// Unique source-addressed chunk ranges sorted by their starting offsets.
    chunks: Vec<Range<usize>>,
    /// Range chosen by the codec's most recent seek to a declared chunk start.
    current: Option<usize>,
}

/// Mutable decoding mode shared by the borrowed reader and its caller's handle.
struct State {
    /// Active page, or unrestricted metadata mode when absent.
    page: Option<Page>,
}

/// Reader over unchanged TIFF bytes, restricted only during selected pixel decoding.
pub(super) struct Source<'a> {
    /// Complete immutable source used by metadata and pixel reads alike.
    bytes: &'a [u8],
    /// Cursor position; unrestricted seeks may pass the end of the source.
    position: u64,
    /// Shared page guard without copying the input or encoded payloads.
    state: Rc<RefCell<State>>,
}

/// Caller-owned handle controlling the codec's current page decoding boundaries.
pub(super) struct Guard {
    /// Exact immutable source length used to validate every activation.
    source_len: usize,
    /// Shared state observed by every source read and seek.
    state: Rc<RefCell<State>>,
}

/// Construct one borrowed reader and its initially unrestricted metadata handle.
pub(super) fn source(bytes: &[u8]) -> (Source<'_>, Guard) {
    let state = Rc::new(RefCell::new(State { page: None }));
    (
        Source {
            bytes,
            position: 0,
            state: Rc::clone(&state),
        },
        Guard {
            source_len: bytes.len(),
            state,
        },
    )
}

impl Guard {
    /// Activate source-bounded original encoded spans for one selected page.
    ///
    /// Exact duplicate spans are retained once. Overlapping spans with distinct
    /// starts remain addressable; differing ends at one start are ambiguous.
    /// Reads require a subsequent codec seek to one declared chunk start.
    ///
    /// # Errors
    /// Returns an error for empty/excessive chunk arrays, nonpositive or
    /// out-of-source spans, or conflicting spans with an identical start.
    pub(super) fn activate(&self, mut chunks: Vec<Range<usize>>) -> Result<()> {
        if chunks.is_empty() || chunks.len() > MAX_CHUNKS {
            return Err("TIFF pixel guard has an invalid chunk count".into());
        }
        if chunks
            .iter()
            .any(|chunk| chunk.start >= chunk.end || chunk.end > self.source_len)
        {
            return Err("TIFF pixel guard has an invalid source span".into());
        }
        chunks.sort_unstable_by_key(|chunk| (chunk.start, chunk.end));
        if chunks
            .windows(2)
            .any(|pair| pair[0].start == pair[1].start && pair[0].end != pair[1].end)
        {
            return Err("TIFF pixel chunks sharing a start have ambiguous byte counts".into());
        }
        chunks.dedup();
        self.state.borrow_mut().page = Some(Page {
            chunks,
            current: None,
        });
        Ok(())
    }

    /// Release the current page spans and restore unrestricted metadata access.
    pub(super) fn clear(&self) {
        self.state.borrow_mut().page = None;
    }
}

/// Produce one ordinary I/O error for a forbidden cursor operation.
fn forbidden(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

impl Read for Source<'_> {
    /// Read at most the remainder of the current declared chunk during decoding.
    ///
    /// # Errors
    /// Returns an error when guarded decoding reads before selecting a chunk,
    /// or when its cursor lies outside the selected chunk.
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let state = self.state.borrow();
        let limit = if let Some(page) = &state.page {
            let index = page
                .current
                .ok_or_else(|| forbidden("TIFF pixel read has no selected declared chunk"))?;
            let chunk = &page.chunks[index];
            if self.position < u64::try_from(chunk.start).map_err(io::Error::other)?
                || self.position > u64::try_from(chunk.end).map_err(io::Error::other)?
            {
                return Err(forbidden("TIFF pixel cursor is outside its declared chunk"));
            }
            chunk.end
        } else {
            self.bytes.len()
        };
        let start = usize::try_from(self.position).map_err(io::Error::other)?;
        if start >= limit {
            return Ok(0);
        }
        let count = buffer.len().min(limit - start);
        buffer[..count].copy_from_slice(&self.bytes[start..start + count]);
        self.position = u64::try_from(start + count).map_err(io::Error::other)?;
        Ok(count)
    }
}

impl Seek for Source<'_> {
    /// Seek freely for metadata or within the active page's declared chunk spans.
    ///
    /// A seek to a declared chunk start selects that chunk's exact end. A current
    /// position query preserves the existing range even if spans overlap.
    ///
    /// # Errors
    /// Returns an error for checked cursor arithmetic failures or active decoding
    /// seeks outside the current span and every declared new chunk start.
    fn seek(&mut self, seek: SeekFrom) -> io::Result<u64> {
        let next = match seek {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
            SeekFrom::End(offset) => u64::try_from(self.bytes.len())
                .map_err(io::Error::other)?
                .checked_add_signed(offset),
        }
        .ok_or_else(|| forbidden("TIFF seek position overflow or negative offset"))?;
        let mut state = self.state.borrow_mut();
        if let Some(page) = &mut state.page {
            let position = usize::try_from(next).map_err(io::Error::other)?;
            let query = matches!(seek, SeekFrom::Current(0));
            let declared = if query {
                None
            } else {
                page.chunks
                    .binary_search_by_key(&position, |chunk| chunk.start)
                    .ok()
            };
            if let Some(index) = declared {
                page.current = Some(index);
            } else if page.current.is_none_or(|index| {
                let chunk = &page.chunks[index];
                position < chunk.start || position > chunk.end
            }) {
                return Err(forbidden("TIFF pixel seek is outside its declared chunk"));
            }
        }
        self.position = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_is_unrestricted_before_activation_and_after_clear() {
        let (mut reader, guard) = source(b"0123456789");
        reader.seek(SeekFrom::Start(4)).unwrap();
        let mut bytes = [0; 6];
        reader.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"456789");
        reader.seek(SeekFrom::Start(20)).unwrap();
        assert_eq!(reader.read(&mut bytes).unwrap(), 0);
        guard.activate(std::iter::once(2..4).collect()).unwrap();
        reader.seek(SeekFrom::Start(2)).unwrap();
        guard.clear();
        reader.seek(SeekFrom::Start(8)).unwrap();
        let mut ending = Vec::new();
        reader.read_to_end(&mut ending).unwrap();
        assert_eq!(ending, b"89");
    }

    #[test]
    fn guarded_reads_stop_at_the_exact_declared_end_and_never_use_adjacent_bytes() {
        let (mut reader, guard) = source(b"0123456789");
        guard.activate(std::iter::once(2..4).collect()).unwrap();
        assert!(reader.read(&mut [0; 1]).is_err());
        reader.seek(SeekFrom::Start(2)).unwrap();
        let mut bytes = [0; 8];
        assert_eq!(reader.read(&mut bytes).unwrap(), 2);
        assert_eq!(&bytes[..2], b"23");
        assert_eq!(reader.read(&mut bytes).unwrap(), 0);
        assert_eq!(reader.read(&mut bytes).unwrap(), 0);
        reader.seek(SeekFrom::Start(2)).unwrap();
        assert_eq!(
            reader.read_exact(&mut [0; 3]).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn exact_chunk_starts_transition_but_other_outside_seeks_are_rejected() {
        let (mut reader, guard) = source(b"0123456789");
        guard.activate(vec![6..9, 1..3, 1..3]).unwrap();
        assert!(reader.seek(SeekFrom::Start(2)).is_err());
        reader.seek(SeekFrom::Start(1)).unwrap();
        assert_eq!(reader.seek(SeekFrom::Current(1)).unwrap(), 2);
        assert!(reader.seek(SeekFrom::Current(2)).is_err());
        assert_eq!(reader.stream_position().unwrap(), 2);
        reader.seek(SeekFrom::Start(6)).unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"678");
        assert!(reader.seek(SeekFrom::End(0)).is_err());
        reader.seek(SeekFrom::Start(1)).unwrap();
        assert!(reader.seek(SeekFrom::Current(i64::MIN)).is_err());
    }

    #[test]
    fn overlapping_distinct_starts_preserve_the_selected_chunk_and_position_queries() {
        let (mut reader, guard) = source(b"0123456789");
        guard.activate(vec![1..4, 3..8]).unwrap();
        reader.seek(SeekFrom::Start(1)).unwrap();
        reader.read_exact(&mut [0; 2]).unwrap();
        assert_eq!(reader.stream_position().unwrap(), 3);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"3");
        reader.seek(SeekFrom::Start(3)).unwrap();
        bytes.clear();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"34567");
    }

    #[test]
    fn invalid_and_ambiguous_ranges_fail_before_activating() {
        let (mut reader, guard) = source(b"0123456789");
        for chunks in [
            vec![],
            std::iter::once(2..2).collect(),
            std::iter::once(2..11).collect(),
            vec![1..3, 1..4],
        ] {
            assert!(guard.activate(chunks).is_err());
        }
        reader.seek(SeekFrom::Start(8)).unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"89");
        assert!(guard.activate(vec![0..1; MAX_CHUNKS + 1]).is_err());
    }
}
