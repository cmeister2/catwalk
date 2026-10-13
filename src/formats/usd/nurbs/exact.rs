//! Exact homogeneous span polynomials, common-factor removal and Sturm certificates.
//!
//! Original binary scalar values become exact rationals before any arithmetic.
//! The four de Boor components are polynomials in the trimmed span's normalized
//! parameter. Cancelling their polynomial GCD preserves all finite removable
//! limits. Sturm's theorem then counts every remaining denominator root in the
//! closed span, including multiple roots with no sign change between samples.

use super::{Budget, Curve, STEPS};
use crate::{Result, scene::Vec3};
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};

/// Maximum numerator or denominator bits, checked before arithmetic allocations.
const MAX_BITS: u64 = 8192;
/// Increasing-power coefficients; an empty vector is the exact zero polynomial.
type Polynomial = Vec<BigRational>;
/// Numerator X/Y/Z and denominator polynomials of one homogeneous span.
type Homogeneous = [Polynomial; 4];

/// Checked rational arithmetic sharing the entire stage's operation budget.
struct Arithmetic<'a> {
    /// Shared counter prevents many small prims from resetting exact work limits.
    budget: &'a mut Budget,
}

impl Arithmetic<'_> {
    /// Check worst-case unreduced integer sizes before allocating an arithmetic result.
    fn check(&mut self, numerator: u64, denominator: u64) -> Result<()> {
        self.budget.exact_operation()?;
        if numerator > MAX_BITS || denominator > MAX_BITS {
            return Err("USD NURBS exact coefficient bit budget exceeded".into());
        }
        self.budget.exact_allocation(numerator + denominator)?;
        Ok(())
    }

    /// Add rationals after bounding both cross products and their sum.
    fn add(&mut self, a: &BigRational, b: &BigRational) -> Result<BigRational> {
        self.check(
            (a.numer().bits() + b.denom().bits()).max(b.numer().bits() + a.denom().bits()) + 1,
            a.denom().bits() + b.denom().bits(),
        )?;
        Ok(a + b)
    }

    /// Subtract rationals with the same preallocation bound as addition.
    fn sub(&mut self, a: &BigRational, b: &BigRational) -> Result<BigRational> {
        self.check(
            (a.numer().bits() + b.denom().bits()).max(b.numer().bits() + a.denom().bits()) + 1,
            a.denom().bits() + b.denom().bits(),
        )?;
        Ok(a - b)
    }

    /// Multiply rationals after bounding the unreduced integer products.
    fn mul(&mut self, a: &BigRational, b: &BigRational) -> Result<BigRational> {
        self.check(
            a.numer().bits() + b.numer().bits(),
            a.denom().bits() + b.denom().bits(),
        )?;
        Ok(a * b)
    }

    /// Divide nonzero rationals after bounding both unreduced integer products.
    fn div(&mut self, a: &BigRational, b: &BigRational) -> Result<BigRational> {
        if b.is_zero() {
            return Err("USD NURBS singular exact denominator".into());
        }
        self.check(
            a.numer().bits() + b.denom().bits(),
            a.denom().bits() + b.numer().bits(),
        )?;
        Ok(a / b)
    }
}

/// Retain the exact dyadic value represented by a finite authored scalar.
fn scalar(value: f64) -> Result<BigRational> {
    BigRational::from_float(value).ok_or_else(|| "USD NURBS scalar must be finite".into())
}

/// Remove exactly zero highest-degree terms without using a numeric tolerance.
fn trim(polynomial: &mut Polynomial) {
    while polynomial.last().is_some_and(Zero::is_zero) {
        polynomial.pop();
    }
}

/// Multiply by a linear polynomial and add it to a second polynomial.
fn blend(
    left: &Polynomial,
    right: &Polynomial,
    alpha: [&BigRational; 2],
    arithmetic: &mut Arithmetic<'_>,
) -> Result<Polynomial> {
    let n = left.len().max(right.len());
    let mut output = vec![BigRational::zero(); n + 1];
    for i in 0..n {
        let a = left.get(i).cloned().unwrap_or_default();
        let b = right.get(i).cloned().unwrap_or_default();
        let delta = arithmetic.sub(&b, &a)?;
        let constant = arithmetic.mul(&delta, alpha[0])?;
        let constant = arithmetic.add(&a, &constant)?;
        output[i] = arithmetic.add(&output[i], &constant)?;
        let linear = arithmetic.mul(&delta, alpha[1])?;
        output[i + 1] = arithmetic.add(&output[i + 1], &linear)?;
    }
    trim(&mut output);
    Ok(output)
}

