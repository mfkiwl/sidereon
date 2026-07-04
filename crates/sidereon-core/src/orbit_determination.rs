//! SP3-anchored numerical orbit determination and residual ledgers.
//!
//! The fit estimates one inertial Cartesian initial state per satellite from a
//! batch of precise ephemeris samples. Input positions follow the SP3 convention:
//! ITRS/ECEF meters at scale-tagged epochs. They are transformed to GCRS
//! kilometers with the shared frame pipeline before the numerical propagator is
//! evaluated. Reported residuals are projected into the propagated state's RTN
//! frame and accumulated per satellite and per constellation.

use std::cell::RefCell;
use std::collections::BTreeMap;

use nalgebra::{DMatrix, DVector};

use crate::astro::covariance::{rtn_to_eci_rotation, RtnFrameError};
use crate::astro::error::PropagationError;
use crate::astro::forces::{DragParameters, SpaceWeatherSource};
use crate::astro::frames::transforms::{
    gcrs_to_itrs_compute, itrs_to_gcrs_compute, FrameTransformError,
};
use crate::astro::iod;
use crate::astro::math::least_squares::{
    self, singular_value_diagnostics, solve_trf_with, LeastSquaresProblem, SolveError,
    SolveOptions, Status, TrustRegionSolve,
};
use crate::astro::propagator::{
    ForceModelKind, IntegratorKind, IntegratorOptions, StatePropagator,
};
use crate::astro::state::CartesianState;
use crate::astro::time::civil::{civil_from_j2000_seconds, j2000_seconds_from_split};
use crate::astro::time::model::{Instant, TimeScale};
use crate::astro::time::scales::TimeScales;
use crate::constants::{M_PER_KM, SECONDS_PER_DAY};
use crate::geometry_quality::{classify, GeometryQuality, GeometryQualityThresholds};
use crate::sp3::{PreciseEphemerisSample, Sp3};
use crate::{GnssSatelliteId, GnssSystem};

const STATE_PARAM_COUNT: usize = 6;
const MIN_SEED_SAMPLES: usize = 2;
const DEFAULT_MIN_LEDGER_SAMPLES: usize = 3;

/// Options controlling a precise-orbit fit.
#[derive(Debug, Clone)]
pub struct OrbitFitOptions {
    /// Force model used by the numerical propagator.
    pub force_model: ForceModelKind,
    /// Integrator used by the numerical propagator.
    pub integrator: IntegratorKind,
    /// Step-size and tolerance controls for propagation.
    pub integrator_options: IntegratorOptions,
    /// Nonlinear least-squares stopping tolerances and evaluation budget.
    pub solver_options: SolveOptions,
    /// Dense subproblem solve used by the least-squares iteration.
    pub linear_solve: TrustRegionSolve,
    /// Geometry classifier thresholds for the final design matrix.
    pub geometry_thresholds: GeometryQualityThresholds,
    /// Minimum residual count before a ledger entry is no longer marked low-n.
    pub min_ledger_samples: usize,
    /// Optional atmospheric drag parameters layered on the selected force model.
    pub drag: Option<DragParameters>,
    /// Optional per-epoch space-weather source for drag.
    pub space_weather: Option<SpaceWeatherSource>,
}

impl Default for OrbitFitOptions {
    fn default() -> Self {
        Self {
            force_model: ForceModelKind::earth_phase_a(None),
            integrator: IntegratorKind::Dp54,
            integrator_options: IntegratorOptions::default(),
            solver_options: SolveOptions {
                gtol: 1.0e-12,
                ftol: 1.0e-12,
                xtol: 1.0e-12,
                max_nfev: 500,
            },
            linear_solve: TrustRegionSolve::OwnedGaussianFirstTie,
            geometry_thresholds: GeometryQualityThresholds::default(),
            min_ledger_samples: DEFAULT_MIN_LEDGER_SAMPLES,
            drag: None,
            space_weather: None,
        }
    }
}

/// State-covariance result for a fitted initial state.
#[derive(Debug, Clone, PartialEq)]
pub enum OrbitFitCovariance {
    /// Estimated row-major covariance for `[x_km, y_km, z_km, vx_km_s,
    /// vy_km_s, vz_km_s]`.
    Estimated {
        /// Row-major state covariance matrix.
        matrix: Box<[[f64; STATE_PARAM_COUNT]; STATE_PARAM_COUNT]>,
    },
    /// The arc has no positive residual degrees of freedom, so no finite
    /// residual-scaled covariance can be inferred.
    Unbounded,
}

