//! Student's t distribution: quantiles for confidence intervals (ADR-0014).
//!
//! Only `libm` and correctly rounded `f64` operations are used, so every
//! quantile is the same on every platform (ADR-0013).
//!
//! Method, by degrees of freedom ν and upper-tail probability q:
//!
//! - ν = 1 and ν = 2: closed forms.
//! - ν ≥ 3000 with z² ≤ ν/100 (z the normal quantile): the Cornish–Fisher
//!   expansion of Abramowitz & Stegun 26.7.5 to the ν⁻⁴ term. Its error is
//!   below 2·10⁻¹⁴ relative there (measured with mpmath).
//! - Otherwise: Hill's (1970) approximation as a start, refined by Newton's
//!   method on the exact CDF, safeguarded by bisection. The CDF is the
//!   regularized incomplete beta function, evaluated with its continued
//!   fraction (modified Lentz) and a prefactor written through the t density.

use std::f64::consts::PI;

use libm::{exp, expm1, log, log1p, pow, tan};

use crate::{ConfidenceLevel, Probability, StatsError};

/// Student's t distribution with a whole number of degrees of freedom.
///
/// ```
/// use taktsim_stats::{ConfidenceLevel, Probability, StudentT};
///
/// let t = StudentT::new(9)?;
/// let q = t.quantile(Probability::new(0.975)?);
/// assert!((q - 2.262_157_162_798_205).abs() < 1e-14);
/// assert_eq!(t.critical_value(ConfidenceLevel::DEFAULT), q);
/// # Ok::<(), taktsim_stats::StatsError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StudentT {
    df: u32,
}

impl StudentT {
    /// The t distribution with `df` degrees of freedom.
    ///
    /// # Errors
    ///
    /// [`StatsError::Zero`] if `df` is 0.
    ///
    /// ```
    /// use taktsim_stats::StudentT;
    ///
    /// assert_eq!(StudentT::new(4)?.degrees_of_freedom(), 4);
    /// assert!(StudentT::new(0).is_err());
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    pub fn new(df: u32) -> Result<Self, StatsError> {
        if df == 0 {
            Err(StatsError::Zero {
                parameter: "degrees_of_freedom",
                example: "9",
            })
        } else {
            Ok(Self { df })
        }
    }

    /// The degrees of freedom.
    ///
    /// ```
    /// use taktsim_stats::StudentT;
    ///
    /// assert_eq!(StudentT::new(29)?.degrees_of_freedom(), 29);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn degrees_of_freedom(self) -> u32 {
        self.df
    }

    /// The quantile: the `t` with P(T ≤ t) = `p`.
    ///
    /// Accurate to about 10⁻¹⁴ relative for the levels used in confidence
    /// intervals, checked against mpmath (`tests/t_quantiles.rs`).
    ///
    /// ```
    /// use taktsim_stats::{Probability, StudentT};
    ///
    /// let t = StudentT::new(1)?;
    /// assert!((t.quantile(Probability::new(0.75)?) - 1.0).abs() < 1e-15);
    /// assert_eq!(t.quantile(Probability::new(0.5)?), 0.0);
    /// assert_eq!(
    ///     t.quantile(Probability::new(0.25)?),
    ///     -t.quantile(Probability::new(0.75)?)
    /// );
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn quantile(self, p: Probability) -> f64 {
        let p = p.get();
        if p > 0.5 {
            // Exact (Sterbenz): p is within a factor 2 of 1.
            upper_quantile(self.df, 1.0 - p)
        } else if p < 0.5 {
            -upper_quantile(self.df, p)
        } else {
            0.0
        }
    }

    /// The two-sided critical value t with P(|T| ≤ t) = `level`, i.e. the
    /// quantile at 1 − α/2, computed from α without rounding 1 − α/2.
    ///
    /// ```
    /// use taktsim_stats::{ConfidenceLevel, StudentT};
    ///
    /// let t = StudentT::new(4)?.critical_value(ConfidenceLevel::DEFAULT);
    /// assert!((t - 2.776_445_105_197_793_5).abs() < 1e-14);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn critical_value(self, level: ConfidenceLevel) -> f64 {
        upper_quantile(self.df, level.alpha() / 2.0)
    }
}

