//! Tightly coupled raw GNSS updates for the INS error-state filter.
//!
//! The update consumes one epoch of satellite pseudorange and optional
//! range-rate observations. It keeps the INS layout unchanged and carries the
//! receiver clock bias and drift in a private two-state augmentation.

use std::collections::BTreeSet;

use crate::astro::math::mat3::{inline_rxr, mul_vec3};
use crate::astro::math::vec3::{add3, cross3};
use crate::constants::C_M_S;
use crate::inertial::state::skew;
use crate::inertial::{validate_finite, validate_vec3};
use crate::observables::{
    transmit_time_satellite_state, ObservableEphemerisSource, ObservablesError, TransmitTimeOptions,
};
use crate::precise_positioning::{
    predict_range_rate_m_s, ReceiverVelocityState, VelocityObservation,
};

use super::ekf::{
    apply_closed_loop_navigation_error, apply_closed_loop_scale_error, innovation_covariance,
    joseph_covariance_update, normalized_innovation_squared, screen_correction, EkfCorrection,
    EkfCorrectionReport, EkfUpdateOptions,
};
use super::loose::{FusionUpdate, InertialFilter};
use super::state::{
    identity, invalid_input, matmul, matrix_add, reproject_covariance_psd, symmetrize_in_place,
    validate_covariance_matrix, validate_finite_slice, validate_nonnegative, validate_positive,
    validate_square_matrix, FusionError, InsFilterState, ERROR_ATTITUDE_INDEX,
    ERROR_GYRO_BIAS_INDEX, ERROR_POSITION_INDEX, ERROR_VELOCITY_INDEX,
};

/// Receiver-clock bias index in the tight augmented covariance.
pub const TIGHT_CLOCK_BIAS_OFFSET: usize = 0;
/// Receiver-clock drift index in the tight augmented covariance.
pub const TIGHT_CLOCK_DRIFT_OFFSET: usize = 1;
/// Number of receiver-clock states appended to the INS error state.
pub const TIGHT_CLOCK_STATE_COUNT: usize = 2;

/// Doppler-derived range-rate measurement for one satellite.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TightRangeRateObservation {
    /// Measured pseudorange rate in meters per second.
    pub measured_range_rate_m_s: f64,
    /// One-sigma range-rate uncertainty in meters per second.
    pub sigma_m_s: f64,
    /// Satellite clock drift as an equivalent range-rate bias in meters per second.
    pub satellite_clock_drift_m_s: f64,
}

impl TightRangeRateObservation {
    /// Validate finite range-rate fields and positive sigma.
    pub fn validate(&self) -> Result<(), FusionError> {
        validate_finite(self.measured_range_rate_m_s, "measured_range_rate_m_s")
            .map_err(FusionError::from)?;
        validate_positive(self.sigma_m_s, "range_rate_sigma_m_s")?;
        validate_finite(self.satellite_clock_drift_m_s, "satellite_clock_drift_m_s")
            .map_err(FusionError::from)
    }
}

/// Carrier-phase range row with a caller-supplied float ambiguity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TightCarrierPhaseObservation {
    /// Carrier phase converted to range units in meters.
    pub phase_range_m: f64,
    /// One-sigma carrier-phase range uncertainty in meters.
    pub sigma_m: f64,
    /// Current float ambiguity estimate for this continuous arc, in meters.
    pub float_ambiguity_m: f64,
}

impl TightCarrierPhaseObservation {
    /// Validate finite carrier fields and positive sigma.
    pub fn validate(&self) -> Result<(), FusionError> {
        validate_finite(self.phase_range_m, "phase_range_m").map_err(FusionError::from)?;
        validate_positive(self.sigma_m, "carrier_sigma_m")?;
        validate_finite(self.float_ambiguity_m, "float_ambiguity_m").map_err(FusionError::from)
    }
}

/// Raw GNSS observation for one satellite in a tight update.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TightGnssObservation {
    /// Satellite identifier.
    pub satellite_id: crate::GnssSatelliteId,
    /// Measured code pseudorange in meters.
    pub pseudorange_m: f64,
    /// One-sigma pseudorange uncertainty in meters.
    pub pseudorange_sigma_m: f64,
    /// Optional Doppler-derived range-rate row.
    pub range_rate: Option<TightRangeRateObservation>,
    /// Optional carrier-phase row using a supplied float ambiguity.
    pub carrier_phase: Option<TightCarrierPhaseObservation>,
    /// Ionospheric group delay correction for code, in meters.
    pub ionosphere_delay_m: f64,
    /// Tropospheric delay correction, in meters.
    pub troposphere_delay_m: f64,
}

impl TightGnssObservation {
    /// Build a pseudorange-only observation.
    pub fn pseudorange(
        satellite_id: crate::GnssSatelliteId,
        pseudorange_m: f64,
        pseudorange_sigma_m: f64,
    ) -> Result<Self, FusionError> {
        let observation = Self {
            satellite_id,
            pseudorange_m,
            pseudorange_sigma_m,
            range_rate: None,
            carrier_phase: None,
            ionosphere_delay_m: 0.0,
            troposphere_delay_m: 0.0,
        };
        observation.validate()?;
        Ok(observation)
    }

    /// Validate finite measurement values, positive sigmas, and optional rows.
    pub fn validate(&self) -> Result<(), FusionError> {
        validate_finite(self.pseudorange_m, "pseudorange_m").map_err(FusionError::from)?;
        validate_positive(self.pseudorange_sigma_m, "pseudorange_sigma_m")?;
        validate_finite(self.ionosphere_delay_m, "ionosphere_delay_m")
            .map_err(FusionError::from)?;
        validate_finite(self.troposphere_delay_m, "troposphere_delay_m")
            .map_err(FusionError::from)?;
        if let Some(range_rate) = self.range_rate {
            range_rate.validate()?;
        }
        if let Some(carrier_phase) = self.carrier_phase {
            carrier_phase.validate()?;
        }
        Ok(())
    }
}

/// One receiver epoch of raw GNSS observations for a tight update.
#[derive(Debug, Clone, PartialEq)]
pub struct TightGnssEpoch {
    /// Measurement epoch in seconds since J2000 on the caller's GNSS time scale.
    pub t_j2000_s: f64,
    /// One or more satellite observations.
    pub observations: Vec<TightGnssObservation>,
}

impl TightGnssEpoch {
    /// Build and validate an epoch from raw observations.
    pub fn new(
        t_j2000_s: f64,
        observations: Vec<TightGnssObservation>,
    ) -> Result<Self, FusionError> {
        let epoch = Self {
            t_j2000_s,
            observations,
        };
        epoch.validate()?;
        Ok(epoch)
    }

