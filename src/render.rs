//! Deterministic, headless thumbnail rendering without graphics drivers or native libraries.

use crate::scene::{
    AlphaMode, Scene, Texture, TextureFilter, TextureWrap, Triangle, TriangleAppearance, Vec3,
};
use image::RgbaImage;
use std::io::Write;

/// Largest permitted width or height of the final thumbnail.
const MAX_DIMENSION: u32 = 8192;
/// Maximum number of samples stored in the supersampled framebuffer.
const MAX_SAMPLE_PIXELS: usize = 16 * 1024 * 1024;
/// Cumulative bounding-box sample budget for material-bearing triangles.
const MAX_APPEARANCE_SAMPLES: u64 = 256 * 1024 * 1024;
/// Maximum retained translucent samples before depth-sorted compositing.
const MAX_TRANSPARENT_FRAGMENTS: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
/// Output dimensions, background, fallback model color and antialiasing settings.
pub struct RenderOptions {
    /// Final image width in pixels; must be between 1 and 8192.
    pub width: u32,
    /// Final image height in pixels; must be between 1 and 8192.
    pub height: u32,
    /// Use a transparent background instead of the configured opaque background.
    pub transparent: bool,
    /// sRGB background color used when transparency is disabled.
    pub background: [u8; 3],
    /// sRGB fallback color for primitives without a supplied color or appearance.
    pub color: [u8; 3],
    /// Samples along each pixel axis; values from 1 through 4 are supported.
    pub supersampling: u32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: 512,
            height: 512,
            transparent: false,
            background: [245, 247, 250],
            color: [100, 160, 220],
            supersampling: 2,
        }
    }
}

#[derive(Clone, Copy)]
/// Axis-aligned bounds accumulated in model or projected coordinates.
struct Bounds {
    /// Smallest included coordinate on each axis.
    min: Vec3,
    /// Largest included coordinate on each axis.
    max: Vec3,
}

impl Bounds {
    /// Initialize an inverted box that the first included point will populate.
    fn empty() -> Self {
        Self {
            min: Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            max: Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
        }
    }

    /// Expand each axis to contain a finite position.
    fn include(&mut self, p: Vec3) {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.min.z = self.min.z.min(p.z);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
        self.max.z = self.max.z.max(p.z);
    }

    /// Return the midpoint without adding potentially overflowing endpoints.
    fn center(self) -> Vec3 {
        self.min + (self.max - self.min) * 0.5
    }
}

#[derive(Clone, Copy)]
/// Scale and center valid geometry before projection to avoid extreme-coordinate overflow.
struct Geometry {
    /// Maximum absolute input coordinate, replaced by one for an all-zero scene.
    coordinate_scale: f64,
    /// Center of nondegenerate geometry after the initial coordinate scaling.
    center: Vec3,
    /// Largest scaled axis extent, replaced by one for coincident points.
    extent: f64,
}

impl Geometry {
    /// Validate the scene and fit its nondegenerate primitives to a unit-sized extent.
    fn new(scene: &Scene) -> crate::Result<Self> {
        scene.validate()?;
        // Scale before subtracting: even a model spanning -f64::MAX..f64::MAX
        // must not overflow while computing its extent or surface normals.
        let coordinate_scale = scene.positions().fold(0.0_f64, |scale, p| {
            scale.max(p.x.abs()).max(p.y.abs()).max(p.z.abs())
        });
        let coordinate_scale = if coordinate_scale == 0.0 {
            1.0
        } else {
            coordinate_scale
        };
        let mut bounds = Bounds::empty();
        for triangle in &scene.triangles {
            let vertices = triangle.vertices.map(|p| p / coordinate_scale);
            if surface_normal(vertices).is_some() {
                for p in vertices {
                    bounds.include(p);
                }
            }
        }
        for point in &scene.points {
            bounds.include(point.position / coordinate_scale);
        }
        for line in &scene.lines {
            for p in line.vertices {
                bounds.include(p / coordinate_scale);
            }
        }
        if !bounds.min.is_finite() {
            return Err("model contains only degenerate triangles".into());
        }
        let size = bounds.max - bounds.min;
        let extent = size.x.max(size.y).max(size.z);
        Ok(Self {
            coordinate_scale,
            center: bounds.center(),
            // A point cloud containing a single position is still renderable.
            extent: if extent == 0.0 { 1.0 } else { extent },
        })
    }

    /// Map a model-space point through the overflow-resistant normalization.
    fn position(self, p: Vec3) -> Vec3 {
        (p / self.coordinate_scale - self.center) / self.extent
    }

    /// Normalize a triangle and return its face normal, discarding collinear corners.
    fn triangle(self, triangle: &Triangle) -> Option<([Vec3; 3], Vec3)> {
        let scaled = triangle.vertices.map(|p| p / self.coordinate_scale);
        // Use the same validity decision as the bounds pass. Otherwise a
        // discarded, distant degenerate triangle could become non-collinear
        // through rounding after centering and distort the image bounds.
        let normal = surface_normal(scaled)?;
        let vertices = scaled.map(|p| (p - self.center) / self.extent);
        Some((vertices, normal))
    }
}

/// Compute a unit face normal using normalized edges; reject nearly collinear triangles.
fn surface_normal(vertices: [Vec3; 3]) -> Option<Vec3> {
    // Normalize the edges separately to preserve small, valid geometry when
    // squaring its coordinates would underflow.
    let a = (vertices[1] - vertices[0]).normalized();
    let b = (vertices[2] - vertices[0]).normalized();
    let cross = a.cross(b);
    if cross.length() <= 1e-14 {
        None
    } else {
        Some(cross.normalized())
    }
}

#[derive(Clone, Copy)]
/// Orthonormal basis of a fixed isometric camera with depth increasing toward the viewer.
struct Camera {
    /// Unit image-horizontal axis.
    right: Vec3,
    /// Unit image-vertical axis before image-space Y inversion.
    up: Vec3,
    /// Unit depth axis pointing from the scene to the viewer.
    toward_viewer: Vec3,
}

impl Camera {
    /// Build a camera basis using the model Z axis as the vertical reference.
    fn new(direction: Vec3) -> Self {
        let toward_viewer = direction.normalized();
        let right = Vec3::new(0.0, 0.0, 1.0).cross(toward_viewer).normalized();
        let up = toward_viewer.cross(right).normalized();
        Self {
            right,
            up,
            toward_viewer,
        }
    }

    /// Project a normalized position to right, downward and viewer-facing axes.
    fn project(self, p: Vec3) -> Vec3 {
        Vec3::new(
            p.dot(self.right),
            -p.dot(self.up),
            p.dot(self.toward_viewer),
        )
    }

    /// Select the first deterministic view that exposes a valid surface or non-surface primitive.
    fn choose(scene: &Scene, geometry: Geometry) -> Self {
        let directions = [
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 0.6),
            Vec3::new(1.0, -0.6, 1.0),
        ];
        for direction in directions {
            let camera = Self::new(direction);
            if !scene.points.is_empty()
                || !scene.lines.is_empty()
                || scene.triangles.iter().any(|triangle| {
                    geometry
                        .triangle(triangle)
                        .is_some_and(|(_, normal)| normal.dot(camera.toward_viewer).abs() > 1e-8)
                })
            {
                return camera;
            }
        }
        // The three directions span 3D, so valid surfaces always have a view.
        Self::new(directions[0])
    }
}

#[derive(Clone, Copy)]
/// Orthographic camera fit and image-space scale for normalized geometry.
struct Projection {
    /// Overflow-resistant model normalization used to prepare positions.
    geometry: Geometry,
    /// Selected deterministic camera basis.
    camera: Camera,
    /// Center of the camera-space geometry bounds.
    center: Vec3,
    /// Pixels per normalized coordinate unit.
    scale: f64,
    /// Framebuffer center, with zero depth offset.
    image_center: Vec3,
}

