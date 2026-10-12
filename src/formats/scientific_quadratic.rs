//! Owned fixed-node VTK interpolation and role-preserving curved boundary extraction.
//!
//! Node roles follow the official VTK cell definitions:
//! <https://github.com/Kitware/VTK/tree/v9.5.2/Common/DataModel>.
use super::{MAX_ITEMS, Scene, Triangle, Vec3};
use crate::Result;
use crate::scene::Line;
use std::collections::BTreeMap;

/// Uniform quadratic surface-edge subdivisions, including midpoints and triangle centroids.
const SUBDIVISIONS: usize = 6;
/// Curve subdivisions include quadratic midpoints and both cubic third-points.
const LINE_SUBDIVISIONS: usize = 12;

/// Conservative new descriptor, temporary and reserved scene-payload accounting.
#[derive(Default)]
pub(super) struct Storage {
    /// Cumulative precharged bytes, including conservative map allocation overhead.
    bytes: usize,
}

impl Storage {
    /// Preflight additional storage before allocating its payload.
    ///
    /// # Errors
    /// Arithmetic overflows or the common 512 MiB importer storage ceiling is exceeded.
    pub(super) fn charge(&mut self, bytes: usize) -> Result<()> {
        let total = self
            .bytes
            .checked_add(bytes)
            .ok_or("VTK storage size overflow")?;
        if total as u64 > super::super::MAX_INPUT_BYTES {
            return Err("VTK topology/output storage exceeds the 512 MiB limit".into());
        }
        self.bytes = total;
        Ok(())
    }
}

/// Supported interpolating surface basis and its VTK node ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// Three corner nodes and three consecutive edge midpoints.
    Triangle,
    /// A quadratic triangle enriched with an authored centroid node.
    TriangleCenter,
    /// Four corners and four consecutive edge midpoints, using serendipity interpolation.
    Quad,
    /// A tensor-product quadratic quadrilateral with its authored center node.
    QuadCenter,
    /// Quadratic horizontal edges and linear vertical edges, with two midpoint nodes.
    QuadLinear,
}

impl Kind {
    /// Number of authored nodes in this surface basis.
    fn nodes(self) -> usize {
        match self {
            Self::Triangle | Self::QuadLinear => 6,
            Self::TriangleCenter => 7,
            Self::Quad => 8,
            Self::QuadCenter => 9,
        }
    }

    /// Number of emitted triangles at the fixed approximation level.
    pub(super) fn triangles(self) -> usize {
        match self {
            Self::Triangle | Self::TriangleCenter => SUBDIVISIONS * SUBDIVISIONS,
            Self::Quad | Self::QuadCenter => 2 * SUBDIVISIONS * SUBDIVISIONS,
            Self::QuadLinear => 2 * SUBDIVISIONS,
        }
    }
}

/// One bounded surface retaining its authored VTK corner, midpoint and center roles.
#[derive(Clone, Copy, Debug)]
pub(super) struct Surface {
    /// The interpolating basis, including absent midpoint or center roles.
    pub(super) kind: Kind,
    /// Authored node IDs in VTK order; unused trailing slots are zero.
    nodes: [usize; 9],
}

impl Surface {
    /// Construct a fixed-node surface without allocating per-face node lists.
    ///
    /// # Errors
    /// The node count does not match the selected basis.
    pub(super) fn new(kind: Kind, nodes: &[usize]) -> Result<Self> {
        if nodes.len() != kind.nodes() {
            return Err("invalid VTK curved surface node count".into());
        }
        let mut result = Self {
            kind,
            nodes: [0; 9],
        };
        result.nodes[..nodes.len()].copy_from_slice(nodes);
        Ok(result)
    }

    /// Map a verified local volume-face layout to its global authored node IDs.
    ///
    /// # Errors
    /// A local node role is outside the enclosing cell or the face basis is inconsistent.
    pub(super) fn local(kind: Kind, cell: &[usize], roles: &[usize]) -> Result<Self> {
        let mut nodes = [0; 9];
        if roles.len() != kind.nodes() {
            return Err("invalid VTK volume-face role count".into());
        }
        for (slot, &role) in nodes.iter_mut().zip(roles) {
            *slot = *cell
                .get(role)
                .ok_or("VTK volume-face role is outside its cell")?;
        }
        Self::new(kind, &nodes[..roles.len()])
    }

