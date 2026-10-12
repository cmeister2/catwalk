//! Bounded Wavefront diffuse materials and local image maps.

use std::{
    collections::HashMap,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use image::{ImageFormat, RgbaImage};

use crate::{
    Result,
    scene::{Texture, TextureFilter, TextureInterpolationSpace, TextureWrap},
};

/// Maximum independent material or texture resource references per OBJ document.
const MAX_REFERENCES: usize = 1024;
/// Maximum material definitions across referenced MTL files.
const MAX_MATERIALS: usize = 65_536;
/// Maximum name or path token length accepted by material readers.
const MAX_NAME_BYTES: usize = 4096;
/// Maximum encoded physical or continued material line length.
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum dimension of one decoded texture.
const MAX_TEXTURE_AXIS: u32 = 8192;
/// Maximum pixels decoded for one texture.
const MAX_TEXTURE_PIXELS: u64 = 16 * 1024 * 1024;
/// Maximum unique texture pixels retained by an OBJ document.
const MAX_TOTAL_TEXTURE_PIXELS: u64 = 64 * 1024 * 1024;

/// Authored diffuse reflectivity, dissolve and optional image coordinates.
#[derive(Clone, Debug)]
pub(super) struct Material {
    /// Byte diffuse color retained for legacy polylines and triangle metadata.
    pub color: Option<[u8; 3]>,
    /// Authored linear diffuse reflectivity for all material-bearing faces.
    pub diffuse: [f64; 3],
    /// Linear opacity; one is fully opaque.
    pub opacity: f64,
    /// Optional decoded diffuse image and coordinate transform.
    pub map: Option<DiffuseMap>,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            color: None,
            diffuse: [1.0; 3],
            opacity: 1.0,
            map: None,
        }
    }
}

/// Diffuse image sampling and a transform in authored bottom-left OBJ coordinates.
#[derive(Clone, Debug)]
pub(super) struct DiffuseMap {
    /// Shared immutable image and sampler settings.
    pub texture: Texture,
    /// Horizontal and vertical scale, applied before offset.
    pub scale: [f64; 2],
    /// Horizontal and vertical texture-coordinate offset.
    pub offset: [f64; 2],
    /// Whether any decoded texel contains authored transparency.
    pub has_alpha: bool,
}

/// One decoded image and its once-computed coverage metadata.
struct CachedImage {
    /// Immutable source texels shared by all materials using this path.
    image: Arc<RgbaImage>,
    /// Whether any source texel contains nonopaque alpha.
    has_alpha: bool,
}

impl DiffuseMap {
    /// Transform OBJ coordinates and convert their bottom-left origin once.
    ///
    /// # Errors
    /// Returns an error if finite input values overflow during the transform.
    pub fn coordinates(&self, uv: [f64; 2]) -> Result<[f64; 2]> {
        let transformed = [
            uv[0] * self.scale[0] + self.offset[0],
            1.0 - (uv[1] * self.scale[1] + self.offset[1]),
        ];
        if transformed.iter().any(|value| !value.is_finite()) {
            return Err("OBJ transformed texture coordinate exceeds numeric range".into());
        }
        Ok(transformed)
    }
}

/// Shared resource caches and budgets, including the primary OBJ's encoded bytes.
pub(super) struct Materials {
    /// Canonical overall OBJ directory; every material and image must remain within it.
    root: PathBuf,
    /// Currently defined diffuse materials.
    pub definitions: HashMap<String, Material>,
    /// Canonical MTL paths already parsed.
    libraries: std::collections::HashSet<PathBuf>,
    /// Canonical image paths already decoded.
    images: HashMap<PathBuf, CachedImage>,
    /// Primary text, encoded sidecar bytes and any owned legacy MTL text copies.
    bytes: u64,
    /// Total unique decoded texture pixels.
    pixels: u64,
    /// Resource statements, bounded even if they repeat missing files.
    references: usize,
    /// Retained material and resource-name storage budget.
    names: usize,
}

impl Materials {
    /// Initialize empty caches after counting primary encoded input bytes.
    ///
    /// # Errors
    /// Returns an error when the overall OBJ directory cannot be canonicalized.
    pub fn new(primary_bytes: usize, directory: &Path) -> Result<Self> {
        Ok(Self {
            root: directory.canonicalize()?,
            definitions: HashMap::new(),
            libraries: std::collections::HashSet::new(),
            images: HashMap::new(),
            bytes: primary_bytes as u64,
            pixels: 0,
            references: 0,
            names: 0,
        })
    }

    /// Account for a resource reference before resolving its path or allocating its name.
    fn reference(&mut self, name: &str) -> Result<()> {
        if name.is_empty() || name.len() > MAX_NAME_BYTES || self.references >= MAX_REFERENCES {
            return Err("OBJ material resources exceed reference or name limits".into());
        }
        self.references += 1;
        Ok(())
    }

