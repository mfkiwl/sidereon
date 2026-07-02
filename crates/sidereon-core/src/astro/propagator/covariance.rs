//! Covariance transport along numerical propagation segments.

use crate::astro::covariance::{
    eci_to_rtn_covariance6, finite6, rtn_to_eci_covariance6, symmetrize6, Covariance6,
    RtnFrameError,
};
use crate::astro::error::PropagationError;
use crate::astro::propagator::api::PropagationContext;
use crate::astro::propagator::dynamics::OrbitalDynamics;
use crate::astro::propagator::numerical::{map_covariance6_error, StatePropagator};
use crate::astro::propagator::StateTransitionMatrix;
use crate::astro::state::CartesianState;

/// Frame a 6x6 state covariance is expressed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CovarianceFrame {
    /// The propagator's inertial frame.
    Inertial,
    /// Satellite-relative radial, transverse, normal axes.
    Rtn,
}

/// A covariance plus the frame label it is expressed in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabeledCovariance6 {
    pub covariance: Covariance6,
    pub frame: CovarianceFrame,
}

/// Process-noise model for covariance transport.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum ProcessNoise {
    /// Pure Phi P Phi^T transport.
    #[default]
    None,
    /// Per-axis white acceleration PSD in RTN, km^2/s^3.
    RtnAccelerationPsd {
        q_radial_km2_s3: f64,
        q_transverse_km2_s3: f64,
        q_normal_km2_s3: f64,
    },
}

/// Options for a covariance propagation run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CovariancePropagationOptions {
    pub process_noise: ProcessNoise,
    pub output_frame: CovarianceFrame,
}

impl Default for CovariancePropagationOptions {
    fn default() -> Self {
        Self {
            process_noise: ProcessNoise::None,
            output_frame: CovarianceFrame::Inertial,
        }
    }
}

/// State plus covariance at one requested epoch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CovarianceNode {
    /// The propagated Cartesian state, always in the inertial frame.
    pub state: CartesianState,
    /// The covariance expressed in `frame`.
    pub covariance: Covariance6,
    /// Frame used by `covariance`.
    pub frame: CovarianceFrame,
}

/// Ordered result of a covariance propagation run.
#[derive(Debug, Clone, PartialEq)]
pub struct CovarianceEphemeris {
    nodes: Vec<CovarianceNode>,
}

impl CovarianceEphemeris {
    /// Borrow the propagated nodes in request order.
    pub fn nodes(&self) -> &[CovarianceNode] {
        &self.nodes
    }

    /// Number of propagated nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether this result contains no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// One caller-supplied covariance transport segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CovarianceSegment {
    /// Segment state-transition matrix.
    pub stm: StateTransitionMatrix,
    /// Segment duration in seconds. Process noise uses the absolute value.
    pub dt_seconds: f64,
    /// State whose RTN axes rotate segment process noise into inertial axes.
    pub q_rotation_state: CartesianState,
}

impl StatePropagator {
    /// Propagate the initial state and covariance to each requested epoch.
    ///
    /// A RTN-labeled input is first rotated to inertial axes at the initial
    /// state. Epochs must be monotonic from the initial epoch, with repeated
    /// epochs allowed. The output covariance is expressed in the requested
    /// frame at each node.
    pub fn propagate_covariance(
        &self,
        initial: LabeledCovariance6,
        epochs_tdb_seconds: &[f64],
        options: &CovariancePropagationOptions,
    ) -> Result<CovarianceEphemeris, PropagationError> {
        validate_initial_state(self.initial)?;
        validate_epochs_monotonic(self.initial.epoch_tdb_seconds, epochs_tdb_seconds)?;
        validate_process_noise(options.process_noise)?;

        let force = self.build_force()?;
        let dynamics = OrbitalDynamics {
            force_model: force.as_ref(),
        };
        let ctx = PropagationContext::default();

        let mut current_state = self.initial;
        let mut current_covariance = match initial.frame {
            CovarianceFrame::Inertial => initial.covariance,
            CovarianceFrame::Rtn => rtn_to_eci_covariance6(&initial.covariance, &self.initial)
                .map_err(map_rtn_frame_error)?,
        };
        let mut nodes = Vec::with_capacity(epochs_tdb_seconds.len());

        for &epoch in epochs_tdb_seconds {
            if epoch != current_state.epoch_tdb_seconds {
                let dt = epoch - current_state.epoch_tdb_seconds;
                let q_rotation_state =
                    segment_q_rotation_state(self, current_state, epoch, options, &dynamics, &ctx)?;
                let stm =
                    self.state_transition_matrix_between(current_state, epoch, &dynamics, &ctx)?;
                let final_state = self.run(current_state, epoch, &dynamics, &ctx)?.final_state;
                let segments = [CovarianceSegment {
                    stm,
                    dt_seconds: dt,
                    q_rotation_state,
                }];
                let transported =
                    transport_covariance(current_covariance, &segments, options.process_noise)?;
                current_covariance = transported[1];
                current_state = final_state;
            }

            nodes.push(CovarianceNode {
                state: current_state,
                covariance: express_output_covariance(
                    current_covariance,
                    current_state,
                    options.output_frame,
                )?,
                frame: options.output_frame,
            });
        }

        Ok(CovarianceEphemeris { nodes })
    }
}

