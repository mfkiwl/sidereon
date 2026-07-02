//! RINEX observation/navigation lint and mechanical repair.
//!
//! This module is a sans-I/O layer over the existing RINEX and CRINEX readers.
//! It does not implement a parser. Text entry points decode through the owning
//! modules, then report typed findings derived from the parsed products.

use std::collections::{BTreeMap, BTreeSet};

use crate::astro::time::model::TimeScale;
use crate::crinex;
use crate::id::{GnssSatelliteId, GnssSystem};
use crate::rinex_nav::{
    parse_iono_corrections, parse_leap_seconds, parse_nav, BroadcastRecord, IonoCorrections,
    NavMessage, NavParseError,
};
use crate::rinex_obs::{ObsEpoch, ObsEpochTime, ObsHeader, RinexObs};
use crate::Result;

const EARTH_FIXED_RADIUS_MIN_M: f64 = 6_300_000.0;
const EARTH_FIXED_RADIUS_MAX_M: f64 = 6_400_000.0;

/// Severity assigned to a lint finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The file cannot be represented by the existing parsed product.
    Fatal,
    /// The parsed product violates a standard-required invariant.
    Error,
    /// The parsed product is suspicious or will lose information in this slice.
    Warning,
    /// A useful fact about product scope or content.
    Info,
}

/// Location associated with a lint finding when known.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FindingRef {
    /// Zero-based epoch index.
    pub epoch_index: Option<usize>,
    /// Satellite token.
    pub satellite: Option<String>,
    /// Header or record field name.
    pub field: Option<&'static str>,
}

impl FindingRef {
    fn field(field: &'static str) -> Self {
        Self {
            field: Some(field),
            ..Self::default()
        }
    }

    fn epoch(epoch_index: usize) -> Self {
        Self {
            epoch_index: Some(epoch_index),
            ..Self::default()
        }
    }

    fn sat(epoch_index: usize, sat: GnssSatelliteId) -> Self {
        Self {
            epoch_index: Some(epoch_index),
            satellite: Some(sat.to_string()),
            ..Self::default()
        }
    }
}

/// A typed RINEX lint finding.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Finding {
    /// OBS parse failed before a parsed product existed.
    ObsFatalParse { at: FindingRef, message: String },
    /// OBS version is not one of the published 3.0x/4.0x versions covered here.
    ObsUnpublishedVersion { at: FindingRef, version: f64 },
    /// A mandatory OBS header retained by the current product is absent.
    ObsMissingHeader { at: FindingRef, label: &'static str },
    /// OBS header has no observation-code table.
    ObsMissingObsTypes { at: FindingRef },
    /// OBS code syntax is not valid for this first slice.
    ObsInvalidObsCode {
        at: FindingRef,
        system: GnssSystem,
        code: String,
    },
    /// OBS code is duplicated in one system table.
    ObsDuplicateObsCode {
        at: FindingRef,
        system: GnssSystem,
        code: String,
    },
    /// TIME OF FIRST OBS disagrees with the body.
    ObsTimeOfFirstMismatch {
        at: FindingRef,
        declared: ObsEpochTime,
        observed: ObsEpochTime,
    },
    /// INTERVAL disagrees with the dominant epoch spacing.
    ObsIntervalMismatch {
        at: FindingRef,
        declared_s: f64,
        observed_s: f64,
    },
    /// GLONASS observations need a valid slot/frequency table.
    ObsGlonassSlotIssue {
        at: FindingRef,
        satellite: GnssSatelliteId,
        issue: &'static str,
    },
    /// SYS / PHASE SHIFT names a code absent from SYS / # / OBS TYPES.
    ObsPhaseShiftUndeclaredCode {
        at: FindingRef,
        system: GnssSystem,
        code: String,
    },
    /// SYS / SCALE FACTOR is invalid or names an undeclared code.
    ObsScaleFactorIssue {
        at: FindingRef,
        system: GnssSystem,
        code: Option<String>,
    },
    /// Approximate position is implausible for a fixed marker.
    ObsImplausibleApproxPosition { at: FindingRef, radius_m: f64 },
    /// Antenna height/east/north offset is implausible.
    ObsImplausibleAntennaDelta {
        at: FindingRef,
        component: usize,
        value_m: f64,
    },
    /// Epoch times are not strictly increasing.
    ObsEpochOrder {
        at: FindingRef,
        previous: ObsEpochTime,
        current: ObsEpochTime,
    },
    /// Two normal epochs carry the same timestamp.
    ObsDuplicateEpoch { at: FindingRef, epoch: ObsEpochTime },
    /// The parser skipped satellite records it could not represent.
    ObsSkippedRecords { at: FindingRef, count: usize },
    /// A pseudorange value is outside the configured plausibility window.
    ObsPseudorangeOutOfRange {
        at: FindingRef,
        code: String,
        value_m: f64,
    },
    /// LLI digit is outside the three defined bits.
    ObsLossOfLockOutOfRange {
        at: FindingRef,
        code: String,
        lli: u8,
    },
    /// Event epoch retained with no special records.
    ObsEventEpoch { at: FindingRef, flag: u8 },
    /// Satellite record has all observation fields blank.
    ObsEmptySatelliteRecord { at: FindingRef },
    /// Epoch gap is larger than 1.5 times the dominant interval.
    ObsEpochGap {
        at: FindingRef,
        gap_s: f64,
        interval_s: f64,
    },
    /// NAV parse failed before a parsed product existed.
    NavFatalParse { at: FindingRef, message: String },
    /// NAV header has no LEAP SECONDS record.
    NavLeapSecondsAbsent { at: FindingRef },
    /// NAV ionospheric correction records are malformed.
    NavIonoMalformed { at: FindingRef, message: String },
    /// Duplicate NAV records share an identity.
    NavDuplicateRecord {
        at: FindingRef,
        satellite: GnssSatelliteId,
        same_payload: bool,
    },
    /// NAV records are not in canonical order.
    NavUnsortedRecords { at: FindingRef },
    /// NAV broadcast fields are outside this slice's plausibility limits.
    NavImplausibleRecord {
        at: FindingRef,
        satellite: GnssSatelliteId,
        field: &'static str,
        value: f64,
    },
    /// NAV records include unhealthy satellite records.
    NavUnhealthyRecords {
        at: FindingRef,
        system: GnssSystem,
        count: usize,
    },
}