/// Initial-state fit result for one satellite.
#[derive(Debug, Clone, PartialEq)]
pub struct OrbitFitSolution {
    /// Satellite fitted by this solution.
    pub satellite: GnssSatelliteId,
    /// Estimated inertial initial state.
    pub initial_state: CartesianState,
    /// Fitted state covariance, or an unbounded marker for short arcs.
    pub covariance: OrbitFitCovariance,
    /// Singular-value geometry diagnostics for the final design matrix.
    pub geometry_quality: GeometryQuality,
    /// Three-dimensional RMS residual of the automatically seeded state, meters.
    pub seed_rms_3d_m: f64,
    /// Three-dimensional RMS residual of the fitted state, meters.
    pub fit_rms_3d_m: f64,
    /// Accepted nonlinear least-squares iterations.
    pub iterations: usize,
}

/// Arc span covered by a residual ledger.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitArcSpan {
    /// Time scale shared by all residual epochs.
    pub time_scale: TimeScale,
    /// First residual epoch, seconds since J2000 in [`Self::time_scale`].
    pub start_j2000_s: f64,
    /// Last residual epoch, seconds since J2000 in [`Self::time_scale`].
    pub end_j2000_s: f64,
    /// `end_j2000_s - start_j2000_s`, seconds.
    pub duration_s: f64,
}

/// RTN residual RMS summary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitResidualStats {
    /// Radial RMS residual, meters.
    pub radial_rms_m: f64,
    /// Along-track RMS residual, meters.
    pub along_rms_m: f64,
    /// Cross-track RMS residual, meters.
    pub cross_rms_m: f64,
    /// Three-dimensional RMS residual, meters.
    pub rms_3d_m: f64,
    /// Number of residual epochs accumulated into this entry.
    pub n: usize,
    /// Whether `n < OrbitFitOptions::min_ledger_samples` for this run.
    pub low_sample_count: bool,
}

/// Residual RMS ledger, grouped by satellite and constellation.
#[derive(Debug, Clone, PartialEq)]
pub struct OrbitResidualLedger {
    /// Per-satellite RTN residual RMS values.
    pub per_sat: BTreeMap<GnssSatelliteId, OrbitResidualStats>,
    /// Per-constellation RTN residual RMS values.
    pub per_constellation: BTreeMap<GnssSystem, OrbitResidualStats>,
    /// Time span covered by all residuals in this ledger.
    pub arc_span: OrbitArcSpan,
}

/// Batch orbit-fit report for one or more satellites.
#[derive(Debug, Clone, PartialEq)]
pub struct OrbitFitReport {
    /// One fitted initial state per requested satellite.
    pub fits: BTreeMap<GnssSatelliteId, OrbitFitSolution>,
    /// RTN residual RMS ledger from the fitted states.
    pub ledger: OrbitResidualLedger,
}

/// Error returned by precise-orbit fitting.
#[derive(Debug, Clone, thiserror::Error)]
pub enum OrbitFitError {
    /// No satellite was requested.
    #[error("no satellites selected for precise-orbit fitting")]
    EmptySelection,
    /// An option value is outside its accepted domain.
    #[error("invalid orbit-fit {field}: {reason}")]
    InvalidOption {
        /// Option field name.
        field: &'static str,
        /// Validation failure reason.
        reason: &'static str,
    },
    /// A satellite does not have enough samples to seed a state.
    #[error("satellite {satellite} has {got} samples; need at least {required}")]
    TooFewSamples {
        /// Satellite being fitted.
        satellite: GnssSatelliteId,
        /// Number of samples available.
        got: usize,
        /// Required sample count.
        required: usize,
    },
    /// A satellite's epochs are not strictly increasing.
    #[error("satellite {satellite} sample epochs are not strictly increasing")]
    NonMonotonicEpochs {
        /// Satellite being fitted.
        satellite: GnssSatelliteId,
    },
    /// Samples selected for one batch carry more than one time scale.
    #[error("precise-orbit fit samples carry mixed time scales")]
    MixedTimeScales,
    /// A sample epoch cannot be represented on the J2000 axis or time-scale
    /// bridge.
    #[error("satellite {satellite} has an invalid epoch: {reason}")]
    InvalidEpoch {
        /// Satellite being fitted.
        satellite: GnssSatelliteId,
        /// Validation failure reason.
        reason: String,
    },
    /// A sample position or fit state was non-finite.
    #[error("satellite {satellite} has an invalid observation: {reason}")]
    InvalidObservation {
        /// Satellite being fitted.
        satellite: GnssSatelliteId,
        /// Validation failure reason.
        reason: &'static str,
    },
    /// A frame conversion failed.
    #[error("satellite {satellite} frame transform failed: {source}")]
    Frame {
        /// Satellite being fitted.
        satellite: GnssSatelliteId,
        /// Source frame-transform error.
        source: FrameTransformError,
    },
    /// Propagation failed during the fit.
    #[error("satellite {satellite} propagation failed: {source}")]
    Propagation {
        /// Satellite being fitted.
        satellite: GnssSatelliteId,
        /// Source propagation error.
        source: PropagationError,
    },
    /// Least-squares failed before a usable fit report was produced.
    #[error("satellite {satellite} least-squares failed: {source}")]
    LeastSquares {
        /// Satellite being fitted.
        satellite: GnssSatelliteId,
        /// Source least-squares error.
        source: SolveError,
    },
    /// The final design matrix is rank deficient.
    #[error("satellite {satellite} has rank-deficient fit geometry")]
    SingularGeometry {
        /// Satellite being fitted.
        satellite: GnssSatelliteId,
        /// Geometry diagnostics from the singular design.
        geometry_quality: GeometryQuality,
    },
    /// The nonlinear least-squares iteration exhausted its evaluation budget.
    #[error("satellite {satellite} fit did not converge after {iterations} iterations")]
    DidNotConverge {
        /// Satellite being fitted.
        satellite: GnssSatelliteId,
        /// Accepted iterations before termination.
        iterations: usize,
    },
    /// The RTN frame was undefined for a propagated state.
    #[error("satellite {satellite} RTN frame failed: {reason:?}")]
    RtnFrame {
        /// Satellite being fitted.
        satellite: GnssSatelliteId,
        /// RTN-frame failure reason.
        reason: RtnFrameError,
    },
}