/// Apply covariance transport over caller-supplied segments.
///
/// The returned vector contains the initial covariance followed by one
/// covariance per segment.
pub fn transport_covariance(
    covariance0: Covariance6,
    segments: &[CovarianceSegment],
    process_noise: ProcessNoise,
) -> Result<Vec<Covariance6>, PropagationError> {
    validate_process_noise(process_noise)?;
    let mut covariances = Vec::with_capacity(segments.len() + 1);
    let mut current = covariance0;
    covariances.push(current);

    for segment in segments {
        if !finite6(&segment.stm) {
            return Err(PropagationError::NumericalFailure(
                "state_transition_matrix not finite".to_string(),
            ));
        }
        crate::validate::finite(segment.dt_seconds, "dt_seconds").map_err(|error| {
            PropagationError::InvalidInput(format!("{} {}", error.field(), error.reason()))
        })?;

        let mut matrix = current
            .propagate_with_stm(&segment.stm)
            .map_err(map_covariance6_error)?
            .into_matrix();
        if let Some(q) = process_noise_increment(segment, process_noise)? {
            let q_matrix = q.as_matrix();
            for (i, row) in matrix.iter_mut().enumerate() {
                for (j, cell) in row.iter_mut().enumerate() {
                    *cell += q_matrix[i][j];
                }
            }
            symmetrize6(&mut matrix);
        }
        current = Covariance6::try_from_matrix(matrix).map_err(map_covariance6_error)?;
        covariances.push(current);
    }

    Ok(covariances)
}

fn segment_q_rotation_state(
    propagator: &StatePropagator,
    current_state: CartesianState,
    epoch: f64,
    options: &CovariancePropagationOptions,
    dynamics: &OrbitalDynamics,
    ctx: &PropagationContext,
) -> Result<CartesianState, PropagationError> {
    if matches!(options.process_noise, ProcessNoise::None) {
        return Ok(current_state);
    }
    let midpoint = 0.5 * (current_state.epoch_tdb_seconds + epoch);
    if midpoint == current_state.epoch_tdb_seconds {
        Ok(current_state)
    } else {
        Ok(propagator
            .run(current_state, midpoint, dynamics, ctx)?
            .final_state)
    }
}

fn express_output_covariance(
    covariance: Covariance6,
    state: CartesianState,
    frame: CovarianceFrame,
) -> Result<Covariance6, PropagationError> {
    match frame {
        CovarianceFrame::Inertial => Ok(covariance),
        CovarianceFrame::Rtn => {
            eci_to_rtn_covariance6(&covariance, &state).map_err(map_rtn_frame_error)
        }
    }
}

fn process_noise_increment(
    segment: &CovarianceSegment,
    process_noise: ProcessNoise,
) -> Result<Option<Covariance6>, PropagationError> {
    let ProcessNoise::RtnAccelerationPsd {
        q_radial_km2_s3,
        q_transverse_km2_s3,
        q_normal_km2_s3,
    } = process_noise
    else {
        return Ok(None);
    };

    let dt = segment.dt_seconds.abs();
    let dt2 = dt * dt;
    let dt3 = dt2 * dt;
    let qs = [q_radial_km2_s3, q_transverse_km2_s3, q_normal_km2_s3];
    let mut matrix = [[0.0_f64; 6]; 6];
    for axis in 0..3 {
        matrix[axis][axis] = qs[axis] * dt3 / 3.0;
        matrix[axis][axis + 3] = qs[axis] * dt2 / 2.0;
        matrix[axis + 3][axis] = qs[axis] * dt2 / 2.0;
        matrix[axis + 3][axis + 3] = qs[axis] * dt;
    }
    let q_rtn = Covariance6::try_from_matrix(matrix).map_err(map_covariance6_error)?;
    rtn_to_eci_covariance6(&q_rtn, &segment.q_rotation_state)
        .map(Some)
        .map_err(map_rtn_frame_error)
}