/// Build exact span polynomials by symbolic de Boor interpolation on original scalars.
fn extract(
    curve: &Curve<'_>,
    span: usize,
    range: [f64; 2],
    arithmetic: &mut Arithmetic<'_>,
) -> Result<Homogeneous> {
    let first = span + 1 - curve.order;
    let start = scalar(range[0])?;
    let end = scalar(range[1])?;
    let length = arithmetic.sub(&end, &start)?;
    let mut controls = Vec::<Homogeneous>::with_capacity(curve.order);
    for i in first..=span {
        let weight = scalar(curve.weight(i))?;
        let point = curve.points[i];
        let mut homogeneous = std::array::from_fn(|_| Vec::new());
        for (axis, coordinate) in [point.x, point.y, point.z].into_iter().enumerate() {
            homogeneous[axis] = vec![arithmetic.mul(&scalar(coordinate)?, &weight)?];
        }
        homogeneous[3] = vec![weight];
        controls.push(homogeneous);
    }
    for depth in 1..curve.order {
        for j in (depth..curve.order).rev() {
            let i = first + j;
            let knot = scalar(curve.knots[i])?;
            let high = scalar(curve.knots[i + curve.order - depth])?;
            let width = arithmetic.sub(&high, &knot)?;
            let displacement = arithmetic.sub(&start, &knot)?;
            let constant = arithmetic.div(&displacement, &width)?;
            let linear = arithmetic.div(&length, &width)?;
            let (before, after) = controls.split_at_mut(j);
            for (previous, current) in before[j - 1].iter().zip(&mut after[0]) {
                *current = blend(previous, current, [&constant, &linear], arithmetic)?;
            }
        }
    }
    let mut result = controls.pop().ok_or("USD NURBS has no span controls")?;
    for polynomial in &mut result {
        trim(polynomial);
    }
    Ok(result)
}

/// Perform exact polynomial division; every iteration removes the highest remainder term.
fn divide(
    dividend: &Polynomial,
    divisor: &Polynomial,
    arithmetic: &mut Arithmetic<'_>,
) -> Result<(Polynomial, Polynomial)> {
    let lead = divisor.last().ok_or("USD NURBS zero polynomial divisor")?;
    let mut remainder = dividend.clone();
    let mut quotient = vec![BigRational::zero(); dividend.len().saturating_sub(divisor.len()) + 1];
    while remainder.len() >= divisor.len() && !remainder.is_empty() {
        let offset = remainder.len() - divisor.len();
        let factor = arithmetic.div(&remainder[remainder.len() - 1], lead)?;
        quotient[offset] = factor.clone();
        for (i, coefficient) in divisor.iter().enumerate() {
            let term = arithmetic.mul(&factor, coefficient)?;
            remainder[offset + i] = arithmetic.sub(&remainder[offset + i], &term)?;
        }
        trim(&mut remainder);
    }
    trim(&mut quotient);
    Ok((quotient, remainder))
}

/// Scale a polynomial to a unit-magnitude leading coefficient without changing its sign.
fn normalize(polynomial: &mut Polynomial, arithmetic: &mut Arithmetic<'_>) -> Result<()> {
    let Some(lead) = polynomial.last().map(Signed::abs) else {
        return Ok(());
    };
    for value in polynomial {
        *value = arithmetic.div(value, &lead)?;
    }
    Ok(())
}

/// Compute a polynomial GCD by exact degree-decreasing Euclidean remainders.
fn gcd(
    mut a: Polynomial,
    mut b: Polynomial,
    arithmetic: &mut Arithmetic<'_>,
) -> Result<Polynomial> {
    normalize(&mut a, arithmetic)?;
    normalize(&mut b, arithmetic)?;
    while !b.is_empty() {
        let (_, mut remainder) = divide(&a, &b, arithmetic)?;
        normalize(&mut remainder, arithmetic)?;
        a = b;
        b = remainder;
    }
    Ok(a)
}

/// Cancel exactly common factors from all homogeneous components, including zero numerators.
fn reduce(homogeneous: &mut Homogeneous, arithmetic: &mut Arithmetic<'_>) -> Result<()> {
    if homogeneous[3].is_empty() {
        return Err("USD NURBS denominator is identically zero".into());
    }
    let mut common = homogeneous[3].clone();
    for numerator in &homogeneous[..3] {
        if !numerator.is_empty() {
            common = gcd(common, numerator.clone(), arithmetic)?;
        }
        if common.len() <= 1 {
            return Ok(());
        }
    }
    for polynomial in homogeneous {
        let (quotient, remainder) = divide(polynomial, &common, arithmetic)?;
        if !remainder.is_empty() {
            return Err("USD NURBS exact factor cancellation failed".into());
        }
        *polynomial = quotient;
    }
    Ok(())
}