    /// Canonicalize corners and their attached roles under cyclic rotation and reflection.
    fn key(self) -> FaceKey {
        let corners = if matches!(self.kind, Kind::Triangle | Kind::TriangleCenter) {
            3
        } else {
            4
        };
        let edges = if self.kind == Kind::QuadLinear {
            [Some(self.nodes[4]), None, Some(self.nodes[5]), None]
        } else {
            std::array::from_fn(|i| (i < corners).then_some(self.nodes[corners + i]))
        };
        let center = match self.kind {
            Kind::TriangleCenter => Some(self.nodes[6]),
            Kind::QuadCenter => Some(self.nodes[8]),
            _ => None,
        };
        let mut least = FaceKey {
            corners,
            roles: [None; 9],
        };
        for start in 0..corners {
            for reflected in [false, true] {
                let mut candidate = FaceKey {
                    corners,
                    roles: [None; 9],
                };
                for i in 0..corners {
                    let corner = if reflected {
                        (start + corners - i) % corners
                    } else {
                        (start + i) % corners
                    };
                    let edge = if reflected {
                        (corner + corners - 1) % corners
                    } else {
                        corner
                    };
                    candidate.roles[i] = Some(self.nodes[corner]);
                    candidate.roles[4 + i] = edges[edge];
                }
                candidate.roles[8] = center;
                if (start == 0 && !reflected) || candidate < least {
                    least = candidate;
                }
            }
        }
        least
    }

    /// Evaluate one lattice point, returning authored interpolation nodes exactly at their sites.
    ///
    /// # Errors
    /// A node index is invalid or the interpolated position is not finite.
    fn sample(self, positions: &[Vec3], column: usize, row: usize) -> Result<Vec3> {
        let subdivisions = SUBDIVISIONS;
        let triangular = matches!(self.kind, Kind::Triangle | Kind::TriangleCenter);
        let height = if self.kind == Kind::QuadLinear {
            1
        } else {
            subdivisions
        };
        let authored = if triangular {
            match (column, row) {
                (0, 0) => Some(0),
                (column, 0) if column == subdivisions => Some(1),
                (0, row) if row == subdivisions => Some(2),
                (column, 0) if column == subdivisions / 2 => Some(3),
                (column, row) if column == subdivisions / 2 && row == subdivisions / 2 => Some(4),
                (0, row) if row == subdivisions / 2 => Some(5),
                (column, row)
                    if self.kind == Kind::TriangleCenter
                        && column == subdivisions / 3
                        && row == subdivisions / 3 =>
                {
                    Some(6)
                }
                _ => None,
            }
        } else {
            match (column, row) {
                (0, 0) => Some(0),
                (column, 0) if column == subdivisions => Some(1),
                (column, row) if column == subdivisions && row == height => Some(2),
                (0, row) if row == height => Some(3),
                (column, 0) if column == subdivisions / 2 => Some(4),
                (column, row) if column == subdivisions / 2 && row == height => {
                    Some(if self.kind == Kind::QuadLinear { 5 } else { 6 })
                }
                (column, row)
                    if self.kind != Kind::QuadLinear
                        && column == subdivisions
                        && row == subdivisions / 2 =>
                {
                    Some(5)
                }
                (0, row) if self.kind != Kind::QuadLinear && row == subdivisions / 2 => Some(7),
                (column, row)
                    if self.kind == Kind::QuadCenter
                        && column == subdivisions / 2
                        && row == subdivisions / 2 =>
                {
                    Some(8)
                }
                _ => None,
            }
        };
        if let Some(node) = authored {
            return finite_position(positions, self.nodes[node]);
        }
        let horizontal = f64::from(u32::try_from(column)?) / 6.;
        let vertical = f64::from(u32::try_from(row)?) / if height == 1 { 1. } else { 6. };
        interpolate(
            positions,
            &self.nodes[..self.kind.nodes()],
            &weights(self.kind, horizontal, vertical),
        )
    }

