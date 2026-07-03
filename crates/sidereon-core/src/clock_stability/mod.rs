//! Clock-stability estimators from IEEE Std 1139-2008 and NIST SP 1065.
//!
//! The natural RINEX receiver-clock phase input is
//! `crates/sidereon-core/src/rinex_obs/mod.rs:ObsEpoch::rcv_clock_offset_s`.
//! [`receiver_clock_phase_deviations`] exposes that field as a phase-deviation
//! series in seconds.
//!
//! Gap handling is explicit. [`GapPolicy::Reject`] requires a contiguous series.
//! [`GapPolicy::OmitTerms`] keeps the series indexed and excludes every estimator
//! term whose required phase samples cross a missing sample.

use crate::rinex::observations::RinexObs;

const SQRT_3: f64 = 1.732_050_807_568_877_2;

/// Tagged input samples for Allan-family estimators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AllanSeries<'a> {
    /// Phase deviations in seconds, sampled every `tau0_s`.
    PhaseSeconds(&'a [f64]),
    /// Fractional-frequency samples, dimensionless, sampled every `tau0_s`.
    FractionalFrequency(&'a [f64]),
    /// Phase deviations in seconds with missing samples.
    PhaseSecondsWithGaps(&'a [Option<f64>]),
    /// Fractional-frequency samples with missing samples.
    FractionalFrequencyWithGaps(&'a [Option<f64>]),
}

/// Averaging-factor grid for requested estimator points.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TauGrid {
    /// `m = 1, 2, 4, 8, ...` while the estimator has at least one term.
    #[default]
    Octave,
    /// `m = 1..=m_max` while the estimator has at least one term.
    All,
    /// Caller-supplied averaging factors.
    Explicit(Vec<usize>),
}

/// Missing-sample policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GapPolicy {
    /// Reject any missing sample before estimation.
    #[default]
    Reject,
    /// Exclude estimator terms whose phase samples cross a missing sample.
    OmitTerms,
}

/// Estimators available through [`compute_allan_deviations`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllanEstimatorSet {
    /// Plain non-overlapping Allan deviation.
    pub adev: bool,
    /// Fully overlapping Allan deviation.
    pub overlapping_adev: bool,
    /// Modified Allan deviation.
    pub mdev: bool,
    /// Overlapping Hadamard deviation.
    pub hdev: bool,
    /// Time deviation derived from MDEV.
    pub tdev: bool,
}

impl AllanEstimatorSet {
    /// No estimators selected.
    pub const fn none() -> Self {
        Self {
            adev: false,
            overlapping_adev: false,
            mdev: false,
            hdev: false,
            tdev: false,
        }
    }

    /// Standard overlapping estimators plus TDEV.
    pub const fn standard() -> Self {
        Self {
            adev: false,
            overlapping_adev: true,
            mdev: true,
            hdev: true,
            tdev: true,
        }
    }

    /// Every implemented estimator.
    pub const fn all() -> Self {
        Self {
            adev: true,
            overlapping_adev: true,
            mdev: true,
            hdev: true,
            tdev: true,
        }
    }

    fn is_empty(self) -> bool {
        !self.adev && !self.overlapping_adev && !self.mdev && !self.hdev && !self.tdev
    }
}

impl Default for AllanEstimatorSet {
    fn default() -> Self {
        Self::standard()
    }
}

/// Options for the combined Allan-family driver.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AllanOptions {
    /// Which estimators to compute.
    pub estimators: AllanEstimatorSet,
    /// Averaging-factor grid.
    pub tau_grid: TauGrid,
    /// Missing-sample policy.
    pub gap_policy: GapPolicy,
}

/// Input package for [`compute_allan_deviations`].
#[derive(Debug, Clone, PartialEq)]
pub struct AllanInput<'a> {
    /// Tagged sample series.
    pub series: AllanSeries<'a>,
    /// Basic sampling interval in seconds.
    pub tau0_s: f64,
    /// Estimator, tau-grid, and gap options.
    pub options: AllanOptions,
}

/// One estimator curve.
#[derive(Debug, Clone, PartialEq)]
pub struct AllanResult {
    /// Averaging times, seconds.
    pub tau_s: Vec<f64>,
    /// Deviation value for each averaging time.
    pub deviation: Vec<f64>,
    /// Number of estimator terms used at each averaging time.
    pub n: Vec<usize>,
}

