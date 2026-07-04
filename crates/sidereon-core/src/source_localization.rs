//! Source localization from arrival times.
//!
//! This module solves for an event position from sensors at known Cartesian
//! coordinates. Inputs are sans-I/O: callers supply sensor positions, arrival
//! times, and a propagation speed. Coordinates are metres in a caller-chosen 2D
//! or 3D Cartesian frame, times are seconds, and speeds are metres per second.

use core::fmt;

pub use trust_region_least_squares::loss::Loss;
use trust_region_least_squares::model::{solve_model, ResidualModel};
use trust_region_least_squares::trf::{TrfError, TrfOptions, TrfResult, XScale};

use crate::dop::{self, Dop, DopError};

const ROOT_TOL: f64 = 1.0e-12;

/// A sensor with a known Cartesian position.
///
/// `propagation_speed_m_s` overrides the call-level propagation speed for this
/// sensor when it is present. That is a simple per-path timing approximation;
/// no refraction or ray tracing is modeled.
#[derive(Debug, Clone, PartialEq)]
pub struct Sensor {
    /// Sensor position in metres. The vector length must be 2 or 3.
    pub position_m: Vec<f64>,
    /// Optional per-sensor propagation speed in metres per second.
    pub propagation_speed_m_s: Option<f64>,
}

impl Sensor {
    /// Construct a sensor that uses the call-level propagation speed.
    pub fn new(position_m: impl Into<Vec<f64>>) -> Self {
        Self {
            position_m: position_m.into(),
            propagation_speed_m_s: None,
        }
    }

    /// Construct a sensor with its own propagation speed.
    pub fn with_speed(position_m: impl Into<Vec<f64>>, propagation_speed_m_s: f64) -> Self {
        Self {
            position_m: position_m.into(),
            propagation_speed_m_s: Some(propagation_speed_m_s),
        }
    }
}

/// Measurement model used by [`locate_source`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceSolveMode {
    /// Absolute time of arrival. The state is `[position..., origin_time]`.
    #[default]
    Toa,
    /// Time difference of arrival against a reference sensor.
    ///
    /// The residual subtracts the reference sensor equation and does not solve
    /// an origin-time state. The returned origin time is estimated after the
    /// position solve from the absolute arrivals.
    Tdoa {
        /// Reference sensor index.
        reference_sensor: usize,
    },
}

/// Options for [`locate_source`].
#[derive(Debug, Clone, PartialEq)]
pub struct SourceLocateOptions {
    /// ToA or TDOA residual form.
    pub mode: SourceSolveMode,
    /// Timing standard deviation used for covariance, CRLB, and normalized
    /// influence scores.
    pub timing_sigma_s: f64,
    /// Loss function passed to the trust-region least-squares solver.
    pub loss: Loss,
    /// Residual scale in seconds for non-linear loss functions.
    pub f_scale_s: f64,
    /// Optional solver function tolerance.
    pub ftol: Option<f64>,
    /// Optional solver step tolerance.
    pub xtol: Option<f64>,
    /// Optional solver gradient tolerance.
    pub gtol: Option<f64>,
    /// Optional maximum residual evaluations.
    pub max_nfev: Option<usize>,
}

impl Default for SourceLocateOptions {
    fn default() -> Self {
        Self {
            mode: SourceSolveMode::Toa,
            timing_sigma_s: 1.0,
            loss: Loss::Linear,
            f_scale_s: 1.0,
            ftol: None,
            xtol: None,
            gtol: None,
            max_nfev: None,
        }
    }
}

/// Closed-form seed used to start the iterative solve.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceInitialGuess {
    /// Initial position in metres.
    pub position_m: Vec<f64>,
    /// Initial origin time in seconds when it can be inferred.
    pub origin_time_s: Option<f64>,
    /// Root-mean-square residual of the seed in seconds.
    pub residual_rms_s: f64,
}

/// One residual associated with a sensor row.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceResidual {
    /// Sensor index in the caller's input slice.
    pub sensor_index: usize,
    /// Reference sensor for a TDOA residual, or `None` for ToA.
    pub reference_sensor_index: Option<usize>,
    /// Residual in seconds.
    pub residual_s: f64,
}

/// Per-sensor leave-one-out diagnostic.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceSensorInfluence {
    /// Sensor index in the caller's input slice.
    pub sensor_index: usize,
    /// ToA residual at the full solution in seconds.
    pub residual_s: f64,
    /// Held-out ToA residual after solving without this sensor, in seconds.
    pub leave_one_out_residual_s: Option<f64>,
    /// Position change between the full and leave-one-out solutions, in metres.
    pub position_delta_m: Option<f64>,
    /// Origin-time change between the full and leave-one-out solutions, in seconds.
    pub origin_time_delta_s: Option<f64>,
    /// First-derivative loss weight for the full-solution residual.
    pub loss_weight: f64,
    /// Normalized diagnostic score. Larger values indicate a poorer fit.
    pub score: f64,
}

/// State covariance or Cramer-Rao lower bound for a source solve.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceCovariance {
    /// Full state covariance in solver state order.
    pub state: Vec<Vec<f64>>,
    /// Position covariance block in square metres.
    pub position_m2: Vec<Vec<f64>>,
    /// Origin-time variance in square seconds when origin time is in the state.
    pub origin_time_s2: Option<f64>,
    /// Timing sigma used to scale the cofactor.
    pub timing_sigma_s: f64,
}

/// Geometry and redundancy diagnostics for a source solve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceGeometryQuality {
    /// Number of residual rows used by the solve.
    pub residual_count: usize,
    /// Number of estimated state parameters.
    pub parameter_count: usize,
    /// `residual_count - parameter_count`, saturated at zero.
    pub redundancy: usize,
    /// Whether the covariance matrix was available from the normal matrix.
    pub covariance_available: bool,
    /// Whether the final normal matrix was rank deficient or not positive definite.
    pub rank_deficient: bool,
}

/// Source solution from [`locate_source`].
#[derive(Debug, Clone, PartialEq)]
pub struct SourceSolution {
    /// Estimated source position in metres.
    pub position_m: Vec<f64>,
    /// Estimated origin time in seconds.
    pub origin_time_s: Option<f64>,
    /// State covariance scaled by [`SourceLocateOptions::timing_sigma_s`].
    pub covariance: Option<SourceCovariance>,
    /// Solver residuals in seconds.
    pub residuals: Vec<SourceResidual>,
    /// Per-sensor influence diagnostics.
    pub per_sensor_influence: Vec<SourceSensorInfluence>,
    /// Geometry rank and redundancy summary.
    pub geometry_quality: SourceGeometryQuality,
    /// Closed-form seed used to start the iterative solve.
    pub initial_guess: SourceInitialGuess,
    /// Trust-region termination status.
    pub status: i32,
    /// Residual evaluations used by the solver.
    pub nfev: usize,
    /// Jacobian evaluations used by the solver.
    pub njev: usize,
    /// Final least-squares cost.
    pub cost: f64,
    /// Infinity norm of the final gradient.
    pub optimality: f64,
}

