//! Cacheable Earth-orientation evaluation for the precise GCRF/ITRF chain.
//!
//! Provenance: IERS Conventions (2010) TN36 and IAU 2006/2000A are consumed
//! through the existing precession, nutation, sidereal-time, and polar-motion
//! routines in [`crate::astro::frames::transforms`]. This module does not
//! reimplement any series; it evaluates the established chain once per epoch and
//! exposes reusable direction-cosine matrices and state transforms.

use crate::astro::constants::earth::OMEGA_E_DOT_RAD_S;
use crate::astro::frames::transforms::{
    gcrs_to_itrs_matrix_with_polar_motion, mat3_vec3_mul, polar_motion_matrix, FrameTransformError,
    PolarMotion,
};
use crate::astro::math::mat3::{inline_rxr, inline_tr, Mat3};
use crate::astro::time::civil::{civil_from_j2000_seconds, j2000_seconds_from_split};
use crate::astro::time::model::{Instant, InstantRepr, TimeScale};
use crate::astro::time::scales::TimeScales;

/// A single evaluated Earth-orientation state for one epoch.
///
/// The stored direction-cosine matrix maps GCRF/GCRS inertial coordinates to
/// ITRF/ITRS Earth-fixed coordinates using the existing IAU 2006/2000A
/// precession-nutation chain, apparent sidereal rotation, and optional polar
/// motion. The inverse matrix is cached as the transpose so callers can evaluate
/// the frame once and reuse it across many satellite states.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EarthOrientation {
    time_scales: TimeScales,
    polar_motion: PolarMotion,
    gcrf_to_itrf: Mat3,
    itrf_to_gcrf: Mat3,
    earth_rotation_vector_itrf_rad_s: [f64; 3],
}

impl EarthOrientation {
    /// Evaluate the full GCRF to ITRF rotation with zero polar motion.
    pub fn from_time_scales(ts: &TimeScales) -> Result<Self, FrameTransformError> {
        Self::from_time_scales_with_polar_motion(ts, PolarMotion::ZERO)
    }

    /// Evaluate the full GCRF to ITRF rotation with caller-supplied polar
    /// motion.
    pub fn from_time_scales_with_polar_motion(
        ts: &TimeScales,
        polar_motion: PolarMotion,
    ) -> Result<Self, FrameTransformError> {
        let gcrf_to_itrf = gcrs_to_itrs_matrix_with_polar_motion(ts, polar_motion)?;
        let itrf_to_gcrf = inline_tr(&gcrf_to_itrf);
        let polar = polar_motion_matrix(polar_motion)?;
        let earth_rotation_vector_itrf_rad_s =
            mat3_vec3_mul(&polar, &[0.0, 0.0, OMEGA_E_DOT_RAD_S])?;
        Ok(Self {
            time_scales: *ts,
            polar_motion,
            gcrf_to_itrf,
            itrf_to_gcrf,
            earth_rotation_vector_itrf_rad_s,
        })
    }

    /// Evaluate the full GCRF to ITRF rotation from UTC calendar fields with
    /// zero polar motion.
    pub fn from_utc(
        year: i32,
        month: i32,
        day: i32,
        hour: i32,
        minute: i32,
        second: f64,
    ) -> Result<Self, FrameTransformError> {
        Self::from_utc_with_polar_motion(year, month, day, hour, minute, second, PolarMotion::ZERO)
    }

    /// Evaluate the full GCRF to ITRF rotation from UTC calendar fields with
    /// caller-supplied polar motion.
    pub fn from_utc_with_polar_motion(
        year: i32,
        month: i32,
        day: i32,
        hour: i32,
        minute: i32,
        second: f64,
        polar_motion: PolarMotion,
    ) -> Result<Self, FrameTransformError> {
        let ts = TimeScales::from_utc(year, month, day, hour, minute, second)
            .map_err(|_| invalid_input("utc", "time-scale conversion failed"))?;
        Self::from_time_scales_with_polar_motion(&ts, polar_motion)
    }

    /// Evaluate the full GCRF to ITRF rotation from a scale-tagged instant with
    /// zero polar motion.
    pub fn from_instant(epoch: Instant) -> Result<Self, FrameTransformError> {
        Self::from_instant_with_polar_motion(epoch, PolarMotion::ZERO)
    }

    /// Evaluate the full GCRF to ITRF rotation from a scale-tagged instant with
    /// caller-supplied polar motion.
    pub fn from_instant_with_polar_motion(
        epoch: Instant,
        polar_motion: PolarMotion,
    ) -> Result<Self, FrameTransformError> {
        let ts = time_scales_from_instant(epoch)?;
        Self::from_time_scales_with_polar_motion(&ts, polar_motion)
    }

