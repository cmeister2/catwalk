//! Static glTF and GLB scenes with bounded resources, first-pose deformation,
//! mesh instances and base-color texture appearance.
use super::{read_file, relative_resource, transform::Transform};
use crate::{
    Result,
    scene::{Line, Point, Scene, Triangle, Vec3},
};
use ::gltf::json::validation::Validate;
use base64::Engine;
use std::{collections::HashSet, path::Path};

/// Maximum decoded elements in any geometry accessor.
const MAX_ELEMENTS: usize = 5_000_000;

#[path = "gltf_deform.rs"]
mod deform;

#[path = "gltf_anim.rs"]
mod animation;

#[path = "gltf_instancing.rs"]
mod instancing;

#[path = "gltf_material.rs"]
mod material;

/// Load a glTF or GLB model into the selected scene and first animation pose.
///
/// # Errors
/// Returns an error for invalid references, unsupported required extensions, malformed
/// accessors or resources, non-finite geometry, and exceeded decoding budgets.
#[allow(
    clippy::too_many_lines,
    reason = "The bounded importer keeps required-extension, resource and scene selection validation in one pipeline."
)]
pub(super) fn load(path: &Path) -> Result<Scene> {
    let source = read_file(path)?;
    // Keep the original JSON: newer morph attribute semantics are absent from
    // gltf-json's typed representation and must not disappear during a roundtrip.
    let raw: serde_json::Value = if source.starts_with(b"glTF") {
        serde_json::from_slice(&::gltf::binary::Glb::from_slice(&source)?.json)?
    } else {
        serde_json::from_slice(&source)?
    };
    let model = if raw
        .get("animations")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .any(|animation| {
            animation
                .get("channels")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .any(|channel| {
                    channel
                        .get("target")
                        .and_then(serde_json::Value::as_object)
                        .is_some_and(|target| !target.contains_key("node"))
                })
        }) {
        // Current glTF permits inactive channels with no target node. The
        // older typed crate requires one, so remove only those inactive
        // channels from its view; the original JSON remains available below.
        let mut typed = raw.clone();
        for animation in typed["animations"].as_array_mut().into_iter().flatten() {
            if let Some(channels) = animation["channels"].as_array_mut() {
                channels.retain(|channel| {
                    channel
                        .get("target")
                        .and_then(serde_json::Value::as_object)
                        .is_none_or(|target| target.contains_key("node"))
                });
            }
        }
        // Keep typed deserialization in gltf's shared constructor so that
        // both branches use one copy of the byte-slice parser.
        let typed_json = serde_json::to_vec(&typed)?;
        drop(typed);
        let mut model = ::gltf::Gltf::from_slice_without_validation(&typed_json).map_err(
            |error| -> crate::Error {
                match error {
                    ::gltf::Error::Deserialize(error) => error.into(),
                    error => error.into(),
                }
            },
        )?;
        drop(typed_json);
        if source.starts_with(b"glTF") {
            model.blob = ::gltf::binary::Glb::from_slice(&source)?
                .bin
                .map(std::borrow::Cow::into_owned);
        }
        model
    } else {
        ::gltf::Gltf::from_slice_without_validation(&source)?
    };
    for required in model.extensions_required() {
        if ![
            "KHR_materials_unlit",
            "KHR_draco_mesh_compression",
            "KHR_mesh_quantization",
            "KHR_texture_transform",
            "EXT_mesh_gpu_instancing",
        ]
        .contains(&required)
        {
            return Err(format!("required glTF extension {required} is not implemented").into());
        }
    }
    // Preserve all core validation. The crate's extension allowlist predates
    // our separate Draco decoder, so its already-checked required names are
    // the only unsupported-extension reports we handle ourselves.
    let root = model.document.as_json();
    let compressed_accessors = compressed_accessors(&raw);
    let mut errors = Vec::new();
    root.validate(root, ::gltf::json::Path::new, &mut |path, error| {
        let path = path();
        let compressed_missing_view = matches!(error, ::gltf::json::validation::Error::Missing)
            && path
                .0
                .strip_prefix("accessors[")
                .and_then(|s| s.strip_suffix("].bufferView"))
                .and_then(|s| s.parse::<usize>().ok())
                .is_some_and(|i| compressed_accessors.contains(&i));
        if !(compressed_missing_view
            || matches!(error, ::gltf::json::validation::Error::Unsupported)
                && path.0.starts_with("extensionsRequired["))
        {
            errors.push((path, error));
        }
    });
    if !errors.is_empty() {
        return Err(::gltf::Error::Validation(errors).into());
    }
    let mut buffers = Vec::new();
    let mut total_bytes = 0u64;
    if model.buffers().len() > 256 {
        return Err("glTF exceeds the buffer reference limit".into());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    for buffer in model.buffers() {
        let bytes = match buffer.source() {
            ::gltf::buffer::Source::Bin => model.blob.clone().ok_or("missing GLB binary chunk")?,
            ::gltf::buffer::Source::Uri(uri) if uri.starts_with("data:") => {
                let (header, data) = uri.split_once(',').ok_or("malformed glTF data URI")?;
                if !header.ends_with(";base64") {
                    return Err("only base64 glTF data URIs are supported".into());
                }
                base64::engine::general_purpose::STANDARD.decode(data)?
            }
            ::gltf::buffer::Source::Uri(uri) => read_file(&relative_resource(parent, uri)?)?,
        };
        if bytes.len() < buffer.length() {
            return Err("glTF buffer is shorter than its declared length".into());
        }
        total_bytes = total_bytes
            .checked_add(bytes.len() as u64)
            .ok_or("glTF buffer byte count overflow")?;
        if total_bytes > super::MAX_INPUT_BYTES {
            return Err("glTF buffers exceed the aggregate 512 MiB limit".into());
        }
        buffers.push(bytes);
    }
    let poses = animation::first_pose(&model, &buffers, &raw)?;
    let views: Vec<_> = model.views().collect();
    let accessors: Vec<_> = model.accessors().collect();
    let morph_counts = deform::morph_target_counts(&raw)?;
    let mut output = Scene::default();
    let roots: Vec<_> = if let Some(scene) = model.default_scene().or_else(|| model.scenes().next())
    {
        scene.nodes().collect()
    } else {
        let children: HashSet<_> = model
            .nodes()
            .flat_map(|n| n.children().map(|c| c.index()).collect::<Vec<_>>())
            .collect();
        model
            .nodes()
            .filter(|n| !children.contains(&n.index()))
            .collect()
    };
    let globals = deform::global_transforms(&model, &roots, &poses)?;
    let materials = material::load(&model, &raw, parent, &buffers, &globals)?;
    let context = LoadContext {
        buffers: &buffers,
        views: &views,
        accessors: &accessors,
        morph_counts: &morph_counts,
        poses: &poses,
        quantized: model
            .extensions_required()
            .any(|name| name == "KHR_mesh_quantization"),
        globals: &globals,
        raw: &raw,
        materials: &materials,
        work: std::cell::Cell::new(0),
    };
    for node in roots {
        visit(node, &context, &mut output)?;
    }
    output.validate()?;
    Ok(output)
}

/// Compressed accessors describe decoded values and legitimately omit a view.
/// Only attributes actually named in this extension and its indices qualify.
fn compressed_accessors(root: &serde_json::Value) -> HashSet<usize> {
    let mut result = HashSet::new();
    for mesh in root
        .get("meshes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        for primitive in mesh
            .get("primitives")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(extension) = primitive
                .get("extensions")
                .and_then(|e| e.get("KHR_draco_mesh_compression"))
            {
                if let Some(attributes) = extension
                    .get("attributes")
                    .and_then(serde_json::Value::as_object)
                {
                    for name in attributes.keys() {
                        if let Some(index) = primitive
                            .get("attributes")
                            .and_then(|a| a.get(name))
                            .and_then(serde_json::Value::as_u64)
                            .and_then(|i| usize::try_from(i).ok())
                        {
                            result.insert(index);
                        }
                    }
                }
                if let Some(index) = primitive
                    .get("indices")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|i| usize::try_from(i).ok())
                {
                    result.insert(index);
                }
            }
        }
    }
    result
}