    /// Validate epoch time, row count, duplicate satellites, and observations.
    pub fn validate(&self) -> Result<(), FusionError> {
        validate_finite(self.t_j2000_s, "t_j2000_s").map_err(FusionError::from)?;
        if self.observations.is_empty() {
            return Err(invalid_input("tight_observations", "must not be empty"));
        }
        let mut seen = BTreeSet::new();
        for observation in &self.observations {
            observation.validate()?;
            if !seen.insert(observation.satellite_id) {
                return Err(invalid_input(
                    "tight_observations",
                    "satellites must be unique",
                ));
            }
        }
        Ok(())
    }
}

/// Configuration for tightly coupled raw GNSS updates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TightCouplingConfig {
    /// Body-frame vector from IMU origin to GNSS antenna phase center, in meters.
    pub lever_arm_body_m: [f64; 3],
    /// Apply fixed-count transmit-time light-time correction.
    pub light_time: bool,
    /// Apply Earth-rotation Sagnac correction.
    pub sagnac: bool,
    /// Initial receiver-clock bias variance in square meters.
    pub initial_clock_bias_variance_m2: f64,
    /// Initial receiver-clock drift variance in `(m/s)^2`.
    pub initial_clock_drift_variance_m2_s2: f64,
    /// Receiver-clock bias random-walk spectral density in `m^2/s`.
    pub clock_bias_random_walk_m2_s: f64,
    /// Receiver-clock drift random-walk spectral density in `m^2/s^3`.
    pub clock_drift_random_walk_m2_s3: f64,
    /// Generic EKF correction options applied to each tight update.
    pub update_options: EkfUpdateOptions,
}

impl Default for TightCouplingConfig {
    fn default() -> Self {
        Self {
            lever_arm_body_m: [0.0; 3],
            light_time: true,
            sagnac: true,
            initial_clock_bias_variance_m2: 1.0e12,
            initial_clock_drift_variance_m2_s2: 1.0e6,
            clock_bias_random_walk_m2_s: 1.0,
            clock_drift_random_walk_m2_s3: 1.0e-2,
            update_options: EkfUpdateOptions::default(),
        }
    }
}

impl TightCouplingConfig {
    /// Validate lever arm, clock covariance, clock process noise, and update options.
    pub fn validate(&self) -> Result<(), FusionError> {
        validate_vec3(self.lever_arm_body_m, "tight_lever_arm_body_m")
            .map_err(FusionError::from)?;
        validate_nonnegative(
            self.initial_clock_bias_variance_m2,
            "initial_clock_bias_variance_m2",
        )?;
        validate_nonnegative(
            self.initial_clock_drift_variance_m2_s2,
            "initial_clock_drift_variance_m2_s2",
        )?;
        validate_nonnegative(
            self.clock_bias_random_walk_m2_s,
            "clock_bias_random_walk_m2_s",
        )?;
        validate_nonnegative(
            self.clock_drift_random_walk_m2_s3,
            "clock_drift_random_walk_m2_s3",
        )?;
        if let Some(gate) = self.update_options.innovation_gate {
            gate.validate()?;
        }
        Ok(())
    }
}

/// Receiver-clock state reported by the tight filter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TightClockState {
    /// Receiver-clock range bias in meters.
    pub bias_m: f64,
    /// Receiver-clock drift in meters per second.
    pub drift_m_s: f64,
    /// Two-by-two clock covariance ordered as `[bias_m, drift_m_s]`.
    pub covariance: [[f64; TIGHT_CLOCK_STATE_COUNT]; TIGHT_CLOCK_STATE_COUNT],
}

/// Snapshot of the tight clock augmentation for replay and restore.
#[derive(Debug, Clone, PartialEq)]
pub struct TightFilterSnapshot {
    /// Receiver-clock range bias in meters.
    pub clock_bias_m: f64,
    /// Receiver-clock drift in meters per second.
    pub clock_drift_m_s: f64,
    /// Full augmented covariance ordered as `[INS error state, clock bias, clock drift]`.
    pub augmented_covariance: Vec<Vec<f64>>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct TightFusionState {
    clock_bias_m: f64,
    clock_drift_m_s: f64,
    augmented_covariance: Vec<Vec<f64>>,
}

impl TightFusionState {
    pub(super) fn from_filter_state(
        state: &InsFilterState,
        config: TightCouplingConfig,
    ) -> Result<Self, FusionError> {
        config.validate()?;
        let base_dim = state.dimension();
        let aug_dim = augmented_dimension(base_dim);
        let mut augmented_covariance = vec![vec![0.0; aug_dim]; aug_dim];
        for (row, base_row) in state.covariance.iter().enumerate().take(base_dim) {
            augmented_covariance[row][..base_dim].copy_from_slice(&base_row[..base_dim]);
        }
        let clock_bias_index = clock_bias_index(base_dim);
        let clock_drift_index = clock_drift_index(base_dim);
        augmented_covariance[clock_bias_index][clock_bias_index] =
            config.initial_clock_bias_variance_m2;
        augmented_covariance[clock_drift_index][clock_drift_index] =
            config.initial_clock_drift_variance_m2_s2;
        let tight = Self {
            clock_bias_m: 0.0,
            clock_drift_m_s: 0.0,
            augmented_covariance,
        };
        tight.validate(base_dim)?;
        Ok(tight)
    }

    pub(super) fn snapshot(&self) -> TightFilterSnapshot {
        TightFilterSnapshot {
            clock_bias_m: self.clock_bias_m,
            clock_drift_m_s: self.clock_drift_m_s,
            augmented_covariance: self.augmented_covariance.clone(),
        }
    }

    pub(super) fn restore(
        &mut self,
        snapshot: &TightFilterSnapshot,
        base_dim: usize,
    ) -> Result<(), FusionError> {
        validate_finite(snapshot.clock_bias_m, "clock_bias_m").map_err(FusionError::from)?;
        validate_finite(snapshot.clock_drift_m_s, "clock_drift_m_s").map_err(FusionError::from)?;
        validate_covariance_matrix(
            &snapshot.augmented_covariance,
            augmented_dimension(base_dim),
            "tight_augmented_covariance",
        )?;
        self.clock_bias_m = snapshot.clock_bias_m;
        self.clock_drift_m_s = snapshot.clock_drift_m_s;
        self.augmented_covariance = snapshot.augmented_covariance.clone();
        self.validate(base_dim)
    }

    pub(super) fn clock_state(&self, base_dim: usize) -> Result<TightClockState, FusionError> {
        self.validate(base_dim)?;
        let bias = clock_bias_index(base_dim);
        let drift = clock_drift_index(base_dim);
        Ok(TightClockState {
            bias_m: self.clock_bias_m,
            drift_m_s: self.clock_drift_m_s,
            covariance: [
                [
                    self.augmented_covariance[bias][bias],
                    self.augmented_covariance[bias][drift],
                ],
                [
                    self.augmented_covariance[drift][bias],
                    self.augmented_covariance[drift][drift],
                ],
            ],
        })
    }