/// Fit one satellite from a parsed SP3 product.
pub fn fit_sp3_precise_orbit(
    product: &Sp3,
    satellite: GnssSatelliteId,
    options: &OrbitFitOptions,
) -> Result<OrbitFitReport, OrbitFitError> {
    fit_sp3_precise_orbits(product, &[satellite], options)
}

/// Fit one satellite from a parsed SP3 product with a caller-supplied arc-start
/// initial-state seed.
pub fn fit_sp3_precise_orbit_with_initial_state(
    product: &Sp3,
    satellite: GnssSatelliteId,
    initial_state: CartesianState,
    options: &OrbitFitOptions,
) -> Result<OrbitFitReport, OrbitFitError> {
    let samples = product.precise_ephemeris_samples();
    fit_precise_ephemeris_sample_orbit_with_initial_state(
        &samples,
        satellite,
        initial_state,
        options,
    )
}

/// Fit selected satellites from a parsed SP3 product.
pub fn fit_sp3_precise_orbits(
    product: &Sp3,
    satellites: &[GnssSatelliteId],
    options: &OrbitFitOptions,
) -> Result<OrbitFitReport, OrbitFitError> {
    let samples = product.precise_ephemeris_samples();
    fit_precise_ephemeris_sample_orbits(&samples, satellites, options)
}

/// Fit every satellite declared in a parsed SP3 product.
pub fn fit_all_sp3_precise_orbits(
    product: &Sp3,
    options: &OrbitFitOptions,
) -> Result<OrbitFitReport, OrbitFitError> {
    fit_sp3_precise_orbits(product, product.satellites(), options)
}

/// Fit one satellite from precise ephemeris samples.
pub fn fit_precise_ephemeris_sample_orbit(
    samples: &[PreciseEphemerisSample],
    satellite: GnssSatelliteId,
    options: &OrbitFitOptions,
) -> Result<OrbitFitReport, OrbitFitError> {
    fit_precise_ephemeris_sample_orbits(samples, &[satellite], options)
}

/// Fit one satellite from precise ephemeris samples with a caller-supplied
/// arc-start initial-state seed.
pub fn fit_precise_ephemeris_sample_orbit_with_initial_state(
    samples: &[PreciseEphemerisSample],
    satellite: GnssSatelliteId,
    initial_state: CartesianState,
    options: &OrbitFitOptions,
) -> Result<OrbitFitReport, OrbitFitError> {
    validate_options(options)?;
    let work = fit_one_sample_arc(samples, satellite, options, Some(initial_state))?;
    let time_scale = work
        .residuals
        .first()
        .map(|residual| residual.time_scale)
        .ok_or(OrbitFitError::EmptySelection)?;
    let ledger = build_ledger(work.residuals, time_scale, options.min_ledger_samples)?;
    let mut fits = BTreeMap::new();
    fits.insert(satellite, work.solution);
    Ok(OrbitFitReport { fits, ledger })
}

