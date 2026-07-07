//! RTS fixed-interval smoothing for fusion error-state histories.
//!
//! The backward pass follows the Rauch-Tung-Striebel recursion as presented in
//! Bar-Shalom, Li, and Kirubarajan, Estimation with Applications to Tracking
//! and Navigation, 2001: `G_k = P_k Phi_k' P_{k+1|k}^{-1}`,
//! `x_k^s = x_k + G_k (x_{k+1}^s - x_{k+1|k})`, and
//! `P_k^s = P_k + G_k (P_{k+1}^s - P_{k+1|k}) G_k'`.

use crate::inertial::validate_finite;

use super::ekf::{apply_closed_loop_navigation_error, apply_closed_loop_scale_error};
use super::loose::{FusionUpdate, GnssFixMeasurement, InertialFilter};
use super::state::{
    identity, invalid_input, matmul, matrix_add, matrix_sub, reproject_covariance_psd, solve_spd,
    symmetrize_in_place, transpose, validate_covariance_matrix, validate_finite_slice,
    validate_square_matrix, ErrorStateVector, FusionError, InsFilterState, ERROR_ACCEL_SCALE_INDEX,
    ERROR_ATTITUDE_INDEX, ERROR_GYRO_SCALE_INDEX, ERROR_POSITION_INDEX, ERROR_VELOCITY_INDEX,
};
use super::tight::{TightGnssEpoch, TIGHT_CLOCK_STATE_COUNT};
use super::timesync::InertialFilterSnapshot;
use crate::inertial::ImuSample;
use crate::observables::ObservableEphemerisSource;

/// One propagated interval returned by the fusion prediction core.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct FusionPredictionStep {
    /// Error-state transition matrix for the interval.
    pub(super) transition: Vec<Vec<f64>>,
}

/// A recorded fusion epoch for RTS smoothing.
///
/// `predicted` is the filter state at the measurement epoch before correction.
/// `updated` is the closed-loop state after the correction. The transition maps
/// the previous epoch's updated error state into this epoch's predicted
/// error-state frame.
#[derive(Debug, Clone, PartialEq)]
pub struct FusionRtsEpoch {
    /// Epoch in seconds since J2000.
    pub t_j2000_s: f64,
    /// Pre-update checkpoint at this epoch.
    pub predicted: InertialFilterSnapshot,
    /// Post-update checkpoint at this epoch.
    pub updated: InertialFilterSnapshot,
    /// Error-state transition from the previous updated epoch to this predicted epoch.
    pub transition_from_previous: Option<Vec<Vec<f64>>>,
}

impl FusionRtsEpoch {
    /// Build and validate one smoothing-history epoch.
    pub fn new(
        predicted: InertialFilterSnapshot,
        updated: InertialFilterSnapshot,
        transition_from_previous: Option<Vec<Vec<f64>>>,
    ) -> Result<Self, FusionError> {
        let t_j2000_s = predicted.state.nominal.t_j2000_s;
        let epoch = Self {
            t_j2000_s,
            predicted,
            updated,
            transition_from_previous,
        };
        epoch.validate()?;
        Ok(epoch)
    }

    fn validate(&self) -> Result<(), FusionError> {
        validate_finite(self.t_j2000_s, "t_j2000_s").map_err(FusionError::from)?;
        validate_snapshot(&self.predicted, "predicted")?;
        validate_snapshot(&self.updated, "updated")?;
        if self.predicted.state.layout() != self.updated.state.layout() {
            return Err(invalid_input(
                "layout",
                "predicted and updated layouts differ",
            ));
        }
        if self.predicted.state.nominal.t_j2000_s != self.t_j2000_s
            || self.updated.state.nominal.t_j2000_s != self.t_j2000_s
        {
            return Err(invalid_input("t_j2000_s", "snapshot epochs must match"));
        }
        if let Some(transition) = &self.transition_from_previous {
            validate_transition_dimension(transition, self.updated.state.dimension())?;
        }
        Ok(())
    }
}