/// Validate the byte range occupied by a strided accessor without overflowing.
fn bounded_range(
    offset: usize,
    count: usize,
    stride: usize,
    item_size: usize,
    available: usize,
) -> Result<()> {
    let required = if count == 0 {
        offset
    } else {
        offset
            .checked_add(
                (count - 1)
                    .checked_mul(stride)
                    .ok_or("glTF accessor size overflow")?,
            )
            .and_then(|v| v.checked_add(item_size))
            .ok_or("glTF accessor size overflow")?
    };
    if count > MAX_ELEMENTS || stride < item_size || required > available {
        return Err("glTF accessor exceeds its buffer view or element limit".into());
    }
    Ok(())
}

/// Check accessor counts, alignment, buffer ranges and ordered sparse indices before iteration.
fn check_accessors(model: &::gltf::Gltf, buffers: &[Vec<u8>]) -> Result<()> {
    for view in model.views() {
        let end = view
            .offset()
            .checked_add(view.length())
            .ok_or("glTF buffer view size overflow")?;
        if end > view.buffer().length() || end > buffers[view.buffer().index()].len() {
            return Err("glTF buffer view exceeds its buffer".into());
        }
    }
    for accessor in model.accessors() {
        if accessor.count() == 0 {
            return Err("glTF accessor count must be positive".into());
        }
        if accessor.count() > MAX_ELEMENTS {
            return Err("glTF accessor exceeds the element limit".into());
        }
        if let Some(view) = accessor.view() {
            bounded_range(
                accessor.offset(),
                accessor.count(),
                view.stride().unwrap_or(accessor.size()),
                accessor.size(),
                view.length(),
            )?;
        }
        if let Some(sparse) = accessor.sparse() {
            if sparse.count() == 0 || sparse.count() > accessor.count() {
                return Err("glTF sparse accessor count exceeds accessor count".into());
            }
            let indices = sparse.indices();
            if indices.view().stride().is_some() || sparse.values().view().stride().is_some() {
                return Err("glTF sparse accessor views must not have strides".into());
            }
            let size = indices.index_type().size();
            bounded_range(
                indices.offset(),
                sparse.count(),
                size,
                size,
                indices.view().length(),
            )?;
            let values = sparse.values();
            bounded_range(
                values.offset(),
                sparse.count(),
                accessor.size(),
                accessor.size(),
                values.view().length(),
            )?;
            let view = indices.view();
            let bytes =
                &buffers[view.buffer().index()][view.offset()..view.offset() + view.length()];
            let mut previous = None;
            for i in 0..sparse.count() {
                let start = indices.offset() + i * size;
                let index = match size {
                    1 => bytes[start] as usize,
                    2 => u16::from_le_bytes(bytes[start..start + 2].try_into()?) as usize,
                    4 => u32::from_le_bytes(bytes[start..start + 4].try_into()?) as usize,
                    _ => return Err("invalid sparse index type".into()),
                };
                if index >= accessor.count() || previous.is_some_and(|p| p >= index) {
                    return Err(
                        "glTF sparse indices must be increasing and within the accessor".into(),
                    );
                }
                previous = Some(index);
            }
        }
    }
    Ok(())
}