impl Projection {
    /// Fit projected geometry inside the framebuffer with a proportional margin.
    fn new(scene: &Scene, geometry: Geometry, width: u32, height: u32) -> Self {
        let camera = Camera::choose(scene, geometry);
        let mut bounds = Bounds::empty();
        for triangle in &scene.triangles {
            if let Some((vertices, _)) = geometry.triangle(triangle) {
                for p in vertices {
                    bounds.include(camera.project(p));
                }
            }
        }
        for point in &scene.points {
            bounds.include(camera.project(geometry.position(point.position)));
        }
        for line in &scene.lines {
            for p in line.vertices {
                bounds.include(camera.project(geometry.position(p)));
            }
        }
        let width = f64::from(width);
        let height = f64::from(height);
        let margin = (width.min(height) * 0.075)
            .max(1.0)
            .min(width.min(height) * 0.2);
        let size = bounds.max - bounds.min;
        let sx = if size.x > 1e-12 {
            (width - 2.0 * margin) / size.x
        } else {
            f64::INFINITY
        };
        let sy = if size.y > 1e-12 {
            (height - 2.0 * margin) / size.y
        } else {
            f64::INFINITY
        };
        let scale = sx.min(sy);
        Self {
            geometry,
            camera,
            center: bounds.center(),
            scale: if scale.is_finite() {
                scale
            } else {
                width.min(height)
            },
            image_center: Vec3::new(width * 0.5, height * 0.5, 0.0),
        }
    }

    /// Convert a normalized position to pixel coordinates while retaining camera depth.
    fn project(self, world: Vec3) -> Vec3 {
        let p = self.camera.project(world);
        Vec3::new(
            (p.x - self.center.x) * self.scale + self.image_center.x,
            (p.y - self.center.y) * self.scale + self.image_center.y,
            p.z,
        )
    }
}

/// Supersampled RGBA framebuffer, opaque depth buffer and bounded translucent sample queue.
struct Canvas {
    /// Supersampled framebuffer width.
    width: u32,
    /// Supersampled framebuffer height.
    height: u32,
    /// Row-major, straight-alpha sRGB bytes, four bytes per sample.
    pixels: Vec<u8>,
    /// Nearest opaque depth at each sample; negative infinity marks untouched background.
    depths: Vec<f32>,
    /// Bounded queue of linear RGBA samples awaiting depth sorting.
    transparent: Vec<TransparentFragment>,
    /// Total material triangle bounding-box samples visited so far.
    appearance_samples: u64,
    /// Whether downsampling uses linear color for a scene containing appearances.
    linear_supersampling: bool,
}

/// One translucent sample retained for deterministic back-to-front compositing.
struct TransparentFragment {
    /// Linear framebuffer sample index, bounded by the sample-pixel limit.
    pixel: u32,
    /// Camera-space depth used for deterministic fragment ordering.
    depth: f64,
    /// Linear, unpremultiplied RGBA components compressed to single precision.
    color: [f32; 4],
}

impl Canvas {
    /// Allocate a background-filled framebuffer and initialize all depths behind the scene.
    fn new(width: u32, height: u32, background: [u8; 4]) -> crate::Result<Self> {
        let count = width as usize * height as usize;
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(count * 4)?;
        pixels.resize(count * 4, 0);
        for pixel in pixels.as_chunks_mut::<4>().0.iter_mut() {
            pixel.copy_from_slice(&background);
        }
        let mut depths = Vec::new();
        depths.try_reserve_exact(count)?;
        depths.resize(count, f32::NEG_INFINITY);
        Ok(Self {
            width,
            height,
            pixels,
            depths,
            transparent: Vec::new(),
            appearance_samples: 0,
            linear_supersampling: false,
        })
    }