impl SourceSolution {
    /// Return the covariance as the CRLB for the timing sigma used by the solve.
    pub fn crlb(&self) -> Option<&SourceCovariance> {
        self.covariance.as_ref()
    }
}

/// CRLB and DOP for a proposed sensor/source geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceCrlb {
    /// DOP scalars formed from the timing design matrix.
    pub dop: Dop,
    /// State covariance scaled by the requested timing sigma.
    pub covariance: SourceCovariance,
}

/// Source-localization failure.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceLocalizationError {
    /// A boundary input is malformed.
    InvalidInput {
        /// Name of the malformed field.
        field: &'static str,
        /// Stable validation reason.
        reason: &'static str,
    },
    /// There are fewer sensors than the selected solve needs.
    TooFewSensors {
        /// Number of sensors supplied.
        sensors: usize,
        /// Minimum number of sensors required.
        needed: usize,
    },
    /// The closed-form initializer could not solve the geometry.
    InitializerSingular,
    /// Geometry DOP or CRLB failed.
    Geometry(DopError),
    /// The trust-region solver failed.
    Solver(TrfError),
    /// The trust-region solver exhausted its evaluation budget.
    DidNotConverge {
        /// Solver status code.
        status: i32,
    },
}

impl fmt::Display for SourceLocalizationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput { field, reason } => {
                write!(f, "invalid source localization input {field}: {reason}")
            }
            Self::TooFewSensors { sensors, needed } => {
                write!(
                    f,
                    "source localization has {sensors} sensors; need at least {needed}"
                )
            }
            Self::InitializerSingular => write!(f, "closed-form source initializer is singular"),
            Self::Geometry(err) => write!(f, "source geometry failed: {err}"),
            Self::Solver(err) => write!(f, "source solver failed: {err}"),
            Self::DidNotConverge { status } => {
                write!(f, "source solver did not converge, status {status}")
            }
        }
    }
}

impl std::error::Error for SourceLocalizationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Geometry(err) => Some(err),
            Self::Solver(err) => Some(err),
            _ => None,
        }
    }
}

impl From<DopError> for SourceLocalizationError {
    fn from(value: DopError) -> Self {
        Self::Geometry(value)
    }
}

impl From<TrfError> for SourceLocalizationError {
    fn from(value: TrfError) -> Self {
        Self::Solver(value)
    }
}

/// Locate a source from sensor arrival times.
///
/// `sensors` and `arrival_times_s` must have matching length. Positions must
/// all be 2D or all be 3D. The call-level propagation speed is used for every
/// sensor without a per-sensor override.
pub fn locate_source(
    sensors: &[Sensor],
    arrival_times_s: &[f64],
    propagation_speed_m_s: f64,
    options: &SourceLocateOptions,
) -> Result<SourceSolution, SourceLocalizationError> {
    locate_source_inner(
        sensors,
        arrival_times_s,
        propagation_speed_m_s,
        options,
        true,
    )
}

/// Compute the closed-form Chan-Ho style seed used by [`locate_source`].
///
/// The seed uses the call-level propagation speed in the closed-form equations.
/// Per-sensor speed overrides are applied by the iterative residual model.
pub fn chan_ho_initial_guess(
    sensors: &[Sensor],
    arrival_times_s: &[f64],
    propagation_speed_m_s: f64,
    mode: SourceSolveMode,
) -> Result<SourceInitialGuess, SourceLocalizationError> {
    let options = SourceLocateOptions {
        mode,
        ..SourceLocateOptions::default()
    };
    let resolved =
        resolve_locate_inputs(sensors, arrival_times_s, propagation_speed_m_s, &options)?;
    chan_ho_initial_guess_resolved(sensors, arrival_times_s, propagation_speed_m_s, &resolved)
}

/// Compute timing DOP for a proposed source location.
///
/// The returned position DOP values multiply timing sigma in seconds to produce
/// metres. The local Cartesian axes are used for the horizontal and vertical
/// split.
pub fn source_dop(
    sensors: &[Sensor],
    source_position_m: &[f64],
    propagation_speed_m_s: f64,
) -> Result<Dop, SourceLocalizationError> {
    let resolved = resolve_geometry_inputs(sensors, source_position_m, propagation_speed_m_s)?;
    let rows = source_toa_design_rows(sensors, source_position_m, &resolved)?;
    let weights = vec![1.0; sensors.len()];
    dop::dop_from_design_rows(&rows, &weights, resolved.dimension, identity_rotation())
        .map_err(SourceLocalizationError::Geometry)
}

/// Compute a timing CRLB for a proposed source location.
///
/// The covariance is `(H^T H)^-1 * timing_sigma_s^2`, where each row is the ToA
/// timing derivative at `source_position_m`.
pub fn source_crlb(
    sensors: &[Sensor],
    source_position_m: &[f64],
    propagation_speed_m_s: f64,
    timing_sigma_s: f64,
) -> Result<SourceCrlb, SourceLocalizationError> {
    validate_positive("timing_sigma_s", timing_sigma_s)?;
    let resolved = resolve_geometry_inputs(sensors, source_position_m, propagation_speed_m_s)?;
    let rows = source_toa_design_rows(sensors, source_position_m, &resolved)?;
    let weights = vec![1.0; sensors.len()];
    let rotation = identity_rotation();
    let cofactor =
        dop::geometry_cofactor_from_design_rows(&rows, &weights, resolved.dimension, rotation)?;
    let dop = dop::dop_from_design_rows(&rows, &weights, resolved.dimension, rotation)?;
    let covariance =
        covariance_from_state_cofactor(&cofactor.state, resolved.dimension, timing_sigma_s, true);
    Ok(SourceCrlb { dop, covariance })
}

#[derive(Debug, Clone)]
struct ResolvedInputs {
    dimension: usize,
    speeds_m_s: Vec<f64>,
    mode: SourceSolveMode,
}

#[derive(Debug, Clone)]
struct ResolvedGeometry {
    dimension: usize,
    speeds_m_s: Vec<f64>,
}

