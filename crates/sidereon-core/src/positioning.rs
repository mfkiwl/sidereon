//! Single-point positioning and GNSS geometry diagnostics.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub use crate::dop::{dop, Dop, DopError, LineOfSight};
use crate::ephemeris::{BroadcastEphemeris, Sp3};
pub use crate::quality::{
    spp_robust_fde_driver, FdeError, FdeOptions, FdeResult, FdeSppError, FdeSppOptions,
};
use crate::rinex::observations::{pseudoranges, ObsEpochTime, ObservationFile, SignalPolicy};
pub use crate::spp::{
    residual_rms, solve, solve_broadcast, solve_doppler_velocity, solve_spp_batch_parallel,
    solve_spp_batch_serial, solve_with_doppler_velocity, solve_with_fallback, solve_with_policy,
    solve_with_solver, BroadcastReason, Corrections, DopplerObservation, DopplerVelocityInputs,
    EphemerisSource, FallbackError, FixSource, GalileoNequickCoeffs, KlobucharCoeffs, Observation,
    ReceiverSolution, RejectedSat, RejectionReason, RobustConfig, SolutionMetadata, SolveInputs,
    SolvePolicy, SolvePolicyError, SourcedSolution, SppDopplerSolution, SppError, SurfaceMet,
    DEFAULT_HUBER_K, DEFAULT_ROBUST_MAX_OUTER, DEFAULT_ROBUST_OUTER_TOL_M,
    DEFAULT_ROBUST_SCALE_FLOOR_M, ELEVATION_MASK_RAD, SIGMA0_M, TRANSMIT_TIME_ITERATIONS,
};
pub use crate::static_positioning::{
    solve_static, StaticClockBias, StaticCovariance, StaticEpoch, StaticEpochInfluence,
    StaticInfluenceStatus, StaticResidual, StaticSatelliteBatchInfluence, StaticSatelliteInfluence,
    StaticSolution, StaticSolutionMetadata, StaticSolveError, StaticSolveOptions,
};
pub use crate::static_reference_station::{
    solve_static_reference_station_rinex, StaticReferenceCarrierRinexOptions,
    StaticReferenceCarrierSolution, StaticReferenceCodeSolution, StaticReferenceEpochDiagnostic,
    StaticReferenceFixStatus, StaticReferenceModeError, StaticReferenceModeReport,
    StaticReferenceModeStatus, StaticReferenceStationCovariance, StaticReferenceStationError,
    StaticReferenceStationMode, StaticReferenceStationRinexOptions, StaticReferenceStationSolution,
};
use crate::{astro::time, Error as CoreError, GnssSatelliteId};

/// Role-oriented alias for a solved receiver state.
pub type Solution = ReceiverSolution;

/// Error type returned by [`solve`].
pub type Error = SppError;

/// Assembly-time error from building SPP inputs out of a parsed RINEX
/// observation file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RinexSppError {
    /// A RINEX observation helper rejected malformed or non-finite input.
    Observation(CoreError),
    /// No initial receiver position was supplied and the observation header did
    /// not carry `APPROX POSITION XYZ`.
    MissingApproxPosition,
}

impl fmt::Display for RinexSppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Observation(error) => write!(f, "RINEX SPP observation assembly failed: {error}"),
            Self::MissingApproxPosition => {
                f.write_str("RINEX SPP assembly needs APPROX POSITION XYZ or an initial guess")
            }
        }
    }
}

impl std::error::Error for RinexSppError {}

impl From<CoreError> for RinexSppError {
    fn from(error: CoreError) -> Self {
        Self::Observation(error)
    }
}

/// Broadcast correction metadata used while converting RINEX observations into
/// SPP [`SolveInputs`].
///
/// A broadcast navigation product supplies ionosphere coefficients and GLONASS
/// FDMA channels. A precise SP3 product does not, so its default assembly
/// context is zero Klobuchar coefficients and no GLONASS channels. When solving
/// precise SP3 positions with broadcast atmosphere metadata, wrap the SP3 with
/// [`RinexSppSource::with_broadcast_context`].
#[derive(Debug, Clone, PartialEq)]
pub struct RinexSppBroadcastCorrections {
    /// GPS Klobuchar coefficients, also used as the fallback for systems
    /// without a dedicated correction set.
    pub klobuchar: KlobucharCoeffs,
    /// BeiDou-specific Klobuchar coefficients, when a NAV product provides
    /// `BDSA`/`BDSB`.
    pub beidou_klobuchar: Option<KlobucharCoeffs>,
    /// Galileo NeQuick-G coefficients, when a NAV product provides `GAL`.
    pub galileo_nequick: Option<GalileoNequickCoeffs>,
    /// GLONASS FDMA channel numbers keyed by GLONASS slot.
    pub glonass_channels: BTreeMap<u8, i8>,
}

