//! Deterministic, headless thumbnail rendering without graphics drivers or native libraries.

use crate::scene::{Scene, Triangle, Vec3};
use image::RgbaImage;
use std::io::Write;

const MAX_DIMENSION: u32 = 8192;
const MAX_SAMPLE_PIXELS: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub width: u32,
    pub height: u32,
    pub transparent: bool,
    pub background: [u8; 3],
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
struct Bounds {
    min: Vec3,
    max: Vec3,
}

impl Bounds {
    fn empty() -> Self {
        Self {
            min: Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            max: Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
        }
    }

    fn include(&mut self, p: Vec3) {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.min.z = self.min.z.min(p.z);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
        self.max.z = self.max.z.max(p.z);
    }

    fn center(self) -> Vec3 {
        self.min + (self.max - self.min) * 0.5
    }
}

#[derive(Clone, Copy)]
struct Geometry {
    coordinate_scale: f64,
    center: Vec3,
    extent: f64,
}

impl Geometry {
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

    fn position(self, p: Vec3) -> Vec3 {
        (p / self.coordinate_scale - self.center) / self.extent
    }

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
struct Camera {
    right: Vec3,
    up: Vec3,
    toward_viewer: Vec3,
}

impl Camera {
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

    fn project(self, p: Vec3) -> Vec3 {
        Vec3::new(
            p.dot(self.right),
            -p.dot(self.up),
            p.dot(self.toward_viewer),
        )
    }

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
struct Projection {
    geometry: Geometry,
    camera: Camera,
    center: Vec3,
    scale: f64,
    image_center: Vec3,
}

impl Projection {
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

    fn project(self, world: Vec3) -> Vec3 {
        let p = self.camera.project(world);
        Vec3::new(
            (p.x - self.center.x) * self.scale + self.image_center.x,
            (p.y - self.center.y) * self.scale + self.image_center.y,
            p.z,
        )
    }
}

struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    depths: Vec<f32>,
}

impl Canvas {
    fn new(width: u32, height: u32, background: [u8; 4]) -> crate::Result<Self> {
        let count = width as usize * height as usize;
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(count * 4)?;
        pixels.resize(count * 4, 0);
        for pixel in pixels.chunks_exact_mut(4) {
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
        })
    }

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
                for sy in 0..samples {
                    for sx in 0..samples {
                        let offset = ((y * samples + sy) as usize * self.width as usize
                            + (x * samples + sx) as usize)
                            * 4;
                        let a = u32::from(self.pixels[offset + 3]);
                        alpha += a;
                        for (channel, sum) in colors.iter_mut().enumerate() {
                            *sum += u32::from(self.pixels[offset + channel]) * a;
                        }
                    }
                }
                let offset = (y as usize * options.width as usize + x as usize) * 4;
                // Average premultiplied colors, then unpremultiply. This
                // keeps translucent edge pixels free of dark halos.
                for (channel, sum) in colors.iter().enumerate() {
                    output[offset + channel] =
                        (sum + alpha / 2).checked_div(alpha).unwrap_or(0) as u8;
                }
                let count = samples * samples;
                output[offset + 3] = ((alpha + count / 2) / count) as u8;
            }
        }
        RgbaImage::from_raw(options.width, options.height, output)
            .ok_or_else(|| "invalid rendered image dimensions".into())
    }
}

fn edge(a: Vec3, b: Vec3, p: Vec3) -> f64 {
    (p.x - a.x) * (b.y - a.y) - (p.y - a.y) * (b.x - a.x)
}

fn shade(color: [u8; 3], normal: Vec3, camera: Camera) -> [u8; 3] {
    let light = Vec3::new(-0.35, -0.6, 1.0).normalized();
    let facing = normal.dot(camera.toward_viewer);
    let normal = if facing < 0.0 || (facing == 0.0 && normal.dot(light) < 0.0) {
        -normal
    } else {
        normal
    };
    let intensity = 0.35 + 0.65 * normal.dot(light).max(0.0);
    color.map(|channel| (f64::from(channel) * intensity).round() as u8)
}

/// Render triangles and point clouds from a fixed, orthographic isometric view.
///
/// A deterministic alternate view is used for a mesh whose surfaces are all
/// edge-on. Triangle winding never hides a surface. Output has straight alpha.
/// The internal render is limited to 16 million sample pixels.
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
    let mut fragments = 0;
    for triangle in &scene.triangles {
        if let Some((vertices, normal)) = geometry.triangle(triangle) {
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
        for triangle in &scene.triangles {
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
    canvas.into_image(options)
}

/// Write a rendered thumbnail as PNG, including its alpha channel.
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
                triangles: vec![near.clone(), far.clone()],
                points: vec![],
            },
            &options(),
        )
        .unwrap();
        let backward = render(
            &Scene {
                lines: Vec::new(),
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
                triangles: vec![original],
                points: vec![],
            },
            &options(),
        )
        .unwrap();
        let second = render(
            &Scene {
                lines: Vec::new(),
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
        assert!(!edges.is_empty());
        for pixel in edges {
            assert_eq!(&pixel.0[..3], &solid.0[..3]);
        }
    }

    #[test]
    fn single_point_and_point_depth_are_supported() {
        let camera = Camera::new(Vec3::new(1.0, -1.0, 1.0));
        let scene = Scene {
            lines: Vec::new(),
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
            triangles: vec![],
            points: vec![Point {
                position: Vec3::new(f64::NAN, 0.0, 0.0),
                color: None,
            }],
        };
        assert!(render(&nonfinite, &options()).is_err());
        let scene = Scene {
            lines: Vec::new(),
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
}