#[derive(Debug)]
struct SourceProblem<'a> {
    sensors: &'a [Sensor],
    arrival_times_s: &'a [f64],
    speeds_m_s: &'a [f64],
    dimension: usize,
    mode: SourceSolveMode,
}

impl SourceProblem<'_> {
    fn residual_records(&self, residuals: &[f64]) -> Vec<SourceResidual> {
        match self.mode {
            SourceSolveMode::Toa => residuals
                .iter()
                .enumerate()
                .map(|(sensor_index, &residual_s)| SourceResidual {
                    sensor_index,
                    reference_sensor_index: None,
                    residual_s,
                })
                .collect(),
            SourceSolveMode::Tdoa { reference_sensor } => {
                let mut out = Vec::with_capacity(residuals.len());
                let mut row = 0;
                for sensor_index in 0..self.sensors.len() {
                    if sensor_index == reference_sensor {
                        continue;
                    }
                    out.push(SourceResidual {
                        sensor_index,
                        reference_sensor_index: Some(reference_sensor),
                        residual_s: residuals[row],
                    });
                    row += 1;
                }
                out
            }
        }
    }
}

impl ResidualModel for SourceProblem<'_> {
    fn residual(&self, x: &[f64], out: &mut Vec<f64>) {
        out.clear();
        match self.mode {
            SourceSolveMode::Toa => {
                let origin_time_s = x[self.dimension];
                for (i, sensor) in self.sensors.iter().enumerate() {
                    let range_m = distance(&x[..self.dimension], &sensor.position_m);
                    out.push(
                        origin_time_s + range_m / self.speeds_m_s[i] - self.arrival_times_s[i],
                    );
                }
            }
            SourceSolveMode::Tdoa { reference_sensor } => {
                let ref_range_m = distance(
                    &x[..self.dimension],
                    &self.sensors[reference_sensor].position_m,
                );
                let ref_time_s = ref_range_m / self.speeds_m_s[reference_sensor];
                for (i, sensor) in self.sensors.iter().enumerate() {
                    if i == reference_sensor {
                        continue;
                    }
                    let range_m = distance(&x[..self.dimension], &sensor.position_m);
                    let predicted_s = range_m / self.speeds_m_s[i] - ref_time_s;
                    let observed_s =
                        self.arrival_times_s[i] - self.arrival_times_s[reference_sensor];
                    out.push(predicted_s - observed_s);
                }
            }
        }
    }

    fn jacobian(&self, x: &[f64], _f0: &[f64], out: &mut Vec<f64>) {
        out.clear();
        match self.mode {
            SourceSolveMode::Toa => {
                let n = self.dimension + 1;
                out.resize(self.sensors.len() * n, 0.0);
                for (row, sensor) in self.sensors.iter().enumerate() {
                    fill_range_derivative(
                        &x[..self.dimension],
                        &sensor.position_m,
                        self.speeds_m_s[row],
                        &mut out[row * n..row * n + self.dimension],
                    );
                    out[row * n + self.dimension] = 1.0;
                }
            }
            SourceSolveMode::Tdoa { reference_sensor } => {
                let n = self.dimension;
                out.resize((self.sensors.len() - 1) * n, 0.0);
                let mut ref_derivative = vec![0.0; self.dimension];
                fill_range_derivative(
                    &x[..self.dimension],
                    &self.sensors[reference_sensor].position_m,
                    self.speeds_m_s[reference_sensor],
                    &mut ref_derivative,
                );
                let mut row = 0;
                for (i, sensor) in self.sensors.iter().enumerate() {
                    if i == reference_sensor {
                        continue;
                    }
                    let start = row * n;
                    fill_range_derivative(
                        &x[..self.dimension],
                        &sensor.position_m,
                        self.speeds_m_s[i],
                        &mut out[start..start + n],
                    );
                    for axis in 0..n {
                        out[start + axis] -= ref_derivative[axis];
                    }
                    row += 1;
                }
            }
        }
    }
}

fn locate_source_inner(
    sensors: &[Sensor],
    arrival_times_s: &[f64],
    propagation_speed_m_s: f64,
    options: &SourceLocateOptions,
    include_influence: bool,
) -> Result<SourceSolution, SourceLocalizationError> {
    let resolved = resolve_locate_inputs(sensors, arrival_times_s, propagation_speed_m_s, options)?;
    let initial_guess =
        chan_ho_initial_guess_resolved(sensors, arrival_times_s, propagation_speed_m_s, &resolved)?;
    let mut x0 = initial_guess.position_m.clone();
    if matches!(resolved.mode, SourceSolveMode::Toa) {
        x0.push(initial_guess.origin_time_s.expect("ToA seed has time"));
    }

    let problem = SourceProblem {
        sensors,
        arrival_times_s,
        speeds_m_s: &resolved.speeds_m_s,
        dimension: resolved.dimension,
        mode: resolved.mode,
    };
    let result = solve_model(&problem, &x0, &solver_options(options))?;
    if !result.success() {
        return Err(SourceLocalizationError::DidNotConverge {
            status: result.status,
        });
    }

    let mut solution = build_solution(
        &problem,
        &resolved,
        &initial_guess,
        result,
        options.timing_sigma_s,
    )?;
    if include_influence {
        solution.per_sensor_influence = compute_influence(
            &solution,
            sensors,
            arrival_times_s,
            propagation_speed_m_s,
            options,
        );
    }
    Ok(solution)
}

fn build_solution(
    problem: &SourceProblem<'_>,
    resolved: &ResolvedInputs,
    initial_guess: &SourceInitialGuess,
    result: TrfResult,
    timing_sigma_s: f64,
) -> Result<SourceSolution, SourceLocalizationError> {
    let position_m = result.x[..resolved.dimension].to_vec();
    let origin_time_s = match resolved.mode {
        SourceSolveMode::Toa => Some(result.x[resolved.dimension]),
        SourceSolveMode::Tdoa { .. } => Some(estimate_origin_time_s(
            problem.sensors,
            problem.arrival_times_s,
            problem.speeds_m_s,
            &position_m,
        )),
    };
    let residuals = problem.residual_records(&result.fun);
    let parameter_count = result.x.len();
    let residual_count = result.fun.len();
    let covariance = covariance_from_jacobian(
        &result.jac,
        residual_count,
        parameter_count,
        resolved.dimension,
        timing_sigma_s,
    );
    let covariance_available = covariance.is_some();
    Ok(SourceSolution {
        position_m,
        origin_time_s,
        covariance,
        residuals,
        per_sensor_influence: Vec::new(),
        geometry_quality: SourceGeometryQuality {
            residual_count,
            parameter_count,
            redundancy: residual_count.saturating_sub(parameter_count),
            covariance_available,
            rank_deficient: !covariance_available,
        },
        initial_guess: initial_guess.clone(),
        status: result.status,
        nfev: result.nfev,
        njev: result.njev,
        cost: result.cost,
        optimality: result.optimality,
    })
}

