//! Addressed FreeCAD GUI visibility, diffuse colors and versioned material lists.

use super::{
    super::fcstd_container::{DocumentArchive, MAX_XML},
    document::{self, ObjectDocument},
};
use crate::Result;
use std::collections::BTreeSet;

/// Maximum selected native face colors in one object.
const MAX_COLORS: usize = 4096;
/// Maximum decoded appearance side-member bytes.
const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Maximum ignored material string extent, without decoding or resolving external assets.
const MAX_STRING: usize = 1024 * 1024;

/// Add optional GUI appearance using exact persisted object names.
///
/// # Errors
///
/// Selected GUI ownership, typed properties, colors or material-list records are invalid or excessive.
pub(super) fn apply(archive: &mut DocumentArchive, objects: &mut ObjectDocument) -> Result<()> {
    let Some(bytes) = archive.read_optional("GuiDocument.xml", MAX_XML)? else {
        return Ok(());
    };
    let text = std::str::from_utf8(&bytes)?;
    let xml = super::super::fcstd_container::xml(text)?;
    let root = xml.root_element();
    if root.tag_name().name() != "Document"
        || root.tag_name().namespace().is_some()
        || root.attribute("SchemaVersion") != Some("1")
    {
        return Err("FCStd invalid GuiDocument root".into());
    }
    let Some(providers) = document::child(root, "ViewProviderData")? else {
        return Ok(());
    };
    let mut seen = BTreeSet::new();
    for provider in providers
        .children()
        .filter(|node| node.has_tag_name("ViewProvider"))
    {
        let name = document::attribute(provider, "name")?;
        let Some(&index) = objects.names.get(name) else {
            continue;
        };
        if !seen.insert(index) {
            return Err("FCStd duplicate GUI view provider".into());
        }
        let properties = document::required_child(provider, "Properties")?;
        apply_provider(
            archive,
            &mut objects.objects[index],
            properties,
            objects.legacy_alpha,
        )?;
    }
    Ok(())
}

/// Apply one qualified view provider's typed metadata and addressed appearance lists.
///
/// # Errors
///
/// Duplicate properties, native types, appearance scalars or selected list carriers are invalid.
fn apply_provider(
    archive: &mut DocumentArchive,
    object: &mut document::Object,
    properties: roxmltree::Node<'_, '_>,
    legacy_alpha: Option<bool>,
) -> Result<()> {
    let mut properties_seen = BTreeSet::new();
    let mut color_file = None;
    let mut material_file = None;
    for property in properties
        .children()
        .filter(|node| node.has_tag_name("Property"))
    {
        let name = document::attribute(property, "name")?;
        if !properties_seen.insert(name) {
            return Err("FCStd duplicate GUI property".into());
        }
        match name {
            "Visibility" => {
                document::typed(property, "App::PropertyBool")?;
                let visible = document::boolean(document::required_child(property, "Bool")?)?;
                if object.visible.is_none() {
                    object.visible = Some(visible);
                }
            }
            "ShapeColor" => {
                document::typed(property, "App::PropertyColor")?;
                let value: u32 = document::attribute(
                    document::required_child(property, "PropertyColor")?,
                    "value",
                )?
                .parse()?;
                object.style.color = rgb(value);
                object.authored.color = true;
            }
            "Transparency" => {
                document::typed(property, "App::PropertyPercent")?;
                let value: u32 =
                    document::attribute(document::required_child(property, "Integer")?, "value")?
                        .parse()?;
                if value > 100 {
                    return Err("FCStd transparency must be between 0 and 100".into());
                }
                object.style.alpha = u8::try_from(255 * (100 - value) / 100)?;
                object.authored.opacity = true;
            }
            "DisplayMode" => {
                document::typed(property, "App::PropertyEnumeration")?;
                let mode: u32 =
                    document::attribute(document::required_child(property, "Integer")?, "value")?
                        .parse()?;
                object.style.wireframe = mode == 1 && object.kind.starts_with("Part::Part2DObject");
            }
            "DiffuseColor" => {
                document::typed(property, "App::PropertyColorList")?;
                color_file = Some(document::attribute(
                    document::required_child(property, "ColorList")?,
                    "file",
                )?);
            }
            "ShapeAppearance" => {
                document::typed(property, "App::PropertyMaterialList")?;
                let list = document::required_child(property, "MaterialList")?;
                let version = list.attribute("version").unwrap_or("0");
                if version != "3" {
                    return Err("FCStd material list requires qualified version 3".into());
                }
                material_file = Some(document::attribute(list, "file")?);
            }
            _ => {}
        }
    }
    if let Some(file) = color_file.filter(|file| !file.is_empty()) {
        let colors = color_list(&archive.read(file, MAX_BYTES)?, legacy_alpha)?;
        let authored = !colors.is_empty();
        set_colors(&mut object.style, colors);
        object.authored.color |= authored;
    }
    if let Some(file) = material_file.filter(|file| !file.is_empty()) {
        let colors = material_list(&archive.read(file, MAX_BYTES)?)?;
        let authored = !colors.is_empty();
        set_colors(&mut object.style, colors);
        object.authored.color |= authored;
        object.authored.opacity |= authored;
    }
    Ok(())
}

