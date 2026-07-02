//! Engineering-unit State Space Representation corrections.
//!
//! The RTCM module stores raw transmitted integers. This module stores scaled
//! correction values keyed by satellite, with enough provider and issue metadata
//! to apply orbit and clock corrections on top of broadcast ephemerides.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::astro::time::model::{GnssWeekTow, TimeScale};
use crate::broadcast::satellite_state_unchecked;
use crate::constants::{C_M_S, GPS_EPOCH_TO_J2000_S, SECONDS_PER_WEEK};
use crate::ephemeris::{BroadcastEphemeris, BroadcastIssue, NavMessage};
use crate::error::{Error, Result};
use crate::id::{GnssSatelliteId, GnssSystem};
use crate::observables::{ObservableEphemerisSource, ObservableState, ObservablesError};
use crate::rinex_nav::is_beidou_geo;
use crate::rtcm::{Message, SsrKind, SsrMessage};
use crate::spp::EphemerisSource;
use crate::staleness::StalenessPolicy;

const DEFAULT_SSR_STALENESS_S: f64 = 90.0;
const FD_HALF_S: f64 = 0.5;

/// Which stream produced a correction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SsrSource {
    /// RTCM SSR messages.
    RtcmSsr,
    /// Galileo HAS messages.
    GalileoHas,
}

/// Orbital basis used by stored RAC components.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OrbitBasis {
    /// Velocity-aligned basis used by RTCM SSR and HAS.
    VelocityAligned,
}

/// Reference point of the corrected orbit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OrbitReferencePoint {
    /// Satellite antenna phase center.
    AntennaPhaseCenter,
    /// Satellite center of mass.
    CenterOfMass,
}

/// Provider and solution identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SsrSolution {
    /// Correction source.
    pub source: SsrSource,
    /// Provider id.
    pub provider_id: u16,
    /// Solution id.
    pub solution_id: u8,
}

/// Orbit correction for one satellite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SsrOrbitCorrection {
    /// Provider and solution identity.
    pub solution: SsrSolution,
    /// Referenced broadcast issue.
    pub iode: u32,
    /// IOD SSR.
    pub iod_ssr: u8,
    /// Orbit basis.
    pub basis: OrbitBasis,
    /// True when the RTCM reference datum bit marks a regional CRS.
    pub crs_regional: bool,
    /// Orbit reference point policy.
    pub reference_point: OrbitReferencePoint,
    /// Radial delta to add, meters.
    pub radial_m: f64,
    /// Along-track delta to add, meters.
    pub along_m: f64,
    /// Cross-track delta to add, meters.
    pub cross_m: f64,
    /// Radial delta rate to add, meters per second.
    pub radial_rate_m_s: f64,
    /// Along-track delta rate to add, meters per second.
    pub along_rate_m_s: f64,
    /// Cross-track delta rate to add, meters per second.
    pub cross_rate_m_s: f64,
    /// Reference epoch, seconds since J2000.
    pub ref_epoch_j2000_s: f64,
    /// Update interval, seconds.
    pub update_interval_s: f64,
}

/// High-rate clock correction for one satellite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SsrHighRateClock {
    /// Provider and solution identity.
    pub solution: SsrSolution,
    /// IOD SSR.
    pub iod_ssr: u8,
    /// Additive C0 term, meters.
    pub c0_m: f64,
    /// Reference epoch, seconds since J2000.
    pub ref_epoch_j2000_s: f64,
    /// Update interval, seconds.
    pub update_interval_s: f64,
}

/// Clock correction for one satellite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SsrClockCorrection {
    /// Provider and solution identity.
    pub solution: SsrSolution,
    /// IOD SSR.
    pub iod_ssr: u8,
    /// C0 term, meters.
    pub c0_m: f64,
    /// C1 term, meters per second.
    pub c1_m_s: f64,
    /// C2 term, meters per second squared.
    pub c2_m_s2: f64,
    /// Reference epoch, seconds since J2000.
    pub ref_epoch_j2000_s: f64,
    /// Update interval, seconds.
    pub update_interval_s: f64,
    /// Matching high-rate correction, when present.
    pub high_rate: Option<SsrHighRateClock>,
}

/// Code-bias correction placeholder for Phase B.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SsrCodeBias {
    biases_m: BTreeMap<u8, f64>,
}

/// Phase-bias correction placeholder for Phase B.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SsrPhaseBias {
    biases_m: BTreeMap<u8, f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct SatCorrections {
    orbit: Option<SsrOrbitCorrection>,
    clock: Option<SsrClockCorrection>,
    pending_high_rate: Option<SsrHighRateClock>,
    ura_index: Option<u8>,
    code_bias: SsrCodeBias,
    phase_bias: SsrPhaseBias,
}