fn chan_ho_initial_guess_resolved(
    sensors: &[Sensor],
    arrival_times_s: &[f64],
    propagation_speed_m_s: f64,
    resolved: &ResolvedInputs,
) -> Result<SourceInitialGuess, SourceLocalizationError> {
    match resolved.mode {
        SourceSolveMode::Toa => {
            chan_ho_toa_initial_guess(sensors, arrival_times_s, propagation_speed_m_s, resolved)
        }
        SourceSolveMode::Tdoa { reference_sensor } => chan_ho_tdoa_initial_guess(
            sensors,
            arrival_times_s,
            propagation_speed_m_s,
            resolved,
            reference_sensor,
        ),
    }
}

fn chan_ho_toa_initial_guess(
    sensors: &[Sensor],
    arrival_times_s: &[f64],
    propagation_speed_m_s: f64,
    resolved: &ResolvedInputs,
) -> Result<SourceInitialGuess, SourceLocalizationError> {
    let d = resolved.dimension;
    let ref_pos = &sensors[0].position_m;
    let z0 = propagation_speed_m_s * arrival_times_s[0];
    let ref_norm2 = dot(ref_pos, ref_pos);
    let mut a = Vec::with_capacity(sensors.len() - 1);
    let mut b = Vec::with_capacity(sensors.len() - 1);
    let mut h = Vec::with_capacity(sensors.len() - 1);
    for i in 1..sensors.len() {
        let row: Vec<f64> = sensors[i]
            .position_m
            .iter()
            .zip(ref_pos)
            .map(|(s, r)| s - r)
            .collect();
        let zi = propagation_speed_m_s * arrival_times_s[i];
        let delta_z = zi - z0;
        let delta_norm = dot(&sensors[i].position_m, &sensors[i].position_m) - ref_norm2;
        a.push(row);
        b.push(0.5 * (delta_norm - (zi * zi - z0 * z0)));
        h.push(delta_z);
    }
    let p0 = least_squares(&a, &b)?;
    let p1 = least_squares(&a, &h)?;
    let q: Vec<f64> = p0.iter().zip(ref_pos).map(|(p, r)| p - r).collect();
    let roots = quadratic_roots(
        dot(&p1, &p1) - 1.0,
        2.0 * dot(&q, &p1) + 2.0 * z0,
        dot(&q, &q) - z0 * z0,
    )?;

    let mut best: Option<SourceInitialGuess> = None;
    let mut best_sse = f64::INFINITY;
    for tau_m in roots {
        let position_m: Vec<f64> = (0..d).map(|axis| p0[axis] + p1[axis] * tau_m).collect();
        if sensors.iter().any(|sensor| {
            distance(&position_m, &sensor.position_m) > propagation_speed_m_s * 1.0e12
        }) {
            continue;
        }
        let origin_time_s = tau_m / propagation_speed_m_s;
        let sse = toa_sse(
            sensors,
            arrival_times_s,
            &resolved.speeds_m_s,
            &position_m,
            origin_time_s,
        );
        if sse < best_sse {
            best_sse = sse;
            best = Some(SourceInitialGuess {
                position_m,
                origin_time_s: Some(origin_time_s),
                residual_rms_s: (sse / sensors.len() as f64).sqrt(),
            });
        }
    }
    best.ok_or(SourceLocalizationError::InitializerSingular)
}

fn chan_ho_tdoa_initial_guess(
    sensors: &[Sensor],
    arrival_times_s: &[f64],
    propagation_speed_m_s: f64,
    resolved: &ResolvedInputs,
    reference_sensor: usize,
) -> Result<SourceInitialGuess, SourceLocalizationError> {
    let d = resolved.dimension;
    let ref_pos = &sensors[reference_sensor].position_m;
    let ref_norm2 = dot(ref_pos, ref_pos);
    let mut a = Vec::with_capacity(sensors.len() - 1);
    let mut b = Vec::with_capacity(sensors.len() - 1);
    let mut h = Vec::with_capacity(sensors.len() - 1);
    for (i, sensor) in sensors.iter().enumerate() {
        if i == reference_sensor {
            continue;
        }
        let row: Vec<f64> = sensor
            .position_m
            .iter()
            .zip(ref_pos)
            .map(|(s, r)| s - r)
            .collect();
        let delta_range_m =
            propagation_speed_m_s * (arrival_times_s[i] - arrival_times_s[reference_sensor]);
        let delta_norm = dot(&sensor.position_m, &sensor.position_m) - ref_norm2;
        a.push(row);
        b.push(0.5 * (delta_norm - delta_range_m * delta_range_m));
        h.push(-delta_range_m);
    }
    let p0 = least_squares(&a, &b)?;
    let p1 = least_squares(&a, &h)?;
    let q: Vec<f64> = p0.iter().zip(ref_pos).map(|(p, r)| p - r).collect();
    let roots = quadratic_roots(dot(&p1, &p1) - 1.0, 2.0 * dot(&q, &p1), dot(&q, &q))?;

    let mut best: Option<SourceInitialGuess> = None;
    let mut best_sse = f64::INFINITY;
    for rho_m in roots {
        if rho_m < 0.0 {
            continue;
        }
        let position_m: Vec<f64> = (0..d).map(|axis| p0[axis] + p1[axis] * rho_m).collect();
        let origin_time_s =
            estimate_origin_time_s(sensors, arrival_times_s, &resolved.speeds_m_s, &position_m);
        let sse = tdoa_sse(
            sensors,
            arrival_times_s,
            &resolved.speeds_m_s,
            &position_m,
            reference_sensor,
        );
        if sse < best_sse {
            best_sse = sse;
            best = Some(SourceInitialGuess {
                position_m,
                origin_time_s: Some(origin_time_s),
                residual_rms_s: (sse / (sensors.len() - 1) as f64).sqrt(),
            });
        }
    }
    best.ok_or(SourceLocalizationError::InitializerSingular)
}

