//! GNSS/INS fusion primitives.
//!
//! The module keeps the already-merged inertial mechanization surface available
//! while adding the indirect error-state prediction and EKF correction core.

pub mod ekf;
pub mod error_state;
pub mod loose;
pub mod state;

pub use crate::inertial::*;
pub use ekf::{
    apply_closed_loop_error, ekf_correct_closed_loop, joseph_covariance_update, EkfCorrection,
    EkfCorrectionReport, EkfUpdateOptions, InnovationGate, InnovationGateReport,
};
pub use error_state::{
    error_state_process_noise_discrete, error_state_system_matrix_ecef,
    error_state_transition_matrix, linearize_error_state_ecef, predict_error_state_covariance,
    ErrorStateImuKinematics, ErrorStateLinearization,
};
pub use loose::{
    loose_coupling_correction, FusionUpdate, GnssFixMeasurement, InertialFilter,
    InertialFilterConfig, LooseCouplingConfig,
};
pub use state::{
    covariance_is_positive_semidefinite, reproject_covariance_psd, validate_covariance_matrix,
    ErrorStateLayout, ErrorStateVector, FusionError, FusionFilterKind, InsFilterState,
    ERROR_ACCEL_BIAS_INDEX, ERROR_ACCEL_SCALE_INDEX, ERROR_ATTITUDE_INDEX, ERROR_GYRO_BIAS_INDEX,
    ERROR_GYRO_SCALE_INDEX, ERROR_MOUNTING_MISALIGNMENT_INDEX,
    ERROR_MOUNTING_MISALIGNMENT_STATE_COUNT, ERROR_POSITION_INDEX, ERROR_STATE_DIMENSION_15,
    ERROR_STATE_DIMENSION_21, ERROR_VELOCITY_INDEX,
};
