//! RINEX observation-file quality-control rollups.
//!
//! This module works from an already parsed [`RinexObs`] product. It does not
//! parse, repair, or resample files; it reports the completeness and signal
//! indicators a caller needs before choosing solver inputs.

use std::collections::BTreeMap;

use crate::astro::time::j2000_seconds;
use crate::id::{GnssSatelliteId, GnssSystem};
use crate::rinex::observations::{ObsEpochTime, RinexObs};

/// Options controlling RINEX observation QC aggregation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObservationQcOptions {
    /// Override the header `INTERVAL` value when detecting missing epochs.
    pub interval_override_s: Option<f64>,
    /// Minimum `delta / interval` ratio that is treated as a data gap.
    pub gap_factor: f64,
}

impl Default for ObservationQcOptions {
    fn default() -> Self {
        Self {
            interval_override_s: None,
            gap_factor: 1.5,
        }
    }
}

/// Error returned when QC options are invalid.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum ObservationQcError {
    /// The supplied nominal interval was zero, negative, or non-finite.
    #[error("invalid observation QC interval: must be finite and positive")]
    InvalidInterval,
    /// The supplied gap factor was not finite and greater than one.
    #[error("invalid observation QC gap factor: must be finite and greater than one")]
    InvalidGapFactor,
}

/// Source of the interval used for gap detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntervalSource {
    /// Caller override.
    Override,
    /// Header `INTERVAL`.
    Header,
    /// Modal positive epoch delta inferred from the body.
    Inferred,
    /// Not enough positive epoch deltas were available.
    Unresolved,
}

/// Non-fatal QC note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationQcNote {
    /// Adjacent observation epochs were duplicate or out of order.
    NonMonotonicEpoch { epoch_index: usize },
    /// No interval could be resolved.
    IntervalUnresolved,
}

/// Aggregate QC report for one parsed RINEX observation file.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservationQcReport {
    /// Total number of epoch records retained by the parser, including events.
    pub total_epoch_records: usize,
    /// Count of normal observation epochs (`flag == 0`) and power-failure
    /// observation epochs (`flag == 1`).
    pub observation_epochs: usize,
    /// Count of non-observation event records (`flag > 1`).
    pub event_records: usize,
    /// Count of observation epochs marked as power-failure epochs (`flag == 1`).
    pub power_failure_epochs: usize,
    /// Count of malformed records skipped by the RINEX observation parser.
    pub skipped_records: usize,
    /// Interval used for gap detection.
    pub interval_s: Option<f64>,
    /// Where `interval_s` came from.
    pub interval_source: IntervalSource,
    /// Estimated number of missing nominal epochs across all detected gaps.
    pub missing_epochs: usize,
    /// Gaps detected from adjacent observation epochs and the nominal interval.
    pub data_gaps: Vec<ObservationDataGap>,
    /// Per-satellite observation completeness.
    pub satellites: Vec<SatelliteObservationQc>,
    /// Per-satellite, per-code observation completeness and SSI statistics.
    pub satellite_signals: Vec<SatelliteSignalQc>,
    /// Per-system, per-code observation completeness and SSI statistics.
    pub system_signals: Vec<SystemSignalQc>,
    /// Non-fatal QC notes.
    pub notes: Vec<ObservationQcNote>,
}

/// One detected gap between adjacent observation epochs.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservationDataGap {
    /// Epoch immediately before the gap.
    pub start_epoch: ObsEpochTime,
    /// Epoch immediately after the gap.
    pub end_epoch: ObsEpochTime,
    /// Nominal interval used for the estimate.
    pub nominal_interval_s: f64,
    /// Observed delta between the two retained epochs.
    pub observed_delta_s: f64,
    /// Estimated missing nominal epochs between `start_epoch` and `end_epoch`.
    pub missing_epochs: usize,
}