/// Convert a linear RGB vertex color to display bytes.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Color components are clamped and rounded to the inclusive byte range before conversion."
)]
fn rgb(value: [f32; 4]) -> [u8; 3] {
    // glTF color factors are linear; PNG stores sRGB.
    std::array::from_fn(|i| {
        let c = value[i].clamp(0., 1.);
        let srgb = if c <= 0.003_130_8 {
            12.92 * c
        } else {
            1.055 * c.powf(1. / 2.4) - 0.055
        };
        (srgb * 255.).round() as u8
    })
}

/// Decoded attributes and indices for one mesh primitive before morphing and skinning.
struct PrimitiveData {
    /// Vertex positions in the primitive coordinate system.
    positions: Vec<Vec3>,
    /// Authored vertex normals, retained at their original lengths until deformation.
    normals: Option<Vec<Vec3>>,
    /// Coordinates from the texture coordinate set selected by the material.
    texcoords: Option<Vec<[f64; 2]>>,
    /// Primitive vertex indices in source order.
    indices: Vec<u32>,
    /// Optional vertex colors with three or four components.
    colors: Option<Vec<[f32; 4]>>,
    /// Whether the attributes came from Draco decompression.
    compressed: bool,
    /// Paired joint-index and weight sets for each vertex.
    influences: Vec<deform::InfluenceSet>,
}