    /// Resolve local material paths against their containing directory within the fixed OBJ root.
    /// Parent segments may reach sibling texture folders but cannot leave the overall model root.
    fn resolve(&self, directory: &Path, name: &str) -> Result<Option<PathBuf>> {
        if name.is_empty() || name.len() > MAX_NAME_BYTES {
            return Err("OBJ material resource filename exceeds limits".into());
        }
        // Wavefront names are filesystem names. A literal percent character,
        // including one followed by hexadecimal digits, does not imply URI syntax.
        if let Some(path) = self.local_path(directory, &name.replace('\\', "/"))? {
            return Ok(Some(path));
        }
        if !name.contains('%') {
            return Ok(None);
        }
        let mut decoded = Vec::with_capacity(name.len());
        let bytes = name.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' {
                let Some(digits) = bytes.get(index + 1..index + 3) else {
                    return Ok(None);
                };
                let Ok(digits) = std::str::from_utf8(digits) else {
                    return Ok(None);
                };
                let Ok(value) = u8::from_str_radix(digits, 16) else {
                    return Ok(None);
                };
                decoded.push(value);
                index += 3;
            } else {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
        let Ok(decoded) = String::from_utf8(decoded) else {
            return Ok(None);
        };
        self.local_path(directory, &decoded.replace('\\', "/"))
    }

