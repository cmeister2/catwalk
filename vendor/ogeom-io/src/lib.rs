//! Data exchange.
//!
//! *Elsewhere:* the shape serialization, STEP, IGES, glTF and transfer-framework
//! packages.
//!
//! The native format is what the test suite round-trips through. STEP is the
//! exchange format with external value, and reading it is mostly not parsing:
//! the Part 21 syntax is the small part, and the rest is the semantic mapping
//! onto this kernel's topology, unit and assembly-transform handling, and
//! surviving the spec-violating output real exporters write.
//!
//! # Two formats, two things
//!
//! [`native`] carries a whole document (topology, geometry, placements,
//! tolerances, provenance) and reads back as the same model, handles and all.
//! [`stl`] carries a triangle soup and loses everything else, which is the
//! format's nature rather than a shortcoming of the writer.
//!
//! The crate root re-exports STL's [`read`] and [`write()`] for the common case;
//! the native pair are reached as [`native::read`] and
//! [`native::write()`], because "write this shape" means something different for
//! each and a name that did not say which would be the wrong kind of convenience.

pub(crate) mod bodies;
pub mod brep;
pub mod dxf;
pub mod iges;
pub(crate) mod inflate;
pub(crate) mod inversion;
pub mod json;
pub mod mesh_formats;
pub mod native;
pub(crate) mod pcurves;
pub mod step;
pub mod stl;
pub mod threemf;
pub mod vrml;
pub(crate) mod xml;

pub use iges::{IgesImport, IgesReport, read_iges, read_iges_unsewn, write_iges};
pub use mesh_formats::{
    ExportMesh, ImportedMesh, read_glb, read_gltf, write_glb, write_obj, write_ply,
};
pub use step::{StepImport, StepReport, UntrimmedFace, WarningSummary, read_step, write_step};
pub use stl::{Encoding, StlMesh, read, read_with_quantum, write};
pub use threemf::{ObjectType, ThreeMfImport, ThreeMfObject, read_3mf, write_3mf};
pub use vrml::read_vrml;
