use sidereon_core::constants::F_L1_HZ;
use sidereon_core::ephemeris::{ObservableEphemerisSource, Sp3};
use sidereon_core::observables::{
    emission_media_batch_at_j2000_s, is_observable_state_gap, predict_with_media,
    EmissionMediaBatchOptions, EmissionMediaStatus, MediaPredictOptions,
    ObservableIonosphereCorrection, ObservableMediaOptions, PredictOptions,
};
use sidereon_core::{
    atmosphere::{Ionex, IonexCoveragePolicy},
    geodetic_to_itrf, GnssSatelliteId, GnssSystem, Wgs84Geodetic,
};

const REAL_SP3: &[u8] = include_bytes!("fixtures/sp3/IGS0OPSFIN_20261330000_03H_15M_ORB.SP3");
const REAL_IONEX: &[u8] = include_bytes!("fixtures/ionex/esa_2024176_first_map_2row.inx");

fn real_products() -> (Sp3, Ionex) {
    (
        Sp3::parse(REAL_SP3).expect("real SP3 fixture parses"),
        Ionex::parse(REAL_IONEX).expect("real IONEX fixture parses"),
    )
}

fn receiver_ecef_m() -> [f64; 3] {
    geodetic_to_itrf(
        Wgs84Geodetic::new(0.0_f64.to_radians(), 0.0_f64.to_radians(), 0.0)
            .expect("valid receiver"),
    )
    .expect("receiver to ECEF")
    .as_array()
}

fn media_options<'a>(ionex: &'a Ionex) -> ObservableMediaOptions<'a> {
    ObservableMediaOptions {
        troposphere: Some(Default::default()),
        ionosphere: Some(ObservableIonosphereCorrection::IonexWithPolicy(
            ionex,
            IonexCoveragePolicy::Hold,
        )),
    }
}

fn scalar_options<'a>(ionex: &'a Ionex) -> MediaPredictOptions<'a> {
    MediaPredictOptions {
        prediction: PredictOptions {
            carrier_hz: F_L1_HZ,
            light_time: false,
            sagnac: false,
        },
        media: media_options(ionex),
    }
}

fn batch_options<'a>(ionex: &'a Ionex) -> EmissionMediaBatchOptions<'a> {
    EmissionMediaBatchOptions {
        carrier_hz: F_L1_HZ,
        media: media_options(ionex),
        min_elevation_rad: None,
    }
}

fn assert_vec3_bits_eq(label: &str, left: [f64; 3], right: [f64; 3]) {
    assert_eq!(left.map(f64::to_bits), right.map(f64::to_bits), "{label}");
}

fn assert_option_bits_eq(label: &str, left: Option<f64>, right: Option<f64>) {
    assert_eq!(left.map(f64::to_bits), right.map(f64::to_bits), "{label}");
}

