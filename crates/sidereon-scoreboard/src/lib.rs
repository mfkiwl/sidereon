//! Validation scoreboard harness.
//!
//! The library keeps the scoring pipeline testable without network access. The
//! binary supplies the HTTPS fetcher and file output paths.

#![deny(missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use serde::Serialize;
use sidereon_core::astro::frames::transforms::FrameTransformError;
use sidereon_core::astro::propagator::ForceModelKind;
use sidereon_core::astro::time::civil::civil_from_j2000_seconds;
use sidereon_core::constants::{J2000_JD, SECONDS_PER_DAY};
use sidereon_core::data::{mgex_sp3, AnalysisCenter, DataCatalogError, ProductDate, ProductSpec};
use sidereon_core::ephemeris::{
    fit_precise_ephemeris_sample_orbit, fit_precise_ephemeris_state_sample_orbit, OrbitFitOptions,
    OrbitResidualStats, OrientedPreciseEphemerisStateSample, PreciseEphemerisSample,
    PreciseEphemerisStateSample, Sp3,
};
use sidereon_core::{
    EarthOrientation, EarthOrientationProvider, Error as CoreError, GnssSatelliteId,
    TdbEarthOrientationProvider,
};

const UNIX_TO_J2000_S: i64 = 946_728_000;
const DEFAULT_LOOKBACK_DAYS: u32 = 7;
const RAPID_CENTER: AnalysisCenter = AnalysisCenter::Gfz;

/// Scoreboard result emitted as JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScoreboardReport {
    /// Product date in UTC, formatted as `YYYY-MM-DD`.
    pub date_utc: String,
    /// Crate version used to build the scorer.
    pub sidereon_version: String,
    /// Product identity and SP3 producing agency.
    pub product: ProductReport,
    /// Per-constellation aggregate residual summaries.
    pub per_constellation: BTreeMap<String, ConstellationReport>,
    /// Per-satellite best, worst, and skipped rows.
    pub per_sat: PerSatelliteReport,
    /// Caveats and method notes that affect interpretation.
    pub notes: Vec<String>,
}

/// Product identity in a scoreboard report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProductReport {
    /// Canonical product filename.
    pub name: String,
    /// SP3 header agency string.
    pub agency: String,
    /// SP3 parser skips for unsupported declaration or record entries.
    pub parser_skipped_records: usize,
}

/// Per-constellation scoreboard aggregate.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConstellationReport {
    /// Satellites declared for this constellation in the SP3 header.
    pub sat_count: usize,
    /// Satellites that produced a fit and residual ledger.
    pub fit_count: usize,
    /// Satellites that were skipped or failed fitting.
    pub skipped: usize,
    /// Median three-dimensional RMS residual, meters, over fitted satellites.
    pub median_rms_3d_m: Option<f64>,
    /// Largest three-dimensional RMS residual, meters, over fitted satellites.
    pub worst_rms_3d_m: Option<f64>,
}

/// Per-satellite scoreboard section.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PerSatelliteReport {
    /// Three fitted satellites with the lowest RMS residuals.
    pub top: Vec<SatelliteFitReport>,
    /// Three fitted satellites with the highest RMS residuals.
    pub bottom: Vec<SatelliteFitReport>,
    /// Satellites that were not fitted, with reasons.
    pub skipped: Vec<SatelliteSkipReport>,
}

/// Residual row for one fitted satellite.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SatelliteFitReport {
    /// SP3 satellite token.
    pub satellite: String,
    /// Constellation display name.
    pub constellation: String,
    /// Three-dimensional RMS residual, meters.
    pub rms_3d_m: f64,
    /// Radial RMS residual, meters.
    pub radial_rms_m: f64,
    /// Along-track RMS residual, meters.
    pub along_rms_m: f64,
    /// Cross-track RMS residual, meters.
    pub cross_rms_m: f64,
    /// Number of residual epochs.
    pub n: usize,
    /// Whether the ledger marks this row as short.
    pub low_sample_count: bool,
}

/// Skip row for one satellite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SatelliteSkipReport {
    /// SP3 satellite token.
    pub satellite: String,
    /// Constellation display name.
    pub constellation: String,
    /// Machine-readable skip reason.
    pub reason: String,
}

/// Candidate product URL resolved through the core data catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCandidate {
    /// Catalog product specification.
    pub spec: ProductSpec,
    /// Canonical product filename.
    pub name: String,
    /// HTTPS archive URL.
    pub url: String,
}

/// Product bytes resolved from the latest available candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProduct {
    /// Candidate metadata.
    pub candidate: ProductCandidate,
    /// Decompressed SP3 bytes.
    pub bytes: Vec<u8>,
}

