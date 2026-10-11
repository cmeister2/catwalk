use crate::{
    Result, formats,
    render::{self, RenderOptions},
    saved_preview,
};
use image::{DynamicImage, Rgba, RgbaImage};
use std::path::Path;

/// Render a model or fit a raster image or saved document preview to a canvas.
pub fn thumbnail(path: &Path, options: &RenderOptions) -> Result<RgbaImage> {
    let extension = formats::extension(path)?;
    if saved_preview::EXTENSIONS.contains(&extension.as_str()) {
        validate_dimensions(options)?;
        let source = saved_preview::extract(path)?.ok_or(saved_preview::NoSavedPreview)?;
        return fit_image(DynamicImage::ImageRgba8(source), options);
    }
    if formats::IMAGE_EXTENSIONS.contains(&extension.as_str()) {
        return image_thumbnail(path, options);
    }
    render::render(&formats::load(path)?, options)
}

fn validate_dimensions(options: &RenderOptions) -> Result<()> {
    if options.width == 0 || options.height == 0 || options.width > 4096 || options.height > 4096 {
        return Err("thumbnail dimensions must be between 1 and 4096".into());
    }
    Ok(())
}

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
    fit_image(source, options)
}

fn fit_image(source: DynamicImage, options: &RenderOptions) -> Result<RgbaImage> {
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
        ((options.width - source.width()) / 2) as i64,
        ((options.height - source.height()) / 2) as i64,
    );
    if !options.transparent {
        for pixel in canvas.pixels_mut() {
            pixel[3] = 255;
        }
    }
    Ok(canvas)
}
