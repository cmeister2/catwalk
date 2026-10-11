//! IGES leaf geometry mapped to public ogeom constructors. Topology is built
//! separately, so independent authored faces never pass through sewing.
#![allow(
    clippy::many_single_char_names,
    reason = "IGES analytic formulas and spline charts use conventional mathematical coordinates."
)]
use super::deck::{Deck, Entity};
use crate::Result;
use ogeom_core::Tolerances;
use ogeom_geom::{
    BSplineCurve, BSplineSurface, CircleCurve, ConeSurface, Curve, Curve3d, CylinderSurface,
    EllipseCurve, ExtrusionSurface, HyperbolaCurve, LineCurve, OffsetCurve, OffsetSurface,
    ParabolaCurve, PlaneSurface, RevolutionSurface, SphereSurface, Surface, SurfaceGeometry,
    TorusSurface, Transformable, TrimmedCurve,
};
use ogeom_math::{
    Circle, Cone, ControlGrid, Cylinder, Direction, Ellipse, Frame, Hyperbola, KnotVector, Matrix3,
    Parabola, Plane, Point, Sphere, Torus, Transform, Vector, Weighted,
};
use std::collections::BTreeSet;

/// One genuine curve and its authored parameter interval.
pub(super) type Segment = (Curve, (f64, f64));

/// Bounded geometry-definition reader and actual definition ownership.
pub(super) struct Geometry<'a> {
    /// Immutable validated exchange records.
    pub deck: &'a Deck,
    /// Actual geometry definitions successfully reached by this reader.
    pub visited: BTreeSet<i64>,
    /// Author-declared coincidence precision converted to millimetres.
    pub tol: Tolerances,
    /// Current recursion depth for bounded reference expansion.
    depth: usize,
    /// Cumulative numeric definition size, including repeated references.
    expanded_parameters: usize,
}

impl<'a> Geometry<'a> {
    /// Create a geometry reader over a validated IGES deck.
    pub fn new(deck: &'a Deck, tol: Tolerances) -> Self {
        Self {
            deck,
            visited: BTreeSet::new(),
            tol,
            depth: 0,
            expanded_parameters: 0,
        }
    }