/// Evaluate an exact polynomial with Horner's rule at a rational parameter.
fn evaluate(
    polynomial: &Polynomial,
    t: &BigRational,
    arithmetic: &mut Arithmetic<'_>,
) -> Result<BigRational> {
    let mut value = BigRational::zero();
    for coefficient in polynomial.iter().rev() {
        value = arithmetic.mul(&value, t)?;
        value = arithmetic.add(&value, coefficient)?;
    }
    Ok(value)
}

/// Evaluate successive nonzero signs at an endpoint and count sign variations.
fn variations(
    sequence: &[Polynomial],
    t: &BigRational,
    arithmetic: &mut Arithmetic<'_>,
) -> Result<usize> {
    let mut previous = 0_i8;
    let mut count = 0_usize;
    for polynomial in sequence {
        let value = evaluate(polynomial, t, arithmetic)?;
        let sign = if value.is_positive() {
            1
        } else if value.is_negative() {
            -1
        } else {
            0
        };
        if sign != 0 {
            if previous != 0 && previous != sign {
                count += 1;
            }
            previous = sign;
        }
    }
    Ok(count)
}

/// Prove the reduced denominator has no roots anywhere on the entire closed span.
fn certify(denominator: &Polynomial, arithmetic: &mut Arithmetic<'_>) -> Result<()> {
    let zero = BigRational::zero();
    let one = BigRational::one();
    if evaluate(denominator, &zero, arithmetic)?.is_zero()
        || evaluate(denominator, &one, arithmetic)?.is_zero()
    {
        return Err("USD NURBS denominator has a nonremovable endpoint pole".into());
    }
    let mut derivative = Vec::with_capacity(denominator.len().saturating_sub(1));
    for (i, coefficient) in denominator.iter().enumerate().skip(1) {
        derivative.push(arithmetic.mul(coefficient, &BigRational::from_integer(i.into()))?);
    }
    trim(&mut derivative);
    if derivative.is_empty() {
        return Ok(());
    }
    let mut first = denominator.clone();
    normalize(&mut first, arithmetic)?;
    normalize(&mut derivative, arithmetic)?;
    let mut sequence = vec![first, derivative];
    loop {
        let n = sequence.len();
        let (_, mut remainder) = divide(&sequence[n - 2], &sequence[n - 1], arithmetic)?;
        if remainder.is_empty() {
            break;
        }
        for coefficient in &mut remainder {
            *coefficient = -coefficient.clone();
        }
        normalize(&mut remainder, arithmetic)?;
        sequence.push(remainder);
    }
    if variations(&sequence, &zero, arithmetic)? != variations(&sequence, &one, arithmetic)? {
        return Err("USD NURBS denominator has a nonremovable interior pole".into());
    }
    Ok(())
}

/// Return certified finite samples, evaluating the same reduced rational curve.
///
/// # Errors
/// Returns an error for exhausted arithmetic budgets, an identically zero
/// denominator, a nonremovable pole, or an unrepresentable finite position.
pub(super) fn samples(
    curve: &Curve<'_>,
    span: usize,
    range: [f64; 2],
    budget: &mut Budget,
) -> Result<Vec<Vec3>> {
    let mut arithmetic = Arithmetic { budget };
    let mut homogeneous = extract(curve, span, range, &mut arithmetic)?;
    reduce(&mut homogeneous, &mut arithmetic)?;
    certify(&homogeneous[3], &mut arithmetic)?;
    let count = if curve.order == 1 { 0 } else { STEPS };
    let mut output = Vec::with_capacity(count + 1);
    for step in 0..=count {
        let t = BigRational::new(step.into(), STEPS.into());
        let denominator = evaluate(&homogeneous[3], &t, &mut arithmetic)?;
        let mut coordinates = [0_f64; 3];
        for (axis, coordinate) in coordinates.iter_mut().enumerate() {
            let numerator = evaluate(&homogeneous[axis], &t, &mut arithmetic)?;
            let value = arithmetic.div(&numerator, &denominator)?;
            *coordinate = value
                .to_f64()
                .filter(|v| v.is_finite())
                .ok_or("USD NURBS evaluated position is not finite")?;
        }
        output.push(Vec3::new(coordinates[0], coordinates[1], coordinates[2]));
    }
    Ok(output)
}
