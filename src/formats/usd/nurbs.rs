//! Variable-order rational B-spline centerlines with bounded exact pole checks.
//!
//! Every nonempty trimmed knot span is sampled independently, so a knot with
//! full order multiplicity cannot accidentally connect discontinuous pieces.
//! Common strictly signed weights have a nonzero denominator by the positive
//! B-spline basis. All other spans use exact input scalars, common-factor removal
//! and a Sturm count over the complete closed span, including even roots.

#![allow(
    clippy::float_cmp,
    reason = "Exact zero and weight-sign comparisons select mathematically certified evaluation paths."
)]

use super::{Colors, MAX_ELEMENTS, Transform, check_skin, ints, rgb, value, vectors};
use crate::{
    Result,
    scene::{Line, Point, Scene, Triangle, Vec3},
};
use openusd::{
    sdf,
    usd::{self, Prim},
};

/// Exact rational polynomial extraction, cancellation and denominator certification.
mod exact;

/// Maximum curve order; extraction and certification have polynomial work in order.
const MAX_ORDER: usize = 64;
/// Uniform intervals per nonempty knot span for nonconstant centerlines.
const STEPS: usize = 24;
/// Aggregate nonempty spans, including spans in point-instancer prototypes.
const MAX_SPANS: usize = 250_000;
/// Aggregate arithmetic work charged before floating or exact span expansion.
const MAX_WORK: usize = 100_000_000;
/// Aggregate exact rational operations, including reduction and root certificates.
const MAX_EXACT_WORK: usize = 5_000_000;
/// Aggregate decoded NURBS input and generated sample storage charged before expansion.
const MAX_BYTES: usize = 512 * 1024 * 1024;

/// Capacities already charged for one scene's triangle, point and line vectors.
#[derive(Default)]
pub(super) struct OutputCapacity {
    /// Allocated element counts in triangle, point, line order.
    elements: [usize; 3],
}

/// Stage-wide accounting retained across ordinary prims and prototype scenes.
#[derive(Default)]
pub(super) struct Budget {
    /// Trimmed nonempty spans encountered in all decoded NURBS prims.
    spans: usize,
    /// Output sample count, including repeated span endpoints.
    samples: usize,
    /// Estimated arithmetic work for span expansion.
    work: usize,
    /// Actual checked rational arithmetic operations.
    exact_work: usize,
    /// Cumulative decoded attribute and generated sample bytes.
    bytes: usize,
}

impl Budget {
    /// Charge one bounded counter before performing the associated work.
    fn charge(counter: &mut usize, amount: usize, limit: usize, label: &str) -> Result<()> {
        *counter = counter
            .checked_add(amount)
            .filter(|&n| n <= limit)
            .ok_or_else(|| format!("USD NURBS {label} budget exceeded"))?;
        Ok(())
    }

    /// Account for a single exact operation before its integer allocations.
    fn exact_operation(&mut self) -> Result<()> {
        Self::charge(&mut self.exact_work, 1, MAX_EXACT_WORK, "exact arithmetic")
    }

    /// Charge temporary rational integer allocations before their arithmetic operation.
    fn exact_allocation(&mut self, bits: u64) -> Result<()> {
        let bytes = usize::try_from(bits.div_ceil(8))?;
        Self::charge(&mut self.bytes, bytes, MAX_BYTES, "allocation")
    }