    /// Append a fixed-resolution curved surface after the caller preflights its complete output.
    ///
    /// # Errors
    /// A sampled point is invalid or the primitive count would exceed the common ceiling.
    pub(super) fn append(self, positions: &[Vec3], scene: &mut Scene) -> Result<()> {
        if scene
            .triangles
            .len()
            .checked_add(self.kind.triangles())
            .is_none_or(|n| n > MAX_ITEMS)
        {
            return Err("VTK curved surface exceeds output triangle limit".into());
        }
        if matches!(self.kind, Kind::Triangle | Kind::TriangleCenter) {
            for j in 0..SUBDIVISIONS {
                for i in 0..SUBDIVISIONS - j {
                    let a = self.sample(positions, i, j)?;
                    let b = self.sample(positions, i + 1, j)?;
                    let c = self.sample(positions, i, j + 1)?;
                    scene.triangles.push(Triangle {
                        vertices: [a, b, c],
                        color: None,
                    });
                    if i + j + 1 < SUBDIVISIONS {
                        let d = self.sample(positions, i + 1, j + 1)?;
                        scene.triangles.push(Triangle {
                            vertices: [b, d, c],
                            color: None,
                        });
                    }
                }
            }
        } else {
            let height = if self.kind == Kind::QuadLinear {
                1
            } else {
                SUBDIVISIONS
            };
            for j in 0..height {
                for i in 0..SUBDIVISIONS {
                    let a = self.sample(positions, i, j)?;
                    let b = self.sample(positions, i + 1, j)?;
                    let c = self.sample(positions, i + 1, j + 1)?;
                    let d = self.sample(positions, i, j + 1)?;
                    scene.triangles.push(Triangle {
                        vertices: [a, b, c],
                        color: None,
                    });
                    scene.triangles.push(Triangle {
                        vertices: [a, c, d],
                        color: None,
                    });
                }
            }
        }
        Ok(())
    }
}

/// Canonical face identity includes corner count, attached edge nodes, missing roles and center.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FaceKey {
    /// Three or four corner roles in the face.
    corners: usize,
    /// Four corner slots, four midpoint slots and an optional face-center slot.
    roles: [Option<usize>; 9],
}

/// Shared curved volume faces retaining the first occurrence's outward node order.
#[derive(Default)]
pub(super) struct Faces {
    /// Canonical face identities, occurrence counts and the first authored surface.
    faces: BTreeMap<FaceKey, (u8, Surface)>,
}

impl Faces {
    /// Register one face with role-preserving cancellation and bounded descriptor storage.
    ///
    /// # Errors
    /// More than two volumes share the same full face or descriptor storage is exhausted.
    pub(super) fn insert(&mut self, surface: Surface, storage: &mut Storage) -> Result<()> {
        let key = surface.key();
        if let Some((count, _)) = self.faces.get_mut(&key) {
            if *count == 2 {
                return Err("VTK grid contains a non-manifold curved volume face".into());
            }
            *count += 1;
        } else {
            if self.faces.len() >= MAX_ITEMS {
                return Err("VTK curved face count exceeds geometry limit".into());
            }
            storage.charge(2 * std::mem::size_of::<(FaceKey, (u8, Surface))>() + 128)?;
            self.faces.insert(key, (1, surface));
        }
        Ok(())
    }

    /// Iterate only post-cancellation exterior surfaces in deterministic canonical order.
    pub(super) fn visible(&self) -> impl Iterator<Item = Surface> + '_ {
        self.faces
            .values()
            .filter_map(|&(count, face)| (count == 1).then_some(face))
    }
}

/// Exact fixed-node cardinality for the standard supported nonlinear cell family.
pub(super) fn node_count(cell_type: usize) -> Option<usize> {
    Some(match cell_type {
        21 => 3,
        22 | 30 => 6,
        23 => 8,
        24 => 10,
        25 => 20,
        26 => 15,
        27 => 13,
        28 => 9,
        29 => 27,
        31 => 12,
        32 => 18,
        33 => 24,
        34 => 7,
        35 => 4,
        37 => 19,
        _ => return None,
    })
}

/// Surface basis for a standalone fixed-node nonlinear surface cell.
pub(super) fn surface_kind(cell_type: usize) -> Option<Kind> {
    match cell_type {
        22 => Some(Kind::Triangle),
        23 => Some(Kind::Quad),
        28 => Some(Kind::QuadCenter),
        30 => Some(Kind::QuadLinear),
        34 => Some(Kind::TriangleCenter),
        _ => None,
    }
}