    /// Write an opaque sample only when it is nearer than the stored depth.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "Validated framebuffer and texture bounds keep indices representable; color and depth quantization intentionally reduce precision"
    )]
    fn pixel(&mut self, x: u32, y: u32, depth: f64, color: [u8; 3]) -> bool {
        let offset = y as usize * self.width as usize + x as usize;
        if depth as f32 <= self.depths[offset] {
            return false;
        }
        self.depths[offset] = depth as f32;
        self.pixels[offset * 4..offset * 4 + 4]
            .copy_from_slice(&[color[0], color[1], color[2], 255]);
        true
    }

    /// Rasterize a flat-color triangle with barycentric depth interpolation.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Validated framebuffer and texture bounds keep indices representable; color and depth quantization intentionally reduce precision"
    )]
    fn triangle(&mut self, vertices: [Vec3; 3], color: [u8; 3]) -> usize {
        let [a, b, c] = vertices;
        let area = edge(a, b, c);
        if area.abs() < 1e-12 {
            return 0;
        }
        let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as u32;
        let max_x = (a.x.max(b.x).max(c.x).ceil() as u32).min(self.width - 1);
        let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as u32;
        let max_y = (a.y.max(b.y).max(c.y).ceil() as u32).min(self.height - 1);
        let mut written = 0;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let p = Vec3::new(f64::from(x) + 0.5, f64::from(y) + 0.5, 0.0);
                let w0 = edge(b, c, p) / area;
                let w1 = edge(c, a, p) / area;
                let w2 = edge(a, b, p) / area;
                if w0 >= -1e-10 && w1 >= -1e-10 && w2 >= -1e-10 {
                    written += usize::from(self.pixel(x, y, w0 * a.z + w1 * b.z + w2 * c.z, color));
                }
            }
        }
        written
    }

    /// Rasterize a validated material with UVs, normal interpolation, alpha and work limits.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::too_many_lines,
        reason = "Validated framebuffer and texture bounds keep indices representable; color and depth quantization intentionally reduce precision"
    )]
    fn appearance_triangle(
        &mut self,
        vertices: [Vec3; 3],
        appearance: &TriangleAppearance,
        normal: Vec3,
        camera: Camera,
    ) -> crate::Result<usize> {
        self.linear_supersampling = true;
        if !appearance.double_sided && normal.dot(camera.toward_viewer) <= 0.0 {
            return Ok(0);
        }
        let [a, b, c] = vertices;
        let area = edge(a, b, c);
        if area.abs() < 1e-12 {
            return Ok(0);
        }
        let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as u32;
        let max_x = (a.x.max(b.x).max(c.x).ceil() as u32).min(self.width - 1);
        let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as u32;
        let max_y = (a.y.max(b.y).max(c.y).ceil() as u32).min(self.height - 1);
        if min_x > max_x || min_y > max_y {
            return Ok(0);
        }
        self.appearance_samples += u64::from(max_x - min_x + 1) * u64::from(max_y - min_y + 1);
        if self.appearance_samples > MAX_APPEARANCE_SAMPLES {
            return Err(
                "appearance rasterization exceeds the 256 million sample work limit".into(),
            );
        }
        let mut written = 0;
        let texture_filter = appearance.texture.as_ref().map(|texture| {
            let uv = appearance.texcoords.expect("validated texture coordinates");
            let edges = [(b, c), (c, a), (a, b)];
            let dx: [f64; 2] = std::array::from_fn(|channel| {
                (0..3)
                    .map(|i| uv[i][channel] * (edges[i].1.y - edges[i].0.y) / area)
                    .sum()
            });
            let dy: [f64; 2] = std::array::from_fn(|channel| {
                (0..3)
                    .map(|i| uv[i][channel] * (edges[i].0.x - edges[i].1.x) / area)
                    .sum()
            });
            let width = f64::from(texture.image.width());
            let height = f64::from(texture.image.height());
            let footprint = (dx[0] * width)
                .hypot(dx[1] * height)
                .max((dy[0] * width).hypot(dy[1] * height));
            if !footprint.is_finite() || footprint > 1.0 {
                texture.min_filter
            } else {
                texture.filter
            }
        });
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let p = Vec3::new(f64::from(x) + 0.5, f64::from(y) + 0.5, 0.0);
                let weights = [
                    edge(b, c, p) / area,
                    edge(c, a, p) / area,
                    edge(a, b, p) / area,
                ];
                // Half-open shared edges prevent two triangles of one blended
                // surface from painting the same sample twice.
                let edges = [(b, c), (c, a), (a, b)];
                if weights.iter().zip(edges).any(|(w, (u, v))| {
                    *w < -1e-10 || (w.abs() <= 1e-10 && !inclusive_edge(u, v, area))
                }) {
                    continue;
                }
                let depth = weights[0] * a.z + weights[1] * b.z + weights[2] * c.z;
                let offset = y as usize * self.width as usize + x as usize;
                if depth as f32 <= self.depths[offset] {
                    continue;
                }
                let color = appearance_color_with_filter(
                    appearance,
                    weights,
                    normal,
                    camera,
                    texture_filter,
                );
                match appearance.alpha_mode {
                    AlphaMode::Opaque => {
                        written += usize::from(self.pixel(x, y, depth, encode_color(color)));
                    }
                    AlphaMode::Mask { cutoff } => {
                        if color[3] >= cutoff {
                            written += usize::from(self.pixel(x, y, depth, encode_color(color)));
                        }
                    }
                    AlphaMode::Blend => {
                        if color[3] <= 0.0 {
                            continue;
                        }
                        if self.transparent.len() == MAX_TRANSPARENT_FRAGMENTS {
                            return Err(
                                "translucent surfaces exceed the four million fragment limit"
                                    .into(),
                            );
                        }
                        if self.transparent.len() == self.transparent.capacity() {
                            let remaining = MAX_TRANSPARENT_FRAGMENTS - self.transparent.len();
                            self.transparent.try_reserve(remaining.min(4096))?;
                        }
                        self.transparent.push(TransparentFragment {
                            pixel: offset as u32,
                            depth,
                            color: color.map(|v| v as f32),
                        });
                        written += 1;
                    }
                }
            }
        }
        Ok(written)
    }

    /// Sort translucent samples by pixel and depth, then composite visible colors in linear space.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Validated framebuffer and texture bounds keep indices representable; color and depth quantization intentionally reduce precision"
    )]
    fn composite_transparency(&mut self) {
        self.transparent.sort_unstable_by(|a, b| {
            a.pixel
                .cmp(&b.pixel)
                .then_with(|| a.depth.total_cmp(&b.depth))
                .then_with(|| {
                    a.color
                        .iter()
                        .zip(b.color)
                        .map(|(a, b)| a.total_cmp(&b))
                        .find(|order| !order.is_eq())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        let mut cursor = 0;
        while cursor < self.transparent.len() {
            let offset = self.transparent[cursor].pixel as usize;
            let pixel = &mut self.pixels[offset * 4..offset * 4 + 4];
            let mut alpha = f64::from(pixel[3]) / 255.0;
            let mut premultiplied = [0.0; 3];
            for channel in 0..3 {
                premultiplied[channel] = srgb_to_linear(f64::from(pixel[channel]) / 255.0) * alpha;
            }
            while cursor < self.transparent.len()
                && self.transparent[cursor].pixel as usize == offset
            {
                let fragment = &self.transparent[cursor];
                if fragment.depth as f32 > self.depths[offset] {
                    let a = f64::from(fragment.color[3]);
                    for (channel, output) in premultiplied.iter_mut().enumerate() {
                        *output = f64::from(fragment.color[channel]) * a + *output * (1.0 - a);
                    }
                    alpha = a + alpha * (1.0 - a);
                }
                cursor += 1;
            }
            for channel in 0..3 {
                pixel[channel] = if alpha > 0.0 {
                    (linear_to_srgb(premultiplied[channel] / alpha) * 255.0).round() as u8
                } else {
                    0
                };
            }
            pixel[3] = (alpha * 255.0).round() as u8;
        }
    }

    /// Draw a depth-tested filled disk around a projected point.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Validated framebuffer and texture bounds keep indices representable; color and depth quantization intentionally reduce precision"
    )]
    fn point(&mut self, p: Vec3, radius: f64, color: [u8; 3]) {
        let min_x = (p.x - radius).floor().max(0.0) as u32;
        let max_x = ((p.x + radius).ceil() as u32).min(self.width - 1);
        let min_y = (p.y - radius).floor().max(0.0) as u32;
        let max_y = ((p.y + radius).ceil() as u32).min(self.height - 1);
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let dx = f64::from(x) + 0.5 - p.x;
                let dy = f64::from(y) + 0.5 - p.y;
                if dx * dx + dy * dy <= radius * radius {
                    self.pixel(x, y, p.z, color);
                }
            }
        }
    }

    /// Draw a segment by sampling overlapping depth-tested disks along its length.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Validated framebuffer and texture bounds keep indices representable; color and depth quantization intentionally reduce precision"
    )]
    fn line(&mut self, a: Vec3, b: Vec3, color: [u8; 3], radius: f64) {
        let count = ((a.x - b.x).abs().max((a.y - b.y).abs()) * 2.0).ceil() as u32;
        for i in 0..=count {
            let t = if count == 0 {
                0.0
            } else {
                f64::from(i) / f64::from(count)
            };
            self.point(a + (b - a) * t, radius, color);
        }
    }

    /// Downsample premultiplied samples and return an RGBA image with straight alpha.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Validated framebuffer and texture bounds keep indices representable; color and depth quantization intentionally reduce precision"
    )]
    fn into_image(self, options: &RenderOptions) -> crate::Result<RgbaImage> {
        let samples = options.supersampling;
        if samples == 1 {
            return RgbaImage::from_raw(options.width, options.height, self.pixels)
                .ok_or_else(|| "invalid rendered image dimensions".into());
        }
        let mut output = Vec::new();
        let output_size = options.width as usize * options.height as usize * 4;
        output.try_reserve_exact(output_size)?;
        output.resize(output_size, 0);
        for y in 0..options.height {
            for x in 0..options.width {
                let mut alpha = 0_u32;
                let mut colors = [0_u32; 3];
                let mut linear_colors = [0.0_f64; 3];
                for sy in 0..samples {
                    for sx in 0..samples {
                        let offset = ((y * samples + sy) as usize * self.width as usize
                            + (x * samples + sx) as usize)
                            * 4;
                        let a = u32::from(self.pixels[offset + 3]);
                        alpha += a;
                        for (channel, sum) in colors.iter_mut().enumerate() {
                            if self.linear_supersampling {
                                linear_colors[channel] += srgb_to_linear(
                                    f64::from(self.pixels[offset + channel]) / 255.0,
                                ) * f64::from(a);
                            } else {
                                *sum += u32::from(self.pixels[offset + channel]) * a;
                            }
                        }
                    }
                }
                let offset = (y as usize * options.width as usize + x as usize) * 4;
                // Average premultiplied colors, then unpremultiply. This
                // keeps translucent edge pixels free of dark halos.
                for (channel, sum) in colors.iter().enumerate() {
                    output[offset + channel] = if self.linear_supersampling {
                        if alpha == 0 {
                            0
                        } else {
                            (linear_to_srgb(linear_colors[channel] / f64::from(alpha)) * 255.0)
                                .round() as u8
                        }
                    } else {
                        (sum + alpha / 2).checked_div(alpha).unwrap_or(0) as u8
                    };
                }
                let count = samples * samples;
                output[offset + 3] = ((alpha + count / 2) / count) as u8;
            }
        }
        RgbaImage::from_raw(options.width, options.height, output)
            .ok_or_else(|| "invalid rendered image dimensions".into())
    }
}

/// Evaluate the signed edge function used for barycentric triangle coverage.
fn edge(a: Vec3, b: Vec3, p: Vec3) -> f64 {
    (p.x - a.x) * (b.y - a.y) - (p.y - a.y) * (b.x - a.x)
}

/// Apply a half-open shared-edge rule so adjacent blended triangles do not double-paint.
#[allow(
    clippy::float_cmp,
    reason = "Shared-edge ownership requires exact equality of the same projected endpoint coordinates"
)]
fn inclusive_edge(mut a: Vec3, mut b: Vec3, area: f64) -> bool {
    if area < 0.0 {
        std::mem::swap(&mut a, &mut b);
    }
    b.y > a.y || (b.y == a.y && b.x < a.x)
}

/// Decode an sRGB component to linear intensity using the standard transfer function.
fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Clamp a linear intensity and encode it with the sRGB transfer function.
fn linear_to_srgb(value: f64) -> f64 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// Quantize the three linear color channels to rounded sRGB bytes.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Validated framebuffer and texture bounds keep indices representable; color and depth quantization intentionally reduce precision"
)]
fn encode_color(color: [f64; 4]) -> [u8; 3] {
    [color[0], color[1], color[2]].map(|v| (linear_to_srgb(v) * 255.0).round() as u8)
}