impl Default for RinexSppBroadcastCorrections {
    fn default() -> Self {
        Self {
            klobuchar: zero_klobuchar(),
            beidou_klobuchar: None,
            galileo_nequick: None,
            glonass_channels: BTreeMap::new(),
        }
    }
}

/// Source of non-observation metadata needed during RINEX SPP assembly.
///
/// [`BroadcastEphemeris`] implements this trait with its parsed NAV ionosphere
/// coefficients and GLONASS FDMA channels. [`Sp3`] implements it with empty
/// broadcast metadata so precise-only callers can still assemble
/// troposphere-only or no-correction inputs.
pub trait RinexSppAssemblySource {
    /// Broadcast correction metadata available to RINEX SPP assembly.
    fn rinex_spp_broadcast_corrections(&self) -> RinexSppBroadcastCorrections;
}

impl RinexSppAssemblySource for BroadcastEphemeris {
    fn rinex_spp_broadcast_corrections(&self) -> RinexSppBroadcastCorrections {
        let iono = self.iono_corrections();
        let gps = iono.gps.map(klobuchar_from_alpha_beta);
        RinexSppBroadcastCorrections {
            klobuchar: gps.unwrap_or_else(zero_klobuchar),
            beidou_klobuchar: iono.beidou.map(klobuchar_from_alpha_beta),
            galileo_nequick: iono.galileo,
            glonass_channels: self.glonass_frequency_channels(),
        }
    }
}

impl RinexSppAssemblySource for Sp3 {
    fn rinex_spp_broadcast_corrections(&self) -> RinexSppBroadcastCorrections {
        RinexSppBroadcastCorrections::default()
    }
}

/// Delegating ephemeris source that lets a precise product solve with broadcast
/// NAV metadata during RINEX SPP assembly.
///
/// Use [`Self::with_broadcast_context`] for the common precise-SP3-plus-RINEX-NAV
/// path: satellite position and clocks come from `ephemeris`, while
/// ionosphere coefficients and GLONASS FDMA channels come from `broadcast`.
pub struct RinexSppSource<'a, E: EphemerisSource + ?Sized> {
    ephemeris: &'a E,
    broadcast: Option<&'a BroadcastEphemeris>,
}

impl<'a, E: EphemerisSource + ?Sized> RinexSppSource<'a, E> {
    /// Build a delegating source with no broadcast assembly context.
    #[must_use]
    pub const fn new(ephemeris: &'a E) -> Self {
        Self {
            ephemeris,
            broadcast: None,
        }
    }

    /// Build a delegating source whose ephemeris is used for the solve and whose
    /// broadcast product is used for RINEX assembly metadata.
    #[must_use]
    pub const fn with_broadcast_context(
        ephemeris: &'a E,
        broadcast: &'a BroadcastEphemeris,
    ) -> Self {
        Self {
            ephemeris,
            broadcast: Some(broadcast),
        }
    }

    /// The ephemeris source delegated to during the SPP solve.
    #[must_use]
    pub const fn ephemeris(&self) -> &'a E {
        self.ephemeris
    }

    /// Broadcast metadata source, when one was supplied.
    #[must_use]
    pub const fn broadcast_context(&self) -> Option<&'a BroadcastEphemeris> {
        self.broadcast
    }
}

impl<E: EphemerisSource + ?Sized> EphemerisSource for RinexSppSource<'_, E> {
    fn position_clock_at_j2000_s(
        &self,
        sat: GnssSatelliteId,
        t_j2000_s: f64,
    ) -> Option<([f64; 3], f64)> {
        self.ephemeris.position_clock_at_j2000_s(sat, t_j2000_s)
    }
}

impl<E: EphemerisSource + ?Sized> RinexSppAssemblySource for RinexSppSource<'_, E> {
    fn rinex_spp_broadcast_corrections(&self) -> RinexSppBroadcastCorrections {
        self.broadcast
            .map(RinexSppAssemblySource::rinex_spp_broadcast_corrections)
            .unwrap_or_default()
    }
}