/// Fit selected satellites from precise ephemeris samples.
pub fn fit_precise_ephemeris_sample_orbits(
    samples: &[PreciseEphemerisSample],
    satellites: &[GnssSatelliteId],
    options: &OrbitFitOptions,
) -> Result<OrbitFitReport, OrbitFitError> {
    validate_options(options)?;
    if satellites.is_empty() {
        return Err(OrbitFitError::EmptySelection);
    }

    let mut fits = BTreeMap::new();
    let mut residuals = Vec::new();
    let mut time_scale = None;
    for &satellite in satellites {
        let work = fit_one_sample_arc(samples, satellite, options, None)?;
        for residual in &work.residuals {
            match time_scale {
                None => time_scale = Some(residual.time_scale),
                Some(scale) if scale == residual.time_scale => {}
                Some(_) => return Err(OrbitFitError::MixedTimeScales),
            }
        }
        residuals.extend(work.residuals);
        fits.insert(satellite, work.solution);
    }

    let ledger = build_ledger(
        residuals,
        time_scale.ok_or(OrbitFitError::EmptySelection)?,
        options.min_ledger_samples,
    )?;
    Ok(OrbitFitReport { fits, ledger })
}

fn validate_options(options: &OrbitFitOptions) -> Result<(), OrbitFitError> {
    if options.min_ledger_samples == 0 {
        return Err(OrbitFitError::InvalidOption {
            field: "min_ledger_samples",
            reason: "not positive",
        });
    }
    Ok(())
}

struct FitWork {
    solution: OrbitFitSolution,
    residuals: Vec<RtnResidual>,
}

fn fit_one_sample_arc(
    samples: &[PreciseEphemerisSample],
    satellite: GnssSatelliteId,
    options: &OrbitFitOptions,
    initial_seed: Option<CartesianState>,
) -> Result<FitWork, OrbitFitError> {
    let observations = collect_observations(samples, satellite)?;
    let seed = match initial_seed {
        Some(seed) => validate_initial_seed(satellite, seed, &observations)?,
        None => seed_initial_state(satellite, &observations, options)?,
    };
    let seed_vector = state_to_vector(seed);
    let param_scales = parameter_scales(&seed_vector);
    let seed_residual =
        residual_vector_for_params(satellite, &seed_vector, &observations, options)?;
    let seed_rms_3d_m = residual_rms_3d_m(seed_residual.as_slice());

    let residual_error = RefCell::new(None);
    let observations_for_closure = observations.clone();
    let residual = |x: &DVector<f64>| -> DVector<f64> {
        let physical = unscale_params(x.as_slice(), &param_scales);
        match residual_vector_for_params(satellite, &physical, &observations_for_closure, options) {
            Ok(values) => DVector::from_vec(values),
            Err(error) => {
                *residual_error.borrow_mut() = Some(error);
                DVector::from_element(observations_for_closure.len() * 3, f64::NAN)
            }
        }
    };

    let problem = LeastSquaresProblem::new(
        residual,
        DVector::from_vec(scale_params(&seed_vector, &param_scales).to_vec()),
    );
    let report = match solve_trf_with(&problem, &options.solver_options, options.linear_solve) {
        Ok(report) => report,
        Err(SolveError::SingularJacobian) => {
            let geometry_quality = singular_geometry_quality(observations.len(), options);
            return Err(OrbitFitError::SingularGeometry {
                satellite,
                geometry_quality,
            });
        }
        Err(error) => {
            if let Some(source) = residual_error.into_inner() {
                return Err(source);
            }
            return Err(OrbitFitError::LeastSquares {
                satellite,
                source: error,
            });
        }
    };

    if matches!(report.status, Status::MaxEvaluations) {
        return Err(OrbitFitError::DidNotConverge {
            satellite,
            iterations: report.iterations,
        });
    }

    let physical_jacobian = physical_jacobian(&report.jacobian, &param_scales);
    let geometry_quality = classify_fit_geometry(&physical_jacobian, options);
    if geometry_quality.rank < STATE_PARAM_COUNT {
        return Err(OrbitFitError::SingularGeometry {
            satellite,
            geometry_quality,
        });
    }

    let covariance = fit_covariance(satellite, &physical_jacobian, report.cost)?;
    let final_params = unscale_params(report.x.as_slice(), &param_scales);
    let initial_state = CartesianState::new(
        observations[0].epoch_j2000_s,
        [final_params[0], final_params[1], final_params[2]],
        [final_params[3], final_params[4], final_params[5]],
    );
    let fit_residuals = rtn_residuals_for_state(satellite, initial_state, &observations, options)?;
    let fit_rms_3d_m = ledger_rms_3d_m(&fit_residuals);

    Ok(FitWork {
        solution: OrbitFitSolution {
            satellite,
            initial_state,
            covariance,
            geometry_quality,
            seed_rms_3d_m,
            fit_rms_3d_m,
            iterations: report.iterations,
        },
        residuals: fit_residuals,
    })
}