impl Finding {
    /// Stable rule identifier.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ObsFatalParse { .. } => "OBS-H01",
            Self::ObsUnpublishedVersion { .. } => "OBS-H02",
            Self::ObsMissingHeader { .. } => "OBS-H03",
            Self::ObsMissingObsTypes { .. } => "OBS-H04",
            Self::ObsInvalidObsCode { .. } => "OBS-H05",
            Self::ObsDuplicateObsCode { .. } => "OBS-H06",
            Self::ObsTimeOfFirstMismatch { .. } => "OBS-H07",
            Self::ObsIntervalMismatch { .. } => "OBS-H09",
            Self::ObsGlonassSlotIssue { .. } => "OBS-H12",
            Self::ObsPhaseShiftUndeclaredCode { .. } => "OBS-H13",
            Self::ObsScaleFactorIssue { .. } => "OBS-H14",
            Self::ObsImplausibleApproxPosition { .. } => "OBS-H17",
            Self::ObsImplausibleAntennaDelta { .. } => "OBS-H18",
            Self::ObsEpochOrder { .. } => "OBS-B01",
            Self::ObsDuplicateEpoch { .. } => "OBS-B02",
            Self::ObsSkippedRecords { .. } => "OBS-B04",
            Self::ObsPseudorangeOutOfRange { .. } => "OBS-B05",
            Self::ObsLossOfLockOutOfRange { .. } => "OBS-B06",
            Self::ObsEventEpoch { .. } => "OBS-B07",
            Self::ObsEmptySatelliteRecord { .. } => "OBS-B08",
            Self::ObsEpochGap { .. } => "OBS-B09",
            Self::NavFatalParse { .. } => "NAV-H01",
            Self::NavLeapSecondsAbsent { .. } => "NAV-H02",
            Self::NavIonoMalformed { .. } => "NAV-H03",
            Self::NavDuplicateRecord { .. } => "NAV-B02",
            Self::NavUnsortedRecords { .. } => "NAV-B03",
            Self::NavImplausibleRecord { .. } => "NAV-B04",
            Self::NavUnhealthyRecords { .. } => "NAV-B05",
        }
    }

    /// Rule severity.
    pub const fn severity(&self) -> Severity {
        match self {
            Self::ObsFatalParse { .. } | Self::NavFatalParse { .. } => Severity::Fatal,
            Self::ObsUnpublishedVersion { .. }
            | Self::ObsSkippedRecords { .. }
            | Self::ObsPseudorangeOutOfRange { .. }
            | Self::ObsLossOfLockOutOfRange { .. }
            | Self::ObsIntervalMismatch { .. }
            | Self::ObsPhaseShiftUndeclaredCode { .. }
            | Self::ObsImplausibleApproxPosition { .. }
            | Self::ObsImplausibleAntennaDelta { .. }
            | Self::NavIonoMalformed { .. }
            | Self::NavImplausibleRecord { .. } => Severity::Warning,
            Self::ObsEventEpoch { .. }
            | Self::ObsEmptySatelliteRecord { .. }
            | Self::ObsEpochGap { .. }
            | Self::NavLeapSecondsAbsent { .. }
            | Self::NavUnsortedRecords { .. }
            | Self::NavUnhealthyRecords { .. } => Severity::Info,
            Self::NavDuplicateRecord { same_payload, .. } => {
                if *same_payload {
                    Severity::Warning
                } else {
                    Severity::Error
                }
            }
            _ => Severity::Error,
        }
    }

    /// Standard or policy reference for the rule.
    pub const fn spec_ref(&self) -> &'static str {
        match self {
            Self::ObsFatalParse { .. } => "RINEX 3.05/4.02 Table A2",
            Self::ObsUnpublishedVersion { .. } => "RINEX version history",
            Self::ObsMissingHeader { .. } => "RINEX 3.05/4.02 Table A2",
            Self::ObsMissingObsTypes { .. } => "RINEX 3.05/4.02 Table A2",
            Self::ObsInvalidObsCode { .. } => "RINEX 3.05 Tables 13-20",
            Self::ObsDuplicateObsCode { .. } => "RINEX 3.05 section 5.2",
            Self::ObsTimeOfFirstMismatch { .. } => "RINEX 3.05 Table A2",
            Self::ObsIntervalMismatch { .. } => "RINEX 3.05 Table A2",
            Self::ObsGlonassSlotIssue { .. } => "RINEX 3.05 Table A2",
            Self::ObsPhaseShiftUndeclaredCode { .. } => "RINEX 3.05 Table A2",
            Self::ObsScaleFactorIssue { .. } => "RINEX 3.05 Table A2",
            Self::ObsImplausibleApproxPosition { .. } => "RINEX 3.05 Table A2",
            Self::ObsImplausibleAntennaDelta { .. } => "RINEX 3.05 Table A2",
            Self::ObsEpochOrder { .. } => "RINEX 3.05 Table A3",
            Self::ObsDuplicateEpoch { .. } => "RINEX 3.05 Table A3",
            Self::ObsSkippedRecords { .. } => "parser diagnostic",
            Self::ObsPseudorangeOutOfRange { .. } => "sidereon RINEX QC policy",
            Self::ObsLossOfLockOutOfRange { .. } => "RINEX 3.05 Table A3 note 1",
            Self::ObsEventEpoch { .. } => "RINEX 3.05 Table A3",
            Self::ObsEmptySatelliteRecord { .. } => "sidereon RINEX QC policy",
            Self::ObsEpochGap { .. } => "sidereon RINEX QC policy",
            Self::NavFatalParse { .. } => "RINEX 3.05 Table A5 / RINEX 4.02 Table A7",
            Self::NavLeapSecondsAbsent { .. } => "RINEX 3.05 Table A5",
            Self::NavIonoMalformed { .. } => "RINEX 3.05 Table A5",
            Self::NavDuplicateRecord { .. } => "RINEX 3.05 section 6.12",
            Self::NavUnsortedRecords { .. } => "sidereon RINEX QC policy",
            Self::NavImplausibleRecord { .. } => "sidereon RINEX QC policy",
            Self::NavUnhealthyRecords { .. } => "RINEX 3.05 broadcast record layout",
        }
    }

    /// Finding location.
    pub const fn at(&self) -> &FindingRef {
        match self {
            Self::ObsFatalParse { at, .. }
            | Self::ObsUnpublishedVersion { at, .. }
            | Self::ObsMissingHeader { at, .. }
            | Self::ObsMissingObsTypes { at }
            | Self::ObsInvalidObsCode { at, .. }
            | Self::ObsDuplicateObsCode { at, .. }
            | Self::ObsTimeOfFirstMismatch { at, .. }
            | Self::ObsIntervalMismatch { at, .. }
            | Self::ObsGlonassSlotIssue { at, .. }
            | Self::ObsPhaseShiftUndeclaredCode { at, .. }
            | Self::ObsScaleFactorIssue { at, .. }
            | Self::ObsImplausibleApproxPosition { at, .. }
            | Self::ObsImplausibleAntennaDelta { at, .. }
            | Self::ObsEpochOrder { at, .. }
            | Self::ObsDuplicateEpoch { at, .. }
            | Self::ObsSkippedRecords { at, .. }
            | Self::ObsPseudorangeOutOfRange { at, .. }
            | Self::ObsLossOfLockOutOfRange { at, .. }
            | Self::ObsEventEpoch { at, .. }
            | Self::ObsEmptySatelliteRecord { at }
            | Self::ObsEpochGap { at, .. }
            | Self::NavFatalParse { at, .. }
            | Self::NavLeapSecondsAbsent { at }
            | Self::NavIonoMalformed { at, .. }
            | Self::NavDuplicateRecord { at, .. }
            | Self::NavUnsortedRecords { at }
            | Self::NavImplausibleRecord { at, .. }
            | Self::NavUnhealthyRecords { at, .. } => at,
        }
    }

    /// Whether this finding can be changed by the repair helpers in this slice.
    pub const fn is_repairable(&self) -> bool {
        matches!(
            self,
            Self::ObsTimeOfFirstMismatch { .. }
                | Self::ObsIntervalMismatch { .. }
                | Self::ObsEpochOrder { .. }
                | Self::ObsDuplicateEpoch { .. }
                | Self::ObsEmptySatelliteRecord { .. }
                | Self::NavDuplicateRecord {
                    same_payload: true,
                    ..
                }
                | Self::NavUnsortedRecords { .. }
        )
    }
}