impl AllanResult {
    fn new() -> Self {
        Self {
            tau_s: Vec::new(),
            deviation: Vec::new(),
            n: Vec::new(),
        }
    }

    fn push(&mut self, tau_s: f64, deviation: f64, n: usize) {
        self.tau_s.push(tau_s);
        self.deviation.push(deviation);
        self.n.push(n);
    }

    /// Number of tau points in the curve.
    pub fn len(&self) -> usize {
        self.tau_s.len()
    }

    /// Whether the curve has no tau points.
    pub fn is_empty(&self) -> bool {
        self.tau_s.is_empty()
    }
}

/// Combined output from [`compute_allan_deviations`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AllanDeviationCurves {
    /// Plain non-overlapping Allan deviation, if requested.
    pub adev: Option<AllanResult>,
    /// Fully overlapping Allan deviation, if requested.
    pub overlapping_adev: Option<AllanResult>,
    /// Modified Allan deviation, if requested.
    pub mdev: Option<AllanResult>,
    /// Overlapping Hadamard deviation, if requested.
    pub hdev: Option<AllanResult>,
    /// Time deviation, if requested.
    pub tdev: Option<AllanResult>,
}

/// Estimator identifier used in errors and options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllanEstimator {
    /// Plain non-overlapping Allan deviation.
    Adev,
    /// Fully overlapping Allan deviation.
    OverlappingAdev,
    /// Modified Allan deviation.
    Mdev,
    /// Overlapping Hadamard deviation.
    Hdev,
    /// Time deviation.
    Tdev,
}

impl AllanEstimator {
    fn label(self) -> &'static str {
        match self {
            Self::Adev => "ADEV",
            Self::OverlappingAdev => "OADEV",
            Self::Mdev => "MDEV",
            Self::Hdev => "HDEV",
            Self::Tdev => "TDEV",
        }
    }
}

/// Error from Allan-family estimator setup or evaluation.
#[derive(Debug, Clone, PartialEq)]
pub enum AllanError {
    /// The input series has no samples.
    EmptySeries,
    /// The basic sampling interval is not finite and positive.
    InvalidTau0 { tau0_s: f64 },
    /// No estimator was requested.
    NoEstimators,
    /// An explicit tau grid was empty.
    EmptyTauGrid,
    /// Averaging factors must be positive.
    InvalidAveragingFactor { averaging_factor: usize },
    /// A requested estimator has no valid term for this sample count.
    TooFewSamples {
        estimator: AllanEstimator,
        averaging_factor: usize,
        available_phase_samples: usize,
    },
    /// A sample was not finite.
    NonFiniteSample { index: usize },
    /// A missing sample was present under [`GapPolicy::Reject`].
    Gap { index: usize },
    /// The computed tau was not finite.
    NonFiniteTau {
        estimator: AllanEstimator,
        averaging_factor: usize,
    },
    /// The computed deviation was not finite.
    NonFiniteDeviation {
        estimator: AllanEstimator,
        averaging_factor: usize,
    },
}

impl core::fmt::Display for AllanError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptySeries => write!(f, "Allan series is empty"),
            Self::InvalidTau0 { tau0_s } => {
                write!(f, "tau0_s must be finite and positive, got {tau0_s}")
            }
            Self::NoEstimators => write!(f, "no Allan estimators selected"),
            Self::EmptyTauGrid => write!(f, "explicit tau grid is empty"),
            Self::InvalidAveragingFactor { averaging_factor } => {
                write!(
                    f,
                    "averaging factor must be positive, got {averaging_factor}"
                )
            }
            Self::TooFewSamples {
                estimator,
                averaging_factor,
                available_phase_samples,
            } => write!(
                f,
                "{} has no valid terms for averaging factor {} with {} phase samples",
                estimator.label(),
                averaging_factor,
                available_phase_samples
            ),
            Self::NonFiniteSample { index } => {
                write!(f, "sample {index} is not finite")
            }
            Self::Gap { index } => {
                write!(f, "sample {index} is missing")
            }
            Self::NonFiniteTau {
                estimator,
                averaging_factor,
            } => write!(
                f,
                "{} tau is not finite for averaging factor {}",
                estimator.label(),
                averaging_factor
            ),
            Self::NonFiniteDeviation {
                estimator,
                averaging_factor,
            } => write!(
                f,
                "{} deviation is not finite for averaging factor {}",
                estimator.label(),
                averaging_factor
            ),
        }
    }
}

impl std::error::Error for AllanError {}