/// Outward VTK face-node layouts; unused volume-interior nodes still require input validation.
#[allow(
    clippy::too_many_lines,
    reason = "Keep the independently verified fixed-node volume-face role tables together."
)]
pub(super) fn volume_layout(cell_type: usize) -> &'static [(Kind, &'static [usize])] {
    use Kind::{Quad, QuadCenter, QuadLinear, Triangle, TriangleCenter};
    match cell_type {
        24 => &[
            (Triangle, &[0, 1, 3, 4, 8, 7]),
            (Triangle, &[1, 2, 3, 5, 9, 8]),
            (Triangle, &[2, 0, 3, 6, 7, 9]),
            (Triangle, &[0, 2, 1, 6, 5, 4]),
        ],
        25 => &[
            (Quad, &[0, 4, 7, 3, 16, 15, 19, 11]),
            (Quad, &[1, 2, 6, 5, 9, 18, 13, 17]),
            (Quad, &[0, 1, 5, 4, 8, 17, 12, 16]),
            (Quad, &[3, 7, 6, 2, 19, 14, 18, 10]),
            (Quad, &[0, 3, 2, 1, 11, 10, 9, 8]),
            (Quad, &[4, 5, 6, 7, 12, 13, 14, 15]),
        ],
        26 => &[
            (Triangle, &[0, 2, 1, 8, 7, 6]),
            (Triangle, &[3, 4, 5, 9, 10, 11]),
            (Quad, &[0, 1, 4, 3, 6, 13, 9, 12]),
            (Quad, &[1, 2, 5, 4, 7, 14, 10, 13]),
            (Quad, &[2, 0, 3, 5, 8, 12, 11, 14]),
        ],
        27 => &[
            (Quad, &[0, 3, 2, 1, 8, 7, 6, 5]),
            (Triangle, &[0, 1, 4, 5, 10, 9]),
            (Triangle, &[1, 2, 4, 6, 11, 10]),
            (Triangle, &[2, 3, 4, 7, 12, 11]),
            (Triangle, &[3, 0, 4, 8, 9, 12]),
        ],
        29 => &[
            (QuadCenter, &[0, 4, 7, 3, 16, 15, 19, 11, 20]),
            (QuadCenter, &[1, 2, 6, 5, 9, 18, 13, 17, 21]),
            (QuadCenter, &[0, 1, 5, 4, 8, 17, 12, 16, 22]),
            (QuadCenter, &[3, 7, 6, 2, 19, 14, 18, 10, 23]),
            (QuadCenter, &[0, 3, 2, 1, 11, 10, 9, 8, 24]),
            (QuadCenter, &[4, 5, 6, 7, 12, 13, 14, 15, 25]),
        ],
        31 => &[
            (Triangle, &[0, 2, 1, 8, 7, 6]),
            (Triangle, &[3, 4, 5, 9, 10, 11]),
            (QuadLinear, &[0, 1, 4, 3, 6, 9]),
            (QuadLinear, &[1, 2, 5, 4, 7, 10]),
            (QuadLinear, &[2, 0, 3, 5, 8, 11]),
        ],
        32 => &[
            (Triangle, &[0, 2, 1, 8, 7, 6]),
            (Triangle, &[3, 4, 5, 9, 10, 11]),
            (QuadCenter, &[0, 1, 4, 3, 6, 13, 9, 12, 15]),
            (QuadCenter, &[1, 2, 5, 4, 7, 14, 10, 13, 16]),
            (QuadCenter, &[2, 0, 3, 5, 8, 12, 11, 14, 17]),
        ],
        33 => &[
            (QuadCenter, &[0, 4, 7, 3, 16, 15, 19, 11, 20]),
            (QuadCenter, &[1, 2, 6, 5, 9, 18, 13, 17, 21]),
            (QuadCenter, &[0, 1, 5, 4, 8, 17, 12, 16, 22]),
            (QuadCenter, &[3, 7, 6, 2, 19, 14, 18, 10, 23]),
            (Quad, &[0, 3, 2, 1, 11, 10, 9, 8]),
            (Quad, &[4, 5, 6, 7, 12, 13, 14, 15]),
        ],
        37 => &[
            (QuadCenter, &[0, 3, 2, 1, 8, 7, 6, 5, 13]),
            (TriangleCenter, &[0, 1, 4, 5, 10, 9, 14]),
            (TriangleCenter, &[1, 2, 4, 6, 11, 10, 15]),
            (TriangleCenter, &[2, 3, 4, 7, 12, 11, 16]),
            (TriangleCenter, &[3, 0, 4, 8, 9, 12, 17]),
        ],
        _ => &[],
    }
}

