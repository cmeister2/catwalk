//! Bounded glTF base-color textures, sampler state and texture coordinates.
use super::{LoadContext, decoded_attribute};
use crate::{
    Result,
    formats::{read_file, relative_resource, transform::Transform},
    scene::{AlphaMode, Texture, TextureFilter, TextureWrap, TriangleAppearance},
};
use ::gltf::accessor::{DataType, Dimensions, Iter};
use base64::Engine;
use image::{ImageFormat, RgbaImage};
use serde_json::Value;
use std::{borrow::Cow, collections::BTreeMap, io::Cursor, path::Path, sync::Arc};

/// Materials indexed by document ID; `None` identifies the glTF default material.
pub(super) type Materials = BTreeMap<Option<usize>, Material>;

#[derive(Clone)]
/// Thumbnail appearance and texture coordinate selection for a glTF material.
pub(super) struct Material {
    /// Base color, texture, alpha behavior and shading flags for rendered triangles.
    pub appearance: TriangleAppearance,
    /// Texture coordinate set requested by the base-color texture, if present.
    pub texcoord_set: Option<u32>,
    /// Texture transform applied after any selected coordinate morphs.
    uv: UvTransform,
}

#[derive(Clone, Copy)]
/// Two-dimensional scale, rotation and offset from the texture transform extension.
struct UvTransform {
    /// Translation applied after scaling and rotation.
    offset: [f64; 2],
    /// Independent texture coordinate scale factors.
    scale: [f64; 2],
    /// Counterclockwise rotation in radians.
    rotation: f64,
}
impl Default for UvTransform {
    fn default() -> Self {
        Self {
            offset: [0.; 2],
            scale: [1.; 2],
            rotation: 0.,
        }
    }
}

impl Material {
    /// Apply material texture scale, rotation and offset to coordinate pairs.
    ///
    /// # Errors
    /// Returns an error if transformed texture coordinates are non-finite.
    pub fn transform_uvs(&self, values: &mut [[f64; 2]]) -> Result<()> {
        let (sin, cos) = self.uv.rotation.sin_cos();
        for uv in values {
            let u = uv[0] * self.uv.scale[0];
            let v = uv[1] * self.uv.scale[1];
            *uv = [
                self.uv.offset[0] + cos * u - sin * v,
                self.uv.offset[1] + sin * u + cos * v,
            ];
            if uv.iter().any(|v| !v.is_finite()) {
                return Err(
                    "glTF transformed texture coordinates exceed the finite coordinate limit"
                        .into(),
                );
            }
        }
        Ok(())
    }
}

/// Read a finite two-number texture transform member or its omitted default.
fn pair(value: Option<&Value>, default: [f64; 2]) -> Result<[f64; 2]> {
    let Some(value) = value else {
        return Ok(default);
    };
    let values = value
        .as_array()
        .filter(|v| v.len() == 2)
        .ok_or("glTF texture transform requires two-number arrays")?;
    let mut result = [0.; 2];
    for (out, value) in result.iter_mut().zip(values) {
        *out = value
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or("glTF texture transform components must be finite numbers")?;
    }
    Ok(result)
}

/// Translate glTF sampler wrapping to the renderer's wrapping modes.
fn wrapping(value: ::gltf::texture::WrappingMode) -> TextureWrap {
    match value {
        ::gltf::texture::WrappingMode::ClampToEdge => TextureWrap::ClampToEdge,
        ::gltf::texture::WrappingMode::MirroredRepeat => TextureWrap::MirroredRepeat,
        ::gltf::texture::WrappingMode::Repeat => TextureWrap::Repeat,
    }
}