/// Lint result.
#[derive(Debug, Clone, PartialEq)]
pub struct LintReport {
    /// Findings in deterministic rule order.
    pub findings: Vec<Finding>,
    /// Whether a CRINEX input was decoded before linting.
    pub decoded_from_crinex: bool,
}

impl LintReport {
    /// Clean means no fatal or error findings.
    pub fn is_clean(&self) -> bool {
        self.findings
            .iter()
            .all(|f| !matches!(f.severity(), Severity::Fatal | Severity::Error))
    }

    /// Count findings by severity.
    pub fn count(&self, severity: Severity) -> usize {
        self.findings
            .iter()
            .filter(|finding| finding.severity() == severity)
            .count()
    }
}

/// Repair options for the first core slice.
#[derive(Debug, Clone, PartialEq)]
pub struct RepairOptions {
    /// Set `INTERVAL` to the dominant normal-epoch spacing.
    pub set_interval: bool,
    /// Drop satellite rows whose observation fields are all blank.
    pub drop_empty_records: bool,
    /// Sort NAV records by satellite and toc.
    pub sort_records: bool,
}

impl Default for RepairOptions {
    fn default() -> Self {
        Self {
            set_interval: false,
            drop_empty_records: false,
            sort_records: true,
        }
    }
}

/// One mechanical repair action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairAction {
    /// Catalog action id, e.g. `A3`.
    pub id: &'static str,
    /// Short action description.
    pub message: String,
}