/// Smallest degrees of freedom for the Cornish–Fisher expansion.
const EXPANSION_MIN_DF: u32 = 3000;

/// The `t > 0` with P(T > t) = `q`, for 0 < q < 0.5.
fn upper_quantile(df: u32, q: f64) -> f64 {
    let nu = f64::from(df);
    match df {
        1 => {
            // P(T > t) = 1/2 − atan(t)/π.
            if q < 0.25 {
                1.0 / tan(PI * q)
            } else {
                // 0.5 − q is exact for q in [0.25, 0.5].
                tan(PI * (0.5 - q))
            }
        }
        // P(T > t) = (1 − t/√(2 + t²))/2, solved for t.
        2 => (1.0 - 2.0 * q) / (2.0 * q * (1.0 - q)).sqrt(),
        _ => {
            let z = normal_upper_quantile(q);
            if df >= EXPANSION_MIN_DF && z * z <= nu / 100.0 {
                cornish_fisher(z, nu)
            } else {
                newton(df, q, hill_start(nu, q, z))
            }
        }
    }
}

/// Abramowitz & Stegun 26.7.5: t = z + g₁/ν + g₂/ν² + g₃/ν³ + g₄/ν⁴.
fn cornish_fisher(z: f64, nu: f64) -> f64 {
    let z2 = z * z;
    let g1 = z * (z2 + 1.0) / 4.0;
    let g2 = z * ((5.0 * z2 + 16.0) * z2 + 3.0) / 96.0;
    let g3 = z * (((3.0 * z2 + 19.0) * z2 + 17.0) * z2 - 15.0) / 384.0;
    let g4 = z * ((((79.0 * z2 + 776.0) * z2 + 1482.0) * z2 - 1920.0) * z2 - 945.0) / 92160.0;
    z + (g1 + (g2 + (g3 + g4 / nu) / nu) / nu) / nu
}

/// Hill (1970), ACM Algorithm 396, as the start for Newton's method; it is
/// accurate to a few digits, which is all Newton needs.
// The names a, b, c, d, x, y are Hill's, so the code can be checked line by
// line against the published algorithm.
#[allow(clippy::many_single_char_names)]
fn hill_start(nu: f64, q: f64, z: f64) -> f64 {
    // Hill's P is the two-sided tail probability.
    let p2 = 2.0 * q;
    let a = 1.0 / (nu - 0.5);
    let b = 48.0 / (a * a);
    let mut c = ((20700.0 * a / b - 98.0) * a - 16.0) * a + 96.36;
    let d = ((94.5 / (b + c) - 3.0) / b + 1.0) * (a * PI / 2.0).sqrt() * nu;
    let mut y = pow(d * p2, 2.0 / nu);
    if y > 0.05 + a {
        // Asymptotic inverse expansion about the normal; x is the lower
        // normal quantile at q.
        let x = -z;
        y = x * x;
        if nu < 5.0 {
            c += 0.3 * (nu - 4.5) * (x + 0.6);
        }
        c += (((0.05 * d * x - 5.0) * x - 7.0) * x - 2.0) * x + b;
        y = (((((0.4 * y + 6.3) * y + 36.0) * y + 94.5) / c - y - 3.0) / b + 1.0) * x;
        y = expm1(a * y * y);
    } else {
        y = ((1.0 / (((nu + 6.0) / (nu * y) - 0.089 * d - 0.822) * (nu + 2.0) * 3.0)
            + 0.5 / (nu + 4.0))
            * y
            - 1.0)
            * (nu + 1.0)
            / (nu + 2.0)
            + 1.0 / y;
    }
    let t = (nu * y).sqrt();
    if t.is_finite() && t > 0.0 { t } else { z }
}

