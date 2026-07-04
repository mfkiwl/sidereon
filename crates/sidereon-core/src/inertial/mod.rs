//! Inertial navigation primitives for ECEF strapdown propagation.
//!
//! This module contains the frame, mechanization, and IMU error-model pieces
//! needed before the GNSS update layers are introduced. It owns no I/O and has
//! no feature-gated behavior.

pub mod config;
pub mod frames;
pub mod imu;
pub mod mechanization;
pub mod sim;
pub mod state;

pub use config::{
    gauss_markov_bias_decay, gauss_markov_bias_variance_increment, ConingCorrection, ImuGrade,
    ImuSpec, MechanizationConfig,
};
pub use frames::{
    gravity_ecef_mps2, normal_gravity_mps2, WGS84_NORMAL_GRAVITY_EQUATOR_MPS2,
    WGS84_NORMAL_GRAVITY_POLE_MPS2, WGS84_SOMIGLIANA_K,
};
pub use imu::{
    CorrectedImuIncrement, ImuBias, ImuCalibration, ImuErrorModel, ImuSample, ImuSampleKind,
};
pub use mechanization::{mechanize_ecef, rodrigues_delta_dcm, StrapdownMechanizer};
pub use sim::{
    simulate_imu_samples, simulate_imu_samples_from_increments, true_imu_increment_between,
    ImuRateRandomWalk, ImuSimulationOptions, ImuSimulationOutput, ImuSimulator,
    SimulatedImuSequence, DEFAULT_IMU_SIM_SEED,
};
pub use state::{
    attitude_yaw_pitch_roll_rad, dcm_to_quaternion, quaternion_to_dcm, reorthonormalize_dcm,
    AttitudeQuaternion, NavState,
};

/// Error returned by inertial frame, IMU, and mechanization entry points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InertialError {
    /// A public input was non-finite or outside its documented domain.
    #[error("invalid inertial input {field}: {reason}")]
    InvalidInput {
        /// Name of the invalid field.
        field: &'static str,
        /// Short reason suitable for logs and tests.
        reason: &'static str,
    },
    /// A sample timestamp did not advance from the previous state timestamp.
    #[error("IMU sample time must be strictly increasing")]
    NonMonotonicSample,
    /// A scale or misalignment matrix could not be inverted.
    #[error("IMU calibration matrix is singular")]
    SingularCalibration,
    /// An attitude matrix could not be normalized into a right-handed frame.
    #[error("attitude matrix is degenerate")]
    DegenerateAttitude,
}

pub(crate) const fn invalid_input(field: &'static str, reason: &'static str) -> InertialError {
    InertialError::InvalidInput { field, reason }
}

pub(crate) fn validate_finite(value: f64, field: &'static str) -> Result<(), InertialError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(invalid_input(field, "must be finite"))
    }
}

pub(crate) fn validate_nonnegative(value: f64, field: &'static str) -> Result<(), InertialError> {
    validate_finite(value, field)?;
    if value >= 0.0 {
        Ok(())
    } else {
        Err(invalid_input(field, "must be non-negative"))
    }
}

pub(crate) fn validate_positive(value: f64, field: &'static str) -> Result<(), InertialError> {
    validate_finite(value, field)?;
    if value > 0.0 {
        Ok(())
    } else {
        Err(invalid_input(field, "must be positive"))
    }
}

pub(crate) fn validate_vec3(value: [f64; 3], field: &'static str) -> Result<(), InertialError> {
    for component in value {
        validate_finite(component, field)?;
    }
    Ok(())
}

pub(crate) fn validate_mat3(
    value: &[[f64; 3]; 3],
    field: &'static str,
) -> Result<(), InertialError> {
    for row in value {
        for component in row {
            validate_finite(*component, field)?;
        }
    }
    Ok(())
}