fn compute_influence(
    solution: &SourceSolution,
    sensors: &[Sensor],
    arrival_times_s: &[f64],
    propagation_speed_m_s: f64,
    options: &SourceLocateOptions,
) -> Vec<SourceSensorInfluence> {
    let speeds = match sensor_speeds(sensors, propagation_speed_m_s) {
        Ok(speeds) => speeds,
        Err(_) => return Vec::new(),
    };
    let origin_time_s = solution.origin_time_s.unwrap_or_else(|| {
        estimate_origin_time_s(sensors, arrival_times_s, &speeds, &solution.position_m)
    });
    let full_residuals = toa_residuals(
        sensors,
        arrival_times_s,
        &speeds,
        &solution.position_m,
        origin_time_s,
    );
    let sigma = options.timing_sigma_s.max(f64::MIN_POSITIVE);

    (0..sensors.len())
        .map(|sensor_index| {
            let loo = leave_one_out_solution(
                sensors,
                arrival_times_s,
                propagation_speed_m_s,
                options,
                sensor_index,
            );
            let (leave_one_out_residual_s, position_delta_m, origin_time_delta_s) =
                if let Some(loo_solution) = loo {
                    let loo_origin = loo_solution.origin_time_s.unwrap_or_else(|| {
                        estimate_origin_time_s(
                            sensors,
                            arrival_times_s,
                            &speeds,
                            &loo_solution.position_m,
                        )
                    });
                    let held_out_residual = single_toa_residual(
                        &sensors[sensor_index],
                        arrival_times_s[sensor_index],
                        speeds[sensor_index],
                        &loo_solution.position_m,
                        loo_origin,
                    );
                    (
                        Some(held_out_residual),
                        Some(distance(&solution.position_m, &loo_solution.position_m)),
                        Some((origin_time_s - loo_origin).abs()),
                    )
                } else {
                    (None, None, None)
                };
            let loss_weight = loss_weight(
                options.loss,
                options.f_scale_s,
                full_residuals[sensor_index],
            );
            let score_basis = leave_one_out_residual_s
                .unwrap_or(full_residuals[sensor_index])
                .abs()
                .max(full_residuals[sensor_index].abs());
            SourceSensorInfluence {
                sensor_index,
                residual_s: full_residuals[sensor_index],
                leave_one_out_residual_s,
                position_delta_m,
                origin_time_delta_s,
                loss_weight,
                score: score_basis / sigma + (1.0 - loss_weight) * 1.0e6,
            }
        })
        .collect()
}

fn leave_one_out_solution(
    sensors: &[Sensor],
    arrival_times_s: &[f64],
    propagation_speed_m_s: f64,
    options: &SourceLocateOptions,
    excluded: usize,
) -> Option<SourceSolution> {
    let mut sub_sensors = Vec::with_capacity(sensors.len() - 1);
    let mut sub_arrivals = Vec::with_capacity(arrival_times_s.len() - 1);
    for (i, sensor) in sensors.iter().enumerate() {
        if i == excluded {
            continue;
        }
        sub_sensors.push(sensor.clone());
        sub_arrivals.push(arrival_times_s[i]);
    }
    let mut sub_options = options.clone();
    sub_options.mode = match options.mode {
        SourceSolveMode::Toa => SourceSolveMode::Toa,
        SourceSolveMode::Tdoa { reference_sensor } => {
            if excluded == reference_sensor {
                SourceSolveMode::Tdoa {
                    reference_sensor: 0,
                }
            } else if excluded < reference_sensor {
                SourceSolveMode::Tdoa {
                    reference_sensor: reference_sensor - 1,
                }
            } else {
                SourceSolveMode::Tdoa { reference_sensor }
            }
        }
    };
    locate_source_inner(
        &sub_sensors,
        &sub_arrivals,
        propagation_speed_m_s,
        &sub_options,
        false,
    )
    .ok()
}

fn resolve_locate_inputs(
    sensors: &[Sensor],
    arrival_times_s: &[f64],
    propagation_speed_m_s: f64,
    options: &SourceLocateOptions,
) -> Result<ResolvedInputs, SourceLocalizationError> {
    if sensors.len() != arrival_times_s.len() {
        return Err(invalid_input(
            "arrival_times_s",
            "length must match sensors",
        ));
    }
    for &arrival in arrival_times_s {
        validate_finite("arrival_times_s", arrival)?;
    }
    validate_positive("timing_sigma_s", options.timing_sigma_s)?;
    if options.loss != Loss::Linear {
        validate_positive("f_scale_s", options.f_scale_s)?;
    }
    validate_optional_positive("ftol", options.ftol)?;
    validate_optional_positive("xtol", options.xtol)?;
    validate_optional_positive("gtol", options.gtol)?;
    if options.max_nfev == Some(0) {
        return Err(invalid_input("max_nfev", "must be positive"));
    }
    let geometry = resolve_geometry_inputs(
        sensors,
        sensors
            .first()
            .map(|sensor| sensor.position_m.as_slice())
            .unwrap_or(&[]),
        propagation_speed_m_s,
    )?;
    if let SourceSolveMode::Tdoa { reference_sensor } = options.mode {
        if reference_sensor >= sensors.len() {
            return Err(invalid_input("reference_sensor", "out of range"));
        }
    }
    let needed = geometry.dimension + 1;
    if sensors.len() < needed {
        return Err(SourceLocalizationError::TooFewSensors {
            sensors: sensors.len(),
            needed,
        });
    }
    Ok(ResolvedInputs {
        dimension: geometry.dimension,
        speeds_m_s: geometry.speeds_m_s,
        mode: options.mode,
    })
}

fn resolve_geometry_inputs(
    sensors: &[Sensor],
    source_position_m: &[f64],
    propagation_speed_m_s: f64,
) -> Result<ResolvedGeometry, SourceLocalizationError> {
    if sensors.is_empty() {
        return Err(SourceLocalizationError::TooFewSensors {
            sensors: 0,
            needed: 3,
        });
    }
    validate_positive("propagation_speed_m_s", propagation_speed_m_s)?;
    let dimension = sensors[0].position_m.len();
    if !(2..=3).contains(&dimension) {
        return Err(invalid_input("position_m", "length must be 2 or 3"));
    }
    if !source_position_m.is_empty() && source_position_m.len() != dimension {
        return Err(invalid_input(
            "source_position_m",
            "length must match sensors",
        ));
    }
    for sensor in sensors {
        if sensor.position_m.len() != dimension {
            return Err(invalid_input("position_m", "length must match sensors"));
        }
        for &value in &sensor.position_m {
            validate_finite("position_m", value)?;
        }
        if let Some(speed) = sensor.propagation_speed_m_s {
            validate_positive("sensor.propagation_speed_m_s", speed)?;
        }
    }
    for &value in source_position_m {
        validate_finite("source_position_m", value)?;
    }
    Ok(ResolvedGeometry {
        dimension,
        speeds_m_s: sensor_speeds(sensors, propagation_speed_m_s)?,
    })
}