/// Most Newton steps; a safeguarded step at least halves the bracket in
/// logarithmic terms, so this is never reached in practice.
const MAX_NEWTON_STEPS: u32 = 200;

/// Newton's method for P(T > t) = q from `start`, with a bracket
/// `lo < root < hi` that every step must stay inside (else bisect).
///
/// For q < 1/4 it solves ln P(T > t) = ln q in u = ln t, which is nearly
/// linear in the heavy tails; otherwise P(|T| < t) = 1 − 2q in t, whose
/// right-hand side is exact.
fn newton(df: u32, q: f64, start: f64) -> f64 {
    let ln_q = log(q);
    let central_target = 1.0 - 2.0 * q;
    let (mut lo, mut hi) = (0.0_f64, f64::INFINITY);
    let mut t = start;
    for _ in 0..MAX_NEWTON_STEPS {
        let e = evaluate(t, df);
        // Decide the side with the same quantity Newton solves for: near
        // the median, P(T > t) ≈ 1/2 has lost the digits that P(|T| < t)
        // still has.
        let below_root = if q < 0.25 {
            e.ln_upper > ln_q
        } else {
            e.central < central_target
        };
        if below_root {
            lo = t;
        } else {
            hi = t;
        }
        let proposed = if q < 0.25 {
            // h(u) = ln P(T > eᵘ) − ln q has h'(u) = −t f(t)/P(T > t).
            t * exp((e.ln_upper - ln_q) * e.upper_over_t_density)
        } else {
            // k(t) = P(|T| < t) − (1 − 2q) has k'(t) = 2 f(t), and
            // 1/(2 f) = t · (P(T > t)/(t f)) / (2 P(T > t)).
            t - (e.central - central_target) * e.upper_over_t_density * t
                / (2.0 * e.upper.max(f64::MIN_POSITIVE))
        };
        let next = if proposed > lo && proposed < hi {
            proposed
        } else if hi.is_finite() {
            if lo > 0.0 { (lo * hi).sqrt() } else { hi / 2.0 }
        } else {
            2.0 * t
        };
        if (next - t).abs() <= 2.0 * f64::EPSILON * t {
            return next;
        }
        t = next;
    }
    t
}

/// The upper tail and related values at one point.
struct Evaluation {
    /// P(T > t).
    upper: f64,
    /// ln P(T > t), accurate even where P(T > t) underflows.
    ln_upper: f64,
    /// P(|T| < t) = 1 − 2 P(T > t).
    central: f64,
    /// P(T > t) / (t f(t)), with f the density.
    upper_over_t_density: f64,
}

/// P(T > t) for t > 0 through the incomplete beta function:
/// P(T > t) = `I_x(ν/2, 1/2)/2` with x = ν/(ν + t²), and
/// P(|T| < t) = `I_y(1/2, ν/2)` with y = t²/(ν + t²). The continued fraction
/// converges fast for x below (a + 1)/(a + b + 2), so the tail is computed
/// directly there and through the central probability otherwise.
///
/// Both prefactors x^a (1 − x)^b / (a B(a, b)) reduce to the density f(t):
/// 2 t f(t)/ν for `I_x(ν/2, 1/2)`, so P(T > t) = t f(t)/ν · fraction, and
/// 2 t f(t) for `I_y(1/2, ν/2)`, so P(|T| < t) = 2 t f(t) · fraction.
fn evaluate(t: f64, df: u32) -> Evaluation {
    let nu = f64::from(df);
    let t2 = t * t;
    let x = nu / (nu + t2);
    let y = t2 / (nu + t2);
    let ln_density = ln_density(t, df);
    if t2 * (nu + 2.0) >= 3.0 * nu {
        let fraction = beta_continued_fraction(nu / 2.0, 0.5, x, y);
        let ln_upper = ln_density + log(t / nu) + log(fraction);
        let upper = exp(ln_upper);
        Evaluation {
            upper,
            ln_upper,
            central: 1.0 - 2.0 * upper,
            upper_over_t_density: fraction / nu,
        }
    } else {
        let fraction = beta_continued_fraction(0.5, nu / 2.0, y, x);
        let t_density = t * exp(ln_density);
        let central = 2.0 * t_density * fraction;
        let upper = (1.0 - central) / 2.0;
        Evaluation {
            upper,
            ln_upper: log(upper),
            central,
            upper_over_t_density: upper / t_density,
        }
    }
}