    /// Canonicalize a normalized local filename after lexical and filesystem containment checks.
    fn local_path(&self, directory: &Path, normalized: &str) -> Result<Option<PathBuf>> {
        if normalized.contains([':', '\0', '?', '#']) {
            return Err("OBJ material resource must be a relative local path".into());
        }
        let directory = directory.canonicalize()?;
        let mut relative = directory
            .strip_prefix(&self.root)
            .map_err(|_| "OBJ material resource directory lies outside the model root")?
            .to_path_buf();
        for component in Path::new(normalized).components() {
            match component {
                std::path::Component::Normal(name) => relative.push(name),
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir if relative.pop() => {}
                _ => {
                    return Err("OBJ material resource must stay within the model directory".into());
                }
            }
        }
        let path = match self.root.join(relative).canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| "OBJ material resource resolves outside the model directory")?;
        // The shared resolver validates the final canonical relative path too.
        // Escape literal percent characters to avoid decoding the same filename twice.
        let uri = relative
            .to_str()
            .ok_or("OBJ material resource path is not UTF-8")?
            .replace('%', "%25")
            .replace('\\', "/");
        Ok(Some(super::relative_resource(&self.root, &uri)?))
    }

    /// Read a local resource within the remaining aggregate encoded-byte budget.
    /// Missing resources are optional; malformed or oversized existing resources are errors.
    fn read(&mut self, directory: &Path, name: &str) -> Result<Option<(PathBuf, Vec<u8>)>> {
        self.reference(name)?;
        let Some(path) = self.resolve(directory, name)? else {
            return Ok(None);
        };
        let remaining = super::MAX_INPUT_BYTES
            .checked_sub(self.bytes)
            .ok_or("OBJ encoded resource budget exceeded")?;
        let mut file = std::fs::File::open(&path)?;
        let length = file.metadata()?.len();
        if length > remaining {
            return Err("OBJ and material resources exceed the aggregate 512 MiB limit".into());
        }
        // Allocate the metadata-qualified size once rather than allowing read_to_end
        // to grow spare capacity beyond the remaining aggregate input budget.
        let mut bytes = vec![0; usize::try_from(length)?];
        file.read_exact(&mut bytes)?;
        let mut extra = [0];
        if file.read(&mut extra)? != 0 {
            return Err("OBJ material resource changed size while being read".into());
        }
        self.bytes += bytes.len() as u64;
        Ok(Some((path, bytes)))
    }

    /// Load one optional MTL file, checking all authored diffuse-map options.
    ///
    /// # Errors
    /// Existing malformed MTLs, unsupported diffuse options or exceeded budgets fail.
    pub fn library(&mut self, directory: &Path, name: &str) -> Result<()> {
        if name.is_empty() || name.len() > MAX_NAME_BYTES {
            return Err("OBJ material filename exceeds limits".into());
        }
        // A repeated library does not consume its data budget again.
        if let Some(path) = self.resolve(directory, name)?
            && self.libraries.contains(&path)
        {
            return self.reference(name);
        }
        let Some((path, bytes)) = self.read(directory, name)? else {
            return Ok(());
        };
        self.libraries.insert(path.clone());
        let source = super::mesh::wavefront_text(&bytes, &mut self.bytes)?;
        let parent = path
            .parent()
            .ok_or("OBJ material library has no parent directory")?;
        let mut current = None;
        logical_lines(&source, |number, line| {
            self.material_line(line, parent, &mut current)
                .map_err(|error| format!("MTL line {number}: {error}").into())
        })
    }

    /// Decode a material directive while retaining unsupported shading as diffuse previews.
    fn material_line(
        &mut self,
        line: &str,
        directory: &Path,
        current: &mut Option<String>,
    ) -> Result<()> {
        let words = words(line, 64)?;
        let Some(directive) = words.first().copied() else {
            return Ok(());
        };
        if directive == "newmtl" {
            if words.len() != 2
                || words[1].len() > MAX_NAME_BYTES
                || self.definitions.len() >= MAX_MATERIALS
            {
                return Err("OBJ material name or definition count exceeds limits".into());
            }
            self.names = self
                .names
                .checked_add(words[1].len() + std::mem::size_of::<Material>())
                .ok_or("OBJ material storage overflow")?;
            if self.names > 16 * 1024 * 1024 {
                return Err("OBJ material definitions exceed the storage budget".into());
            }
            *current = Some(words[1].to_owned());
            self.definitions
                .insert(words[1].to_owned(), Material::default());
            return Ok(());
        }
        let Some(name) = current.as_ref() else {
            return Ok(());
        };
        match directive {
            "Kd" => {
                let diffuse = diffuse(&words[1..])?;
                let material = self
                    .definitions
                    .get_mut(name)
                    .ok_or("OBJ current material is absent")?;
                material.diffuse = diffuse;
                material.color = Some(bytes_rgb(diffuse));
            }
            "d" | "Tr" => {
                if words.len() != 2 {
                    return Err(
                        "OBJ dissolve requires a single scalar; halo dissolve is unsupported"
                            .into(),
                    );
                }
                let value = scalar(words[1])?;
                if !(0.0..=1.0).contains(&value) {
                    return Err("OBJ dissolve must be between zero and one".into());
                }
                self.definitions
                    .get_mut(name)
                    .ok_or("OBJ current material is absent")?
                    .opacity = if directive == "Tr" {
                    1.0 - value
                } else {
                    value
                };
            }
            "map_Kd" => {
                let options = map_options(&words[1..])?;
                let texture = self.image(directory, &options.name)?;
                self.definitions
                    .get_mut(name)
                    .ok_or("OBJ current material is absent")?
                    .map = texture.map(|(image, has_alpha)| DiffuseMap {
                    texture: Texture {
                        image,
                        wrap_s: options.wrap,
                        wrap_t: options.wrap,
                        filter: TextureFilter::Linear,
                        min_filter: TextureFilter::Linear,
                        interpolation_space: TextureInterpolationSpace::Linear,
                    },
                    scale: options.scale,
                    offset: options.offset,
                    has_alpha,
                });
            }
            _ => {}
        }
        Ok(())
    }

    /// Decode one image once, bounding encoded input, dimensions and all retained pixels.
    fn image(&mut self, directory: &Path, name: &str) -> Result<Option<(Arc<RgbaImage>, bool)>> {
        if name.is_empty() || name.len() > MAX_NAME_BYTES {
            return Err("OBJ image filename exceeds limits".into());
        }
        if let Some(path) = self.resolve(directory, name)?
            && let Some(image) = self.images.get(&path)
        {
            let result = (Arc::clone(&image.image), image.has_alpha);
            self.reference(name)?;
            return Ok(Some(result));
        }
        let Some((path, bytes)) = self.read(directory, name)? else {
            return Ok(None);
        };
        // TGA has no unique magic signature; only its explicit filename extension
        // selects that already-enabled Rust decoder when signature detection fails.
        let format = image::guess_format(&bytes).or_else(|error| {
            if ImageFormat::from_path(&path).ok() == Some(ImageFormat::Tga) {
                Ok(ImageFormat::Tga)
            } else {
                Err(error)
            }
        })?;
        let (width, height) = image_reader(&bytes, format).into_dimensions()?;
        let pixels = u64::from(width) * u64::from(height);
        let total = self
            .pixels
            .checked_add(pixels)
            .ok_or("OBJ texture pixel count overflow")?;
        if width == 0
            || height == 0
            || width > MAX_TEXTURE_AXIS
            || height > MAX_TEXTURE_AXIS
            || pixels > MAX_TEXTURE_PIXELS
            || total > MAX_TOTAL_TEXTURE_PIXELS
        {
            return Err("OBJ images exceed texture dimensions or aggregate pixel limits".into());
        }
        self.pixels = total;
        let image = Arc::new(image_reader(&bytes, format).decode()?.to_rgba8());
        let has_alpha = image.pixels().any(|pixel| pixel[3] != 255);
        self.images.insert(
            path,
            CachedImage {
                image: Arc::clone(&image),
                has_alpha,
            },
        );
        Ok(Some((image, has_alpha)))
    }
}