/// Fetch result for one product candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    /// Candidate was present and returned decompressed SP3 bytes.
    Available(Vec<u8>),
    /// Candidate was not posted at the archive.
    NotPosted,
}

/// Minimal fetch interface used by the resolver.
pub trait ProductFetcher {
    /// Fetch one product candidate.
    fn fetch(&self, candidate: &ProductCandidate) -> Result<FetchOutcome, ScoreboardError>;
}

/// HTTPS product fetcher used by the binary.
#[derive(Debug, Clone, Copy, Default)]
pub struct HttpsFetcher;

impl ProductFetcher for HttpsFetcher {
    fn fetch(&self, candidate: &ProductCandidate) -> Result<FetchOutcome, ScoreboardError> {
        fetch_https_product(candidate)
    }
}

/// Scoring options for one SP3 product.
#[derive(Debug, Clone)]
pub struct ScoreOptions {
    /// Orbit fit options passed to the core orbit-determination fitter.
    pub fit_options: OrbitFitOptions,
    /// Whether velocity-bearing SP3 state samples should use the state path.
    pub prefer_state_samples: bool,
}

impl Default for ScoreOptions {
    fn default() -> Self {
        Self {
            fit_options: OrbitFitOptions::default(),
            prefer_state_samples: true,
        }
    }
}