/// Extract RINEX receiver-clock offsets as phase deviations in seconds.
///
/// Event epochs (`flag > 1`) are returned as gaps. The source field is
/// `crates/sidereon-core/src/rinex_obs/mod.rs:ObsEpoch::rcv_clock_offset_s`.
pub fn receiver_clock_phase_deviations(obs: &RinexObs) -> Vec<Option<f64>> {
    obs.epochs()
        .iter()
        .map(|epoch| {
            if epoch.flag > 1 {
                None
            } else {
                epoch.rcv_clock_offset_s
            }
        })
        .collect()
}

/// Compute the requested Allan-family curves.
pub fn compute_allan_deviations(
    input: &AllanInput<'_>,
) -> Result<AllanDeviationCurves, AllanError> {
    if input.options.estimators.is_empty() {
        return Err(AllanError::NoEstimators);
    }

    let phase = prepare_phase(input.series, input.tau0_s, input.options.gap_policy)?;
    let mut curves = AllanDeviationCurves::default();

    if input.options.estimators.adev {
        curves.adev = Some(compute_curve(
            &phase,
            input.tau0_s,
            &input.options.tau_grid,
            AllanEstimator::Adev,
        )?);
    }
    if input.options.estimators.overlapping_adev {
        curves.overlapping_adev = Some(compute_curve(
            &phase,
            input.tau0_s,
            &input.options.tau_grid,
            AllanEstimator::OverlappingAdev,
        )?);
    }
    if input.options.estimators.mdev {
        curves.mdev = Some(compute_curve(
            &phase,
            input.tau0_s,
            &input.options.tau_grid,
            AllanEstimator::Mdev,
        )?);
    }
    if input.options.estimators.hdev {
        curves.hdev = Some(compute_curve(
            &phase,
            input.tau0_s,
            &input.options.tau_grid,
            AllanEstimator::Hdev,
        )?);
    }
    if input.options.estimators.tdev {
        curves.tdev = Some(compute_curve(
            &phase,
            input.tau0_s,
            &input.options.tau_grid,
            AllanEstimator::Tdev,
        )?);
    }

    Ok(curves)
}

/// Plain non-overlapping Allan deviation for explicit averaging factors.
pub fn allan_deviation(
    series: AllanSeries<'_>,
    tau0_s: f64,
    averaging_factors: &[usize],
) -> Result<AllanResult, AllanError> {
    compute_explicit(series, tau0_s, averaging_factors, AllanEstimator::Adev)
}

/// Fully overlapping Allan deviation for explicit averaging factors.
pub fn overlapping_adev(
    series: AllanSeries<'_>,
    tau0_s: f64,
    averaging_factors: &[usize],
) -> Result<AllanResult, AllanError> {
    compute_explicit(
        series,
        tau0_s,
        averaging_factors,
        AllanEstimator::OverlappingAdev,
    )
}

/// Modified Allan deviation for explicit averaging factors.
pub fn modified_adev(
    series: AllanSeries<'_>,
    tau0_s: f64,
    averaging_factors: &[usize],
) -> Result<AllanResult, AllanError> {
    compute_explicit(series, tau0_s, averaging_factors, AllanEstimator::Mdev)
}

/// Overlapping Hadamard deviation for explicit averaging factors.
pub fn hadamard_deviation(
    series: AllanSeries<'_>,
    tau0_s: f64,
    averaging_factors: &[usize],
) -> Result<AllanResult, AllanError> {
    compute_explicit(series, tau0_s, averaging_factors, AllanEstimator::Hdev)
}

/// Time deviation for explicit averaging factors.
pub fn time_deviation(
    series: AllanSeries<'_>,
    tau0_s: f64,
    averaging_factors: &[usize],
) -> Result<AllanResult, AllanError> {
    compute_explicit(series, tau0_s, averaging_factors, AllanEstimator::Tdev)
}

fn compute_explicit(
    series: AllanSeries<'_>,
    tau0_s: f64,
    averaging_factors: &[usize],
    estimator: AllanEstimator,
) -> Result<AllanResult, AllanError> {
    let phase = prepare_phase(series, tau0_s, GapPolicy::Reject)?;
    compute_curve(
        &phase,
        tau0_s,
        &TauGrid::Explicit(averaging_factors.to_vec()),
        estimator,
    )
}

#[derive(Debug, Clone, Copy)]
struct PhasePoint {
    value_s: f64,
    run_id: usize,
}