/// Observation repair result.
#[derive(Debug, Clone, PartialEq)]
pub struct ObsRepair {
    /// Repaired parsed product.
    pub repaired: RinexObs,
    /// Actions applied.
    pub actions: Vec<RepairAction>,
    /// Lint report after repair.
    pub remaining: LintReport,
    /// Whether text input was decoded from CRINEX.
    pub decoded_from_crinex: bool,
}

/// Navigation repair result.
#[derive(Debug, Clone, PartialEq)]
pub struct NavRepair {
    /// Repaired broadcast records.
    pub records: Vec<BroadcastRecord>,
    /// Header ionospheric corrections parsed from text, if available.
    pub iono: Option<IonoCorrections>,
    /// Header leap-second count parsed from text, if available.
    pub leap_seconds: Option<f64>,
    /// Actions applied.
    pub actions: Vec<RepairAction>,
    /// Lint report after repair.
    pub remaining: LintReport,
}

/// Lint an already parsed observation product.
pub fn lint_obs(obs: &RinexObs) -> LintReport {
    LintReport {
        findings: obs_findings(obs),
        decoded_from_crinex: false,
    }
}

/// CRINEX-transparent observation lint entry point.
pub fn lint_obs_text(text: &str) -> LintReport {
    let (decoded_from_crinex, text) = match decode_if_crinex(text) {
        Ok(v) => v,
        Err(error) => {
            return LintReport {
                findings: vec![Finding::ObsFatalParse {
                    at: FindingRef::default(),
                    message: error.to_string(),
                }],
                decoded_from_crinex: true,
            };
        }
    };
    match RinexObs::parse(&text) {
        Ok(obs) => LintReport {
            findings: obs_findings(&obs),
            decoded_from_crinex,
        },
        Err(error) => LintReport {
            findings: vec![Finding::ObsFatalParse {
                at: FindingRef::default(),
                message: error.to_string(),
            }],
            decoded_from_crinex,
        },
    }
}

/// Lint navigation text with the existing NAV parser and header readers.
pub fn lint_nav_text(text: &str) -> LintReport {
    let mut findings = Vec::new();
    match parse_nav(text) {
        Ok(records) => findings.extend(nav_findings(&records)),
        Err(error) => findings.push(Finding::NavFatalParse {
            at: FindingRef::default(),
            message: error.to_string(),
        }),
    }
    if matches!(parse_leap_seconds(text), Ok(None)) {
        findings.push(Finding::NavLeapSecondsAbsent {
            at: FindingRef::field("LEAP SECONDS"),
        });
    }
    if let Err(error) = parse_iono_corrections(text) {
        findings.push(Finding::NavIonoMalformed {
            at: FindingRef::field("IONOSPHERIC CORR"),
            message: error.to_string(),
        });
    }
    LintReport {
        findings,
        decoded_from_crinex: false,
    }
}

/// Repair an already parsed observation product.
pub fn repair_obs(obs: &RinexObs, options: &RepairOptions) -> ObsRepair {
    let mut repaired = obs.clone();
    let mut actions = Vec::new();
    repair_obs_order_and_duplicates(&mut repaired, &mut actions);
    repair_obs_time_of_first(&mut repaired, &mut actions);
    if options.set_interval {
        repair_obs_interval(&mut repaired, &mut actions);
    }
    if options.drop_empty_records {
        repair_obs_empty_records(&mut repaired, &mut actions);
    }
    let remaining = lint_obs(&repaired);
    ObsRepair {
        repaired,
        actions,
        remaining,
        decoded_from_crinex: false,
    }
}

/// CRINEX-transparent observation repair entry point.
pub fn repair_obs_text(text: &str, options: &RepairOptions) -> Result<ObsRepair> {
    let (decoded_from_crinex, text) = decode_if_crinex(text)?;
    let obs = RinexObs::parse(&text)?;
    let mut repaired = repair_obs(&obs, options);
    repaired.decoded_from_crinex = decoded_from_crinex;
    repaired.remaining.decoded_from_crinex = decoded_from_crinex;
    Ok(repaired)
}

/// Encode an observation repair product as CRINEX through the existing codec.
pub fn repair_obs_to_crinex_string(repair: &ObsRepair) -> Result<String> {
    crinex::encode_crinex(&repair.repaired.to_rinex_string())
}