/// Resolve a referenced coordinate and require every component to be finite.
///
/// # Errors
/// The point index is out of bounds or any coordinate is nonfinite.
fn finite_position(positions: &[Vec3], index: usize) -> Result<Vec3> {
    let point = *positions
        .get(index)
        .ok_or("VTK curved node index is out of bounds")?;
    if !point.x.is_finite() || !point.y.is_finite() || !point.z.is_finite() {
        return Err("VTK curved node coordinate is nonfinite".into());
    }
    Ok(point)
}

/// Form a scale-resistant weighted position from finite authored node coordinates.
///
/// # Errors
/// Node references are invalid or the resulting curved coordinate is nonfinite.
fn interpolate(positions: &[Vec3], nodes: &[usize], weights: &[f64]) -> Result<Vec3> {
    let mut points = [Vec3::default(); 9];
    if nodes.is_empty() || nodes.len() > points.len() || weights.len() < nodes.len() {
        return Err("VTK interpolation node/weight count mismatch".into());
    }
    let mut scale = [0_f64; 3];
    for (point, &node) in points.iter_mut().zip(nodes) {
        *point = finite_position(positions, node)?;
        for (maximum, coordinate) in scale.iter_mut().zip([point.x, point.y, point.z]) {
            *maximum = maximum.max(coordinate.abs());
        }
    }
    let anchor = points[0];
    if points[..nodes.len()].iter().all(|&point| point == anchor) {
        return Ok(anchor);
    }
    let origin = [anchor.x, anchor.y, anchor.z];
    let mut delta = [0_f64; 3];
    for (point, &weight) in points.iter().zip(weights).take(nodes.len()).skip(1) {
        for ((sum, &base), coordinate) in delta
            .iter_mut()
            .zip(&origin)
            .zip([point.x, point.y, point.z])
        {
            *sum += (coordinate - base) * weight;
        }
    }
    let mut result = std::array::from_fn::<_, 3, _>(|i| origin[i] + delta[i]);
    if result.iter().any(|coordinate| !coordinate.is_finite()) {
        // Opposite finite extremes can overflow subtraction; normalize only for that fallback.
        delta = [0.; 3];
        for (point, &weight) in points.iter().zip(weights).take(nodes.len()).skip(1) {
            for (((sum, &maximum), &base), coordinate) in delta
                .iter_mut()
                .zip(&scale)
                .zip(&origin)
                .zip([point.x, point.y, point.z])
            {
                if maximum > 0. {
                    *sum += (coordinate / maximum - base / maximum) * weight;
                }
            }
        }
        for i in 0..3 {
            result[i] = if scale[i] > 0. {
                (origin[i] / scale[i] + delta[i]) * scale[i]
            } else {
                origin[i]
            };
        }
    }
    let point = Vec3::new(result[0], result[1], result[2]);
    if !point.x.is_finite() || !point.y.is_finite() || !point.z.is_finite() {
        return Err("VTK curved interpolation produces a nonfinite coordinate".into());
    }
    Ok(point)
}

/// Quadratic one-dimensional Lagrange weights for endpoint zero, endpoint one and midpoint.
fn quadratic(u: f64) -> [f64; 3] {
    [
        (1. - u) * (1. - 2. * u),
        u * (2. * u - 1.),
        4. * u * (1. - u),
    ]
}