fn fit_covariance(
    satellite: GnssSatelliteId,
    jacobian: &DMatrix<f64>,
    cost: f64,
) -> Result<OrbitFitCovariance, OrbitFitError> {
    if jacobian.nrows() <= jacobian.ncols() {
        return Ok(OrbitFitCovariance::Unbounded);
    }
    let covariance = least_squares::covariance_from_jacobian(jacobian, cost)
        .map_err(|source| OrbitFitError::LeastSquares { satellite, source })?;
    Ok(OrbitFitCovariance::Estimated {
        matrix: Box::new(matrix6(&covariance)),
    })
}

#[derive(Debug, Clone)]
struct OrbitObservation {
    epoch_j2000_s: f64,
    time_scale: TimeScale,
    time_scales: TimeScales,
    observed_itrs_km: [f64; 3],
    observed_gcrs_km: [f64; 3],
}

fn collect_observations(
    samples: &[PreciseEphemerisSample],
    satellite: GnssSatelliteId,
) -> Result<Vec<OrbitObservation>, OrbitFitError> {
    let mut observations = Vec::new();
    for sample in samples.iter().filter(|sample| sample.sat == satellite) {
        validate_position(sample.position_ecef_m, satellite)?;
        let epoch_j2000_s = instant_j2000_seconds(sample.epoch, satellite)?;
        let ts = time_scales_from_instant(sample.epoch, epoch_j2000_s, satellite)?;
        let [x_m, y_m, z_m] = sample.position_ecef_m;
        let (x, y, z) = itrs_to_gcrs_compute(x_m / M_PER_KM, y_m / M_PER_KM, z_m / M_PER_KM, &ts)
            .map_err(|source| OrbitFitError::Frame { satellite, source })?;
        observations.push(OrbitObservation {
            epoch_j2000_s,
            time_scale: sample.epoch.scale,
            time_scales: ts,
            observed_itrs_km: [x_m / M_PER_KM, y_m / M_PER_KM, z_m / M_PER_KM],
            observed_gcrs_km: [x, y, z],
        });
    }
    observations.sort_by(|a, b| a.epoch_j2000_s.total_cmp(&b.epoch_j2000_s));
    if observations.len() < MIN_SEED_SAMPLES {
        return Err(OrbitFitError::TooFewSamples {
            satellite,
            got: observations.len(),
            required: MIN_SEED_SAMPLES,
        });
    }
    if observations
        .windows(2)
        .any(|window| window[1].epoch_j2000_s <= window[0].epoch_j2000_s)
    {
        return Err(OrbitFitError::NonMonotonicEpochs { satellite });
    }
    if observations
        .windows(2)
        .any(|window| window[1].time_scale != window[0].time_scale)
    {
        return Err(OrbitFitError::MixedTimeScales);
    }
    Ok(observations)
}

fn validate_position(
    position_ecef_m: [f64; 3],
    satellite: GnssSatelliteId,
) -> Result<(), OrbitFitError> {
    if position_ecef_m.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(OrbitFitError::InvalidObservation {
            satellite,
            reason: "position components must be finite",
        })
    }
}

fn instant_j2000_seconds(
    instant: Instant,
    satellite: GnssSatelliteId,
) -> Result<f64, OrbitFitError> {
    let jd = instant
        .julian_date()
        .ok_or_else(|| OrbitFitError::InvalidEpoch {
            satellite,
            reason: "epoch is not a split Julian date".to_string(),
        })?;
    let seconds = j2000_seconds_from_split(jd.jd_whole, jd.fraction);
    if seconds.is_finite() {
        Ok(seconds)
    } else {
        Err(OrbitFitError::InvalidEpoch {
            satellite,
            reason: "J2000 seconds are not finite".to_string(),
        })
    }
}

fn time_scales_from_instant(
    instant: Instant,
    epoch_j2000_s: f64,
    satellite: GnssSatelliteId,
) -> Result<TimeScales, OrbitFitError> {
    let whole = epoch_j2000_s.floor();
    if whole < i64::MIN as f64 || whole > i64::MAX as f64 {
        return Err(OrbitFitError::InvalidEpoch {
            satellite,
            reason: "J2000 seconds are outside calendar range".to_string(),
        });
    }
    let fraction = epoch_j2000_s - whole;
    let (year, month, day, hour, minute, second) = civil_from_j2000_seconds(whole as i64);
    TimeScales::from_scale(
        instant.scale,
        year as i32,
        month as i32,
        day as i32,
        hour as i32,
        minute as i32,
        second as f64 + fraction,
    )
    .map_err(|error| OrbitFitError::InvalidEpoch {
        satellite,
        reason: error.to_string(),
    })
}