/// Active SSR corrections keyed by satellite.
#[derive(Clone, Debug, PartialEq)]
pub struct SsrCorrectionStore {
    corrections: BTreeMap<GnssSatelliteId, SatCorrections>,
    reference_point: OrbitReferencePoint,
    staleness: StalenessPolicy,
}

impl Default for SsrCorrectionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SsrCorrectionStore {
    /// Build an empty correction store.
    pub fn new() -> Self {
        Self {
            corrections: BTreeMap::new(),
            reference_point: OrbitReferencePoint::CenterOfMass,
            staleness: StalenessPolicy::seconds(DEFAULT_SSR_STALENESS_S),
        }
    }

    /// Set the orbit reference point policy for later ingests.
    pub fn with_reference_point(mut self, reference_point: OrbitReferencePoint) -> Self {
        self.reference_point = reference_point;
        self
    }

    /// Set the store staleness policy.
    pub fn with_staleness(mut self, policy: StalenessPolicy) -> Self {
        self.staleness = policy;
        self
    }

    /// The store-level staleness policy.
    pub fn staleness(&self) -> StalenessPolicy {
        self.staleness
    }

    /// Ingest one RTCM message, ignoring non-SSR messages.
    pub fn ingest(&mut self, message: &Message, week: GnssWeekTow) -> Result<()> {
        if let Message::Ssr(ssr) = message {
            self.ingest_ssr(ssr, week)?;
        }
        Ok(())
    }

    /// Ingest one decoded RTCM SSR message.
    pub fn ingest_ssr(&mut self, message: &SsrMessage, week: GnssWeekTow) -> Result<()> {
        let update_interval_s = update_interval_s(message.header.update_interval);
        let ref_epoch_j2000_s = ssr_epoch_j2000_s(
            message.system,
            week,
            message.header.epoch_time_s,
            update_interval_s,
        )?;
        let solution = SsrSolution {
            source: SsrSource::RtcmSsr,
            provider_id: message.header.provider_id,
            solution_id: message.header.solution_id,
        };

        match message.kind {
            SsrKind::Orbit => {
                for record in &message.orbit {
                    let sat = ssr_satellite(message.system, record.satellite_id)?;
                    let orbit = orbit_from_rtcm(
                        self.reference_point,
                        message,
                        solution,
                        record,
                        ref_epoch_j2000_s,
                        update_interval_s,
                    );
                    let entry = self.corrections.entry(sat).or_default();
                    entry.orbit = Some(orbit);
                }
            }
            SsrKind::Clock => {
                for record in &message.clock {
                    let sat = ssr_satellite(message.system, record.satellite_id)?;
                    let entry = self.corrections.entry(sat).or_default();
                    let mut clock = SsrClockCorrection {
                        solution,
                        iod_ssr: message.header.iod_ssr,
                        c0_m: f64::from(record.c0) * 1.0e-4,
                        c1_m_s: f64::from(record.c1) * 1.0e-6,
                        c2_m_s2: f64::from(record.c2) * 2.0e-8,
                        ref_epoch_j2000_s,
                        update_interval_s,
                        high_rate: None,
                    };
                    if let Some(hr) = entry.pending_high_rate {
                        if high_rate_matches(&clock, &hr) {
                            clock.high_rate = Some(hr);
                        }
                    }
                    entry.clock = Some(clock);
                }
            }
            SsrKind::CombinedOrbitClock => {
                for (orbit_record, clock_record) in message.orbit.iter().zip(&message.clock) {
                    let sat = ssr_satellite(message.system, orbit_record.satellite_id)?;
                    let orbit = orbit_from_rtcm(
                        self.reference_point,
                        message,
                        solution,
                        orbit_record,
                        ref_epoch_j2000_s,
                        update_interval_s,
                    );
                    let entry = self.corrections.entry(sat).or_default();
                    entry.orbit = Some(orbit);
                    entry.clock = Some(SsrClockCorrection {
                        solution,
                        iod_ssr: message.header.iod_ssr,
                        c0_m: f64::from(clock_record.c0) * 1.0e-4,
                        c1_m_s: f64::from(clock_record.c1) * 1.0e-6,
                        c2_m_s2: f64::from(clock_record.c2) * 2.0e-8,
                        ref_epoch_j2000_s,
                        update_interval_s,
                        high_rate: entry.pending_high_rate,
                    });
                }
            }
            SsrKind::Ura => {
                for &(satellite_id, ura_index) in &message.ura {
                    let sat = ssr_satellite(message.system, satellite_id)?;
                    self.corrections.entry(sat).or_default().ura_index = Some(ura_index);
                }
            }
            SsrKind::HighRateClock => {
                for record in &message.clock {
                    let sat = ssr_satellite(message.system, record.satellite_id)?;
                    let high_rate = SsrHighRateClock {
                        solution,
                        iod_ssr: message.header.iod_ssr,
                        c0_m: f64::from(record.c0) * 1.0e-4,
                        ref_epoch_j2000_s,
                        update_interval_s,
                    };
                    let entry = self.corrections.entry(sat).or_default();
                    entry.pending_high_rate = Some(high_rate);
                    if let Some(clock) = &mut entry.clock {
                        if high_rate_matches(clock, &high_rate) {
                            clock.high_rate = Some(high_rate);
                        }
                    }
                }
            }
            SsrKind::CodeBias | SsrKind::PhaseBias | SsrKind::Vtec => {}
        }
        Ok(())
    }