/// Recorded history accepted by [`smooth_fusion_rts`].
#[derive(Debug, Clone, PartialEq)]
pub struct FusionRtsHistory {
    /// Ordered epochs to smooth.
    pub epochs: Vec<FusionRtsEpoch>,
}

impl FusionRtsHistory {
    /// Build and validate an ordered history.
    pub fn new(epochs: Vec<FusionRtsEpoch>) -> Result<Self, FusionError> {
        let history = Self { epochs };
        history.validate()?;
        Ok(history)
    }

    /// Validate epoch order, layouts, and transition placement.
    pub fn validate(&self) -> Result<(), FusionError> {
        if self.epochs.is_empty() {
            return Err(invalid_input("history", "must not be empty"));
        }
        let layout = self.epochs[0].updated.state.layout();
        for (idx, epoch) in self.epochs.iter().enumerate() {
            epoch.validate()?;
            if epoch.updated.state.layout() != layout {
                return Err(invalid_input("layout", "history layouts differ"));
            }
            match (idx, &epoch.transition_from_previous) {
                (0, None) => {}
                (0, Some(_)) => {
                    return Err(invalid_input(
                        "transition_from_previous",
                        "first epoch must not have a transition",
                    ));
                }
                (_, Some(transition)) => {
                    validate_transition_dimension(transition, layout.dimension())?;
                    if epoch.t_j2000_s < self.epochs[idx - 1].t_j2000_s {
                        return Err(invalid_input("history", "epochs must not regress"));
                    }
                }
                (_, None) => {
                    return Err(invalid_input(
                        "transition_from_previous",
                        "missing transition",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Builder for recording a forward fusion pass for RTS smoothing.
#[derive(Debug, Clone, PartialEq)]
pub struct FusionRtsHistoryBuilder {
    epochs: Vec<FusionRtsEpoch>,
    pending_transition: Option<Vec<Vec<f64>>>,
}

impl FusionRtsHistoryBuilder {
    /// Start a history from the filter's current closed-loop checkpoint.
    pub fn from_filter(filter: &InertialFilter) -> Result<Self, FusionError> {
        let snapshot = filter.snapshot();
        let initial = FusionRtsEpoch::new(snapshot.clone(), snapshot, None)?;
        Ok(Self {
            epochs: vec![initial],
            pending_transition: None,
        })
    }

    /// Start an empty history for callers that record epochs manually.
    pub const fn empty() -> Self {
        Self {
            epochs: Vec::new(),
            pending_transition: None,
        }
    }

    /// Append a propagated transition to the current open interval.
    pub fn record_prediction_step(&mut self, transition: Vec<Vec<f64>>) -> Result<(), FusionError> {
        let dimension = transition.len();
        validate_square_matrix(&transition, dimension, "transition")?;
        let combined = if let Some(previous) = &self.pending_transition {
            matmul_square_transition(&transition, previous)?
        } else {
            transition
        };
        self.pending_transition = Some(combined);
        Ok(())
    }

    /// Append an update epoch, consuming the transition accumulated since the previous epoch.
    pub fn record_update(
        &mut self,
        predicted: InertialFilterSnapshot,
        updated: InertialFilterSnapshot,
    ) -> Result<(), FusionError> {
        let transition = if self.epochs.is_empty() {
            None
        } else if let Some(transition) = self.pending_transition.clone() {
            Some(transition)
        } else {
            let last_epoch = self.epochs[self.epochs.len() - 1].t_j2000_s;
            if predicted.state.nominal.t_j2000_s == last_epoch {
                Some(identity(predicted.state.dimension()))
            } else {
                return Err(invalid_input("transition", "missing propagated interval"));
            }
        };
        let epoch = FusionRtsEpoch::new(predicted, updated, transition)?;
        let mut epochs = self.epochs.clone();
        epochs.push(epoch);
        FusionRtsHistory {
            epochs: epochs.clone(),
        }
        .validate()?;
        self.epochs = epochs;
        self.pending_transition = None;
        Ok(())
    }

    /// Return a validated history.
    pub fn finish(self) -> Result<FusionRtsHistory, FusionError> {
        if self.pending_transition.is_some() {
            return Err(invalid_input("transition", "unclosed propagated interval"));
        }
        FusionRtsHistory::new(self.epochs)
    }

    fn validate_update_ready(&self) -> Result<(), FusionError> {
        Ok(())
    }
}

fn matmul_square_transition(
    left: &[Vec<f64>],
    right: &[Vec<f64>],
) -> Result<Vec<Vec<f64>>, FusionError> {
    let dimension = left.len();
    if right.len() != dimension {
        return Err(FusionError::DimensionMismatch {
            field: "transition",
            expected: dimension,
            actual: right.len(),
        });
    }
    let mut out = vec![vec![0.0; dimension]; dimension];
    for row in 0..dimension {
        if left[row].len() != dimension {
            return Err(FusionError::DimensionMismatch {
                field: "transition",
                expected: dimension,
                actual: left[row].len(),
            });
        }
        for k in 0..dimension {
            let lhs = left[row][k];
            if lhs == 0.0 {
                continue;
            }
            if right[k].len() != dimension {
                return Err(FusionError::DimensionMismatch {
                    field: "transition",
                    expected: dimension,
                    actual: right[k].len(),
                });
            }
            for col in 0..dimension {
                out[row][col] += lhs * right[k][col];
            }
        }
    }
    Ok(out)
}

/// One epoch in a smoothed fusion trajectory.
#[derive(Debug, Clone, PartialEq)]
pub struct SmoothedFusionEpoch {
    /// Epoch in seconds since J2000.
    pub t_j2000_s: f64,
    /// Smoothed closed-loop snapshot.
    pub snapshot: InertialFilterSnapshot,
    /// INS error-state and tight-clock correction applied to the filtered checkpoint.
    pub error_state_correction: Vec<f64>,
    /// Smoothed covariance over the INS error state and tight clock states.
    pub covariance: Vec<Vec<f64>>,
    /// RTS gain from this epoch to the next, absent at the final epoch.
    pub rts_gain_to_next: Option<Vec<Vec<f64>>>,
}

/// Smoothed trajectory returned by [`smooth_fusion_rts`].
#[derive(Debug, Clone, PartialEq)]
pub struct SmoothedFusionTrajectory {
    /// Smoothed epochs in the same order as the recorded history.
    pub epochs: Vec<SmoothedFusionEpoch>,
}

/// Apply the RTS fixed-interval smoother to a recorded fusion history.
pub fn smooth_fusion_rts(
    history: &FusionRtsHistory,
) -> Result<SmoothedFusionTrajectory, FusionError> {
    history.validate()?;
    let len = history.epochs.len();
    let base_dimension = history.epochs[0].updated.state.dimension();
    let dimension = smoothing_dimension(base_dimension);
    let mut output: Vec<Option<SmoothedFusionEpoch>> = vec![None; len];

    let final_epoch = &history.epochs[len - 1];
    let final_covariance = final_epoch.updated.tight.augmented_covariance.clone();
    validate_covariance_matrix(&final_covariance, dimension, "smoothed_covariance")?;
    output[len - 1] = Some(SmoothedFusionEpoch {
        t_j2000_s: final_epoch.t_j2000_s,
        snapshot: final_epoch.updated.clone(),
        error_state_correction: vec![0.0; dimension],
        covariance: final_covariance,
        rts_gain_to_next: None,
    });

    for idx in (0..len - 1).rev() {
        let current = &history.epochs[idx];
        let next = &history.epochs[idx + 1];
        let next_smoothed = output[idx + 1]
            .as_ref()
            .expect("next smoothed epoch is populated");
        let transition = next
            .transition_from_previous
            .as_ref()
            .ok_or_else(|| invalid_input("transition_from_previous", "missing transition"))?;
        let transition = smoothing_transition(
            transition,
            base_dimension,
            next.t_j2000_s - current.t_j2000_s,
        )?;
        let gain = rts_gain(
            &current.updated.tight.augmented_covariance,
            &transition,
            &next.predicted.tight.augmented_covariance,
        )?;
        let next_delta = error_state_between(&next.predicted, &next_smoothed.snapshot)?;
        let correction = matvec_allow_empty(&gain, &next_delta)?;
        validate_finite_slice(&correction, "smoothed_error_state")?;

        let mut covariance = smoothed_covariance(
            &current.updated.tight.augmented_covariance,
            &gain,
            &next.predicted.tight.augmented_covariance,
            &next_smoothed.covariance,
        )?;
        reproject_covariance_psd(&mut covariance, "smoothed_covariance")?;
        validate_covariance_matrix(&covariance, dimension, "smoothed_covariance")?;

        let mut snapshot = apply_error_state_correction(&current.updated, &correction)?;
        set_smoothed_covariance(&mut snapshot, &covariance)?;
        validate_snapshot(&snapshot, "smoothed")?;

        output[idx] = Some(SmoothedFusionEpoch {
            t_j2000_s: current.t_j2000_s,
            snapshot,
            error_state_correction: correction,
            covariance,
            rts_gain_to_next: Some(gain),
        });
    }

    let epochs = output
        .into_iter()
        .map(|epoch| epoch.expect("all smoothed epochs populated"))
        .collect::<Vec<_>>();
    Ok(SmoothedFusionTrajectory { epochs })
}

impl InertialFilter {
    /// Propagate and record the transition for later RTS smoothing.
    pub fn propagate_recorded(
        &mut self,
        sample: ImuSample,
        history: &mut FusionRtsHistoryBuilder,
    ) -> Result<&InsFilterState, FusionError> {
        let previous_t_j2000_s = self.state.nominal.t_j2000_s;
        self.time_sync
            .validate_next_imu(previous_t_j2000_s, sample)?;
        let mut working_filter = self.clone();
        let mut working_history = history.clone();
        let prediction = working_filter.propagate_core(sample)?;
        working_history.record_prediction_step(prediction.transition)?;
        working_filter
            .time_sync
            .push_imu(previous_t_j2000_s, sample);
        *self = working_filter;
        *history = working_history;
        Ok(&self.state)
    }

    /// Apply a loose update and record the before/after checkpoints.
    pub fn update_loose_recorded(
        &mut self,
        measurement: &GnssFixMeasurement,
        history: &mut FusionRtsHistoryBuilder,
    ) -> Result<FusionUpdate, FusionError> {
        history.validate_update_ready()?;
        let predicted = self.snapshot();
        let mut working_filter = self.clone();
        let mut working_history = history.clone();
        let update = working_filter.update_loose(measurement)?;
        let updated = working_filter.snapshot();
        working_history.record_update(predicted, updated)?;
        *self = working_filter;
        *history = working_history;
        Ok(update)
    }

    /// Apply a stationary update and record the before/after checkpoints.
    pub fn update_stationary_recorded(
        &mut self,
        history: &mut FusionRtsHistoryBuilder,
    ) -> Result<Option<FusionUpdate>, FusionError> {
        history.validate_update_ready()?;
        let predicted = self.snapshot();
        let mut working_filter = self.clone();
        let mut working_history = history.clone();
        let update = working_filter.update_stationary()?;
        if update.is_some() {
            let updated = working_filter.snapshot();
            working_history.record_update(predicted, updated)?;
            *self = working_filter;
            *history = working_history;
        }
        Ok(update)
    }

    /// Apply a non-holonomic constraint and record the before/after checkpoints.
    pub fn update_non_holonomic_recorded(
        &mut self,
        history: &mut FusionRtsHistoryBuilder,
    ) -> Result<Option<FusionUpdate>, FusionError> {
        history.validate_update_ready()?;
        let predicted = self.snapshot();
        let mut working_filter = self.clone();
        let mut working_history = history.clone();
        let update = working_filter.update_non_holonomic()?;
        if update.is_some() {
            let updated = working_filter.snapshot();
            working_history.record_update(predicted, updated)?;
            *self = working_filter;
            *history = working_history;
        }
        Ok(update)
    }

    /// Apply a tight update and record the before/after checkpoints.
    pub fn update_tight_recorded(
        &mut self,
        source: &dyn ObservableEphemerisSource,
        epoch: &TightGnssEpoch,
        history: &mut FusionRtsHistoryBuilder,
    ) -> Result<FusionUpdate, FusionError> {
        history.validate_update_ready()?;
        let predicted = self.snapshot();
        let mut working_filter = self.clone();
        let mut working_history = history.clone();
        let update = working_filter.update_tight(source, epoch)?;
        let updated = working_filter.snapshot();
        working_history.record_update(predicted, updated)?;
        *self = working_filter;
        *history = working_history;
        Ok(update)
    }
}

fn rts_gain(
    filtered_covariance: &[Vec<f64>],
    transition: &[Vec<f64>],
    predicted_covariance: &[Vec<f64>],
) -> Result<Vec<Vec<f64>>, FusionError> {
    let dimension = filtered_covariance.len();
    validate_covariance_matrix(filtered_covariance, dimension, "filtered_covariance")?;
    validate_square_matrix(transition, dimension, "transition")?;
    validate_covariance_matrix(predicted_covariance, dimension, "predicted_covariance")?;
    let transition_t = transpose(transition)?;
    let cross = matmul(filtered_covariance, &transition_t)?;
    let mut gain = vec![vec![0.0; dimension]; dimension];
    let mut scratch = crate::astro::math::linear::FlatCholeskySolveScratch::default();
    for row in 0..dimension {
        gain[row] =
            solve_spd(predicted_covariance, &cross[row], &mut scratch).map_err(|error| {
                if matches!(error, FusionError::SingularInnovation) {
                    FusionError::NonPositiveDefinite {
                        field: "predicted_covariance",
                    }
                } else {
                    error
                }
            })?;
    }
    Ok(gain)
}

fn smoothed_covariance(
    filtered_covariance: &[Vec<f64>],
    gain: &[Vec<f64>],
    predicted_covariance_next: &[Vec<f64>],
    smoothed_covariance_next: &[Vec<f64>],
) -> Result<Vec<Vec<f64>>, FusionError> {
    let dimension = filtered_covariance.len();
    validate_covariance_matrix(filtered_covariance, dimension, "filtered_covariance")?;
    validate_square_matrix(gain, dimension, "rts_gain")?;
    validate_covariance_matrix(
        predicted_covariance_next,
        dimension,
        "predicted_covariance_next",
    )?;
    validate_covariance_matrix(
        smoothed_covariance_next,
        dimension,
        "smoothed_covariance_next",
    )?;

    let delta = matrix_sub(smoothed_covariance_next, predicted_covariance_next)?;
    let left = matmul(gain, &delta)?;
    let gain_t = transpose(gain)?;
    let adjustment = matmul(&left, &gain_t)?;
    let mut covariance = matrix_add(filtered_covariance, &adjustment)?;
    symmetrize_in_place(&mut covariance);
    Ok(covariance)
}

fn error_state_between(
    reference: &InertialFilterSnapshot,
    candidate: &InertialFilterSnapshot,
) -> Result<Vec<f64>, FusionError> {
    validate_snapshot(reference, "reference")?;
    validate_snapshot(candidate, "candidate")?;
    if reference.state.layout() != candidate.state.layout() {
        return Err(invalid_input("layout", "states use different layouts"));
    }
    let layout = reference.state.layout();
    let base_dimension = layout.dimension();
    let mut dx = vec![0.0; smoothing_dimension(base_dimension)];
    for axis in 0..3 {
        dx[ERROR_POSITION_INDEX + axis] = reference.state.nominal.position_ecef_m[axis]
            - candidate.state.nominal.position_ecef_m[axis];
        dx[ERROR_VELOCITY_INDEX + axis] = reference.state.nominal.velocity_ecef_mps[axis]
            - candidate.state.nominal.velocity_ecef_mps[axis];
        dx[ERROR_ATTITUDE_INDEX + axis] = attitude_error_component(
            reference.state.nominal.attitude_body_to_ecef,
            candidate.state.nominal.attitude_body_to_ecef,
            axis,
        );
        dx[super::state::ERROR_ACCEL_BIAS_INDEX + axis] = candidate.state.nominal.accel_bias_mps2
            [axis]
            - reference.state.nominal.accel_bias_mps2[axis];
        dx[super::state::ERROR_GYRO_BIAS_INDEX + axis] = candidate.state.nominal.gyro_bias_rps
            [axis]
            - reference.state.nominal.gyro_bias_rps[axis];
    }
    if layout.includes_scale_factors() {
        for axis in 0..3 {
            dx[ERROR_ACCEL_SCALE_INDEX + axis] =
                candidate.state.accel_scale_factor[axis] - reference.state.accel_scale_factor[axis];
            dx[ERROR_GYRO_SCALE_INDEX + axis] =
                candidate.state.gyro_scale_factor[axis] - reference.state.gyro_scale_factor[axis];
        }
    }
    dx[base_dimension] = candidate.tight.clock_bias_m - reference.tight.clock_bias_m;
    dx[base_dimension + 1] = candidate.tight.clock_drift_m_s - reference.tight.clock_drift_m_s;
    validate_finite_slice(&dx, "error_state_delta")?;
    Ok(dx)
}

fn attitude_error_component(
    reference_body_to_ecef: [[f64; 3]; 3],
    candidate_body_to_ecef: [[f64; 3]; 3],
    axis: usize,
) -> f64 {
    let reference_ecef_to_body = crate::astro::math::mat3::inline_tr(&reference_body_to_ecef);
    let delta =
        crate::astro::math::mat3::inline_rxr(&candidate_body_to_ecef, &reference_ecef_to_body);
    let skew = [
        [
            0.0,
            0.5 * (delta[1][0] - delta[0][1]),
            0.5 * (delta[2][0] - delta[0][2]),
        ],
        [
            0.5 * (delta[0][1] - delta[1][0]),
            0.0,
            0.5 * (delta[2][1] - delta[1][2]),
        ],
        [
            0.5 * (delta[0][2] - delta[2][0]),
            0.5 * (delta[1][2] - delta[2][1]),
            0.0,
        ],
    ];
    match axis {
        0 => skew[2][1],
        1 => skew[0][2],
        _ => skew[1][0],
    }
}

fn apply_error_state_correction(
    snapshot: &InertialFilterSnapshot,
    correction: &[f64],
) -> Result<InertialFilterSnapshot, FusionError> {
    let mut smoothed = snapshot.clone();
    let base_dimension = smoothed.state.dimension();
    smoothed
        .state
        .layout()
        .validate_len(base_dimension, "smoothed_error_state")?;
    if correction.len() != base_dimension && correction.len() != smoothing_dimension(base_dimension)
    {
        return Err(FusionError::DimensionMismatch {
            field: "smoothed_error_state",
            expected: smoothing_dimension(base_dimension),
            actual: correction.len(),
        });
    }
    validate_finite_slice(correction, "smoothed_error_state")?;
    apply_closed_loop_navigation_error(&mut smoothed.state.nominal, &correction[..base_dimension])?;
    apply_closed_loop_scale_error(&mut smoothed.state, &correction[..base_dimension]);
    if correction.len() == smoothing_dimension(base_dimension) {
        smoothed.tight.clock_bias_m += correction[base_dimension];
        smoothed.tight.clock_drift_m_s += correction[base_dimension + 1];
    }
    smoothed.state.error_state =
        ErrorStateVector::from_vec(smoothed.state.layout(), vec![0.0; base_dimension])?;
    smoothed.state.validate()?;
    Ok(smoothed)
}

fn set_smoothed_covariance(
    snapshot: &mut InertialFilterSnapshot,
    covariance: &[Vec<f64>],
) -> Result<(), FusionError> {
    let base_dim = snapshot.state.dimension();
    let smooth_dim = smoothing_dimension(base_dim);
    validate_covariance_matrix(covariance, smooth_dim, "smoothed_covariance")?;

    snapshot.state.covariance = covariance
        .iter()
        .take(base_dim)
        .map(|row| row[..base_dim].to_vec())
        .collect();
    snapshot.tight.augmented_covariance = covariance.to_vec();
    validate_snapshot(snapshot, "smoothed")
}

fn validate_snapshot(
    snapshot: &InertialFilterSnapshot,
    field: &'static str,
) -> Result<(), FusionError> {
    snapshot.state.validate()?;
    validate_finite_slice(
        &[
            snapshot.tight.clock_bias_m,
            snapshot.tight.clock_drift_m_s,
            snapshot.last_body_rate_wrt_ecef_rps[0],
            snapshot.last_body_rate_wrt_ecef_rps[1],
            snapshot.last_body_rate_wrt_ecef_rps[2],
        ],
        field,
    )?;
    validate_covariance_matrix(
        &snapshot.tight.augmented_covariance,
        smoothing_dimension(snapshot.state.dimension()),
        "tight_augmented_covariance",
    )?;
    validate_tight_base_block_matches_state(snapshot)
}

fn validate_transition_dimension(
    transition: &[Vec<f64>],
    base_dimension: usize,
) -> Result<(), FusionError> {
    let dimension = transition.len();
    let smooth_dimension = smoothing_dimension(base_dimension);
    if dimension != base_dimension && dimension != smooth_dimension {
        return Err(FusionError::DimensionMismatch {
            field: "transition",
            expected: smooth_dimension,
            actual: dimension,
        });
    }
    validate_square_matrix(transition, dimension, "transition")
}

fn smoothing_transition(
    transition: &[Vec<f64>],
    base_dimension: usize,
    dt_s: f64,
) -> Result<Vec<Vec<f64>>, FusionError> {
    validate_transition_dimension(transition, base_dimension)?;
    validate_finite(dt_s, "dt_s").map_err(FusionError::from)?;
    if dt_s < 0.0 {
        return Err(invalid_input("dt_s", "must be non-negative"));
    }
    let smooth_dimension = smoothing_dimension(base_dimension);
    if transition.len() == smooth_dimension {
        return Ok(transition.to_vec());
    }

    let mut augmented = vec![vec![0.0; smooth_dimension]; smooth_dimension];
    for (idx, row) in augmented.iter_mut().enumerate() {
        row[idx] = 1.0;
    }
    for row in 0..base_dimension {
        augmented[row][..base_dimension].copy_from_slice(&transition[row][..base_dimension]);
    }
    augmented[base_dimension][base_dimension + 1] = dt_s;
    Ok(augmented)
}

const fn smoothing_dimension(base_dimension: usize) -> usize {
    base_dimension + TIGHT_CLOCK_STATE_COUNT
}

fn validate_tight_base_block_matches_state(
    snapshot: &InertialFilterSnapshot,
) -> Result<(), FusionError> {
    let base_dimension = snapshot.state.dimension();
    for row in 0..base_dimension {
        for col in 0..base_dimension {
            if snapshot.tight.augmented_covariance[row][col].to_bits()
                != snapshot.state.covariance[row][col].to_bits()
            {
                return Err(invalid_input(
                    "tight_augmented_covariance",
                    "base block must match state covariance",
                ));
            }
        }
    }
    Ok(())
}

fn matvec_allow_empty(matrix: &[Vec<f64>], vector: &[f64]) -> Result<Vec<f64>, FusionError> {
    if matrix.is_empty() {
        return Ok(Vec::new());
    }
    super::state::matvec(matrix, vector)
}