    /// Validate and account for a numeric geometry definition.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    pub fn entity(&mut self, id: i64) -> Result<&'a Entity> {
        let e = self.deck.entity(id)?;
        e.numeric_geometry()?;
        self.reserve_parameters(e.params.len())?;
        self.visited
            .insert(id.checked_abs().ok_or("IGES pointer overflow")?);
        Ok(e)
    }

    /// Charge repeated definitions and copied spline controls before expansion.
    ///
    /// # Errors
    ///
    /// Cumulative numeric geometry would exceed two million parameters.
    pub fn reserve_parameters(&mut self, count: usize) -> Result<()> {
        self.expanded_parameters = self
            .expanded_parameters
            .checked_add(count)
            .filter(|&total| total <= 2_000_000)
            .ok_or("IGES repeated geometry exceeds two million expanded parameters")?;
        Ok(())
    }

    /// Bound recursive geometry/placement references.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.depth >= 64 {
            return Err("IGES geometry reference cycle or depth exceeds 64".into());
        }
        self.depth += 1;
        let result = f(self);
        self.depth -= 1;
        result
    }

    /// Decode a chained authored model-space placement.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    pub fn placement(&mut self, e: &Entity) -> Result<Transform> {
        self.nested(|s| {
            if e.transform == 0 {
                return Ok(Transform::IDENTITY);
            }
            let t = s.entity(e.transform)?;
            if t.kind != 124 {
                return Err("IGES transform pointer does not name type124".into());
            }
            let r = |i| t.number(i);
            let matrix =
                Matrix3::new([[r(0), r(1), r(2)], [r(4), r(5), r(6)], [r(8), r(9), r(10)]]);
            let shift = Vector::new(r(3), r(7), r(11)) * s.deck.scale;
            Ok(s.placement(t)? * Transform::from_parts(matrix, 1., shift, s.tol.angular())?)
        })
    }

    /// Convert an inline model-space coordinate triple to millimetres.
    fn point(&self, e: &Entity, i: usize) -> Point {
        Point::new(
            e.number(i) * self.deck.scale,
            e.number(i + 1) * self.deck.scale,
            e.number(i + 2) * self.deck.scale,
        )
    }

    /// Decode a referenced point116.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    fn location(&mut self, id: i64) -> Result<Point> {
        let e = self.entity(id)?;
        if e.kind != 116 {
            return Err("IGES analytic surface location must be a point116".into());
        }
        Ok(self.point(e, 0))
    }

    /// Decode a referenced direction123.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    fn direction(&mut self, id: i64) -> Result<Direction> {
        let e = self.entity(id)?;
        if e.kind != 123 {
            return Err("IGES analytic surface direction must be type123".into());
        }
        Ok(Direction::from_coords(
            e.number(0),
            e.number(1),
            e.number(2),
            self.tol,
        )?)
    }

    /// Construct an analytic frame from authored axis/reference directions.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    fn frame(&mut self, p: Point, z: Direction, e: &Entity, i: usize) -> Result<Frame> {
        let id = e.integer(i)?;
        if id != 0 && self.deck.entities.contains_key(&id) {
            let r = self.direction(id)?.vector();
            if let Ok(x) = Direction::new(r - z.vector() * r.dot(z.vector()), self.tol) {
                return Ok(Frame::new(p, z, x, self.tol)?);
            }
        }
        let seed = if z.vector().dot(Vector::X).abs() < 0.9 {
            Vector::X
        } else {
            Vector::Y
        };
        Ok(Frame::new(
            p,
            z,
            Direction::from_cross(z.vector(), seed, self.tol)?,
            self.tol,
        )?)
    }

    /// Flatten an authored composite curve into genuine curve segments.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    pub fn segments(&mut self, id: i64) -> Result<Vec<Segment>> {
        self.nested(|s| {
            let e = s.entity(id)?;
            if e.kind != 102 {
                return Ok(vec![s.curve(id)?]);
            }
            let mut out = Vec::new();
            for i in 0..e.count(0)? {
                let child = s.segments(e.integer(i + 1)?)?;
                if out
                    .len()
                    .checked_add(child.len())
                    .is_none_or(|n| n > 100_000)
                {
                    return Err("IGES composite curve exceeds preview limit".into());
                }
                out.extend(child);
            }
            if out.len() > 100_000 {
                return Err("IGES composite curve exceeds preview limit".into());
            }
            Ok(out)
        })
    }

    /// Decode an authored model-space curve and its parameter range.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    pub fn curve(&mut self, id: i64) -> Result<Segment> {
        self.nested(|s| {
            let e = s.entity(id)?;
            let r = |i| e.number(i);
            let (curve, range) = match e.kind {
                110 => {
                    let a = s.point(e, 0);
                    let b = s.point(e, 3);
                    (LineCurve::segment(a, b, s.tol)?.into(), (0., a.distance(b)))
                }
                100 => {
                    let z = r(0) * s.deck.scale;
                    let p = |i| Point::new(r(i) * s.deck.scale, r(i + 1) * s.deck.scale, z);
                    let c = p(1);
                    let start = p(3);
                    let end = p(5);
                    let frame =
                        Frame::new(c, Direction::Z, Direction::new(start - c, s.tol)?, s.tol)?;
                    let v = end - c;
                    let a = v
                        .dot(frame.y().vector())
                        .atan2(v.dot(frame.x().vector()))
                        .rem_euclid(std::f64::consts::TAU);
                    (
                        CircleCurve::new(Circle::new(frame, c.distance(start), s.tol)?).into(),
                        (
                            0.,
                            if a <= s.tol.parametric() {
                                std::f64::consts::TAU
                            } else {
                                a
                            },
                        ),
                    )
                }
                104 => s.conic(e)?,
                112 => s.polynomial_curve(e)?,
                126 => {
                    let n = e.count(0)? + 1;
                    let degree = e.count(1)?;
                    if degree == 0 {
                        return Err("IGES NURBS curve degree is zero".into());
                    }
                    let nk = n + degree + 1;
                    let wb = 6 + nk;
                    let pb = wb + n;
                    if pb + 3 * n + 2 > e.params.len() {
                        return Err("truncated IGES NURBS curve".into());
                    }
                    let knots = (0..nk).map(|i| r(6 + i)).collect();
                    let controls = (0..n)
                        .map(|i| Weighted::new(s.point(e, pb + 3 * i), r(wb + i), s.tol))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    let b = BSplineCurve::rational(KnotVector::new(knots, degree)?, controls)?;
                    let domain = b.domain();
                    let lo = r(pb + 3 * n);
                    let hi = r(pb + 3 * n + 1);
                    (b.into(), if hi > lo { (lo, hi) } else { domain })
                }
                130 => s.offset_curve(e)?,
                kind => {
                    return Err(
                        format!("IGES curve type{kind} form{} is unsupported", e.form).into(),
                    );
                }
            };
            let placed = s.placement(e)?;
            Ok((
                if placed == Transform::IDENTITY {
                    curve
                } else {
                    curve.transformed(&placed, s.tol)?
                },
                range,
            ))
        })
    }

    /// Convert an implicit IGES conic to its exact public analytic geometry.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    #[allow(
        clippy::too_many_lines,
        reason = "Exact ellipse, hyperbola and parabola arms share authored coefficient normalization."
    )]
    fn conic(&self, e: &Entity) -> Result<Segment> {
        let [a, b, c, d, f, g] = std::array::from_fn(|i| e.number(i));
        let theta = if b.abs() > 1e-12 {
            0.5 * b.atan2(a - c)
        } else {
            0.
        };
        let (sin, cos) = theta.sin_cos();
        let aa = a * cos * cos + b * cos * sin + c * sin * sin;
        let cc = a * sin * sin - b * cos * sin + c * cos * cos;
        let dd = d * cos + f * sin;
        let ee = -d * sin + f * cos;
        let axis = ogeom_math::Axis::new(Point::ORIGIN, Direction::Z);
        let back = Transform::rotation(axis, theta);
        let into = Transform::rotation(axis, -theta);
        let p = |i| {
            into.apply(Point::new(
                e.number(i) * self.deck.scale,
                e.number(i + 1) * self.deck.scale,
                e.number(6) * self.deck.scale,
            ))
        };
        let start = p(7);
        let end = p(9);
        let z = start.z;
        let scale = self.deck.scale;
        let t = self.tol;
        let (curve, range): Segment = if aa != 0. && cc != 0. {
            let cx = -dd / (2. * aa);
            let cy = -ee / (2. * cc);
            let rhs = aa * cx * cx + cc * cy * cy - g;
            let center = Point::new(cx * scale, cy * scale, z);
            if (aa > 0.) == (cc > 0.) {
                let rx = (rhs / aa).sqrt() * scale;
                let ry = (rhs / cc).sqrt() * scale;
                let (x, major, minor) = if rx >= ry {
                    (Direction::X, rx, ry)
                } else {
                    (Direction::Y, ry, rx)
                };
                let frame = Frame::new(center, Direction::Z, x, t)?;
                let angle = |p| {
                    let p = frame.to_local(p);
                    (p.y / minor)
                        .atan2(p.x / major)
                        .rem_euclid(std::f64::consts::TAU)
                };
                let lo = angle(start);
                let mut hi = angle(end);
                if hi <= lo + t.parametric() {
                    hi += std::f64::consts::TAU;
                }
                (
                    EllipseCurve::new(Ellipse::new(frame, major, minor, t)?).into(),
                    (lo, hi),
                )
            } else {
                let px = aa / rhs;
                let py = cc / rhs;
                let (x, major, minor) = if px > 0. {
                    (
                        Direction::X,
                        (1. / px).sqrt() * scale,
                        (-1. / py).sqrt() * scale,
                    )
                } else {
                    (
                        Direction::Y,
                        (1. / py).sqrt() * scale,
                        (-1. / px).sqrt() * scale,
                    )
                };
                let x = if (start - center).dot(x.vector()) >= 0. {
                    x
                } else {
                    Direction::new(-x.vector(), t)?
                };
                let frame = Frame::new(center, Direction::Z, x, t)?;
                let lo = (frame.to_local(start).y / minor).asinh();
                let hi = (frame.to_local(end).y / minor).asinh();
                let frame = if hi > lo {
                    frame
                } else {
                    Frame::new(center, Direction::new(-Vector::Z, t)?, x, t)?
                };
                let lo = (frame.to_local(start).y / minor).asinh();
                let hi = (frame.to_local(end).y / minor).asinh();
                (
                    HyperbolaCurve::new(
                        Hyperbola::new(frame, major, minor, t)?,
                        lo.abs().max(hi.abs()).max(1e-3) * 1.5,
                    )?
                    .into(),
                    (lo, hi),
                )
            }
        } else if (aa == 0.) != (cc == 0.) {
            let (across, along, k, x) = if cc == 0. {
                let u = -dd / (2. * aa);
                (u, -(aa * u * u + dd * u + g) / ee, -aa / ee, Direction::Y)
            } else {
                let v = -ee / (2. * cc);
                (v, -(cc * v * v + ee * v + g) / dd, -cc / dd, Direction::X)
            };
            let apex = if cc == 0. {
                Point::new(across * scale, along * scale, z)
            } else {
                Point::new(along * scale, across * scale, z)
            };
            let x = if k >= 0. {
                x
            } else {
                Direction::new(-x.vector(), t)?
            };
            let frame = Frame::new(apex, Direction::Z, x, t)?;
            let frame = if frame.to_local(end).y > frame.to_local(start).y {
                frame
            } else {
                Frame::new(apex, Direction::new(-Vector::Z, t)?, x, t)?
            };
            let lo = frame.to_local(start).y;
            let hi = frame.to_local(end).y;
            (
                ParabolaCurve::new(
                    Parabola::new(frame, scale / (4. * k.abs()), t)?,
                    lo.abs().max(hi.abs()).max(1e-3) * 1.5,
                )?
                .into(),
                (lo, hi),
            )
        } else {
            return Err("IGES conic coefficients do not define a conic".into());
        };
        Ok((curve.transformed(&back, t)?, range))
    }

    /// Convert piecewise cubic monomials to exact spline controls.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    fn polynomial_curve(&self, e: &Entity) -> Result<Segment> {
        let n = e.count(3)?;
        if n == 0 {
            return Err("IGES polynomial curve has no spans".into());
        }
        let base = 5 + n;
        if base + 12 * n > e.params.len() {
            return Err("truncated IGES polynomial curve".into());
        }
        let mut pts = Vec::with_capacity(3 * n + 1);
        let mut knots = vec![e.number(4); 4];
        for i in 0..n {
            let h = e.number(5 + i) - e.number(4 + i);
            if h <= 0. {
                return Err("IGES polynomial curve has nonpositive span".into());
            }
            let c = |axis: usize, k: usize| e.number(base + 12 * i + 4 * axis + k);
            let p = |s| {
                Point::new(
                    c(0, 0) + s * (c(0, 1) + s * (c(0, 2) + s * c(0, 3))),
                    c(1, 0) + s * (c(1, 1) + s * (c(1, 2) + s * c(1, 3))),
                    c(2, 0) + s * (c(2, 1) + s * (c(2, 2) + s * c(2, 3))),
                )
            };
            let v = |s| {
                Vector::new(
                    c(0, 1) + 2. * s * c(0, 2) + 3. * s * s * c(0, 3),
                    c(1, 1) + 2. * s * c(1, 2) + 3. * s * s * c(1, 3),
                    c(2, 1) + 2. * s * c(2, 2) + 3. * s * s * c(2, 3),
                )
            };
            let scaled = |p: Point| {
                Point::new(
                    p.x * self.deck.scale,
                    p.y * self.deck.scale,
                    p.z * self.deck.scale,
                )
            };
            if i == 0 {
                pts.push(scaled(p(0.)));
            }
            pts.extend([
                scaled(p(0.) + v(0.) * (h / 3.)),
                scaled(p(h) - v(h) * (h / 3.)),
                scaled(p(h)),
            ]);
            if i + 1 < n {
                knots.extend([e.number(5 + i); 3]);
            }
        }
        knots.extend([e.number(4 + n); 4]);
        let b = BSplineCurve::new(KnotVector::new(knots, 3)?, pts, self.tol)?;
        let range = b.domain();
        Ok((b.into(), range))
    }

    /// Construct an authored constant or bounded fitted variable offset.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    fn offset_curve(&mut self, e: &Entity) -> Result<Segment> {
        let (basis, original) = self.curve(e.integer(0)?)?;
        let reference = Direction::from_coords(e.number(9), e.number(10), e.number(11), self.tol)?;
        let range = if e.number(13) > e.number(12) {
            (e.number(12), e.number(13))
        } else {
            original
        };
        let kind = e.integer(1)?;
        if kind == 1 && (e.number(5) - e.number(7)).abs() <= 1e-12 {
            return Ok((
                Curve::Offset(Box::new(OffsetCurve::new(
                    basis,
                    -e.number(5) * self.deck.scale,
                    reference,
                )?)),
                range,
            ));
        }
        let stations = ogeom_algo::traced::stations(&basis, range, 400);
        let mut lengths = vec![0.];
        if kind == 2 {
            for pair in stations.windows(2) {
                lengths.push(
                    lengths[lengths.len() - 1]
                        + ogeom_algo::curve_length(&basis, (pair[0], pair[1]), self.tol)?
                            / self.deck.scale,
                );
            }
        }
        let function = if kind == 3 {
            Some(self.curve(e.integer(2)?)?.0)
        } else {
            None
        };
        if !matches!(kind, 2 | 3) {
            return Err("IGES offset curve has unsupported distance law".into());
        }
        let t = self.tol;
        let scale = self.deck.scale;
        let fitted = ogeom_algo::traced::fit_traced(
            |u| {
                let distance = if let Some(c) = &function {
                    let p = c.point_at(u, t)?;
                    match e.number(3) {
                        1. => p.x / scale,
                        2. => p.y / scale,
                        _ => p.z / scale,
                    }
                } else {
                    let i = stations.partition_point(|&v| v <= u).saturating_sub(1);
                    let along =
                        lengths[i] + ogeom_algo::curve_length(&basis, (stations[i], u), t)? / scale;
                    let span = e.number(8) - e.number(6);
                    let fraction = if span == 0. {
                        0.
                    } else {
                        (along - e.number(6)) / span
                    };
                    e.number(5) + (e.number(7) - e.number(5)) * fraction
                };
                let side = reference.vector().cross(basis.d1_at(u, t)?);
                let mag = side.magnitude();
                if mag <= t.angular() {
                    return Err(ogeom_core::ogeom_err!(
                        Construction,
                        "IGES offset reference is parallel to its tangent"
                    ));
                }
                Ok(basis.point_at(u, t)? + side * (distance * scale / mag))
            },
            &stations,
            3,
            t.confusion() * 100.,
            t,
        )?;
        if !fitted.met {
            return Err(format!(
                "IGES varying offset exceeds fit tolerance: {}",
                fitted.error
            )
            .into());
        }
        Ok((fitted.curve.into(), range))
    }

    /// Decode an authored surface through public geometry constructors.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    #[allow(
        clippy::too_many_lines,
        reason = "One constructor per supported IGES surface family keeps the schema mapping explicit."
    )]
    pub fn surface(&mut self, id: i64) -> Result<SurfaceGeometry> {
        self.nested(|s| {
            let e = s.entity(id)?;
            let r = |i| e.number(i);
            let scale = s.deck.scale;
            let t = s.tol;
            let extent = (-1e5, 1e5);
            let surf: SurfaceGeometry = match e.kind {
                108 => {
                    let n = Vector::new(r(0), r(1), r(2));
                    let p = Point::ORIGIN + n * (r(3) / n.dot(n) * scale);
                    PlaneSurface::over(Plane::through(p, Direction::new(n, t)?), extent, extent)?
                        .into()
                }
                190 => {
                    let p = s.location(e.integer(0)?)?;
                    let n = s.direction(e.integer(1)?)?;
                    let plane = if e.integer(2)? != 0 {
                        Plane::new(s.frame(p, n, e, 2)?)
                    } else {
                        Plane::through(p, n)
                    };
                    PlaneSurface::over(plane, extent, extent)?.into()
                }
                192 => {
                    let p = s.location(e.integer(0)?)?;
                    let n = s.direction(e.integer(1)?)?;
                    CylinderSurface::new(
                        Cylinder::new(s.frame(p, n, e, 3)?, r(2) * scale, t)?,
                        extent,
                    )?
                    .into()
                }
                194 => {
                    let p = s.location(e.integer(0)?)?;
                    let n = s.direction(e.integer(1)?)?;
                    ConeSurface::new(
                        Cone::new(s.frame(p, n, e, 4)?, r(2) * scale, r(3).to_radians(), t)?,
                        extent,
                    )?
                    .into()
                }
                196 => {
                    let p = s.location(e.integer(0)?)?;
                    let sphere = if e.integer(2)? != 0 {
                        let n = s.direction(e.integer(2)?)?;
                        Sphere::new(s.frame(p, n, e, 3)?, r(1) * scale, t)?
                    } else {
                        Sphere::centred(p, r(1) * scale, t)?
                    };
                    SphereSurface::new(sphere).into()
                }
                198 => {
                    let p = s.location(e.integer(0)?)?;
                    let n = s.direction(e.integer(1)?)?;
                    TorusSurface::new(Torus::new(
                        s.frame(p, n, e, 4)?,
                        r(2) * scale,
                        r(3) * scale,
                        t,
                    )?)
                    .into()
                }
                128 => s.nurbs_surface(e)?,
                114 => s.polynomial_surface(e)?,
                118 => s.ruled_surface(e)?,
                120 => {
                    let (axis, _) = s.curve(e.integer(0)?)?;
                    let Curve::Line(axis) = axis else {
                        return Err("IGES revolution axis is not a line".into());
                    };
                    let (c, range) = s.curve(e.integer(1)?)?;
                    let c = trimmed(c, range, t)?;
                    let c = if r(2).abs() > t.parametric() {
                        c.transformed(&Transform::rotation(axis.axis(), r(2)), t)?
                    } else {
                        c
                    };
                    RevolutionSurface::new(
                        c,
                        axis.axis(),
                        if r(3) > r(2) {
                            r(3) - r(2)
                        } else {
                            std::f64::consts::TAU
                        },
                    )?
                    .into()
                }
                122 => {
                    let (c, range) = s.curve(e.integer(0)?)?;
                    let v = s.point(e, 1) - c.point_at(range.0, t)?;
                    ExtrusionSurface::new(
                        trimmed(c, range, t)?,
                        Direction::new(v, t)?,
                        v.magnitude(),
                    )?
                    .into()
                }
                140 => {
                    let given = Direction::from_coords(r(0), r(1), r(2), t)?;
                    let basis = s.surface(e.integer(4)?)?;
                    let ((ua, ub), (va, vb)) = basis.domain();
                    let n = basis.normal_at(f64::midpoint(ua, ub), f64::midpoint(va, vb), t)?;
                    let sign = if n.vector().dot(given.vector()) < 0. {
                        -1.
                    } else {
                        1.
                    };
                    SurfaceGeometry::Offset(Box::new(OffsetSurface::new(
                        basis,
                        sign * r(3) * scale,
                    )?))
                }
                kind => {
                    return Err(
                        format!("IGES surface type{kind} form{} is unsupported", e.form).into(),
                    );
                }
            };
            let placement = s.placement(e)?;
            Ok(if placement == Transform::IDENTITY {
                surf
            } else {
                surf.transformed(&placement, t)?
            })
        })
    }

    /// Map the file's rational u-fast control net to the public surface grid.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    fn nurbs_surface(&self, e: &Entity) -> Result<SurfaceGeometry> {
        let nu = e.count(0)? + 1;
        let nv = e.count(1)? + 1;
        let du = e.count(2)?;
        let dv = e.count(3)?;
        let cells = nu
            .checked_mul(nv)
            .filter(|&n| n <= e.params.len())
            .ok_or("IGES surface control grid exceeds its record")?;
        let ku = nu + du + 1;
        let kv = nv + dv + 1;
        let wb = 9 + ku + kv;
        let pb = wb + cells;
        if du == 0 || dv == 0 || pb + 3 * cells > e.params.len() {
            return Err("invalid/truncated IGES NURBS surface".into());
        }
        let mut controls = Vec::with_capacity(cells);
        for u in 0..nu {
            for v in 0..nv {
                let i = v * nu + u;
                controls.push(Weighted::new(
                    self.point(e, pb + 3 * i),
                    e.number(wb + i),
                    self.tol,
                )?);
            }
        }
        let u = (0..ku).map(|i| e.number(9 + i)).collect();
        let v = (0..kv).map(|i| e.number(9 + ku + i)).collect();
        Ok(BSplineSurface::rational(
            KnotVector::new(u, du)?,
            KnotVector::new(v, dv)?,
            ControlGrid::new(controls, nu, nv)?,
        )?
        .into())
    }

    /// Unify the two exact curve nets and span them with a degree-one surface.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    fn ruled_surface(&mut self, e: &Entity) -> Result<SurfaceGeometry> {
        let (a, ar) = self.curve(e.integer(0)?)?;
        let (b, br) = self.curve(e.integer(1)?)?;
        let mut a = a.to_bspline_over(ar, self.tol)?;
        let mut b = b.to_bspline_over(br, self.tol)?;
        if e.integer(2)? == 1 {
            let Curve::BSpline(back) = ogeom_geom::Reversible::reversed(&Curve::BSpline(b)) else {
                return Err("invalid reversed IGES ruled curve".into());
            };
            b = back;
        }
        while a.degree() < b.degree() {
            a = a.elevated(self.tol)?;
        }
        while b.degree() < a.degree() {
            b = b.elevated(self.tol)?;
        }
        unify_knots(&mut a, &b, self.tol)?;
        unify_knots(&mut b, &a, self.tol)?;
        let n = a.control_points().len();
        if b.control_points().len() != n {
            return Err("IGES ruled curves have different refined nets".into());
        }
        let controls = (0..n)
            .flat_map(|i| [a.control_points()[i], b.control_points()[i]])
            .collect();
        Ok(BSplineSurface::rational(
            a.knots().clone(),
            KnotVector::new(vec![0., 0., 1., 1.], 1)?,
            ControlGrid::new(controls, n, 2)?,
        )?
        .into())
    }

    /// Convert bicubic monomials into exact tensor spline controls.
    ///
    /// # Errors
    ///
    /// Invalid definitions, referenced entities or kernel construction fail.
    fn polynomial_surface(&self, e: &Entity) -> Result<SurfaceGeometry> {
        let m = e.count(2)?;
        let n = e.count(3)?;
        if m == 0 || n == 0 {
            return Err("IGES polynomial surface has no patches".into());
        }
        let rows = 3 * m + 1;
        let cols = 3 * n + 1;
        let cells = rows
            .checked_mul(cols)
            .filter(|&n| n <= 100_000)
            .ok_or("IGES polynomial surface exceeds preview net limit")?;
        let base = 6 + m + n;
        let last = base
            .checked_add((m * (n + 1) + n) * 48)
            .filter(|&v| v <= e.params.len())
            .ok_or("truncated IGES polynomial surface")?;
        let _ = last;
        let mut net = vec![Point::ORIGIN; cells];
        for i in 0..m {
            for j in 0..n {
                let hu = e.number(5 + i) - e.number(4 + i);
                let hv = e.number(6 + m + j) - e.number(5 + m + j);
                if hu <= 0. || hv <= 0. {
                    return Err("IGES polynomial surface has nonpositive span".into());
                }
                let at = base + (i * (n + 1) + j) * 48;
                for r in 0..4 {
                    for c in 0..4 {
                        let coords = std::array::from_fn::<_, 3, _>(|axis| {
                            let mut value = 0.;
                            for (mu, &u_factor) in CUBIC_BERNSTEIN[r].iter().enumerate() {
                                for (lv, &v_factor) in CUBIC_BERNSTEIN[c].iter().enumerate() {
                                    value += u_factor
                                        * v_factor
                                        * e.number(at + axis * 16 + mu + 4 * lv)
                                        * hu.powi(i32::try_from(mu).unwrap_or(0))
                                        * hv.powi(i32::try_from(lv).unwrap_or(0));
                                }
                            }
                            value * self.deck.scale
                        });
                        net[(3 * i + r) * cols + 3 * j + c] =
                            Point::new(coords[0], coords[1], coords[2]);
                    }
                }
            }
        }
        let knots = |n: usize, base: usize| {
            let mut k = vec![e.number(base); 4];
            for i in 1..n {
                k.extend([e.number(base + i); 3]);
            }
            k.extend([e.number(base + n); 4]);
            k
        };
        let controls = net
            .into_iter()
            .map(|p| Weighted::new(p, 1., self.tol))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(BSplineSurface::rational(
            KnotVector::new(knots(m, 4), 3)?,
            KnotVector::new(knots(n, 5 + m), 3)?,
            ControlGrid::new(controls, rows, cols)?,
        )?
        .into())
    }
}

