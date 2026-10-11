//! Preview dispatch and aspect-preserving placement of raster and saved images.

use crate::{
    Result, formats,
    render::{self, RenderOptions},
    saved_preview,
};
use image::{DynamicImage, Rgba, RgbaImage};
use std::path::Path;

/// Render a model or fit a raster image or saved document preview to a canvas.
///
/// Raster and saved-image canvases require width and height between 1 and 4096.
/// Model previews follow the dimensions and work budgets of [`render::render`].
///
/// # Errors
/// Returns an error for unsupported or malformed inputs, I/O or codec failures,
/// an absent saved preview, invalid output dimensions, or importer/render limits.
pub fn thumbnail(path: &Path, options: &RenderOptions) -> Result<RgbaImage> {
    let extension = formats::extension(path)?;
    if saved_preview::EXTENSIONS.contains(&extension.as_str()) {
        validate_dimensions(options)?;
        let source = saved_preview::extract(path)?.ok_or(saved_preview::NoSavedPreview)?;
        return Ok(fit_image(&DynamicImage::ImageRgba8(source), options));
    }
    if formats::IMAGE_EXTENSIONS.contains(&extension.as_str()) {
        return image_thumbnail(path, options);
    }
    render::render(&formats::load(path)?, options)
}

/// Require a nonzero canvas no larger than 4096 pixels in either dimension.
fn validate_dimensions(options: &RenderOptions) -> Result<()> {
    if options.width == 0 || options.height == 0 || options.width > 4096 || options.height > 4096 {
        return Err("thumbnail dimensions must be between 1 and 4096".into());
    }
    Ok(())
}

/// Decode one image frame under codec limits, then fit it onto the preview canvas.
fn image_thumbnail(path: &Path, options: &RenderOptions) -> Result<RgbaImage> {
    validate_dimensions(options)?;
    let source = if formats::extension(path)? == "dcm" {
        formats::dicom::load(path)?
    } else {
        let data = formats::read_file(path)?;
        // TGA has no dependable magic signature. Select the advertised reader
        // from the extension so every supported image type gets its decoder.
        let format = image::ImageFormat::from_extension(formats::extension(path)?)
            .ok_or("unsupported image extension")?;
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(data), format);
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(256 * 1024 * 1024);
        reader.limits(limits);
        reader.decode()?
    };
    Ok(fit_image(&source, options))
}

/// Center an aspect-preserving Lanczos resize on the requested RGBA background.
fn fit_image(source: &DynamicImage, options: &RenderOptions) -> RgbaImage {
    let source = source
        .resize(
            options.width,
            options.height,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8();
    let alpha = if options.transparent { 0 } else { 255 };
    let mut canvas = RgbaImage::from_pixel(
        options.width,
        options.height,
        Rgba([
            options.background[0],
            options.background[1],
            options.background[2],
            alpha,
        ]),
    );
    image::imageops::overlay(
        &mut canvas,
        &source,
        i64::from((options.width - source.width()) / 2),
        i64::from((options.height - source.height()) / 2),
    );
    if !options.transparent {
        for pixel in canvas.pixels_mut() {
            pixel[3] = 255;
        }
    }
    canvas
}
