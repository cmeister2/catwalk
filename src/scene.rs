use std::ops::{Add, Div, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
    pub fn dot(self, b: Self) -> f64 {
        self.x * b.x + self.y * b.y + self.z * b.z
    }
    pub fn cross(self, b: Self) -> Self {
        Self::new(
            self.y * b.z - self.z * b.y,
            self.z * b.x - self.x * b.z,
            self.x * b.y - self.y * b.x,
        )
    }
    pub fn length(self) -> f64 {
        self.x.hypot(self.y).hypot(self.z)
    }
    pub fn normalized(self) -> Self {
        let length = self.length();
        if length > 0.0 && length.is_finite() {
            self / length
        } else {
            Self::default()
        }
    }
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
        Self::new(v[0] as f64, v[1] as f64, v[2] as f64)
    }
}

#[derive(Clone, Debug)]
pub struct Triangle {
    pub vertices: [Vec3; 3],
    pub color: Option<[u8; 3]>,
}

#[derive(Clone, Debug)]
pub struct Point {
    pub position: Vec3,
    pub color: Option<[u8; 3]>,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub vertices: [Vec3; 2],
    pub color: Option<[u8; 3]>,
}

/// Coordinates use Z up. Importers convert formats with other conventions.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub triangles: Vec<Triangle>,
    pub points: Vec<Point>,
    pub lines: Vec<Line>,
}

impl Scene {
    pub fn validate(&self) -> crate::Result<()> {
        if self.triangles.is_empty() && self.points.is_empty() && self.lines.is_empty() {
            return Err("model contains no renderable geometry".into());
        }
        if self.positions().any(|p| !p.is_finite()) {
            return Err("model contains non-finite coordinates".into());
        }
        Ok(())
    }
    pub fn positions(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.triangles
            .iter()
            .flat_map(|t| t.vertices)
            .chain(self.points.iter().map(|p| p.position))
            .chain(self.lines.iter().flat_map(|line| line.vertices))
    }
}