/// ln f(t) = ln Γ((ν+1)/2) − ln Γ(ν/2) − ln √(νπ) − (ν+1)/2 · ln(1 + t²/ν).
fn ln_density(t: f64, df: u32) -> f64 {
    let nu = f64::from(df);
    log(gamma_half_ratio(df)) - 0.5 * log(nu * PI) - f64::midpoint(nu, 1.0) * log1p(t * t / nu)
}

/// Γ((ν+1)/2) / Γ(ν/2).
///
/// Up to ν = 400 by the recurrence r(ν + 2) = r(ν)(ν + 1)/ν from
/// r(1) = 1/√π and r(2) = √π/2; beyond, the asymptotic series
/// Γ(a + ½)/Γ(a) = √a (1 − 1/(8a) + 1/(128a²) + 5/(1024a³) − 21/(32768a⁴)
/// − 399/(262144a⁵) + 869/(4194304a⁶) + …) with a = ν/2, whose next term is
/// below 10⁻¹⁷ there.
fn gamma_half_ratio(df: u32) -> f64 {
    const SQRT_PI: f64 = 1.772_453_850_905_516;
    let nu = f64::from(df);
    if df <= 400 {
        let (mut ratio, mut k) = if df % 2 == 1 {
            (1.0 / SQRT_PI, 1.0)
        } else {
            (SQRT_PI / 2.0, 2.0)
        };
        while k < nu {
            ratio *= (k + 1.0) / k;
            k += 2.0;
        }
        ratio
    } else {
        let a = nu / 2.0;
        let x = 1.0 / a;
        let series = 1.0
            + x * (-1.0 / 8.0
                + x * (1.0 / 128.0
                    + x * (5.0 / 1024.0
                        + x * (-21.0 / 32768.0
                            + x * (-399.0 / 262_144.0 + x * (869.0 / 4_194_304.0))))));
        a.sqrt() * series
    }
}

/// Most terms of the continued fraction, a guard only: where it is used
/// (ν < 3000, or t far in the tail) it converges in a few dozen terms (at
/// most 50 in a prototype over ν ≤ 10⁷).
const MAX_FRACTION_TERMS: u32 = 10_000;

/// The continued fraction of `I_x(a, b)` (Numerical Recipes `betacf`, modified
/// Lentz), with `complement` = 1 − x given separately so its first term,
/// 1 − (a + b)x/(a + 1) = ((a + 1)(1 − x) + (1 − b)x)/(a + 1), has no
/// cancellation.
fn beta_continued_fraction(a: f64, b: f64, x: f64, complement: f64) -> f64 {
    const TINY: f64 = 1e-300;
    let guard = |v: f64| if v.abs() < TINY { TINY } else { v };
    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    // Lentz's C, D and the value of the fraction so far.
    let mut lentz_c = 1.0;
    let mut lentz_d = 1.0 / guard((qap * complement + (1.0 - b) * x) / qap);
    let mut value = lentz_d;
    for step in 1..=MAX_FRACTION_TERMS {
        let m = f64::from(step);
        let m2 = 2.0 * m;
        let even = m * (b - m) * x / ((qam + m2) * (a + m2));
        lentz_d = 1.0 / guard(1.0 + even * lentz_d);
        lentz_c = guard(1.0 + even / lentz_c);
        value *= lentz_d * lentz_c;
        let odd = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        lentz_d = 1.0 / guard(1.0 + odd * lentz_d);
        lentz_c = guard(1.0 + odd / lentz_c);
        let delta = lentz_d * lentz_c;
        value *= delta;
        if (delta - 1.0).abs() <= f64::EPSILON {
            break;
        }
    }
    value
}

