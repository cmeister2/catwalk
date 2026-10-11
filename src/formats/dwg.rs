//! Native DWG records decoded in Rust, then passed to the static DXF geometry reader.
//! ACIS solids and drawing entities outside that reader's subset remain explicit errors.
use crate::scene::Scene;
use crate::{Error, Result};
use acadrust::{DwgReader, DxfWriter};
use std::io::{Cursor, Write};
use std::path::Path;

/// In-memory DXF output capped at the shared input-size ceiling.
struct BoundedOutput(Vec<u8>);

impl Write for BoundedOutput {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "The shared 512 MiB limit fits usize on supported 32- and 64-bit targets."
    )]
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > (super::MAX_INPUT_BYTES as usize).saturating_sub(self.0.len()) {
            return Err(std::io::Error::other(
                "DWG expanded drawing exceeds size limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Decode a native DWG drawing strictly and emit supported static DXF geometry.
///
/// The record and expanded-DXF limits apply after the upstream DWG decoder;
/// they do not bound every allocation performed inside that decoder.
///
/// # Errors
/// Returns an error for unreadable or oversized inputs, malformed DWG records,
/// decoder panics, incomplete or recovered decoding, excessive record counts,
/// oversized expanded DXF or entities outside the supported DXF subset.
pub(crate) fn load(path: &Path) -> Result<Scene> {
    let bytes = super::read_file(path)?;
    // Keep the default strict reader; recovery could silently omit visible geometry.
    let outcome =
        std::panic::catch_unwind(|| DwgReader::from_stream(Cursor::new(bytes)).read_with_stats())
            .map_err(|_| -> Error { "DWG reader rejected malformed records".into() })??;
    let stats = &outcome.stats;
    if !stats.has_usable_drawing_data()
        || !stats.stream_completed
        || stats.skipped_source_records != 0
        || stats.recovered_errors != 0
    {
        return Err(format!(
            "DWG geometry decode incomplete: {} skipped records, {} recovered errors, stream complete={}",
            stats.skipped_source_records, stats.recovered_errors, stats.stream_completed
        )
        .into());
    }
    if stats.output_records() > 500_000 {
        return Err("DWG exceeds drawing record limit".into());
    }
    let mut drawing = BoundedOutput(Vec::new());
    DxfWriter::new(&outcome.document).write_to_writer(&mut drawing)?;
    super::dxf::parse(&drawing.0)
}