/// Load only materials used by the selected scene. Unused scene images do not
/// consume the texture budget or require unrelated optional texture decoders.
///
/// # Errors
/// Returns an error for invalid material values, required texture transforms or
/// image references, unsupported image encodings, unsafe local resource paths,
/// malformed image data, and exceeded reference, byte or pixel budgets.
#[allow(
    clippy::too_many_lines,
    reason = "The bounded material pass validates appearance, sampler and cached image references together."
)]
pub(super) fn load(
    model: &::gltf::Gltf,
    raw: &Value,
    parent: &Path,
    buffers: &[Vec<u8>],
    globals: &[Option<Transform>],
) -> Result<Materials> {
    if model.images().len() > 256
        || model.textures().len() > 1024
        || model.materials().len() > 100_000
    {
        return Err("glTF exceeds the image, texture or material reference limit".into());
    }
    let mut images = BTreeMap::new();
    let mut total_bytes = buffers.iter().map(|b| b.len() as u64).sum::<u64>();
    let mut pixels = 0u64;
    let mut materials = BTreeMap::new();
    for node in model.nodes().filter(|node| globals[node.index()].is_some()) {
        for primitive in node.mesh().into_iter().flat_map(|mesh| mesh.primitives()) {
            let source = primitive.material();
            let index = source.index();
            if materials.contains_key(&index) {
                continue;
            }
            let pbr = source.pbr_metallic_roughness();
            let base_color = pbr.base_color_factor().map(f64::from);
            if base_color
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            {
                return Err(
                    "glTF base color factor must contain finite values between zero and one".into(),
                );
            }
            let cutoff = f64::from(source.alpha_cutoff().unwrap_or(0.5));
            if !cutoff.is_finite() || cutoff < 0. {
                return Err("glTF alpha cutoff must be finite and nonnegative".into());
            }
            let alpha_mode = match source.alpha_mode() {
                ::gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
                ::gltf::material::AlphaMode::Mask => AlphaMode::Mask { cutoff },
                ::gltf::material::AlphaMode::Blend => AlphaMode::Blend,
            };
            let mut material = Material {
                appearance: TriangleAppearance {
                    base_color,
                    alpha_mode,
                    unlit: source.unlit(),
                    double_sided: source.double_sided(),
                    ..TriangleAppearance::default()
                },
                texcoord_set: None,
                uv: UvTransform::default(),
            };
            if let Some(info) = pbr.base_color_texture() {
                let mut set = info.tex_coord();
                if let Some(extension) = index
                    .and_then(|i| raw.get("materials")?.get(i))
                    .and_then(|m| m.get("pbrMetallicRoughness"))
                    .and_then(|p| p.get("baseColorTexture"))
                    .and_then(|t| t.get("extensions"))
                    .and_then(|e| e.get("KHR_texture_transform"))
                {
                    let extension = extension
                        .as_object()
                        .ok_or("glTF texture transform must be an object")?;
                    material.uv.offset = pair(extension.get("offset"), [0.; 2])?;
                    material.uv.scale = pair(extension.get("scale"), [1.; 2])?;
                    if let Some(rotation) = extension.get("rotation") {
                        material.uv.rotation = rotation
                            .as_f64()
                            .filter(|v| v.is_finite())
                            .ok_or("glTF texture rotation must be finite")?;
                    }
                    if let Some(override_set) = extension.get("texCoord") {
                        set = override_set
                            .as_u64()
                            .and_then(|v| u32::try_from(v).ok())
                            .ok_or(
                                "glTF texture coordinate override must be an unsigned integer",
                            )?;
                    }
                }
                let texture = info.texture();
                if raw
                    .get("textures")
                    .and_then(|t| t.get(texture.index()))
                    .and_then(|t| t.get("source"))
                    .and_then(Value::as_u64)
                    .is_none()
                {
                    return Err("glTF base color texture has no supported image source".into());
                }
                let image = texture.source();
                let image_index = image.index();
                if let std::collections::btree_map::Entry::Vacant(entry) = images.entry(image_index)
                {
                    entry.insert(read_image(
                        image,
                        parent,
                        buffers,
                        &mut total_bytes,
                        &mut pixels,
                    )?);
                }
                let sampler = texture.sampler();
                let filter = match sampler.mag_filter() {
                    Some(::gltf::texture::MagFilter::Nearest) => TextureFilter::Nearest,
                    _ => TextureFilter::Linear,
                };
                // glTF specifies these base-level fallbacks when mipmaps cannot
                // be generated. The rasterizer still distinguishes min/mag.
                let min_filter = match sampler.min_filter() {
                    Some(
                        ::gltf::texture::MinFilter::Nearest
                        | ::gltf::texture::MinFilter::NearestMipmapNearest
                        | ::gltf::texture::MinFilter::NearestMipmapLinear,
                    ) => TextureFilter::Nearest,
                    _ => TextureFilter::Linear,
                };
                material.appearance.texture = Some(Texture {
                    interpolation_space: crate::scene::TextureInterpolationSpace::Linear,
                    image: Arc::clone(&images[&image_index]),
                    wrap_s: wrapping(sampler.wrap_s()),
                    wrap_t: wrapping(sampler.wrap_t()),
                    filter,
                    min_filter,
                });
                material.texcoord_set = Some(set);
            }
            materials.insert(index, material);
        }
    }
    Ok(materials)
}