/// Apply decoder allocation limits during both metadata inspection and pixel decoding.
fn image_reader(bytes: &[u8], format: ImageFormat) -> image::ImageReader<Cursor<&[u8]>> {
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_TEXTURE_AXIS);
    limits.max_image_height = Some(MAX_TEXTURE_AXIS);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    reader
}

/// Parse a finite authored material scalar.
fn scalar(word: &str) -> Result<f64> {
    let value: f64 = word.parse()?;
    if !value.is_finite() {
        return Err("OBJ material value must be finite".into());
    }
    Ok(value)
}

/// Decode RGB reflectivity, including the original format's single-channel shorthand.
fn diffuse(words: &[&str]) -> Result<[f64; 3]> {
    let values = match words {
        [r] => [scalar(r)?; 3],
        [r, g, b] => [scalar(r)?, scalar(g)?, scalar(b)?],
        _ => return Err("OBJ diffuse reflectivity requires one or three RGB values".into()),
    };
    if values.iter().any(|value| !(0.0..=1.0).contains(value)) {
        return Err("OBJ diffuse reflectivity outside zero to one is unsupported".into());
    }
    Ok(values)
}

/// Encode linear diffuse reflectivity as sRGB bytes for point, line and triangle metadata.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Validated normalized RGB channels are rounded and remain within the byte range."
)]
fn bytes_rgb(color: [f64; 3]) -> [u8; 3] {
    color.map(|value| {
        let encoded = if value <= 0.003_130_8 {
            value * 12.92
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        };
        (encoded.clamp(0.0, 1.0) * 255.0).round() as u8
    })
}

/// Validated two-dimensional diffuse-map addressing options.
struct MapOptions {
    /// Local image filename, potentially containing spaces.
    name: String,
    /// Authored two-dimensional scale.
    scale: [f64; 2],
    /// Authored two-dimensional offset.
    offset: [f64; 2],
    /// Repeat or leave the underlying material outside the original image.
    wrap: TextureWrap,
}

/// Parse ordinary map transforms and reject options whose effect cannot be represented.
#[allow(
    clippy::float_cmp,
    reason = "Only exact identity range modifiers have no effect; approximate values must not silently change authored semantics."
)]
fn map_options(words: &[&str]) -> Result<MapOptions> {
    let mut result = MapOptions {
        name: String::new(),
        scale: [1.0; 2],
        offset: [0.0; 2],
        wrap: TextureWrap::Repeat,
    };
    let mut index = 0;
    while words.get(index).is_some_and(|word| word.starts_with('-')) {
        let option = words[index];
        index += 1;
        match option {
            "-o" | "-s" | "-t" => {
                let mut values = if option == "-s" { [1.0; 3] } else { [0.0; 3] };
                let mut count = 0;
                for value in &mut values {
                    let Some(word) = words.get(index) else { break };
                    let Ok(number) = word.parse::<f64>() else {
                        break;
                    };
                    if !number.is_finite() {
                        return Err("OBJ map transform must be finite".into());
                    }
                    *value = number;
                    index += 1;
                    count += 1;
                }
                if count == 0 {
                    return Err("OBJ map transform requires at least one scalar".into());
                }
                match option {
                    "-o" => result.offset = [values[0], values[1]],
                    "-s" => result.scale = [values[0], values[1]],
                    _ if values.iter().all(|value| *value == 0.0) => {}
                    _ => return Err("OBJ diffuse-map turbulence is unsupported".into()),
                }
            }
            "-clamp" => {
                result.wrap = match words.get(index).copied() {
                    Some("off") => TextureWrap::Repeat,
                    Some("on") => TextureWrap::ClampToBorderWhite,
                    _ => return Err("OBJ map clamp requires on or off".into()),
                };
                index += 1;
            }
            "-mm" => {
                let base = scalar(
                    words
                        .get(index)
                        .ok_or("OBJ map range requires base and gain")?,
                )?;
                let gain = scalar(
                    words
                        .get(index + 1)
                        .ok_or("OBJ map range requires base and gain")?,
                )?;
                if base != 0.0 || gain != 1.0 {
                    return Err(
                        "OBJ nonidentity diffuse-map range modifiers are unsupported".into(),
                    );
                }
                index += 2;
            }
            "-blendu" | "-blendv" | "-cc" => {
                let expected = if option == "-cc" { "off" } else { "on" };
                if words.get(index).copied() != Some(expected) {
                    return Err(format!("OBJ diffuse-map option {option} requires unsupported sampling or color correction").into());
                }
                index += 1;
            }
            _ => return Err(format!("unsupported OBJ diffuse-map option: {option}").into()),
        }
    }
    if index >= words.len() {
        return Err("OBJ diffuse map has no image filename".into());
    }
    let length = words[index..]
        .iter()
        .map(|word| word.len() + 1)
        .sum::<usize>();
    if length > MAX_NAME_BYTES {
        return Err("OBJ diffuse map filename exceeds limits".into());
    }
    result.name = words[index..].join(" ");
    Ok(result)
}

