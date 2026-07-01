pub(crate) const SMALL: f64 = 1.0e-8;
pub(crate) const TWO_PI: f64 = 2.0 * std::f64::consts::PI;

/// Normalize an angle to the documented `[0, 2*pi)` range.
#[inline]
pub(crate) fn normalize_angle(x: f64) -> f64 {
    x.rem_euclid(TWO_PI)
}

/// Fold an angle to `(-pi, pi]`.
#[inline]
pub(crate) fn wrap_to_pi(x: f64) -> f64 {
    let wrapped = (x + std::f64::consts::PI).rem_euclid(TWO_PI) - std::f64::consts::PI;
    if wrapped <= -std::f64::consts::PI {
        std::f64::consts::PI
    } else {
        wrapped
    }
}