    pub(super) fn validate(&self, base_dim: usize) -> Result<(), FusionError> {
        validate_finite(self.clock_bias_m, "clock_bias_m").map_err(FusionError::from)?;
        validate_finite(self.clock_drift_m_s, "clock_drift_m_s").map_err(FusionError::from)?;
        validate_covariance_matrix(
            &self.augmented_covariance,
            augmented_dimension(base_dim),
            "tight_augmented_covariance",
        )
    }

    pub(super) fn align_with_filter_state(
        &mut self,
        state: &InsFilterState,
    ) -> Result<(), FusionError> {
        state.validate()?;
        let base_dim = state.dimension();
        self.validate(base_dim)?;
        let mut differs = false;
        'outer: for row in 0..base_dim {
            for col in 0..base_dim {
                if self.augmented_covariance[row][col].to_bits()
                    != state.covariance[row][col].to_bits()
                {
                    differs = true;
                    break 'outer;
                }
            }
        }
        if differs {
            self.replace_base_covariance_and_clear_cross(&state.covariance)?;
        }
        Ok(())
    }

    pub(super) fn replace_base_covariance_and_clear_cross(
        &mut self,
        base_covariance: &[Vec<f64>],
    ) -> Result<(), FusionError> {
        let base_dim = base_covariance.len();
        validate_covariance_matrix(base_covariance, base_dim, "covariance")?;
        self.validate(base_dim)?;
        let aug_dim = augmented_dimension(base_dim);
        for (row, base_row) in base_covariance.iter().enumerate().take(base_dim) {
            self.augmented_covariance[row][..base_dim].copy_from_slice(&base_row[..base_dim]);
        }
        for idx in 0..base_dim {
            for clock in base_dim..aug_dim {
                self.augmented_covariance[idx][clock] = 0.0;
                self.augmented_covariance[clock][idx] = 0.0;
            }
        }
        self.validate(base_dim)
    }

    pub(super) fn predict_covariance(
        &mut self,
        phi_base: &[Vec<f64>],
        q_base: &[Vec<f64>],
        dt_s: f64,
        config: TightCouplingConfig,
    ) -> Result<(), FusionError> {
        config.validate()?;
        validate_nonnegative(dt_s, "dt_s")?;
        let base_dim = phi_base.len();
        validate_square_matrix(phi_base, base_dim, "phi")?;
        validate_covariance_matrix(q_base, base_dim, "q_d")?;
        self.validate(base_dim)?;

        let aug_dim = augmented_dimension(base_dim);
        let mut phi = identity(aug_dim);
        for row in 0..base_dim {
            for col in 0..base_dim {
                phi[row][col] = phi_base[row][col];
            }
        }
        let bias = clock_bias_index(base_dim);
        let drift = clock_drift_index(base_dim);
        phi[bias][drift] = dt_s;

        let mut q = vec![vec![0.0; aug_dim]; aug_dim];
        for row in 0..base_dim {
            for col in 0..base_dim {
                q[row][col] = q_base[row][col];
            }
        }
        let dt2 = dt_s * dt_s;
        let dt3 = dt2 * dt_s;
        q[bias][bias] += config.clock_bias_random_walk_m2_s * dt_s
            + config.clock_drift_random_walk_m2_s3 * dt3 / 3.0;
        q[bias][drift] += config.clock_drift_random_walk_m2_s3 * dt2 / 2.0;
        q[drift][bias] = q[bias][drift];
        q[drift][drift] += config.clock_drift_random_walk_m2_s3 * dt_s;
        reproject_covariance_psd(&mut q, "tight_process_noise")?;

        let left = matmul(&phi, &self.augmented_covariance)?;
        let phi_t = super::state::transpose(&phi)?;
        let propagated = matmul(&left, &phi_t)?;
        let mut next = matrix_add(&propagated, &q)?;
        symmetrize_in_place(&mut next);
        reproject_covariance_psd(&mut next, "tight_augmented_covariance")?;
        self.clock_bias_m += self.clock_drift_m_s * dt_s;
        self.augmented_covariance = next;
        self.validate(base_dim)
    }

    pub(super) fn copy_base_covariance_to_state(
        &self,
        state: &mut InsFilterState,
    ) -> Result<(), FusionError> {
        let base_dim = state.dimension();
        self.validate(base_dim)?;
        for row in 0..base_dim {
            for col in 0..base_dim {
                state.covariance[row][col] = self.augmented_covariance[row][col];
            }
        }
        state.validate()
    }
}

impl InertialFilter {
    /// Borrow the current receiver-clock state carried by tight coupling.
    pub fn tight_clock_state(&self) -> Result<TightClockState, FusionError> {
        self.tight.clock_state(self.state.dimension())
    }

    /// Apply a tight raw GNSS update at the current propagated epoch.
    ///
    /// GNSS epochs must be strictly increasing across the filter's stateful
    /// update surface. One satellite is a valid update.
    pub fn update_tight(
        &mut self,
        source: &dyn ObservableEphemerisSource,
        epoch: &TightGnssEpoch,
    ) -> Result<FusionUpdate, FusionError> {
        if let Some(last) = self.time_sync.last_measurement_t_j2000_s() {
            if epoch.t_j2000_s <= last {
                return Err(invalid_input(
                    "t_j2000_s",
                    "GNSS measurement epochs must be strictly increasing",
                ));
            }
        }
        let update = self.update_tight_core(source, epoch)?;
        let snapshot = self.snapshot();
        self.time_sync
            .push_tight_measurement_and_checkpoint(epoch.clone(), snapshot);
        Ok(update)
    }

    pub(super) fn update_tight_core(
        &mut self,
        source: &dyn ObservableEphemerisSource,
        epoch: &TightGnssEpoch,
    ) -> Result<FusionUpdate, FusionError> {
        self.tight.align_with_filter_state(&self.state)?;
        let correction = tight_coupling_correction(
            source,
            &self.state,
            &self.tight,
            epoch,
            self.config.tight,
            self.last_body_rate_wrt_ecef_rps,
        )?;
        let rows = correction.row_count();
        let report = apply_tight_correction(self, &correction, self.config.tight.update_options)?;
        Ok(FusionUpdate {
            applied: report.applied,
            nis: report.normalized_innovation_squared,
            rows,
            accepted_rows: report.accepted_rows,
            rejected_rows: report.rejected_rows,
            ekf: report,
        })
    }
}