    /// Orbit correction for a satellite.
    pub fn orbit(&self, sat: GnssSatelliteId) -> Option<&SsrOrbitCorrection> {
        self.corrections.get(&sat)?.orbit.as_ref()
    }

    /// Clock correction for a satellite.
    pub fn clock(&self, sat: GnssSatelliteId) -> Option<&SsrClockCorrection> {
        self.corrections.get(&sat)?.clock.as_ref()
    }

    /// URA index for a satellite.
    pub fn ura_index(&self, sat: GnssSatelliteId) -> Option<u8> {
        self.corrections.get(&sat)?.ura_index
    }

    /// Code bias in meters for a satellite and raw signal id.
    pub fn code_bias(&self, sat: GnssSatelliteId, signal: u8) -> Option<f64> {
        self.corrections
            .get(&sat)?
            .code_bias
            .biases_m
            .get(&signal)
            .copied()
    }

    /// Phase bias for a satellite.
    pub fn phase_bias(&self, sat: GnssSatelliteId, signal: u8) -> Option<f64> {
        self.corrections
            .get(&sat)?
            .phase_bias
            .biases_m
            .get(&signal)
            .copied()
    }
}

fn orbit_from_rtcm(
    reference_point: OrbitReferencePoint,
    message: &SsrMessage,
    solution: SsrSolution,
    record: &crate::rtcm::SsrOrbitRecord,
    ref_epoch_j2000_s: f64,
    update_interval_s: f64,
) -> SsrOrbitCorrection {
    SsrOrbitCorrection {
        solution,
        iode: record.iode,
        iod_ssr: message.header.iod_ssr,
        basis: OrbitBasis::VelocityAligned,
        crs_regional: message.header.satellite_reference_datum.unwrap_or(false),
        reference_point,
        radial_m: -f64::from(record.delta_radial) * 1.0e-4,
        along_m: -f64::from(record.delta_along) * 4.0e-4,
        cross_m: -f64::from(record.delta_cross) * 4.0e-4,
        radial_rate_m_s: -f64::from(record.dot_delta_radial) * 1.0e-6,
        along_rate_m_s: -f64::from(record.dot_delta_along) * 4.0e-6,
        cross_rate_m_s: -f64::from(record.dot_delta_cross) * 4.0e-6,
        ref_epoch_j2000_s,
        update_interval_s,
    }
}

/// Behavior when a correction is missing or stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingCorrectionAction {
    /// Decline the satellite.
    Decline,
    /// Return the plain broadcast state.
    FallBackToBroadcast,
}

/// Regional CRS handling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegionalPolicy {
    /// Decline regional corrections.
    DeclineRegional,
    /// Allow regional corrections from these provider ids.
    AllowProviders(BTreeSet<u16>),
}

/// Missing-correction and regional policy for corrected ephemerides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SsrFallbackPolicy {
    /// Missing or stale correction behavior.
    pub on_missing_correction: MissingCorrectionAction,
    /// Regional CRS behavior.
    pub regional: RegionalPolicy,
}

impl Default for SsrFallbackPolicy {
    fn default() -> Self {
        Self {
            on_missing_correction: MissingCorrectionAction::Decline,
            regional: RegionalPolicy::DeclineRegional,
        }
    }
}

/// Broadcast ephemeris corrected by an SSR store.
#[derive(Clone)]
pub struct SsrCorrectedEphemeris<'a> {
    broadcast: &'a BroadcastEphemeris,
    store: &'a SsrCorrectionStore,
    staleness: StalenessPolicy,
    fallback: SsrFallbackPolicy,
}

impl<'a> SsrCorrectedEphemeris<'a> {
    /// Build a corrected source from borrowed broadcast and SSR stores.
    pub fn new(broadcast: &'a BroadcastEphemeris, store: &'a SsrCorrectionStore) -> Self {
        Self {
            broadcast,
            store,
            staleness: store.staleness(),
            fallback: SsrFallbackPolicy::default(),
        }
    }

