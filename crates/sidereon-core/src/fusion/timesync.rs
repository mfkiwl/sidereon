//! Time alignment for loosely coupled GNSS/INS updates.
//!
//! The caller supplies all IMU and GNSS epochs in one GNSS time scale. The
//! routines here keep the filter sans-IO: they retain enough recent IMU and
//! filter history to apply a late GNSS fix at its measurement epoch, then replay
//! retained inertial samples and later GNSS fixes.

use std::collections::VecDeque;

use crate::astro::math::vec3::{add3, scale3, sub3};
use crate::inertial::{ImuSample, ImuSampleKind};

use super::loose::{FusionUpdate, GnssFixMeasurement, InertialFilter};
use super::state::{invalid_input, validate_positive, FusionError, InsFilterState};

/// Default number of retained IMU samples for time-sync replay.
pub const DEFAULT_TIME_SYNC_IMU_CAPACITY: usize = 256;
/// Default number of retained GNSS checkpoints for time-sync replay.
pub const DEFAULT_TIME_SYNC_CHECKPOINT_CAPACITY: usize = 64;

/// Retained history limits for bounded-latency time synchronization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSyncHistoryConfig {
    /// Number of recent IMU samples retained for fractional replay.
    pub imu_capacity: usize,
    /// Number of recent filter checkpoints retained at GNSS epochs.
    pub checkpoint_capacity: usize,
}

impl Default for TimeSyncHistoryConfig {
    fn default() -> Self {
        Self {
            imu_capacity: DEFAULT_TIME_SYNC_IMU_CAPACITY,
            checkpoint_capacity: DEFAULT_TIME_SYNC_CHECKPOINT_CAPACITY,
        }
    }
}

impl TimeSyncHistoryConfig {
    /// Build retained-history limits for time-synchronized replay.
    pub const fn new(imu_capacity: usize, checkpoint_capacity: usize) -> Self {
        Self {
            imu_capacity,
            checkpoint_capacity,
        }
    }

    /// Validate that both retained-history capacities are nonzero.
    pub fn validate(&self) -> Result<(), FusionError> {
        validate_capacity(self.imu_capacity, "imu_capacity")?;
        validate_capacity(self.checkpoint_capacity, "checkpoint_capacity")
    }
}

/// Snapshot of a closed-loop inertial filter at one epoch.
#[derive(Debug, Clone, PartialEq)]
pub struct InertialFilterSnapshot {
    /// Closed-loop INS state and covariance.
    pub state: InsFilterState,
    /// Last propagated body angular rate relative to ECEF, resolved in body axes.
    pub last_body_rate_wrt_ecef_rps: [f64; 3],
}

/// Current retained-history occupancy for time synchronization.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeSyncHistoryStatus {
    /// Configured IMU sample capacity.
    pub imu_capacity: usize,
    /// Number of retained IMU samples.
    pub imu_len: usize,
    /// Configured checkpoint capacity.
    pub checkpoint_capacity: usize,
    /// Number of retained filter checkpoints.
    pub checkpoint_len: usize,
    /// Oldest retained IMU sample end epoch, if any.
    pub oldest_imu_epoch_j2000_s: Option<f64>,
    /// Newest retained IMU sample end epoch, if any.
    pub newest_imu_epoch_j2000_s: Option<f64>,
    /// Oldest retained checkpoint epoch, if any.
    pub oldest_checkpoint_epoch_j2000_s: Option<f64>,
    /// Newest retained checkpoint epoch, if any.
    pub newest_checkpoint_epoch_j2000_s: Option<f64>,
}

/// Result of a time-synchronized loose GNSS update.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeSyncUpdate {
    /// Loose-coupled EKF update report for the newly supplied measurement.
    pub update: FusionUpdate,
    /// Whether the measurement epoch was older than the current propagated epoch.
    pub late_measurement: bool,
    /// Number of IMU segments replayed while applying the measurement.
    pub replayed_imu_segments: usize,
    /// Checkpoint epoch used as the replay start.
    pub restored_checkpoint_epoch_j2000_s: f64,
    /// Filter epoch after any required replay is complete.
    pub current_epoch_j2000_s: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct TimeSyncHistory {
    config: TimeSyncHistoryConfig,
    imu_samples: VecDeque<StoredImuSample>,
    checkpoints: VecDeque<StoredCheckpoint>,
    measurements: VecDeque<GnssFixMeasurement>,
}

impl TimeSyncHistory {
    pub(super) fn from_initial(state: &InsFilterState) -> Self {
        let mut history = Self {
            config: TimeSyncHistoryConfig::default(),
            imu_samples: VecDeque::new(),
            checkpoints: VecDeque::new(),
            measurements: VecDeque::new(),
        };
        history.push_checkpoint(InertialFilterSnapshot {
            state: state.clone(),
            last_body_rate_wrt_ecef_rps: [0.0; 3],
        });
        history
    }