/// Tokenize one bounded directive without allowing a corrupt face or material line to allocate unbounded tokens.
pub(super) fn words(line: &str, limit: usize) -> Result<Vec<&str>> {
    if line.len() > MAX_LINE_BYTES {
        return Err("OBJ or MTL line exceeds the 4 MiB limit".into());
    }
    let mut result = Vec::new();
    for word in line.split('#').next().unwrap_or("").split_whitespace() {
        if result.len() == limit {
            return Err("OBJ or MTL directive has too many fields".into());
        }
        result.push(word);
    }
    Ok(result)
}

/// Visit physical or backslash-continued directives while bounding the continuation buffer.
pub(super) fn logical_lines(
    source: &str,
    mut visit: impl FnMut(usize, &str) -> Result<()>,
) -> Result<()> {
    let mut continued = String::new();
    let mut start = 1;
    for (index, line) in source.lines().enumerate() {
        if line.len().saturating_add(continued.len()) > MAX_LINE_BYTES {
            return Err("OBJ or MTL logical line exceeds the 4 MiB limit".into());
        }
        if let Some(line) = line.strip_suffix('\\') {
            if continued.is_empty() {
                start = index + 1;
            }
            continued.push_str(line);
            continued.push(' ');
        } else if continued.is_empty() {
            visit(index + 1, line)?;
        } else {
            continued.push_str(line);
            visit(start, &continued)?;
            continued.clear();
        }
    }
    if !continued.is_empty() {
        return Err("OBJ or MTL ends with an incomplete continued line".into());
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "Exact format defaults, authored fractions and identity options are directly compared in these controls."
)]
/// Bounded resource and material-option controls using process-unique local files.
mod tests {
    use super::*;

    /// Temporary resource directory, removed when a control completes.
    struct Directory(PathBuf);

    impl Directory {
        /// Create a process-unique directory without sharing mutable fixture names.
        fn new() -> Self {
            /// Next directory suffix allocated within this test process.
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "catwalk-obj-material-{}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        /// Write a tiny asymmetric PNG used to prove image orientation and sharing.
        fn texture(&self) {
            let image = RgbaImage::from_fn(2, 2, |x, y| {
                image::Rgba([
                    if x == 0 { 255 } else { 0 },
                    if y == 0 { 255 } else { 0 },
                    0,
                    255,
                ])
            });
            image.save(self.0.join("texture.png")).unwrap();
        }
    }