pub(super) fn tight_coupling_correction(
    source: &dyn ObservableEphemerisSource,
    state: &InsFilterState,
    tight_state: &TightFusionState,
    epoch: &TightGnssEpoch,
    config: TightCouplingConfig,
    body_rate_wrt_ecef_rps: [f64; 3],
) -> Result<EkfCorrection, FusionError> {
    state.validate()?;
    tight_state.validate(state.dimension())?;
    epoch.validate()?;
    config.validate()?;
    validate_vec3(body_rate_wrt_ecef_rps, "body_rate_wrt_ecef_rps").map_err(FusionError::from)?;
    if epoch.t_j2000_s != state.nominal.t_j2000_s {
        return Err(invalid_input("t_j2000_s", "must equal nominal state epoch"));
    }

    let base_dim = state.dimension();
    let aug_dim = augmented_dimension(base_dim);
    let clock_bias = clock_bias_index(base_dim);
    let clock_drift = clock_drift_index(base_dim);
    let kinematics = antenna_kinematics(state, config.lever_arm_body_m, body_rate_wrt_ecef_rps);
    let options = TransmitTimeOptions {
        light_time: config.light_time,
        sagnac: config.sagnac,
    };

    let mut innovation = Vec::new();
    let mut design = Vec::new();
    let mut variances = Vec::new();

    for observation in &epoch.observations {
        let satellite = transmit_time_satellite_state(
            source,
            observation.satellite_id,
            kinematics.antenna_position_ecef_m,
            epoch.t_j2000_s,
            options,
        )
        .map_err(map_observables_error)?;
        let sat_clock_s = satellite
            .clock_s
            .ok_or_else(|| invalid_input("satellite_clock_s", "must be present"))?;

        let code_prediction_m = satellite.geometric_range_m + tight_state.clock_bias_m
            - C_M_S * sat_clock_s
            + observation.ionosphere_delay_m
            + observation.troposphere_delay_m;
        let mut row = pseudorange_design_row(
            aug_dim,
            clock_bias,
            satellite.los_unit,
            kinematics.lever_arm_ecef_m,
        );
        innovation.push(observation.pseudorange_m - code_prediction_m);
        design.push(row);
        variances.push(observation.pseudorange_sigma_m * observation.pseudorange_sigma_m);

        if let Some(carrier_phase) = observation.carrier_phase {
            let phase_prediction_m = satellite.geometric_range_m + tight_state.clock_bias_m
                - C_M_S * sat_clock_s
                - observation.ionosphere_delay_m
                + observation.troposphere_delay_m
                + carrier_phase.float_ambiguity_m;
            row = pseudorange_design_row(
                aug_dim,
                clock_bias,
                satellite.los_unit,
                kinematics.lever_arm_ecef_m,
            );
            innovation.push(carrier_phase.phase_range_m - phase_prediction_m);
            design.push(row);
            variances.push(carrier_phase.sigma_m * carrier_phase.sigma_m);
        }

        if let Some(range_rate) = observation.range_rate {
            let velocity_observation = VelocityObservation {
                sat: observation.satellite_id,
                satellite_position_m: satellite.position_ecef_m,
                satellite_velocity_m_s: satellite.velocity_m_s,
                measured_range_rate_m_s: range_rate.measured_range_rate_m_s,
                sigma_m_s: range_rate.sigma_m_s,
                satellite_clock_drift_m_s: range_rate.satellite_clock_drift_m_s,
            };
            let receiver = ReceiverVelocityState {
                position_m: kinematics.antenna_position_ecef_m,
                velocity_m_s: kinematics.antenna_velocity_ecef_mps,
                clock_drift_m_s: tight_state.clock_drift_m_s,
            };
            let prediction = predict_range_rate_m_s(&velocity_observation, receiver)
                .ok_or_else(|| invalid_input("range_rate", "line of sight must be nonzero"))?;
            let row = range_rate_design_row(
                aug_dim,
                clock_drift,
                prediction.los_unit,
                kinematics.lever_velocity_ecef_mps,
                kinematics.gyro_bias_velocity_block,
            );
            innovation.push(range_rate.measured_range_rate_m_s - prediction.range_rate_m_s);
            design.push(row);
            variances.push(range_rate.sigma_m_s * range_rate.sigma_m_s);
        }
    }

    validate_finite_slice(&innovation, "tight_innovation")?;
    let measurement_covariance = diagonal_covariance(&variances)?;
    EkfCorrection::new(innovation, design, measurement_covariance)
}

fn apply_tight_correction(
    filter: &mut InertialFilter,
    correction: &EkfCorrection,
    options: EkfUpdateOptions,
) -> Result<EkfCorrectionReport, FusionError> {
    filter.state.validate()?;
    let base_dim = filter.state.dimension();
    filter.tight.validate(base_dim)?;
    correction.validate_for_dimension(augmented_dimension(base_dim))?;

    if let Some(gate) = options.innovation_gate {
        gate.validate()?;
        let full_s = innovation_covariance(&filter.tight.augmented_covariance, correction)?;
        let (screened, gate_report) = screen_correction(correction, &full_s, gate)?;
        let full_nis = normalized_innovation_squared(&full_s, &correction.innovation)?;
        if gate_report.coasted {
            return Ok(EkfCorrectionReport {
                applied: false,
                normalized_innovation_squared: full_nis,
                accepted_rows: gate_report.accepted_rows,
                rejected_rows: gate_report.rejected_rows,
                innovation_gate: Some(gate_report),
                innovation_covariance: full_s,
                kalman_gain: vec![vec![0.0; correction.row_count()]; augmented_dimension(base_dim)],
                dx: vec![0.0; augmented_dimension(base_dim)],
            });
        }
        let accepted_rows = gate_report.accepted_rows;
        let rejected_rows = gate_report.rejected_rows;
        let mut report = apply_tight_correction_inner(filter, &screened)?;
        report.accepted_rows = accepted_rows;
        report.rejected_rows = rejected_rows;
        report.innovation_gate = Some(gate_report);
        return Ok(report);
    }

    apply_tight_correction_inner(filter, correction)
}

fn apply_tight_correction_inner(
    filter: &mut InertialFilter,
    correction: &EkfCorrection,
) -> Result<EkfCorrectionReport, FusionError> {
    let base_dim = filter.state.dimension();
    let aug_dim = augmented_dimension(base_dim);
    let s = innovation_covariance(&filter.tight.augmented_covariance, correction)?;
    let h_t = super::state::transpose(&correction.design)?;
    let p_h_t = matmul(&filter.tight.augmented_covariance, &h_t)?;
    let mut kalman_gain = vec![vec![0.0; correction.row_count()]; aug_dim];
    let mut scratch = crate::astro::math::linear::FlatCholeskySolveScratch::default();
    for row in 0..aug_dim {
        kalman_gain[row] = super::state::solve_spd(&s, &p_h_t[row], &mut scratch)?;
    }
    let dx = super::state::matvec(&kalman_gain, &correction.innovation)?;
    let nis = normalized_innovation_squared(&s, &correction.innovation)?;
    let covariance = joseph_covariance_update(
        &filter.tight.augmented_covariance,
        &correction.design,
        &kalman_gain,
        &correction.measurement_covariance,
    )?;

    apply_closed_loop_navigation_error(&mut filter.state.nominal, &dx[..base_dim])?;
    apply_closed_loop_scale_error(&mut filter.state, &dx[..base_dim]);
    filter.tight.clock_bias_m += dx[clock_bias_index(base_dim)];
    filter.tight.clock_drift_m_s += dx[clock_drift_index(base_dim)];
    filter.tight.augmented_covariance = covariance;
    filter
        .tight
        .copy_base_covariance_to_state(&mut filter.state)?;
    filter.state.reset_error_state();
    filter.state.validate()?;
    filter.tight.validate(base_dim)?;

    Ok(EkfCorrectionReport {
        applied: true,
        normalized_innovation_squared: nis,
        accepted_rows: correction.row_count(),
        rejected_rows: 0,
        innovation_gate: None,
        innovation_covariance: s,
        kalman_gain,
        dx,
    })
}