    /// Precharge bounded geometric capacity growth before any geometry insertion.
    ///
    /// Local ledgers prevent charging an existing capacity repeatedly; the shared
    /// stage charge survives when prototype ledgers or cached scenes are dropped.
    ///
    /// # Errors
    /// Returns an error for exceeded element/allocation budgets or failed reserves.
    pub(super) fn reserve_output(
        &mut self,
        scene: &mut Scene,
        recorded: &mut OutputCapacity,
        additional: [usize; 3],
    ) -> Result<()> {
        let sizes = [size_of::<Triangle>(), size_of::<Point>(), size_of::<Line>()];
        let lengths = [scene.triangles.len(), scene.points.len(), scene.lines.len()];
        let capacities = [
            scene.triangles.capacity(),
            scene.points.capacity(),
            scene.lines.capacity(),
        ];
        let mut required = [0_usize; 3];
        let mut total = 0_usize;
        for axis in 0..3 {
            required[axis] = lengths[axis]
                .checked_add(additional[axis])
                .ok_or("USD output element count overflow")?;
            total = total
                .checked_add(required[axis])
                .ok_or("USD output element count overflow")?;
            let unrecorded = capacities[axis].saturating_sub(recorded.elements[axis]);
            let bytes = unrecorded
                .checked_mul(sizes[axis])
                .ok_or("USD output byte count overflow")?;
            Self::charge(&mut self.bytes, bytes, MAX_BYTES, "allocation")?;
        }
        if total > MAX_ELEMENTS {
            return Err("USD geometry exceeds five million elements".into());
        }
        let mut desired = capacities;
        for axis in 0..3 {
            if required[axis] > capacities[axis] {
                let remaining = (MAX_BYTES - self.bytes) / sizes[axis];
                if required[axis] - capacities[axis] > remaining {
                    return Err("USD NURBS allocation budget exceeded by geometry output".into());
                }
                desired[axis] = capacities[axis]
                    .saturating_mul(2)
                    .max(4)
                    .max(required[axis])
                    .min(capacities[axis] + remaining)
                    .min(MAX_ELEMENTS);
                let bytes = (desired[axis] - capacities[axis])
                    .checked_mul(sizes[axis])
                    .ok_or("USD output byte count overflow")?;
                Self::charge(&mut self.bytes, bytes, MAX_BYTES, "allocation")?;
            }
        }
        if desired[0] > capacities[0] {
            scene.triangles.try_reserve_exact(desired[0] - lengths[0])?;
        }
        if desired[1] > capacities[1] {
            scene.points.try_reserve_exact(desired[1] - lengths[1])?;
        }
        if desired[2] > capacities[2] {
            scene.lines.try_reserve_exact(desired[2] - lengths[2])?;
        }
        let actual = [
            scene.triangles.capacity(),
            scene.points.capacity(),
            scene.lines.capacity(),
        ];
        for axis in 0..3 {
            let spare = actual[axis].saturating_sub(desired[axis]);
            let bytes = spare
                .checked_mul(sizes[axis])
                .ok_or("USD output byte count overflow")?;
            Self::charge(&mut self.bytes, bytes, MAX_BYTES, "allocation")?;
        }
        recorded.elements = actual;
        Ok(())
    }

    /// Reserve a span's samples, allocation and worst-case polynomial extraction work.
    fn span(&mut self, order: usize) -> Result<()> {
        let samples = if order == 1 { 1 } else { STEPS + 1 };
        Self::charge(&mut self.spans, 1, MAX_SPANS, "span")?;
        Self::charge(
            &mut self.samples,
            samples,
            MAX_ELEMENTS + MAX_SPANS,
            "sample",
        )?;
        let work = order
            .checked_pow(3)
            .and_then(|n| n.checked_mul(16))
            .ok_or("USD NURBS work count overflow")?;
        Self::charge(&mut self.work, work, MAX_WORK, "evaluation work")?;
        let bytes = samples * size_of::<Vec3>() + samples * order * size_of::<[f64; 4]>();
        Self::charge(&mut self.bytes, bytes, MAX_BYTES, "allocation")?;
        Ok(())
    }
}

/// One validated curve's positions, knots and optional original rational weights.
struct Curve<'a> {
    /// Unweighted authored control points in the prim's local frame.
    points: &'a [Vec3],
    /// Complete nondecreasing knot vector, including endpoint extension knots.
    knots: &'a [f64],
    /// One weight per position, or an empty slice representing unit weights.
    weights: &'a [f64],
    /// Polynomial degree plus one.
    order: usize,
    /// Closed authored parameter interval within the active spline domain.
    range: [f64; 2],
}

