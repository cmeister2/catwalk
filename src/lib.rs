#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
pub mod formats;
pub mod render;
pub mod saved_preview;
pub mod scene;
mod thumbnail;

pub use thumbnail::thumbnail;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;