#[derive(Debug, Clone, Copy)]
struct AntennaKinematics {
    antenna_position_ecef_m: [f64; 3],
    antenna_velocity_ecef_mps: [f64; 3],
    lever_arm_ecef_m: [f64; 3],
    lever_velocity_ecef_mps: [f64; 3],
    gyro_bias_velocity_block: [[f64; 3]; 3],
}

fn antenna_kinematics(
    state: &InsFilterState,
    lever_arm_body_m: [f64; 3],
    body_rate_wrt_ecef_rps: [f64; 3],
) -> AntennaKinematics {
    let c_b_e = state.nominal.attitude_body_to_ecef;
    let lever_arm_ecef_m = mul_vec3(&c_b_e, lever_arm_body_m);
    let antenna_position_ecef_m = add3(state.nominal.position_ecef_m, lever_arm_ecef_m);
    let lever_velocity_body_mps = cross3(body_rate_wrt_ecef_rps, lever_arm_body_m);
    let lever_velocity_ecef_mps = mul_vec3(&c_b_e, lever_velocity_body_mps);
    let antenna_velocity_ecef_mps = add3(state.nominal.velocity_ecef_mps, lever_velocity_ecef_mps);
    let gyro_bias_velocity_block = inline_rxr(&c_b_e, &skew(lever_arm_body_m));
    AntennaKinematics {
        antenna_position_ecef_m,
        antenna_velocity_ecef_mps,
        lever_arm_ecef_m,
        lever_velocity_ecef_mps,
        gyro_bias_velocity_block,
    }
}

fn pseudorange_design_row(
    aug_dim: usize,
    clock_bias: usize,
    los_unit: [f64; 3],
    lever_arm_ecef_m: [f64; 3],
) -> Vec<f64> {
    let mut row = vec![0.0; aug_dim];
    for axis in 0..3 {
        row[ERROR_POSITION_INDEX + axis] = -los_unit[axis];
    }
    let lever_skew = skew(lever_arm_ecef_m);
    for col in 0..3 {
        row[ERROR_ATTITUDE_INDEX + col] = los_unit[0] * lever_skew[0][col]
            + los_unit[1] * lever_skew[1][col]
            + los_unit[2] * lever_skew[2][col];
    }
    row[clock_bias] = 1.0;
    row
}

fn range_rate_design_row(
    aug_dim: usize,
    clock_drift: usize,
    los_unit: [f64; 3],
    lever_velocity_ecef_mps: [f64; 3],
    gyro_bias_velocity_block: [[f64; 3]; 3],
) -> Vec<f64> {
    let mut row = vec![0.0; aug_dim];
    for axis in 0..3 {
        row[ERROR_VELOCITY_INDEX + axis] = -los_unit[axis];
    }
    let lever_velocity_skew = skew(lever_velocity_ecef_mps);
    for col in 0..3 {
        row[ERROR_ATTITUDE_INDEX + col] = los_unit[0] * lever_velocity_skew[0][col]
            + los_unit[1] * lever_velocity_skew[1][col]
            + los_unit[2] * lever_velocity_skew[2][col];
        row[ERROR_GYRO_BIAS_INDEX + col] = los_unit[0] * gyro_bias_velocity_block[0][col]
            + los_unit[1] * gyro_bias_velocity_block[1][col]
            + los_unit[2] * gyro_bias_velocity_block[2][col];
    }
    row[clock_drift] = 1.0;
    row
}

fn diagonal_covariance(variances: &[f64]) -> Result<Vec<Vec<f64>>, FusionError> {
    if variances.is_empty() {
        return Err(invalid_input("measurement_covariance", "must not be empty"));
    }
    let mut covariance = vec![vec![0.0; variances.len()]; variances.len()];
    for (idx, variance) in variances.iter().enumerate() {
        validate_positive(*variance, "measurement_variance")?;
        covariance[idx][idx] = *variance;
    }
    Ok(covariance)
}

fn map_observables_error(error: ObservablesError) -> FusionError {
    match error {
        ObservablesError::NoEphemeris => invalid_input("ephemeris", "no usable satellite state"),
        ObservablesError::InvalidInput { .. } => {
            invalid_input("observable_state", "must be finite and in range")
        }
        ObservablesError::Ephemeris(_) => invalid_input("ephemeris", "satellite state failed"),
    }
}

pub(super) const fn augmented_dimension(base_dim: usize) -> usize {
    base_dim + TIGHT_CLOCK_STATE_COUNT
}

pub(super) const fn clock_bias_index(base_dim: usize) -> usize {
    base_dim + TIGHT_CLOCK_BIAS_OFFSET
}

pub(super) const fn clock_drift_index(base_dim: usize) -> usize {
    base_dim + TIGHT_CLOCK_DRIFT_OFFSET
}

#[cfg(test)]
mod tests {
    //! Provenance: tight-coupled GNSS/INS rows follow Groves, Principles of
    //! GNSS, Inertial, and Multisensor Integrated Navigation Systems, 2nd ed.,
    //! Chapter 14.2. Pseudorange-only convergence is checked against the
    //! in-crate SPP solver as an independent snapshot oracle. Doppler rows are
    //! checked against the existing `predict_range_rate_m_s` primitive. The
    //! weak-geometry properties check the information-form identity
    //! `P_plus^-1 = P_minus^-1 + H' R^-1 H`.

    use super::*;
    use crate::astro::constants::earth::WGS84_A_M;
    use crate::fusion::state::{
        covariance_is_positive_semidefinite, ErrorStateLayout, ERROR_STATE_DIMENSION_15,
    };
    use crate::inertial::config::RANDOM_WALK_BIAS_TAU_S;
    use crate::inertial::state::mat3_identity;
    use crate::inertial::{ImuSample, ImuSpec, NavState};
    use crate::observables::{ObservableState, ObservablesError};
    use crate::spp::{
        Corrections, KlobucharCoeffs, Observation, SolveInputs, SppError, SurfaceMet,
    };
    use crate::{GnssSatelliteId, GnssSystem};
    use nalgebra::DMatrix;