/// Decode one bounded PNG or JPEG from a local URI, data URI or buffer view.
#[allow(
    clippy::needless_pass_by_value,
    reason = "glTF resource handles are cheap document values matching reader API ownership conventions."
)]
fn read_image(
    image: ::gltf::Image<'_>,
    parent: &Path,
    buffers: &[Vec<u8>],
    total_bytes: &mut u64,
    total_pixels: &mut u64,
) -> Result<Arc<RgbaImage>> {
    let (bytes, mime): (Cow<'_, [u8]>, Option<&str>) = match image.source() {
        ::gltf::image::Source::View { view, mime_type } => {
            if view.stride().is_some() {
                return Err("glTF image view must not have an element stride".into());
            }
            let end = view
                .offset()
                .checked_add(view.length())
                .ok_or("glTF image extent overflow")?;
            let bytes = buffers
                .get(view.buffer().index())
                .and_then(|b| b.get(view.offset()..end))
                .ok_or("glTF image view exceeds its buffer")?;
            (Cow::Borrowed(bytes), Some(mime_type))
        }
        ::gltf::image::Source::Uri { uri, mime_type } => {
            let (bytes, mime) = if uri.starts_with("data:") {
                let (header, data) = uri.split_once(',').ok_or("malformed glTF image data URI")?;
                let mime = header
                    .strip_prefix("data:")
                    .and_then(|h| h.strip_suffix(";base64"))
                    .ok_or("glTF image data URI must use base64")?;
                if !matches!(mime, "image/png" | "image/jpeg") {
                    return Err("glTF base color images require PNG or JPEG".into());
                }
                if mime_type.is_some_and(|declared| declared != mime) {
                    return Err("glTF image MIME type differs from its data URI".into());
                }
                (
                    base64::engine::general_purpose::STANDARD.decode(data)?,
                    Some(mime),
                )
            } else {
                (read_file(&relative_resource(parent, uri)?)?, mime_type)
            };
            *total_bytes = total_bytes
                .checked_add(bytes.len() as u64)
                .ok_or("glTF image byte count overflow")?;
            if *total_bytes > crate::formats::MAX_INPUT_BYTES {
                return Err(
                    "glTF buffers and external images exceed the aggregate 512 MiB limit".into(),
                );
            }
            (Cow::Owned(bytes), mime)
        }
    };
    let format = image::guess_format(&bytes)?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg)
        || mime.is_some_and(|m| {
            !matches!(
                (m, format),
                ("image/png", ImageFormat::Png) | ("image/jpeg", ImageFormat::Jpeg)
            )
        })
    {
        return Err("glTF base color image format must match its PNG/JPEG MIME type".into());
    }
    let (width, height) =
        image::ImageReader::with_format(Cursor::new(&bytes), format).into_dimensions()?;
    let pixels = u64::from(width) * u64::from(height);
    *total_pixels = total_pixels
        .checked_add(pixels)
        .ok_or("glTF image pixel count overflow")?;
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || pixels > 16 * 1024 * 1024
        || *total_pixels > 64 * 1024 * 1024
    {
        return Err("glTF color images exceed the texture dimensions or pixel budget".into());
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(&bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    Ok(Arc::new(reader.decode()?.to_rgba8()))
}

/// Validate a base or morph UV accessor against core and required quantization layouts.
fn uv_layout(accessor: &::gltf::Accessor<'_>, quantized: bool, morph: bool) -> Result<()> {
    let valid = match (accessor.data_type(), accessor.normalized()) {
        (DataType::F32, false) | (DataType::U8 | DataType::U16, true) => true,
        (DataType::I8 | DataType::I16, true) => quantized || morph,
        (DataType::I8 | DataType::I16, false) => quantized,
        (DataType::U8 | DataType::U16, false) => quantized && !morph,
        _ => false,
    };
    if !valid || accessor.dimensions() != Dimensions::Vec2 {
        return Err("glTF texture coordinate accessor requires VEC2 float32 or permitted integer components".into());
    }
    Ok(())
}

/// Convert raw UV components while preserving negative unnormalized quantized values.
fn uv_vector(values: [f64; 2], accessor: &::gltf::Accessor<'_>) -> [f64; 2] {
    let scale = if accessor.normalized() {
        match accessor.data_type() {
            DataType::I8 => 127.,
            DataType::U8 => 255.,
            DataType::I16 => 32767.,
            DataType::U16 => 65535.,
            _ => 1.,
        }
    } else {
        1.
    };
    if accessor.normalized() {
        values.map(|v| (v / scale).max(-1.))
    } else {
        values
    }
}

/// Decode finite texture coordinate pairs with the accessor's normalization rules.
#[allow(
    clippy::needless_pass_by_value,
    reason = "glTF resource handles are cheap document values matching reader API ownership conventions."
)]
fn read_uv_accessor(
    accessor: ::gltf::Accessor<'_>,
    buffers: &[Vec<u8>],
    quantized: bool,
    morph: bool,
) -> Result<Vec<[f64; 2]>> {
    uv_layout(&accessor, quantized, morph)?;
    macro_rules! read {
        ($ty:ty) => {{
            Iter::<[$ty; 2]>::new(accessor.clone(), |b| {
                buffers.get(b.index()).map(Vec::as_slice)
            })
            .ok_or("glTF texture coordinates are unreadable")?
            .map(|v| uv_vector(v.map(f64::from), &accessor))
            .collect()
        }};
    }
    Ok(match accessor.data_type() {
        DataType::F32 => read!(f32),
        DataType::I8 => read!(i8),
        DataType::U8 => read!(u8),
        DataType::I16 => read!(i16),
        DataType::U16 => read!(u16),
        DataType::U32 => unreachable!(),
    })
}