fn prepare_phase(
    series: AllanSeries<'_>,
    tau0_s: f64,
    gap_policy: GapPolicy,
) -> Result<Vec<Option<PhasePoint>>, AllanError> {
    validate_tau0(tau0_s)?;
    match series {
        AllanSeries::PhaseSeconds(values) => phase_from_contiguous(values),
        AllanSeries::FractionalFrequency(values) => phase_from_contiguous_frequency(values, tau0_s),
        AllanSeries::PhaseSecondsWithGaps(values) => phase_from_gapped(values, gap_policy),
        AllanSeries::FractionalFrequencyWithGaps(values) => {
            phase_from_gapped_frequency(values, tau0_s, gap_policy)
        }
    }
}

fn validate_tau0(tau0_s: f64) -> Result<(), AllanError> {
    if tau0_s.is_finite() && tau0_s > 0.0 {
        Ok(())
    } else {
        Err(AllanError::InvalidTau0 { tau0_s })
    }
}

fn phase_from_contiguous(values: &[f64]) -> Result<Vec<Option<PhasePoint>>, AllanError> {
    if values.is_empty() {
        return Err(AllanError::EmptySeries);
    }
    values
        .iter()
        .enumerate()
        .map(|(index, &value_s)| {
            validate_sample(index, value_s).map(|value_s| Some(PhasePoint { value_s, run_id: 0 }))
        })
        .collect()
}

fn phase_from_contiguous_frequency(
    values: &[f64],
    tau0_s: f64,
) -> Result<Vec<Option<PhasePoint>>, AllanError> {
    if values.is_empty() {
        return Err(AllanError::EmptySeries);
    }
    let mut phase = Vec::with_capacity(values.len() + 1);
    let mut value_s = 0.0;
    phase.push(Some(PhasePoint { value_s, run_id: 0 }));
    for (index, &frequency) in values.iter().enumerate() {
        let frequency = validate_sample(index, frequency)?;
        value_s += tau0_s * frequency;
        if !value_s.is_finite() {
            return Err(AllanError::NonFiniteSample { index });
        }
        phase.push(Some(PhasePoint { value_s, run_id: 0 }));
    }
    Ok(phase)
}

fn phase_from_gapped(
    values: &[Option<f64>],
    gap_policy: GapPolicy,
) -> Result<Vec<Option<PhasePoint>>, AllanError> {
    if values.is_empty() {
        return Err(AllanError::EmptySeries);
    }

    let mut phase = Vec::with_capacity(values.len());
    let mut run_id = 0usize;
    let mut in_run = false;
    for (index, value) in values.iter().enumerate() {
        match value {
            Some(value_s) => {
                let value_s = validate_sample(index, *value_s)?;
                if !in_run {
                    run_id += 1;
                    in_run = true;
                }
                phase.push(Some(PhasePoint { value_s, run_id }));
            }
            None => {
                if gap_policy == GapPolicy::Reject {
                    return Err(AllanError::Gap { index });
                }
                in_run = false;
                phase.push(None);
            }
        }
    }
    Ok(phase)
}

fn phase_from_gapped_frequency(
    values: &[Option<f64>],
    tau0_s: f64,
    gap_policy: GapPolicy,
) -> Result<Vec<Option<PhasePoint>>, AllanError> {
    if values.is_empty() {
        return Err(AllanError::EmptySeries);
    }
    if gap_policy == GapPolicy::Reject {
        for (index, value) in values.iter().enumerate() {
            if value.is_none() {
                return Err(AllanError::Gap { index });
            }
        }
    }

    let mut phase = vec![None; values.len() + 1];
    let mut run_id = 0usize;
    let mut index = 0usize;
    while index < values.len() {
        if values[index].is_none() {
            index += 1;
            continue;
        };

        run_id += 1;
        let mut value_s = 0.0;
        phase[index] = Some(PhasePoint { value_s, run_id });
        let mut sample_index = index;
        while let Some(current) = values.get(sample_index).copied().flatten() {
            let current = validate_sample(sample_index, current)?;
            value_s += tau0_s * current;
            if !value_s.is_finite() {
                return Err(AllanError::NonFiniteSample {
                    index: sample_index,
                });
            }
            phase[sample_index + 1] = Some(PhasePoint { value_s, run_id });
            sample_index += 1;
        }
        index = sample_index + 1;
    }

    Ok(phase)
}

fn validate_sample(index: usize, value: f64) -> Result<f64, AllanError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(AllanError::NonFiniteSample { index })
    }
}