/// Repair parsed navigation records.
pub fn repair_nav(records: &[BroadcastRecord], options: &RepairOptions) -> NavRepair {
    let mut records = records.to_vec();
    let mut actions = Vec::new();
    repair_nav_duplicates(&mut records, &mut actions);
    if options.sort_records {
        repair_nav_order(&mut records, &mut actions);
    }
    let remaining = LintReport {
        findings: nav_findings(&records),
        decoded_from_crinex: false,
    };
    NavRepair {
        records,
        iono: None,
        leap_seconds: None,
        actions,
        remaining,
    }
}

/// Repair navigation text through the existing parser.
pub fn repair_nav_text(
    text: &str,
    options: &RepairOptions,
) -> std::result::Result<NavRepair, NavParseError> {
    let records = parse_nav(text)?;
    let mut repair = repair_nav(&records, options);
    repair.iono = parse_iono_corrections(text).ok();
    repair.leap_seconds = parse_leap_seconds(text).ok().flatten();
    Ok(repair)
}

fn decode_if_crinex(text: &str) -> Result<(bool, String)> {
    let is_crinex = text
        .lines()
        .next()
        .is_some_and(|line| line.get(60..80).unwrap_or("").contains("CRINEX VERS"));
    if is_crinex {
        Ok((true, crinex::decode(text)?))
    } else {
        Ok((false, text.to_string()))
    }
}

fn obs_findings(obs: &RinexObs) -> Vec<Finding> {
    let mut findings = Vec::new();
    lint_obs_header(&obs.header, &mut findings);
    lint_obs_body(obs, &mut findings);
    findings
}

fn lint_obs_header(header: &ObsHeader, findings: &mut Vec<Finding>) {
    if !matches!(published_obs_version(header.version), Some(())) {
        findings.push(Finding::ObsUnpublishedVersion {
            at: FindingRef::field("RINEX VERSION / TYPE"),
            version: header.version,
        });
    }
    if header.marker_name.is_none() {
        findings.push(Finding::ObsMissingHeader {
            at: FindingRef::field("MARKER NAME"),
            label: "MARKER NAME",
        });
    }
    if header.antenna_delta_hen_m.is_none() {
        findings.push(Finding::ObsMissingHeader {
            at: FindingRef::field("ANTENNA: DELTA H/E/N"),
            label: "ANTENNA: DELTA H/E/N",
        });
    }
    if header.time_of_first_obs.is_none() {
        findings.push(Finding::ObsMissingHeader {
            at: FindingRef::field("TIME OF FIRST OBS"),
            label: "TIME OF FIRST OBS",
        });
    }
    if header.obs_codes.is_empty() {
        findings.push(Finding::ObsMissingObsTypes {
            at: FindingRef::field("SYS / # / OBS TYPES"),
        });
    }
    for (&system, codes) in &header.obs_codes {
        let mut seen = BTreeSet::new();
        for code in codes {
            if !is_valid_obs_code(system, code, header.version) {
                findings.push(Finding::ObsInvalidObsCode {
                    at: FindingRef::field("SYS / # / OBS TYPES"),
                    system,
                    code: code.clone(),
                });
            }
            if !seen.insert(code.as_str()) {
                findings.push(Finding::ObsDuplicateObsCode {
                    at: FindingRef::field("SYS / # / OBS TYPES"),
                    system,
                    code: code.clone(),
                });
            }
        }
    }
    for shift in &header.phase_shifts {
        if !header
            .obs_codes
            .get(&shift.system)
            .is_some_and(|codes| codes.iter().any(|code| code == &shift.code))
        {
            findings.push(Finding::ObsPhaseShiftUndeclaredCode {
                at: FindingRef::field("SYS / PHASE SHIFT"),
                system: shift.system,
                code: shift.code.clone(),
            });
        }
    }
    for factor in &header.scale_factors {
        if !matches!(factor.factor as i64, 1 | 10 | 100 | 1000) {
            findings.push(Finding::ObsScaleFactorIssue {
                at: FindingRef::field("SYS / SCALE FACTOR"),
                system: factor.system,
                code: None,
            });
        }
        for code in &factor.codes {
            if !header
                .obs_codes
                .get(&factor.system)
                .is_some_and(|codes| codes.iter().any(|declared| declared == code))
            {
                findings.push(Finding::ObsScaleFactorIssue {
                    at: FindingRef::field("SYS / SCALE FACTOR"),
                    system: factor.system,
                    code: Some(code.clone()),
                });
            }
        }
    }
    if let Some(pos) = header.approx_position_m {
        let radius = (pos[0] * pos[0] + pos[1] * pos[1] + pos[2] * pos[2]).sqrt();
        if radius != 0.0 && !(EARTH_FIXED_RADIUS_MIN_M..=EARTH_FIXED_RADIUS_MAX_M).contains(&radius)
        {
            findings.push(Finding::ObsImplausibleApproxPosition {
                at: FindingRef::field("APPROX POSITION XYZ"),
                radius_m: radius,
            });
        }
    }
    if let Some(delta) = header.antenna_delta_hen_m {
        for (idx, value) in delta.into_iter().enumerate() {
            if value.abs() > 100.0 {
                findings.push(Finding::ObsImplausibleAntennaDelta {
                    at: FindingRef::field("ANTENNA: DELTA H/E/N"),
                    component: idx,
                    value_m: value,
                });
            }
        }
    }
}