/// Error returned by the scoreboard harness.
#[derive(Debug, thiserror::Error)]
pub enum ScoreboardError {
    /// Product catalog resolution failed.
    #[error("data catalog error: {0}")]
    DataCatalog(#[from] DataCatalogError),
    /// The resolved archive URL was not HTTPS.
    #[error("non-HTTPS product URL: {url}")]
    NonHttpsUrl {
        /// URL rejected by the fetcher.
        url: String,
    },
    /// No candidate product was posted within the lookback window.
    #[error("no rapid SP3 product posted in {attempts} attempted candidates")]
    ProductNotPosted {
        /// Number of candidate products checked.
        attempts: usize,
    },
    /// An HTTP status other than a not-posted status was returned.
    #[error("HTTP status {status} while fetching {url}")]
    HttpStatus {
        /// URL requested.
        url: String,
        /// HTTP status code.
        status: u16,
    },
    /// Network transport failed.
    #[error("network error while fetching {url}: {message}")]
    Network {
        /// URL requested.
        url: String,
        /// Transport error message.
        message: String,
    },
    /// File or stream I/O failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// SP3 parsing failed.
    #[error("SP3 parse error: {0}")]
    Sp3(#[from] CoreError),
    /// Earth-orientation evaluation failed.
    #[error("frame transform error: {0}")]
    Frame(#[from] FrameTransformError),
    /// JSON serialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// CLI arguments were invalid.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// System time was before the Unix epoch.
    #[error("system time is before the Unix epoch")]
    SystemTimeBeforeUnixEpoch,
}

/// Return the default lookback window, in whole UTC days.
#[must_use]
pub const fn default_lookback_days() -> u32 {
    DEFAULT_LOOKBACK_DAYS
}

/// Resolve and fetch the latest available rapid multi-GNSS SP3 product.
pub fn resolve_latest_available_rapid_sp3(
    target_date: ProductDate,
    lookback_days: u32,
    fetcher: &impl ProductFetcher,
) -> Result<ResolvedProduct, ScoreboardError> {
    let mut attempts = 0usize;
    for date in product_date_candidates(target_date, lookback_days)? {
        let candidate = rapid_sp3_candidate(date)?;
        attempts += 1;
        match fetcher.fetch(&candidate)? {
            FetchOutcome::Available(bytes) => {
                return Ok(ResolvedProduct { candidate, bytes });
            }
            FetchOutcome::NotPosted => {}
        }
    }
    Err(ScoreboardError::ProductNotPosted { attempts })
}

/// Build a scoreboard report from SP3 bytes.
pub fn score_sp3_bytes(
    bytes: &[u8],
    product_name: &str,
    product_date: ProductDate,
    options: &ScoreOptions,
) -> Result<ScoreboardReport, ScoreboardError> {
    let product = Sp3::parse(bytes)?;
    let state_samples = product.precise_ephemeris_state_samples();
    let position_samples = product.precise_ephemeris_samples();
    let provider = TdbEarthOrientationProvider::new();
    let use_state_samples = options.prefer_state_samples && !state_samples.is_empty();
    let state_counts = state_sample_counts(&state_samples);
    let position_counts = position_sample_counts(&position_samples);
    let oriented_samples = if use_state_samples {
        orient_state_samples(&state_samples, &provider)?
    } else {
        Vec::new()
    };

    let mut fitted = Vec::new();
    let mut skipped = Vec::new();
    let mut used_position_fallback = false;

    for &satellite in product.satellites() {
        let position_count = position_counts.get(&satellite).copied().unwrap_or(0);
        if position_count == 0 {
            skipped.push(skip_row(satellite, "missing_position_samples"));
            continue;
        }

        if use_state_samples {
            let state_count = state_counts.get(&satellite).copied().unwrap_or(0);
            if state_count != position_count {
                skipped.push(skip_row(
                    satellite,
                    &format!("partial_velocity_samples:{state_count}/{position_count}"),
                ));
                continue;
            }
            match fit_precise_ephemeris_state_sample_orbit(
                &oriented_samples,
                satellite,
                &options.fit_options,
            ) {
                Ok(report) => {
                    if let Some(stats) = report.ledger.per_sat.get(&satellite) {
                        fitted.push(fit_row(satellite, *stats));
                    } else {
                        skipped.push(skip_row(satellite, "missing_ledger"));
                    }
                }
                Err(error) => skipped.push(skip_row(satellite, &format!("fit_error:{error}"))),
            }
            continue;
        }

        let sat_position_samples: Vec<PreciseEphemerisSample> = position_samples
            .iter()
            .copied()
            .filter(|sample| sample.sat == satellite)
            .collect();

        used_position_fallback = true;
        match fit_precise_ephemeris_sample_orbit(
            &sat_position_samples,
            satellite,
            &options.fit_options,
        ) {
            Ok(report) => {
                if let Some(stats) = report.ledger.per_sat.get(&satellite) {
                    fitted.push(fit_row(satellite, *stats));
                } else {
                    skipped.push(skip_row(satellite, "missing_ledger"));
                }
            }
            Err(error) => skipped.push(skip_row(satellite, &format!("fit_error:{error}"))),
        }
    }

    fitted.sort_by(|a, b| {
        a.rms_3d_m
            .total_cmp(&b.rms_3d_m)
            .then_with(|| a.satellite.cmp(&b.satellite))
    });
    skipped.sort_by(|a, b| a.satellite.cmp(&b.satellite));

    let per_constellation = constellation_reports(product.satellites(), &fitted, &skipped);
    let bottom = fitted.iter().rev().take(3).cloned().collect::<Vec<_>>();
    let top = fitted.iter().take(3).cloned().collect::<Vec<_>>();

    let mut notes = vec![
        force_model_note(&options.fit_options.force_model),
        "EOP source: core time-scale and Earth-orientation tables with zero polar motion."
            .to_string(),
        "Large residuals do not affect process exit status; skipped and failed satellites are shown."
            .to_string(),
    ];
    if used_position_fallback {
        notes.push(
            "Position-only SP3 rows used the core position-sample fitter; no velocity was synthesized."
                .to_string(),
        );
    }
    if product.skipped_records > 0 {
        notes.push(format!(
            "SP3 parser skipped {} unsupported declaration or record entries; see product.parser_skipped_records.",
            product.skipped_records
        ));
    }

    Ok(ScoreboardReport {
        date_utc: product_date.to_string(),
        sidereon_version: env!("CARGO_PKG_VERSION").to_string(),
        product: ProductReport {
            name: product_name.to_string(),
            agency: product.header.agency,
            parser_skipped_records: product.skipped_records,
        },
        per_constellation,
        per_sat: PerSatelliteReport {
            top,
            bottom,
            skipped,
        },
        notes,
    })
}

/// Write the latest report file and append one compact JSON line to history.
pub fn write_report_outputs(
    report: &ScoreboardReport,
    output_path: Option<&Path>,
    history_path: Option<&Path>,
) -> Result<(), ScoreboardError> {
    if let Some(path) = output_path {
        let file = File::create(path)?;
        serde_json::to_writer_pretty(file, report)?;
    }
    if let Some(path) = history_path {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        serde_json::to_writer(&mut file, report)?;
        file.write_all(b"\n")?;
    }
    Ok(())
}

/// Format a report as pretty JSON.
pub fn report_json_pretty(report: &ScoreboardReport) -> Result<String, ScoreboardError> {
    serde_json::to_string_pretty(report).map_err(ScoreboardError::from)
}

/// Current UTC product date from the system clock.
pub fn utc_today() -> Result<ProductDate, ScoreboardError> {
    let unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ScoreboardError::SystemTimeBeforeUnixEpoch)?
        .as_secs();
    let j2000 = i64::try_from(unix_seconds)
        .map_err(|_| ScoreboardError::InvalidArgument("system time out of range".to_string()))?
        - UNIX_TO_J2000_S;
    product_date_from_j2000_seconds(j2000)
}

/// Parse `YYYY-MM-DD` into a product date.
pub fn parse_product_date(value: &str) -> Result<ProductDate, ScoreboardError> {
    let mut parts = value.split('-');
    let year = parts
        .next()
        .ok_or_else(|| ScoreboardError::InvalidArgument("date missing year".to_string()))?
        .parse::<i32>()
        .map_err(|_| ScoreboardError::InvalidArgument("date year is invalid".to_string()))?;
    let month = parts
        .next()
        .ok_or_else(|| ScoreboardError::InvalidArgument("date missing month".to_string()))?
        .parse::<u8>()
        .map_err(|_| ScoreboardError::InvalidArgument("date month is invalid".to_string()))?;
    let day = parts
        .next()
        .ok_or_else(|| ScoreboardError::InvalidArgument("date missing day".to_string()))?
        .parse::<u8>()
        .map_err(|_| ScoreboardError::InvalidArgument("date day is invalid".to_string()))?;
    if parts.next().is_some() {
        return Err(ScoreboardError::InvalidArgument(
            "date has extra fields".to_string(),
        ));
    }
    ProductDate::new(year, month, day).map_err(ScoreboardError::from)
}

fn rapid_sp3_candidate(date: ProductDate) -> Result<ProductCandidate, ScoreboardError> {
    let spec = mgex_sp3(RAPID_CENTER, date, None)?;
    let name = spec.canonical_filename()?;
    let url = spec.archive_url()?;
    Ok(ProductCandidate { spec, name, url })
}

fn product_date_candidates(
    target: ProductDate,
    lookback_days: u32,
) -> Result<Vec<ProductDate>, ScoreboardError> {
    let start = sidereon_core::astro::time::civil::j2000_seconds(
        target.year,
        i32::from(target.month),
        i32::from(target.day),
        0,
        0,
        0.0,
    ) as i64;
    let mut out = Vec::with_capacity(usize::try_from(lookback_days).unwrap_or(usize::MAX) + 1);
    for back in 0..=lookback_days {
        out.push(product_date_from_j2000_seconds(
            start - i64::from(back) * 86_400,
        )?);
    }
    Ok(out)
}

fn product_date_from_j2000_seconds(seconds: i64) -> Result<ProductDate, ScoreboardError> {
    let (year, month, day, _, _, _) = civil_from_j2000_seconds(seconds);
    ProductDate::new(
        i32::try_from(year)
            .map_err(|_| ScoreboardError::InvalidArgument("year out of range".to_string()))?,
        u8::try_from(month)
            .map_err(|_| ScoreboardError::InvalidArgument("month out of range".to_string()))?,
        u8::try_from(day)
            .map_err(|_| ScoreboardError::InvalidArgument("day out of range".to_string()))?,
    )
    .map_err(ScoreboardError::from)
}

fn fetch_https_product(candidate: &ProductCandidate) -> Result<FetchOutcome, ScoreboardError> {
    if !candidate.url.starts_with("https://") {
        return Err(ScoreboardError::NonHttpsUrl {
            url: candidate.url.clone(),
        });
    }

    let status_output = Command::new("curl")
        .args([
            "--location",
            "--head",
            "--silent",
            "--output",
            "/dev/null",
            "--write-out",
            "%{http_code}",
            &candidate.url,
        ])
        .output()?;
    if !status_output.status.success() {
        return Err(ScoreboardError::Network {
            url: candidate.url.clone(),
            message: String::from_utf8_lossy(&status_output.stderr).to_string(),
        });
    }
    let status_text = String::from_utf8_lossy(&status_output.stdout);
    let status = status_text
        .trim()
        .parse::<u16>()
        .map_err(|_| ScoreboardError::Network {
            url: candidate.url.clone(),
            message: format!("curl returned invalid HTTP status {status_text:?}"),
        })?;
    if status == 403 || status == 404 {
        return Ok(FetchOutcome::NotPosted);
    }
    if !(200..300).contains(&status) {
        return Err(ScoreboardError::HttpStatus {
            url: candidate.url.clone(),
            status,
        });
    }

    let response = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            &candidate.url,
        ])
        .output()?;
    if !response.status.success() {
        return Err(ScoreboardError::Network {
            url: candidate.url.clone(),
            message: String::from_utf8_lossy(&response.stderr).to_string(),
        });
    }
    let bytes = response.stdout;
    if candidate.url.ends_with(".gz") {
        let mut decoder = GzDecoder::new(bytes.as_slice());
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded)?;
        Ok(FetchOutcome::Available(decoded))
    } else {
        Ok(FetchOutcome::Available(bytes))
    }
}