fn seed_initial_state(
    satellite: GnssSatelliteId,
    observations: &[OrbitObservation],
    options: &OrbitFitOptions,
) -> Result<CartesianState, OrbitFitError> {
    if observations.len() >= 3 {
        let r1 = observations[0].observed_gcrs_km;
        let r2 = observations[1].observed_gcrs_km;
        let r3 = observations[2].observed_gcrs_km;
        let jd1 = observations[0].epoch_j2000_s / SECONDS_PER_DAY;
        let jd2 = observations[1].epoch_j2000_s / SECONDS_PER_DAY;
        let jd3 = observations[2].epoch_j2000_s / SECONDS_PER_DAY;
        if let Ok((v2, _, _, _)) = iod::hgibbs(&r1, &r2, &r3, jd1, jd2, jd3) {
            let midpoint = CartesianState::new(observations[1].epoch_j2000_s, r2, v2);
            if let Ok(result) =
                build_propagator(midpoint, options).propagate_to(observations[0].epoch_j2000_s)
            {
                return Ok(result.final_state);
            }
        }
    }

    let first = &observations[0];
    let second = &observations[1];
    let dt = second.epoch_j2000_s - first.epoch_j2000_s;
    if !dt.is_finite() || dt <= 0.0 {
        return Err(OrbitFitError::NonMonotonicEpochs { satellite });
    }
    let velocity = [
        (second.observed_gcrs_km[0] - first.observed_gcrs_km[0]) / dt,
        (second.observed_gcrs_km[1] - first.observed_gcrs_km[1]) / dt,
        (second.observed_gcrs_km[2] - first.observed_gcrs_km[2]) / dt,
    ];
    Ok(CartesianState::new(
        first.epoch_j2000_s,
        first.observed_gcrs_km,
        velocity,
    ))
}

fn validate_initial_seed(
    satellite: GnssSatelliteId,
    seed: CartesianState,
    observations: &[OrbitObservation],
) -> Result<CartesianState, OrbitFitError> {
    if seed.epoch_tdb_seconds != observations[0].epoch_j2000_s {
        return Err(OrbitFitError::InvalidEpoch {
            satellite,
            reason: "initial-state seed epoch must match the first sample".to_string(),
        });
    }
    let params = state_to_vector(seed);
    if params.iter().all(|value| value.is_finite()) {
        Ok(seed)
    } else {
        Err(OrbitFitError::InvalidObservation {
            satellite,
            reason: "initial-state seed components must be finite",
        })
    }
}

fn state_to_vector(state: CartesianState) -> [f64; STATE_PARAM_COUNT] {
    [
        state.position_km.x,
        state.position_km.y,
        state.position_km.z,
        state.velocity_km_s.x,
        state.velocity_km_s.y,
        state.velocity_km_s.z,
    ]
}

fn parameter_scales(params: &[f64; STATE_PARAM_COUNT]) -> [f64; STATE_PARAM_COUNT] {
    let position_scale = (params[0] * params[0] + params[1] * params[1] + params[2] * params[2])
        .sqrt()
        .max(1.0);
    let velocity_scale = (params[3] * params[3] + params[4] * params[4] + params[5] * params[5])
        .sqrt()
        .max(1.0);
    [
        position_scale,
        position_scale,
        position_scale,
        velocity_scale,
        velocity_scale,
        velocity_scale,
    ]
}

fn scale_params(
    params: &[f64; STATE_PARAM_COUNT],
    scales: &[f64; STATE_PARAM_COUNT],
) -> [f64; STATE_PARAM_COUNT] {
    [
        params[0] / scales[0],
        params[1] / scales[1],
        params[2] / scales[2],
        params[3] / scales[3],
        params[4] / scales[4],
        params[5] / scales[5],
    ]
}

fn unscale_params(params: &[f64], scales: &[f64; STATE_PARAM_COUNT]) -> [f64; STATE_PARAM_COUNT] {
    [
        params[0] * scales[0],
        params[1] * scales[1],
        params[2] * scales[2],
        params[3] * scales[3],
        params[4] * scales[4],
        params[5] * scales[5],
    ]
}

fn physical_jacobian(
    scaled_jacobian: &DMatrix<f64>,
    scales: &[f64; STATE_PARAM_COUNT],
) -> DMatrix<f64> {
    let mut jacobian = scaled_jacobian.clone();
    for col in 0..STATE_PARAM_COUNT {
        for row in 0..jacobian.nrows() {
            jacobian[(row, col)] /= scales[col];
        }
    }
    jacobian
}