/// Retain uniform or per-face source colors with the object's opacity bound.
fn set_colors(style: &mut super::super::cad::ShapeStyle, colors: Vec<[u8; 4]>) {
    if colors.is_empty() {
        return;
    }
    if colors.len() == 1 {
        let [r, g, b, alpha] = colors[0];
        style.color = [r, g, b];
        style.alpha = style.alpha.min(alpha);
    } else {
        style.faces = colors;
    }
}

/// Extract source sRGB channels from native packed 0xRRGGBBAA numeric colors.
fn rgb(value: u32) -> [u8; 3] {
    let [r, g, b, _] = value.to_be_bytes();
    [r, g, b]
}

/// Read one exact little-endian native integer field.
///
/// # Errors
///
/// The selected scalar is truncated.
fn word(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..)
            .and_then(|tail| tail.get(..4))
            .ok_or("FCStd truncated appearance scalar")?
            .try_into()?,
    ))
}

/// Validate a selected native list count before any vector allocation.
///
/// # Errors
///
/// The count is truncated, excessive or inconsistent with its fixed record extent.
fn count(bytes: &[u8], stride: usize) -> Result<(usize, usize)> {
    let count = usize::try_from(word(bytes, 0)?)?;
    if count > MAX_COLORS {
        return Err("FCStd appearance exceeds 4096 face records".into());
    }
    let end = count
        .checked_mul(stride)
        .and_then(|value| value.checked_add(4))
        .filter(|&end| end <= bytes.len())
        .ok_or("FCStd truncated appearance list")?;
    Ok((count, end))
}

/// Decode qualified packed legacy or 1.1 per-face color opacity.
///
/// # Errors
///
/// Version interpretation, count, exact file extent or individual scalar data is invalid.
fn color_list(bytes: &[u8], legacy: Option<bool>) -> Result<Vec<[u8; 4]>> {
    let (count, end) = count(bytes, 4)?;
    if end != bytes.len() {
        return Err("FCStd color list has trailing bytes".into());
    }
    let mut colors = Vec::with_capacity(count);
    for index in 0..count {
        let value = word(bytes, 4 + index * 4)?;
        let [r, g, b] = rgb(value);
        let packed = value.to_be_bytes()[3];
        let alpha = if count == 1 {
            255
        } else if legacy.ok_or("FCStd per-face opacity requires a known ProgramVersion")? {
            255 - packed
        } else {
            packed
        };
        colors.push([r, g, b, alpha]);
    }
    Ok(colors)
}

/// Decode version-three diffuse materials and validate every ignored bounded trailing string.
///
/// # Errors
///
/// Counts, scalar opacity, version-three strings or exact file extent are invalid.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Finite zero-to-one material opacity is explicitly rounded into a bounded byte."
)]
fn material_list(bytes: &[u8]) -> Result<Vec<[u8; 4]>> {
    let (count, mut end) = count(bytes, 24)?;
    let mut colors = Vec::with_capacity(count);
    for index in 0..count {
        let offset = 4 + index * 24;
        let [r, g, b] = rgb(word(bytes, offset + 4)?);
        let shininess = f32::from_bits(word(bytes, offset + 16)?);
        let transparency = f32::from_bits(word(bytes, offset + 20)?);
        if !shininess.is_finite()
            || !transparency.is_finite()
            || !(0.0..=1.0).contains(&transparency)
        {
            return Err("FCStd material has invalid finite opacity".into());
        }
        let alpha = (255.0_f32 * (1.0 - transparency) + 0.5) as u8;
        colors.push([r, g, b, alpha]);
    }
    for _ in 0..count {
        for _ in 0..3 {
            let length = usize::try_from(word(bytes, end)?)?;
            if length > MAX_STRING {
                return Err("FCStd material string exceeds one MiB".into());
            }
            end = end
                .checked_add(4)
                .and_then(|value| value.checked_add(length))
                .filter(|&end| end <= bytes.len())
                .ok_or("FCStd truncated material string")?;
        }
    }
    if end != bytes.len() {
        return Err("FCStd material list has trailing bytes".into());
    }
    Ok(colors)
}