fn sensor_speeds(
    sensors: &[Sensor],
    propagation_speed_m_s: f64,
) -> Result<Vec<f64>, SourceLocalizationError> {
    validate_positive("propagation_speed_m_s", propagation_speed_m_s)?;
    sensors
        .iter()
        .map(|sensor| {
            let speed = sensor
                .propagation_speed_m_s
                .unwrap_or(propagation_speed_m_s);
            validate_positive("sensor.propagation_speed_m_s", speed)?;
            Ok(speed)
        })
        .collect()
}

fn source_toa_design_rows(
    sensors: &[Sensor],
    source_position_m: &[f64],
    resolved: &ResolvedGeometry,
) -> Result<Vec<Vec<f64>>, SourceLocalizationError> {
    sensors
        .iter()
        .zip(&resolved.speeds_m_s)
        .map(|(sensor, &speed)| {
            let mut row = vec![0.0; resolved.dimension + 1];
            let range_m = distance(source_position_m, &sensor.position_m);
            if range_m <= 0.0 {
                return Err(invalid_input(
                    "source_position_m",
                    "coincident with a sensor",
                ));
            }
            for axis in 0..resolved.dimension {
                row[axis] = (source_position_m[axis] - sensor.position_m[axis]) / range_m / speed;
            }
            row[resolved.dimension] = 1.0;
            Ok(row)
        })
        .collect()
}

fn covariance_from_jacobian(
    jac: &[f64],
    m: usize,
    n: usize,
    dimension: usize,
    timing_sigma_s: f64,
) -> Option<SourceCovariance> {
    if jac.len() != m.checked_mul(n)? {
        return None;
    }
    let mut normal = vec![vec![0.0_f64; n]; n];
    for row in 0..m {
        for i in 0..n {
            for j in 0..n {
                normal[i][j] += jac[row * n + i] * jac[row * n + j];
            }
        }
    }
    let cofactor = crate::astro::math::linear::invert_symmetric_pd(&normal)?;
    Some(covariance_from_state_cofactor(
        &cofactor,
        dimension,
        timing_sigma_s,
        n == dimension + 1,
    ))
}

fn covariance_from_state_cofactor(
    cofactor: &[Vec<f64>],
    dimension: usize,
    timing_sigma_s: f64,
    has_origin_time: bool,
) -> SourceCovariance {
    let scale = timing_sigma_s * timing_sigma_s;
    let state: Vec<Vec<f64>> = cofactor
        .iter()
        .map(|row| row.iter().map(|value| value * scale).collect())
        .collect();
    let position_m2: Vec<Vec<f64>> = (0..dimension)
        .map(|i| (0..dimension).map(|j| state[i][j]).collect())
        .collect();
    SourceCovariance {
        origin_time_s2: if has_origin_time {
            Some(state[dimension][dimension])
        } else {
            None
        },
        state,
        position_m2,
        timing_sigma_s,
    }
}

fn solver_options(config: &SourceLocateOptions) -> TrfOptions {
    let mut options = TrfOptions::default();
    if let Some(ftol) = config.ftol {
        options.ftol = ftol;
    }
    if let Some(xtol) = config.xtol {
        options.xtol = xtol;
    }
    if let Some(gtol) = config.gtol {
        options.gtol = gtol;
    }
    options.max_nfev = config.max_nfev;
    options.x_scale = XScale::Jac;
    options.loss = config.loss;
    options.f_scale = config.f_scale_s;
    options
}

fn least_squares(a: &[Vec<f64>], y: &[f64]) -> Result<Vec<f64>, SourceLocalizationError> {
    let n = a.first().map(Vec::len).unwrap_or(0);
    if n == 0 || a.len() != y.len() || a.len() < n {
        return Err(SourceLocalizationError::InitializerSingular);
    }
    let mut normal = vec![vec![0.0_f64; n]; n];
    let mut rhs = vec![0.0_f64; n];
    for (row, &value) in a.iter().zip(y) {
        if row.len() != n {
            return Err(SourceLocalizationError::InitializerSingular);
        }
        for i in 0..n {
            rhs[i] += row[i] * value;
            for j in 0..n {
                normal[i][j] += row[i] * row[j];
            }
        }
    }
    let inv = crate::astro::math::linear::invert_symmetric_pd(&normal)
        .ok_or(SourceLocalizationError::InitializerSingular)?;
    Ok((0..n)
        .map(|i| (0..n).map(|j| inv[i][j] * rhs[j]).sum())
        .collect())
}

fn quadratic_roots(a: f64, b: f64, c: f64) -> Result<Vec<f64>, SourceLocalizationError> {
    if !a.is_finite() || !b.is_finite() || !c.is_finite() {
        return Err(SourceLocalizationError::InitializerSingular);
    }
    if a.abs() <= ROOT_TOL {
        if b.abs() <= ROOT_TOL {
            return Err(SourceLocalizationError::InitializerSingular);
        }
        return Ok(vec![-c / b]);
    }
    let disc = b * b - 4.0 * a * c;
    if disc < -ROOT_TOL || !disc.is_finite() {
        return Err(SourceLocalizationError::InitializerSingular);
    }
    let root = disc.max(0.0).sqrt();
    Ok(vec![(-b - root) / (2.0 * a), (-b + root) / (2.0 * a)])
}

fn toa_sse(
    sensors: &[Sensor],
    arrival_times_s: &[f64],
    speeds_m_s: &[f64],
    position_m: &[f64],
    origin_time_s: f64,
) -> f64 {
    toa_residuals(
        sensors,
        arrival_times_s,
        speeds_m_s,
        position_m,
        origin_time_s,
    )
    .iter()
    .map(|value| value * value)
    .sum()
}

fn tdoa_sse(
    sensors: &[Sensor],
    arrival_times_s: &[f64],
    speeds_m_s: &[f64],
    position_m: &[f64],
    reference_sensor: usize,
) -> f64 {
    let ref_time =
        distance(position_m, &sensors[reference_sensor].position_m) / speeds_m_s[reference_sensor];
    let mut sse = 0.0;
    for (i, sensor) in sensors.iter().enumerate() {
        if i == reference_sensor {
            continue;
        }
        let predicted = distance(position_m, &sensor.position_m) / speeds_m_s[i] - ref_time;
        let observed = arrival_times_s[i] - arrival_times_s[reference_sensor];
        let residual = predicted - observed;
        sse += residual * residual;
    }
    sse
}