/// The z > 0 with P(Z > z) = q for the standard normal, 0 < q ≤ 0.5:
/// Wichura (1988), Algorithm AS 241 (PPND16), about 16 digits. The
/// coefficients are those of the paper, lowest power first.
fn normal_upper_quantile(q: f64) -> f64 {
    const A: [f64; 8] = [
        3.387_132_872_796_366_5,
        133.141_667_891_784_38,
        1_971.590_950_306_551_3,
        13_731.693_765_509_46,
        45_921.953_931_549_87,
        67_265.770_927_008_7,
        33_430.575_583_588_13,
        2_509.080_928_730_122_7,
    ];
    const B: [f64; 8] = [
        1.0,
        42.313_330_701_600_91,
        687.187_007_492_057_9,
        5_394.196_021_424_751,
        21_213.794_301_586_597,
        39_307.895_800_092_71,
        28_729.085_735_721_943,
        5_226.495_278_852_854,
    ];
    const C: [f64; 8] = [
        1.423_437_110_749_683_5,
        4.630_337_846_156_546,
        5.769_497_221_460_691,
        3.647_848_324_763_204_5,
        1.270_458_252_452_368_4,
        0.241_780_725_177_450_6,
        0.022_723_844_989_269_184,
        0.000_774_545_014_278_341_4,
    ];
    const D: [f64; 8] = [
        1.0,
        2.053_191_626_637_759,
        1.676_384_830_183_803_8,
        0.689_767_334_985_1,
        0.148_103_976_427_480_08,
        0.015_198_666_563_616_457,
        0.000_547_593_808_499_534_5,
        1.050_750_071_644_416_9e-9,
    ];
    const E: [f64; 8] = [
        6.657_904_643_501_103,
        5.463_784_911_164_114,
        1.784_826_539_917_291_3,
        0.296_560_571_828_504_87,
        0.026_532_189_526_576_124,
        0.001_242_660_947_388_078_4,
        2.711_555_568_743_487_6e-5,
        2.010_334_399_292_288_1e-7,
    ];
    const F: [f64; 8] = [
        1.0,
        0.599_832_206_555_888,
        0.136_929_880_922_735_8,
        0.014_875_361_290_850_615,
        0.000_786_869_131_145_613_3,
        1.846_318_317_510_054_8e-5,
        1.421_511_758_316_446e-7,
        2.044_263_103_389_939_7e-15,
    ];
    let r = 0.5 - q;
    if r <= 0.425 {
        let s = 0.180_625 - r * r;
        return r * polynomial(&A, s) / polynomial(&B, s);
    }
    let s = (-log(q)).sqrt();
    if s <= 5.0 {
        polynomial(&C, s - 1.6) / polynomial(&D, s - 1.6)
    } else {
        polynomial(&E, s - 5.0) / polynomial(&F, s - 5.0)
    }
}

/// c₀ + c₁s + … + c₇s⁷ by Horner's rule.
fn polynomial(c: &[f64; 8], s: f64) -> f64 {
    c.iter().rev().fold(0.0, |acc, &ci| acc * s + ci)
}