fn residual_vector_for_params(
    satellite: GnssSatelliteId,
    params: &[f64],
    observations: &[OrbitObservation],
    options: &OrbitFitOptions,
) -> Result<Vec<f64>, OrbitFitError> {
    if params.len() != STATE_PARAM_COUNT {
        return Err(OrbitFitError::InvalidObservation {
            satellite,
            reason: "state parameter length mismatch",
        });
    }
    if !params.iter().all(|value| value.is_finite()) {
        return Err(OrbitFitError::InvalidObservation {
            satellite,
            reason: "state parameters must be finite",
        });
    }
    let initial = CartesianState::new(
        observations[0].epoch_j2000_s,
        [params[0], params[1], params[2]],
        [params[3], params[4], params[5]],
    );
    let states = propagate_to_observations(satellite, initial, observations, options)?;
    let mut residual = Vec::with_capacity(observations.len() * 3);
    for (state, observation) in states.iter().zip(observations) {
        let predicted_itrs = gcrs_to_itrs_compute(
            state.position_km.x,
            state.position_km.y,
            state.position_km.z,
            &observation.time_scales,
            false,
        )
        .map_err(|source| OrbitFitError::Frame { satellite, source })?;
        residual.push(predicted_itrs.0 - observation.observed_itrs_km[0]);
        residual.push(predicted_itrs.1 - observation.observed_itrs_km[1]);
        residual.push(predicted_itrs.2 - observation.observed_itrs_km[2]);
    }
    Ok(residual)
}

fn propagate_to_observations(
    satellite: GnssSatelliteId,
    initial: CartesianState,
    observations: &[OrbitObservation],
    options: &OrbitFitOptions,
) -> Result<Vec<CartesianState>, OrbitFitError> {
    let epochs: Vec<f64> = observations
        .iter()
        .map(|observation| observation.epoch_j2000_s)
        .collect();
    build_propagator(initial, options)
        .ephemeris(&epochs)
        .map_err(|source| OrbitFitError::Propagation { satellite, source })
}

fn build_propagator(initial: CartesianState, options: &OrbitFitOptions) -> StatePropagator {
    StatePropagator {
        initial,
        force_model: options.force_model,
        integrator: options.integrator,
        options: options.integrator_options,
        drag: options.drag,
        space_weather: options.space_weather.clone(),
    }
}

fn residual_rms_3d_m(residual_km: &[f64]) -> f64 {
    let n = residual_km.len() / 3;
    let sumsq_m2 = residual_km
        .iter()
        .map(|value| {
            let meters = value * M_PER_KM;
            meters * meters
        })
        .sum::<f64>();
    (sumsq_m2 / n as f64).sqrt()
}

fn singular_geometry_quality(
    observation_count: usize,
    options: &OrbitFitOptions,
) -> GeometryQuality {
    classify(
        0,
        STATE_PARAM_COUNT,
        observation_count as i32 * 3 - STATE_PARAM_COUNT as i32,
        f64::INFINITY,
        f64::INFINITY,
        false,
        options.geometry_thresholds,
    )
}

fn classify_fit_geometry(jacobian: &DMatrix<f64>, options: &OrbitFitOptions) -> GeometryQuality {
    let singular = jacobian.clone().svd(false, false).singular_values;
    let diagnostics =
        singular_value_diagnostics(singular.as_slice(), jacobian.nrows(), jacobian.ncols());
    let gdop = least_squares::normal_covariance(jacobian, 1.0)
        .map(|cofactor| {
            (0..cofactor.nrows())
                .map(|index| cofactor[(index, index)])
                .sum::<f64>()
                .sqrt()
        })
        .unwrap_or(f64::INFINITY);
    classify(
        diagnostics.rank,
        STATE_PARAM_COUNT,
        jacobian.nrows() as i32 - STATE_PARAM_COUNT as i32,
        diagnostics.condition_number,
        gdop,
        false,
        options.geometry_thresholds,
    )
}

fn matrix6(matrix: &DMatrix<f64>) -> [[f64; STATE_PARAM_COUNT]; STATE_PARAM_COUNT] {
    let mut out = [[0.0_f64; STATE_PARAM_COUNT]; STATE_PARAM_COUNT];
    for row in 0..STATE_PARAM_COUNT {
        for col in 0..STATE_PARAM_COUNT {
            out[row][col] = matrix[(row, col)];
        }
    }
    out
}

#[derive(Debug, Clone, Copy)]
struct RtnResidual {
    satellite: GnssSatelliteId,
    time_scale: TimeScale,
    epoch_j2000_s: f64,
    radial_m: f64,
    along_m: f64,
    cross_m: f64,
}