/// Own barycentric, serendipity or tensor-product interpolation in VTK surface-node order.
fn weights(kind: Kind, u: f64, v: f64) -> [f64; 9] {
    let mut values = [0_f64; 9];
    match kind {
        Kind::Triangle | Kind::TriangleCenter => {
            let barycentric = [1. - u - v, u, v];
            let product = barycentric.iter().product::<f64>();
            for i in 0..3 {
                values[i] = barycentric[i] * (2. * barycentric[i] - 1.);
                values[3 + i] = 4. * barycentric[i] * barycentric[(i + 1) % 3];
                if kind == Kind::TriangleCenter {
                    values[i] += 3. * product;
                    values[3 + i] -= 12. * product;
                }
            }
            if kind == Kind::TriangleCenter {
                values[6] = 27. * product;
            }
        }
        Kind::Quad => {
            values[4] = 4. * u * (1. - u) * (1. - v);
            values[5] = 4. * u * v * (1. - v);
            values[6] = 4. * u * (1. - u) * v;
            values[7] = 4. * (1. - u) * v * (1. - v);
            for (i, corner) in [(1. - u) * (1. - v), u * (1. - v), u * v, (1. - u) * v]
                .into_iter()
                .enumerate()
            {
                values[i] = corner - f64::midpoint(values[4 + i], values[4 + (i + 3) % 4]);
            }
        }
        Kind::QuadCenter => {
            let a = quadratic(u);
            let b = quadratic(v);
            for (i, (x, y)) in [
                (0, 0),
                (1, 0),
                (1, 1),
                (0, 1),
                (2, 0),
                (1, 2),
                (2, 1),
                (0, 2),
                (2, 2),
            ]
            .into_iter()
            .enumerate()
            {
                values[i] = a[x] * b[y];
            }
        }
        Kind::QuadLinear => {
            let a = quadratic(u);
            values[..6].copy_from_slice(&[
                a[0] * (1. - v),
                a[1] * (1. - v),
                a[1] * v,
                a[0] * v,
                a[2] * (1. - v),
                a[2] * v,
            ]);
        }
    }
    values
}

/// Sample an authored quadratic or cubic line, omitting coincident consecutive sampled endpoints.
///
/// # Errors
/// Node counts/references, finite interpolation or the common output line budget are invalid.
pub(super) fn append_curve(
    cell_type: usize,
    positions: &[Vec3],
    nodes: &[usize],
    scene: &mut Scene,
) -> Result<()> {
    let expected = if cell_type == 21 {
        3
    } else if cell_type == 35 {
        4
    } else {
        0
    };
    if expected == 0 || nodes.len() != expected {
        return Err("invalid VTK curved line type or node count".into());
    }
    if scene
        .lines
        .len()
        .checked_add(LINE_SUBDIVISIONS)
        .is_none_or(|n| n > MAX_ITEMS)
    {
        return Err("VTK curved line exceeds output limit".into());
    }
    let mut previous = finite_position(positions, nodes[0])?;
    for step in 1..=LINE_SUBDIVISIONS {
        let authored = if step == LINE_SUBDIVISIONS {
            Some(1)
        } else if (cell_type == 21 && step == LINE_SUBDIVISIONS / 2)
            || (cell_type == 35 && step == LINE_SUBDIVISIONS / 3)
        {
            Some(2)
        } else if cell_type == 35 && step == 2 * LINE_SUBDIVISIONS / 3 {
            Some(3)
        } else {
            None
        };
        let point = if let Some(node) = authored {
            finite_position(positions, nodes[node])?
        } else {
            let u = f64::from(u32::try_from(step)?) / 12.;
            let mut values = [0_f64; 4];
            if cell_type == 21 {
                values[..3].copy_from_slice(&quadratic(u));
            } else {
                let sites = [0., 1., 1. / 3., 2. / 3.];
                for i in 0..4 {
                    values[i] = 1.;
                    for j in 0..4 {
                        if i != j {
                            values[i] *= (u - sites[j]) / (sites[i] - sites[j]);
                        }
                    }
                }
            }
            interpolate(positions, nodes, &values)?
        };
        if previous != point {
            scene.lines.push(Line {
                vertices: [previous, point],
                color: None,
            });
        }
        previous = point;
    }
    Ok(())
}