/// Options for assembling RINEX observation epochs into SPP [`SolveInputs`].
#[derive(Debug, Clone, PartialEq)]
pub struct RinexSppOptions {
    /// Per-constellation pseudorange-code selection policy.
    pub signal_policy: SignalPolicy,
    /// Correction terms to request in each assembled solve.
    pub corrections: Corrections,
    /// Optional initial guess `[x_m, y_m, z_m, b_m]`. When absent, the RINEX
    /// header's `APPROX POSITION XYZ` provides the position and the clock seed
    /// is zero.
    pub initial_guess: Option<[f64; 4]>,
    /// Optional satellite allow-list. `None` keeps every satellite with a
    /// selected pseudorange.
    pub satellites: Option<BTreeSet<GnssSatelliteId>>,
    /// Surface meteorology for troposphere correction.
    pub met: SurfaceMet,
    /// Optional robust reweighting for every assembled epoch.
    pub robust: Option<RobustConfig>,
}

impl RinexSppOptions {
    /// Build options from an explicit signal policy.
    #[must_use]
    pub fn new(signal_policy: SignalPolicy) -> Self {
        Self {
            signal_policy,
            corrections: Corrections::IONO_TROPO,
            initial_guess: None,
            satellites: None,
            met: SurfaceMet::default(),
            robust: None,
        }
    }

    /// Build options using the default single-frequency signal policy for the
    /// observation file's RINEX version.
    pub fn default_for(obs: &ObservationFile) -> Result<Self, RinexSppError> {
        Ok(Self::new(SignalPolicy::default_for(obs.header().version)?))
    }

    /// Replace the correction request.
    #[must_use]
    pub const fn with_corrections(mut self, corrections: Corrections) -> Self {
        self.corrections = corrections;
        self
    }

    /// Replace the initial solve guess.
    #[must_use]
    pub const fn with_initial_guess(mut self, initial_guess: [f64; 4]) -> Self {
        self.initial_guess = Some(initial_guess);
        self
    }

    /// Restrict assembly to the supplied satellites.
    #[must_use]
    pub fn with_satellites<I>(mut self, satellites: I) -> Self
    where
        I: IntoIterator<Item = GnssSatelliteId>,
    {
        self.satellites = Some(satellites.into_iter().collect());
        self
    }

    /// Replace surface meteorology.
    #[must_use]
    pub const fn with_surface_met(mut self, met: SurfaceMet) -> Self {
        self.met = met;
        self
    }

    /// Replace robust-reweighting config.
    #[must_use]
    pub const fn with_robust(mut self, robust: Option<RobustConfig>) -> Self {
        self.robust = robust;
        self
    }
}

/// One assembled RINEX observation epoch and its SPP inputs.
#[derive(Debug, Clone)]
pub struct RinexSppEpochInputs {
    /// Index in [`ObservationFile::epochs`].
    pub epoch_index: usize,
    /// Civil epoch exactly as it appears in the RINEX observation file.
    pub epoch: ObsEpochTime,
    /// Fully assembled SPP inputs for this epoch.
    pub inputs: SolveInputs,
}

/// One RINEX observation epoch paired with its serial SPP solve result.
#[derive(Debug, Clone)]
pub struct RinexSppEpochSolution {
    /// Index in [`ObservationFile::epochs`].
    pub epoch_index: usize,
    /// Civil epoch exactly as it appears in the RINEX observation file.
    pub epoch: ObsEpochTime,
    /// Result from solving the assembled epoch.
    pub solution: Result<ReceiverSolution, SolvePolicyError>,
}

