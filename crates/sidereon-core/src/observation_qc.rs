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
    /// The supplied gap factor was zero, negative, or non-finite.
    #[error("invalid observation QC gap factor: must be finite and positive")]
    InvalidGapFactor,
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
    pub ssi: Option<SsiStats>,
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
    pub ssi: Option<SsiStats>,
}

/// Summary statistics over RINEX signal-strength indicator digits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SsiStats {
    /// Number of SSI digits included.
    pub count: usize,
    /// Minimum SSI digit.
    pub min: u8,
    /// Maximum SSI digit.
    pub max: u8,
    /// Arithmetic mean of SSI digits.
    pub mean: f64,
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
                sat_signal.add(value.ssi);

                let sys_signal = system_signals
                    .entry((satellite.system, code.clone()))
                    .or_default();
                sys_signal.add(value.ssi);
            }
        }
    }

    let data_gaps = detect_gaps(obs, options, &observation_epoch_times)?;
    let missing_epochs = data_gaps.iter().map(|gap| gap.missing_epochs).sum();

    Ok(ObservationQcReport {
        total_epoch_records: obs.epochs().len(),
        observation_epochs,
        event_records,
        power_failure_epochs,
        skipped_records: obs.skipped_records,
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
            })
            .collect(),
        system_signals: system_signals
            .into_iter()
            .map(|((system, code), acc)| SystemSignalQc {
                system,
                code,
                value_observations: acc.value_observations,
                ssi: acc.ssi.finish(),
            })
            .collect(),
    })
}

fn validate_options(options: ObservationQcOptions) -> Result<(), ObservationQcError> {
    if !options.gap_factor.is_finite() || options.gap_factor <= 0.0 {
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

fn detect_gaps(
    obs: &RinexObs,
    options: ObservationQcOptions,
    observation_epoch_times: &[ObsEpochTime],
) -> Result<Vec<ObservationDataGap>, ObservationQcError> {
    let Some(interval_s) = options.interval_override_s.or(obs.header().interval_s) else {
        return Ok(Vec::new());
    };
    validate_interval(interval_s)?;

    let mut gaps = Vec::new();
    for window in observation_epoch_times.windows(2) {
        let start_epoch = window[0];
        let end_epoch = window[1];
        let observed_delta_s = epoch_seconds(end_epoch) - epoch_seconds(start_epoch);
        if observed_delta_s <= interval_s * options.gap_factor {
            continue;
        }

        let missing_epochs = ((observed_delta_s / interval_s).round() as isize - 1).max(1) as usize;
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
}

impl SignalAccum {
    fn add(&mut self, ssi: Option<u8>) {
        self.value_observations += 1;
        if let Some(ssi) = ssi {
            self.ssi.add(ssi);
        }
    }
}

#[derive(Debug, Default)]
struct SsiAccum {
    count: usize,
    min: Option<u8>,
    max: Option<u8>,
    sum: u64,
}

impl SsiAccum {
    fn add(&mut self, value: u8) {
        self.count += 1;
        self.min = Some(self.min.map_or(value, |current| current.min(value)));
        self.max = Some(self.max.map_or(value, |current| current.max(value)));
        self.sum += u64::from(value);
    }

    fn finish(self) -> Option<SsiStats> {
        if self.count == 0 {
            return None;
        }

        Some(SsiStats {
            count: self.count,
            min: self.min.expect("count > 0 sets SSI minimum"),
            max: self.max.expect("count > 0 sets SSI maximum"),
            mean: self.sum as f64 / self.count as f64,
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
            Some(SsiStats {
                count: 2,
                min: 5,
                max: 7,
                mean: 6.0,
            })
        );

        let gps_c1c = report
            .system_signals
            .iter()
            .find(|signal| signal.system == GnssSystem::Gps && signal.code == "C1C")
            .expect("GPS C1C signal");
        assert_eq!(gps_c1c.value_observations, 3);
        assert!((gps_c1c.ssi.expect("SSI").mean - (16.0 / 3.0)).abs() < 1.0e-12);
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
                gap_factor: f64::NAN,
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
                interval_s: Some(30.0),
                time_of_first_obs: None,
                phase_shifts: Vec::new(),
                scale_factors: Vec::new(),
                glonass_slots: BTreeMap::new(),
                marker_name: None,
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