/// Retain the curve's authored finite parameter window.
///
/// # Errors
///
/// Invalid definitions, referenced entities or kernel construction fail.
fn trimmed(c: Curve, r: (f64, f64), t: Tolerances) -> Result<Curve> {
    let d = c.domain();
    if (r.0 - d.0).abs() < t.parametric() && (r.1 - d.1).abs() < t.parametric() {
        Ok(c)
    } else {
        Ok(TrimmedCurve::new(c, r.0, r.1, t)?.into())
    }
}

/// Refine one curve to include the other curve's interior knots.
///
/// # Errors
///
/// Knot insertion fails for an invalid spline.
fn unify_knots(target: &mut BSplineCurve, source: &BSplineCurve, tol: Tolerances) -> Result<()> {
    let (lo, hi) = source.knots().domain();
    for (value, count) in source.knots().distinct() {
        if value > lo + 1e-12 && value < hi - 1e-12 {
            let held = target.knots().multiplicity_of(value);
            if count > held {
                *target = target.with_knot_inserted(value, count - held, tol)?;
            }
        }
    }
    Ok(())
}

/// Cubic monomial-to-Bernstein conversion coefficients.
const CUBIC_BERNSTEIN: [[f64; 4]; 4] = [
    [1., 0., 0., 0.],
    [1., 1. / 3., 0., 0.],
    [1., 2. / 3., 1. / 3., 0.],
    [1., 1., 1., 1.],
];