/// Fixed line-output expansion count for either standard higher-order line.
pub(super) const fn line_segments() -> usize {
    LINE_SUBDIVISIONS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_faces_preserve_attached_roles_and_missing_edges() {
        let triangle = Surface::new(Kind::Triangle, &[10, 20, 30, 40, 50, 60]).unwrap();
        let rotated = Surface::new(Kind::Triangle, &[20, 30, 10, 50, 60, 40]).unwrap();
        let reflected = Surface::new(Kind::Triangle, &[10, 30, 20, 60, 50, 40]).unwrap();
        assert_eq!(triangle.key(), rotated.key());
        assert_eq!(triangle.key(), reflected.key());
        let wrong_edges = Surface::new(Kind::Triangle, &[10, 20, 30, 50, 40, 60]).unwrap();
        assert_ne!(triangle.key(), wrong_edges.key());
        let centered = Surface::new(Kind::TriangleCenter, &[10, 20, 30, 40, 50, 60, 70]).unwrap();
        assert_ne!(triangle.key(), centered.key());
        let quad = Surface::new(Kind::QuadLinear, &[0, 1, 2, 3, 4, 5]).unwrap();
        let half_turn = Surface::new(Kind::QuadLinear, &[2, 3, 0, 1, 5, 4]).unwrap();
        let changed_axis = Surface::new(Kind::QuadLinear, &[1, 2, 3, 0, 4, 5]).unwrap();
        assert_eq!(quad.key(), half_turn.key());
        assert_ne!(quad.key(), changed_axis.key());
    }

    #[test]
    fn complete_shared_faces_cancel_and_nonmanifold_storage_fails_before_growth() {
        let triangle = Surface::new(Kind::Triangle, &[0, 1, 2, 3, 4, 5]).unwrap();
        let reflected = Surface::new(Kind::Triangle, &[0, 2, 1, 5, 4, 3]).unwrap();
        let mut faces = Faces::default();
        let mut storage = Storage::default();
        faces.insert(triangle, &mut storage).unwrap();
        assert_eq!(faces.visible().count(), 1);
        faces.insert(reflected, &mut storage).unwrap();
        assert_eq!(faces.visible().count(), 0);
        assert!(faces.insert(triangle, &mut storage).is_err());
        let mut empty = Faces::default();
        let mut exhausted = Storage {
            bytes: usize::try_from(super::super::super::MAX_INPUT_BYTES).unwrap(),
        };
        assert!(empty.insert(triangle, &mut exhausted).is_err());
        assert!(empty.faces.is_empty());
        assert!(Storage { bytes: usize::MAX }.charge(1).is_err());
    }

    #[test]
    fn curved_lines_preserve_authored_endpoint_and_third_node_order() {
        let quadratic_nodes = [
            Vec3::new(0., 0., 0.),
            Vec3::new(2., 0., 0.),
            Vec3::new(1., 2., 0.),
        ];
        let mut scene = Scene::default();
        append_curve(21, &quadratic_nodes, &[0, 1, 2], &mut scene).unwrap();
        assert_eq!(scene.lines.len(), 12);
        assert_eq!(scene.lines[5].vertices[1], quadratic_nodes[2]);
        assert_eq!(scene.lines[11].vertices[1], quadratic_nodes[1]);
        let cubic_nodes = [
            Vec3::new(0., 0., 0.),
            Vec3::new(3., 0., 0.),
            Vec3::new(1., 2., 0.),
            Vec3::new(2., -1., 0.),
        ];
        let mut cubic = Scene::default();
        append_curve(35, &cubic_nodes, &[0, 1, 2, 3], &mut cubic).unwrap();
        assert_eq!(cubic.lines.len(), 12);
        assert_eq!(cubic.lines[3].vertices[1], cubic_nodes[2]);
        assert_eq!(cubic.lines[7].vertices[1], cubic_nodes[3]);
        assert_eq!(cubic.lines[11].vertices[1], cubic_nodes[1]);
    }

    #[test]
    fn collapsed_and_partly_stationary_curves_emit_only_nonzero_segments() {
        let collapsed = [Vec3::new(2., 3., 4.); 4];
        for (kind, count) in [(21, 3), (35, 4)] {
            let mut empty = Scene::default();
            append_curve(
                kind,
                &collapsed,
                &(0..count).collect::<Vec<_>>(),
                &mut empty,
            )
            .unwrap();
            assert!(empty.lines.is_empty());
        }
        let partly_stationary = [
            Vec3::new(4., 3., 4.),
            Vec3::new(26., 3., 4.),
            Vec3::new(9., 3., 4.),
        ];
        let mut partial = Scene::default();
        append_curve(21, &partly_stationary, &[0, 1, 2], &mut partial).unwrap();
        assert_eq!(partial.lines.len(), 11);
        assert!(
            partial
                .lines
                .iter()
                .all(|line| line.vertices[0] != line.vertices[1])
        );
        assert_eq!(partial.lines[0].vertices[0], partly_stationary[0]);
    }

    #[test]
    fn anchored_interpolation_preserves_offsets_and_handles_opposite_finite_extremes() {
        let translated = [
            Vec3::new(1e16, 3., 4.),
            Vec3::new(1e16 + 8., 3., 4.),
            Vec3::new(1e16 + 4., 3., 4.),
        ];
        let point = interpolate(&translated, &[0, 1, 2], &quadratic(0.25)).unwrap();
        assert_eq!(point, Vec3::new(1e16 + 2., 3., 4.));
        let opposite = [
            Vec3::new(1e308, 0., 0.),
            Vec3::new(-1e308, 0., 0.),
            Vec3::new(0., 0., 0.),
        ];
        let point = interpolate(&opposite, &[0, 1, 2], &quadratic(0.25)).unwrap();
        assert!((point.x / 5e307 - 1.).abs() < 1e-14);
        let invalid = [
            Vec3::new(1., 2., 3.),
            Vec3::new(1., 2., 3.),
            Vec3::new(f64::NAN, 2., 3.),
        ];
        assert!(interpolate(&invalid, &[0, 1, 2], &quadratic(0.25)).is_err());
        assert!(interpolate(&translated, &[0, 1, 9], &quadratic(0.25)).is_err());
    }

    #[test]
    fn enriched_triangles_retain_authored_midpoints_and_centroid() {
        let triangle_nodes = [
            Vec3::new(0., 0., 0.),
            Vec3::new(2., 0., 0.),
            Vec3::new(0., 2., 0.),
            Vec3::new(1., 0., 1.),
            Vec3::new(1., 1., 1.),
            Vec3::new(0., 1., 1.),
            Vec3::new(2. / 3., 2. / 3., 3.),
        ];
        let surface = Surface::new(Kind::TriangleCenter, &[0, 1, 2, 3, 4, 5, 6]).unwrap();
        let mut triangle = Scene::default();
        surface.append(&triangle_nodes, &mut triangle).unwrap();
        assert_eq!(triangle.triangles.len(), 36);
        assert!(
            triangle_nodes
                .iter()
                .all(|node| triangle.positions().any(|point| point == *node))
        );
    }

    #[test]
    fn quadrilateral_bases_retain_authored_nodes_and_distinct_center_response() {
        let quad_nodes = [
            Vec3::new(0., 0., 0.),
            Vec3::new(2., 0., 0.),
            Vec3::new(2., 2., 0.),
            Vec3::new(0., 2., 0.),
            Vec3::new(1., 0., 1.),
            Vec3::new(2., 1., 1.),
            Vec3::new(1., 2., 1.),
            Vec3::new(0., 1., 1.),
            Vec3::new(1., 1., 3.),
        ];
        let serendipity = Surface::new(Kind::Quad, &[0, 1, 2, 3, 4, 5, 6, 7]).unwrap();
        let tensor = Surface::new(Kind::QuadCenter, &[0, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(
            serendipity.sample(&quad_nodes, 3, 3).unwrap(),
            Vec3::new(1., 1., 2.)
        );
        assert_eq!(tensor.sample(&quad_nodes, 3, 3).unwrap(), quad_nodes[8]);
        let mut quad = Scene::default();
        tensor.append(&quad_nodes, &mut quad).unwrap();
        assert_eq!(quad.triangles.len(), 72);
        assert!(
            quad_nodes
                .iter()
                .all(|node| quad.positions().any(|point| point == *node))
        );
        let mixed_nodes = [
            quad_nodes[0],
            quad_nodes[1],
            quad_nodes[2],
            quad_nodes[3],
            quad_nodes[4],
            Vec3::new(1., 2., 2.),
        ];
        let mixed = Surface::new(Kind::QuadLinear, &[0, 1, 2, 3, 4, 5]).unwrap();
        let mut quad_linear = Scene::default();
        mixed.append(&mixed_nodes, &mut quad_linear).unwrap();
        assert_eq!(quad_linear.triangles.len(), 12);
        assert!(
            mixed_nodes
                .iter()
                .all(|node| quad_linear.positions().any(|point| point == *node))
        );
    }
}