/// Per-satellite observation counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SatelliteObservationQc {
    /// Satellite id.
    pub satellite: GnssSatelliteId,
    /// Epochs where the satellite has at least one non-blank observation value.
    pub epochs_with_observations: usize,
    /// Non-blank observation values across all codes and observation epochs.
    pub value_observations: usize,
}

/// Per-satellite, per-observation-code counts.
#[derive(Debug, Clone, PartialEq)]
pub struct SatelliteSignalQc {
    /// Satellite id.
    pub satellite: GnssSatelliteId,
    /// RINEX observation code, e.g. `C1C`, `L1C`, or `S1C`.
    pub code: String,
    /// Non-blank values for this satellite/code pair.
    pub value_observations: usize,
    /// Signal-strength indicator statistics for non-blank values that carried
    /// an SSI digit.
    pub ssi: Option<SsiHistogram>,
    /// Raw S-code statistics when this code is an `S*` observable.
    pub snr: Option<SnrStats>,
}

/// Per-system, per-observation-code counts.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemSignalQc {
    /// GNSS constellation.
    pub system: GnssSystem,
    /// RINEX observation code, e.g. `C1C`, `L1C`, or `S1C`.
    pub code: String,
    /// Non-blank values for this system/code pair across satellites.
    pub value_observations: usize,
    /// Signal-strength indicator statistics for non-blank values that carried
    /// an SSI digit.
    pub ssi: Option<SsiHistogram>,
    /// Raw S-code statistics when this code is an `S*` observable.
    pub snr: Option<SnrStats>,
}

/// Histogram over RINEX SSI digits. Index 0 is blank/unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SsiHistogram {
    /// Counts indexed by SSI digit.
    pub counts: [u64; 10],
}

/// Summary statistics over raw numeric `S*` signal-strength observations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnrStats {
    /// Number of samples.
    pub n: usize,
    /// Arithmetic mean.
    pub mean: f64,
    /// Minimum sample.
    pub min: f64,
    /// Maximum sample.
    pub max: f64,
    /// Sample standard deviation, absent for one sample.
    pub std: Option<f64>,
}

/// Build a QC report with default options.
pub fn observation_qc(obs: &RinexObs) -> ObservationQcReport {
    observation_qc_with_options(obs, ObservationQcOptions::default())
        .expect("default observation QC options are valid")
}

/// Build a QC report with explicit options.
pub fn observation_qc_with_options(
    obs: &RinexObs,
    options: ObservationQcOptions,
) -> Result<ObservationQcReport, ObservationQcError> {
    validate_options(options)?;

    let mut satellites: BTreeMap<GnssSatelliteId, SatelliteAccum> = BTreeMap::new();
    let mut satellite_signals: BTreeMap<(GnssSatelliteId, String), SignalAccum> = BTreeMap::new();
    let mut system_signals: BTreeMap<(GnssSystem, String), SignalAccum> = BTreeMap::new();
    let mut observation_epoch_times = Vec::new();

    let mut observation_epochs = 0;
    let mut event_records = 0;
    let mut power_failure_epochs = 0;

    for epoch in obs.epochs() {
        if epoch.flag > 1 {
            event_records += 1;
            continue;
        }

        observation_epochs += 1;
        if epoch.flag == 1 {
            power_failure_epochs += 1;
        }
        observation_epoch_times.push(epoch.epoch);

        for (satellite, values) in &epoch.sats {
            let value_observations = values.iter().filter(|value| value.value.is_some()).count();
            if value_observations == 0 {
                continue;
            }

            let satellite_acc = satellites.entry(*satellite).or_default();
            satellite_acc.epochs_with_observations += 1;
            satellite_acc.value_observations += value_observations;

            let Some(codes) = obs.header().obs_codes.get(&satellite.system) else {
                continue;
            };

            for (index, value) in values.iter().enumerate() {
                if value.value.is_none() {
                    continue;
                }

                let Some(code) = codes.get(index) else {
                    continue;
                };

                let sat_signal = satellite_signals
                    .entry((*satellite, code.clone()))
                    .or_default();
                sat_signal.add(code, value.value, value.ssi);

                let sys_signal = system_signals
                    .entry((satellite.system, code.clone()))
                    .or_default();
                sys_signal.add(code, value.value, value.ssi);
            }
        }
    }

    let mut notes = non_monotonic_notes(&observation_epoch_times);
    let (interval_s, interval_source) =
        resolve_interval(obs, options, &observation_epoch_times, &mut notes)?;
    let data_gaps = detect_gaps(options, &observation_epoch_times, interval_s)?;
    let missing_epochs = data_gaps.iter().map(|gap| gap.missing_epochs).sum();

    Ok(ObservationQcReport {
        total_epoch_records: obs.epochs().len(),
        observation_epochs,
        event_records,
        power_failure_epochs,
        skipped_records: obs.skipped_records,
        interval_s,
        interval_source,
        missing_epochs,
        data_gaps,
        satellites: satellites
            .into_iter()
            .map(|(satellite, acc)| SatelliteObservationQc {
                satellite,
                epochs_with_observations: acc.epochs_with_observations,
                value_observations: acc.value_observations,
            })
            .collect(),
        satellite_signals: satellite_signals
            .into_iter()
            .map(|((satellite, code), acc)| SatelliteSignalQc {
                satellite,
                code,
                value_observations: acc.value_observations,
                ssi: acc.ssi.finish(),
                snr: acc.snr.finish(),
            })
            .collect(),
        system_signals: system_signals
            .into_iter()
            .map(|((system, code), acc)| SystemSignalQc {
                system,
                code,
                value_observations: acc.value_observations,
                ssi: acc.ssi.finish(),
                snr: acc.snr.finish(),
            })
            .collect(),
        notes,
    })
}