/// Assemble every non-event RINEX observation epoch with at least one selected
/// pseudorange into SPP [`SolveInputs`].
///
/// The function preserves observation-file epoch order, skips RINEX event
/// epochs (`flag > 1`), selects one single-frequency pseudorange per satellite
/// under [`RinexSppOptions::signal_policy`], derives receive time from the RINEX
/// civil epoch, seeds the receiver from `APPROX POSITION XYZ` unless
/// `initial_guess` is supplied, and combines GLONASS channels from the assembly
/// source with any observation-header `GLONASS SLOT / FRQ #` entries. Observation
/// header channels take precedence.
pub fn spp_inputs_from_rinex_obs<S>(
    obs: &ObservationFile,
    source: &S,
    options: &RinexSppOptions,
) -> Result<Vec<RinexSppEpochInputs>, RinexSppError>
where
    S: RinexSppAssemblySource + ?Sized,
{
    let initial_guess = initial_guess(obs, options)?;
    let base_corrections = merged_broadcast_corrections(obs, source);
    let mut out = Vec::new();

    for (epoch_index, epoch) in obs.epochs().iter().enumerate() {
        if epoch.flag > 1 {
            continue;
        }
        let mut selected = pseudoranges(obs, epoch, &options.signal_policy)?;
        if let Some(allowed) = &options.satellites {
            selected.retain(|(sat, _)| allowed.contains(sat));
        }
        if selected.is_empty() {
            continue;
        }

        let epoch_context = epoch_time_context(epoch.epoch);
        let observations = selected
            .into_iter()
            .map(|(satellite_id, pseudorange_m)| Observation {
                satellite_id,
                pseudorange_m,
            })
            .collect();

        out.push(RinexSppEpochInputs {
            epoch_index,
            epoch: epoch.epoch,
            inputs: SolveInputs {
                observations,
                t_rx_j2000_s: epoch_context.t_rx_j2000_s,
                t_rx_second_of_day_s: epoch_context.t_rx_second_of_day_s,
                day_of_year: epoch_context.day_of_year,
                initial_guess,
                corrections: options.corrections,
                klobuchar: base_corrections.klobuchar,
                beidou_klobuchar: base_corrections.beidou_klobuchar,
                galileo_nequick: base_corrections.galileo_nequick,
                sbas_iono: None,
                glonass_channels: base_corrections.glonass_channels.clone(),
                met: options.met,
                robust: options.robust,
            },
        });
    }

    Ok(out)
}

/// Assemble RINEX SPP epochs and solve them serially against the same source.
///
/// The returned vector has one entry per assembled epoch, not one entry per raw
/// RINEX epoch. Per-epoch solve failures are retained in
/// [`RinexSppEpochSolution::solution`], matching [`solve_spp_batch_serial`].
pub fn solve_spp_from_rinex_obs<S>(
    source: &S,
    obs: &ObservationFile,
    options: &RinexSppOptions,
    with_geodetic: bool,
    policy: SolvePolicy,
) -> Result<Vec<RinexSppEpochSolution>, RinexSppError>
where
    S: EphemerisSource + RinexSppAssemblySource,
{
    let epochs = spp_inputs_from_rinex_obs(obs, source, options)?;
    let inputs = epochs
        .iter()
        .map(|epoch| epoch.inputs.clone())
        .collect::<Vec<_>>();
    let results = solve_spp_batch_serial(source, &inputs, with_geodetic, policy);
    Ok(epochs
        .into_iter()
        .zip(results)
        .map(|(epoch, solution)| RinexSppEpochSolution {
            epoch_index: epoch.epoch_index,
            epoch: epoch.epoch,
            solution,
        })
        .collect())
}

fn klobuchar_from_alpha_beta(value: crate::ephemeris::KlobucharAlphaBeta) -> KlobucharCoeffs {
    KlobucharCoeffs {
        alpha: value.alpha,
        beta: value.beta,
    }
}

const fn zero_klobuchar() -> KlobucharCoeffs {
    KlobucharCoeffs {
        alpha: [0.0; 4],
        beta: [0.0; 4],
    }
}

fn initial_guess(
    obs: &ObservationFile,
    options: &RinexSppOptions,
) -> Result<[f64; 4], RinexSppError> {
    if let Some(initial_guess) = options.initial_guess {
        return Ok(initial_guess);
    }
    let approx = obs
        .header()
        .approx_position_m
        .ok_or(RinexSppError::MissingApproxPosition)?;
    Ok([approx[0], approx[1], approx[2], 0.0])
}

fn merged_broadcast_corrections<S>(
    obs: &ObservationFile,
    source: &S,
) -> RinexSppBroadcastCorrections
where
    S: RinexSppAssemblySource + ?Sized,
{
    let mut corrections = source.rinex_spp_broadcast_corrections();
    corrections.glonass_channels.extend(
        obs.header()
            .glonass_slots
            .iter()
            .map(|(&slot, &channel)| (slot, channel)),
    );
    corrections
}

struct EpochTimeContext {
    t_rx_j2000_s: f64,
    t_rx_second_of_day_s: f64,
    day_of_year: f64,
}

fn epoch_time_context(epoch: ObsEpochTime) -> EpochTimeContext {
    let year = epoch.year;
    let month = i32::from(epoch.month);
    let day = i32::from(epoch.day);
    let hour = i32::from(epoch.hour);
    let minute = i32::from(epoch.minute);
    EpochTimeContext {
        t_rx_j2000_s: time::j2000_seconds(year, month, day, hour, minute, epoch.second),
        t_rx_second_of_day_s: time::second_of_day(hour, minute, epoch.second),
        day_of_year: time::day_of_year(year, month, day, hour, minute, epoch.second),
    }
}