fn compute_curve(
    phase: &[Option<PhasePoint>],
    tau0_s: f64,
    tau_grid: &TauGrid,
    estimator: AllanEstimator,
) -> Result<AllanResult, AllanError> {
    let averaging_factors = averaging_factors(phase.len(), estimator, tau_grid)?;
    let strict = matches!(tau_grid, TauGrid::Explicit(_));
    let mut result = AllanResult::new();

    for m in averaging_factors {
        if m == 0 {
            return Err(AllanError::InvalidAveragingFactor {
                averaging_factor: m,
            });
        }
        if candidate_count(phase.len(), estimator, m).is_none() {
            if strict {
                return Err(AllanError::TooFewSamples {
                    estimator,
                    averaging_factor: m,
                    available_phase_samples: phase.len(),
                });
            }
            continue;
        }

        let tau_s = tau0_s * m as f64;
        if !tau_s.is_finite() {
            return Err(AllanError::NonFiniteTau {
                estimator,
                averaging_factor: m,
            });
        }

        let (sum_sq, n) = estimator_sum_squares(phase, estimator, m);
        if n == 0 {
            if strict {
                return Err(AllanError::TooFewSamples {
                    estimator,
                    averaging_factor: m,
                    available_phase_samples: phase.len(),
                });
            }
            continue;
        }

        let deviation = deviation_from_sum(sum_sq, n, tau_s, m, estimator);
        if !deviation.is_finite() {
            return Err(AllanError::NonFiniteDeviation {
                estimator,
                averaging_factor: m,
            });
        }
        result.push(tau_s, deviation, n);
    }

    if result.is_empty() {
        return Err(AllanError::TooFewSamples {
            estimator,
            averaging_factor: 1,
            available_phase_samples: phase.len(),
        });
    }
    Ok(result)
}

fn averaging_factors(
    phase_len: usize,
    estimator: AllanEstimator,
    tau_grid: &TauGrid,
) -> Result<Vec<usize>, AllanError> {
    match tau_grid {
        TauGrid::Explicit(values) => {
            if values.is_empty() {
                Err(AllanError::EmptyTauGrid)
            } else {
                Ok(values.clone())
            }
        }
        TauGrid::All => Ok((1..=max_averaging_factor(phase_len, estimator)).collect()),
        TauGrid::Octave => {
            let max_m = max_averaging_factor(phase_len, estimator);
            let mut values = Vec::new();
            let mut m = 1usize;
            while m <= max_m {
                values.push(m);
                if m > max_m / 2 {
                    break;
                }
                m *= 2;
            }
            Ok(values)
        }
    }
}

fn max_averaging_factor(phase_len: usize, estimator: AllanEstimator) -> usize {
    match estimator {
        AllanEstimator::Adev | AllanEstimator::OverlappingAdev => phase_len.saturating_sub(1) / 2,
        AllanEstimator::Mdev | AllanEstimator::Tdev => phase_len / 3,
        AllanEstimator::Hdev => phase_len.saturating_sub(1) / 3,
    }
}

fn candidate_count(phase_len: usize, estimator: AllanEstimator, m: usize) -> Option<usize> {
    if m == 0 {
        return None;
    }
    match estimator {
        AllanEstimator::Adev => {
            let frequency_len = phase_len.checked_sub(1)?;
            (frequency_len / m).checked_sub(1)
        }
        AllanEstimator::OverlappingAdev => phase_len.checked_sub(m.checked_mul(2)?),
        AllanEstimator::Mdev | AllanEstimator::Tdev => {
            phase_len.checked_sub(m.checked_mul(3)?)?.checked_add(1)
        }
        AllanEstimator::Hdev => phase_len.checked_sub(m.checked_mul(3)?),
    }
}

fn estimator_sum_squares(
    phase: &[Option<PhasePoint>],
    estimator: AllanEstimator,
    m: usize,
) -> (f64, usize) {
    match estimator {
        AllanEstimator::Adev => plain_adev_sum_squares(phase, m),
        AllanEstimator::OverlappingAdev => overlapping_adev_sum_squares(phase, m),
        AllanEstimator::Mdev | AllanEstimator::Tdev => modified_adev_sum_squares(phase, m),
        AllanEstimator::Hdev => hadamard_sum_squares(phase, m),
    }
}