fn lint_obs_body(obs: &RinexObs, findings: &mut Vec<Finding>) {
    if obs.skipped_records > 0 {
        findings.push(Finding::ObsSkippedRecords {
            at: FindingRef::default(),
            count: obs.skipped_records,
        });
    }
    if let Some(first) = first_normal_epoch(obs) {
        if let Some((declared, _)) = obs.header.time_of_first_obs {
            if !same_epoch_time(declared, first.epoch) {
                findings.push(Finding::ObsTimeOfFirstMismatch {
                    at: FindingRef::field("TIME OF FIRST OBS"),
                    declared,
                    observed: first.epoch,
                });
            }
        }
    }
    lint_obs_epoch_order(obs, findings);
    if let (Some(declared), Some(observed)) =
        (obs.header.interval_s, dominant_interval_s(&obs.epochs))
    {
        if (declared - observed).abs() > 1.0e-6 {
            findings.push(Finding::ObsIntervalMismatch {
                at: FindingRef::field("INTERVAL"),
                declared_s: declared,
                observed_s: observed,
            });
        }
        lint_obs_gaps(obs, observed, findings);
    } else if let Some(observed) = dominant_interval_s(&obs.epochs) {
        lint_obs_gaps(obs, observed, findings);
    }
    lint_obs_glonass_slots(obs, findings);
    lint_obs_values(obs, findings);
}

fn lint_obs_epoch_order(obs: &RinexObs, findings: &mut Vec<Finding>) {
    let mut previous: Option<(usize, ObsEpochTime)> = None;
    let mut seen = BTreeMap::new();
    for (idx, epoch) in obs.epochs.iter().enumerate().filter(|(_, e)| e.flag <= 1) {
        let key = epoch_key(epoch.epoch);
        if let Some((_, prev)) = previous {
            if key < epoch_key(prev) {
                findings.push(Finding::ObsEpochOrder {
                    at: FindingRef::epoch(idx),
                    previous: prev,
                    current: epoch.epoch,
                });
            }
        }
        if seen.insert(key, idx).is_some() {
            findings.push(Finding::ObsDuplicateEpoch {
                at: FindingRef::epoch(idx),
                epoch: epoch.epoch,
            });
        }
        previous = Some((idx, epoch.epoch));
    }
}

fn lint_obs_glonass_slots(obs: &RinexObs, findings: &mut Vec<Finding>) {
    let has_glonass_codes = obs.header.obs_codes.contains_key(&GnssSystem::Glonass);
    if !has_glonass_codes {
        return;
    }
    for epoch in &obs.epochs {
        for sat in epoch
            .sats
            .keys()
            .filter(|sat| sat.system == GnssSystem::Glonass)
        {
            if !obs.header.glonass_slots.contains_key(&sat.prn) {
                findings.push(Finding::ObsGlonassSlotIssue {
                    at: FindingRef {
                        satellite: Some(sat.to_string()),
                        field: Some("GLONASS SLOT / FRQ #"),
                        ..FindingRef::default()
                    },
                    satellite: *sat,
                    issue: "missing slot",
                });
            }
        }
    }
}

fn lint_obs_values(obs: &RinexObs, findings: &mut Vec<Finding>) {
    for (epoch_index, epoch) in obs.epochs.iter().enumerate() {
        if epoch.flag > 1 {
            findings.push(Finding::ObsEventEpoch {
                at: FindingRef::epoch(epoch_index),
                flag: epoch.flag,
            });
            continue;
        }
        for (&sat, values) in &epoch.sats {
            let all_blank = values.iter().all(|value| value.value.is_none());
            if all_blank {
                findings.push(Finding::ObsEmptySatelliteRecord {
                    at: FindingRef::sat(epoch_index, sat),
                });
            }
            let codes = obs.header.obs_codes.get(&sat.system).map(Vec::as_slice);
            for (idx, value) in values.iter().enumerate() {
                let code = codes
                    .and_then(|codes| codes.get(idx))
                    .map_or("", String::as_str);
                if code.starts_with('C') {
                    if let Some(v) = value.value {
                        if !(15_000_000.0..=50_000_000.0).contains(&v) || !v.is_finite() {
                            findings.push(Finding::ObsPseudorangeOutOfRange {
                                at: FindingRef::sat(epoch_index, sat),
                                code: code.to_string(),
                                value_m: v,
                            });
                        }
                    }
                }
                if let Some(lli) = value.lli {
                    if lli > 7 {
                        findings.push(Finding::ObsLossOfLockOutOfRange {
                            at: FindingRef::sat(epoch_index, sat),
                            code: code.to_string(),
                            lli,
                        });
                    }
                }
            }
        }
    }
}

fn lint_obs_gaps(obs: &RinexObs, interval_s: f64, findings: &mut Vec<Finding>) {
    let mut previous: Option<ObsEpochTime> = None;
    for (idx, epoch) in obs.epochs.iter().enumerate().filter(|(_, e)| e.flag <= 1) {
        if let Some(prev) = previous {
            let gap = epoch_seconds(epoch.epoch) - epoch_seconds(prev);
            if gap > interval_s * 1.5 {
                findings.push(Finding::ObsEpochGap {
                    at: FindingRef::epoch(idx),
                    gap_s: gap,
                    interval_s,
                });
            }
        }
        previous = Some(epoch.epoch);
    }
}

