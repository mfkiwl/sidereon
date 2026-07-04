//! Geocentric Schwarzschild relativistic correction.
//!
//! The acceleration is the single central-body term
//! `GM/(c^2 r^3) * [ (4 GM/r - v^2) r + 4 (r . v) v ]`, as given by
//! Montenbruck & Gill, "Satellite Orbits", equation 3.146. Position is in km,
//! velocity is in km/s, and the returned acceleration is km/s^2.

use crate::astro::constants::physics::SPEED_OF_LIGHT_KM_S;
use crate::astro::constants::MU_EARTH;
use crate::astro::error::PropagationError;
use crate::astro::forces::r#trait::ForceModel;
use crate::astro::propagator::api::PropagationContext;
use crate::astro::state::CartesianState;
use nalgebra::Vector3;

/// Schwarzschild correction for a single central body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SchwarzschildRelativity {
    /// Central-body gravitational parameter, km^3/s^2.
    pub mu_km3_s2: f64,
    /// Speed of light, km/s.
    pub c_km_s: f64,
}

impl Default for SchwarzschildRelativity {
    fn default() -> Self {
        Self {
            mu_km3_s2: MU_EARTH,
            c_km_s: SPEED_OF_LIGHT_KM_S,
        }
    }
}

impl SchwarzschildRelativity {
    /// Build a Schwarzschild correction with explicit constants.
    pub fn new(mu_km3_s2: f64, c_km_s: f64) -> Self {
        Self { mu_km3_s2, c_km_s }
    }
}

impl ForceModel for SchwarzschildRelativity {
    fn acceleration(
        &self,
        state: &CartesianState,
        _ctx: &PropagationContext,
    ) -> Result<Vector3<f64>, PropagationError> {
        if self.c_km_s.is_infinite() && self.c_km_s.is_sign_positive() {
            return Ok(Vector3::zeros());
        }
        if !self.c_km_s.is_finite() || self.c_km_s <= 0.0 {
            return Err(PropagationError::InvalidInput(
                "c_km_s must be finite and positive".to_string(),
            ));
        }

        let r2 = state.position_km.norm_squared();
        if r2 == 0.0 {
            return Err(PropagationError::NumericalFailure(
                "Zero position magnitude".to_string(),
            ));
        }
        let r = r2.sqrt();
        let r3 = r2 * r;
        let v2 = state.velocity_km_s.norm_squared();
        let r_dot_v = state.position_km.dot(&state.velocity_km_s);
        let factor = self.mu_km3_s2 / (self.c_km_s * self.c_km_s * r3);
        let bracket = state.position_km * (4.0 * self.mu_km3_s2 / r - v2)
            + state.velocity_km_s * (4.0 * r_dot_v);

        Ok(bracket * factor)
    }
}

#[cfg(test)]
mod tests {
    //! Clean-room validation anchors:
    //!
    //! For a circular GNSS-altitude orbit, `r . v = 0` and `v^2 = GM/r`, so
    //! Montenbruck & Gill equation 3.146 reduces to magnitude
    //! `3 * GM^2 / (c^2 * r^3)`. At `r = 26560 km`, this is
    //! `2.830552329290399e-10 m/s^2`.

    use super::*;

    #[test]
    fn gnss_circular_magnitude_matches_closed_form() {
        let r = 26_560.0;
        let v = (MU_EARTH / r).sqrt();
        let state = CartesianState::new(0.0, [r, 0.0, 0.0], [0.0, v, 0.0]);
        let accel = SchwarzschildRelativity::default()
            .acceleration(&state, &PropagationContext::default())
            .expect("relativistic acceleration");
        let expected = 2.830_552_329_290_399e-13;

        assert!((accel.x - expected).abs() < 1.0e-27);
        assert_eq!(accel.y, 0.0);
        assert_eq!(accel.z, 0.0);
        assert!((1.0e-10..1.0e-9).contains(&(accel.norm() * 1000.0)));
    }

    #[test]
    fn acceleration_vanishes_in_newtonian_limit() {
        let state = CartesianState::new(0.0, [26_560.0, 0.0, 0.0], [0.0, 3.874, 0.0]);
        let accel = SchwarzschildRelativity::new(MU_EARTH, f64::INFINITY)
            .acceleration(&state, &PropagationContext::default())
            .expect("relativistic acceleration");
        assert_eq!(accel, Vector3::zeros());
    }
}
