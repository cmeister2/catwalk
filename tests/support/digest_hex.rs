//! Lowercase digest formatting shared by source-integrity and snapshot controls.

use std::fmt;

/// Display digest bytes as two lowercase hexadecimal digits each, including leading zeros.
pub(super) struct Hex<T>(
    /// Digest output whose complete bytes are written in their existing order.
    pub(super) T,
);

impl<T: AsRef<[u8]>> fmt::LowerHex for Hex<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0.as_ref() {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}