/// Reduce a finite texture coordinate according to its sampler addressing mode.
fn wrap_coordinate(value: f64, wrap: TextureWrap) -> f64 {
    match wrap {
        TextureWrap::Repeat => value.rem_euclid(1.0),
        TextureWrap::ClampToEdge => value.clamp(0.0, 1.0),
        TextureWrap::MirroredRepeat => {
            let value = value.rem_euclid(2.0);
            if value > 1.0 { 2.0 - value } else { value }
        }
    }
}

/// Address a texel in a nonempty image using repeat, clamp or reflected repeat.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Validated framebuffer and texture bounds keep indices representable; color and depth quantization intentionally reduce precision"
)]
fn texture_index(value: i64, size: u32, wrap: TextureWrap) -> u32 {
    let size = i64::from(size);
    match wrap {
        TextureWrap::Repeat => value.rem_euclid(size) as u32,
        TextureWrap::ClampToEdge => value.clamp(0, size - 1) as u32,
        TextureWrap::MirroredRepeat => {
            let index = value.rem_euclid(size * 2);
            if index < size {
                index as u32
            } else {
                (size * 2 - index - 1) as u32
            }
        }
    }
}

#[cfg(test)]
/// Sample a texture using its magnification filter for focused sampler tests.
fn sample_texture(texture: &Texture, uv: [f64; 2]) -> [f64; 4] {
    sample_texture_with_filter(texture, uv, texture.filter)
}

/// Sample decoded texels in linear RGBA, applying addressing and nearest or bilinear filtering.
#[allow(
    clippy::cast_possible_truncation,
    reason = "Validated framebuffer and texture bounds keep indices representable; color and depth quantization intentionally reduce precision"
)]
fn sample_texture_with_filter(texture: &Texture, uv: [f64; 2], filter: TextureFilter) -> [f64; 4] {
    let width = texture.image.width();
    let height = texture.image.height();
    let u = wrap_coordinate(uv[0], texture.wrap_s);
    let v = wrap_coordinate(uv[1], texture.wrap_t);
    let texel = |x: i64, y: i64| {
        let pixel = texture.image.get_pixel(
            texture_index(x, width, texture.wrap_s),
            texture_index(y, height, texture.wrap_t),
        );
        [
            srgb_to_linear(f64::from(pixel[0]) / 255.0),
            srgb_to_linear(f64::from(pixel[1]) / 255.0),
            srgb_to_linear(f64::from(pixel[2]) / 255.0),
            f64::from(pixel[3]) / 255.0,
        ]
    };
    match filter {
        TextureFilter::Nearest => texel(
            (u * f64::from(width)).floor().min(f64::from(width - 1)) as i64,
            (v * f64::from(height)).floor().min(f64::from(height - 1)) as i64,
        ),
        TextureFilter::Linear => {
            let x = u * f64::from(width) - 0.5;
            let y = v * f64::from(height) - 0.5;
            let x0 = x.floor() as i64;
            let y0 = y.floor() as i64;
            let tx = x - x.floor();
            let ty = y - y.floor();
            let corners = [
                texel(x0, y0),
                texel(x0 + 1, y0),
                texel(x0, y0 + 1),
                texel(x0 + 1, y0 + 1),
            ];
            std::array::from_fn(|channel| {
                (corners[0][channel] * (1.0 - tx) + corners[1][channel] * tx) * (1.0 - ty)
                    + (corners[2][channel] * (1.0 - tx) + corners[3][channel] * tx) * ty
            })
        }
    }
}

#[cfg(test)]
/// Evaluate an appearance without a derivative-selected filter override for renderer tests.
fn appearance_color(
    appearance: &TriangleAppearance,
    weights: [f64; 3],
    normal: Vec3,
    camera: Camera,
) -> [f64; 4] {
    appearance_color_with_filter(appearance, weights, normal, camera, None)
}

/// Interpolate material attributes, multiply the sampled texture and apply optional diffuse lighting.
fn appearance_color_with_filter(
    appearance: &TriangleAppearance,
    weights: [f64; 3],
    normal: Vec3,
    camera: Camera,
    texture_filter: Option<TextureFilter>,
) -> [f64; 4] {
    let geometric_normal = normal;
    let weights = weights.map(|w| w.max(0.0));
    let sum: f64 = weights.iter().sum();
    let weights = weights.map(|w| w / sum);
    let mut color = appearance.base_color;
    if let Some(colors) = appearance.colors {
        for channel in 0..4 {
            color[channel] *= (0..3)
                .map(|vertex| colors[vertex][channel] * weights[vertex])
                .sum::<f64>();
        }
    }
    if let (Some(texture), Some(uvs)) = (&appearance.texture, appearance.texcoords) {
        let uv = std::array::from_fn(|channel| {
            let scale = uvs
                .iter()
                .map(|uv| uv[channel].abs())
                .fold(0.0_f64, f64::max);
            if scale > 1e100 {
                // The convex result is finite even if summation near f64::MAX
                // would overflow through rounding. Preserve ordinary UV math.
                (0..3)
                    .map(|vertex| uvs[vertex][channel] / scale * weights[vertex])
                    .sum::<f64>()
                    .clamp(-1.0, 1.0)
                    * scale
            } else {
                (0..3)
                    .map(|vertex| uvs[vertex][channel] * weights[vertex])
                    .sum()
            }
        });
        let sampled =
            sample_texture_with_filter(texture, uv, texture_filter.unwrap_or(texture.filter));
        for channel in 0..4 {
            color[channel] *= sampled[channel];
        }
    }
    if !appearance.unlit {
        let normal = appearance.normals.map_or(normal, |normals| {
            let blended = normals[0].normalized() * weights[0]
                + normals[1].normalized() * weights[1]
                + normals[2].normalized() * weights[2];
            if blended.length() > 1e-14 {
                blended.normalized()
            } else {
                normal
            }
        });
        // glTF reverses normals on double-sided back faces according to the
        // geometric face, including independently supplied vertex normals.
        let back_facing =
            appearance.double_sided && geometric_normal.dot(camera.toward_viewer) < 0.0;
        let normal = if back_facing { -normal } else { normal };
        let light = Vec3::new(-0.35, -0.6, 1.0).normalized();
        let intensity = 0.35 + 0.65 * normal.dot(light).max(0.0);
        for channel in &mut color[..3] {
            *channel *= intensity;
        }
    }
    color
}

/// Compute two-sided diffuse lighting for primitives without an explicit material.
fn lighting_intensity(normal: Vec3, camera: Camera) -> f64 {
    let light = Vec3::new(-0.35, -0.6, 1.0).normalized();
    let facing = normal.dot(camera.toward_viewer);
    let normal = if facing < 0.0 || (facing == 0.0 && normal.dot(light) < 0.0) {
        -normal
    } else {
        normal
    };
    0.35 + 0.65 * normal.dot(light).max(0.0)
}

/// Apply fallback diffuse lighting directly to the legacy byte-valued primitive color.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Validated framebuffer and texture bounds keep indices representable; color and depth quantization intentionally reduce precision"
)]
fn shade(color: [u8; 3], normal: Vec3, camera: Camera) -> [u8; 3] {
    let intensity = lighting_intensity(normal, camera);
    color.map(|channel| (f64::from(channel) * intensity).round() as u8)
}