    impl Drop for Directory {
        /// Clean up only the temporary directory owned by this test.
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// OBJ and MTL share exact legacy names without collapsing distinct undefined bytes.
    #[test]
    fn legacy_material_names_match_obj_faces_without_substitution() {
        let directory = Directory::new();
        let mtl = b"newmtl Terraind\xe6k\nKd .25 .5 .75\nnewmtl distinct\x81\nKd 1 0 0\nnewmtl distinct\x8d\nKd 0 1 0\n";
        std::fs::write(directory.0.join("legacy.mtl"), mtl).unwrap();
        let mut materials = Materials::new(0, &directory.0).unwrap();
        materials.library(&directory.0, "legacy.mtl").unwrap();
        assert_eq!(materials.definitions.len(), 3);
        assert_eq!(
            materials.definitions["Terraindæk"].diffuse,
            [0.25, 0.5, 0.75]
        );
        assert_eq!(
            materials.definitions["distinct\u{81}"].diffuse,
            [1.0, 0.0, 0.0]
        );
        assert_eq!(
            materials.definitions["distinct\u{8d}"].diffuse,
            [0.0, 1.0, 0.0]
        );
        assert!(materials.bytes > u64::try_from(mtl.len()).unwrap());
        let obj = b"mtllib legacy.mtl\nv 0 0 0\nv 2 0 0\nv 0 1 0\nusemtl Terraind\xe6k\nf 1 2 3\nusemtl distinct\x81\nf 1 2 3\nusemtl distinct\x8d\nf 1 2 3\n";
        let path = directory.0.join("legacy.obj");
        std::fs::write(&path, obj).unwrap();
        let scene = super::super::mesh::load(&path).unwrap();
        assert_eq!(scene.triangles.len(), 3);
        assert_eq!(scene.appearances[&0].base_color, [0.25, 0.5, 0.75, 1.0]);
        assert_eq!(scene.appearances[&1].base_color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(scene.appearances[&2].base_color, [0.0, 1.0, 0.0, 1.0]);
    }

    /// Legacy text cannot consume uncharged sidecar storage or bypass decoded name limits.
    #[test]
    fn legacy_material_expansion_preserves_byte_and_name_limits() {
        let directory = Directory::new();
        let mtl = b"newmtl legacy\xe6\nKd 1 0 0\n";
        std::fs::write(directory.0.join("legacy.mtl"), mtl).unwrap();
        let mut materials = Materials::new(0, &directory.0).unwrap();
        materials.bytes = super::super::MAX_INPUT_BYTES - u64::try_from(mtl.len()).unwrap();
        assert!(materials.library(&directory.0, "legacy.mtl").is_err());
        assert!(materials.definitions.is_empty());
        assert_eq!(materials.bytes, super::super::MAX_INPUT_BYTES);
        let mut mtl = b"newmtl a".to_vec();
        mtl.extend(std::iter::repeat_n(0xe6, MAX_NAME_BYTES / 2));
        mtl.extend_from_slice(b"\nKd 1 0 0\n");
        std::fs::write(directory.0.join("long.mtl"), mtl).unwrap();
        let mut materials = Materials::new(0, &directory.0).unwrap();
        assert!(materials.library(&directory.0, "long.mtl").is_err());
        assert!(materials.definitions.is_empty());
    }

    /// Transform, addressing and identity modifier options retain their authored values.
    #[test]
    fn diffuse_options_transform_and_repeat_defaults_are_exact() {
        let options = map_options(&[
            "-s",
            "2",
            ".5",
            "1",
            "-o",
            "-.25",
            ".125",
            "0",
            "-mm",
            "0",
            "1",
            "-t",
            "0",
            "0",
            "0",
            "-cc",
            "off",
            "-blendu",
            "on",
            "-blendv",
            "on",
            "-clamp",
            "on",
            "texture.png",
        ])
        .unwrap();
        assert_eq!(options.scale, [2.0, 0.5]);
        assert_eq!(options.offset, [-0.25, 0.125]);
        assert_eq!(options.wrap, TextureWrap::ClampToBorderWhite);
        let defaults = map_options(&["texture", "with", "spaces.png"]).unwrap();
        assert_eq!(defaults.name, "texture with spaces.png");
        assert_eq!(defaults.scale, [1.0; 2]);
        assert_eq!(defaults.offset, [0.0; 2]);
        assert_eq!(defaults.wrap, TextureWrap::Repeat);
    }

    /// Unsupported required options and nonfinite transforms cannot be silently dropped.
    #[test]
    fn unsupported_or_nonfinite_map_options_fail() {
        for line in [
            "-mm .1 1 texture.png",
            "-mm 0 2 texture.png",
            "-t 0 .2 0 texture.png",
            "-cc on texture.png",
            "-blendu off texture.png",
            "-blendv off texture.png",
            "-imfchan r texture.png",
            "-texres 32 texture.png",
            "-unknown texture.png",
            "-o NaN texture.png",
            "-s inf texture.png",
            "-s texture.png",
            "-clamp maybe texture.png",
            "-o 1",
            "-mm 0",
        ] {
            assert!(
                map_options(&line.split_whitespace().collect::<Vec<_>>()).is_err(),
                "{line}"
            );
        }
    }

    /// MTL images are decoded once and their rows retain the image codec's orientation.
    #[test]
    fn local_image_cache_preserves_pixels_windows_paths_and_obj_v_conversion() {
        let directory = Directory::new();
        directory.texture();
        std::fs::write(directory.0.join("paint.mtl"), "newmtl a\nKd .5\nd .25\nmap_Kd -s 2 .5 -o -.25 .125 .\\texture.png\nnewmtl b\nTr .75\nmap_Kd texture.png\n").unwrap();
        let mut materials = Materials::new(0, &directory.0).unwrap();
        materials.library(&directory.0, "paint.mtl").unwrap();
        let a = &materials.definitions["a"];
        let b = &materials.definitions["b"];
        assert_eq!(a.diffuse, [0.5; 3]);
        assert_eq!(a.opacity, 0.25);
        assert_eq!(b.opacity, 0.25);
        let map = a.map.as_ref().unwrap();
        assert_eq!(map.coordinates([0.25, 0.5]).unwrap(), [0.25, 0.625]);
        assert_eq!(map.texture.image.get_pixel(0, 0).0, [255, 255, 0, 255]);
        assert_eq!(map.texture.image.get_pixel(1, 1).0, [0, 0, 0, 255]);
        assert!(Arc::ptr_eq(
            &map.texture.image,
            &b.map.as_ref().unwrap().texture.image
        ));
        assert_eq!(materials.images.len(), 1);
        assert_eq!(materials.pixels, 4);
        let bytes = materials.bytes;
        materials.library(&directory.0, "paint.mtl").unwrap();
        assert_eq!(materials.bytes, bytes);
    }

    /// Material diffuse reflectivity has one meaning across triangles, lines and points.
    #[test]
    fn linear_diffuse_is_encoded_consistently_for_all_primitive_types() {
        let directory = Directory::new();
        std::fs::write(directory.0.join("paint.mtl"), "newmtl gray\nKd .5\n").unwrap();
        let scene = super::super::obj::parse(
            "mtllib paint.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl gray\nf 1 2 3\nl 1 2\np 3\n",
            &directory.0,
        )
        .unwrap();
        assert_eq!(scene.appearances[&0].base_color, [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(scene.triangles[0].color, Some([188; 3]));
        assert_eq!(scene.lines[0].color, Some([188; 3]));
        assert_eq!(scene.points[0].color, Some([188; 3]));
    }

    /// Nonopaque source-image alpha is discovered once and reused by later material maps.
    #[test]
    fn cached_image_alpha_is_retained_without_repeated_pixel_scans() {
        let directory = Directory::new();
        let image = RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 127]));
        image.save(directory.0.join("alpha.png")).unwrap();
        let mut materials = Materials::new(0, &directory.0).unwrap();
        let first = materials.image(&directory.0, "alpha.png").unwrap().unwrap();
        let second = materials.image(&directory.0, "alpha.png").unwrap().unwrap();
        assert!(first.1 && second.1);
        assert!(Arc::ptr_eq(&first.0, &second.0));
        assert_eq!(materials.pixels, 1);
    }

