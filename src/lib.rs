#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
/// Model-format discovery and bounded Rust geometry importers.
pub mod formats;
/// Fixed-view CPU rendering and PNG encoding.
pub mod render;
/// Extraction of saved document images without loading model geometry.
pub mod saved_preview;
/// Geometry and appearance values shared by importers and the renderer.
pub mod scene;
/// Dispatch and aspect-preserving fitting for model, raster and saved previews.
mod thumbnail;

pub use thumbnail::thumbnail;

/// An importer, rendering, decoding or I/O failure that can cross thread boundaries.
pub type Error = Box<dyn std::error::Error + Send + Sync>;
/// The common fallible return type used throughout Catwalk's API.
pub type Result<T> = std::result::Result<T, Error>;