/// Shared validated resources and decoding budgets used while traversing a glTF scene.
struct LoadContext<'a, 'b> {
    /// Decoded bytes for each glTF buffer.
    buffers: &'a [Vec<u8>],
    /// Buffer views indexed by their document indices.
    views: &'a [::gltf::buffer::View<'b>],
    /// Accessors indexed by their document indices.
    accessors: &'a [::gltf::Accessor<'b>],
    /// Validated morph target counts indexed by mesh.
    morph_counts: &'a [usize],
    /// Local node transforms and morph weights evaluated at the first animation pose.
    poses: &'a [animation::Pose],
    /// Whether the required mesh quantization extension permits integer geometry attributes.
    quantized: bool,
    /// World transforms for nodes reachable from the selected scene.
    globals: &'a [Option<Transform>],
    /// Original JSON retaining attributes absent from the typed glTF representation.
    raw: &'a serde_json::Value,
    /// Materials used by selected nodes, including the default material.
    materials: &'a material::Materials,
    /// Accumulated vertex deformation and instance expansion work.
    work: std::cell::Cell<usize>,
}

/// Find a decompressed attribute by its extension ID and validate its semantic layout.
#[allow(
    clippy::needless_pass_by_value,
    reason = "glTF accessors are cheap document handles passed by value consistently with the reader API."
)]
fn decoded_attribute(
    geometry: &super::draco::DecodedGeometry,
    id: &serde_json::Value,
    accessor: ::gltf::Accessor<'_>,
) -> Result<super::draco::DecodedAttribute> {
    use ::gltf::accessor::DataType as A;
    use draco_core::DataType as D;
    let id = u32::try_from(
        id.as_u64()
            .ok_or("glTF Draco attribute ID must be an integer")?,
    )?;
    let attribute = geometry.attribute(id)?;
    if accessor.count() != geometry.point_count
        || accessor.dimensions().multiplicity() != attribute.components
    {
        return Err("glTF Draco attribute layout differs from its accessor".into());
    }
    if !matches!(
        (accessor.data_type(), attribute.data_type),
        (A::I8, D::Int8)
            | (A::U8, D::Uint8)
            | (A::I16, D::Int16)
            | (A::U16, D::Uint16)
            | (A::U32, D::Uint32)
            | (A::F32, D::Float32)
    ) {
        return Err("glTF Draco scalar type differs from its accessor".into());
    }
    Ok(attribute)
}