    /// Absent optional files retain material definitions and untextured geometry.
    #[test]
    fn missing_optional_resources_are_tolerated_but_existing_corruption_fails() {
        let directory = Directory::new();
        let mut materials = Materials::new(0, &directory.0).unwrap();
        materials.library(&directory.0, "missing.mtl").unwrap();
        std::fs::write(
            directory.0.join("paint.mtl"),
            "newmtl a\nKd 1 0 0\nmap_Kd missing.png\n",
        )
        .unwrap();
        materials.library(&directory.0, "paint.mtl").unwrap();
        assert!(materials.definitions["a"].map.is_none());
        assert_eq!(materials.definitions["a"].diffuse, [1.0, 0.0, 0.0]);
        std::fs::write(
            directory.0.join("broken.mtl"),
            "newmtl broken\nKd NaN 0 0\n",
        )
        .unwrap();
        assert!(materials.library(&directory.0, "broken.mtl").is_err());
        std::fs::write(directory.0.join("broken.png"), b"not an image").unwrap();
        assert!(materials.image(&directory.0, "broken.png").is_err());
    }

    /// Relative resource normalization cannot escape the MTL directory or follow an external symlink.
    #[test]
    fn local_texture_paths_cannot_escape_their_resource_root() {
        let directory = Directory::new();
        directory.texture();
        let model = directory.0.join("model");
        std::fs::create_dir(&model).unwrap();
        let mut materials = Materials::new(0, &model).unwrap();
        for path in [
            "../texture.png",
            "..\\texture.png",
            "%2e%2e/texture.png",
            "https://example.invalid/texture.png",
        ] {
            assert!(materials.image(&model, path).is_err(), "{path}");
        }
        assert!(
            materials
                .image(&model, directory.0.join("texture.png").to_str().unwrap())
                .is_err()
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(directory.0.join("texture.png"), model.join("escaped.png"))
                .unwrap();
            assert!(materials.image(&model, "escaped.png").is_err());
        }
        assert_eq!(materials.bytes, 0);
        assert!(materials.images.is_empty());
    }

    /// Nested material libraries may reference sibling texture directories inside the OBJ root.
    #[test]
    fn nested_materials_resolve_parent_segments_within_the_overall_model_root() {
        let directory = Directory::new();
        directory.texture();
        std::fs::create_dir(directory.0.join("materials")).unwrap();
        std::fs::create_dir(directory.0.join("textures")).unwrap();
        std::fs::rename(
            directory.0.join("texture.png"),
            directory.0.join("textures/diffuse.png"),
        )
        .unwrap();
        std::fs::write(directory.0.join("materials/paint.mtl"), "newmtl a\nmap_Kd ../textures/diffuse.png\nnewmtl b\nmap_Kd ..\\textures\\diffuse.png\n").unwrap();
        let mut materials = Materials::new(0, &directory.0).unwrap();
        materials
            .library(&directory.0, "materials/paint.mtl")
            .unwrap();
        let a = &materials.definitions["a"]
            .map
            .as_ref()
            .unwrap()
            .texture
            .image;
        let b = &materials.definitions["b"]
            .map
            .as_ref()
            .unwrap()
            .texture
            .image;
        assert!(Arc::ptr_eq(a, b));
        assert_eq!(a.get_pixel(0, 0).0, [255, 255, 0, 255]);
        assert_eq!(materials.images.len(), 1);
        assert_eq!(materials.pixels, 4);
        // A missing sibling image is still optional, even with a parent segment.
        assert!(
            materials
                .image(&directory.0.join("materials"), "../textures/missing.png")
                .unwrap()
                .is_none()
        );
        assert!(
            materials
                .image(&directory.0.join("materials"), "../../outside.png")
                .is_err()
        );
    }