fn nav_findings(records: &[BroadcastRecord]) -> Vec<Finding> {
    let mut findings = Vec::new();
    lint_nav_duplicates(records, &mut findings);
    lint_nav_order(records, &mut findings);
    lint_nav_plausibility(records, &mut findings);
    findings
}

fn lint_nav_duplicates(records: &[BroadcastRecord], findings: &mut Vec<Finding>) {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, record) in records.iter().enumerate() {
        let key = nav_identity(record);
        if let Some(first_idx) = seen.get(&key).copied() {
            findings.push(Finding::NavDuplicateRecord {
                at: FindingRef::epoch(idx),
                satellite: record.satellite_id,
                same_payload: records[first_idx] == *record,
            });
        } else {
            seen.insert(key, idx);
        }
    }
}

fn lint_nav_order(records: &[BroadcastRecord], findings: &mut Vec<Finding>) {
    if records
        .windows(2)
        .any(|pair| nav_sort_key(&pair[0]) > nav_sort_key(&pair[1]))
    {
        findings.push(Finding::NavUnsortedRecords {
            at: FindingRef::default(),
        });
    }
}

fn lint_nav_plausibility(records: &[BroadcastRecord], findings: &mut Vec<Finding>) {
    let mut unhealthy: BTreeMap<GnssSystem, usize> = BTreeMap::new();
    for (idx, record) in records.iter().enumerate() {
        if !(0.0..=0.1).contains(&record.elements.e) {
            findings.push(Finding::NavImplausibleRecord {
                at: FindingRef::epoch(idx),
                satellite: record.satellite_id,
                field: "eccentricity",
                value: record.elements.e,
            });
        }
        if !(4_000.0..=8_000.0).contains(&record.elements.sqrt_a) {
            findings.push(Finding::NavImplausibleRecord {
                at: FindingRef::epoch(idx),
                satellite: record.satellite_id,
                field: "sqrt_a",
                value: record.elements.sqrt_a,
            });
        }
        if record.sv_health != 0.0 {
            *unhealthy.entry(record.satellite_id.system).or_default() += 1;
        }
    }
    for (system, count) in unhealthy {
        findings.push(Finding::NavUnhealthyRecords {
            at: FindingRef::default(),
            system,
            count,
        });
    }
}

fn repair_obs_order_and_duplicates(obs: &mut RinexObs, actions: &mut Vec<RepairAction>) {
    if obs.epochs.iter().any(|epoch| epoch.flag > 1) {
        return;
    }
    let before = obs.epochs.clone();
    obs.epochs.sort_by_key(|epoch| epoch_key(epoch.epoch));
    let mut merged: Vec<ObsEpoch> = Vec::new();
    let mut discarded = 0_usize;
    for epoch in obs.epochs.drain(..) {
        if let Some(last) = merged.last_mut() {
            if same_epoch_time(last.epoch, epoch.epoch) {
                for (sat, values) in epoch.sats {
                    if last.sats.insert(sat, values).is_some() {
                        discarded += 1;
                    }
                }
                continue;
            }
        }
        merged.push(epoch);
    }
    obs.epochs = merged;
    if obs.epochs != before {
        actions.push(RepairAction {
            id: "A3",
            message: format!("sorted epochs and merged duplicate epochs, discarded {discarded} duplicate satellite rows"),
        });
    }
}

fn repair_obs_time_of_first(obs: &mut RinexObs, actions: &mut Vec<RepairAction>) {
    let Some(first) = first_normal_epoch(obs).map(|epoch| epoch.epoch) else {
        return;
    };
    let scale = obs
        .header
        .time_of_first_obs
        .map_or(TimeScale::Gpst, |(_, scale)| scale);
    if obs
        .header
        .time_of_first_obs
        .is_none_or(|(declared, _)| !same_epoch_time(declared, first))
    {
        obs.header.time_of_first_obs = Some((first, scale));
        actions.push(RepairAction {
            id: "A4",
            message: "recomputed TIME OF FIRST OBS".to_string(),
        });
    }
}

fn repair_obs_interval(obs: &mut RinexObs, actions: &mut Vec<RepairAction>) {
    let Some(interval) = dominant_interval_s(&obs.epochs) else {
        return;
    };
    if obs
        .header
        .interval_s
        .is_none_or(|declared| (declared - interval).abs() > 1.0e-6)
    {
        obs.header.interval_s = Some(interval);
        actions.push(RepairAction {
            id: "A6",
            message: format!("set INTERVAL to {interval:.3} seconds"),
        });
    }
}

fn repair_obs_empty_records(obs: &mut RinexObs, actions: &mut Vec<RepairAction>) {
    let mut dropped = 0_usize;
    for epoch in &mut obs.epochs {
        let before = epoch.sats.len();
        epoch
            .sats
            .retain(|_, values| values.iter().any(|value| value.value.is_some()));
        dropped += before - epoch.sats.len();
    }
    if dropped > 0 {
        actions.push(RepairAction {
            id: "A7",
            message: format!("dropped {dropped} empty satellite records"),
        });
    }
}

