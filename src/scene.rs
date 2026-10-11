//! Z-up geometry and linear-color appearances used by the software renderer.

use image::RgbaImage;
use std::collections::{BTreeMap, HashSet};
use std::ops::{Add, Div, Mul, Neg, Sub};
use std::sync::Arc;

/// A three-component position or direction in double precision.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[must_use]
pub struct Vec3 {
    /// Component along the X axis.
    pub x: f64,
    /// Component along the Y axis.
    pub y: f64,
    /// Component along the vertical Z axis.
    pub z: f64,
}

impl Vec3 {
    /// Construct a vector without changing or validating its components.
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
    /// Return the scalar dot product with another vector.
    #[must_use]
    pub fn dot(self, b: Self) -> f64 {
        self.x * b.x + self.y * b.y + self.z * b.z
    }
    /// Return the right-handed cross product with another vector.
    pub fn cross(self, b: Self) -> Self {
        Self::new(
            self.y * b.z - self.z * b.y,
            self.z * b.x - self.x * b.z,
            self.x * b.y - self.y * b.x,
        )
    }
    /// Return the Euclidean length using overflow-resistant hypotenuse operations.
    #[must_use]
    pub fn length(self) -> f64 {
        self.x.hypot(self.y).hypot(self.z)
    }
    /// Return a unit vector, or the zero vector for zero or non-finite length.
    pub fn normalized(self) -> Self {
        let length = self.length();
        if length > 0.0 && length.is_finite() {
            self / length
        } else {
            Self::default()
        }
    }
    /// Return whether all three components are finite floating-point values.
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self::new(self.x + b.x, self.y + b.y, self.z + b.z)
    }
}
impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        Self::new(self.x - b.x, self.y - b.y, self.z - b.z)
    }
}
impl Mul<f64> for Vec3 {
    type Output = Self;
    fn mul(self, s: f64) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}
impl Div<f64> for Vec3 {
    type Output = Self;
    fn div(self, s: f64) -> Self {
        Self::new(self.x / s, self.y / s, self.z / s)
    }
}
impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        self * -1.0
    }
}
impl From<[f32; 3]> for Vec3 {
    fn from(v: [f32; 3]) -> Self {
        Self::new(f64::from(v[0]), f64::from(v[1]), f64::from(v[2]))
    }
}

/// A triangle with an optional legacy sRGB flat color.
#[derive(Clone, Debug)]
pub struct Triangle {
    /// Its three corner positions in winding order.
    pub vertices: [Vec3; 3],
    /// An sRGB color; absent colors use the renderer's configured default.
    pub color: Option<[u8; 3]>,
}

/// A point rendered at a fixed screen-space size.
#[derive(Clone, Debug)]
pub struct Point {
    /// Point position in scene coordinates.
    pub position: Vec3,
    /// Optional sRGB color overriding the renderer's default.
    pub color: Option<[u8; 3]>,
}

/// A line segment with linearly interpolated depth.
#[derive(Clone, Debug)]
pub struct Line {
    /// Segment endpoints in scene coordinates.
    pub vertices: [Vec3; 2],
    /// Optional sRGB color overriding the renderer's default.
    pub color: Option<[u8; 3]>,
}

/// Handling of texture coordinates outside the image's unit square.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextureWrap {
    /// Repeat the image at every integer coordinate.
    #[default]
    Repeat,
    /// Extend the nearest edge texel beyond the image.
    ClampToEdge,
    /// Repeat the image with alternating mirrored intervals.
    MirroredRepeat,
}

/// Base-level texture reconstruction used for each sampled UV coordinate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextureFilter {
    /// Select the closest image texel.
    Nearest,
    /// Interpolate four neighboring texels in linear color.
    #[default]
    Linear,
}

/// An sRGB color texture. Alpha is linear; UV (0, 0) is the upper-left
/// image corner, following glTF's image origin convention.
#[derive(Clone, Debug)]
pub struct Texture {
    /// Shared source texels in straight-alpha, sRGB RGBA8 form.
    pub image: Arc<RgbaImage>,
    /// Wrapping along the horizontal U coordinate.
    pub wrap_s: TextureWrap,
    /// Wrapping along the vertical V coordinate.
    pub wrap_t: TextureWrap,
    /// Magnification filtering.
    pub filter: TextureFilter,
    /// Minification filtering, selected from projected UV derivatives. The
    /// renderer uses the base level rather than constructing mipmaps.
    pub min_filter: TextureFilter,
}