/// Decode a primitive and validate matching attribute counts, component layouts and indices.
#[allow(
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::cast_possible_truncation,
    reason = "Cheap glTF handles follow reader ownership; geometry budgets bound index casts, and decoded colors retain glTF f32 storage."
)]
fn primitive_data(
    primitive: ::gltf::mesh::Primitive<'_>,
    buffers: &[Vec<u8>],
    views: &[::gltf::buffer::View<'_>],
    quantized: bool,
    texcoord_set: Option<u32>,
) -> Result<PrimitiveData> {
    let reader = primitive.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
    let fallback_colors = || {
        reader
            .read_colors(0)
            .map(|c| c.into_rgba_f32().collect::<Vec<_>>())
    };
    if let Some(extension) = primitive.extension_value("KHR_draco_mesh_compression") {
        if !matches!(
            primitive.mode(),
            ::gltf::mesh::Mode::Triangles | ::gltf::mesh::Mode::TriangleStrip
        ) {
            return Err("glTF Draco extension requires triangle or triangle-strip mode".into());
        }
        let view = usize::try_from(
            extension
                .get("bufferView")
                .and_then(serde_json::Value::as_u64)
                .ok_or("glTF Draco extension lacks a buffer view")?,
        )?;
        let view = views
            .get(view)
            .ok_or("glTF Draco buffer view index is out of bounds")?;
        if view.stride().is_some() {
            return Err("glTF Draco buffer view must not have an element stride".into());
        }
        let bytes = buffers
            .get(view.buffer().index())
            .and_then(|b| b.get(view.offset()..view.offset() + view.length()))
            .ok_or("glTF Draco buffer view exceeds input")?;
        let geometry = super::draco::decode_geometry(bytes)?;
        let faces = geometry
            .faces
            .as_ref()
            .ok_or("glTF Draco payload must be a triangle mesh")?;
        let attributes = extension
            .get("attributes")
            .and_then(serde_json::Value::as_object)
            .ok_or("glTF Draco extension lacks attribute IDs")?;
        let accessor = primitive
            .get(&::gltf::Semantic::Positions)
            .ok_or("glTF primitive has no POSITION accessor")?;
        deform::position_layout(&accessor, quantized, false)?;
        let positions = if let Some(id) = attributes.get("POSITION") {
            decoded_attribute(&geometry, id, accessor.clone())?
                .values
                .as_chunks::<3>()
                .0
                .iter()
                .map(|v| deform::position_vector([v[0], v[1], v[2]], &accessor))
                .collect()
        } else {
            let values = deform::read_positions(accessor, buffers, quantized, false)?;
            if values.len() != geometry.point_count {
                return Err("glTF fallback positions differ from Draco point count".into());
            }
            values
        };
        let colors = if let Some(id) = attributes.get("COLOR_0") {
            let accessor = primitive
                .get(&::gltf::Semantic::Colors(0))
                .ok_or("glTF Draco COLOR_0 has no accessor")?;
            let normalized = accessor.normalized();
            let color = decoded_attribute(&geometry, id, accessor)?;
            if ![3, 4].contains(&color.components) {
                return Err("glTF Draco colors require VEC3/VEC4".into());
            }
            let scale = match color.data_type {
                draco_core::DataType::Float32 => 1.0,
                draco_core::DataType::Uint8 if normalized => 255.0,
                draco_core::DataType::Uint16 if normalized => 65535.0,
                _ => {
                    return Err(
                        "glTF Draco colors require float32 or normalized uint8/uint16".into(),
                    );
                }
            };
            Some(
                color
                    .values
                    .chunks_exact(color.components)
                    .map(|v| {
                        std::array::from_fn(|i| {
                            if i < v.len() {
                                (v[i] / scale) as f32
                            } else {
                                1.0
                            }
                        })
                    })
                    .collect(),
            )
        } else {
            fallback_colors()
        };
        let indices: Vec<u32> = faces.iter().flat_map(|f| f.map(|i| i as u32)).collect();
        if let Some(accessor) = primitive.indices()
            && (accessor.dimensions() != ::gltf::accessor::Dimensions::Scalar
                || !matches!(
                    accessor.data_type(),
                    ::gltf::accessor::DataType::U8
                        | ::gltf::accessor::DataType::U16
                        | ::gltf::accessor::DataType::U32
                )
                || (primitive.mode() == ::gltf::mesh::Mode::Triangles
                    && accessor.count() != indices.len()))
        {
            return Err("glTF Draco index accessor differs from decoded triangles".into());
        }
        let influences =
            deform::read_influences(primitive.clone(), buffers, Some(&geometry), positions.len())?;
        let normals = if let Some(accessor) = primitive.get(&::gltf::Semantic::Normals) {
            Some(if let Some(id) = attributes.get("NORMAL") {
                deform::normal_layout(&accessor, quantized)?;
                decoded_attribute(&geometry, id, accessor.clone())?
                    .values
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|v| deform::normal_vector([v[0], v[1], v[2]], &accessor))
                    .collect()
            } else {
                deform::read_normals(accessor, buffers, quantized)?
            })
        } else {
            None
        };
        let texcoords = texcoord_set
            .map(|set| {
                material::read_texcoords(
                    primitive.clone(),
                    buffers,
                    Some(&geometry),
                    quantized,
                    set,
                )
            })
            .transpose()?;
        Ok(PrimitiveData {
            positions,
            normals,
            texcoords,
            indices,
            colors,
            compressed: true,
            influences,
        })
    } else {
        let accessor = primitive
            .get(&::gltf::Semantic::Positions)
            .ok_or("glTF primitive has no POSITION accessor")?;
        let positions = deform::read_positions(accessor, buffers, quantized, false)?;
        let indices = reader.read_indices().map_or_else(
            || (0..positions.len() as u32).collect(),
            |v| v.into_u32().collect(),
        );
        let influences =
            deform::read_influences(primitive.clone(), buffers, None, positions.len())?;
        let normals = primitive
            .get(&::gltf::Semantic::Normals)
            .map(|accessor| deform::read_normals(accessor, buffers, quantized))
            .transpose()?;
        let texcoords = texcoord_set
            .map(|set| material::read_texcoords(primitive.clone(), buffers, None, quantized, set))
            .transpose()?;
        Ok(PrimitiveData {
            positions,
            normals,
            texcoords,
            indices,
            colors: fallback_colors(),
            compressed: false,
            influences,
        })
    }
}