    /// Set the staleness policy.
    pub fn with_staleness(mut self, policy: StalenessPolicy) -> Self {
        self.staleness = policy;
        self
    }

    /// Set missing-correction and regional behavior.
    pub fn with_fallback(mut self, policy: SsrFallbackPolicy) -> Self {
        self.fallback = policy;
        self
    }

    /// Mark one regional provider as applicable.
    pub fn allow_regional_provider(mut self, provider_id: u16) -> Self {
        match &mut self.fallback.regional {
            RegionalPolicy::DeclineRegional => {
                let mut providers = BTreeSet::new();
                providers.insert(provider_id);
                self.fallback.regional = RegionalPolicy::AllowProviders(providers);
            }
            RegionalPolicy::AllowProviders(providers) => {
                providers.insert(provider_id);
            }
        }
        self
    }

    /// Corrected ECEF position and satellite clock at a J2000 epoch.
    pub fn corrected_state(&self, sat: GnssSatelliteId, t_j2000_s: f64) -> Option<([f64; 3], f64)> {
        self.corrected_state_inner(sat, t_j2000_s)
            .or_else(|| self.broadcast_fallback(sat, t_j2000_s))
    }

    fn corrected_state_inner(
        &self,
        sat: GnssSatelliteId,
        t_j2000_s: f64,
    ) -> Option<([f64; 3], f64)> {
        let orbit = self.store.orbit(sat)?;
        let clock = self.store.clock(sat)?;
        if orbit.solution != clock.solution || orbit.iod_ssr != clock.iod_ssr {
            return None;
        }
        if !self.correction_fresh(t_j2000_s, orbit.ref_epoch_j2000_s) {
            return None;
        }
        if !self.correction_fresh(t_j2000_s, clock.ref_epoch_j2000_s) {
            return None;
        }
        if orbit.crs_regional && !self.regional_allowed(orbit.solution.provider_id) {
            return None;
        }

        let nav_message = default_nav_message(sat.system)?;
        let issue = BroadcastIssue {
            issue: orbit.iode,
            message: nav_message,
        };
        let record = self
            .broadcast
            .select_by_issue_at(sat, issue, nav_message, t_j2000_s)?;
        let (t_continuous_s, is_geo) = continuous_time_for_sat(sat, t_j2000_s)?;
        let sow = t_continuous_s.rem_euclid(SECONDS_PER_WEEK);
        let state = satellite_state_unchecked(
            &record.elements,
            &record.clock,
            &record.constants(),
            sow,
            record.broadcast_clock_group_delay_s(),
            is_geo,
        );
        let r = state.orbit.position().ok()?.as_array();
        let v = broadcast_velocity(record, sat, t_j2000_s, is_geo)?;
        let (er, ea, ec) = velocity_aligned_basis(r, v)?;
        let dt_orbit = t_j2000_s - orbit.ref_epoch_j2000_s;
        let radial = orbit.radial_m + orbit.radial_rate_m_s * dt_orbit;
        let along = orbit.along_m + orbit.along_rate_m_s * dt_orbit;
        let cross = orbit.cross_m + orbit.cross_rate_m_s * dt_orbit;
        let corrected_position = [
            r[0] + radial * er[0] + along * ea[0] + cross * ec[0],
            r[1] + radial * er[1] + along * ea[1] + cross * ec[1],
            r[2] + radial * er[2] + along * ea[2] + cross * ec[2],
        ];

        let dt_clock = t_j2000_s - clock.ref_epoch_j2000_s;
        let mut dclock_m =
            clock.c0_m + clock.c1_m_s * dt_clock + clock.c2_m_s2 * dt_clock * dt_clock;
        if let Some(high_rate) = clock.high_rate {
            if high_rate_matches(clock, &high_rate)
                && self.correction_fresh(t_j2000_s, high_rate.ref_epoch_j2000_s)
            {
                dclock_m += high_rate.c0_m;
            }
        }
        let corrected_clock_s = state.clock.dt_clock_total_s + dclock_m / C_M_S;
        Some((corrected_position, corrected_clock_s))
    }

    fn correction_fresh(&self, t_j2000_s: f64, ref_epoch_j2000_s: f64) -> bool {
        let age = (t_j2000_s - ref_epoch_j2000_s).abs();
        age.is_finite() && age <= self.staleness.max_staleness_s
    }

    fn regional_allowed(&self, provider_id: u16) -> bool {
        match &self.fallback.regional {
            RegionalPolicy::DeclineRegional => false,
            RegionalPolicy::AllowProviders(providers) => providers.contains(&provider_id),
        }
    }

