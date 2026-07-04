//! Special functions with a deterministic, cross-platform implementation.
//!
//! The Gauss error function `erf` is provided by the pure-Rust `libm` crate
//! rather than the platform C math library. The system `libm` is not
//! bit-identical across platforms, so binding it via `extern "C"` would break
//! cross-platform 0-ULP determinism. `libm::erf` is a deterministic port that
//! produces the same bits on every platform.

/// Gauss error function, deterministic across platforms via the `libm` crate.
#[inline]
pub fn erf(x: f64) -> f64 {
    libm::erf(x)
}

/// Complementary error function, deterministic across platforms via `libm`.
#[inline]
pub fn erfc(x: f64) -> f64 {
    libm::erfc(x)
}

/// Upper standard-normal tail probability `Q(x) = P(Z > x)`.
#[inline]
pub fn normal_q(x: f64) -> f64 {
    0.5 * erfc(x * core::f64::consts::FRAC_1_SQRT_2)
}

/// Inverse complementary error function for `y` in `(0, 2)`.
#[inline]
pub fn erfc_inv(y: f64) -> Option<f64> {
    if !(0.0..2.0).contains(&y) || !y.is_finite() {
        return None;
    }
    erfc_inv_raw(y)
}

/// Inverse upper standard-normal tail probability for `p` in `(0, 1)`.
#[inline]
pub fn normal_q_inv(p: f64) -> Option<f64> {
    if !(0.0..1.0).contains(&p) || !p.is_finite() {
        return None;
    }
    Some(core::f64::consts::SQRT_2 * erfc_inv(2.0 * p)?)
}

fn erfc_inv_raw(y: f64) -> Option<f64> {
    let p_tail = 0.5 * y;
    let p_cdf = 1.0 - p_tail;
    let mut x = inverse_normal_cdf_approx(p_cdf)?;
    for _ in 0..2 {
        let f = normal_q(x) - p_tail;
        let phi = normal_pdf(x);
        if phi == 0.0 || !phi.is_finite() {
            break;
        }
        let ratio = f / phi;
        let den = 1.0 - 0.5 * x * ratio;
        if den == 0.0 || !den.is_finite() {
            x += ratio;
        } else {
            x += ratio / den;
        }
    }
    if x.is_finite() {
        Some(x * core::f64::consts::FRAC_1_SQRT_2)
    } else {
        None
    }
}

fn normal_pdf(x: f64) -> f64 {
    const INV_SQRT_2PI: f64 = 0.398_942_280_401_432_7;
    INV_SQRT_2PI * (-0.5 * x * x).exp()
}

fn inverse_normal_cdf_approx(p: f64) -> Option<f64> {
    if !(0.0..1.0).contains(&p) || !p.is_finite() {
        return None;
    }

    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    const P_LOW: f64 = 0.024_25;
    const P_HIGH: f64 = 1.0 - P_LOW;

    if p < P_LOW {
        let q = (-2.0 * p.ln()).sqrt();
        Some(poly6(&C, q) / poly4p1(&D, q))
    } else if p <= P_HIGH {
        let q = p - 0.5;
        let r = q * q;
        Some(q * poly6(&A, r) / poly5p1(&B, r))
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        Some(-poly6(&C, q) / poly4p1(&D, q))
    }
}

fn poly6(c: &[f64; 6], x: f64) -> f64 {
    (((((c[0] * x + c[1]) * x + c[2]) * x + c[3]) * x + c[4]) * x) + c[5]
}

fn poly5p1(c: &[f64; 5], x: f64) -> f64 {
    (((((c[0] * x + c[1]) * x + c[2]) * x + c[3]) * x + c[4]) * x) + 1.0
}

fn poly4p1(c: &[f64; 4], x: f64) -> f64 {
    ((((c[0] * x + c[1]) * x + c[2]) * x + c[3]) * x) + 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erf_has_frozen_bits() {
        // Frozen-bits golden of the deterministic `libm::erf`. These bits are
        // identical on every platform; a tolerance check would have hidden the
        // 1-ULP drift the platform C `erf` introduced at erf(1.0).
        assert_eq!(erf(0.0).to_bits(), 0x0000_0000_0000_0000);
        assert_eq!(erf(0.75).to_bits(), 0x3fe6_c1c9_759d_0e60);
        assert_eq!(erf(1.0).to_bits(), 0x3fea_f767_a741_088b);
        assert_eq!(erf(1.5).to_bits(), 0x3fee_ea55_5713_7ae0);
        assert_eq!(erf(2.0).to_bits(), 0x3fef_d9ae_1427_95e3);
        assert_eq!(erf(6.0).to_bits(), 0x3ff0_0000_0000_0000);
        // erf is odd: erf(-x) == -erf(x), exactly.
        assert_eq!(erf(-0.75).to_bits(), (-erf(0.75)).to_bits());
        assert_eq!(erf(-6.0).to_bits(), (-1.0_f64).to_bits());
    }

    #[test]
    fn normal_q_inv_round_trips_tail_probabilities() {
        for p in [1.0e-12, 1.0e-9, 5.0e-8, 1.0e-4, 0.01, 0.5, 0.9] {
            let x = normal_q_inv(p).expect("valid tail probability");
            let got = normal_q(x);
            let tol = (p * 2.0e-12).max(1.0e-15);
            assert!(
                (got - p).abs() <= tol,
                "p={p} x={x} got={got} diff={}",
                (got - p).abs()
            );
        }
    }

    #[test]
    fn normal_q_inv_has_frozen_bits() {
        assert_eq!(normal_q_inv(0.5).unwrap().to_bits(), 0x0000_0000_0000_0000);
        assert_eq!(normal_q_inv(0.01).unwrap().to_bits(), 0x4002_9c5c_4630_ff0f);
        assert_eq!(
            normal_q_inv(5.0e-8).unwrap().to_bits(),
            0x4015_4e90_b4db_5fad
        );
    }
}
