//! Bounded BSP operations on closed, consistently oriented triangle solids.
use crate::scene::{Triangle, Vec3};
use crate::{Error, Result};
/// Maximum BSP polygon count retained during a Boolean operation.
const MAX_POLYGONS: usize = 100_000;
/// Maximum recursive reference, geometry or partition depth.
const MAX_DEPTH: usize = 256;
/// Maximum charged spline or Boolean-processing work.
const MAX_WORK: usize = 10_000_000;
/// Construct a solid-operation error from its diagnostic text.
fn error(message: &str) -> Error {
    message.into()
}
/// Oriented BSP plane defined by a unit normal and signed distance.
#[derive(Clone, Copy)]
struct Plane {
    /// Oriented unit normal of the polygon partition plane.
    normal: Vec3,
    /// Signed plane distance satisfying normal dot point equals distance.
    distance: f64,
}
impl Plane {
    /// Reverse the oriented surface's normal and sidedness.
    fn flipped(self) -> Self {
        Self {
            normal: self.normal * -1.,
            distance: -self.distance,
        }
    }
}
/// An oriented convex polygon retaining its source triangle color.
struct Polygon {
    /// Ordered spatial vertices defining this polygon boundary.
    vertices: Vec<Vec3>,
    /// Oriented plane supporting this convex polygon.
    plane: Plane,
    /// Optional inherited RGB surface or line color.
    color: Option<[u8; 3]>,
}
impl Polygon {
    /// Reverse the oriented surface's normal and sidedness.
    fn flipped(mut self) -> Self {
        self.vertices.reverse();
        self.plane = self.plane.flipped();
        self
    }
}
/// Numerical coincidence tolerance and shared Boolean-operation work counter.
struct Budget {
    /// Distance tolerance for classifying points on a partition plane.
    epsilon: f64,
    /// Accumulated processing work charged against the preview budget.
    work: usize,
}
impl Budget {
    /// Charge polygon-processing work against the shared Boolean-operation budget.
    ///
    /// # Errors
    ///
    /// The accumulated work overflows or exceeds the limit.
    fn use_work(&mut self, n: usize) -> Result<()> {
        self.work = self
            .work
            .checked_add(n)
            .ok_or_else(|| error("IFC solid operation work overflow"))?;
        if self.work > MAX_WORK {
            return Err(error("IFC solid operation exceeds its work limit"));
        }
        Ok(())
    }
}
/// Polygon buckets classified relative to one BSP plane.
struct Split {
    /// Coplanar polygons whose normal agrees with the partition plane.
    coplanar_front: Vec<Polygon>,
    /// Coplanar polygons whose normal opposes the partition plane.
    coplanar_back: Vec<Polygon>,
    /// Polygons or subtree in front of the partition plane.
    front: Vec<Polygon>,
    /// Polygons or subtree behind the partition plane.
    back: Vec<Polygon>,
}
/// Classify and split a polygon against a BSP plane.
///
/// # Errors
///
/// Intersection coordinates overflow or work/polygon limits are exceeded.
fn split(plane: Plane, polygon: Polygon, budget: &mut Budget, out: &mut Split) -> Result<()> {
    budget.use_work(polygon.vertices.len())?;
    let distances: Vec<_> = polygon
        .vertices
        .iter()
        .map(|p| plane.normal.dot(*p) - plane.distance)
        .collect();
    let side: Vec<_> = distances
        .iter()
        .map(|d| {
            if *d > budget.epsilon {
                1u8
            } else if *d < -budget.epsilon {
                2
            } else {
                0
            }
        })
        .collect();
    match side.iter().fold(0, |a, b| a | b) {
        0 => {
            if plane.normal.dot(polygon.plane.normal) >= 0. {
                out.coplanar_front.push(polygon);
            } else {
                out.coplanar_back.push(polygon);
            }
        }
        1 => out.front.push(polygon),
        2 => out.back.push(polygon),
        _ => {
            let mut front = Vec::new();
            let mut back = Vec::new();
            for i in 0..polygon.vertices.len() {
                let next = (i + 1) % polygon.vertices.len();
                let point = polygon.vertices[i];
                if side[i] != 2 {
                    front.push(point);
                }
                if side[i] != 1 {
                    back.push(point);
                }
                if (side[i] | side[next]) == 3 {
                    let ratio = distances[i] / (distances[i] - distances[next]);
                    let p = point + (polygon.vertices[next] - point) * ratio;
                    if !p.is_finite() {
                        return Err(error("IFC solid plane intersection overflow"));
                    }
                    front.push(p);
                    back.push(p);
                }
            }
            if front.len() >= 3 {
                out.front.push(Polygon {
                    vertices: front,
                    plane: polygon.plane,
                    color: polygon.color,
                });
            }
            if back.len() >= 3 {
                out.back.push(Polygon {
                    vertices: back,
                    plane: polygon.plane,
                    color: polygon.color,
                });
            }
        }
    }
    Ok(())
}
impl Split {
    /// Create empty front/back and coplanar polygon buckets.
    fn new() -> Self {
        Self {
            coplanar_front: Vec::new(),
            coplanar_back: Vec::new(),
            front: Vec::new(),
            back: Vec::new(),
        }
    }
    /// Count polygons across every split bucket.
    fn count(&self) -> usize {
        self.coplanar_front.len() + self.coplanar_back.len() + self.front.len() + self.back.len()
    }
}
/// One bounded BSP partition with coplanar polygons and front/back children.
#[derive(Default)]
struct Node {
    /// Oriented plane supporting this convex polygon.
    plane: Option<Plane>,
    /// Coplanar polygons stored at this BSP partition.
    polygons: Vec<Polygon>,
    /// Polygons or subtree in front of the partition plane.
    front: Option<Box<Node>>,
    /// Polygons or subtree behind the partition plane.
    back: Option<Box<Node>>,
}
impl Node {
    /// Insert polygons into a bounded binary-space-partition tree.
    ///
    /// # Errors
    ///
    /// Polygon count, processing work or tree depth exceeds supported limits.
    fn build(&mut self, polygons: Vec<Polygon>, budget: &mut Budget, depth: usize) -> Result<()> {
        if polygons.is_empty() {
            return Ok(());
        }
        if depth > MAX_DEPTH || polygons.len() > MAX_POLYGONS {
            return Err(error("IFC solid BSP depth/polygon limit exceeded"));
        }
        let plane = *self.plane.get_or_insert(polygons[0].plane);
        let mut out = Split::new();
        for p in polygons {
            split(plane, p, budget, &mut out)?;
        }
        if self.polygons.len() + out.count() > MAX_POLYGONS {
            return Err(error("IFC solid polygon limit exceeded"));
        }
        self.polygons.extend(out.coplanar_front);
        self.polygons.extend(out.coplanar_back);
        if !out.front.is_empty() {
            self.front
                .get_or_insert_with(Default::default)
                .build(out.front, budget, depth + 1)?;
        }
        if !out.back.is_empty() {
            self.back
                .get_or_insert_with(Default::default)
                .build(out.back, budget, depth + 1)?;
        }
        Ok(())
    }
    /// Reverse every polygon/plane and exchange front/back BSP subtrees.
    fn invert(&mut self) {
        self.plane = self.plane.map(Plane::flipped);
        self.polygons = std::mem::take(&mut self.polygons)
            .into_iter()
            .map(Polygon::flipped)
            .collect();
        if let Some(child) = &mut self.front {
            child.invert();
        }
        if let Some(child) = &mut self.back {
            child.invert();
        }
        std::mem::swap(&mut self.front, &mut self.back);
    }
    /// Clip polygons through the BSP tree and return the surviving set.
    ///
    /// # Errors
    ///
    /// Splitting exceeds numeric, polygon, work or depth bounds.
    fn clip(
        &self,
        polygons: Vec<Polygon>,
        budget: &mut Budget,
        depth: usize,
    ) -> Result<Vec<Polygon>> {
        if depth > MAX_DEPTH {
            return Err(error("IFC solid clipping depth exceeded"));
        }
        let Some(plane) = self.plane else {
            return Ok(polygons);
        };
        let mut out = Split::new();
        for p in polygons {
            split(plane, p, budget, &mut out)?;
        }
        if out.count() > MAX_POLYGONS {
            return Err(error("IFC solid clipping polygon count exceeded"));
        }
        out.front.extend(out.coplanar_front);
        out.back.extend(out.coplanar_back);
        let mut front = if let Some(child) = &self.front {
            child.clip(out.front, budget, depth + 1)?
        } else {
            out.front
        };
        if let Some(child) = &self.back {
            front.extend(child.clip(out.back, budget, depth + 1)?);
        }
        if front.len() > MAX_POLYGONS {
            return Err(error("IFC solid clipping polygon count exceeded"));
        }
        Ok(front)
    }
    /// Clip this tree's polygons against another solid's BSP tree.
    ///
    /// # Errors
    ///
    /// Clipping exceeds numeric, polygon, work or depth bounds.
    fn clip_to(&mut self, other: &Self, budget: &mut Budget) -> Result<()> {
        self.polygons = other.clip(std::mem::take(&mut self.polygons), budget, 0)?;
        if let Some(child) = &mut self.front {
            child.clip_to(other, budget)?;
        }
        if let Some(child) = &mut self.back {
            child.clip_to(other, budget)?;
        }
        Ok(())
    }
    /// Collect all tree polygons while enforcing the output limit.
    ///
    /// # Errors
    ///
    /// The collected polygon count exceeds its limit.
    fn take_polygons(self, out: &mut Vec<Polygon>) -> Result<()> {
        out.extend(self.polygons);
        if out.len() > MAX_POLYGONS {
            return Err(error("IFC solid output polygon count exceeded"));
        }
        if let Some(child) = self.front {
            child.take_polygons(out)?;
        }
        if let Some(child) = self.back {
            child.take_polygons(out)?;
        }
        Ok(())
    }
}
/// Convert valid triangle faces to oriented BSP polygons.
///
/// # Errors
///
/// Triangle count exceeds the limit or geometry is non-finite/degenerate.
fn polygons(triangles: Vec<Triangle>) -> Result<Vec<Polygon>> {
    if triangles.len() > MAX_POLYGONS {
        return Err(error("IFC solid operands exceed 100000 triangles"));
    }
    triangles
        .into_iter()
        .map(|t| {
            let normal = (t.vertices[1] - t.vertices[0]).cross(t.vertices[2] - t.vertices[0]);
            let length = normal.length();
            if !length.is_finite() || length == 0. {
                return Err(error("invalid IFC solid triangle"));
            }
            let normal = normal / length;
            Ok(Polygon {
                plane: Plane {
                    normal,
                    distance: normal.dot(t.vertices[0]),
                },
                vertices: t.vertices.to_vec(),
                color: t.color,
            })
        })
        .collect()
}
/// Evaluate a supported union, intersection or difference of closed triangle solids.
///
/// # Errors
///
/// The operator/operands are invalid or BSP/numeric/geometry budgets are exceeded.
pub(super) fn boolean(
    mut a: Vec<Triangle>,
    mut b: Vec<Triangle>,
    operator: &str,
) -> Result<Vec<Triangle>> {
    if !matches!(operator, "UNION" | "DIFFERENCE" | "INTERSECTION") {
        return Err(error("unsupported IFC Boolean operator"));
    }
    if a.is_empty() || b.is_empty() {
        return Ok(match operator {
            "UNION" => {
                if a.is_empty() {
                    b
                } else {
                    a
                }
            }
            "DIFFERENCE" => a,
            _ => Vec::new(),
        });
    }
    let origin = a[0].vertices[0];
    let mut span = 0_f64;
    for p in a.iter().chain(&b).flat_map(|t| t.vertices) {
        span = span.max((p - origin).length());
    }
    if !span.is_finite() || span == 0. {
        return Err(error("invalid IFC Boolean operand extent"));
    }
    for triangle in a.iter_mut().chain(&mut b) {
        triangle.vertices = triangle.vertices.map(|p| p - origin);
    }
    let mut budget = Budget {
        epsilon: span * 1e-8,
        work: 0,
    };
    let mut left = Node::default();
    left.build(polygons(a)?, &mut budget, 0)?;
    let mut right = Node::default();
    right.build(polygons(b)?, &mut budget, 0)?;
    match operator {
        "UNION" => {
            left.clip_to(&right, &mut budget)?;
            right.clip_to(&left, &mut budget)?;
            right.invert();
            right.clip_to(&left, &mut budget)?;
            right.invert();
        }
        "DIFFERENCE" => {
            left.invert();
            left.clip_to(&right, &mut budget)?;
            right.clip_to(&left, &mut budget)?;
            right.invert();
            right.clip_to(&left, &mut budget)?;
            right.invert();
        }
        _ => {
            left.invert();
            right.clip_to(&left, &mut budget)?;
            right.invert();
            left.clip_to(&right, &mut budget)?;
            right.clip_to(&left, &mut budget)?;
        }
    }
    let mut other = Vec::new();
    right.take_polygons(&mut other)?;
    left.build(other, &mut budget, 0)?;
    if operator != "UNION" {
        left.invert();
    }
    let mut output = Vec::new();
    left.take_polygons(&mut output)?;
    let mut triangles = Vec::new();
    for p in output {
        for i in 1..p.vertices.len() - 1 {
            let vertices = [p.vertices[0], p.vertices[i], p.vertices[i + 1]];
            if (vertices[1] - vertices[0])
                .cross(vertices[2] - vertices[0])
                .length()
                > budget.epsilon.powi(2)
            {
                triangles.push(Triangle {
                    vertices: vertices.map(|p| p + origin),
                    color: p.color,
                });
                if triangles.len() > MAX_POLYGONS {
                    return Err(error("IFC solid output triangle count exceeded"));
                }
            }
        }
    }
    Ok(triangles)
}