    /// Time scales used to evaluate this orientation.
    pub fn time_scales(&self) -> TimeScales {
        self.time_scales
    }

    /// Polar-motion coordinates used to evaluate this orientation.
    pub fn polar_motion(&self) -> PolarMotion {
        self.polar_motion
    }

    /// Earth rotation vector in ITRF axes, radians per second.
    pub fn earth_rotation_vector_itrf_rad_s(&self) -> [f64; 3] {
        self.earth_rotation_vector_itrf_rad_s
    }

    /// GCRF to ITRF direction-cosine matrix.
    pub fn gcrf_to_itrf_matrix(&self) -> Mat3 {
        self.gcrf_to_itrf
    }

    /// ITRF to GCRF direction-cosine matrix.
    pub fn itrf_to_gcrf_matrix(&self) -> Mat3 {
        self.itrf_to_gcrf
    }

    /// Time derivative of the GCRF to ITRF matrix from Earth rotation, with
    /// precession, nutation, and polar motion frozen at this evaluation point.
    pub fn gcrf_to_itrf_rotation_rate_matrix(&self) -> Mat3 {
        let neg_skew = neg_skew_matrix(self.earth_rotation_vector_itrf_rad_s);
        inline_rxr(&neg_skew, &self.gcrf_to_itrf)
    }

    /// Time derivative of the ITRF to GCRF matrix from Earth rotation, with
    /// precession, nutation, and polar motion frozen at this evaluation point.
    pub fn itrf_to_gcrf_rotation_rate_matrix(&self) -> Mat3 {
        let skew = skew_matrix(self.earth_rotation_vector_itrf_rad_s);
        inline_rxr(&self.itrf_to_gcrf, &skew)
    }

    /// Rotate a GCRF position vector in kilometers into ITRF.
    pub fn gcrf_to_itrf_position_km(
        &self,
        position_gcrf_km: [f64; 3],
    ) -> Result<[f64; 3], FrameTransformError> {
        validate_vec3("position_gcrf_km", &position_gcrf_km)?;
        mat3_vec3_mul(&self.gcrf_to_itrf, &position_gcrf_km)
    }

    /// Rotate an ITRF position vector in kilometers into GCRF.
    pub fn itrf_to_gcrf_position_km(
        &self,
        position_itrf_km: [f64; 3],
    ) -> Result<[f64; 3], FrameTransformError> {
        validate_vec3("position_itrf_km", &position_itrf_km)?;
        mat3_vec3_mul(&self.itrf_to_gcrf, &position_itrf_km)
    }

    /// Transform a GCRF state in kilometers and kilometers per second into ITRF.
    ///
    /// The velocity includes the rotating-frame term
    /// `v_itrf = R v_gcrf - omega_itrf x r_itrf`.
    pub fn gcrf_to_itrf_state_km(
        &self,
        position_gcrf_km: [f64; 3],
        velocity_gcrf_km_s: [f64; 3],
    ) -> Result<([f64; 3], [f64; 3]), FrameTransformError> {
        validate_vec3("position_gcrf_km", &position_gcrf_km)?;
        validate_vec3("velocity_gcrf_km_s", &velocity_gcrf_km_s)?;
        let position_itrf_km = mat3_vec3_mul(&self.gcrf_to_itrf, &position_gcrf_km)?;
        let rotated_velocity = mat3_vec3_mul(&self.gcrf_to_itrf, &velocity_gcrf_km_s)?;
        let transport = cross(self.earth_rotation_vector_itrf_rad_s, position_itrf_km);
        Ok((position_itrf_km, sub(rotated_velocity, transport)))
    }

    /// Transform an ITRF state in kilometers and kilometers per second into GCRF.
    ///
    /// The velocity includes the inertial transport term
    /// `v_gcrf = R^T (v_itrf + omega_itrf x r_itrf)`.
    pub fn itrf_to_gcrf_state_km(
        &self,
        position_itrf_km: [f64; 3],
        velocity_itrf_km_s: [f64; 3],
    ) -> Result<([f64; 3], [f64; 3]), FrameTransformError> {
        validate_vec3("position_itrf_km", &position_itrf_km)?;
        validate_vec3("velocity_itrf_km_s", &velocity_itrf_km_s)?;
        let position_gcrf_km = mat3_vec3_mul(&self.itrf_to_gcrf, &position_itrf_km)?;
        let transport = cross(self.earth_rotation_vector_itrf_rad_s, position_itrf_km);
        let velocity_rotating = add(velocity_itrf_km_s, transport);
        let velocity_gcrf_km_s = mat3_vec3_mul(&self.itrf_to_gcrf, &velocity_rotating)?;
        Ok((position_gcrf_km, velocity_gcrf_km_s))
    }
}