    fn broadcast_fallback(&self, sat: GnssSatelliteId, t_j2000_s: f64) -> Option<([f64; 3], f64)> {
        if self.fallback.on_missing_correction == MissingCorrectionAction::FallBackToBroadcast {
            self.broadcast.position_clock_at_j2000_s(sat, t_j2000_s)
        } else {
            None
        }
    }
}

impl EphemerisSource for SsrCorrectedEphemeris<'_> {
    fn position_clock_at_j2000_s(
        &self,
        sat: GnssSatelliteId,
        t_j2000_s: f64,
    ) -> Option<([f64; 3], f64)> {
        self.corrected_state(sat, t_j2000_s)
    }
}

impl ObservableEphemerisSource for SsrCorrectedEphemeris<'_> {
    fn observable_state_at_j2000_s(
        &self,
        sat: GnssSatelliteId,
        t_j2000_s: f64,
    ) -> std::result::Result<ObservableState, ObservablesError> {
        let Some((position_ecef_m, clock_s)) = self.corrected_state(sat, t_j2000_s) else {
            return Err(ObservablesError::NoEphemeris);
        };
        Ok(ObservableState {
            position_ecef_m,
            clock_s: Some(clock_s),
        })
    }
}

/// Owned corrected ephemeris source.
#[derive(Clone)]
pub struct SsrCorrectedEphemerisOwned {
    broadcast: Arc<BroadcastEphemeris>,
    store: Arc<SsrCorrectionStore>,
    staleness: StalenessPolicy,
    fallback: SsrFallbackPolicy,
}

impl SsrCorrectedEphemerisOwned {
    /// Build an owned corrected source.
    pub fn new(broadcast: Arc<BroadcastEphemeris>, store: Arc<SsrCorrectionStore>) -> Self {
        let staleness = store.staleness();
        Self {
            broadcast,
            store,
            staleness,
            fallback: SsrFallbackPolicy::default(),
        }
    }

    /// Set the staleness policy.
    pub fn with_staleness(mut self, policy: StalenessPolicy) -> Self {
        self.staleness = policy;
        self
    }

    /// Set missing-correction and regional behavior.
    pub fn with_fallback(mut self, policy: SsrFallbackPolicy) -> Self {
        self.fallback = policy;
        self
    }

    /// Mark one regional provider as applicable.
    pub fn allow_regional_provider(mut self, provider_id: u16) -> Self {
        match &mut self.fallback.regional {
            RegionalPolicy::DeclineRegional => {
                let mut providers = BTreeSet::new();
                providers.insert(provider_id);
                self.fallback.regional = RegionalPolicy::AllowProviders(providers);
            }
            RegionalPolicy::AllowProviders(providers) => {
                providers.insert(provider_id);
            }
        }
        self
    }

    fn borrowed(&self) -> SsrCorrectedEphemeris<'_> {
        SsrCorrectedEphemeris::new(&self.broadcast, &self.store)
            .with_staleness(self.staleness)
            .with_fallback(self.fallback.clone())
    }
}

impl EphemerisSource for SsrCorrectedEphemerisOwned {
    fn position_clock_at_j2000_s(
        &self,
        sat: GnssSatelliteId,
        t_j2000_s: f64,
    ) -> Option<([f64; 3], f64)> {
        self.borrowed().position_clock_at_j2000_s(sat, t_j2000_s)
    }
}

impl ObservableEphemerisSource for SsrCorrectedEphemerisOwned {
    fn observable_state_at_j2000_s(
        &self,
        sat: GnssSatelliteId,
        t_j2000_s: f64,
    ) -> std::result::Result<ObservableState, ObservablesError> {
        self.borrowed().observable_state_at_j2000_s(sat, t_j2000_s)
    }
}

fn update_interval_s(index: u8) -> f64 {
    const TABLE: [f64; 16] = [
        1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 240.0, 300.0, 600.0, 900.0, 1800.0, 3600.0,
        7200.0, 10800.0,
    ];
    TABLE[usize::from(index)]
}

fn ssr_epoch_j2000_s(
    system: GnssSystem,
    week: GnssWeekTow,
    epoch_time_s: u32,
    update_interval_s: f64,
) -> Result<f64> {
    let scale = match system {
        GnssSystem::Galileo => TimeScale::Gst,
        GnssSystem::BeiDou => TimeScale::Bdt,
        _ => TimeScale::Gpst,
    };
    let normalized = GnssWeekTow::new(
        scale,
        week.week,
        f64::from(epoch_time_s) + update_interval_s / 2.0,
    )
    .and_then(GnssWeekTow::normalized)
    .map_err(|_| Error::Parse("SSR epoch is out of range".to_string()))?;
    let continuous = f64::from(normalized.week) * SECONDS_PER_WEEK + normalized.tow_s;
    Ok(match system {
        GnssSystem::BeiDou => {
            continuous
                + crate::constants::BDS_EPOCH_MINUS_GPS_EPOCH_S
                + crate::constants::GPST_MINUS_BDT_S
                - GPS_EPOCH_TO_J2000_S
        }
        _ => continuous - GPS_EPOCH_TO_J2000_S,
    })
}