fn rtn_residuals_for_state(
    satellite: GnssSatelliteId,
    initial: CartesianState,
    observations: &[OrbitObservation],
    options: &OrbitFitOptions,
) -> Result<Vec<RtnResidual>, OrbitFitError> {
    let states = propagate_to_observations(satellite, initial, observations, options)?;
    let mut residuals = Vec::with_capacity(observations.len());
    for (state, observation) in states.iter().zip(observations) {
        let rot = rtn_to_eci_rotation(state.position_array(), state.velocity_array())
            .map_err(|reason| OrbitFitError::RtnFrame { satellite, reason })?;
        let predicted_itrs = gcrs_to_itrs_compute(
            state.position_km.x,
            state.position_km.y,
            state.position_km.z,
            &observation.time_scales,
            false,
        )
        .map_err(|source| OrbitFitError::Frame { satellite, source })?;
        let diff_itrs = [
            predicted_itrs.0 - observation.observed_itrs_km[0],
            predicted_itrs.1 - observation.observed_itrs_km[1],
            predicted_itrs.2 - observation.observed_itrs_km[2],
        ];
        let diff_gcrs = itrs_to_gcrs_compute(
            diff_itrs[0],
            diff_itrs[1],
            diff_itrs[2],
            &observation.time_scales,
        )
        .map_err(|source| OrbitFitError::Frame { satellite, source })?;
        let diff = [diff_gcrs.0, diff_gcrs.1, diff_gcrs.2];
        let radial_km = diff[0] * rot[0][0] + diff[1] * rot[1][0] + diff[2] * rot[2][0];
        let along_km = diff[0] * rot[0][1] + diff[1] * rot[1][1] + diff[2] * rot[2][1];
        let cross_km = diff[0] * rot[0][2] + diff[1] * rot[1][2] + diff[2] * rot[2][2];
        residuals.push(RtnResidual {
            satellite,
            time_scale: observation.time_scale,
            epoch_j2000_s: observation.epoch_j2000_s,
            radial_m: radial_km * M_PER_KM,
            along_m: along_km * M_PER_KM,
            cross_m: cross_km * M_PER_KM,
        });
    }
    Ok(residuals)
}

fn ledger_rms_3d_m(residuals: &[RtnResidual]) -> f64 {
    let mut sumsq = 0.0;
    for residual in residuals {
        sumsq += residual.radial_m * residual.radial_m;
        sumsq += residual.along_m * residual.along_m;
        sumsq += residual.cross_m * residual.cross_m;
    }
    (sumsq / residuals.len() as f64).sqrt()
}

#[derive(Default)]
struct ResidualAccum {
    radial_sumsq_m2: f64,
    along_sumsq_m2: f64,
    cross_sumsq_m2: f64,
    n: usize,
}

impl ResidualAccum {
    fn push(&mut self, residual: RtnResidual) {
        self.radial_sumsq_m2 += residual.radial_m * residual.radial_m;
        self.along_sumsq_m2 += residual.along_m * residual.along_m;
        self.cross_sumsq_m2 += residual.cross_m * residual.cross_m;
        self.n += 1;
    }

    fn finish(&self, min_ledger_samples: usize) -> OrbitResidualStats {
        let n = self.n as f64;
        OrbitResidualStats {
            radial_rms_m: (self.radial_sumsq_m2 / n).sqrt(),
            along_rms_m: (self.along_sumsq_m2 / n).sqrt(),
            cross_rms_m: (self.cross_sumsq_m2 / n).sqrt(),
            rms_3d_m: ((self.radial_sumsq_m2 + self.along_sumsq_m2 + self.cross_sumsq_m2) / n)
                .sqrt(),
            n: self.n,
            low_sample_count: self.n < min_ledger_samples,
        }
    }
}

fn build_ledger(
    residuals: Vec<RtnResidual>,
    time_scale: TimeScale,
    min_ledger_samples: usize,
) -> Result<OrbitResidualLedger, OrbitFitError> {
    if residuals.is_empty() {
        return Err(OrbitFitError::EmptySelection);
    }
    let mut per_sat_accum: BTreeMap<GnssSatelliteId, ResidualAccum> = BTreeMap::new();
    let mut per_constellation_accum: BTreeMap<GnssSystem, ResidualAccum> = BTreeMap::new();
    let mut start = f64::INFINITY;
    let mut end = f64::NEG_INFINITY;
    for residual in residuals {
        start = start.min(residual.epoch_j2000_s);
        end = end.max(residual.epoch_j2000_s);
        per_sat_accum
            .entry(residual.satellite)
            .or_default()
            .push(residual);
        per_constellation_accum
            .entry(residual.satellite.system)
            .or_default()
            .push(residual);
    }

    let per_sat = per_sat_accum
        .iter()
        .map(|(&sat, accum)| (sat, accum.finish(min_ledger_samples)))
        .collect();
    let per_constellation = per_constellation_accum
        .iter()
        .map(|(&system, accum)| (system, accum.finish(min_ledger_samples)))
        .collect();

    Ok(OrbitResidualLedger {
        per_sat,
        per_constellation,
        arc_span: OrbitArcSpan {
            time_scale,
            start_j2000_s: start,
            end_j2000_s: end,
            duration_s: end - start,
        },
    })
}
