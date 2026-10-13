//! Preview dispatch and aspect-preserving placement of raster and saved images.

use crate::{
    Result, formats,
    render::{self, RenderOptions},
    saved_preview,
};
use image::{DynamicImage, Rgba, RgbaImage};
use std::path::Path;

/// Render model or volume geometry, or fit a raster or saved image to a canvas.
///
/// Raster and saved-image canvases require width and height between 1 and 4096.
/// Model previews follow the dimensions and work budgets of [`render::render`].
///
/// # Errors
/// Returns an error for unsupported or malformed inputs, I/O or codec failures,
/// an absent saved preview without a qualified geometry reader, invalid output
/// dimensions, or importer/render limits. Supported native documents fall back
/// to fresh geometry when their saved image is absent or cannot be decoded.
pub fn thumbnail(path: &Path, options: &RenderOptions) -> Result<RgbaImage> {
    let extension = formats::extension(path)?;
    if saved_preview::EXTENSIONS.contains(&extension.as_str()) {
        validate_dimensions(options)?;
        match saved_preview::extract(path) {
            Ok(Some(source)) => return Ok(fit_image(&DynamicImage::ImageRgba8(source), options)),
            Ok(None) => {}
            Err(saved_error)
                if formats::NATIVE_GEOMETRY_EXTENSIONS.contains(&extension.as_str()) =>
            {
                // The geometry reader can support a document header or version
                // outside the saved-image reader's qualification, or a valid
                // model can carry a corrupt saved thumbnail.
                return geometry_thumbnail(path, options).map_err(|geometry_error| {
                    format!("native geometry failed: {geometry_error}; saved preview unavailable: {saved_error}").into()
                });
            }
            Err(error) => return Err(error),
        }
        if formats::NATIVE_GEOMETRY_EXTENSIONS.contains(&extension.as_str()) {
            return geometry_thumbnail(path, options);
        }
        return Err(saved_preview::NoSavedPreview.into());
    }
    if formats::IMAGE_EXTENSIONS.contains(&extension.as_str()) {
        return image_thumbnail(path, options);
    }
    geometry_thumbnail(path, options)
}

/// Generate a fresh preview from stored geometry, without reading a saved image.
///
/// `LightBurn` vectors use a fitted XY view; other geometry uses the fixed model
/// camera. Dimensions and work budgets follow the corresponding render function.
///
/// # Errors
/// Returns an error for unsupported or malformed geometry, resource or codec
/// failures, invalid output settings, and exceeded importer/render budgets.
pub fn geometry_thumbnail(path: &Path, options: &RenderOptions) -> Result<RgbaImage> {
    let scene = formats::load(path)?;
    if matches!(formats::extension(path)?.as_str(), "lbrn" | "lbrn2") {
        render::render_xy(&scene, options)
    } else {
        render::render(&scene, options)
    }
}

/// Require a nonzero canvas no larger than 4096 pixels in either dimension.
fn validate_dimensions(options: &RenderOptions) -> Result<()> {
    if options.width == 0 || options.height == 0 || options.width > 4096 || options.height > 4096 {
        return Err("thumbnail dimensions must be between 1 and 4096".into());
    }
    Ok(())
}

/// Render a scalar TIFF stack or fit one bounded raster frame to the canvas.
fn image_thumbnail(path: &Path, options: &RenderOptions) -> Result<RgbaImage> {
    validate_dimensions(options)?;
    let source = if formats::extension(path)? == "dcm" {
        formats::dicom::load(path)?
    } else {
        let data = formats::read_file(path)?;
        if matches!(formats::extension(path)?.as_str(), "tif" | "tiff")
            && let Some(scene) = formats::tiff_volume::scene(&data, data.capacity())?
        {
            drop(data);
            return render::render(&scene, options);
        }
        // TGA has no dependable magic signature. Select the advertised reader
        // from the extension so every supported image type gets its decoder.
        let format = image::ImageFormat::from_extension(formats::extension(path)?)
            .ok_or("unsupported image extension")?;
        let mut reader =
            image::ImageReader::with_format(std::io::Cursor::new(data.as_slice()), format);
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