fn validate_epochs_monotonic(
    initial_epoch: f64,
    epochs_tdb_seconds: &[f64],
) -> Result<(), PropagationError> {
    crate::validate::finite(initial_epoch, "initial.epoch_tdb_seconds").map_err(|error| {
        PropagationError::InvalidInput(format!("{} {}", error.field(), error.reason()))
    })?;
    let mut current = initial_epoch;
    let mut direction = 0.0_f64;
    for &epoch in epochs_tdb_seconds {
        crate::validate::finite(epoch, "epochs_tdb_seconds").map_err(|error| {
            PropagationError::InvalidInput(format!("{} {}", error.field(), error.reason()))
        })?;
        let dt = epoch - current;
        if dt != 0.0 {
            let sign = dt.signum();
            if direction == 0.0 {
                direction = sign;
            } else if sign != direction {
                return Err(PropagationError::InvalidInput(
                    "epochs_tdb_seconds direction reversal".to_string(),
                ));
            }
        }
        current = epoch;
    }
    Ok(())
}

fn validate_initial_state(initial: CartesianState) -> Result<(), PropagationError> {
    validate_state_vector(initial.position_array(), "initial.position_km")?;
    validate_state_vector(initial.velocity_array(), "initial.velocity_km_s")
}

fn validate_state_vector(values: [f64; 3], field: &'static str) -> Result<(), PropagationError> {
    crate::validate::finite_slice(&values, field).map_err(|error| {
        PropagationError::InvalidInput(format!("{} {}", error.field(), error.reason()))
    })
}

fn validate_process_noise(process_noise: ProcessNoise) -> Result<(), PropagationError> {
    let ProcessNoise::RtnAccelerationPsd {
        q_radial_km2_s3,
        q_transverse_km2_s3,
        q_normal_km2_s3,
    } = process_noise
    else {
        return Ok(());
    };

    validate_q("q_radial_km2_s3", q_radial_km2_s3)?;
    validate_q("q_transverse_km2_s3", q_transverse_km2_s3)?;
    validate_q("q_normal_km2_s3", q_normal_km2_s3)
}

fn validate_q(field: &'static str, value: f64) -> Result<(), PropagationError> {
    if !value.is_finite() {
        return Err(PropagationError::InvalidInput(format!(
            "{field} not finite"
        )));
    }
    if value < 0.0 {
        return Err(PropagationError::InvalidInput(format!("{field} negative")));
    }
    Ok(())
}