impl Curve<'_> {
    /// Obtain one original authored weight or the omitted unit-weight default.
    fn weight(&self, index: usize) -> f64 {
        self.weights.get(index).copied().unwrap_or(1.)
    }

    /// Enumerate only active nonzero knot spans intersecting the trimmed range.
    fn spans(&self) -> impl Iterator<Item = (usize, [f64; 2])> + '_ {
        (self.order - 1..self.points.len()).filter_map(|span| {
            let first = self.knots[span].max(self.range[0]);
            let last = self.knots[span + 1].min(self.range[1]);
            (first < last).then_some((span, [first, last]))
        })
    }

    /// Evaluate a span only when every active weight has the same strict sign.
    ///
    /// Positive basis functions then certify a nonzero denominator throughout
    /// the span. Overflow, underflow or ill-conditioned knot subtraction routes
    /// the whole span to exact arithmetic instead of changing the curve.
    fn floating_samples(&self, span: usize, range: [f64; 2]) -> Option<Vec<Vec3>> {
        let length = range[1] - range[0];
        if !length.is_finite() || length <= 0. {
            return None;
        }
        let first = span + 1 - self.order;
        let sign = self.weight(first).signum();
        let mut scale = 0_f64;
        for i in first..=span {
            let w = self.weight(i);
            if w == 0. || w.signum() != sign {
                return None;
            }
            scale = scale.max(w.abs());
        }
        let position = self.points[first];
        if self.points[first..=span]
            .iter()
            .all(|&point| point == position)
        {
            return Some(vec![position; if self.order == 1 { 1 } else { STEPS + 1 }]);
        }
        let mut original = Vec::with_capacity(self.order);
        for i in first..=span {
            let w = self.weight(i) / scale;
            let p = self.points[i];
            let h = [p.x * w, p.y * w, p.z * w, w];
            if w == 0.
                || h.iter().any(|v| !v.is_finite() || v.is_subnormal())
                || [p.x, p.y, p.z]
                    .iter()
                    .zip(&h)
                    .any(|(&coordinate, &weighted)| coordinate != 0. && weighted == 0.)
            {
                return None;
            }
            original.push(h);
        }
        let mut samples = Vec::with_capacity(STEPS + 1);
        let count = if self.order == 1 { 0 } else { STEPS };
        for step in 0..=count {
            let parameter = f64::from(u32::try_from(step).ok()?) / 24.;
            let mut h = original.clone();
            for depth in 1..self.order {
                for j in (depth..self.order).rev() {
                    let i = first + j;
                    let alpha = span_alpha(
                        [self.knots[i], self.knots[i + self.order - depth]],
                        range[0],
                        length,
                        parameter,
                    )?;
                    let mut next = [0_f64; 4];
                    for (axis, value) in next.iter_mut().enumerate() {
                        let left = (1. - alpha) * h[j - 1][axis];
                        let right = alpha * h[j][axis];
                        if ((1. - alpha) != 0. && h[j - 1][axis] != 0. && left == 0.)
                            || (alpha != 0. && h[j][axis] != 0. && right == 0.)
                            || left.is_subnormal()
                            || right.is_subnormal()
                        {
                            return None;
                        }
                        *value = left + right;
                        if !value.is_finite() || value.is_subnormal() {
                            return None;
                        }
                    }
                    h[j] = next;
                }
            }
            let value = h[self.order - 1];
            if value[3] == 0. || value.iter().any(|v| !v.is_finite()) {
                return None;
            }
            let p = Vec3::new(
                value[0] / value[3],
                value[1] / value[3],
                value[2] / value[3],
            );
            if !p.is_finite() {
                return None;
            }
            samples.push(p);
        }
        Some(samples)
    }
}

/// Compute a normalized-span de Boor ratio or require exact arithmetic when unresolved.
fn span_alpha(knots: [f64; 2], first: f64, length: f64, parameter: f64) -> Option<f64> {
    let width = knots[1] - knots[0];
    let displacement = first - knots[0];
    if !width.is_finite() || width <= 0. || !displacement.is_finite() {
        return None;
    }
    let start = displacement / width;
    let interval = length / width;
    if (displacement != 0. && (start == 0. || start.is_subnormal()))
        || interval == 0.
        || interval.is_subnormal()
    {
        return None;
    }
    let increment = parameter * interval;
    if parameter != 0.
        && (increment == 0.
            || increment.is_subnormal()
            || increment <= 64. * f64::EPSILON * start.abs())
    {
        return None;
    }
    let alpha = start + increment;
    (alpha.is_finite() && (0. ..=1.).contains(&alpha)).then_some(alpha)
}