    /// Literal percent signs and spaces take priority over optional URI-encoded compatibility names.
    #[test]
    fn authored_texture_filenames_preserve_literal_percent_characters_and_spaces() {
        let directory = Directory::new();
        directory.texture();
        std::fs::copy(
            directory.0.join("texture.png"),
            directory.0.join("100% texture.png"),
        )
        .unwrap();
        std::fs::copy(
            directory.0.join("texture.png"),
            directory.0.join("tile%20literal.png"),
        )
        .unwrap();
        RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 255, 255]))
            .save(directory.0.join("tile literal.png"))
            .unwrap();
        std::fs::write(
            directory.0.join("literal.mtl"),
            "newmtl a\nmap_Kd 100% texture.png\nnewmtl b\nmap_Kd tile%20literal.png\n",
        )
        .unwrap();
        let mut materials = Materials::new(0, &directory.0).unwrap();
        materials.library(&directory.0, "literal.mtl").unwrap();
        let first = &materials.definitions["a"]
            .map
            .as_ref()
            .unwrap()
            .texture
            .image;
        let second = &materials.definitions["b"]
            .map
            .as_ref()
            .unwrap()
            .texture
            .image;
        assert_eq!(first.dimensions(), (2, 2));
        assert_eq!(second.dimensions(), (2, 2));
        // Missing malformed literal filenames stay optional; valid percent escapes
        // can load an encoded compatibility name if the exact literal is absent.
        assert!(
            materials
                .image(&directory.0, "missing%texture.png")
                .unwrap()
                .is_none()
        );
        assert!(
            materials
                .image(&directory.0, "missing%.png")
                .unwrap()
                .is_none()
        );
        std::fs::copy(
            directory.0.join("texture.png"),
            directory.0.join("encoded only.png"),
        )
        .unwrap();
        assert_eq!(
            materials
                .image(&directory.0, "encoded%20only.png")
                .unwrap()
                .unwrap()
                .0
                .dimensions(),
            (2, 2)
        );
    }

    /// Metadata and cumulative-byte limits reject oversized images before decoding pixels.
    #[test]
    fn texture_dimension_encoded_byte_and_reference_budgets_are_enforced() {
        let directory = Directory::new();
        directory.texture();
        let mut materials = Materials::new(0, &directory.0).unwrap();
        materials.bytes = super::super::MAX_INPUT_BYTES - 1;
        assert!(materials.image(&directory.0, "texture.png").is_err());
        assert!(materials.images.is_empty());
        let image = RgbaImage::from_pixel(MAX_TEXTURE_AXIS + 1, 1, image::Rgba([255; 4]));
        image.save(directory.0.join("wide.png")).unwrap();
        let mut materials = Materials::new(0, &directory.0).unwrap();
        assert!(materials.image(&directory.0, "wide.png").is_err());
        assert!(materials.images.is_empty());
        materials.references = MAX_REFERENCES;
        assert!(materials.library(&directory.0, "missing.mtl").is_err());
        assert!(
            materials
                .library(&directory.0, &"a".repeat(MAX_NAME_BYTES + 1))
                .is_err()
        );
        let mut materials = Materials::new(0, &directory.0).unwrap();
        materials.pixels = MAX_TOTAL_TEXTURE_PIXELS;
        assert!(materials.image(&directory.0, "texture.png").is_err());
        assert!(materials.images.is_empty());
    }

    /// Texture-coordinate transforms reject overflow rather than hiding invalid authored UVs.
    #[test]
    fn transformed_coordinate_overflow_is_rejected() {
        let map = DiffuseMap {
            texture: Texture {
                image: Arc::new(RgbaImage::from_pixel(1, 1, image::Rgba([255; 4]))),
                wrap_s: TextureWrap::Repeat,
                wrap_t: TextureWrap::Repeat,
                filter: TextureFilter::Linear,
                min_filter: TextureFilter::Linear,
                interpolation_space: TextureInterpolationSpace::Linear,
            },
            scale: [2.0, 1.0],
            offset: [0.0; 2],
            has_alpha: false,
        };
        assert!(map.coordinates([f64::MAX, 0.0]).is_err());
    }

    /// Continued logical lines are joined without allowing an unbounded buffer or unterminated record.
    #[test]
    fn logical_directives_bound_tokens_and_continuations() {
        let mut lines = Vec::new();
        logical_lines("v 0 0 \\\r\n1\n# comment\n", |number, line| {
            lines.push((number, line.to_owned()));
            Ok(())
        })
        .unwrap();
        assert_eq!(lines[0], (1, "v 0 0  1".to_owned()));
        assert!(logical_lines("v 0 0 \\", |_, _| Ok(())).is_err());
        assert!(words("f 1 2 3 4", 4).is_err());
        assert!(words(&"a".repeat(MAX_LINE_BYTES + 1), 1).is_err());
    }
}