/// Interpretation of the multiplied material, vertex and texture alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum AlphaMode {
    /// Ignore alpha and write opaque color and depth.
    #[default]
    Opaque,
    /// Discard samples below a threshold; retained samples are fully opaque.
    Mask {
        /// Minimum retained alpha; finite nonnegative values above one discard everything.
        cutoff: f64,
    },
    /// Composite samples over their depth-sorted background in linear color.
    Blend,
}

/// Optional appearance for one triangle. Colors are linear RGBA multipliers;
/// normals are world-space directions. A texture requires three UV pairs.
/// Texture RGB is decoded from sRGB before filtering and multiplication.
#[derive(Clone, Debug)]
pub struct TriangleAppearance {
    /// Linear RGBA material multiplier, with components between zero and one.
    pub base_color: [f64; 4],
    /// Optional linear RGBA multipliers at the three triangle corners.
    pub colors: Option<[[f64; 4]; 3]>,
    /// Optional UV pairs at the three corners; (0, 0) is the image's upper-left.
    pub texcoords: Option<[[f64; 2]; 3]>,
    /// Optional finite, nonzero world-space normals at the three corners.
    pub normals: Option<[Vec3; 3]>,
    /// Optional base-color texture, requiring texture coordinates.
    pub texture: Option<Texture>,
    /// Coverage or blending applied after color and alpha multiplication.
    pub alpha_mode: AlphaMode,
    /// Whether to bypass lighting and display the multiplied base color directly.
    pub unlit: bool,
    /// General mesh importers retain visible back faces by default. Importers
    /// with material culling semantics can set this to false.
    pub double_sided: bool,
}

impl Default for TriangleAppearance {
    fn default() -> Self {
        Self {
            base_color: [1.0; 4],
            colors: None,
            texcoords: None,
            normals: None,
            texture: None,
            alpha_mode: AlphaMode::Opaque,
            unlit: false,
            double_sided: true,
        }
    }
}

/// Coordinates use Z up. Importers convert formats with other conventions.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    /// Surface triangles in a common coordinate system.
    pub triangles: Vec<Triangle>,
    /// Colored points in that coordinate system.
    pub points: Vec<Point>,
    /// Colored line segments in that coordinate system.
    pub lines: Vec<Line>,
    /// Triangle indices with appearance attributes. Missing entries retain the
    /// original flat color renderer, including its established output colors.
    pub appearances: BTreeMap<usize, TriangleAppearance>,
}