/// Render triangles and point clouds from a fixed, orthographic isometric view.
///
/// A deterministic alternate view is used for a mesh whose surfaces are all
/// edge-on. Triangle winding hides surfaces only when an appearance explicitly
/// requests single-sided rendering. Output has straight alpha. Texture colors
/// and blended fragments use linear color internally; old flat-color triangles
/// retain their established flat-color shading. The render is limited to 16 million sample
/// pixels, 256 million appearance sample visits and four million blend fragments.
///
/// # Errors
///
/// Returns an error for invalid dimensions or supersampling, empty or invalid scene geometry,
/// allocation failures, or exceeded rasterization and transparency budgets.
#[allow(
    clippy::too_many_lines,
    reason = "Keep validated scene setup and the ordered raster passes together"
)]
pub fn render(scene: &Scene, options: &RenderOptions) -> crate::Result<RgbaImage> {
    if options.width == 0
        || options.height == 0
        || options.width > MAX_DIMENSION
        || options.height > MAX_DIMENSION
    {
        return Err(format!("thumbnail dimensions must be between 1 and {MAX_DIMENSION}").into());
    }
    if !(1..=4).contains(&options.supersampling) {
        return Err("supersampling must be between 1 and 4".into());
    }
    let width = options
        .width
        .checked_mul(options.supersampling)
        .ok_or("thumbnail width is too large")?;
    let height = options
        .height
        .checked_mul(options.supersampling)
        .ok_or("thumbnail height is too large")?;
    let sample_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or("thumbnail is too large")?;
    if sample_count > MAX_SAMPLE_PIXELS {
        return Err("thumbnail exceeds the limit of 16 million sample pixels; reduce dimensions or supersampling".into());
    }
    let geometry = Geometry::new(scene)?;
    let projection = Projection::new(scene, geometry, width, height);
    let background = if options.transparent {
        [0, 0, 0, 0]
    } else {
        [
            options.background[0],
            options.background[1],
            options.background[2],
            255,
        ]
    };
    let mut canvas = Canvas::new(width, height, background)?;
    canvas.linear_supersampling = !scene.appearances.is_empty();
    let mut fragments = 0;
    for (index, triangle) in scene.triangles.iter().enumerate() {
        if let Some((vertices, normal)) = geometry.triangle(triangle) {
            if let Some(appearance) = scene.appearances.get(&index) {
                fragments += canvas.appearance_triangle(
                    vertices.map(|p| projection.project(p)),
                    appearance,
                    normal,
                    projection.camera,
                )?;
                continue;
            }
            let color = shade(
                triangle.color.unwrap_or(options.color),
                normal,
                projection.camera,
            );
            fragments += canvas.triangle(vertices.map(|p| projection.project(p)), color);
        }
    }
    // A valid but very thin surface can fall between all pixel centers.
    // Draw its outline so a successful render never produces a blank image.
    if fragments == 0 && scene.points.is_empty() {
        for (index, triangle) in scene.triangles.iter().enumerate() {
            if scene.appearances.contains_key(&index) {
                continue;
            }
            if let Some((vertices, normal)) = geometry.triangle(triangle) {
                let vertices = vertices.map(|p| projection.project(p));
                let color = shade(
                    triangle.color.unwrap_or(options.color),
                    normal,
                    projection.camera,
                );
                for edge in 0..3 {
                    canvas.line(
                        vertices[edge],
                        vertices[(edge + 1) % 3],
                        color,
                        f64::from(options.supersampling) * 0.6,
                    );
                }
            }
        }
    }
    for point in &scene.points {
        canvas.point(
            projection.project(projection.geometry.position(point.position)),
            f64::from(options.supersampling) * 1.5,
            point.color.unwrap_or(options.color),
        );
    }
    for line in &scene.lines {
        let [a, b] = line
            .vertices
            .map(|p| projection.project(geometry.position(p)));
        canvas.line(
            a,
            b,
            line.color.unwrap_or(options.color),
            f64::from(options.supersampling) * 0.75,
        );
    }
    canvas.composite_transparency();
    canvas.into_image(options)
}

