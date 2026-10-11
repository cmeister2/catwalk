//! STEP exchange files: ISO 10303.
//!
//! *Elsewhere* this is `STEPControl` and everything under it. Two layers,
//! deliberately separate: [`parse`] understands Part 21, the exchange
//! *syntax*, and produces an instance graph with no opinions; [`read`]
//! understands the B-rep subset of the schema and builds real topology,
//! counting what it walked past and warning about what it compromised.

pub mod parse;
pub mod read;
pub mod write;

pub use read::{StepImport, StepReport, UntrimmedFace, WarningSummary, read_step};
pub use write::write_step;
