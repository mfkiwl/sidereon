//! Cannonball solar radiation pressure.
//!
//! The acceleration follows the Montenbruck & Gill cannonball SRP model,
//! `-P_sr * (AU / |r_sun|)^2 * C_R * (A/m) * nu * s_hat`, where `s_hat`
//! points from the satellite to the Sun. SI pressure and area-to-mass units are
//! used internally, then converted from m/s^2 to km/s^2 for the propagation
//! state.

use crate::astro::bodies::sun_moon::sun_moon_eci;
use crate::astro::constants::astro::{AU_KM, SOLAR_RADIATION_PRESSURE_N_M2};
use crate::astro::constants::time::{DAYS_PER_JULIAN_CENTURY, SECONDS_PER_DAY};
use crate::astro::constants::units::M_PER_KM;
use crate::astro::error::PropagationError;
use crate::astro::events::eclipse::shadow_fraction;
use crate::astro::forces::r#trait::ForceModel;
use crate::astro::propagator::api::PropagationContext;
use crate::astro::state::CartesianState;
use nalgebra::Vector3;

/// Cannonball solar radiation pressure parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolarRadiationPressure {
    cr: f64,
    area_to_mass_m2_kg: f64,
    pressure_n_m2: f64,
    au_km: f64,
}

impl SolarRadiationPressure {
    /// Build from reflectivity coefficient `C_R` and area-to-mass `A/m` in m^2/kg.
    pub fn new(cr: f64, area_to_mass_m2_kg: f64) -> Result<Self, PropagationError> {
        Self::with_pressure(cr, area_to_mass_m2_kg, SOLAR_RADIATION_PRESSURE_N_M2, AU_KM)
    }

    /// Build with explicit pressure and AU constants.
    pub fn with_pressure(
        cr: f64,
        area_to_mass_m2_kg: f64,
        pressure_n_m2: f64,
        au_km: f64,
    ) -> Result<Self, PropagationError> {
        validate_positive("cr", cr)?;
        validate_positive("area_to_mass_m2_kg", area_to_mass_m2_kg)?;
        validate_positive("pressure_n_m2", pressure_n_m2)?;
        validate_positive("au_km", au_km)?;
        Ok(Self {
            cr,
            area_to_mass_m2_kg,
            pressure_n_m2,
            au_km,
        })
    }

    /// Reflectivity coefficient `C_R`.
    pub fn cr(&self) -> f64 {
        self.cr
    }

    /// Area-to-mass ratio `A/m`, m^2/kg.
    pub fn area_to_mass_m2_kg(&self) -> f64 {
        self.area_to_mass_m2_kg
    }

    /// Solar radiation pressure at 1 AU, N/m^2.
    pub fn pressure_n_m2(&self) -> f64 {
        self.pressure_n_m2
    }

    /// Astronomical unit used in the inverse-square scale, km.
    pub fn au_km(&self) -> f64 {
        self.au_km
    }
}

impl ForceModel for SolarRadiationPressure {
    fn acceleration(
        &self,
        state: &CartesianState,
        _ctx: &PropagationContext,
    ) -> Result<Vector3<f64>, PropagationError> {
        let sun_pos_km = sun_position_km(state.epoch_tdb_seconds)?;
        srp_acceleration_with_sun(state.position_km, sun_pos_km, *self)
    }
}

fn sun_position_km(epoch_tdb_seconds: f64) -> Result<Vector3<f64>, PropagationError> {
    let t_tt_centuries = epoch_tdb_seconds / (DAYS_PER_JULIAN_CENTURY * SECONDS_PER_DAY);
    let bodies = sun_moon_eci(t_tt_centuries)
        .map_err(|error| PropagationError::ForceModelFailure(format!("Sun/Moon: {error}")))?;
    Ok(Vector3::new(
        bodies.sun[0] / M_PER_KM,
        bodies.sun[1] / M_PER_KM,
        bodies.sun[2] / M_PER_KM,
    ))
}

fn srp_acceleration_with_sun(
    sat_pos_km: Vector3<f64>,
    sun_pos_km: Vector3<f64>,
    model: SolarRadiationPressure,
) -> Result<Vector3<f64>, PropagationError> {
    let sat_to_sun = sun_pos_km - sat_pos_km;
    let sat_to_sun_r2 = sat_to_sun.norm_squared();
    if sat_to_sun_r2 == 0.0 {
        return Err(PropagationError::NumericalFailure(
            "Satellite coincident with Sun".to_string(),
        ));
    }
    let sun_r2 = sun_pos_km.norm_squared();
    if sun_r2 == 0.0 {
        return Err(PropagationError::NumericalFailure(
            "Zero Sun position magnitude".to_string(),
        ));
    }

    let shadow = shadow_fraction(
        [sat_pos_km.x, sat_pos_km.y, sat_pos_km.z],
        [sun_pos_km.x, sun_pos_km.y, sun_pos_km.z],
    )
    .map_err(|error| PropagationError::ForceModelFailure(format!("eclipse: {error}")))?;
    let illumination = 1.0 - shadow;
    if illumination == 0.0 {
        return Ok(Vector3::zeros());
    }

    let s_hat = sat_to_sun / sat_to_sun_r2.sqrt();
    let pressure_scale = model.pressure_n_m2 * (model.au_km * model.au_km / sun_r2);
    let accel_m_s2 = -pressure_scale * model.cr * model.area_to_mass_m2_kg * illumination * s_hat;
    Ok(accel_m_s2 / M_PER_KM)
}

fn validate_positive(field: &'static str, value: f64) -> Result<(), PropagationError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(PropagationError::InvalidInput(format!(
            "{field} must be finite and positive"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Clean-room validation anchors:
    //!
    //! The expected 1 AU acceleration magnitude is the published cannonball SRP
    //! pressure `4.56e-6 N/m^2` multiplied by `C_R * A/m` and converted from
    //! m/s^2 to km/s^2. The shadow tests use fixed conical geometry and the
    //! existing eclipse helper, whose return value is geometric shadow fraction.

    use super::*;
    use crate::astro::constants::astro::AU_KM;

    #[test]
    fn one_au_magnitude_matches_pressure_area_mass_scale() {
        let model = SolarRadiationPressure::new(1.2, 0.02).expect("valid SRP");
        let sat = Vector3::new(7000.0, 0.0, 0.0);
        let sun = Vector3::new(AU_KM, 0.0, 0.0);
        let accel = srp_acceleration_with_sun(sat, sun, model).expect("SRP acceleration");

        let expected_km_s2 = 4.56e-6 * 1.2 * 0.02 / M_PER_KM;
        assert!((accel.norm() - expected_km_s2).abs() < 1.0e-24);
    }

    #[test]
    fn acceleration_is_zero_in_deep_umbra() {
        let model = SolarRadiationPressure::new(1.2, 0.02).expect("valid SRP");
        let sat = Vector3::new(-7000.0, 0.0, 0.0);
        let sun = Vector3::new(AU_KM, 0.0, 0.0);
        let accel = srp_acceleration_with_sun(sat, sun, model).expect("SRP acceleration");

        assert_eq!(accel, Vector3::zeros());
    }

    #[test]
    fn direction_is_away_from_sun() {
        let model = SolarRadiationPressure::new(1.0, 0.01).expect("valid SRP");
        let sat = Vector3::new(7000.0, 0.0, 0.0);
        let sun = Vector3::new(AU_KM, 0.0, 0.0);
        let accel = srp_acceleration_with_sun(sat, sun, model).expect("SRP acceleration");

        assert!(accel.x < 0.0);
        assert_eq!(accel.y, 0.0);
        assert_eq!(accel.z, 0.0);
    }
}