impl Scene {
    /// Check geometry, appearance references, numeric values and texture budgets.
    ///
    /// # Errors
    /// Returns an error for empty geometry, non-finite positions, missing triangle
    /// references, invalid colors/UVs/normals/cutoffs, missing textured UVs, or
    /// textures exceeding individual or aggregate dimension and pixel limits.
    pub fn validate(&self) -> crate::Result<()> {
        if self.triangles.is_empty() && self.points.is_empty() && self.lines.is_empty() {
            return Err("model contains no renderable geometry".into());
        }
        if self.positions().any(|p| !p.is_finite()) {
            return Err("model contains non-finite coordinates".into());
        }
        let mut textures = HashSet::new();
        let mut texture_pixels = 0_u64;
        for (index, appearance) in &self.appearances {
            if *index >= self.triangles.len() {
                return Err("triangle appearance refers to a missing triangle".into());
            }
            let valid_color = |color: &[f64; 4]| {
                color
                    .iter()
                    .all(|c| c.is_finite() && (0.0..=1.0).contains(c))
            };
            if !valid_color(&appearance.base_color)
                || appearance
                    .colors
                    .is_some_and(|colors| colors.iter().any(|c| !valid_color(c)))
            {
                return Err(
                    "triangle appearance colors must be finite values between zero and one".into(),
                );
            }
            if appearance
                .texcoords
                .is_some_and(|uvs| uvs.iter().flatten().any(|v| !v.is_finite()))
            {
                return Err("triangle texture coordinates must be finite".into());
            }
            if appearance.normals.is_some_and(|normals| {
                normals
                    .iter()
                    .any(|n| !n.is_finite() || !n.length().is_finite() || n.length() == 0.0)
            }) {
                return Err("triangle appearance normals must be finite nonzero directions".into());
            }
            if let AlphaMode::Mask { cutoff } = appearance.alpha_mode
                && (!cutoff.is_finite() || cutoff < 0.0)
            {
                return Err("triangle alpha cutoff must be finite and nonnegative".into());
            }
            if let Some(texture) = &appearance.texture {
                if appearance.texcoords.is_none() {
                    return Err("triangle color texture requires texture coordinates".into());
                }
                let width = texture.image.width();
                let height = texture.image.height();
                let pixels = u64::from(width) * u64::from(height);
                if width == 0
                    || height == 0
                    || width > 8192
                    || height > 8192
                    || pixels > 16 * 1024 * 1024
                {
                    return Err(
                        "triangle texture exceeds the dimension or 16 million pixel limit".into(),
                    );
                }
                if textures.insert(Arc::as_ptr(&texture.image)) {
                    texture_pixels += pixels;
                    if texture_pixels > 64 * 1024 * 1024 {
                        return Err("scene color textures exceed the 64 million pixel limit".into());
                    }
                }
            }
        }
        Ok(())
    }
    /// Iterate all triangle corners, points and line endpoints, including duplicates.
    pub fn positions(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.triangles
            .iter()
            .flat_map(|t| t.vertices)
            .chain(self.points.iter().map(|p| p.position))
            .chain(self.lines.iter().flat_map(|line| line.vertices))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_triangle_appearance_is_rejected_before_rasterization() {
        let scene = |appearance: TriangleAppearance| Scene {
            triangles: vec![Triangle {
                vertices: [
                    Vec3::default(),
                    Vec3::new(1.0, 0.0, 0.0),
                    Vec3::new(0.0, 1.0, 0.0),
                ],
                color: None,
            }],
            appearances: [(0, appearance)].into_iter().collect(),
            ..Scene::default()
        };
        assert!(scene(TriangleAppearance::default()).validate().is_ok());
        assert!(
            scene(TriangleAppearance {
                alpha_mode: AlphaMode::Mask { cutoff: 1.1 },
                ..TriangleAppearance::default()
            })
            .validate()
            .is_ok()
        );
        assert!(
            scene(TriangleAppearance {
                texcoords: Some([[f64::MAX, 0.0]; 3]),
                ..TriangleAppearance::default()
            })
            .validate()
            .is_ok()
        );
        let invalid = [
            TriangleAppearance {
                base_color: [1.0, 0.0, f64::NAN, 1.0],
                ..TriangleAppearance::default()
            },
            TriangleAppearance {
                colors: Some([[1.1; 4]; 3]),
                ..TriangleAppearance::default()
            },
            TriangleAppearance {
                texcoords: Some([[f64::INFINITY, 0.0]; 3]),
                ..TriangleAppearance::default()
            },
            TriangleAppearance {
                texcoords: Some([[f64::NAN, 0.0]; 3]),
                ..TriangleAppearance::default()
            },
            TriangleAppearance {
                normals: Some([Vec3::default(); 3]),
                ..TriangleAppearance::default()
            },
            TriangleAppearance {
                alpha_mode: AlphaMode::Mask { cutoff: -0.1 },
                ..TriangleAppearance::default()
            },
            TriangleAppearance {
                texture: Some(Texture {
                    image: Arc::new(RgbaImage::new(1, 1)),
                    wrap_s: TextureWrap::Repeat,
                    wrap_t: TextureWrap::Repeat,
                    filter: TextureFilter::Nearest,
                    min_filter: TextureFilter::Nearest,
                }),
                ..TriangleAppearance::default()
            },
            TriangleAppearance {
                texture: Some(Texture {
                    image: Arc::new(RgbaImage::new(0, 1)),
                    wrap_s: TextureWrap::Repeat,
                    wrap_t: TextureWrap::Repeat,
                    filter: TextureFilter::Nearest,
                    min_filter: TextureFilter::Nearest,
                }),
                texcoords: Some([[0.0; 2]; 3]),
                ..TriangleAppearance::default()
            },
        ];
        for appearance in invalid {
            assert!(scene(appearance).validate().is_err());
        }
        let mut missing = scene(TriangleAppearance::default());
        missing.appearances.insert(1, TriangleAppearance::default());
        assert!(missing.validate().is_err());
    }
}