/// Supplies Earth orientation at a propagator integration epoch.
///
/// Force models that need body-fixed coordinates can request this provider from
/// [`crate::astro::propagator::PropagationContext`]. The default propagation
/// context carries no provider, so existing forces do not change behavior.
pub trait EarthOrientationProvider: Send + Sync {
    /// Evaluate Earth orientation at absolute TDB seconds since J2000.
    fn orientation_at_tdb_seconds(
        &self,
        epoch_tdb_seconds: f64,
    ) -> Result<EarthOrientation, FrameTransformError>;
}

/// Earth-orientation provider for propagator epochs expressed as TDB seconds
/// since J2000.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TdbEarthOrientationProvider {
    polar_motion: PolarMotion,
}

impl TdbEarthOrientationProvider {
    /// Build a provider with zero polar motion.
    pub const fn new() -> Self {
        Self {
            polar_motion: PolarMotion::ZERO,
        }
    }

    /// Build a provider with fixed polar motion applied at every epoch.
    pub const fn with_polar_motion(polar_motion: PolarMotion) -> Self {
        Self { polar_motion }
    }

    /// Polar-motion coordinates used by this provider.
    pub fn polar_motion(&self) -> PolarMotion {
        self.polar_motion
    }
}

impl Default for TdbEarthOrientationProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl EarthOrientationProvider for TdbEarthOrientationProvider {
    fn orientation_at_tdb_seconds(
        &self,
        epoch_tdb_seconds: f64,
    ) -> Result<EarthOrientation, FrameTransformError> {
        let ts = time_scales_from_scale_j2000_seconds(TimeScale::Tdb, epoch_tdb_seconds)?;
        EarthOrientation::from_time_scales_with_polar_motion(&ts, self.polar_motion)
    }
}

fn time_scales_from_instant(epoch: Instant) -> Result<TimeScales, FrameTransformError> {
    let seconds = match epoch.repr {
        InstantRepr::JulianDate(jd) => j2000_seconds_from_split(jd.jd_whole, jd.fraction),
        InstantRepr::Nanos(_) => {
            return Err(invalid_input("epoch", "must be a split Julian date"));
        }
    };
    time_scales_from_scale_j2000_seconds(epoch.scale, seconds)
}

fn time_scales_from_scale_j2000_seconds(
    scale: TimeScale,
    epoch_j2000_s: f64,
) -> Result<TimeScales, FrameTransformError> {
    if !epoch_j2000_s.is_finite() {
        return Err(invalid_input("epoch_j2000_s", "must be finite"));
    }
    let whole = epoch_j2000_s.floor();
    if whole < i64::MIN as f64 || whole > i64::MAX as f64 {
        return Err(invalid_input(
            "epoch_j2000_s",
            "whole seconds are out of range",
        ));
    }
    let fraction = epoch_j2000_s - whole;
    let (year, month, day, hour, minute, second) = civil_from_j2000_seconds(whole as i64);
    TimeScales::from_scale(
        scale,
        year as i32,
        month as i32,
        day as i32,
        hour as i32,
        minute as i32,
        second as f64 + fraction,
    )
    .map_err(|_| invalid_input("epoch_j2000_s", "time-scale conversion failed"))
}

fn invalid_input(field: &'static str, reason: &'static str) -> FrameTransformError {
    FrameTransformError::InvalidInput { field, reason }
}

fn validate_vec3(field: &'static str, value: &[f64; 3]) -> Result<(), FrameTransformError> {
    if value.iter().all(|component| component.is_finite()) {
        Ok(())
    } else {
        Err(invalid_input(field, "components must be finite"))
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn skew_matrix(omega: [f64; 3]) -> Mat3 {
    [
        [0.0, -omega[2], omega[1]],
        [omega[2], 0.0, -omega[0]],
        [-omega[1], omega[0], 0.0],
    ]
}

fn neg_skew_matrix(omega: [f64; 3]) -> Mat3 {
    [
        [0.0, omega[2], -omega[1]],
        [-omega[2], 0.0, omega[0]],
        [omega[1], -omega[0], 0.0],
    ]
}