fn state_sample_counts(
    samples: &[PreciseEphemerisStateSample],
) -> BTreeMap<GnssSatelliteId, usize> {
    let mut counts = BTreeMap::new();
    for sample in samples {
        *counts.entry(sample.sat).or_insert(0) += 1;
    }
    counts
}

fn position_sample_counts(samples: &[PreciseEphemerisSample]) -> BTreeMap<GnssSatelliteId, usize> {
    let mut counts = BTreeMap::new();
    for sample in samples {
        *counts.entry(sample.sat).or_insert(0) += 1;
    }
    counts
}

fn orient_state_samples(
    samples: &[PreciseEphemerisStateSample],
    provider: &impl EarthOrientationProvider,
) -> Result<Vec<OrientedPreciseEphemerisStateSample>, ScoreboardError> {
    samples
        .iter()
        .map(|sample| {
            let seed = EarthOrientation::from_instant(sample.epoch)?;
            let tdb_seconds = (seed.time_scales().jd_tdb - J2000_JD) * SECONDS_PER_DAY;
            let orientation = provider.orientation_at_tdb_seconds(tdb_seconds)?;
            Ok(OrientedPreciseEphemerisStateSample::new(
                *sample,
                orientation,
            ))
        })
        .collect()
}

fn fit_row(satellite: GnssSatelliteId, stats: OrbitResidualStats) -> SatelliteFitReport {
    SatelliteFitReport {
        satellite: satellite.to_string(),
        constellation: satellite.system.as_str().to_string(),
        rms_3d_m: stats.rms_3d_m,
        radial_rms_m: stats.radial_rms_m,
        along_rms_m: stats.along_rms_m,
        cross_rms_m: stats.cross_rms_m,
        n: stats.n,
        low_sample_count: stats.low_sample_count,
    }
}