/// Traverse a selected node, deform its geometry and expand bounded mesh instances into the scene.
#[allow(
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::manual_midpoint,
    reason = "The bounded node visitor handles primitive modes together; cheap node handles follow glTF readers, and color averaging preserves snapshot rounding order."
)]
fn visit(node: ::gltf::Node<'_>, context: &LoadContext<'_, '_>, scene: &mut Scene) -> Result<()> {
    let transform = context.globals[node.index()].ok_or("glTF node has no scene transform")?;
    let skin = node
        .skin()
        .map(|skin| deform::skin_matrices(skin, context.buffers, context.globals))
        .transpose()?;
    let instances = instancing::transforms(
        context.raw,
        node.index(),
        context.accessors,
        context.buffers,
    )?;
    if instances.is_some() && skin.is_some() {
        return Err("glTF skin combined with EXT_mesh_gpu_instancing is not implemented".into());
    }
    let instances = instances.unwrap_or_else(|| vec![Transform::IDENTITY]);
    if let Some(mesh) = node.mesh() {
        for primitive in mesh.primitives() {
            let material = context
                .materials
                .get(&primitive.material().index())
                .ok_or("glTF selected material is missing")?;
            let mut data = primitive_data(
                primitive.clone(),
                context.buffers,
                context.views,
                context.quantized,
                material.texcoord_set,
            )?;
            deform::apply_morphs(&node, &mesh, primitive.index(), context, &mut data)?;
            if data
                .normals
                .as_ref()
                .is_some_and(|n| n.len() != data.positions.len())
                || data
                    .texcoords
                    .as_ref()
                    .is_some_and(|uv| uv.len() != data.positions.len())
            {
                return Err("glTF normal/texture coordinate count does not match positions".into());
            }
            if let Some(texcoords) = &mut data.texcoords {
                material::apply_uv_morphs(
                    &node,
                    &mesh,
                    primitive.index(),
                    context,
                    material
                        .texcoord_set
                        .ok_or("glTF texture coordinates lack a selected set")?,
                    texcoords,
                )?;
                material.transform_uvs(texcoords)?;
            }
            if data.colors.as_ref().is_some_and(|colors| {
                colors.len() != data.positions.len()
                    || colors
                        .iter()
                        .flatten()
                        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            }) {
                return Err("glTF vertex colors must match positions and contain finite RGBA values between zero and one".into());
            }
            let work = data
                .positions
                .len()
                .checked_mul(instances.len())
                .and_then(|n| context.work.get().checked_add(n))
                .ok_or("glTF instanced vertex work overflow")?;
            if work > 50_000_000 {
                return Err("glTF exceeds the expanded vertex work limit".into());
            }
            context.work.set(work);
            let mode = if data.compressed {
                ::gltf::mesh::Mode::Triangles
            } else {
                primitive.mode()
            };
            let count = match mode {
                ::gltf::mesh::Mode::Triangles => data.indices.len() / 3,
                ::gltf::mesh::Mode::TriangleStrip | ::gltf::mesh::Mode::TriangleFan => {
                    data.indices.len().saturating_sub(2)
                }
                ::gltf::mesh::Mode::Points => data.indices.len(),
                ::gltf::mesh::Mode::Lines => data.indices.len() / 2,
                ::gltf::mesh::Mode::LineStrip => data.indices.len().saturating_sub(1),
                ::gltf::mesh::Mode::LineLoop => {
                    if data.indices.len() > 1 {
                        data.indices.len()
                    } else {
                        0
                    }
                }
            };
            let existing = scene.triangles.len() + scene.lines.len() + scene.points.len();
            if count
                .checked_mul(instances.len())
                .and_then(|n| n.checked_add(existing))
                .is_none_or(|n| n > MAX_ELEMENTS)
            {
                return Err("glTF scene exceeds the expanded geometry limit".into());
            }
            for instance in &instances {
                let transform = transform.then(*instance);
                let normals = data
                    .normals
                    .as_ref()
                    .map(|normals| {
                        deform::apply_normals(normals, &data.influences, skin.as_deref(), transform)
                            .map(|normals| {
                                normals
                                    .into_iter()
                                    .map(|v| Vec3::new(v.x, -v.z, v.y))
                                    .collect::<Vec<_>>()
                            })
                    })
                    .transpose()?;
                let world = deform::apply_skin(
                    data.positions.clone(),
                    &data.influences,
                    skin.as_deref(),
                    transform,
                )?;
                let vertices: Vec<_> = world
                    .into_iter()
                    .map(|v| Vec3::new(v.x, -v.z, v.y))
                    .collect();
                let indices = &data.indices;
                if indices.iter().any(|&i| i as usize >= vertices.len()) {
                    return Err("glTF triangle references a missing vertex".into());
                }
                let factor = primitive
                    .material()
                    .pbr_metallic_roughness()
                    .base_color_factor();
                let colors: Option<Vec<_>> = data.colors.as_ref().map(|v| {
                    v.iter()
                        .map(|c| std::array::from_fn::<_, 4, _>(|i| c[i] * factor[i]))
                        .collect()
                });
                if colors.as_ref().is_some_and(|c| c.len() != vertices.len()) {
                    return Err("glTF color count does not match position count".into());
                }
                let face_color = |face: [u32; 3]| -> [u8; 3] {
                    if let Some(colors) = &colors {
                        rgb(std::array::from_fn(|channel| {
                            face.iter()
                                .map(|&i| colors[i as usize][channel])
                                .sum::<f32>()
                                / 3.
                        }))
                    } else {
                        rgb(factor)
                    }
                };
                let mut add_triangle = |face: [u32; 3]| -> Result<()> {
                    if scene.triangles.len() >= MAX_ELEMENTS {
                        return Err("glTF scene exceeds triangle limit".into());
                    }
                    let mut face = face;
                    // glTF reverses front-face winding for mirrored node transforms.
                    if skin.is_none() && mirrored(transform) {
                        face.swap(1, 2);
                    }
                    let mut appearance = material.appearance.clone();
                    appearance.colors = data
                        .colors
                        .as_ref()
                        .map(|colors| face.map(|i| colors[i as usize].map(f64::from)));
                    appearance.texcoords = data
                        .texcoords
                        .as_ref()
                        .map(|uvs| face.map(|i| uvs[i as usize]));
                    appearance.normals = normals.as_ref().and_then(|normals| {
                        let values = face.map(|i| normals[i as usize]);
                        values.iter().all(|n| n.length() > 0.).then_some(values)
                    });
                    scene.appearances.insert(scene.triangles.len(), appearance);
                    scene.triangles.push(Triangle {
                        vertices: face.map(|i| vertices[i as usize]),
                        color: Some(face_color(face)),
                    });
                    Ok(())
                };
                match mode {
                    ::gltf::mesh::Mode::Triangles => {
                        if indices.len() % 3 != 0 {
                            return Err(
                                "glTF triangle index count is not a multiple of three".into()
                            );
                        }
                        for face in indices.as_chunks::<3>().0 {
                            add_triangle([face[0], face[1], face[2]])?;
                        }
                    }
                    ::gltf::mesh::Mode::TriangleStrip => {
                        for (i, face) in indices.windows(3).enumerate() {
                            add_triangle(if i % 2 == 0 {
                                [face[0], face[1], face[2]]
                            } else {
                                [face[1], face[0], face[2]]
                            })?;
                        }
                    }
                    ::gltf::mesh::Mode::TriangleFan => {
                        if let Some(&first) = indices.first() {
                            for face in indices[1..].windows(2) {
                                add_triangle([first, face[0], face[1]])?;
                            }
                        }
                    }
                    ::gltf::mesh::Mode::Points => {
                        for &i in indices {
                            if scene.points.len() >= MAX_ELEMENTS {
                                return Err("glTF scene exceeds point limit".into());
                            }
                            scene.points.push(Point {
                                position: vertices[i as usize],
                                color: Some(
                                    colors
                                        .as_ref()
                                        .map_or_else(|| rgb(factor), |c| rgb(c[i as usize])),
                                ),
                            });
                        }
                    }
                    ::gltf::mesh::Mode::Lines
                    | ::gltf::mesh::Mode::LineStrip
                    | ::gltf::mesh::Mode::LineLoop => {
                        let edges: Vec<[u32; 2]> = if mode == ::gltf::mesh::Mode::Lines {
                            if !indices.len().is_multiple_of(2) {
                                return Err("glTF line index count is not a multiple of two".into());
                            }
                            indices
                                .as_chunks::<2>()
                                .0
                                .iter()
                                .map(|v| [v[0], v[1]])
                                .collect()
                        } else {
                            let mut edges: Vec<_> =
                                indices.windows(2).map(|v| [v[0], v[1]]).collect();
                            if mode == ::gltf::mesh::Mode::LineLoop && indices.len() > 1 {
                                edges.push([*indices.last().unwrap(), indices[0]]);
                            }
                            edges
                        };
                        if scene.lines.len() + edges.len() > MAX_ELEMENTS {
                            return Err("glTF scene exceeds line limit".into());
                        }
                        for edge in edges {
                            let color = colors.as_ref().map_or_else(
                                || rgb(factor),
                                |colors| {
                                    rgb(std::array::from_fn(|i| {
                                        (colors[edge[0] as usize][i] + colors[edge[1] as usize][i])
                                            / 2.0
                                    }))
                                },
                            );
                            scene.lines.push(Line {
                                vertices: edge.map(|i| vertices[i as usize]),
                                color: Some(color),
                            });
                        }
                    }
                }
            }
        }
    }
    for child in node.children() {
        visit(child, context, scene)?;
    }
    Ok(())
}

/// Detect whether the linear transform reverses orientation.
fn mirrored(transform: Transform) -> bool {
    let columns: [Vec3; 3] =
        std::array::from_fn(|i| Vec3::new(transform.0[i][0], transform.0[i][1], transform.0[i][2]));
    // Scale before the sign test to avoid overflow for large finite transforms.
    let largest = columns
        .iter()
        .flat_map(|v| [v.x.abs(), v.y.abs(), v.z.abs()])
        .fold(0., f64::max);
    if largest == 0. {
        return false;
    }
    let columns = columns.map(|v| Vec3::new(v.x / largest, v.y / largest, v.z / largest));
    columns[0].dot(columns[1].cross(columns[2])) < 0.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessor_ranges_check_last_element_and_overflow() {
        assert!(bounded_range(0, 3, 12, 12, 36).is_ok());
        assert!(bounded_range(0, 3, 16, 12, 43).is_err());
        assert!(bounded_range(0, 3, 16, 12, 44).is_ok());
        assert!(bounded_range(usize::MAX, 3, 12, 12, 36).is_err());
    }
}