fn validate_options(options: ObservationQcOptions) -> Result<(), ObservationQcError> {
    if !options.gap_factor.is_finite() || options.gap_factor <= 1.0 {
        return Err(ObservationQcError::InvalidGapFactor);
    }

    if let Some(interval_s) = options.interval_override_s {
        validate_interval(interval_s)?;
    }

    Ok(())
}

fn validate_interval(interval_s: f64) -> Result<(), ObservationQcError> {
    if interval_s.is_finite() && interval_s > 0.0 {
        Ok(())
    } else {
        Err(ObservationQcError::InvalidInterval)
    }
}

fn resolve_interval(
    obs: &RinexObs,
    options: ObservationQcOptions,
    observation_epoch_times: &[ObsEpochTime],
    notes: &mut Vec<ObservationQcNote>,
) -> Result<(Option<f64>, IntervalSource), ObservationQcError> {
    let Some(interval_s) = options.interval_override_s else {
        if let Some(interval_s) = obs.header().interval_s {
            validate_interval(interval_s)?;
            return Ok((Some(interval_s), IntervalSource::Header));
        }
        if let Some(interval_s) = infer_interval_s(observation_epoch_times) {
            return Ok((Some(interval_s), IntervalSource::Inferred));
        }
        notes.push(ObservationQcNote::IntervalUnresolved);
        return Ok((None, IntervalSource::Unresolved));
    };
    validate_interval(interval_s)?;
    Ok((Some(interval_s), IntervalSource::Override))
}

fn detect_gaps(
    options: ObservationQcOptions,
    observation_epoch_times: &[ObsEpochTime],
    interval_s: Option<f64>,
) -> Result<Vec<ObservationDataGap>, ObservationQcError> {
    let Some(interval_s) = interval_s else {
        return Ok(Vec::new());
    };

    let mut gaps = Vec::new();
    for window in observation_epoch_times.windows(2) {
        let start_epoch = window[0];
        let end_epoch = window[1];
        let observed_delta_s = epoch_seconds(end_epoch) - epoch_seconds(start_epoch);
        if observed_delta_s <= 0.0 || observed_delta_s <= interval_s * options.gap_factor {
            continue;
        }

        let missing_epochs = ((observed_delta_s / interval_s).round() as isize - 1) as usize;
        gaps.push(ObservationDataGap {
            start_epoch,
            end_epoch,
            nominal_interval_s: interval_s,
            observed_delta_s,
            missing_epochs,
        });
    }

    Ok(gaps)
}