fn toa_residuals(
    sensors: &[Sensor],
    arrival_times_s: &[f64],
    speeds_m_s: &[f64],
    position_m: &[f64],
    origin_time_s: f64,
) -> Vec<f64> {
    sensors
        .iter()
        .enumerate()
        .map(|(i, sensor)| {
            single_toa_residual(
                sensor,
                arrival_times_s[i],
                speeds_m_s[i],
                position_m,
                origin_time_s,
            )
        })
        .collect()
}

fn single_toa_residual(
    sensor: &Sensor,
    arrival_time_s: f64,
    speed_m_s: f64,
    position_m: &[f64],
    origin_time_s: f64,
) -> f64 {
    origin_time_s + distance(position_m, &sensor.position_m) / speed_m_s - arrival_time_s
}

fn estimate_origin_time_s(
    sensors: &[Sensor],
    arrival_times_s: &[f64],
    speeds_m_s: &[f64],
    position_m: &[f64],
) -> f64 {
    let sum: f64 = sensors
        .iter()
        .enumerate()
        .map(|(i, sensor)| {
            arrival_times_s[i] - distance(position_m, &sensor.position_m) / speeds_m_s[i]
        })
        .sum();
    sum / sensors.len() as f64
}

fn fill_range_derivative(position_m: &[f64], sensor_m: &[f64], speed_m_s: f64, out: &mut [f64]) {
    let range_m = distance(position_m, sensor_m);
    if range_m <= 0.0 || !range_m.is_finite() {
        out.fill(0.0);
        return;
    }
    for axis in 0..out.len() {
        out[axis] = (position_m[axis] - sensor_m[axis]) / range_m / speed_m_s;
    }
}

fn loss_weight(loss: Loss, f_scale_s: f64, residual_s: f64) -> f64 {
    if loss == Loss::Linear {
        return 1.0;
    }
    let z = (residual_s / f_scale_s) * (residual_s / f_scale_s);
    match loss {
        Loss::Linear => 1.0,
        Loss::Huber => {
            if z <= 1.0 {
                1.0
            } else {
                z.sqrt().recip()
            }
        }
        Loss::SoftL1 => (1.0 + z).sqrt().recip(),
        Loss::Cauchy => (1.0 + z).recip(),
        Loss::Arctan => (1.0 + z * z).recip(),
    }
}

fn distance(a: &[f64], b: &[f64]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let d = x - y;
            d * d
        })
        .sum::<f64>()
        .sqrt()
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn identity_rotation() -> [[f64; 3]; 3] {
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
}

fn invalid_input(field: &'static str, reason: &'static str) -> SourceLocalizationError {
    SourceLocalizationError::InvalidInput { field, reason }
}

fn validate_optional_positive(
    field: &'static str,
    value: Option<f64>,
) -> Result<(), SourceLocalizationError> {
    if let Some(value) = value {
        validate_positive(field, value)?;
    }
    Ok(())
}

fn validate_positive(field: &'static str, value: f64) -> Result<(), SourceLocalizationError> {
    validate_finite(field, value)?;
    if value <= 0.0 {
        return Err(invalid_input(field, "must be > 0"));
    }
    Ok(())
}

fn validate_finite(field: &'static str, value: f64) -> Result<(), SourceLocalizationError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(invalid_input(field, "must be finite"))
    }
}

#[cfg(test)]
mod tests {
    //! Analytic source-localization fixtures.
    //!
    //! The tests below use Euclidean ranges, closed-form normal equations, and
    //! synthetic corrupted arrivals. They do not compare against another
    //! implementation.

    use super::*;
    use crate::dop::{dop, dop_from_design_rows, ecef_to_enu_rotation, LineOfSight};
    use crate::frame::Wgs84Geodetic;

    fn arrivals(sensors: &[Sensor], source: &[f64], origin: f64, speed: f64) -> Vec<f64> {
        sensors
            .iter()
            .map(|sensor| {
                let s = sensor.propagation_speed_m_s.unwrap_or(speed);
                origin + distance(source, &sensor.position_m) / s
            })
            .collect()
    }

    fn assert_vec_close(actual: &[f64], expected: &[f64], tol: f64) {
        for (axis, (a, e)) in actual.iter().zip(expected).enumerate() {
            assert!(
                (a - e).abs() < tol,
                "axis {axis}: actual {a}, expected {e}, tol {tol}"
            );
        }
    }

    #[test]
    fn chan_ho_toa_initializer_recovers_clean_3d() {
        let sensors = vec![
            Sensor::new(vec![0.0, 0.0, 0.0]),
            Sensor::new(vec![1200.0, 0.0, 0.0]),
            Sensor::new(vec![0.0, 900.0, 0.0]),
            Sensor::new(vec![0.0, 0.0, 700.0]),
            Sensor::new(vec![1100.0, 800.0, 600.0]),
        ];
        let source = vec![320.0, 260.0, 180.0];
        let origin = 12.5;
        let speed = 343.0;
        let times = arrivals(&sensors, &source, origin, speed);

        let seed =
            chan_ho_initial_guess(&sensors, &times, speed, SourceSolveMode::Toa).expect("seed");
        assert_vec_close(&seed.position_m, &source, 1.0e-8);
        assert!((seed.origin_time_s.unwrap() - origin).abs() < 1.0e-10);
        assert!(seed.residual_rms_s < 1.0e-11);
    }

    #[test]
    fn locate_source_toa_recovers_clean_3d() {
        let sensors = vec![
            Sensor::new(vec![0.0, 0.0, 0.0]),
            Sensor::new(vec![1200.0, 0.0, 0.0]),
            Sensor::new(vec![0.0, 900.0, 0.0]),
            Sensor::new(vec![0.0, 0.0, 700.0]),
            Sensor::new(vec![1100.0, 800.0, 600.0]),
        ];
        let source = vec![320.0, 260.0, 180.0];
        let origin = 12.5;
        let speed = 343.0;
        let times = arrivals(&sensors, &source, origin, speed);
        let options = SourceLocateOptions {
            timing_sigma_s: 0.001,
            ..SourceLocateOptions::default()
        };

        let solution = locate_source(&sensors, &times, speed, &options).expect("solution");
        assert_vec_close(&solution.position_m, &source, 1.0e-7);
        assert!((solution.origin_time_s.unwrap() - origin).abs() < 1.0e-10);
        assert!(solution.covariance.is_some());
        assert!(solution
            .residuals
            .iter()
            .all(|row| row.residual_s.abs() < 1.0e-10));
    }

