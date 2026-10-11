//! Saved images extracted from document containers without opening model geometry.
//!
//! These readers support the carriers and versions recorded in the research
//! evidence. A document can be valid while containing no saved preview.
pub mod autodesk;
pub mod blend_dwg;
pub mod inventor;
pub mod lightburn;

use crate::{Result, formats};
use image::RgbaImage;
use std::path::Path;

pub const EXTENSIONS: &[&str] = &["ipt", "iam", "lbrn", "lbrn2", "blend", "f3d", "dwg"];

/// A supported document has no usable saved thumbnail carrier.
#[derive(Debug)]
pub struct NoSavedPreview;

impl std::fmt::Display for NoSavedPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("document contains no supported saved preview")
    }
}

impl std::error::Error for NoSavedPreview {}

/// Return the saved image at its original size, or `None` when absent.
/// Malformed data and unsupported versions return an error.
pub fn extract(path: &Path) -> Result<Option<RgbaImage>> {
    match formats::extension(path)?.as_str() {
        "ipt" | "iam" => inventor::extract(path),
        "lbrn" | "lbrn2" => lightburn::extract(path),
        "blend" => blend_dwg::extract_blend(path),
        "f3d" => autodesk::extract_fusion(path),
        "dwg" => blend_dwg::extract_dwg(path),
        extension => Err(format!("no saved-preview reader for .{extension}").into()),
    }
}