fn repair_nav_duplicates(records: &mut Vec<BroadcastRecord>, actions: &mut Vec<RepairAction>) {
    let mut seen: BTreeMap<String, BroadcastRecord> = BTreeMap::new();
    let mut out = Vec::with_capacity(records.len());
    let mut dropped = 0_usize;
    for record in records.drain(..) {
        let key = nav_identity(&record);
        match seen.get(&key) {
            Some(existing) if *existing == record => {
                dropped += 1;
            }
            Some(_) => out.push(record),
            None => {
                seen.insert(key, record);
                out.push(record);
            }
        }
    }
    *records = out;
    if dropped > 0 {
        actions.push(RepairAction {
            id: "A11",
            message: format!("dropped {dropped} identical duplicate NAV records"),
        });
    }
}

fn repair_nav_order(records: &mut [BroadcastRecord], actions: &mut Vec<RepairAction>) {
    let before = records.to_vec();
    records.sort_by_key(nav_sort_key);
    if records != before {
        actions.push(RepairAction {
            id: "A12",
            message: "sorted NAV records".to_string(),
        });
    }
}

fn published_obs_version(version: f64) -> Option<()> {
    let scaled = (version * 100.0).round() as i64;
    matches!(scaled, 300 | 301 | 302 | 303 | 304 | 305 | 400 | 401 | 402).then_some(())
}

fn is_valid_obs_code(system: GnssSystem, code: &str, version: f64) -> bool {
    let mut chars = code.chars();
    let Some(kind) = chars.next() else {
        return false;
    };
    let Some(band) = chars.next() else {
        return false;
    };
    let Some(attr) = chars.next() else {
        return false;
    };
    if chars.next().is_some() || !"CLDSX".contains(kind) || !band.is_ascii_digit() {
        return false;
    }
    let attrs = match system {
        GnssSystem::Gps => "CWPYMLXIQS",
        GnssSystem::Glonass => "CPXAB",
        GnssSystem::Galileo => "ABCXZIQ",
        GnssSystem::BeiDou => {
            if version >= 4.0 {
                "DPXAINQ"
            } else {
                "IQXDPAN"
            }
        }
        GnssSystem::Qzss => "CSLXZ",
        GnssSystem::Navic => "ABCX",
        GnssSystem::Sbas => "CIX",
    };
    attrs.contains(attr)
}

fn first_normal_epoch(obs: &RinexObs) -> Option<&ObsEpoch> {
    obs.epochs.iter().find(|epoch| epoch.flag <= 1)
}

fn dominant_interval_s(epochs: &[ObsEpoch]) -> Option<f64> {
    let normal: Vec<_> = epochs
        .iter()
        .filter(|epoch| epoch.flag <= 1)
        .map(|epoch| epoch.epoch)
        .collect();
    if normal.len() < 2 {
        return None;
    }
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    for pair in normal.windows(2) {
        let delta_ms = ((epoch_seconds(pair[1]) - epoch_seconds(pair[0])) * 1000.0).round() as i64;
        if delta_ms > 0 {
            *counts.entry(delta_ms).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(delta_ms, count)| (*count, -(*delta_ms)))
        .map(|(delta_ms, _)| delta_ms as f64 / 1000.0)
}

fn epoch_seconds(epoch: ObsEpochTime) -> f64 {
    let days = civil_days(epoch.year, epoch.month, epoch.day);
    days as f64 * 86_400.0
        + f64::from(epoch.hour) * 3600.0
        + f64::from(epoch.minute) * 60.0
        + epoch.second
}

fn epoch_key(epoch: ObsEpochTime) -> (i32, u8, u8, u8, u8, i64) {
    (
        epoch.year,
        epoch.month,
        epoch.day,
        epoch.hour,
        epoch.minute,
        (epoch.second * 10_000_000.0).round() as i64,
    )
}

fn same_epoch_time(a: ObsEpochTime, b: ObsEpochTime) -> bool {
    epoch_key(a) == epoch_key(b)
}

fn civil_days(year: i32, month: u8, day: u8) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from(month) + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn nav_identity(record: &BroadcastRecord) -> String {
    format!(
        "{}:{:?}:{}:{:016x}:{}",
        record.satellite_id,
        record.message,
        record.toc.week,
        record.toc.tow_s.to_bits(),
        record.issue_of_data.issue
    )
}

fn nav_sort_key(record: &BroadcastRecord) -> (GnssSystem, u8, u32, u64, u8) {
    (
        record.satellite_id.system,
        record.satellite_id.prn,
        record.toc.week,
        record.toc.tow_s.to_bits(),
        nav_message_rank(record.message),
    )
}

const fn nav_message_rank(message: NavMessage) -> u8 {
    match message {
        NavMessage::GpsLnav => 0,
        NavMessage::GpsCnav => 1,
        NavMessage::GpsCnav2 => 2,
        NavMessage::QzssCnav => 3,
        NavMessage::QzssCnav2 => 4,
        NavMessage::GalileoInav => 5,
        NavMessage::GalileoFnav => 6,
        NavMessage::BeidouD1 => 7,
        NavMessage::BeidouD2 => 8,
    }
}

#[cfg(test)]
mod tests;