/// Read a bounded finite double array, preserving the original binary scalars.
fn doubles(prim: &Prim, name: &str, time: usd::TimeCode) -> Result<Option<Vec<f64>>> {
    match value(prim, name, time)? {
        None => Ok(None),
        Some(sdf::Value::DoubleVec(values))
            if values.len() <= MAX_ELEMENTS && values.iter().all(|v| v.is_finite()) =>
        {
            Ok(Some(values))
        }
        _ => Err(format!("USD NURBS {name} must be a bounded finite double array").into()),
    }
}

/// Read one finite double pair per curve without supplying an invented domain.
fn ranges(prim: &Prim, time: usd::TimeCode) -> Result<Vec<[f64; 2]>> {
    match value(prim, "ranges", time)? {
        Some(sdf::Value::Vec2dVec(values))
            if values.len() <= MAX_ELEMENTS
                && values.iter().all(|v| v.x.is_finite() && v.y.is_finite()) =>
        {
            Ok(values.into_iter().map(|v| [v.x, v.y]).collect())
        }
        _ => Err("USD NURBS ranges must be a bounded finite double2 array".into()),
    }
}

/// Validate cardinalities, ordered knots, schema ranges and the supported active domain.
fn validate(curve: &Curve<'_>) -> Result<()> {
    if curve.order == 0 || curve.order > curve.points.len() {
        return Err("USD NURBS order must be positive and at most its vertex count".into());
    }
    if curve.order > MAX_ORDER {
        return Err("USD NURBS order exceeds the bounded maximum of 64".into());
    }
    if curve.knots.len() != curve.points.len() + curve.order
        || curve.knots.windows(2).any(|p| p[0] > p[1])
    {
        return Err("USD NURBS knots require vertex count plus order nondecreasing entries".into());
    }
    if curve.range[0] >= curve.range[1]
        || curve.range[0] < curve.knots[curve.order - 1]
        || curve.range[1] > curve.knots[curve.knots.len() - 1]
    {
        return Err("USD NURBS range must be increasing and inside its schema knot domain".into());
    }
    if curve.range[1] > curve.knots[curve.points.len()] {
        return Err("unsupported USD NURBS range beyond the active spline domain".into());
    }
    Ok(())
}