fn skip_row(satellite: GnssSatelliteId, reason: &str) -> SatelliteSkipReport {
    SatelliteSkipReport {
        satellite: satellite.to_string(),
        constellation: satellite.system.as_str().to_string(),
        reason: reason.to_string(),
    }
}

fn constellation_reports(
    satellites: &[GnssSatelliteId],
    fitted: &[SatelliteFitReport],
    skipped: &[SatelliteSkipReport],
) -> BTreeMap<String, ConstellationReport> {
    let mut systems = BTreeSet::new();
    for sat in satellites {
        systems.insert(sat.system);
    }

    let mut reports = BTreeMap::new();
    for system in systems {
        let name = system.as_str().to_string();
        let sat_count = satellites.iter().filter(|sat| sat.system == system).count();
        let fit_rows: Vec<&SatelliteFitReport> = fitted
            .iter()
            .filter(|row| row.constellation == name)
            .collect();
        let skipped_count = skipped
            .iter()
            .filter(|row| row.constellation == name)
            .count();
        let mut rms_values = fit_rows.iter().map(|row| row.rms_3d_m).collect::<Vec<_>>();
        rms_values.sort_by(f64::total_cmp);
        reports.insert(
            name,
            ConstellationReport {
                sat_count,
                fit_count: fit_rows.len(),
                skipped: skipped_count,
                median_rms_3d_m: median(&rms_values),
                worst_rms_3d_m: rms_values.last().copied(),
            },
        );
    }
    reports
}

fn median(sorted: &[f64]) -> Option<f64> {
    match sorted.len() {
        0 => None,
        len if len % 2 == 1 => Some(sorted[len / 2]),
        len => Some((sorted[len / 2 - 1] + sorted[len / 2]) / 2.0),
    }
}

fn force_model_note(force_model: &ForceModelKind) -> String {
    match force_model {
        ForceModelKind::Composite { .. } => {
            "Force model: core composite model, production default is Earth Phase A without spacecraft SRP parameters.".to_string()
        }
        ForceModelKind::TwoBody { .. } => "Force model: core two-body model.".to_string(),
        ForceModelKind::TwoBodyJ2 { .. } => "Force model: core two-body plus J2 model.".to_string(),
    }
}

/// Run the default network-backed scoreboard pipeline.
pub fn run_default(
    target_date: ProductDate,
    lookback_days: u32,
) -> Result<ScoreboardReport, ScoreboardError> {
    let resolved = resolve_latest_available_rapid_sp3(target_date, lookback_days, &HttpsFetcher)?;
    score_sp3_bytes(
        &resolved.bytes,
        &resolved.candidate.name,
        resolved.candidate.spec.date,
        &ScoreOptions::default(),
    )
}