    const T0: f64 = 646_229_000.0;
    const SOD: f64 = 200.0;
    const DOY: f64 = 176.0;

    #[derive(Debug, Clone)]
    struct LinearSource {
        t0_j2000_s: f64,
        states: Vec<(GnssSatelliteId, [f64; 3], [f64; 3], f64)>,
    }

    impl LinearSource {
        fn new(t0_j2000_s: f64, states: Vec<(GnssSatelliteId, [f64; 3], [f64; 3], f64)>) -> Self {
            Self { t0_j2000_s, states }
        }
    }

    impl ObservableEphemerisSource for LinearSource {
        fn observable_state_at_j2000_s(
            &self,
            sat: GnssSatelliteId,
            t_j2000_s: f64,
        ) -> Result<ObservableState, ObservablesError> {
            let (_, position, velocity, clock_s) = self
                .states
                .iter()
                .find(|(id, _, _, _)| *id == sat)
                .ok_or(ObservablesError::NoEphemeris)?;
            let dt_s = t_j2000_s - self.t0_j2000_s;
            Ok(ObservableState {
                position_ecef_m: [
                    position[0] + velocity[0] * dt_s,
                    position[1] + velocity[1] * dt_s,
                    position[2] + velocity[2] * dt_s,
                ],
                clock_s: Some(*clock_s),
            })
        }
    }

    impl crate::spp::EphemerisSource for LinearSource {
        fn position_clock_at_j2000_s(
            &self,
            sat: GnssSatelliteId,
            t_j2000_s: f64,
        ) -> Option<([f64; 3], f64)> {
            let (_, position, velocity, clock_s) =
                self.states.iter().find(|(id, _, _, _)| *id == sat)?;
            let dt_s = t_j2000_s - self.t0_j2000_s;
            Some((
                [
                    position[0] + velocity[0] * dt_s,
                    position[1] + velocity[1] * dt_s,
                    position[2] + velocity[2] * dt_s,
                ],
                *clock_s,
            ))
        }
    }

    fn sat(prn: u8) -> GnssSatelliteId {
        GnssSatelliteId::new(GnssSystem::Gps, prn).expect("valid satellite id")
    }