/// Write a rendered thumbnail as PNG, including its alpha channel.
///
/// # Errors
///
/// Returns an error if PNG encoding fails or the output writer cannot accept or finish the image.
pub fn write_png<W: Write>(image: &RgbaImage, writer: W) -> crate::Result<()> {
    let mut encoder = png::Encoder::new(writer, image.width(), image.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(image.as_raw())?;
    writer.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Point;

    fn options() -> RenderOptions {
        RenderOptions {
            width: 64,
            height: 64,
            transparent: true,
            ..RenderOptions::default()
        }
    }

    fn triangle(depth: f64, color: [u8; 3]) -> Triangle {
        let camera = Camera::new(Vec3::new(1.0, -1.0, 1.0));
        Triangle {
            vertices: [(-1.0, -1.0), (1.0, -1.0), (0.0, 1.0)]
                .map(|(x, y)| camera.right * x + camera.up * y + camera.toward_viewer * depth),
            color: Some(color),
        }
    }

    fn occupied(image: &RgbaImage) -> (u32, u32, u32, u32) {
        let mut bounds = (u32::MAX, u32::MAX, 0, 0);
        for (x, y, pixel) in image.enumerate_pixels() {
            if pixel[3] != 0 {
                bounds.0 = bounds.0.min(x);
                bounds.1 = bounds.1.min(y);
                bounds.2 = bounds.2.max(x);
                bounds.3 = bounds.3.max(y);
            }
        }
        assert_ne!(bounds.0, u32::MAX, "thumbnail is empty");
        bounds
    }

    #[test]
    fn nearer_geometry_wins_regardless_of_draw_order() {
        let near = triangle(0.25, [0, 255, 0]);
        let far = triangle(-0.25, [255, 0, 0]);
        let forward = render(
            &Scene {
                lines: Vec::new(),
                appearances: std::collections::BTreeMap::new(),
                triangles: vec![near.clone(), far.clone()],
                points: vec![],
            },
            &options(),
        )
        .unwrap();
        let backward = render(
            &Scene {
                lines: Vec::new(),
                appearances: std::collections::BTreeMap::new(),
                triangles: vec![far, near],
                points: vec![],
            },
            &options(),
        )
        .unwrap();
        assert_eq!(forward, backward);
        let center = forward.get_pixel(32, 32);
        assert_eq!(center[0], 0);
        assert!(center[1] > 100);
        assert_eq!(center[3], 255);
    }

    #[test]
    fn lines_fit_and_interpolate_depth_independent_of_draw_order() {
        use crate::scene::Line;
        let camera = Camera::new(Vec3::new(1.0, -1.0, 1.0));
        let line = |depth, color| Line {
            vertices: [-1.0, 1.0].map(|x| camera.right * x + camera.toward_viewer * depth),
            color: Some(color),
        };
        let near = line(0.25, [0, 255, 0]);
        let far = line(-0.25, [255, 0, 0]);
        let a = render(
            &Scene {
                lines: vec![near.clone(), far.clone()],
                ..Scene::default()
            },
            &options(),
        )
        .unwrap();
        let b = render(
            &Scene {
                lines: vec![far, near],
                ..Scene::default()
            },
            &options(),
        )
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(a.get_pixel(32, 32).0[..3], [0, 255, 0]);
        assert!(a.get_pixel(32, 32)[3] > 0);
        let bounds = occupied(&a);
        assert!(bounds.2 - bounds.0 > 50);
        assert!(bounds.3 - bounds.1 <= 3);
        let invalid = Scene {
            lines: vec![Line {
                vertices: [Vec3::new(f64::NAN, 0., 0.), Vec3::default()],
                color: None,
            }],
            ..Scene::default()
        };
        assert!(render(&invalid, &options()).is_err());
    }

    #[test]
    fn reverse_winding_keeps_surfaces_visible() {
        let original = triangle(0.0, [80, 140, 240]);
        let mut reversed = original.clone();
        reversed.vertices.swap(0, 2);
        let first = render(
            &Scene {
                lines: Vec::new(),
                appearances: std::collections::BTreeMap::new(),
                triangles: vec![original],
                points: vec![],
            },
            &options(),
        )
        .unwrap();
        let second = render(
            &Scene {
                lines: Vec::new(),
                appearances: std::collections::BTreeMap::new(),
                triangles: vec![reversed],
                points: vec![],
            },
            &options(),
        )
        .unwrap();
        assert_eq!(first, second);
        occupied(&first);
    }

    #[test]
    fn nonsquare_output_fits_and_centers_without_stretching() {
        let camera = Camera::new(Vec3::new(1.0, -1.0, 1.0));
        let corners = [(-2.0, -0.5), (2.0, -0.5), (2.0, 0.5), (-2.0, 0.5)]
            .map(|(x, y)| camera.right * x + camera.up * y);
        let scene = Scene {
            lines: Vec::new(),
            appearances: std::collections::BTreeMap::new(),
            triangles: vec![
                Triangle {
                    vertices: [corners[0], corners[1], corners[2]],
                    color: None,
                },
                Triangle {
                    vertices: [corners[0], corners[2], corners[3]],
                    color: None,
                },
            ],
            points: vec![],
        };
        let image = render(
            &scene,
            &RenderOptions {
                width: 400,
                height: 100,
                ..options()
            },
        )
        .unwrap();
        let (left, top, right, bottom) = occupied(&image);
        assert!((f64::from(right - left + 1) / f64::from(bottom - top + 1) - 4.0).abs() < 0.1);
        assert!((i64::from(left + right) - 399).abs() <= 1);
        assert!((i64::from(top + bottom) - 99).abs() <= 1);
        assert!(left > 0 && top > 0 && right < 399 && bottom < 99);
    }

    #[test]
    fn transparent_edges_preserve_color_without_dark_halos() {
        let image = render(
            &Scene {
                lines: Vec::new(),
                appearances: std::collections::BTreeMap::new(),
                triangles: vec![triangle(0.0, [50, 150, 250])],
                points: vec![],
            },
            &options(),
        )
        .unwrap();
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 0]);
        let solid = image.pixels().find(|pixel| pixel[3] == 255).unwrap();
        let edges: Vec<_> = image
            .pixels()
            .filter(|pixel| pixel[3] > 0 && pixel[3] < 255)
            .collect();
        assert_ne!(edges, [] as [&image::Rgba<u8>; 0]);
        for pixel in edges {
            assert_eq!(&pixel.0[..3], &solid.0[..3]);
        }
    }

    #[test]
    fn single_point_and_point_depth_are_supported() {
        let camera = Camera::new(Vec3::new(1.0, -1.0, 1.0));
        let scene = Scene {
            lines: Vec::new(),
            appearances: std::collections::BTreeMap::new(),
            triangles: vec![],
            points: vec![
                Point {
                    position: camera.toward_viewer,
                    color: Some([0, 255, 0]),
                },
                Point {
                    position: -camera.toward_viewer,
                    color: Some([255, 0, 0]),
                },
            ],
        };
        let image = render(&scene, &options()).unwrap();
        assert_eq!(image.get_pixel(32, 32).0, [0, 255, 0, 255]);
        let single = render(
            &Scene {
                lines: Vec::new(),
                appearances: std::collections::BTreeMap::new(),
                triangles: vec![],
                points: vec![Point {
                    position: Vec3::default(),
                    color: None,
                }],
            },
            &options(),
        )
        .unwrap();
        let (left, top, right, bottom) = occupied(&single);
        assert_eq!(left + right, 63);
        assert_eq!(top + bottom, 63);
    }

    #[test]
    fn extreme_finite_coordinates_do_not_overflow() {
        let triangle = Triangle {
            vertices: [
                Vec3::new(-f64::MAX, 0.0, 0.0),
                Vec3::new(f64::MAX, 0.0, 0.0),
                Vec3::new(0.0, f64::MAX, f64::MAX),
            ],
            color: None,
        };
        occupied(
            &render(
                &Scene {
                    lines: Vec::new(),
                    appearances: std::collections::BTreeMap::new(),
                    triangles: vec![triangle],
                    points: vec![],
                },
                &options(),
            )
            .unwrap(),
        );
    }

    #[test]
    fn entirely_edge_on_mesh_uses_a_visible_view() {
        let camera = Camera::new(Vec3::new(1.0, -1.0, 1.0));
        let scene = Scene {
            lines: Vec::new(),
            appearances: std::collections::BTreeMap::new(),
            triangles: vec![Triangle {
                vertices: [camera.right, -camera.right, camera.toward_viewer],
                color: None,
            }],
            points: vec![],
        };
        let image = render(&scene, &options()).unwrap();
        let (left, top, right, bottom) = occupied(&image);
        assert!(right - left > 20);
        assert!(bottom - top > 3);
    }

    #[test]
    fn invalid_geometry_and_image_limits_return_errors() {
        assert!(render(&Scene::default(), &options()).is_err());
        let collinear = Scene {
            lines: Vec::new(),
            appearances: std::collections::BTreeMap::new(),
            triangles: vec![Triangle {
                vertices: [
                    Vec3::default(),
                    Vec3::new(1.0, 0.0, 0.0),
                    Vec3::new(2.0, 0.0, 0.0),
                ],
                color: None,
            }],
            points: vec![],
        };
        assert!(render(&collinear, &options()).is_err());
        let nonfinite = Scene {
            lines: Vec::new(),
            appearances: std::collections::BTreeMap::new(),
            triangles: vec![],
            points: vec![Point {
                position: Vec3::new(f64::NAN, 0.0, 0.0),
                color: None,
            }],
        };
        assert!(render(&nonfinite, &options()).is_err());
        let scene = Scene {
            lines: Vec::new(),
            appearances: std::collections::BTreeMap::new(),
            triangles: vec![triangle(0.0, [255; 3])],
            points: vec![],
        };
        for invalid in [
            RenderOptions {
                width: 0,
                ..options()
            },
            RenderOptions {
                height: 9000,
                ..options()
            },
            RenderOptions {
                supersampling: 0,
                ..options()
            },
            RenderOptions {
                supersampling: 5,
                ..options()
            },
            RenderOptions {
                width: 8192,
                height: 8192,
                ..options()
            },
        ] {
            assert!(render(&scene, &invalid).is_err());
        }
    }

    #[test]
    fn png_round_trip_preserves_dimensions_and_alpha() {
        let image = render(
            &Scene {
                lines: Vec::new(),
                appearances: std::collections::BTreeMap::new(),
                triangles: vec![triangle(0.0, [60, 150, 220])],
                points: vec![],
            },
            &options(),
        )
        .unwrap();
        let mut bytes = Vec::new();
        write_png(&image, &mut bytes).unwrap();
        let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .unwrap()
            .to_rgba8();
        assert_eq!(image, decoded);
    }

    fn color_texture(filter: TextureFilter) -> Texture {
        use std::sync::Arc;
        Texture {
            image: Arc::new(
                RgbaImage::from_raw(
                    2,
                    2,
                    vec![
                        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
                    ],
                )
                .unwrap(),
            ),
            wrap_s: TextureWrap::ClampToEdge,
            wrap_t: TextureWrap::ClampToEdge,
            filter,
            min_filter: filter,
        }
    }

    fn unlit(color: [f64; 4], alpha_mode: AlphaMode) -> TriangleAppearance {
        TriangleAppearance {
            base_color: color,
            alpha_mode,
            unlit: true,
            ..TriangleAppearance::default()
        }
    }

    fn canvas_square(
        canvas: &mut Canvas,
        depth_left: f64,
        depth_right: f64,
        appearance: &TriangleAppearance,
    ) {
        let corners = [
            Vec3::new(0.0, 0.0, depth_left),
            Vec3::new(8.0, 0.0, depth_right),
            Vec3::new(8.0, 8.0, depth_right),
            Vec3::new(0.0, 8.0, depth_left),
        ];
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for indices in [[0, 1, 2], [0, 2, 3]] {
            let mut appearance = appearance.clone();
            if appearance.texture.is_some() {
                appearance.texcoords = Some(indices.map(|index| uvs[index]));
            }
            canvas
                .appearance_triangle(
                    indices.map(|index| corners[index]),
                    &appearance,
                    Vec3::new(0.0, 0.0, 1.0),
                    Camera::new(Vec3::new(1.0, -1.0, 1.0)),
                )
                .unwrap();
        }
    }

    fn canvas_image(mut canvas: Canvas) -> RgbaImage {
        canvas.composite_transparency();
        canvas
            .into_image(&RenderOptions {
                width: 8,
                height: 8,
                supersampling: 1,
                ..options()
            })
            .unwrap()
    }

    #[test]
    fn texture_quadrants_are_sampled_at_interpolated_uvs() {
        let mut canvas = Canvas::new(8, 8, [0; 4]).unwrap();
        let appearance = TriangleAppearance {
            texture: Some(color_texture(TextureFilter::Nearest)),
            unlit: true,
            ..TriangleAppearance::default()
        };
        canvas_square(&mut canvas, 0.0, 0.0, &appearance);
        let image = canvas_image(canvas);
        assert_eq!(image.get_pixel(1, 1).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(6, 1).0, [0, 255, 0, 255]);
        assert_eq!(image.get_pixel(1, 6).0, [0, 0, 255, 255]);
        assert_eq!(image.get_pixel(6, 6).0, [255; 4]);
        assert!(image.pixels().all(|pixel| pixel[3] == 255));
    }

    #[test]
    fn wrapping_and_bilinear_filtering_follow_texel_centers_and_srgb() {
        let mut texture = color_texture(TextureFilter::Nearest);
        for (wrap, expected) in [
            (TextureWrap::Repeat, [0, 255, 0]),
            (TextureWrap::ClampToEdge, [255, 0, 0]),
            (TextureWrap::MirroredRepeat, [255, 0, 0]),
        ] {
            texture.wrap_s = wrap;
            assert_eq!(
                encode_color(sample_texture(&texture, [-0.25, 0.25])),
                expected
            );
        }
        texture.wrap_s = TextureWrap::MirroredRepeat;
        assert_eq!(
            encode_color(sample_texture(&texture, [1.75, 0.25])),
            [255, 0, 0]
        );
        assert_eq!(
            encode_color(sample_texture(&texture, [1.25, 0.25])),
            [0, 255, 0]
        );
        texture.filter = TextureFilter::Linear;
        texture.wrap_s = TextureWrap::ClampToEdge;
        assert_eq!(encode_color(sample_texture(&texture, [0.5, 0.5])), [188; 3]);
        assert_eq!(
            encode_color(sample_texture(&texture, [-0.5, 0.25])),
            [255, 0, 0]
        );
        texture.wrap_s = TextureWrap::Repeat;
        assert_eq!(
            encode_color(sample_texture(&texture, [0.0, 0.25])),
            [188, 188, 0]
        );
        texture.wrap_s = TextureWrap::MirroredRepeat;
        assert_eq!(
            encode_color(sample_texture(&texture, [0.0, 0.25])),
            [255, 0, 0]
        );
        let appearance = TriangleAppearance {
            texture: Some(texture),
            texcoords: Some([[f64::MAX, 0.25]; 3]),
            unlit: true,
            ..TriangleAppearance::default()
        };
        let color = appearance_color(
            &appearance,
            [0.2, 0.3, 0.5],
            Vec3::new(0.0, 0.0, 1.0),
            Camera::new(Vec3::new(1.0, -1.0, 1.0)),
        );
        assert!(color.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn projected_texel_derivatives_select_minification_and_magnification_filters() {
        let mut texture = color_texture(TextureFilter::Linear);
        texture.min_filter = TextureFilter::Nearest;
        texture.wrap_s = TextureWrap::Repeat;
        texture.wrap_t = TextureWrap::Repeat;
        let mut appearance = TriangleAppearance {
            texture: Some(texture),
            unlit: true,
            ..TriangleAppearance::default()
        };
        let vertices = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(8.0, 0.0, 0.0),
            Vec3::new(0.0, 8.0, 0.0),
        ];
        let mut magnified = Canvas::new(8, 8, [0; 4]).unwrap();
        appearance.texcoords = Some([[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
        magnified
            .appearance_triangle(
                vertices,
                &appearance,
                Vec3::new(0.0, 0.0, 1.0),
                Camera::new(Vec3::new(1.0, -1.0, 1.0)),
            )
            .unwrap();
        let magnified = canvas_image(magnified);
        let pixel = magnified.get_pixel(3, 1);
        assert!(
            pixel[0] > 0 && pixel[0] < 255 && pixel[1] > 0 && pixel[1] < 255,
            "magnification must use the specified linear filter"
        );
        let mut minified = Canvas::new(8, 8, [0; 4]).unwrap();
        appearance.texcoords = Some([[0.0, 0.0], [8.0, 0.0], [0.0, 8.0]]);
        minified
            .appearance_triangle(
                vertices,
                &appearance,
                Vec3::new(0.0, 0.0, 1.0),
                Camera::new(Vec3::new(1.0, -1.0, 1.0)),
            )
            .unwrap();
        assert_eq!(
            canvas_image(minified).get_pixel(3, 1).0,
            [255; 4],
            "minification must select the distinct nearest filter"
        );
    }

    #[test]
    fn vertex_rgba_and_base_factor_multiply_before_alpha_coverage() {
        let appearance = TriangleAppearance {
            base_color: [0.5, 1.0, 0.25, 0.8],
            colors: Some([
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0, 0.5],
            ]),
            unlit: true,
            ..TriangleAppearance::default()
        };
        let color = appearance_color(
            &appearance,
            [0.25, 0.25, 0.5],
            Vec3::new(0.0, 0.0, 1.0),
            Camera::new(Vec3::new(1.0, -1.0, 1.0)),
        );
        assert_eq!(color, [0.125, 0.25, 0.125, 0.4]);
        let mut canvas = Canvas::new(8, 8, [0; 4]).unwrap();
        canvas_square(
            &mut canvas,
            0.0,
            0.0,
            &unlit([1.0, 0.0, 0.0, 0.0], AlphaMode::Opaque),
        );
        assert_eq!(canvas_image(canvas).get_pixel(4, 4).0, [255, 0, 0, 255]);
    }

    #[test]
    fn masked_texels_leave_back_geometry_and_depth_visible() {
        use std::sync::Arc;
        let texture = Texture {
            image: Arc::new(RgbaImage::from_raw(2, 1, vec![255, 0, 0, 0, 255, 0, 0, 192]).unwrap()),
            wrap_s: TextureWrap::ClampToEdge,
            wrap_t: TextureWrap::ClampToEdge,
            filter: TextureFilter::Nearest,
            min_filter: TextureFilter::Nearest,
        };
        let mask = TriangleAppearance {
            texture: Some(texture),
            alpha_mode: AlphaMode::Mask { cutoff: 0.5 },
            unlit: true,
            ..TriangleAppearance::default()
        };
        let mut canvas = Canvas::new(8, 8, [0; 4]).unwrap();
        canvas_square(&mut canvas, 1.0, 1.0, &mask);
        assert_eq!(canvas.depths[9], f32::NEG_INFINITY);
        assert_eq!(canvas.depths[14], 1.0);
        canvas_square(
            &mut canvas,
            0.0,
            0.0,
            &unlit([0.0, 1.0, 0.0, 1.0], AlphaMode::Opaque),
        );
        let image = canvas_image(canvas);
        assert_eq!(image.get_pixel(1, 1).0, [0, 255, 0, 255]);
        assert_eq!(image.get_pixel(6, 1).0, [255, 0, 0, 255]);
        let mut scene = Scene {
            triangles: vec![triangle(0.0, [255; 3])],
            ..Scene::default()
        };
        scene.appearances.insert(
            0,
            unlit([1.0, 0.0, 0.0, 0.0], AlphaMode::Mask { cutoff: 0.5 }),
        );
        assert!(
            render(&scene, &options())
                .unwrap()
                .pixels()
                .all(|pixel| pixel[3] == 0),
            "discarded triangles must not receive the legacy visible-outline fallback"
        );
    }

    #[test]
    fn blend_is_independent_of_triangle_order_and_shared_edges_are_not_doubled() {
        let red = unlit([1.0, 0.0, 0.0, 0.5], AlphaMode::Blend);
        let green = unlit([0.0, 1.0, 0.0, 0.5], AlphaMode::Blend);
        let mut outputs = Vec::new();
        for reverse in [false, true] {
            let mut canvas = Canvas::new(8, 8, [0; 4]).unwrap();
            let cards = if reverse {
                [(&red, 1.0), (&green, -1.0)]
            } else {
                [(&green, -1.0), (&red, 1.0)]
            };
            for (appearance, depth) in cards {
                canvas_square(&mut canvas, depth, depth, appearance);
            }
            outputs.push(canvas_image(canvas));
        }
        assert_eq!(outputs[0], outputs[1]);
        assert!(
            outputs[0]
                .pixels()
                .all(|pixel| pixel.0 == [213, 156, 0, 191]),
            "the shared quad diagonal must have exactly the same alpha as its interior"
        );
    }

    #[test]
    fn alpha_mask_cutoff_above_one_discards_even_fully_opaque_fragments() {
        let mask = unlit([1.0; 4], AlphaMode::Mask { cutoff: 1.1 });
        let mut canvas = Canvas::new(8, 8, [0; 4]).unwrap();
        canvas_square(&mut canvas, 1.0, 1.0, &mask);
        assert!(
            canvas
                .depths
                .iter()
                .all(|depth| *depth == f32::NEG_INFINITY)
        );
        canvas_square(
            &mut canvas,
            0.0,
            0.0,
            &unlit([0.0, 1.0, 0.0, 1.0], AlphaMode::Opaque),
        );
        assert!(
            canvas_image(canvas)
                .pixels()
                .all(|pixel| pixel.0 == [0, 255, 0, 255])
        );
        let scene = Scene {
            triangles: vec![triangle(0.0, [255; 3])],
            appearances: [(0, mask)].into_iter().collect(),
            ..Scene::default()
        };
        assert!(
            render(&scene, &options())
                .unwrap()
                .pixels()
                .all(|pixel| pixel[3] == 0)
        );
    }

    #[test]
    fn blend_orders_crossing_surfaces_per_sample_and_respects_later_opaque_geometry() {
        let red = unlit([1.0, 0.0, 0.0, 0.5], AlphaMode::Blend);
        let green = unlit([0.0, 1.0, 0.0, 0.5], AlphaMode::Blend);
        let mut canvas = Canvas::new(8, 8, [0; 4]).unwrap();
        canvas_square(&mut canvas, -1.0, 1.0, &red);
        canvas_square(&mut canvas, 0.0, 0.0, &green);
        let image = canvas_image(canvas);
        assert_eq!(image.get_pixel(1, 1).0, [156, 213, 0, 191]);
        assert_eq!(image.get_pixel(6, 1).0, [213, 156, 0, 191]);
        let mut outputs = Vec::new();
        for opaque_first in [false, true] {
            let mut canvas = Canvas::new(8, 8, [0; 4]).unwrap();
            let blue = unlit([0.0, 0.0, 1.0, 1.0], AlphaMode::Opaque);
            if opaque_first {
                canvas_square(&mut canvas, 0.0, 0.0, &blue);
            }
            canvas_square(&mut canvas, -1.0, -1.0, &green);
            canvas_square(&mut canvas, 1.0, 1.0, &red);
            if !opaque_first {
                canvas_square(&mut canvas, 0.0, 0.0, &blue);
            }
            outputs.push(canvas_image(canvas));
        }
        assert_eq!(outputs[0], outputs[1]);
        assert!(
            outputs[0]
                .pixels()
                .all(|pixel| pixel.0 == [188, 0, 188, 255])
        );
    }

    #[test]
    fn imported_normals_are_interpolated_for_lighting_and_unlit_bypasses_it() {
        let camera = Camera::new(Vec3::new(1.0, -1.0, 1.0));
        let light = Vec3::new(-0.35, -0.6, 1.0).normalized();
        let geometric = Vec3::new(0.0, 0.0, 1.0);
        let appearance = TriangleAppearance {
            normals: Some([light; 3]),
            ..TriangleAppearance::default()
        };
        let bright = appearance_color(&appearance, [0.2, 0.3, 0.5], geometric, camera);
        assert!((bright[0] - 1.0).abs() < 1e-12);
        let baseline = appearance_color(
            &TriangleAppearance::default(),
            [0.2, 0.3, 0.5],
            geometric,
            camera,
        );
        assert!(bright[0] > baseline[0]);
        let varied = TriangleAppearance {
            normals: Some([light, camera.right, light]),
            ..TriangleAppearance::default()
        };
        let a = appearance_color(&varied, [1.0, 0.0, 0.0], geometric, camera);
        let b = appearance_color(&varied, [0.0, 1.0, 0.0], geometric, camera);
        assert!(a[0] > b[0] + 0.1);
        let unlit = TriangleAppearance {
            unlit: true,
            ..varied
        };
        assert_eq!(
            appearance_color(&unlit, [0.0, 1.0, 0.0], geometric, camera),
            [1.0; 4]
        );
    }

    #[test]
    fn single_sided_material_culls_reversed_winding_and_double_sided_flips_normals() {
        let camera = Camera::new(Vec3::new(1.0, -1.0, 1.0));
        let original = triangle(0.0, [255; 3]);
        let mut reversed = original.clone();
        reversed.vertices.swap(0, 2);
        let render_face = |triangle: Triangle, appearance: TriangleAppearance| {
            render(
                &Scene {
                    triangles: vec![triangle],
                    appearances: [(0, appearance)].into_iter().collect(),
                    ..Scene::default()
                },
                &options(),
            )
            .unwrap()
        };
        let material = TriangleAppearance {
            double_sided: false,
            unlit: true,
            ..TriangleAppearance::default()
        };
        let front = render_face(original.clone(), material.clone());
        occupied(&front);
        assert!(
            render_face(reversed.clone(), material)
                .pixels()
                .all(|pixel| pixel[3] == 0)
        );
        let lit_front = TriangleAppearance {
            normals: Some([camera.toward_viewer; 3]),
            ..TriangleAppearance::default()
        };
        let lit_back = TriangleAppearance {
            normals: Some([-camera.toward_viewer; 3]),
            ..TriangleAppearance::default()
        };
        assert_eq!(
            render_face(original, lit_front),
            render_face(reversed, lit_back)
        );
        // An independently supplied normal pointing toward the camera must
        // still reverse when its geometric face is viewed from behind.
        let light = Vec3::new(-0.35, -0.6, 1.0).normalized();
        let appearance = TriangleAppearance {
            normals: Some([light; 3]),
            ..TriangleAppearance::default()
        };
        let back = appearance_color(&appearance, [1.0, 0.0, 0.0], -camera.toward_viewer, camera);
        assert_eq!(back, [0.35, 0.35, 0.35, 1.0]);
    }

    #[test]
    fn translucent_supersampling_preserves_straight_color_and_reduces_edge_alpha() {
        let mut scene = Scene {
            triangles: vec![triangle(0.0, [0; 3])],
            ..Scene::default()
        };
        scene
            .appearances
            .insert(0, unlit([1.0, 0.0, 0.0, 0.5], AlphaMode::Blend));
        let image = render(&scene, &options()).unwrap();
        assert_eq!(image.get_pixel(32, 32).0, [255, 0, 0, 128]);
        assert!(image.pixels().any(|pixel| pixel[3] > 0 && pixel[3] < 128));
        for pixel in image.pixels().filter(|pixel| pixel[3] > 0) {
            assert_eq!(&pixel.0[..3], &[255, 0, 0]);
        }
        let mut canvas = Canvas::new(8, 8, [0; 4]).unwrap();
        canvas.appearance_samples = MAX_APPEARANCE_SAMPLES;
        assert!(
            canvas
                .appearance_triangle(
                    [
                        Vec3::new(0.0, 0.0, 0.0),
                        Vec3::new(8.0, 0.0, 0.0),
                        Vec3::new(0.0, 8.0, 0.0)
                    ],
                    &TriangleAppearance::default(),
                    Vec3::new(0.0, 0.0, 1.0),
                    Camera::new(Vec3::new(1.0, -1.0, 1.0))
                )
                .is_err()
        );
    }

    #[test]
    fn appearance_supersampling_resolves_black_white_edges_in_linear_premultiplied_color() {
        let output = RenderOptions {
            width: 1,
            height: 1,
            supersampling: 2,
            ..options()
        };
        let samples = vec![
            255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255,
        ];
        for (linear_supersampling, expected) in [(false, 128), (true, 188)] {
            let mut canvas = Canvas::new(2, 2, [0; 4]).unwrap();
            canvas.pixels = samples.clone();
            canvas.linear_supersampling = linear_supersampling;
            assert_eq!(
                canvas.into_image(&output).unwrap().get_pixel(0, 0).0,
                [expected, expected, expected, 255]
            );
        }
        let mut canvas = Canvas::new(2, 2, [0; 4]).unwrap();
        canvas.linear_supersampling = true;
        canvas.pixels = vec![
            255, 255, 255, 255, 0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0,
        ];
        assert_eq!(
            canvas.into_image(&output).unwrap().get_pixel(0, 0).0,
            [255, 255, 255, 128],
            "transparent samples must affect coverage, without darkening visible samples"
        );
    }
}