/// Append genuine rational centerlines with the stage's placement and color policies.
///
/// # Errors
/// Returns an error for malformed arrays, unsupported ranges, exceeded budgets,
/// nonremovable rational poles, or nonfinite geometry and colors.
#[allow(
    clippy::too_many_lines,
    reason = "One bounded pass validates all parallel arrays and reserves expansion before emitting geometry."
)]
pub(super) fn append(
    prim: &Prim,
    time: usd::TimeCode,
    world: Transform,
    scene: &mut Scene,
    budget: &mut Budget,
    output_capacity: &mut OutputCapacity,
) -> Result<()> {
    check_skin(prim)?;
    let positions = vectors(prim, "points", time)?;
    let counts = ints(prim, "curveVertexCounts", time)?;
    let orders = ints(prim, "order", time)?;
    let knots = doubles(prim, "knots", time)?.ok_or("USD NURBS knots are required")?;
    let ranges = ranges(prim, time)?;
    let weights = doubles(prim, "pointWeights", time)?;
    if counts.len() != orders.len()
        || counts.len() != ranges.len()
        || weights.as_ref().is_some_and(|w| w.len() != positions.len())
    {
        return Err(
            "USD NURBS curve orders, ranges, counts or weights have different lengths".into(),
        );
    }
    let point_count = counts
        .iter()
        .try_fold(0_usize, |sum, &count| -> Result<usize> {
            sum.checked_add(usize::try_from(count)?)
                .ok_or_else(|| "USD NURBS point count overflow".into())
        })?;
    let order_count = orders
        .iter()
        .try_fold(0_usize, |sum, &order| -> Result<usize> {
            sum.checked_add(usize::try_from(order)?)
                .ok_or_else(|| "USD NURBS order count overflow".into())
        })?;
    if point_count != positions.len() || point_count.checked_add(order_count) != Some(knots.len()) {
        return Err("USD NURBS counts do not match the complete points and knots arrays".into());
    }
    let input_bytes = positions.len() * size_of::<Vec3>()
        + (counts.len() + orders.len()) * size_of::<i32>()
        + knots.len() * size_of::<f64>()
        + ranges.len() * size_of::<[f64; 2]>()
        + weights.as_ref().map_or(0, |w| w.len() * size_of::<f64>());
    Budget::charge(&mut budget.bytes, input_bytes, MAX_BYTES, "allocation")?;
    Budget::charge(
        &mut budget.bytes,
        counts.len() * size_of::<(usize, Curve<'_>)>(),
        MAX_BYTES,
        "allocation",
    )?;
    let mut curves = Vec::with_capacity(counts.len());
    let mut point_offset = 0_usize;
    let mut knot_offset = 0_usize;
    for i in 0..counts.len() {
        let count = usize::try_from(counts[i])?;
        let order = usize::try_from(orders[i])?;
        let point_end = point_offset
            .checked_add(count)
            .ok_or("USD NURBS count overflow")?;
        let knot_end = knot_offset
            .checked_add(count)
            .and_then(|n| n.checked_add(order))
            .ok_or("USD NURBS knot count overflow")?;
        let curve = Curve {
            points: positions
                .get(point_offset..point_end)
                .ok_or("USD NURBS counts exceed points")?,
            knots: knots
                .get(knot_offset..knot_end)
                .ok_or("USD NURBS counts exceed knots")?,
            weights: weights.as_ref().map_or(Ok(&[][..]), |w| {
                w.get(point_offset..point_end)
                    .ok_or("USD NURBS counts exceed weights")
            })?,
            order,
            range: ranges[i],
        };
        validate(&curve)?;
        for _ in curve.spans() {
            budget.span(order)?;
        }
        curves.push((point_offset, curve));
        point_offset = point_end;
        knot_offset = knot_end;
    }
    if point_offset != positions.len() || knot_offset != knots.len() {
        return Err("USD NURBS points or knots have trailing entries".into());
    }
    let colors = Colors::new(prim, time)?;
    for (index, (offset, curve)) in curves.iter().enumerate() {
        let color = if let Some(color) = colors.fallback {
            Some(color)
        } else {
            let mut sum = Vec3::default();
            let mut has_color = true;
            for vertex in *offset..offset + curve.points.len() {
                if let Some(color) = colors.at(vertex, index, vertex)? {
                    sum = sum + color;
                } else {
                    has_color = false;
                }
            }
            if has_color {
                Some(rgb(sum / f64::from(u32::try_from(curve.points.len())?)))
            } else {
                None
            }
        };
        for (span, range) in curve.spans() {
            let samples = match curve.floating_samples(span, range) {
                Some(samples) => samples,
                None => exact::samples(curve, span, range, budget)?,
            };
            if curve.order == 1 || samples.iter().all(|&p| p == samples[0]) {
                budget.reserve_output(scene, output_capacity, [0, 1, 0])?;
                scene.points.push(Point {
                    position: world.apply(samples[0]),
                    color,
                });
            } else {
                let lines = samples.windows(2).filter(|pair| pair[0] != pair[1]).count();
                budget.reserve_output(scene, output_capacity, [0, 0, lines])?;
                for pair in samples.windows(2) {
                    if pair[0] != pair[1] {
                        scene.lines.push(Line {
                            vertices: [world.apply(pair[0]), world.apply(pair[1])],
                            color,
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
/// Owned capacity controls that exercise small allocations at the configured byte boundary.
mod tests {
    use super::*;

    /// One literal triangle used solely as a live output payload in reservation controls.
    fn triangle() -> Triangle {
        Triangle {
            vertices: [
                Vec3::default(),
                Vec3::new(1., 0., 0.),
                Vec3::new(0., 1., 0.),
            ],
            color: None,
        }
    }

    #[test]
    fn existing_capacity_is_charged_once_and_reused_for_later_insertions() -> Result<()> {
        let mut budget = Budget::default();
        let mut capacity = OutputCapacity::default();
        let mut scene = Scene::default();
        budget.reserve_output(&mut scene, &mut capacity, [0, 1, 0])?;
        let charged = budget.bytes;
        let initial = scene.points.capacity();
        assert!(initial >= 4);
        scene.points.push(Point {
            position: Vec3::default(),
            color: None,
        });
        budget.reserve_output(&mut scene, &mut capacity, [0, 1, 0])?;
        assert_eq!(scene.points.capacity(), initial);
        assert_eq!(budget.bytes, charged);
        assert_eq!(charged, initial * size_of::<Point>());
        Ok(())
    }

    #[test]
    fn prototype_charges_survive_local_ledgers_and_destination_copies_add_only_new_capacity()
    -> Result<()> {
        let mut budget = Budget::default();
        let source = {
            let mut scene = Scene::default();
            let mut capacity = OutputCapacity::default();
            budget.reserve_output(&mut scene, &mut capacity, [1, 0, 0])?;
            scene.triangles.push(triangle());
            scene
        };
        let source_charge = source.triangles.capacity() * size_of::<Triangle>();
        assert_eq!(budget.bytes, source_charge);
        let mut destination = Scene::default();
        let mut capacity = OutputCapacity::default();
        budget.reserve_output(
            &mut destination,
            &mut capacity,
            [source.triangles.len(), 0, 0],
        )?;
        destination.triangles.extend_from_slice(&source.triangles);
        let destination_charge = destination.triangles.capacity() * size_of::<Triangle>();
        assert_eq!(budget.bytes, source_charge + destination_charge);
        budget.reserve_output(
            &mut destination,
            &mut capacity,
            [source.triangles.len(), 0, 0],
        )?;
        assert_eq!(budget.bytes, source_charge + destination_charge);
        drop(source);
        assert_eq!(budget.bytes, source_charge + destination_charge);
        Ok(())
    }

    #[test]
    fn mixed_existing_payloads_are_included_before_any_new_output_reservation() {
        let mut scene = Scene {
            triangles: vec![triangle()],
            points: vec![Point {
                position: Vec3::default(),
                color: None,
            }],
            lines: vec![Line {
                vertices: [Vec3::default(), Vec3::new(1., 0., 0.)],
                color: None,
            }],
            ..Scene::default()
        };
        let payload = scene.triangles.capacity() * size_of::<Triangle>()
            + scene.points.capacity() * size_of::<Point>()
            + scene.lines.capacity() * size_of::<Line>();
        let remaining = size_of::<Point>() - 1;
        let mut budget = Budget {
            bytes: MAX_BYTES - payload - remaining,
            ..Budget::default()
        };
        let mut capacity = OutputCapacity::default();
        let before = [
            scene.triangles.capacity(),
            scene.points.capacity(),
            scene.lines.capacity(),
        ];
        assert!(
            budget
                .reserve_output(&mut scene, &mut capacity, [0, 1, 0])
                .is_err()
        );
        assert_eq!(budget.bytes, MAX_BYTES - remaining);
        assert_eq!(
            [
                scene.triangles.capacity(),
                scene.points.capacity(),
                scene.lines.capacity()
            ],
            before
        );
        assert_eq!(
            [scene.triangles.len(), scene.points.len(), scene.lines.len()],
            [1, 1, 1]
        );
    }

    #[test]
    fn an_instance_copy_fails_before_growing_the_destination_when_bytes_do_not_fit() -> Result<()> {
        let mut budget = Budget::default();
        let mut source = Scene::default();
        let mut source_capacity = OutputCapacity::default();
        budget.reserve_output(&mut source, &mut source_capacity, [1, 0, 0])?;
        source.triangles.push(triangle());
        budget.bytes = MAX_BYTES - size_of::<Triangle>() + 1;
        let mut destination = Scene::default();
        let mut capacity = OutputCapacity::default();
        assert!(
            budget
                .reserve_output(
                    &mut destination,
                    &mut capacity,
                    [source.triangles.len(), 0, 0]
                )
                .is_err()
        );
        assert_eq!(destination.triangles.capacity(), 0);
        assert_eq!(destination.triangles.len(), 0);
        Ok(())
    }
}