#[test]
fn emission_media_batch_matches_scalar_public_calls_on_real_products() {
    let (sp3, ionex) = real_products();
    let receiver = receiver_ecef_m();
    let sp3_epochs = sp3.epochs_j2000_seconds();
    let candidate_epochs = [
        sp3_epochs[2],
        0.5 * (sp3_epochs[2] + sp3_epochs[3]),
        sp3_epochs[4],
        0.25 * sp3_epochs[5] + 0.75 * sp3_epochs[6],
        sp3_epochs[sp3_epochs.len() - 3],
    ];

    let mut satellites = Vec::new();
    let mut emission_epochs = Vec::new();
    let sp3_sats = sp3.satellites();
    for (epoch_index, &epoch_j2000_s) in candidate_epochs.iter().enumerate() {
        // Rotate each epoch's satellite order so the property gate exercises
        // input ordering, not only the product's native satellite order.
        let rotation = (epoch_index * 7) % sp3_sats.len();
        for offset in 0..sp3_sats.len() {
            satellites.push(sp3_sats[(rotation + offset) % sp3_sats.len()]);
            emission_epochs.push(epoch_j2000_s);
        }
    }

    let missing_sat = GnssSatelliteId::new(GnssSystem::Sbas, 20).expect("valid missing satellite");
    for &epoch_j2000_s in &candidate_epochs {
        satellites.push(missing_sat);
        emission_epochs.push(epoch_j2000_s);
    }
    satellites.push(sp3_sats[0]);
    emission_epochs.push(candidate_epochs[0]);

    let batch = emission_media_batch_at_j2000_s(
        &sp3,
        &satellites,
        &emission_epochs,
        receiver,
        batch_options(&ionex),
    )
    .expect("emission media batch");

    assert_eq!(batch.len(), satellites.len());
    assert_eq!(batch.positions_ecef_m.len(), satellites.len());
    assert_eq!(batch.clocks_s.len(), satellites.len());
    assert_eq!(batch.ionosphere_slant_delays_m.len(), satellites.len());
    assert_eq!(batch.troposphere_delays_m.len(), satellites.len());
    assert_eq!(batch.statuses.len(), satellites.len());
    assert_eq!(batch.element_errors.len(), satellites.len());

    let scalar_options = scalar_options(&ionex);
    let mut valid_count = 0usize;
    let mut media_error_count = 0usize;
    let mut gap_count = 0usize;

    for index in 0..satellites.len() {
        let sat = satellites[index];
        let epoch_j2000_s = emission_epochs[index];

        match sp3.observable_state_at_j2000_s(sat, epoch_j2000_s) {
            Ok(state) => {
                let scalar = predict_with_media(&sp3, sat, receiver, epoch_j2000_s, scalar_options);
                assert_vec3_bits_eq(
                    "state position",
                    batch.positions_ecef_m[index].expect("batch position"),
                    state.position_ecef_m,
                );
                assert_option_bits_eq("state clock", batch.clocks_s[index], state.clock_s);
                match scalar {
                    Ok(scalar) => {
                        valid_count += 1;
                        assert_eq!(
                            batch.element_status(index),
                            Some(EmissionMediaStatus::Valid)
                        );
                        assert_eq!(batch.element_errors[index], None);
                        assert_vec3_bits_eq(
                            "prediction position",
                            batch.positions_ecef_m[index].expect("batch position"),
                            scalar.prediction.sat_pos_ecef_m,
                        );
                        assert_option_bits_eq(
                            "prediction clock",
                            batch.clocks_s[index],
                            scalar.prediction.sat_clock_s,
                        );
                        assert_eq!(
                            batch.ionosphere_slant_delays_m[index]
                                .expect("batch ionosphere")
                                .to_bits(),
                            scalar.media.ionosphere_m.to_bits(),
                            "IONEX slant delay"
                        );
                        assert_eq!(
                            batch.troposphere_delays_m[index]
                                .expect("batch troposphere")
                                .to_bits(),
                            scalar.media.troposphere_m.to_bits(),
                            "troposphere delay"
                        );
                    }
                    Err(error) => {
                        media_error_count += 1;
                        assert_eq!(
                            batch.element_status(index),
                            Some(EmissionMediaStatus::Error)
                        );
                        assert_eq!(batch.ionosphere_slant_delays_m[index], None);
                        assert_eq!(batch.troposphere_delays_m[index], None);
                        assert_eq!(batch.element_errors[index], Some(error));
                    }
                }
            }
            Err(error) if is_observable_state_gap(&error) => {
                gap_count += 1;
                assert_eq!(batch.element_status(index), Some(EmissionMediaStatus::Gap));
                assert_eq!(batch.positions_ecef_m[index], None);
                assert_eq!(batch.clocks_s[index], None);
                assert_eq!(batch.ionosphere_slant_delays_m[index], None);
                assert_eq!(batch.troposphere_delays_m[index], None);
                assert_eq!(batch.element_errors[index], Some(error));
            }
            Err(error) => {
                assert_eq!(
                    batch.element_status(index),
                    Some(EmissionMediaStatus::Error)
                );
                assert_eq!(batch.positions_ecef_m[index], None);
                assert_eq!(batch.clocks_s[index], None);
                assert_eq!(batch.ionosphere_slant_delays_m[index], None);
                assert_eq!(batch.troposphere_delays_m[index], None);
                assert_eq!(batch.element_errors[index], Some(error));
            }
        }
    }

    assert!(
        valid_count > 0,
        "fixture sweep must include valid media rows"
    );
    assert!(
        media_error_count > 0,
        "fixture sweep must include state-valid media error rows"
    );
    assert_eq!(
        gap_count,
        candidate_epochs.len(),
        "one explicit missing satellite gap per candidate epoch"
    );
}

#[test]
fn emission_media_batch_cutoff_keeps_state_without_delay_placeholders() {
    let (sp3, ionex) = real_products();
    let receiver = receiver_ecef_m();
    let epoch_j2000_s = sp3.epochs_j2000_seconds()[3];
    let sat = sp3
        .satellites()
        .iter()
        .copied()
        .find(|&sat| {
            predict_with_media(&sp3, sat, receiver, epoch_j2000_s, scalar_options(&ionex)).is_ok()
        })
        .expect("at least one scalar-valid satellite");
    let state = sp3
        .observable_state_at_j2000_s(sat, epoch_j2000_s)
        .expect("scalar state");

    let batch = emission_media_batch_at_j2000_s(
        &sp3,
        &[sat],
        &[epoch_j2000_s],
        receiver,
        EmissionMediaBatchOptions {
            min_elevation_rad: Some(core::f64::consts::FRAC_PI_2),
            ..batch_options(&ionex)
        },
    )
    .expect("cutoff batch");

    assert_eq!(
        batch.element_status(0),
        Some(EmissionMediaStatus::BelowElevationCutoff)
    );
    assert_vec3_bits_eq(
        "cutoff preserves state",
        batch.positions_ecef_m[0].expect("state is still present"),
        state.position_ecef_m,
    );
    assert_option_bits_eq("cutoff preserves clock", batch.clocks_s[0], state.clock_s);
    assert_eq!(batch.ionosphere_slant_delays_m[0], None);
    assert_eq!(batch.troposphere_delays_m[0], None);
    assert_eq!(batch.element_errors[0], None);
}