fn ssr_satellite(system: GnssSystem, satellite_id: u8) -> Result<GnssSatelliteId> {
    GnssSatelliteId::new(system, satellite_id)
        .map_err(|e| Error::Parse(format!("invalid SSR satellite id {satellite_id}: {e}")))
}

fn high_rate_matches(clock: &SsrClockCorrection, high_rate: &SsrHighRateClock) -> bool {
    clock.solution == high_rate.solution && clock.iod_ssr == high_rate.iod_ssr
}

fn default_nav_message(system: GnssSystem) -> Option<NavMessage> {
    match system {
        GnssSystem::Gps => Some(NavMessage::GpsLnav),
        GnssSystem::Galileo => Some(NavMessage::GalileoInav),
        GnssSystem::BeiDou => Some(NavMessage::BeidouD1),
        _ => None,
    }
}

fn continuous_time_for_sat(sat: GnssSatelliteId, t_j2000_s: f64) -> Option<(f64, bool)> {
    if !matches!(
        sat.system,
        GnssSystem::Gps | GnssSystem::Galileo | GnssSystem::BeiDou
    ) {
        return None;
    }
    let gpst_continuous = t_j2000_s + GPS_EPOCH_TO_J2000_S;
    if sat.system == GnssSystem::BeiDou {
        Some((
            gpst_continuous
                - crate::constants::GPST_MINUS_BDT_S
                - crate::constants::BDS_EPOCH_MINUS_GPS_EPOCH_S,
            is_beidou_geo(sat),
        ))
    } else {
        Some((gpst_continuous, false))
    }
}

fn broadcast_velocity(
    record: &crate::rinex_nav::BroadcastRecord,
    sat: GnssSatelliteId,
    t_j2000_s: f64,
    is_geo: bool,
) -> Option<[f64; 3]> {
    let p_plus = broadcast_position_from_record(record, sat, t_j2000_s + FD_HALF_S, is_geo)?;
    let p_minus = broadcast_position_from_record(record, sat, t_j2000_s - FD_HALF_S, is_geo)?;
    let denom = 2.0 * FD_HALF_S;
    Some([
        (p_plus[0] - p_minus[0]) / denom,
        (p_plus[1] - p_minus[1]) / denom,
        (p_plus[2] - p_minus[2]) / denom,
    ])
}

fn broadcast_position_from_record(
    record: &crate::rinex_nav::BroadcastRecord,
    sat: GnssSatelliteId,
    t_j2000_s: f64,
    is_geo: bool,
) -> Option<[f64; 3]> {
    let (t_continuous_s, _) = continuous_time_for_sat(sat, t_j2000_s)?;
    let sow = t_continuous_s.rem_euclid(SECONDS_PER_WEEK);
    satellite_state_unchecked(
        &record.elements,
        &record.clock,
        &record.constants(),
        sow,
        record.broadcast_clock_group_delay_s(),
        is_geo,
    )
    .orbit
    .position()
    .ok()
    .map(|p| p.as_array())
}

fn velocity_aligned_basis(r: [f64; 3], v: [f64; 3]) -> Option<([f64; 3], [f64; 3], [f64; 3])> {
    let ea = normalize(v)?;
    let rc = cross(r, v);
    let ec = normalize(rc)?;
    let er = cross(ea, ec);
    Some((er, ea, ec))
}