fn infer_interval_s(observation_epoch_times: &[ObsEpochTime]) -> Option<f64> {
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    for window in observation_epoch_times.windows(2) {
        let delta_ms =
            ((epoch_seconds(window[1]) - epoch_seconds(window[0])) * 1000.0).round() as i64;
        if delta_ms > 0 {
            *counts.entry(delta_ms).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(delta_ms, count)| (*count, -(*delta_ms)))
        .map(|(delta_ms, _)| delta_ms as f64 / 1000.0)
}

fn non_monotonic_notes(observation_epoch_times: &[ObsEpochTime]) -> Vec<ObservationQcNote> {
    let mut notes = Vec::new();
    for (idx, window) in observation_epoch_times.windows(2).enumerate() {
        if epoch_seconds(window[1]) - epoch_seconds(window[0]) <= 0.0 {
            notes.push(ObservationQcNote::NonMonotonicEpoch {
                epoch_index: idx + 1,
            });
        }
    }
    notes
}

fn epoch_seconds(epoch: ObsEpochTime) -> f64 {
    j2000_seconds(
        epoch.year,
        epoch.month as i32,
        epoch.day as i32,
        epoch.hour as i32,
        epoch.minute as i32,
        epoch.second,
    )
}

#[derive(Debug, Default)]
struct SatelliteAccum {
    epochs_with_observations: usize,
    value_observations: usize,
}

#[derive(Debug, Default)]
struct SignalAccum {
    value_observations: usize,
    ssi: SsiAccum,
    snr: SnrAccum,
}

impl SignalAccum {
    fn add(&mut self, code: &str, value: Option<f64>, ssi: Option<u8>) {
        self.value_observations += 1;
        self.ssi.add(ssi);
        if code.starts_with('S') {
            if let Some(value) = value {
                self.snr.add(value);
            }
        }
    }
}

#[derive(Debug, Default)]
struct SsiAccum {
    counts: [u64; 10],
}

impl SsiAccum {
    fn add(&mut self, value: Option<u8>) {
        let idx = value.unwrap_or(0).min(9) as usize;
        self.counts[idx] += 1;
    }

    fn finish(self) -> Option<SsiHistogram> {
        if self.counts.iter().all(|count| *count == 0) {
            return None;
        }

        Some(SsiHistogram {
            counts: self.counts,
        })
    }
}

#[derive(Debug, Default)]
struct SnrAccum {
    samples: Vec<f64>,
}

impl SnrAccum {
    fn add(&mut self, value: f64) {
        self.samples.push(value);
    }

    fn finish(self) -> Option<SnrStats> {
        if self.samples.is_empty() {
            return None;
        }
        let n = self.samples.len();
        let mean = self.samples.iter().sum::<f64>() / n as f64;
        let min = self.samples.iter().copied().fold(f64::INFINITY, f64::min);
        let max = self
            .samples
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let std = (n > 1).then(|| {
            let sum_sq = self
                .samples
                .iter()
                .map(|value| {
                    let residual = *value - mean;
                    residual * residual
                })
                .sum::<f64>();
            (sum_sq / (n - 1) as f64).sqrt()
        });
        Some(SnrStats {
            n,
            mean,
            min,
            max,
            std,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rinex::observations::{ObsEpoch, ObsHeader, ObsValue};
    use std::collections::BTreeMap;

    #[test]
    fn observation_qc_counts_epochs_satellites_signals_and_ssi() {
        let g01 = sat(1);
        let g02 = sat(2);
        let obs = observation_file(vec![
            epoch(
                0,
                0.0,
                0,
                BTreeMap::from([
                    (
                        g01,
                        vec![
                            obs_value(Some(1.0), Some(5)),
                            obs_value(Some(2.0), Some(6)),
                            obs_value(None, None),
                        ],
                    ),
                    (
                        g02,
                        vec![
                            obs_value(Some(10.0), Some(4)),
                            obs_value(None, None),
                            obs_value(None, None),
                        ],
                    ),
                ]),
            ),
            epoch(
                0,
                30.0,
                1,
                BTreeMap::from([(
                    g01,
                    vec![
                        obs_value(Some(3.0), Some(7)),
                        obs_value(None, None),
                        obs_value(Some(9.0), Some(8)),
                    ],
                )]),
            ),
            epoch(1, 0.0, 2, BTreeMap::new()),
        ]);

        let report = observation_qc(&obs);

        assert_eq!(report.total_epoch_records, 3);
        assert_eq!(report.observation_epochs, 2);
        assert_eq!(report.event_records, 1);
        assert_eq!(report.power_failure_epochs, 1);
        assert_eq!(report.skipped_records, 0);
        assert_eq!(report.satellites.len(), 2);
        assert_eq!(
            report.satellites[0],
            SatelliteObservationQc {
                satellite: g01,
                epochs_with_observations: 2,
                value_observations: 4,
            }
        );
        assert_eq!(
            report.satellites[1],
            SatelliteObservationQc {
                satellite: g02,
                epochs_with_observations: 1,
                value_observations: 1,
            }
        );

        let g01_c1c = report
            .satellite_signals
            .iter()
            .find(|signal| signal.satellite == g01 && signal.code == "C1C")
            .expect("G01 C1C signal");
        assert_eq!(g01_c1c.value_observations, 2);
        assert_eq!(
            g01_c1c.ssi,
            Some(SsiHistogram {
                counts: [0, 0, 0, 0, 0, 1, 0, 1, 0, 0],
            })
        );
        assert_eq!(g01_c1c.snr, None);

        let gps_c1c = report
            .system_signals
            .iter()
            .find(|signal| signal.system == GnssSystem::Gps && signal.code == "C1C")
            .expect("GPS C1C signal");
        assert_eq!(gps_c1c.value_observations, 3);
        assert_eq!(
            gps_c1c.ssi,
            Some(SsiHistogram {
                counts: [0, 0, 0, 0, 1, 1, 0, 1, 0, 0],
            })
        );

        let gps_s1c = report
            .system_signals
            .iter()
            .find(|signal| signal.system == GnssSystem::Gps && signal.code == "S1C")
            .expect("GPS S1C signal");
        assert_eq!(
            gps_s1c.snr,
            Some(SnrStats {
                n: 1,
                mean: 9.0,
                min: 9.0,
                max: 9.0,
                std: None,
            })
        );
    }

    #[test]
    fn observation_qc_detects_nominal_interval_gaps() {
        let g01 = sat(1);
        let obs = observation_file(vec![
            epoch(
                0,
                0.0,
                0,
                BTreeMap::from([(g01, vec![obs_value(Some(1.0), Some(5))])]),
            ),
            epoch(
                1,
                30.0,
                0,
                BTreeMap::from([(g01, vec![obs_value(Some(2.0), Some(6))])]),
            ),
        ]);

        let report = observation_qc(&obs);

        assert_eq!(report.missing_epochs, 2);
        assert_eq!(report.data_gaps.len(), 1);
        assert_eq!(report.data_gaps[0].nominal_interval_s, 30.0);
        assert_eq!(report.data_gaps[0].observed_delta_s, 90.0);
        assert_eq!(report.data_gaps[0].missing_epochs, 2);
    }

    #[test]
    fn observation_qc_infers_interval_when_header_is_absent() {
        let g01 = sat(1);
        let mut obs = observation_file(vec![
            epoch(
                0,
                0.0,
                0,
                BTreeMap::from([(g01, vec![obs_value(Some(1.0), Some(5))])]),
            ),
            epoch(
                0,
                30.0,
                0,
                BTreeMap::from([(g01, vec![obs_value(Some(2.0), Some(6))])]),
            ),
            epoch(
                2,
                0.0,
                0,
                BTreeMap::from([(g01, vec![obs_value(Some(3.0), Some(7))])]),
            ),
        ]);
        obs.header.interval_s = None;

        let report = observation_qc(&obs);

        assert_eq!(report.interval_s, Some(30.0));
        assert_eq!(report.interval_source, IntervalSource::Inferred);
        assert_eq!(report.missing_epochs, 2);
    }

    #[test]
    fn observation_qc_notes_non_monotonic_epochs_and_excludes_them_from_gaps() {
        let g01 = sat(1);
        let obs = observation_file(vec![
            epoch(
                1,
                0.0,
                0,
                BTreeMap::from([(g01, vec![obs_value(Some(1.0), Some(5))])]),
            ),
            epoch(
                0,
                30.0,
                0,
                BTreeMap::from([(g01, vec![obs_value(Some(2.0), Some(6))])]),
            ),
        ]);

        let report = observation_qc(&obs);

        assert_eq!(
            report.notes,
            vec![ObservationQcNote::NonMonotonicEpoch { epoch_index: 1 }]
        );
        assert!(report.data_gaps.is_empty());
    }

    #[test]
    fn observation_qc_rejects_invalid_options() {
        let obs = observation_file(Vec::new());

        let err = observation_qc_with_options(
            &obs,
            ObservationQcOptions {
                interval_override_s: Some(0.0),
                gap_factor: 1.5,
            },
        )
        .expect_err("invalid interval");
        assert_eq!(err, ObservationQcError::InvalidInterval);

        let err = observation_qc_with_options(
            &obs,
            ObservationQcOptions {
                interval_override_s: None,
                gap_factor: 1.0,
            },
        )
        .expect_err("invalid gap factor");
        assert_eq!(err, ObservationQcError::InvalidGapFactor);
    }

    fn observation_file(epochs: Vec<ObsEpoch>) -> RinexObs {
        RinexObs {
            header: ObsHeader {
                version: 3.05,
                approx_position_m: None,
                antenna_delta_hen_m: None,
                obs_codes: BTreeMap::from([(
                    GnssSystem::Gps,
                    vec!["C1C".to_string(), "L1C".to_string(), "S1C".to_string()],
                )]),
                program_run_by_date: None,
                comments: Vec::new(),
                marker_number: None,
                marker_type: None,
                observer: None,
                agency: None,
                receiver: None,
                antenna: None,
                interval_s: Some(30.0),
                time_of_first_obs: None,
                time_of_last_obs: None,
                n_satellites: None,
                prn_obs_counts: BTreeMap::new(),
                phase_shifts: Vec::new(),
                scale_factors: Vec::new(),
                glonass_slots: BTreeMap::new(),
                glonass_cod_phs_bis: None,
                signal_strength_unit: None,
                leap_seconds: None,
                marker_name: None,
                unretained_header_labels: Vec::new(),
            },
            epochs,
            skipped_records: 0,
        }
    }

    fn epoch(
        minute: u8,
        second: f64,
        flag: u8,
        sats: BTreeMap<GnssSatelliteId, Vec<ObsValue>>,
    ) -> ObsEpoch {
        ObsEpoch {
            epoch: ObsEpochTime {
                year: 2024,
                month: 1,
                day: 1,
                hour: 0,
                minute,
                second,
            },
            flag,
            rcv_clock_offset_s: None,
            epoch_picoseconds: None,
            declared_record_count: sats.len(),
            special_record_count: if flag > 1 { sats.len() } else { 0 },
            sats,
        }
    }

    fn obs_value(value: Option<f64>, ssi: Option<u8>) -> ObsValue {
        ObsValue {
            value,
            lli: None,
            ssi,
        }
    }

    fn sat(prn: u8) -> GnssSatelliteId {
        GnssSatelliteId::new(GnssSystem::Gps, prn).expect("valid GPS PRN")
    }
}