fn map_rtn_frame_error(error: RtnFrameError) -> PropagationError {
    PropagationError::InvalidInput(format!("covariance frame {}", error.message()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::astro::constants::MU_EARTH;
    use crate::astro::propagator::{ForceModelKind, IntegratorKind, IntegratorOptions};

    fn circular_propagator() -> StatePropagator {
        let r: f64 = 7000.0;
        let v = (MU_EARTH / r).sqrt();
        StatePropagator::new(
            0.0,
            [r, 0.0, 0.0],
            [0.0, v, 0.0],
            ForceModelKind::two_body(),
            IntegratorKind::Rk4,
        )
        .with_options(IntegratorOptions {
            initial_step: 2.0,
            ..IntegratorOptions::default()
        })
    }

    fn test_covariance() -> Covariance6 {
        Covariance6::from_diagonal([1.0e-4, 2.0e-4, 3.0e-4, 1.0e-8, 2.0e-8, 3.0e-8]).unwrap()
    }

    #[test]
    fn multi_epoch_no_noise_matches_segment_chaining() {
        let propagator = circular_propagator();
        let covariance0 = test_covariance();
        let epochs = [0.0, 60.0, 120.0];

        let ephemeris = propagator
            .propagate_covariance(
                LabeledCovariance6 {
                    covariance: covariance0,
                    frame: CovarianceFrame::Inertial,
                },
                &epochs,
                &CovariancePropagationOptions::default(),
            )
            .expect("covariance ephemeris");

        assert_eq!(ephemeris.len(), 3);
        assert_eq!(ephemeris.nodes()[0].covariance, covariance0);
        assert_eq!(ephemeris.nodes()[2].state.epoch_tdb_seconds, 120.0);
        assert!(ephemeris.nodes()[2].covariance.is_positive_semidefinite());

        let (_, single_segment) = propagator
            .propagate_state_with_covariance(covariance0, 60.0)
            .expect("single segment covariance");
        assert_eq!(ephemeris.nodes()[1].covariance, single_segment);
    }

    #[test]
    fn rtn_output_is_labeled_and_round_trips_to_inertial() {
        let propagator = circular_propagator();
        let covariance0 = test_covariance();
        let epochs = [120.0];

        let inertial = propagator
            .propagate_covariance(
                LabeledCovariance6 {
                    covariance: covariance0,
                    frame: CovarianceFrame::Inertial,
                },
                &epochs,
                &CovariancePropagationOptions::default(),
            )
            .expect("inertial covariance ephemeris");
        let rtn = propagator
            .propagate_covariance(
                LabeledCovariance6 {
                    covariance: covariance0,
                    frame: CovarianceFrame::Inertial,
                },
                &epochs,
                &CovariancePropagationOptions {
                    output_frame: CovarianceFrame::Rtn,
                    ..CovariancePropagationOptions::default()
                },
            )
            .expect("RTN covariance ephemeris");

        assert_eq!(rtn.nodes()[0].frame, CovarianceFrame::Rtn);
        let round_trip =
            rtn_to_eci_covariance6(&rtn.nodes()[0].covariance, &rtn.nodes()[0].state).unwrap();
        for i in 0..6 {
            for j in 0..6 {
                let expected = inertial.nodes()[0].covariance.as_matrix()[i][j];
                let actual = round_trip.as_matrix()[i][j];
                assert!((actual - expected).abs() <= 1.0e-12 * expected.abs().max(1.0));
            }
        }
    }

    #[test]
    fn process_noise_increases_velocity_variance() {
        let propagator = circular_propagator();
        let covariance0 = test_covariance();
        let epochs = [60.0, 120.0];
        let options = CovariancePropagationOptions {
            process_noise: ProcessNoise::RtnAccelerationPsd {
                q_radial_km2_s3: 1.0e-12,
                q_transverse_km2_s3: 2.0e-12,
                q_normal_km2_s3: 3.0e-12,
            },
            output_frame: CovarianceFrame::Inertial,
        };

        let no_noise = propagator
            .propagate_covariance(
                LabeledCovariance6 {
                    covariance: covariance0,
                    frame: CovarianceFrame::Inertial,
                },
                &epochs,
                &CovariancePropagationOptions::default(),
            )
            .expect("no-noise covariance ephemeris");
        let with_noise = propagator
            .propagate_covariance(
                LabeledCovariance6 {
                    covariance: covariance0,
                    frame: CovarianceFrame::Inertial,
                },
                &epochs,
                &options,
            )
            .expect("noise covariance ephemeris");

        let no_noise_trace = velocity_trace(no_noise.nodes()[1].covariance);
        let with_noise_trace = velocity_trace(with_noise.nodes()[1].covariance);
        assert!(with_noise_trace > no_noise_trace);
        assert!(with_noise.nodes()[1].covariance.is_positive_semidefinite());
    }

    #[test]
    fn backward_process_noise_uses_positive_time_span() {
        let state = CartesianState::new(0.0, [7000.0, 0.0, 0.0], [0.0, 7.5, 0.0]);
        let segment = CovarianceSegment {
            stm: identity6(),
            dt_seconds: -10.0,
            q_rotation_state: state,
        };
        let covariance0 = Covariance6::from_diagonal([1.0, 1.0, 1.0, 1.0, 1.0, 1.0]).unwrap();

        let covariances = transport_covariance(
            covariance0,
            &[segment],
            ProcessNoise::RtnAccelerationPsd {
                q_radial_km2_s3: 1.0e-9,
                q_transverse_km2_s3: 0.0,
                q_normal_km2_s3: 0.0,
            },
        )
        .expect("backward process noise");

        assert!(covariances[1].as_matrix()[0][0] > covariance0.as_matrix()[0][0]);
        assert!(covariances[1].as_matrix()[3][3] > covariance0.as_matrix()[3][3]);
    }

    #[test]
    fn rejects_direction_reversal_and_bad_noise() {
        let propagator = circular_propagator();
        let covariance0 = test_covariance();
        let initial = LabeledCovariance6 {
            covariance: covariance0,
            frame: CovarianceFrame::Inertial,
        };

        let err = propagator
            .propagate_covariance(
                initial,
                &[60.0, 30.0],
                &CovariancePropagationOptions::default(),
            )
            .unwrap_err();
        assert!(
            matches!(err, PropagationError::InvalidInput(message) if message.contains("epochs_tdb_seconds"))
        );

        let err = propagator
            .propagate_covariance(
                initial,
                &[60.0],
                &CovariancePropagationOptions {
                    process_noise: ProcessNoise::RtnAccelerationPsd {
                        q_radial_km2_s3: -1.0,
                        q_transverse_km2_s3: 0.0,
                        q_normal_km2_s3: 0.0,
                    },
                    output_frame: CovarianceFrame::Inertial,
                },
            )
            .unwrap_err();
        assert!(
            matches!(err, PropagationError::InvalidInput(message) if message.contains("q_radial_km2_s3"))
        );
    }

    fn velocity_trace(covariance: Covariance6) -> f64 {
        covariance.as_matrix()[3][3] + covariance.as_matrix()[4][4] + covariance.as_matrix()[5][5]
    }

    fn identity6() -> StateTransitionMatrix {
        let mut matrix = [[0.0_f64; 6]; 6];
        for (idx, row) in matrix.iter_mut().enumerate() {
            row[idx] = 1.0;
        }
        matrix
    }
}