fn deviation_from_sum(
    sum_sq: f64,
    n: usize,
    tau_s: f64,
    m: usize,
    estimator: AllanEstimator,
) -> f64 {
    let n = n as f64;
    match estimator {
        AllanEstimator::Adev | AllanEstimator::OverlappingAdev => {
            (sum_sq / (2.0 * n * tau_s * tau_s)).sqrt()
        }
        AllanEstimator::Mdev => {
            let mf = m as f64;
            (sum_sq / (2.0 * mf * mf * n * tau_s * tau_s)).sqrt()
        }
        AllanEstimator::Hdev => (sum_sq / (6.0 * n * tau_s * tau_s)).sqrt(),
        AllanEstimator::Tdev => {
            let mf = m as f64;
            let mdev = (sum_sq / (2.0 * mf * mf * n * tau_s * tau_s)).sqrt();
            tau_s * mdev / SQRT_3
        }
    }
}

fn plain_adev_sum_squares(phase: &[Option<PhasePoint>], m: usize) -> (f64, usize) {
    let Some(count) = candidate_count(phase.len(), AllanEstimator::Adev, m) else {
        return (0.0, 0);
    };
    let mut sum_sq = 0.0;
    let mut n = 0usize;
    for j in 0..count {
        let i = j * m;
        if let Some((diff, _)) = second_difference(phase, i, m) {
            let square = diff * diff;
            sum_sq += square;
            n += 1;
        }
    }
    (sum_sq, n)
}

fn overlapping_adev_sum_squares(phase: &[Option<PhasePoint>], m: usize) -> (f64, usize) {
    let Some(count) = candidate_count(phase.len(), AllanEstimator::OverlappingAdev, m) else {
        return (0.0, 0);
    };
    let mut sum_sq = 0.0;
    let mut n = 0usize;
    for i in 0..count {
        if let Some((diff, _)) = second_difference(phase, i, m) {
            let square = diff * diff;
            sum_sq += square;
            n += 1;
        }
    }
    (sum_sq, n)
}

fn modified_adev_sum_squares(phase: &[Option<PhasePoint>], m: usize) -> (f64, usize) {
    let Some(count) = candidate_count(phase.len(), AllanEstimator::Mdev, m) else {
        return (0.0, 0);
    };
    let mut sum_sq = 0.0;
    let mut n = 0usize;
    for j in 0..count {
        let mut inner = 0.0;
        let mut run_id = None;
        let mut valid = true;
        for i in j..(j + m) {
            let Some((diff, diff_run_id)) = second_difference(phase, i, m) else {
                valid = false;
                break;
            };
            if run_id.is_some_and(|current| current != diff_run_id) {
                valid = false;
                break;
            }
            run_id = Some(diff_run_id);
            inner += diff;
        }
        if valid {
            let square = inner * inner;
            sum_sq += square;
            n += 1;
        }
    }
    (sum_sq, n)
}

fn hadamard_sum_squares(phase: &[Option<PhasePoint>], m: usize) -> (f64, usize) {
    let Some(count) = candidate_count(phase.len(), AllanEstimator::Hdev, m) else {
        return (0.0, 0);
    };
    let mut sum_sq = 0.0;
    let mut n = 0usize;
    for i in 0..count {
        if let Some(diff) = third_difference(phase, i, m) {
            let square = diff * diff;
            sum_sq += square;
            n += 1;
        }
    }
    (sum_sq, n)
}

fn second_difference(phase: &[Option<PhasePoint>], i: usize, m: usize) -> Option<(f64, usize)> {
    let x0 = phase.get(i).copied().flatten()?;
    let x1 = phase.get(i + m).copied().flatten()?;
    let x2 = phase.get(i + 2 * m).copied().flatten()?;
    if x0.run_id != x1.run_id || x0.run_id != x2.run_id {
        return None;
    }
    Some((x2.value_s - 2.0 * x1.value_s + x0.value_s, x0.run_id))
}

fn third_difference(phase: &[Option<PhasePoint>], i: usize, m: usize) -> Option<f64> {
    let x0 = phase.get(i).copied().flatten()?;
    let x1 = phase.get(i + m).copied().flatten()?;
    let x2 = phase.get(i + 2 * m).copied().flatten()?;
    let x3 = phase.get(i + 3 * m).copied().flatten()?;
    if x0.run_id != x1.run_id || x0.run_id != x2.run_id || x0.run_id != x3.run_id {
        return None;
    }
    Some(x3.value_s - 3.0 * x2.value_s + 3.0 * x1.value_s - x0.value_s)
}
