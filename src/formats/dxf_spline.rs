//! Evaluate DXF's authored rational B-spline knot/control-point representation.
use super::{Entity, Result, Vec3, error};

/// Maximum authored spline control-point count.
const MAX_CONTROLS: usize = 100_000;
/// Maximum adaptive spline approximation segments.
const MAX_SEGMENTS: usize = 100_000;
/// Maximum charged spline or Boolean-processing work.
const MAX_WORK: usize = 10_000_000;

/// Decode every finite numeric group value for one spline code.
///
/// # Errors
///
/// A matching value is nonnumeric or non-finite.
fn values(entity: &Entity, code: u16) -> Result<Vec<f64>> {
    entity
        .all(code)
        .map(|s| {
            let n: f64 = s.parse()?;
            if !n.is_finite() {
                return Err(error("non-finite DXF spline value"));
            }
            Ok(n)
        })
        .collect()
}
/// Decode ordered spline point components without accepting duplicate or misplaced coordinates.
///
/// # Errors
///
/// Coordinates are invalid, duplicate, out of order or above the control-point limit.
fn points(entity: &Entity, code: u16) -> Result<Vec<Vec3>> {
    let mut result = Vec::new();
    let mut y_set = false;
    let mut z_set = false;
    for pair in &entity.pairs {
        if ![code, code + 10, code + 20].contains(&pair.code) {
            continue;
        }
        let n: f64 = pair.value.parse()?;
        if !n.is_finite() {
            return Err(error("non-finite DXF spline point"));
        }
        if pair.code == code {
            if result.len() == MAX_CONTROLS {
                return Err(error("DXF spline point count exceeds limit"));
            }
            result.push(Vec3::new(n, 0., 0.));
            y_set = false;
            z_set = false;
        } else {
            let p = result
                .last_mut()
                .ok_or_else(|| error("DXF spline point component precedes its X coordinate"))?;
            if pair.code == code + 10 {
                if y_set {
                    return Err(error("duplicate DXF spline Y component"));
                }
                p.y = n;
                y_set = true;
            } else {
                if z_set {
                    return Err(error("duplicate DXF spline Z component"));
                }
                p.z = n;
                z_set = true;
            }
        }
    }
    Ok(result)
}
/// Validated rational spline data and bounded adaptive-evaluation state.
struct Curve {
    /// Authored finite spline control-point coordinates.
    controls: Vec<Vec3>,
    /// Positive rational weight paired with each control point.
    weights: Vec<f64>,
    /// Validated nondecreasing spline knot vector.
    knots: Vec<f64>,
    /// Supported polynomial degree of the authored spline.
    degree: usize,
    /// Local numerical origin used to stabilize homogeneous interpolation.
    origin: Vec3,
    /// Accumulated processing work charged against the preview budget.
    work: usize,
    /// Number of emitted adaptive approximation segments.
    segments: usize,
    /// Maximum accepted geometric chord deviation for the sampled spline.
    tolerance: f64,
}
impl Curve {
    /// Evaluate a rational B-spline span using homogeneous de Boor interpolation.
    ///
    /// # Errors
    ///
    /// Evaluation exceeds its work limit or produces an invalid denominator/non-finite point.
    #[allow(
        clippy::float_cmp,
        reason = "Exact knot equality and subdivision stagnation are structural checks, not approximate geometry comparisons."
    )]
    fn evaluate(&mut self, span: usize, t: f64) -> Result<Vec3> {
        self.work = self
            .work
            .checked_add((self.degree + 1) * (self.degree + 1))
            .ok_or_else(|| error("DXF spline work overflow"))?;
        if self.work > MAX_WORK {
            return Err(error("DXF spline evaluation exceeds work limit"));
        }
        let mut d: Vec<[f64; 4]> = (span - self.degree..=span)
            .map(|i| {
                let p = self.controls[i] - self.origin;
                let w = self.weights[i];
                [p.x * w, p.y * w, p.z * w, w]
            })
            .collect();
        for r in 1..=self.degree {
            for j in (r..=self.degree).rev() {
                let a = self.knots[span - self.degree + j];
                let b = self.knots[span + 1 + j - r];
                let alpha = if b == a { 0. } else { (t - a) / (b - a) };
                let previous = d[j - 1];
                for (value, previous) in d[j].iter_mut().zip(previous) {
                    *value = (1. - alpha) * previous + alpha * *value;
                }
            }
        }
        let p = d[self.degree];
        if p[3] <= 0. || !p[3].is_finite() {
            return Err(error(
                "DXF rational spline has an invalid homogeneous denominator",
            ));
        }
        let result = self.origin + Vec3::new(p[0] / p[3], p[1] / p[3], p[2] / p[3]);
        if !result.is_finite() {
            return Err(error("DXF spline evaluation overflow"));
        }
        Ok(result)
    }
    /// Adaptively subdivide a spline interval against the geometric chord tolerance.
    ///
    /// # Errors
    ///
    /// Evaluation, recursion, segment or work limits prevent the requested approximation.
    #[allow(
        clippy::float_cmp,
        reason = "Exact knot equality and subdivision stagnation are structural checks, not approximate geometry comparisons."
    )]
    fn interval(
        &mut self,
        span: usize,
        times: [f64; 2],
        ends: [Vec3; 2],
        depth: usize,
        output: &mut Vec<Vec3>,
    ) -> Result<()> {
        let middle = times[0] * 0.5 + times[1] * 0.5;
        let quarter = times[0] * 0.5 + middle * 0.5;
        let three_quarter = middle * 0.5 + times[1] * 0.5;
        let q = [
            self.evaluate(span, quarter)?,
            self.evaluate(span, middle)?,
            self.evaluate(span, three_quarter)?,
        ];
        let chord = ends[1] - ends[0];
        let length = chord.length();
        if !length.is_finite() {
            return Err(error("DXF spline coordinate range overflow"));
        }
        let distance = |p: Vec3| {
            if length == 0. {
                (p - ends[0]).length()
            } else {
                let unit = chord / length;
                let t = (p - ends[0]).dot(unit).clamp(0., length);
                (p - (ends[0] + unit * t)).length()
            }
        };
        let sampled_length = (q[0] - ends[0]).length()
            + (q[1] - q[0]).length()
            + (q[2] - q[1]).length()
            + (ends[1] - q[2]).length();
        let flat = q.iter().all(|&p| distance(p) <= self.tolerance)
            && sampled_length - length <= 4. * self.tolerance;
        if flat {
            if self.segments >= MAX_SEGMENTS {
                return Err(error("DXF spline exceeds sampled segment limit"));
            }
            self.segments += 1;
            output.push(ends[1]);
            return Ok(());
        }
        if depth >= 20 || middle == times[0] || middle == times[1] {
            return Err(error(
                "DXF spline cannot reach preview tolerance within subdivision limit",
            ));
        }
        self.interval(span, [times[0], middle], [ends[0], q[1]], depth + 1, output)?;
        self.interval(span, [middle, times[1]], [q[1], ends[1]], depth + 1, output)
    }
}