    fn normalized(v: [f64; 3]) -> [f64; 3] {
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / n, v[1] / n, v[2] / n]
    }

    fn source_from_directions(receiver: [f64; 3], directions: &[[f64; 3]]) -> LinearSource {
        let range_m = 22_000_000.0;
        let states = directions
            .iter()
            .enumerate()
            .map(|(idx, direction)| {
                let unit = normalized(*direction);
                (
                    sat((idx + 1) as u8),
                    [
                        receiver[0] + range_m * unit[0],
                        receiver[1] + range_m * unit[1],
                        receiver[2] + range_m * unit[2],
                    ],
                    [0.0; 3],
                    0.0,
                )
            })
            .collect();
        LinearSource::new(T0, states)
    }

    fn tight_epoch_from_source(
        source: &LinearSource,
        receiver: [f64; 3],
        clock_m: f64,
        sigma_m: f64,
    ) -> TightGnssEpoch {
        let observations = source
            .states
            .iter()
            .map(|(satellite_id, _, _, _)| {
                let prediction = transmit_time_satellite_state(
                    source,
                    *satellite_id,
                    receiver,
                    T0,
                    TransmitTimeOptions::default(),
                )
                .expect("satellite state");
                TightGnssObservation::pseudorange(
                    *satellite_id,
                    prediction.geometric_range_m + clock_m,
                    sigma_m,
                )
                .expect("observation")
            })
            .collect();
        TightGnssEpoch::new(T0, observations).expect("tight epoch")
    }

    fn solve_inputs_from_epoch(epoch: &TightGnssEpoch, initial_guess: [f64; 4]) -> SolveInputs {
        SolveInputs {
            observations: epoch
                .observations
                .iter()
                .map(|observation| Observation {
                    satellite_id: observation.satellite_id,
                    pseudorange_m: observation.pseudorange_m,
                })
                .collect(),
            t_rx_j2000_s: epoch.t_j2000_s,
            t_rx_second_of_day_s: SOD,
            day_of_year: DOY,
            initial_guess,
            corrections: Corrections::NONE,
            klobuchar: KlobucharCoeffs {
                alpha: [0.0; 4],
                beta: [0.0; 4],
            },
            beidou_klobuchar: None,
            galileo_nequick: None,
            sbas_iono: None,
            glonass_channels: std::collections::BTreeMap::new(),
            met: SurfaceMet::default(),
            robust: None,
        }
    }

    fn zero_noise_spec() -> ImuSpec {
        ImuSpec::datasheet(
            0.0,
            0.0,
            0.0,
            0.0,
            RANDOM_WALK_BIAS_TAU_S,
            RANDOM_WALK_BIAS_TAU_S,
            None,
            None,
        )
    }

    fn filter_with_config(
        nominal: NavState,
        diagonal: &[f64],
        tight: TightCouplingConfig,
    ) -> InertialFilter {
        let state = InsFilterState::from_diagonal(nominal, ErrorStateLayout::Fifteen, diagonal)
            .expect("state");
        let mut config =
            super::super::loose::InertialFilterConfig::new(zero_noise_spec()).expect("config");
        config.tight = tight;
        InertialFilter::with_config(state, config).expect("filter")
    }

    fn tight_config_for_test() -> TightCouplingConfig {
        TightCouplingConfig {
            initial_clock_bias_variance_m2: 1.0e12,
            initial_clock_drift_variance_m2_s2: 1.0e6,
            clock_bias_random_walk_m2_s: 0.0,
            clock_drift_random_walk_m2_s3: 0.0,
            ..TightCouplingConfig::default()
        }
    }

    fn assert_close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "actual {actual:.17e}, expected {expected:.17e}, tolerance {tolerance:.17e}"
        );
    }

    fn logdet_spd(matrix: &[Vec<f64>]) -> f64 {
        let n = matrix.len();
        let flat = matrix.iter().flatten().copied().collect::<Vec<_>>();
        let dmatrix = DMatrix::from_row_slice(n, n, &flat);
        let cholesky = dmatrix.cholesky().expect("SPD matrix");
        2.0 * cholesky
            .l()
            .diagonal()
            .iter()
            .map(|value| value.ln())
            .sum::<f64>()
    }

    fn position_clock_block(filter: &InertialFilter) -> Vec<Vec<f64>> {
        let base_dim = filter.state.dimension();
        let clock = clock_bias_index(base_dim);
        let indices = [0usize, 1, 2, clock];
        indices
            .iter()
            .map(|row| {
                indices
                    .iter()
                    .map(|col| filter.tight.augmented_covariance[*row][*col])
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn snapshot_position_clock_covariance(
        source: &LinearSource,
        receiver: [f64; 3],
        epoch: &TightGnssEpoch,
    ) -> Vec<Vec<f64>> {
        let mut normal = DMatrix::<f64>::zeros(4, 4);
        for observation in &epoch.observations {
            let prediction = transmit_time_satellite_state(
                source,
                observation.satellite_id,
                receiver,
                epoch.t_j2000_s,
                TransmitTimeOptions::default(),
            )
            .expect("satellite state");
            let h = [
                -prediction.los_unit[0],
                -prediction.los_unit[1],
                -prediction.los_unit[2],
                1.0,
            ];
            let inv_var = 1.0 / (observation.pseudorange_sigma_m * observation.pseudorange_sigma_m);
            for row in 0..4 {
                for col in 0..4 {
                    normal[(row, col)] += h[row] * h[col] * inv_var;
                }
            }
        }
        let covariance = normal.try_inverse().expect("full-rank snapshot");
        (0..4)
            .map(|row| (0..4).map(|col| covariance[(row, col)]).collect())
            .collect()
    }

    #[test]
    fn pseudorange_only_update_matches_spp_clock_oracle_with_frozen_ins_prior() {
        let receiver = [WGS84_A_M, 0.0, 0.0];
        let directions = [
            [1.0, 0.0, 0.0],
            [0.82, 0.42, 0.39],
            [0.83, -0.46, 0.31],
            [0.90, 0.18, -0.40],
            [0.78, -0.25, -0.58],
        ];
        let clock_m = 12.5;
        let source = source_from_directions(receiver, &directions);
        let epoch = tight_epoch_from_source(&source, receiver, clock_m, 1.0);
        let inputs = solve_inputs_from_epoch(&epoch, [receiver[0], receiver[1], receiver[2], 0.0]);
        let spp = crate::spp::solve(&source, &inputs, false).expect("SPP solution");

        let spp_position = spp.position.as_array();
        let nominal = NavState::new(T0, spp_position, [0.0; 3], mat3_identity()).expect("nominal");
        let diagonal = vec![0.0; ERROR_STATE_DIMENSION_15];
        let mut filter = filter_with_config(nominal, &diagonal, tight_config_for_test());

        let update = filter.update_tight(&source, &epoch).expect("tight update");

        assert!(update.applied);
        for (got, expected) in filter
            .state()
            .nominal
            .position_ecef_m
            .iter()
            .zip(spp_position)
        {
            assert_close(*got, expected, 1.0e-6);
        }
        let clock = filter.tight_clock_state().expect("clock");
        assert_close(clock.bias_m, spp.rx_clock_s * C_M_S, 1.0e-5);
    }

    #[test]
    fn doppler_row_uses_range_rate_predictor_geometry_bits() {
        let receiver = [WGS84_A_M, 0.0, 0.0];
        let satellite_id = sat(1);
        let source = LinearSource::new(
            T0,
            vec![(
                satellite_id,
                [WGS84_A_M + 22_000_000.0, 1_000_000.0, 2_000_000.0],
                [120.0, -40.0, 30.0],
                0.0,
            )],
        );
        let sat_state = transmit_time_satellite_state(
            &source,
            satellite_id,
            receiver,
            T0,
            TransmitTimeOptions::default(),
        )
        .expect("satellite state");
        let measured_receiver = ReceiverVelocityState {
            position_m: receiver,
            velocity_m_s: [5.0, -2.0, 1.0],
            clock_drift_m_s: 0.25,
        };
        let velocity_observation = VelocityObservation {
            sat: satellite_id,
            satellite_position_m: sat_state.position_ecef_m,
            satellite_velocity_m_s: sat_state.velocity_m_s,
            measured_range_rate_m_s: 0.0,
            sigma_m_s: 0.05,
            satellite_clock_drift_m_s: 0.01,
        };
        let measured = predict_range_rate_m_s(&velocity_observation, measured_receiver)
            .expect("measured range rate")
            .range_rate_m_s;
        let observation = TightGnssObservation {
            satellite_id,
            pseudorange_m: sat_state.geometric_range_m,
            pseudorange_sigma_m: 2.0,
            range_rate: Some(TightRangeRateObservation {
                measured_range_rate_m_s: measured,
                sigma_m_s: 0.05,
                satellite_clock_drift_m_s: 0.01,
            }),
            carrier_phase: None,
            ionosphere_delay_m: 0.0,
            troposphere_delay_m: 0.0,
        };
        let epoch = TightGnssEpoch::new(T0, vec![observation]).expect("epoch");
        let nominal = NavState::new(T0, receiver, [0.0; 3], mat3_identity()).expect("nominal");
        let filter = filter_with_config(
            nominal,
            &[1.0; ERROR_STATE_DIMENSION_15],
            tight_config_for_test(),
        );
        let correction = tight_coupling_correction(
            &source,
            filter.state(),
            &filter.tight,
            &epoch,
            filter.config.tight,
            [0.0; 3],
        )
        .expect("correction");
        let predicted_at_nominal = predict_range_rate_m_s(
            &VelocityObservation {
                measured_range_rate_m_s: measured,
                ..velocity_observation
            },
            ReceiverVelocityState {
                position_m: receiver,
                velocity_m_s: [0.0; 3],
                clock_drift_m_s: 0.0,
            },
        )
        .expect("nominal range rate");

        let doppler_row = &correction.design[1];
        for axis in 0..3 {
            assert_eq!(
                doppler_row[ERROR_VELOCITY_INDEX + axis].to_bits(),
                (-predicted_at_nominal.los_unit[axis]).to_bits()
            );
        }
        assert_eq!(
            doppler_row[clock_drift_index(filter.state.dimension())].to_bits(),
            1.0_f64.to_bits()
        );
        assert_eq!(
            correction.innovation[1].to_bits(),
            (measured - predicted_at_nominal.range_rate_m_s).to_bits()
        );
    }

    #[test]
    fn singular_snapshot_geometry_keeps_unobserved_prior_covariance() {
        let receiver = [WGS84_A_M, 0.0, 0.0];
        let directions = [[1.0, 0.0, 0.0]; 5];
        let source = source_from_directions(receiver, &directions);
        let epoch = tight_epoch_from_source(&source, receiver, 0.0, 1.0);
        let inputs = solve_inputs_from_epoch(&epoch, [receiver[0], receiver[1], receiver[2], 0.0]);
        assert!(matches!(
            crate::spp::solve(&source, &inputs, false),
            Err(SppError::Singular(_))
        ));

        let nominal = NavState::new(T0, receiver, [0.0; 3], mat3_identity()).expect("nominal");
        let mut diagonal = vec![1.0e-6; ERROR_STATE_DIMENSION_15];
        diagonal[ERROR_POSITION_INDEX] = 100.0;
        diagonal[ERROR_POSITION_INDEX + 1] = 225.0;
        diagonal[ERROR_POSITION_INDEX + 2] = 400.0;
        let mut filter = filter_with_config(nominal, &diagonal, tight_config_for_test());
        let prior_y = filter.state.covariance[ERROR_POSITION_INDEX + 1][ERROR_POSITION_INDEX + 1];
        let prior_z = filter.state.covariance[ERROR_POSITION_INDEX + 2][ERROR_POSITION_INDEX + 2];

        let update = filter.update_tight(&source, &epoch).expect("tight update");

        assert!(update.applied);
        assert!(covariance_is_positive_semidefinite(&filter.state.covariance).expect("PSD"));
        assert_eq!(
            filter.state.covariance[ERROR_POSITION_INDEX + 1][ERROR_POSITION_INDEX + 1].to_bits(),
            prior_y.to_bits()
        );
        assert_eq!(
            filter.state.covariance[ERROR_POSITION_INDEX + 2][ERROR_POSITION_INDEX + 2].to_bits(),
            prior_z.to_bits()
        );
        assert!(filter
            .state
            .nominal
            .position_ecef_m
            .iter()
            .all(|value| value.is_finite() && value.abs() < 1.0e8));
    }

    #[test]
    fn high_dop_fused_covariance_has_lower_logdet_than_snapshot() {
        let receiver = [WGS84_A_M, 0.0, 0.0];
        let directions = [
            [0.44974122498328417, -0.8581153514788689, 0.2477314556265159],
            [0.20081904418348107, 0.5332143328087052, 0.8217993591994339],
            [0.43760604888398824, -0.4903647504582244, 0.7536865114145189],
            [
                0.2148508784686108,
                -0.9558725523345635,
                -0.20036657334663732,
            ],
            [0.30949187488876595, 0.3289789392404428, 0.8921813923827763],
        ];
        let source = source_from_directions(receiver, &directions);
        let epoch = tight_epoch_from_source(&source, receiver, 0.0, 1.0);
        let inputs = solve_inputs_from_epoch(&epoch, [receiver[0], receiver[1], receiver[2], 0.0]);
        let spp = crate::spp::solve(&source, &inputs, false).expect("SPP solution");
        assert_eq!(
            spp.geometry_quality.tier,
            crate::geometry_quality::ObservabilityTier::Weak
        );
        let snapshot_covariance = snapshot_position_clock_covariance(&source, receiver, &epoch);
        let snapshot_logdet = logdet_spd(&snapshot_covariance);

        let nominal = NavState::new(T0, receiver, [0.0; 3], mat3_identity()).expect("nominal");
        let mut diagonal = vec![1.0; ERROR_STATE_DIMENSION_15];
        for axis in 0..3 {
            diagonal[ERROR_POSITION_INDEX + axis] = 1.0e8;
        }
        let mut filter = filter_with_config(nominal, &diagonal, tight_config_for_test());

        filter.update_tight(&source, &epoch).expect("tight update");

        let fused_logdet = logdet_spd(&position_clock_block(&filter));
        assert!(
            fused_logdet < snapshot_logdet,
            "fused {fused_logdet:.17e}, snapshot {snapshot_logdet:.17e}"
        );
    }

    #[test]
    fn outage_growth_and_single_satellite_observed_direction_update() {
        let receiver = [WGS84_A_M, 0.0, 0.0];
        let nominal = NavState::new(T0, receiver, [0.0; 3], mat3_identity()).expect("nominal");
        let diagonal = vec![1.0; ERROR_STATE_DIMENSION_15];
        let state = InsFilterState::from_diagonal(nominal, ErrorStateLayout::Fifteen, &diagonal)
            .expect("state");
        let spec = ImuSpec::datasheet(0.02, 0.001, 0.004, 2.0e-4, 300.0, 300.0, None, None);
        let mut config = super::super::loose::InertialFilterConfig::new(spec).expect("config");
        config.tight = TightCouplingConfig {
            light_time: false,
            sagnac: false,
            initial_clock_bias_variance_m2: 100.0,
            initial_clock_drift_variance_m2_s2: 1.0,
            clock_bias_random_walk_m2_s: 4.0,
            clock_drift_random_walk_m2_s3: 0.25,
            ..TightCouplingConfig::default()
        };
        let mut filter = InertialFilter::with_config(state, config).expect("filter");
        let mut previous_logdet = logdet_spd(&filter.tight.augmented_covariance);

        for step in 1..=3 {
            filter
                .propagate(ImuSample::increment(
                    T0 + step as f64,
                    [0.0; 3],
                    [0.0; 3],
                    1.0,
                ))
                .expect("propagate");
            let next_logdet = logdet_spd(&filter.tight.augmented_covariance);
            assert!(
                next_logdet > previous_logdet,
                "step {step} logdet {next_logdet:.17e} <= {previous_logdet:.17e}"
            );
            previous_logdet = next_logdet;
        }

        let current_position = filter.state.nominal.position_ecef_m;
        let satellite_id = sat(1);
        let source = LinearSource::new(
            filter.state.nominal.t_j2000_s,
            vec![(
                satellite_id,
                [
                    current_position[0] + 22_000_000.0,
                    current_position[1],
                    current_position[2],
                ],
                [0.0; 3],
                0.0,
            )],
        );
        let prediction = transmit_time_satellite_state(
            &source,
            satellite_id,
            current_position,
            filter.state.nominal.t_j2000_s,
            TransmitTimeOptions {
                light_time: false,
                sagnac: false,
            },
        )
        .expect("satellite state");
        let clock = filter.tight_clock_state().expect("clock");
        let epoch = TightGnssEpoch::new(
            filter.state.nominal.t_j2000_s,
            vec![TightGnssObservation::pseudorange(
                satellite_id,
                prediction.geometric_range_m + clock.bias_m,
                0.5,
            )
            .expect("observation")],
        )
        .expect("epoch");
        let pre = filter.state.covariance.clone();

        filter
            .update_tight(&source, &epoch)
            .expect("single-sat update");

        assert!(
            filter.state.covariance[ERROR_POSITION_INDEX][ERROR_POSITION_INDEX]
                < pre[ERROR_POSITION_INDEX][ERROR_POSITION_INDEX]
        );
        for axis in [1usize, 2] {
            assert_eq!(
                filter.state.covariance[ERROR_POSITION_INDEX + axis][ERROR_POSITION_INDEX + axis]
                    .to_bits(),
                pre[ERROR_POSITION_INDEX + axis][ERROR_POSITION_INDEX + axis].to_bits()
            );
        }
    }
}