/// Decode the material-selected texture coordinate attribute for one primitive.
///
/// # Errors
/// Returns an error for a missing selected attribute, incompatible layout or vertex
/// count, unreadable ordinary or compressed data, or non-finite coordinates.
#[allow(
    clippy::needless_pass_by_value,
    reason = "glTF resource handles are cheap document values matching reader API ownership conventions."
)]
pub(super) fn read_texcoords(
    primitive: ::gltf::mesh::Primitive<'_>,
    buffers: &[Vec<u8>],
    geometry: Option<&crate::formats::draco::DecodedGeometry>,
    quantized: bool,
    set: u32,
) -> Result<Vec<[f64; 2]>> {
    let accessor = primitive
        .get(&::gltf::Semantic::TexCoords(set))
        .ok_or("glTF base color texture references missing texture coordinates")?;
    uv_layout(&accessor, quantized, false)?;
    let id = primitive
        .extension_value("KHR_draco_mesh_compression")
        .and_then(|e| e.get("attributes"))
        .and_then(|a| a.get(format!("TEXCOORD_{set}")));
    let values = if let Some((geometry, id)) = geometry.zip(id) {
        decoded_attribute(geometry, id, accessor.clone())?
            .values
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| uv_vector([v[0], v[1]], &accessor))
            .collect()
    } else {
        read_uv_accessor(accessor, buffers, quantized, false)?
    };
    Ok(values)
}

/// Add a selected texture coordinate morph before the material texture transform.
///
/// # Errors
/// Returns an error for a missing base UV set, invalid target reference or layout,
/// incompatible counts, unreadable data, or non-finite morphed coordinates.
pub(super) fn apply_uv_morphs(
    node: &::gltf::Node<'_>,
    mesh: &::gltf::Mesh<'_>,
    primitive: usize,
    context: &LoadContext<'_, '_>,
    set: u32,
    values: &mut [[f64; 2]],
) -> Result<()> {
    let weights = context.poses[node.index()]
        .weights
        .as_deref()
        .or_else(|| mesh.weights());
    let targets = context.raw["meshes"][mesh.index()]["primitives"][primitive]
        .get("targets")
        .and_then(Value::as_array);
    for (i, target) in targets.into_iter().flatten().enumerate() {
        let Some(index) = target.get(format!("TEXCOORD_{set}")) else {
            continue;
        };
        let index = index
            .as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .ok_or("glTF UV morph accessor index is invalid")?;
        let accessor = context
            .accessors
            .get(index)
            .ok_or("glTF UV morph accessor is missing")?
            .clone();
        if accessor.count() != values.len() {
            return Err("glTF UV morph count differs from its base coordinates".into());
        }
        let weight = weights.map_or(0., |w| f64::from(w[i]));
        for (uv, delta) in values.iter_mut().zip(read_uv_accessor(
            accessor,
            context.buffers,
            context.quantized,
            true,
        )?) {
            for j in 0..2 {
                if !delta[j].is_finite() {
                    return Err("glTF UV morph has non-finite deltas".into());
                }
                uv[j] += delta[j] * weight;
            }
        }
    }
    Ok(())
}