fn normalize(v: [f64; 3]) -> Option<[f64; 3]> {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if n > 0.0 && n.is_finite() {
        Some([v[0] / n, v[1] / n, v[2] / n])
    } else {
        None
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtcm::{
        SsrClockRecord, SsrHeader, SsrOrbitRecord, SsrPhaseBiasRecord, SsrPhaseBiasSignal,
        SsrStreamAssembler,
    };
    use crate::sp3::Sp3;

    const REAL_SSRA02IGS0_1060_FRAME_HEX: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ssr/SSRA02IGS0_2026181234930_1060.hex"
    ));
    const REAL_SSR_WEEK: u32 = 2425;
    const REAL_SSR_EPOCH_TOW_S: f64 = 344_970.0;

    fn header(kind: SsrKind) -> SsrHeader {
        SsrHeader {
            epoch_time_s: 100_000,
            update_interval: 3,
            multiple_message: false,
            iod_ssr: 4,
            provider_id: 7,
            solution_id: 2,
            satellite_reference_datum: matches!(kind, SsrKind::Orbit | SsrKind::CombinedOrbitClock)
                .then_some(false),
            dispersive_bias_consistency: None,
            mw_consistency: None,
            satellite_count: 1,
        }
    }

    #[test]
    fn rtcm_ingest_scales_orbit_and_clock() {
        let message = SsrMessage {
            message_number: 1060,
            system: GnssSystem::Gps,
            kind: SsrKind::CombinedOrbitClock,
            header: header(SsrKind::CombinedOrbitClock),
            orbit: vec![SsrOrbitRecord {
                satellite_id: 1,
                iode: 42,
                delta_radial: 10_000,
                delta_along: -20_000,
                delta_cross: 30_000,
                dot_delta_radial: 100,
                dot_delta_along: -200,
                dot_delta_cross: 300,
            }],
            clock: vec![SsrClockRecord {
                satellite_id: 1,
                c0: 10_000,
                c1: -2_000,
                c2: 300,
            }],
            code_bias: Vec::new(),
            phase_bias: Vec::<SsrPhaseBiasRecord>::new(),
            ura: Vec::new(),
            padding_bits: Vec::new(),
        };
        let mut store = SsrCorrectionStore::new();
        let week = GnssWeekTow::new(TimeScale::Gpst, 2_400, 100_000.0).unwrap();
        store.ingest_ssr(&message, week).unwrap();
        let sat = GnssSatelliteId::new(GnssSystem::Gps, 1).unwrap();
        let orbit = store.orbit(sat).unwrap();
        assert_eq!(orbit.iode, 42);
        assert_eq!(orbit.radial_m.to_bits(), (-1.0_f64).to_bits());
        assert_eq!(orbit.along_m.to_bits(), 8.0_f64.to_bits());
        assert_eq!(orbit.cross_m.to_bits(), (-12.0_f64).to_bits());
        assert!((orbit.radial_rate_m_s + 1.0e-4).abs() < 1.0e-18);
        let clock = store.clock(sat).unwrap();
        assert_eq!(clock.c0_m.to_bits(), 1.0_f64.to_bits());
        assert!((clock.c1_m_s + 0.002).abs() < 1.0e-18);
        assert!((clock.c2_m_s2 - 6.0e-6).abs() < 1.0e-18);
    }

    #[test]
    fn high_rate_clock_is_additive_when_identity_matches() {
        let mut store = SsrCorrectionStore::new();
        let week = GnssWeekTow::new(TimeScale::Gpst, 2_400, 100_000.0).unwrap();
        let low = SsrMessage {
            message_number: 1058,
            system: GnssSystem::Gps,
            kind: SsrKind::Clock,
            header: header(SsrKind::Clock),
            orbit: Vec::new(),
            clock: vec![SsrClockRecord {
                satellite_id: 1,
                c0: 1_000,
                c1: 0,
                c2: 0,
            }],
            code_bias: Vec::new(),
            phase_bias: Vec::<SsrPhaseBiasRecord>::new(),
            ura: Vec::new(),
            padding_bits: Vec::new(),
        };
        let high = SsrMessage {
            message_number: 1062,
            system: GnssSystem::Gps,
            kind: SsrKind::HighRateClock,
            header: header(SsrKind::HighRateClock),
            orbit: Vec::new(),
            clock: vec![SsrClockRecord {
                satellite_id: 1,
                c0: 500,
                c1: 0,
                c2: 0,
            }],
            code_bias: Vec::new(),
            phase_bias: vec![SsrPhaseBiasRecord {
                satellite_id: 1,
                yaw_angle: 0,
                yaw_rate: 0,
                biases: vec![SsrPhaseBiasSignal {
                    signal_id: 1,
                    integer_indicator: 0,
                    wide_lane_integer_indicator: 0,
                    discontinuity_counter: 0,
                    bias: 0,
                }],
            }],
            ura: Vec::new(),
            padding_bits: Vec::new(),
        };
        store.ingest_ssr(&high, week).unwrap();
        store.ingest_ssr(&low, week).unwrap();
        let sat = GnssSatelliteId::new(GnssSystem::Gps, 1).unwrap();
        assert_eq!(
            store.clock(sat).unwrap().high_rate.unwrap().c0_m.to_bits(),
            0.05_f64.to_bits()
        );
    }

    #[test]
    fn corrected_ephemeris_uses_real_rtcm_broadcast_and_sp3_products() {
        let nav_text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/ssr/BRDC00WRD_S_20261820000_G30_G31.rnx"
        ))
        .expect("read NAV fixture");
        let broadcast = BroadcastEphemeris::from_nav(&nav_text).expect("parse NAV fixture");
        let sp3_bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/ssr/IGS0OPSULT_20261811800_02D_15M_ORB.SP3"
        ))
        .expect("read SP3 fixture");
        let sp3 = Sp3::parse(&sp3_bytes).expect("parse SP3 fixture");
        let store = real_gps_ssr_store();
        let source = SsrCorrectedEphemeris::new(&broadcast, &store)
            .with_staleness(StalenessPolicy::seconds(60.0));
        let t = ssr_j2000(REAL_SSR_EPOCH_TOW_S);

        let mut orbit_error_sum_m2 = 0.0;
        let mut clock_error_sum_ns2 = 0.0;
        let mut count = 0_usize;
        for sat in [
            GnssSatelliteId::new(GnssSystem::Gps, 30).unwrap(),
            GnssSatelliteId::new(GnssSystem::Gps, 31).unwrap(),
        ] {
            let (corrected_position, corrected_clock) =
                source.corrected_state(sat, t).expect("corrected state");
            let sp3_state = sp3.position_at_j2000_seconds(sat, t).expect("SP3 state");
            let sp3_position = sp3_state.position.as_array();
            let sp3_clock = sp3_state.clock_s.expect("SP3 clock");
            let orbit_error_m = norm([
                corrected_position[0] - sp3_position[0],
                corrected_position[1] - sp3_position[1],
                corrected_position[2] - sp3_position[2],
            ]);
            let clock_error_ns = (corrected_clock - sp3_clock) * 1.0e9;
            orbit_error_sum_m2 += orbit_error_m * orbit_error_m;
            clock_error_sum_ns2 += clock_error_ns * clock_error_ns;
            count += 1;
        }
        assert_eq!(count, 2);
        let orbit_rms_m = (orbit_error_sum_m2 / count as f64).sqrt();
        let clock_rms_ns = (clock_error_sum_ns2 / count as f64).sqrt();

        // The fixture is the closest public triple assembled on 2026-07-02:
        // captured SSRA02IGS0, matching broadcast records, and ultra-rapid SP3.
        // A rapid or final SP3 for this UTC day was not available at capture time.
        assert!(orbit_rms_m < 1.6, "{orbit_rms_m}");
        assert!(clock_rms_ns < 22.0, "{clock_rms_ns}");
    }

    #[test]
    fn corrected_position_matches_rtklib_satpos_ssr_oracle_for_one_epoch() {
        let nav_text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/ssr/BRDC00WRD_S_20261820000_G30_G31.rnx"
        ))
        .expect("read NAV fixture");
        let broadcast = BroadcastEphemeris::from_nav(&nav_text).expect("parse NAV fixture");
        let store = real_gps_ssr_store();
        let source = SsrCorrectedEphemeris::new(&broadcast, &store)
            .with_staleness(StalenessPolicy::seconds(60.0));
        let sat = GnssSatelliteId::new(GnssSystem::Gps, 30).unwrap();
        let (position, _clock) = source
            .corrected_state(sat, ssr_j2000(REAL_SSR_EPOCH_TOW_S))
            .expect("corrected state");
        let rtklib_position = [
            -6_327_381.424_159_626,
            15_802_129.789_888_298,
            -20_121_898.098_271_403,
        ];
        let position_error_m = norm([
            position[0] - rtklib_position[0],
            position[1] - rtklib_position[1],
            position[2] - rtklib_position[2],
        ]);
        assert!(position_error_m < 1.0e-6, "{position_error_m}");
    }

    fn real_gps_ssr_store() -> SsrCorrectionStore {
        let mut assembler = SsrStreamAssembler::new();
        let mut store = SsrCorrectionStore::new();
        let week = GnssWeekTow::new(TimeScale::Gpst, REAL_SSR_WEEK, REAL_SSR_EPOCH_TOW_S)
            .expect("valid SSR week");
        for decoded in assembler.push(&hex_bytes(REAL_SSRA02IGS0_1060_FRAME_HEX)) {
            let message = decoded.expect("decode SSR frame");
            store.ingest(&message, week).expect("ingest SSR frame");
        }
        assert_eq!(assembler.retained_len(), 0);
        store
    }

    fn ssr_j2000(tow_s: f64) -> f64 {
        f64::from(REAL_SSR_WEEK) * SECONDS_PER_WEEK + tow_s - GPS_EPOCH_TO_J2000_S
    }

    fn hex_bytes(hex: &str) -> Vec<u8> {
        let compact: String = hex.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        assert_eq!(compact.len() % 2, 0);
        compact
            .as_bytes()
            .chunks_exact(2)
            .map(|chunk| {
                let hi = (chunk[0] as char).to_digit(16).unwrap();
                let lo = (chunk[1] as char).to_digit(16).unwrap();
                ((hi << 4) | lo) as u8
            })
            .collect()
    }

    fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    fn norm(v: [f64; 3]) -> f64 {
        dot(v, v).sqrt()
    }
}