    pub(super) fn validate_next_imu(
        &self,
        previous_t_j2000_s: f64,
        sample: ImuSample,
    ) -> Result<(), FusionError> {
        validate_epoch(previous_t_j2000_s, "previous_t_j2000_s")?;
        validate_epoch(sample.t_j2000_s, "t_j2000_s")?;
        if sample.t_j2000_s <= previous_t_j2000_s {
            return Err(invalid_input(
                "imu_samples",
                "must be strictly ordered by epoch",
            ));
        }
        if let Some(last) = self.imu_samples.back() {
            if sample.t_j2000_s <= last.sample.t_j2000_s {
                return Err(invalid_input(
                    "imu_samples",
                    "must be strictly ordered by epoch",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn push_imu(&mut self, previous_t_j2000_s: f64, sample: ImuSample) {
        let previous_rate = self.imu_samples.back().and_then(|stored| {
            rate_payload(stored.sample).map(|payload| RateEndpoint {
                t_j2000_s: stored.sample.t_j2000_s,
                specific_force_mps2: payload.specific_force_mps2,
                angular_rate_rps: payload.angular_rate_rps,
            })
        });
        bounded_push(
            &mut self.imu_samples,
            self.config.imu_capacity,
            StoredImuSample {
                previous_t_j2000_s,
                sample,
                previous_rate,
            },
        );
    }

    /// Epoch of the most recently accepted GNSS measurement, if any.
    pub(super) fn last_measurement_t_j2000_s(&self) -> Option<f64> {
        self.measurements.back().map(|m| m.t_j2000_s)
    }

    pub(super) fn push_measurement_and_checkpoint(
        &mut self,
        measurement: GnssFixMeasurement,
        snapshot: InertialFilterSnapshot,
    ) {
        bounded_push(
            &mut self.measurements,
            self.config.checkpoint_capacity,
            measurement,
        );
        self.push_checkpoint(snapshot);
    }

    fn push_checkpoint(&mut self, snapshot: InertialFilterSnapshot) {
        bounded_push(
            &mut self.checkpoints,
            self.config.checkpoint_capacity,
            StoredCheckpoint {
                t_j2000_s: snapshot.state.nominal.t_j2000_s,
                snapshot,
            },
        );
    }

    pub(super) fn restore_to_snapshot(&mut self, snapshot: InertialFilterSnapshot) {
        let restored_epoch_j2000_s = snapshot.state.nominal.t_j2000_s;
        while self
            .imu_samples
            .back()
            .is_some_and(|stored| stored.sample.t_j2000_s > restored_epoch_j2000_s)
        {
            self.imu_samples.pop_back();
        }
        while self
            .checkpoints
            .back()
            .is_some_and(|checkpoint| checkpoint.t_j2000_s > restored_epoch_j2000_s)
        {
            self.checkpoints.pop_back();
        }
        while self
            .measurements
            .back()
            .is_some_and(|measurement| measurement.t_j2000_s > restored_epoch_j2000_s)
        {
            self.measurements.pop_back();
        }
        if let Some(checkpoint) = self.checkpoints.back_mut() {
            if checkpoint.t_j2000_s == restored_epoch_j2000_s {
                checkpoint.snapshot = snapshot;
                return;
            }
        }
        self.push_checkpoint(snapshot);
    }

    fn rebase_through_checkpoint(&self, checkpoint_epoch_j2000_s: f64) -> Self {
        let mut history = Self {
            config: self.config,
            imu_samples: self.imu_samples.clone(),
            checkpoints: VecDeque::new(),
            measurements: VecDeque::new(),
        };
        for checkpoint in &self.checkpoints {
            if checkpoint.t_j2000_s <= checkpoint_epoch_j2000_s {
                history.checkpoints.push_back(checkpoint.clone());
            }
        }
        for measurement in &self.measurements {
            if measurement.t_j2000_s <= checkpoint_epoch_j2000_s {
                history.measurements.push_back(measurement.clone());
            }
        }
        history
    }

    fn checkpoint_at_or_before(&self, t_j2000_s: f64) -> Option<&StoredCheckpoint> {
        self.checkpoints
            .iter()
            .rev()
            .find(|checkpoint| checkpoint.t_j2000_s <= t_j2000_s)
    }

    fn measurements_after(&self, t_j2000_s: f64) -> Vec<ReplayMeasurement> {
        self.measurements
            .iter()
            .enumerate()
            .filter(|(_, measurement)| measurement.t_j2000_s > t_j2000_s)
            .map(|(order, measurement)| ReplayMeasurement {
                measurement: measurement.clone(),
                order,
                is_new: false,
            })
            .collect()
    }

    fn sample_covering(&self, t_j2000_s: f64) -> Option<&StoredImuSample> {
        self.imu_samples.iter().find(|stored| {
            stored.previous_t_j2000_s <= t_j2000_s && t_j2000_s < stored.sample.t_j2000_s
        })
    }

    fn set_config(&mut self, config: TimeSyncHistoryConfig) {
        self.config = config;
        truncate_front(&mut self.imu_samples, config.imu_capacity);
        truncate_front(&mut self.checkpoints, config.checkpoint_capacity);
        truncate_front(&mut self.measurements, config.checkpoint_capacity);
    }

    fn status(&self) -> TimeSyncHistoryStatus {
        TimeSyncHistoryStatus {
            imu_capacity: self.config.imu_capacity,
            imu_len: self.imu_samples.len(),
            checkpoint_capacity: self.config.checkpoint_capacity,
            checkpoint_len: self.checkpoints.len(),
            oldest_imu_epoch_j2000_s: self
                .imu_samples
                .front()
                .map(|stored| stored.sample.t_j2000_s),
            newest_imu_epoch_j2000_s: self
                .imu_samples
                .back()
                .map(|stored| stored.sample.t_j2000_s),
            oldest_checkpoint_epoch_j2000_s: self
                .checkpoints
                .front()
                .map(|checkpoint| checkpoint.t_j2000_s),
            newest_checkpoint_epoch_j2000_s: self
                .checkpoints
                .back()
                .map(|checkpoint| checkpoint.t_j2000_s),
        }
    }
}

/// Validate strictly increasing IMU sample epochs.
pub fn validate_time_sync_imu_order(samples: &[ImuSample]) -> Result<(), FusionError> {
    let mut previous_t_j2000_s = None;
    for sample in samples {
        validate_epoch(sample.t_j2000_s, "imu_samples")?;
        if let Some(previous) = previous_t_j2000_s {
            if sample.t_j2000_s <= previous {
                return Err(invalid_input(
                    "imu_samples",
                    "must be strictly ordered by epoch",
                ));
            }
        }
        previous_t_j2000_s = Some(sample.t_j2000_s);
    }
    Ok(())
}

/// Validate strictly increasing GNSS measurement epochs.
pub fn validate_time_sync_gnss_order(
    measurements: &[GnssFixMeasurement],
) -> Result<(), FusionError> {
    let mut previous_t_j2000_s = None;
    for measurement in measurements {
        measurement.validate()?;
        if let Some(previous) = previous_t_j2000_s {
            if measurement.t_j2000_s <= previous {
                return Err(invalid_input(
                    "gnss_measurements",
                    "must be strictly ordered by epoch",
                ));
            }
        }
        previous_t_j2000_s = Some(measurement.t_j2000_s);
    }
    Ok(())
}

impl InertialFilter {
    /// Return a copy of the filter state needed for time-sync checkpoint replay.
    pub fn snapshot(&self) -> InertialFilterSnapshot {
        InertialFilterSnapshot {
            state: self.state.clone(),
            last_body_rate_wrt_ecef_rps: self.last_body_rate_wrt_ecef_rps,
        }
    }

    /// Restore the filter state from a snapshot.
    pub fn restore_snapshot(
        &mut self,
        snapshot: &InertialFilterSnapshot,
    ) -> Result<(), FusionError> {
        snapshot.state.validate()?;
        validate_vec3(
            snapshot.last_body_rate_wrt_ecef_rps,
            "last_body_rate_wrt_ecef_rps",
        )?;
        let restored = snapshot.clone();
        self.state = restored.state.clone();
        self.last_body_rate_wrt_ecef_rps = restored.last_body_rate_wrt_ecef_rps;
        self.time_sync.restore_to_snapshot(restored);
        Ok(())
    }

    /// Replace retained-history capacities for later time-sync replay.
    pub fn configure_time_sync_history(
        &mut self,
        config: TimeSyncHistoryConfig,
    ) -> Result<(), FusionError> {
        config.validate()?;
        self.time_sync.set_config(config);
        Ok(())
    }

    /// Return current retained-history capacity and occupancy.
    pub fn time_sync_history_status(&self) -> TimeSyncHistoryStatus {
        self.time_sync.status()
    }

    /// Apply a loose GNSS update at the measurement epoch, replaying history if needed.
    pub fn update_loose_time_sync(
        &mut self,
        measurement: &GnssFixMeasurement,
    ) -> Result<TimeSyncUpdate, FusionError> {
        measurement.validate()?;
        let target_t_j2000_s = measurement.t_j2000_s;
        let current_t_j2000_s = self.state.nominal.t_j2000_s;
        if target_t_j2000_s > current_t_j2000_s {
            return Err(invalid_input(
                "t_j2000_s",
                "must not exceed current inertial epoch",
            ));
        }

        if target_t_j2000_s == current_t_j2000_s {
            let update = self.update_loose(measurement)?;
            return Ok(TimeSyncUpdate {
                update,
                late_measurement: false,
                replayed_imu_segments: 0,
                restored_checkpoint_epoch_j2000_s: current_t_j2000_s,
                current_epoch_j2000_s: self.state.nominal.t_j2000_s,
            });
        }

        self.apply_late_loose_update(measurement, current_t_j2000_s)
    }

    fn apply_late_loose_update(
        &mut self,
        measurement: &GnssFixMeasurement,
        original_current_t_j2000_s: f64,
    ) -> Result<TimeSyncUpdate, FusionError> {
        let original_history = self.time_sync.clone();
        let checkpoint = original_history
            .checkpoint_at_or_before(measurement.t_j2000_s)
            .ok_or_else(|| invalid_input("t_j2000_s", "outside retained checkpoint history"))?
            .clone();
        let mut replay_measurements = original_history.measurements_after(checkpoint.t_j2000_s);
        if replay_measurements
            .iter()
            .any(|r| r.measurement.t_j2000_s == measurement.t_j2000_s)
        {
            return Err(invalid_input(
                "t_j2000_s",
                "duplicate GNSS measurement epoch in late replay",
            ));
        }
        let new_order = replay_measurements.len();
        replay_measurements.push(ReplayMeasurement {
            measurement: measurement.clone(),
            order: new_order,
            is_new: true,
        });
        replay_measurements.sort_by(|a, b| {
            a.measurement
                .t_j2000_s
                .total_cmp(&b.measurement.t_j2000_s)
                .then_with(|| a.order.cmp(&b.order))
                .then_with(|| a.is_new.cmp(&b.is_new))
        });

        let mut working = self.clone();
        working.restore_snapshot(&checkpoint.snapshot)?;
        working.time_sync = original_history.rebase_through_checkpoint(checkpoint.t_j2000_s);

        let mut replayed_imu_segments = 0usize;
        let mut supplied_update = None;
        for replay in replay_measurements {
            replayed_imu_segments +=
                working.replay_imu_to_epoch(replay.measurement.t_j2000_s, &original_history)?;
            let update = working.update_loose_core(&replay.measurement)?;
            let snapshot = working.snapshot();
            working
                .time_sync
                .push_measurement_and_checkpoint(replay.measurement.clone(), snapshot);
            if replay.is_new {
                supplied_update = Some(update);
            }
        }
        replayed_imu_segments +=
            working.replay_imu_to_epoch(original_current_t_j2000_s, &original_history)?;
        let update = supplied_update.ok_or_else(|| {
            invalid_input("gnss_measurements", "supplied measurement was not replayed")
        })?;
        let restored_checkpoint_epoch_j2000_s = checkpoint.t_j2000_s;
        let current_epoch_j2000_s = working.state.nominal.t_j2000_s;
        *self = working;
        Ok(TimeSyncUpdate {
            update,
            late_measurement: true,
            replayed_imu_segments,
            restored_checkpoint_epoch_j2000_s,
            current_epoch_j2000_s,
        })
    }

    fn replay_imu_to_epoch(
        &mut self,
        target_t_j2000_s: f64,
        source: &TimeSyncHistory,
    ) -> Result<usize, FusionError> {
        validate_epoch(target_t_j2000_s, "target_t_j2000_s")?;
        let mut segments = 0usize;
        loop {
            let current_t_j2000_s = self.state.nominal.t_j2000_s;
            if current_t_j2000_s == target_t_j2000_s {
                return Ok(segments);
            }
            if current_t_j2000_s > target_t_j2000_s {
                return Err(invalid_input(
                    "target_t_j2000_s",
                    "must not be older than the restored epoch",
                ));
            }
            let stored = source.sample_covering(current_t_j2000_s).ok_or_else(|| {
                invalid_input("imu_samples", "target epoch outside retained IMU history")
            })?;
            let segment_end_t_j2000_s = stored.sample.t_j2000_s.min(target_t_j2000_s);
            let sample = stored.segment_sample(current_t_j2000_s, segment_end_t_j2000_s)?;
            self.propagate_core(sample)?;
            segments += 1;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StoredImuSample {
    previous_t_j2000_s: f64,
    sample: ImuSample,
    previous_rate: Option<RateEndpoint>,
}

impl StoredImuSample {
    fn segment_sample(
        &self,
        segment_start_t_j2000_s: f64,
        segment_end_t_j2000_s: f64,
    ) -> Result<ImuSample, FusionError> {
        validate_epoch(segment_start_t_j2000_s, "segment_start_t_j2000_s")?;
        validate_epoch(segment_end_t_j2000_s, "segment_end_t_j2000_s")?;
        if segment_start_t_j2000_s < self.previous_t_j2000_s
            || segment_start_t_j2000_s >= segment_end_t_j2000_s
            || segment_end_t_j2000_s > self.sample.t_j2000_s
        {
            return Err(invalid_input(
                "imu_samples",
                "segment outside retained sample",
            ));
        }
        if segment_start_t_j2000_s == self.previous_t_j2000_s
            && segment_end_t_j2000_s == self.sample.t_j2000_s
        {
            return Ok(self.sample);
        }
        let dt_s = segment_end_t_j2000_s - segment_start_t_j2000_s;
        match self.sample.kind {
            ImuSampleKind::Rate {
                specific_force_mps2,
                angular_rate_rps,
            } => {
                let current = RateEndpoint {
                    t_j2000_s: self.sample.t_j2000_s,
                    specific_force_mps2,
                    angular_rate_rps,
                };
                let previous = self.previous_rate.ok_or_else(|| {
                    invalid_input("imu_samples", "fractional rate segment needs prior rate")
                })?;
                if previous.t_j2000_s >= current.t_j2000_s {
                    return Err(invalid_input(
                        "imu_samples",
                        "fractional rate segment needs ordered rate endpoints",
                    ));
                }
                let start = interpolate_rate(previous, current, segment_start_t_j2000_s);
                let end = interpolate_rate(previous, current, segment_end_t_j2000_s);
                Ok(ImuSample::rate(
                    segment_end_t_j2000_s,
                    scale3(
                        add3(start.specific_force_mps2, end.specific_force_mps2),
                        0.5,
                    ),
                    scale3(add3(start.angular_rate_rps, end.angular_rate_rps), 0.5),
                ))
            }
            ImuSampleKind::Increment {
                delta_velocity_mps,
                delta_theta_rad,
                dt_s: sample_dt_s,
            } => {
                validate_positive(sample_dt_s, "dt_s")?;
                let sample_interval_s = self.sample.t_j2000_s - self.previous_t_j2000_s;
                validate_positive(sample_interval_s, "imu_samples")?;
                let fraction = dt_s / sample_interval_s;
                Ok(ImuSample::increment(
                    segment_end_t_j2000_s,
                    scale3(delta_velocity_mps, fraction),
                    scale3(delta_theta_rad, fraction),
                    dt_s,
                ))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct StoredCheckpoint {
    t_j2000_s: f64,
    snapshot: InertialFilterSnapshot,
}

#[derive(Debug, Clone, PartialEq)]
struct ReplayMeasurement {
    measurement: GnssFixMeasurement,
    order: usize,
    is_new: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RateEndpoint {
    t_j2000_s: f64,
    specific_force_mps2: [f64; 3],
    angular_rate_rps: [f64; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RatePayload {
    specific_force_mps2: [f64; 3],
    angular_rate_rps: [f64; 3],
}

fn rate_payload(sample: ImuSample) -> Option<RatePayload> {
    match sample.kind {
        ImuSampleKind::Rate {
            specific_force_mps2,
            angular_rate_rps,
        } => Some(RatePayload {
            specific_force_mps2,
            angular_rate_rps,
        }),
        ImuSampleKind::Increment { .. } => None,
    }
}

fn interpolate_rate(start: RateEndpoint, end: RateEndpoint, t_j2000_s: f64) -> RateEndpoint {
    let alpha = (t_j2000_s - start.t_j2000_s) / (end.t_j2000_s - start.t_j2000_s);
    RateEndpoint {
        t_j2000_s,
        specific_force_mps2: add3(
            start.specific_force_mps2,
            scale3(
                sub3(end.specific_force_mps2, start.specific_force_mps2),
                alpha,
            ),
        ),
        angular_rate_rps: add3(
            start.angular_rate_rps,
            scale3(sub3(end.angular_rate_rps, start.angular_rate_rps), alpha),
        ),
    }
}

fn bounded_push<T>(items: &mut VecDeque<T>, capacity: usize, item: T) {
    if items.len() == capacity {
        items.pop_front();
    }
    items.push_back(item);
}

fn truncate_front<T>(items: &mut VecDeque<T>, capacity: usize) {
    while items.len() > capacity {
        items.pop_front();
    }
}

fn validate_capacity(capacity: usize, field: &'static str) -> Result<(), FusionError> {
    if capacity == 0 {
        Err(invalid_input(field, "must be positive"))
    } else {
        Ok(())
    }
}

fn validate_epoch(value: f64, field: &'static str) -> Result<(), FusionError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(invalid_input(field, "must be finite"))
    }
}

fn validate_vec3(value: [f64; 3], field: &'static str) -> Result<(), FusionError> {
    for component in value {
        validate_epoch(component, field)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Provenance: time synchronization follows the Kalman implementation issue
    //! described by Groves, Principles of GNSS, Inertial, and Multisensor
    //! Integrated Navigation Systems, 2nd ed., Section 3.3.4. The fractional-increment
    //! tests use synthetic constant integral IMU samples, so the oracle is the
    //! same ordered sequence of split increments at the GNSS epoch. Late-update
    //! parity compares the replayed path against direct in-order processing
    //! with bit equality.

    use super::*;
    use crate::astro::constants::earth::{OMEGA_E_DOT_RAD_S, WGS84_A_M};
    use crate::fusion::state::{
        ErrorStateLayout, ERROR_POSITION_INDEX, ERROR_STATE_DIMENSION_15, ERROR_VELOCITY_INDEX,
    };
    use crate::inertial::config::RANDOM_WALK_BIAS_TAU_S;
    use crate::inertial::state::mat3_identity;
    use crate::inertial::{ImuSpec, NavState};
    use nalgebra::DMatrix;

    fn filter_at(t_j2000_s: f64) -> InertialFilter {
        let nominal = NavState::new(
            t_j2000_s,
            [WGS84_A_M, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            mat3_identity(),
        )
        .expect("nominal");
        let diagonal = vec![1.0; ERROR_STATE_DIMENSION_15];
        let state = InsFilterState::from_diagonal(nominal, ErrorStateLayout::Fifteen, &diagonal)
            .expect("state");
        let spec = ImuSpec::datasheet(
            0.0,
            0.0,
            0.0,
            0.0,
            RANDOM_WALK_BIAS_TAU_S,
            RANDOM_WALK_BIAS_TAU_S,
            None,
            None,
        );
        InertialFilter::new(state, spec).expect("filter")
    }

    fn noisy_filter_at(t_j2000_s: f64) -> InertialFilter {
        let nominal = NavState::new(
            t_j2000_s,
            [WGS84_A_M, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            mat3_identity(),
        )
        .expect("nominal");
        let diagonal = vec![1.0e-6; ERROR_STATE_DIMENSION_15];
        let state = InsFilterState::from_diagonal(nominal, ErrorStateLayout::Fifteen, &diagonal)
            .expect("state");
        let spec = ImuSpec::datasheet(0.02, 0.001, 0.004, 2.0e-4, 300.0, 300.0, None, None);
        InertialFilter::new(state, spec).expect("filter")
    }

    fn increment(t_j2000_s: f64, dt_s: f64) -> ImuSample {
        ImuSample::increment(
            t_j2000_s,
            [0.015625 * dt_s, -0.0078125 * dt_s, 0.00390625 * dt_s],
            [
                OMEGA_E_DOT_RAD_S * dt_s,
                0.0009765625 * dt_s,
                -0.00048828125 * dt_s,
            ],
            dt_s,
        )
    }

    fn measurement_at(t_j2000_s: f64, position_ecef_m: [f64; 3]) -> GnssFixMeasurement {
        GnssFixMeasurement::position(
            t_j2000_s,
            position_ecef_m,
            [[4.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 4.0]],
            8,
        )
        .expect("measurement")
    }

    fn logdet_spd(covariance: &[Vec<f64>]) -> f64 {
        let dimension = covariance.len();
        let mut data = Vec::<f64>::with_capacity(dimension * dimension);
        for row in covariance {
            data.extend(row.iter().copied());
        }
        let matrix = DMatrix::from_row_slice(dimension, dimension, &data);
        let cholesky = matrix.cholesky().expect("covariance SPD");
        2.0 * cholesky
            .l()
            .diagonal()
            .iter()
            .map(|value| value.ln())
            .sum::<f64>()
    }

    #[test]
    fn split_increment_substep_matches_explicit_epoch_split_bits() {
        let mut split = filter_at(0.0);
        split
            .configure_time_sync_history(TimeSyncHistoryConfig {
                imu_capacity: 4,
                checkpoint_capacity: 4,
            })
            .expect("history");
        split.propagate(increment(1.0, 1.0)).expect("propagate");
        let mut explicit = filter_at(0.0);
        explicit
            .configure_time_sync_history(TimeSyncHistoryConfig {
                imu_capacity: 4,
                checkpoint_capacity: 4,
            })
            .expect("history");
        explicit
            .propagate(increment(0.75, 0.75))
            .expect("first split");
        let measurement = measurement_at(0.75, [WGS84_A_M + 0.125, -0.0625, 0.03125]);
        explicit.update_loose(&measurement).expect("direct update");
        explicit
            .propagate(increment(1.0, 0.25))
            .expect("second split");

        let update = split
            .update_loose_time_sync(&measurement)
            .expect("time-sync update");
        assert!(update.late_measurement);
        assert_eq!(update.replayed_imu_segments, 2);
        assert_filter_bits(split.state(), explicit.state());
    }

    #[test]
    fn late_measurement_replay_matches_in_order_bits() {
        let mut in_order = filter_at(0.0);
        in_order.propagate(increment(0.5, 0.5)).expect("imu");
        let first = measurement_at(0.5, [WGS84_A_M + 0.25, 0.0, 0.0]);
        in_order.update_loose(&first).expect("first update");
        in_order.propagate(increment(1.0, 0.5)).expect("imu");
        let late = measurement_at(1.0, [WGS84_A_M - 0.125, 0.0625, 0.0]);
        in_order.update_loose(&late).expect("late in order");
        in_order.propagate(increment(1.5, 0.5)).expect("imu");
        let final_fix = measurement_at(1.5, [WGS84_A_M, 0.0, 0.03125]);
        in_order.update_loose(&final_fix).expect("final update");

        let mut replay = filter_at(0.0);
        replay.propagate(increment(0.5, 0.5)).expect("imu");
        replay.update_loose(&first).expect("first update");
        replay.propagate(increment(1.0, 0.5)).expect("imu");
        replay.propagate(increment(1.5, 0.5)).expect("imu");
        replay.update_loose(&final_fix).expect("final update");
        let update = replay
            .update_loose_time_sync(&late)
            .expect("late update replay");

        assert!(update.late_measurement);
        assert_eq!(
            update.restored_checkpoint_epoch_j2000_s.to_bits(),
            0.5f64.to_bits()
        );
        assert_filter_bits(replay.state(), in_order.state());
    }

    #[test]
    fn coasting_covariance_logdet_grows_monotonically() {
        let mut filter = noisy_filter_at(0.0);
        let mut previous_logdet = logdet_spd(&filter.state().covariance);
        for step in 1..=6 {
            filter
                .propagate(increment(step as f64 * 0.25, 0.25))
                .expect("coast");
            let logdet = logdet_spd(&filter.state().covariance);
            assert!(
                logdet > previous_logdet,
                "step {step} logdet {logdet:.17e}, previous {previous_logdet:.17e}"
            );
            previous_logdet = logdet;
        }
    }

    #[test]
    fn ring_buffer_wraparound_retains_exact_tail_epochs() {
        let mut filter = filter_at(0.0);
        filter
            .configure_time_sync_history(TimeSyncHistoryConfig::new(3, 2))
            .expect("history");
        for step in 1..=5 {
            filter
                .propagate(increment(step as f64 * 0.25, 0.25))
                .expect("imu");
        }
        let status = filter.time_sync_history_status();
        assert_eq!(status.imu_len, 3);
        assert_eq!(
            status.oldest_imu_epoch_j2000_s.map(f64::to_bits),
            Some(0.75f64.to_bits())
        );
        assert_eq!(
            status.newest_imu_epoch_j2000_s.map(f64::to_bits),
            Some(1.25f64.to_bits())
        );

        let first = GnssFixMeasurement::position(
            1.25,
            filter.state().nominal.position_ecef_m,
            [[4.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 4.0]],
            8,
        )
        .expect("first");
        filter.update_loose(&first).expect("first update");
        filter
            .propagate(increment(1.5, 0.25))
            .expect("additional imu");
        let second = GnssFixMeasurement::position(
            1.5,
            filter.state().nominal.position_ecef_m,
            [[4.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 4.0]],
            8,
        )
        .expect("second");
        filter.update_loose(&second).expect("second update");
        let status = filter.time_sync_history_status();
        assert_eq!(status.checkpoint_len, 2);
        assert_eq!(
            status.oldest_checkpoint_epoch_j2000_s.map(f64::to_bits),
            Some(1.25f64.to_bits())
        );
        assert_eq!(
            status.newest_checkpoint_epoch_j2000_s.map(f64::to_bits),
            Some(1.5f64.to_bits())
        );
    }

    #[test]
    fn restore_snapshot_trims_future_history_bits() {
        let mut filter = filter_at(0.0);
        filter
            .configure_time_sync_history(TimeSyncHistoryConfig::new(4, 4))
            .expect("history");
        filter.propagate(increment(1.0, 1.0)).expect("first");
        let snapshot = filter.snapshot();
        filter.propagate(increment(2.0, 1.0)).expect("second");
        assert_eq!(
            filter
                .time_sync_history_status()
                .newest_imu_epoch_j2000_s
                .map(f64::to_bits),
            Some(2.0f64.to_bits())
        );

        filter.restore_snapshot(&snapshot).expect("restore");
        assert_eq!(
            filter
                .time_sync_history_status()
                .newest_imu_epoch_j2000_s
                .map(f64::to_bits),
            Some(1.0f64.to_bits())
        );
        filter
            .propagate(increment(1.5, 0.5))
            .expect("after restore");
        let status = filter.time_sync_history_status();
        assert_eq!(
            status.newest_imu_epoch_j2000_s.map(f64::to_bits),
            Some(1.5f64.to_bits())
        );
        assert_eq!(filter.state().nominal.t_j2000_s.to_bits(), 1.5f64.to_bits());
    }

    #[test]
    fn validators_reject_unordered_epochs() {
        let samples = [increment(1.0, 1.0), increment(0.5, 0.5)];
        assert!(matches!(
            validate_time_sync_imu_order(&samples),
            Err(FusionError::InvalidInput {
                field: "imu_samples",
                reason: "must be strictly ordered by epoch"
            })
        ));

        let first = GnssFixMeasurement::position(
            2.0,
            [WGS84_A_M, 0.0, 0.0],
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            8,
        )
        .expect("first");
        let second = GnssFixMeasurement::position(
            1.0,
            [WGS84_A_M, 0.0, 0.0],
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            8,
        )
        .expect("second");
        assert!(matches!(
            validate_time_sync_gnss_order(&[first, second]),
            Err(FusionError::InvalidInput {
                field: "gnss_measurements",
                reason: "must be strictly ordered by epoch"
            })
        ));
    }

    #[test]
    fn validators_reject_equal_gnss_epochs() {
        let first = GnssFixMeasurement::position(
            2.0,
            [WGS84_A_M, 0.0, 0.0],
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            8,
        )
        .expect("first");
        let second = GnssFixMeasurement::position(
            2.0,
            [WGS84_A_M, 1.0, 0.0],
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            8,
        )
        .expect("second");
        assert!(matches!(
            validate_time_sync_gnss_order(&[first, second]),
            Err(FusionError::InvalidInput {
                field: "gnss_measurements",
                reason: "must be strictly ordered by epoch"
            })
        ));
    }

    #[test]
    fn fractional_rate_sample_uses_linear_average_oracle() {
        let stored = StoredImuSample {
            previous_t_j2000_s: 1.0,
            sample: ImuSample::rate(2.0, [3.0, 5.0, 7.0], [11.0, 13.0, 17.0]),
            previous_rate: Some(RateEndpoint {
                t_j2000_s: 1.0,
                specific_force_mps2: [1.0, 2.0, 4.0],
                angular_rate_rps: [6.0, 8.0, 10.0],
            }),
        };
        let segment = stored.segment_sample(1.25, 1.75).expect("segment");
        match segment.kind {
            ImuSampleKind::Rate {
                specific_force_mps2,
                angular_rate_rps,
            } => {
                assert_eq!(specific_force_mps2[0].to_bits(), 2.0f64.to_bits());
                assert_eq!(specific_force_mps2[1].to_bits(), 3.5f64.to_bits());
                assert_eq!(specific_force_mps2[2].to_bits(), 5.5f64.to_bits());
                assert_eq!(angular_rate_rps[0].to_bits(), 8.5f64.to_bits());
                assert_eq!(angular_rate_rps[1].to_bits(), 10.5f64.to_bits());
                assert_eq!(angular_rate_rps[2].to_bits(), 13.5f64.to_bits());
            }
            ImuSampleKind::Increment { .. } => panic!("rate segment expected"),
        }
    }

    #[test]
    fn fractional_rate_without_prior_endpoint_is_rejected() {
        let stored = StoredImuSample {
            previous_t_j2000_s: 1.0,
            sample: ImuSample::rate(2.0, [3.0, 5.0, 7.0], [11.0, 13.0, 17.0]),
            previous_rate: None,
        };
        assert!(matches!(
            stored.segment_sample(1.25, 1.75),
            Err(FusionError::InvalidInput {
                field: "imu_samples",
                reason: "fractional rate segment needs prior rate"
            })
        ));
        let full = stored.segment_sample(1.0, 2.0).expect("full segment");
        assert_eq!(full, stored.sample);
    }

    #[test]
    fn fractional_increment_sample_splits_integral_bits() {
        let stored = StoredImuSample {
            previous_t_j2000_s: 10.0,
            sample: ImuSample::increment(14.0, [8.0, -4.0, 2.0], [1.0, -0.5, 0.25], 3.999999999999),
            previous_rate: None,
        };
        let segment = stored.segment_sample(11.0, 12.0).expect("segment");
        match segment.kind {
            ImuSampleKind::Increment {
                delta_velocity_mps,
                delta_theta_rad,
                dt_s,
            } => {
                assert_eq!(segment.t_j2000_s.to_bits(), 12.0f64.to_bits());
                assert_eq!(dt_s.to_bits(), 1.0f64.to_bits());
                assert_eq!(delta_velocity_mps[0].to_bits(), 2.0f64.to_bits());
                assert_eq!(delta_velocity_mps[1].to_bits(), (-1.0f64).to_bits());
                assert_eq!(delta_velocity_mps[2].to_bits(), 0.5f64.to_bits());
                assert_eq!(delta_theta_rad[0].to_bits(), 0.25f64.to_bits());
                assert_eq!(delta_theta_rad[1].to_bits(), (-0.125f64).to_bits());
                assert_eq!(delta_theta_rad[2].to_bits(), 0.0625f64.to_bits());
            }
            ImuSampleKind::Rate { .. } => panic!("increment segment expected"),
        }
    }

    fn assert_filter_bits(actual: &InsFilterState, expected: &InsFilterState) {
        assert_eq!(
            actual.nominal.t_j2000_s.to_bits(),
            expected.nominal.t_j2000_s.to_bits()
        );
        assert_vec_bits(
            actual.nominal.position_ecef_m,
            expected.nominal.position_ecef_m,
        );
        assert_vec_bits(
            actual.nominal.velocity_ecef_mps,
            expected.nominal.velocity_ecef_mps,
        );
        for row in 0..3 {
            assert_vec_bits(
                actual.nominal.attitude_body_to_ecef[row],
                expected.nominal.attitude_body_to_ecef[row],
            );
        }
        for row in 0..actual.covariance.len() {
            for col in 0..actual.covariance[row].len() {
                assert_eq!(
                    actual.covariance[row][col].to_bits(),
                    expected.covariance[row][col].to_bits(),
                    "covariance {row},{col}"
                );
            }
        }
        for axis in 0..3 {
            assert_eq!(
                actual.nominal.accel_bias_mps2[axis].to_bits(),
                expected.nominal.accel_bias_mps2[axis].to_bits()
            );
            assert_eq!(
                actual.nominal.gyro_bias_rps[axis].to_bits(),
                expected.nominal.gyro_bias_rps[axis].to_bits()
            );
            assert_eq!(
                actual.accel_scale_factor[axis].to_bits(),
                expected.accel_scale_factor[axis].to_bits()
            );
            assert_eq!(
                actual.gyro_scale_factor[axis].to_bits(),
                expected.gyro_scale_factor[axis].to_bits()
            );
        }
    }

    fn assert_vec_bits(actual: [f64; 3], expected: [f64; 3]) {
        for axis in 0..3 {
            assert_eq!(
                actual[axis].to_bits(),
                expected[axis].to_bits(),
                "axis {axis}"
            );
        }
    }

    #[test]
    fn measurement_update_moves_only_position_states_in_test_setup() {
        let mut filter = filter_at(0.0);
        let measurement = GnssFixMeasurement::position(
            0.0,
            [WGS84_A_M + 1.0, -2.0, 3.0],
            [[4.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 4.0]],
            8,
        )
        .expect("measurement");
        let update = filter.update_loose_time_sync(&measurement).expect("update");
        assert!(update.update.applied);
        for axis in 0..3 {
            assert!(
                filter.state().covariance[ERROR_POSITION_INDEX + axis][ERROR_POSITION_INDEX + axis]
                    < 1.0
            );
            assert_eq!(
                filter.state().covariance[ERROR_VELOCITY_INDEX + axis][ERROR_VELOCITY_INDEX + axis]
                    .to_bits(),
                1.0f64.to_bits()
            );
        }
    }

    #[test]
    fn duplicate_gnss_epochs_are_rejected_on_stateful_paths() {
        // Regression: the stateful surface must enforce the same strict
        // ordering as the standalone time-sync validator.
        let mut filter = filter_at(0.0);
        filter.propagate(increment(1.0, 1.0)).expect("propagate");
        let fix = measurement_at(1.0, [WGS84_A_M + 0.125, 0.0, 0.0]);
        filter.update_loose(&fix).expect("first update");
        assert!(
            filter.update_loose(&fix).is_err(),
            "duplicate epoch must be rejected"
        );
        let regressed = measurement_at(0.5, [WGS84_A_M + 0.125, 0.0, 0.0]);
        assert!(
            filter.update_loose(&regressed).is_err(),
            "regressed epoch must be rejected"
        );
    }
}