/// Validate a DXF spline's authored knots, controls and weights, then sample each continuous span.
///
/// # Errors
///
/// Spline structure, counts, knots, weights or bounded approximation are invalid or unsupported.
#[allow(
    clippy::float_cmp,
    reason = "Exact knot equality and subdivision stagnation are structural checks, not approximate geometry comparisons."
)]
pub(super) fn sample(entity: &Entity) -> Result<Vec<Vec<Vec3>>> {
    let degree = entity.integer(71, 0)?;
    if !(1..=32).contains(&degree) {
        return Err(error("DXF spline degree must be between 1 and 32"));
    }
    let degree = usize::try_from(degree)?;
    let controls = points(entity, 10)?;
    let fits = points(entity, 11)?;
    let knots = values(entity, 40)?;
    if entity.integer(72, i64::try_from(knots.len())?)? != i64::try_from(knots.len())?
        || entity.integer(73, i64::try_from(controls.len())?)? != i64::try_from(controls.len())?
        || entity.integer(74, i64::try_from(fits.len())?)? != i64::try_from(fits.len())?
    {
        return Err(error(
            "DXF spline declared counts disagree with knots/control/fit points",
        ));
    }
    if controls.len() <= degree || controls.len() > MAX_CONTROLS {
        return Err(error(
            "DXF spline requires authored control points; fit-point-only splines are not implemented",
        ));
    }
    if knots.len() != controls.len() + degree + 1
        || knots.windows(2).any(|w| w[0] > w[1])
        || knots[degree] >= knots[controls.len()]
    {
        return Err(error("invalid DXF spline knot vector"));
    }
    if knots.windows(degree + 2).any(|w| w.first() == w.last()) {
        return Err(error(
            "DXF spline knot multiplicity exceeds degree plus one",
        ));
    }
    let mut weights = values(entity, 41)?;
    if weights.is_empty() {
        weights = vec![1.; controls.len()];
    }
    if weights.len() != controls.len() || weights.iter().any(|&w| w <= 0.) {
        return Err(error(
            "DXF spline weights must be positive and match control points",
        ));
    }
    let maximum = weights.iter().copied().fold(0., f64::max);
    for w in &mut weights {
        *w /= maximum;
    }
    let origin = controls[0];
    let range = controls
        .iter()
        .map(|&p| (p - origin).length())
        .fold(0., f64::max);
    if !range.is_finite() {
        return Err(error("DXF spline coordinate range overflow"));
    }
    let mut curve = Curve {
        controls,
        weights,
        knots,
        degree,
        origin,
        work: 0,
        segments: 0,
        tolerance: (range * 1e-4).max(f64::MIN_POSITIVE),
    };
    let mut result = Vec::new();
    for span in degree..curve.controls.len() {
        let times = [curve.knots[span], curve.knots[span + 1]];
        if times[0] == times[1] {
            continue;
        }
        let ends = [
            curve.evaluate(span, times[0])?,
            curve.evaluate(span, times[1])?,
        ];
        let mut samples = vec![ends[0]];
        curve.interval(span, times, ends, 0, &mut samples)?;
        result.push(samples);
    }
    // Each knot span is stroked independently. Full-multiplicity interior knots can
    // describe discontinuities; joining those endpoints would fabricate a straight edge.
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::super::parse;
    use super::*;
    use std::fmt::Write as _;
    fn drawing(entities: &str) -> Vec<u8> {
        format!("0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n").into_bytes()
    }
    fn spline_entity(
        degree: usize,
        controls: &[[f64; 3]],
        knots: &[f64],
        weights: &[f64],
    ) -> String {
        let mut text = format!(
            "0\nSPLINE\n71\n{degree}\n72\n{}\n73\n{}\n74\n0\n",
            knots.len(),
            controls.len()
        );
        for n in knots {
            let _ = write!(text, "40\n{n}\n");
        }
        for w in weights {
            let _ = write!(text, "41\n{w}\n");
        }
        for [x, y, z] in controls {
            let _ = write!(text, "10\n{x}\n20\n{y}\n30\n{z}\n");
        }
        text
    }
    #[test]
    fn rational_spline_samples_a_real_quarter_circle_in_wcs() {
        let mut entity = spline_entity(
            2,
            &[[1., 0., 2.], [1., 1., 2.], [0., 1., 2.]],
            &[0., 0., 0., 1., 1., 1.],
            &[1., 0.5f64.sqrt(), 1.],
        );
        entity.push_str("70\n12\n210\n1\n220\n0\n230\n0\n62\n1\n");
        let scene = parse(&drawing(&entity)).unwrap();
        assert!(scene.lines.len() > 8);
        assert_eq!(scene.lines[0].vertices[0], Vec3::new(1., 0., 2.));
        assert_eq!(
            scene.lines.last().unwrap().vertices[1],
            Vec3::new(0., 1., 2.)
        );
        assert!(scene.lines.iter().all(|l| l.color == Some([255, 0, 0])));
        for p in scene.positions() {
            assert_eq!(p.z, 2.);
            assert!((p.x * p.x + p.y * p.y - 1.).abs() < 1e-12);
        }
        assert!(crate::render::render(&scene, &crate::render::RenderOptions::default()).is_ok());
    }
    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "The authored fixture verifies exact coordinates or discrete output properties."
    )]
    fn spline_sampling_retains_s_curve_and_knot_discontinuities() {
        let entity = spline_entity(
            3,
            &[[0., 0., 0.], [0., 1., 0.], [1., -1., 0.], [1., 0., 0.]],
            &[0., 0., 0., 0., 1., 1., 1., 1.],
            &[],
        );
        let scene = parse(&drawing(&entity)).unwrap();
        assert!(
            scene
                .positions()
                .any(|p| (p - Vec3::new(0.15625, 0.28125, 0.)).length() < 1e-12)
        );
        assert!(scene.positions().any(|p| p.y < -0.28));
        let jump = spline_entity(
            1,
            &[[0., 0., 0.], [1., 0., 0.], [10., 0., 0.], [11., 0., 0.]],
            &[0., 0., 0.5, 0.5, 1., 1.],
            &[],
        );
        let scene = parse(&drawing(&jump)).unwrap();
        assert_eq!(scene.lines.len(), 2);
        assert!(
            scene
                .lines
                .iter()
                .all(|l| (l.vertices[1] - l.vertices[0]).length() == 1.)
        );
        assert!(parse(&drawing(&entity.replace("72\n8", "72\n9"))).is_err());
        assert!(parse(&drawing(&entity.replace("73\n4", "73\n3"))).is_err());
        assert!(parse(&drawing(&entity.replace("40\n1", "40\n-1"))).is_err());
        let bad_weights = spline_entity(
            1,
            &[[0., 0., 0.], [1., 0., 0.]],
            &[0., 0., 1., 1.],
            &[1., 0.],
        );
        assert!(parse(&drawing(&bad_weights)).is_err());
    }
    #[test]
    fn drafting_annotations_do_not_reject_or_reframe_model_geometry() {
        let mut entities = "0\nLINE\n10\n0\n20\n0\n11\n1\n21\n0\n".to_owned();
        for kind in [
            "TEXT",
            "MTEXT",
            "ATTRIB",
            "ATTDEF",
            "DIMENSION",
            "ARC_DIMENSION",
            "LARGE_RADIAL_DIMENSION",
            "LEADER",
            "MULTILEADER",
            "TOLERANCE",
            "ACAD_TABLE",
            "VIEWPORT",
        ] {
            let _ = write!(entities, "0\n{kind}\n10\n100000\n20\n100000\n1\nlabel\n");
        }
        let scene = parse(&drawing(&entities)).unwrap();
        assert_eq!(scene.lines.len(), 1);
        assert_eq!(scene.positions().map(|p| p.x).fold(0., f64::max), 1.);
        for kind in ["HATCH", "3DSOLID", "REGION", "UNKNOWN_GEOMETRY"] {
            assert!(parse(&drawing(&format!("{entities}0\n{kind}\n"))).is_err());
        }
    }
}