/// P(Z > z) for the standard normal, for tests of the quantile.
#[cfg(test)]
fn normal_upper_tail(z: f64) -> f64 {
    0.5 * libm::erfc(z / std::f64::consts::SQRT_2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Upper-tail normal quantiles z with P(Z > z) = q, solved with mpmath
    /// 1.4.1 at 60 digits: `findroot(lambda z: log(ncdf(-z)) - log(q), …)`.
    const NORMAL: [(f64, f64); 21] = [
        (0.499_999_999_999, 2.506_572_823_701_860_7e-12),
        (0.4, 0.253_347_103_135_799_7),
        (0.3, 0.524_400_512_708_040_8),
        (0.25, 0.674_489_750_196_081_7),
        (0.1, 1.281_551_565_544_600_4),
        (0.075, 1.439_531_470_938_456),
        (0.074_9, 1.440_238_267_527_963_7),
        (0.05, 1.644_853_626_951_472_6),
        (0.025, 1.959_963_984_540_054_3),
        (0.01, 2.326_347_874_040_841),
        (0.005, 2.575_829_303_548_901),
        (0.001, 3.090_232_306_167_813_6),
        (2.5e-5, 4.055_626_981_122_401),
        (1e-06, 4.753_424_308_822_899),
        (1e-09, 5.997_807_015_007_686_5),
        (1e-12, 7.034_483_825_301_132),
        (1e-20, 9.262_340_089_798_407),
        (1e-50, 14.933_337_534_788_489),
        (1e-100, 21.273_453_560_965_326),
        (1e-200, 30.205_594_179_579_64),
        (1e-300, 37.047_096_299_361_2),
    ];

    fn relative_error(found: f64, expected: f64) -> f64 {
        ((found - expected) / expected).abs()
    }

    #[test]
    fn normal_quantile_matches_mpmath() {
        for (q, z) in NORMAL {
            let found = normal_upper_quantile(q);
            assert!(relative_error(found, z) < 1e-15, "q {q}: {found} vs {z}");
        }
    }

    #[test]
    fn normal_quantile_inverts_the_tail() {
        for q in [0.4, 0.1, 1e-3, 1e-8] {
            let back = normal_upper_tail(normal_upper_quantile(q));
            assert!(relative_error(back, q) < 1e-13, "q {q}: {back}");
        }
    }

    #[test]
    fn gamma_ratio_branches_agree_at_the_switch() {
        // Γ(201)/Γ(200.5) and Γ(200.5)/Γ(200) by the other branch, compared
        // with the closed form for consecutive ν: r(ν) r(ν + 1) = ν/2.
        for df in [399, 400, 401, 402] {
            let product = gamma_half_ratio(df) * gamma_half_ratio(df + 1);
            assert!(
                relative_error(product, f64::from(df) / 2.0) < 1e-14,
                "ν {df}"
            );
        }
    }

    #[test]
    fn evaluation_branches_agree_at_the_switch() {
        // The tail form and the central form meet at t² = 3ν/(ν + 2).
        for df in [3, 10, 100, 2999] {
            let nu = f64::from(df);
            let t = (3.0 * nu / (nu + 2.0)).sqrt();
            let below = evaluate(t * (1.0 - 1e-12), df);
            let at = evaluate(t, df);
            assert!(relative_error(below.upper, at.upper) < 1e-11, "ν {nu}");
            assert!(
                relative_error(below.ln_upper, at.ln_upper) < 1e-11,
                "ν {nu}"
            );
        }
    }

    #[test]
    fn hill_start_is_close() {
        // Hill's start is within 10⁻³ of the quantile for these cases.
        for (nu, q, expected) in [
            (3.0, 0.025, 3.182_446_305_283_708),
            (10.0, 0.4, 0.260_184_829_492_080_16),
        ] {
            let start = hill_start(nu, q, normal_upper_quantile(q));
            assert!(relative_error(start, expected) < 1e-3, "ν {nu}: {start}");
        }
    }

    #[test]
    fn newton_recovers_from_a_poor_start() {
        let expected = 3.182_446_305_283_708; // t_{3, 0.975}
        for start in [1e-3, 0.5, 100.0, 1e6] {
            let t = newton(3, 0.025, start);
            assert!(relative_error(t, expected) < 1e-14, "start {start}: {t}");
        }
        let expected = 0.260_184_829_492_080_16; // t_{10, 0.6}
        for start in [1e-6, 5.0, 1e8] {
            let t = newton(10, 0.4, start);
            assert!(relative_error(t, expected) < 1e-14, "start {start}: {t}");
        }
    }
}