    #[test]
    fn locate_source_tdoa_recovers_clean_2d() {
        let sensors = vec![
            Sensor::new(vec![0.0, 0.0]),
            Sensor::new(vec![1000.0, 0.0]),
            Sensor::new(vec![0.0, 800.0]),
            Sensor::new(vec![900.0, 900.0]),
        ];
        let source = vec![300.0, 260.0];
        let origin = 4.0;
        let speed = 340.0;
        let times = arrivals(&sensors, &source, origin, speed);
        let options = SourceLocateOptions {
            mode: SourceSolveMode::Tdoa {
                reference_sensor: 0,
            },
            timing_sigma_s: 0.001,
            ..SourceLocateOptions::default()
        };

        let solution = locate_source(&sensors, &times, speed, &options).expect("solution");
        assert_vec_close(&solution.position_m, &source, 1.0e-7);
        assert!((solution.origin_time_s.unwrap() - origin).abs() < 1.0e-9);
        assert_eq!(solution.residuals.len(), sensors.len() - 1);
    }

    #[test]
    fn per_sensor_speed_override_refines_from_uniform_seed() {
        let sensors = vec![
            Sensor::new(vec![0.0, 0.0, 0.0]),
            Sensor::with_speed(vec![1200.0, 0.0, 0.0], 330.0),
            Sensor::new(vec![0.0, 900.0, 0.0]),
            Sensor::new(vec![0.0, 0.0, 700.0]),
            Sensor::new(vec![1100.0, 800.0, 600.0]),
        ];
        let source = vec![320.0, 260.0, 180.0];
        let origin = 12.5;
        let speed = 343.0;
        let times = arrivals(&sensors, &source, origin, speed);

        let solution =
            locate_source(&sensors, &times, speed, &SourceLocateOptions::default()).expect("solve");
        assert_vec_close(&solution.position_m, &source, 1.0e-6);
        assert!((solution.origin_time_s.unwrap() - origin).abs() < 1.0e-9);
    }

    #[test]
    fn source_dop_matches_hand_computed_square_layout() {
        let sensors = vec![
            Sensor::new(vec![100.0, 0.0]),
            Sensor::new(vec![-100.0, 0.0]),
            Sensor::new(vec![0.0, 100.0]),
            Sensor::new(vec![0.0, -100.0]),
        ];
        let source = vec![0.0, 0.0];
        let speed = 10.0;

        let d = source_dop(&sensors, &source, speed).expect("dop");
        assert!((d.pdop - 10.0).abs() < 1.0e-12);
        assert!((d.hdop - 10.0).abs() < 1.0e-12);
        assert_eq!(d.vdop.to_bits(), 0.0_f64.to_bits());
        assert!((d.tdop - 0.5).abs() < 1.0e-12);
        assert!((d.gdop - 100.25_f64.sqrt()).abs() < 1.0e-12);

        let crlb = source_crlb(&sensors, &source, speed, 0.01).expect("crlb");
        assert!((crlb.covariance.position_m2[0][0] - 0.005).abs() < 1.0e-15);
        assert!((crlb.covariance.position_m2[1][1] - 0.005).abs() < 1.0e-15);
        assert!((crlb.covariance.origin_time_s2.unwrap() - 0.000025).abs() < 1.0e-18);
    }

    #[test]
    fn generalized_dop_matches_gnss_rows() {
        let receiver = Wgs84Geodetic::new(45.0_f64.to_radians(), -75.0_f64.to_radians(), 100.0)
            .expect("receiver");
        let los = vec![
            LineOfSight::new(0.6509445549041194, -0.3229151081253906, 0.6870132099084238),
            LineOfSight::new(-0.1936430033175727, 0.7473746634879952, 0.6356771337896102),
            LineOfSight::new(
                -0.730_360_483_841_695,
                -0.506583142388898,
                0.4579016226872558,
            ),
            LineOfSight::new(0.189511839684945, -0.9347210311772362, 0.300573550871319),
        ];
        let weights = vec![1.0, 0.9, 1.2, 0.8];
        let gnss = dop(&los, &weights, receiver).expect("gnss dop");
        let rows = los
            .iter()
            .map(|line| vec![-line.e_x, -line.e_y, -line.e_z, 1.0])
            .collect::<Vec<_>>();
        let rotation = ecef_to_enu_rotation(receiver.lat_rad, receiver.lon_rad);
        let general = dop_from_design_rows(&rows, &weights, 3, rotation).expect("general dop");

        assert_eq!(gnss.gdop.to_bits(), general.gdop.to_bits());
        assert_eq!(gnss.pdop.to_bits(), general.pdop.to_bits());
        assert_eq!(gnss.hdop.to_bits(), general.hdop.to_bits());
        assert_eq!(gnss.vdop.to_bits(), general.vdop.to_bits());
        assert_eq!(gnss.tdop.to_bits(), general.tdop.to_bits());
    }

    #[test]
    fn corrupted_arrival_is_downweighted_and_flagged() {
        let sensors = vec![
            Sensor::new(vec![100.0, 0.0]),
            Sensor::new(vec![-100.0, 0.0]),
            Sensor::new(vec![0.0, 100.0]),
            Sensor::new(vec![0.0, -100.0]),
            Sensor::new(vec![120.0, 120.0]),
            Sensor::new(vec![-120.0, 80.0]),
        ];
        let source = vec![15.0, -20.0];
        let origin = 1.25;
        let speed = 50.0;
        let mut times = arrivals(&sensors, &source, origin, speed);
        times[4] += 0.5;
        let options = SourceLocateOptions {
            loss: Loss::Huber,
            f_scale_s: 0.01,
            timing_sigma_s: 0.01,
            ..SourceLocateOptions::default()
        };

        let solution = locate_source(&sensors, &times, speed, &options).expect("solution");
        let worst = solution
            .per_sensor_influence
            .iter()
            .max_by(|a, b| a.score.total_cmp(&b.score))
            .expect("influence");
        assert_eq!(worst.sensor_index, 4);
        assert!(worst.loss_weight < 0.05);
    }

    #[test]
    fn degenerate_collinear_geometry_reports_singular_dop() {
        let sensors = vec![
            Sensor::new(vec![0.0, 0.0]),
            Sensor::new(vec![100.0, 0.0]),
            Sensor::new(vec![200.0, 0.0]),
            Sensor::new(vec![300.0, 0.0]),
        ];
        let err = source_dop(&sensors, &[50.0, 0.0], 300.0).expect_err("singular");
        assert!(matches!(
            err,
            SourceLocalizationError::Geometry(DopError::Singular)
        ));
    }
}
