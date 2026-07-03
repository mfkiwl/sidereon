//! Clock-stability validation from public references only.
//!
//! The 1000-point frequency series and printed reference values are from
//! NIST SP 1065, section 12.4, Table 31. The series is generated from the
//! published prime-modulus linear congruential recurrence with seed sample
//! `n0 = 1234567890`, multiplier `16807`, and modulus `2147483647`.

use sidereon_core::clock_stability::{
    allan_deviation, compute_allan_deviations, hadamard_deviation, modified_adev, overlapping_adev,
    time_deviation, AllanEstimatorSet, AllanInput, AllanOptions, AllanResult, AllanSeries,
    GapPolicy, TauGrid,
};

const NIST_MODULUS: u64 = 2_147_483_647;
const NIST_MULTIPLIER: u64 = 16_807;
const NIST_SEED: u64 = 1_234_567_890;

fn nist_sp1065_frequency_data(len: usize) -> Vec<f64> {
    let mut state = NIST_SEED;
    let mut values = Vec::with_capacity(len);
    for _ in 0..len {
        values.push(state as f64 / NIST_MODULUS as f64);
        state = (NIST_MULTIPLIER * state) % NIST_MODULUS;
    }
    values
}

fn assert_printed_precision(actual: f64, expected: f64, tolerance: f64, context: &str) {
    let diff = (actual - expected).abs();
    assert!(
        diff <= tolerance,
        "{context}: got {actual:.15e}, expected {expected:.15e}, diff {diff:.3e}, tolerance {tolerance:.3e}"
    );
}

fn assert_curve(
    result: &AllanResult,
    expected_tau_s: &[f64],
    expected_n: &[usize],
    expected_deviation: &[(f64, f64)],
    context: &str,
) {
    assert_eq!(
        result.tau_s.len(),
        expected_tau_s.len(),
        "{context} tau len"
    );
    assert_eq!(
        result.deviation.len(),
        expected_tau_s.len(),
        "{context} dev len"
    );
    assert_eq!(result.n, expected_n, "{context} term counts");
    for (index, ((&tau_s, &(expected, tolerance)), &actual)) in result
        .tau_s
        .iter()
        .zip(expected_deviation.iter())
        .zip(result.deviation.iter())
        .enumerate()
    {
        assert_eq!(
            tau_s.to_bits(),
            expected_tau_s[index].to_bits(),
            "{context} tau[{index}]"
        );
        assert_printed_precision(
            actual,
            expected,
            tolerance,
            &format!("{context} deviation[{index}]"),
        );
    }
}

#[test]
fn nist_sp1065_1000_point_reference_table() {
    let frequency = nist_sp1065_frequency_data(1000);
    let m = [1, 10, 100];
    let tau_s = [1.0, 10.0, 100.0];

    let adev =
        allan_deviation(AllanSeries::FractionalFrequency(&frequency), 1.0, &m).expect("plain ADEV");
    assert_curve(
        &adev,
        &tau_s,
        &[999, 99, 9],
        &[
            (2.922_319e-1, 5.0e-8),
            (9.965_736e-2, 5.0e-9),
            (3.897_804e-2, 5.0e-9),
        ],
        "NIST Table 31 Normal Allan Dev",
    );

    let oadev = overlapping_adev(AllanSeries::FractionalFrequency(&frequency), 1.0, &m)
        .expect("overlapping ADEV");
    assert_curve(
        &oadev,
        &tau_s,
        &[999, 981, 801],
        &[
            (2.922_319e-1, 5.0e-8),
            (9.159_953e-2, 5.0e-9),
            (3.241_343e-2, 5.0e-9),
        ],
        "NIST Table 31 Overlapping Allan Dev",
    );

    let mdev = modified_adev(AllanSeries::FractionalFrequency(&frequency), 1.0, &m).expect("MDEV");
    assert_curve(
        &mdev,
        &tau_s,
        &[999, 972, 702],
        &[
            (2.922_319e-1, 5.0e-8),
            (6.172_376e-2, 5.0e-9),
            (2.170_921e-2, 5.0e-9),
        ],
        "NIST Table 31 Mod Allan Dev",
    );

    let tdev = time_deviation(AllanSeries::FractionalFrequency(&frequency), 1.0, &m).expect("TDEV");
    assert_curve(
        &tdev,
        &tau_s,
        &[999, 972, 702],
        &[
            (1.687_202e-1, 5.0e-8),
            (3.563_623e-1, 5.0e-8),
            (1.253_382e0, 5.0e-7),
        ],
        "NIST Table 31 Time Deviation",
    );

    let hdev =
        hadamard_deviation(AllanSeries::FractionalFrequency(&frequency), 1.0, &m).expect("HDEV");
    assert_curve(
        &hdev,
        &tau_s,
        &[998, 971, 701],
        &[
            (2.943_883e-1, 5.0e-8),
            (9.581_083e-2, 5.0e-9),
            (3.237_638e-2, 5.0e-9),
        ],
        "NIST Table 31 Overlap Had Dev",
    );
}

#[test]
fn synthetic_white_fm_overlapping_adev_has_expected_slope_and_level() {
    let frequency: Vec<f64> = nist_sp1065_frequency_data(131_072)
        .into_iter()
        .map(|value| value - 0.5)
        .collect();
    let averaging_factors = [1, 2, 4, 8, 16, 32, 64, 128, 256];
    let result = overlapping_adev(
        AllanSeries::FractionalFrequency(&frequency),
        1.0,
        &averaging_factors,
    )
    .expect("synthetic white-FM OADEV");

    let sigma_y = (1.0_f64 / 12.0).sqrt();
    for (&m, &deviation) in averaging_factors.iter().zip(result.deviation.iter()) {
        let expected = sigma_y / (m as f64).sqrt();
        let rel = (deviation - expected).abs() / expected;
        assert!(
            rel <= 0.025,
            "white-FM level m={m}: got {deviation:.15e}, expected {expected:.15e}, rel {rel:.3e}"
        );
    }

    let slope = log_log_slope(&result.tau_s, &result.deviation);
    assert!(
        (slope + 0.5).abs() <= 0.01,
        "white-FM slope got {slope:.15e}, expected -5.0e-1"
    );
}

#[test]
fn gapped_phase_series_omits_only_cross_gap_terms() {
    let phase = [
        Some(0.0),
        Some(1.0),
        Some(2.0),
        None,
        Some(4.0),
        Some(8.0),
        Some(16.0),
    ];

    let rejected = compute_allan_deviations(&AllanInput {
        series: AllanSeries::PhaseSecondsWithGaps(&phase),
        tau0_s: 1.0,
        options: AllanOptions {
            estimators: AllanEstimatorSet {
                overlapping_adev: true,
                ..AllanEstimatorSet::none()
            },
            tau_grid: TauGrid::Explicit(vec![1]),
            gap_policy: GapPolicy::Reject,
        },
    });
    assert!(rejected.is_err());

    let omitted = compute_allan_deviations(&AllanInput {
        series: AllanSeries::PhaseSecondsWithGaps(&phase),
        tau0_s: 1.0,
        options: AllanOptions {
            estimators: AllanEstimatorSet {
                overlapping_adev: true,
                ..AllanEstimatorSet::none()
            },
            tau_grid: TauGrid::Explicit(vec![1]),
            gap_policy: GapPolicy::OmitTerms,
        },
    })
    .expect("gap omission");

    let oadev = omitted.overlapping_adev.expect("OADEV curve");
    assert_eq!(oadev.n, vec![2]);
    assert_eq!(oadev.deviation[0].to_bits(), 2.0_f64.to_bits());
}

#[test]
fn fixed_phase_series_has_frozen_bits() {
    let phase = [0.0, 0.125, -0.0625, 0.5, 0.25, -0.375, 0.75, 0.875, 0.0];
    let m = [1, 2];

    let adev = allan_deviation(AllanSeries::PhaseSeconds(&phase), 0.5, &m).expect("ADEV");
    assert_bits(
        &adev.deviation,
        &[0x3ff5_d7f6_29f3_a2ab, 0x3fe1_3958_ff5a_7732],
        "ADEV",
    );

    let oadev = overlapping_adev(AllanSeries::PhaseSeconds(&phase), 0.5, &m).expect("OADEV");
    assert_bits(
        &oadev.deviation,
        &[0x3ff5_d7f6_29f3_a2ab, 0x3fec_4a95_5a20_bd69],
        "OADEV",
    );

    let mdev = modified_adev(AllanSeries::PhaseSeconds(&phase), 0.5, &m).expect("MDEV");
    assert_bits(
        &mdev.deviation,
        &[0x3ff5_d7f6_29f3_a2ab, 0x3fe0_01ff_e003_ff60],
        "MDEV",
    );

    let hdev = hadamard_deviation(AllanSeries::PhaseSeconds(&phase), 0.5, &m).expect("HDEV");
    assert_bits(
        &hdev.deviation,
        &[0x3ff5_39ee_66c1_bb49, 0x3feb_b46d_5083_1d4f],
        "HDEV",
    );

    let tdev = time_deviation(AllanSeries::PhaseSeconds(&phase), 0.5, &m).expect("TDEV");
    assert_bits(
        &tdev.deviation,
        &[0x3fd9_390a_81e0_e261, 0x3fd2_7bf6_558a_3449],
        "TDEV",
    );
}

fn assert_bits(actual: &[f64], expected: &[u64], context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context} len");
    for (index, (&actual, &expected)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(actual.to_bits(), expected, "{context}[{index}]");
    }
}

fn log_log_slope(x: &[f64], y: &[f64]) -> f64 {
    assert_eq!(x.len(), y.len());
    let n = x.len() as f64;
    let (sum_x, sum_y) = x
        .iter()
        .zip(y.iter())
        .fold((0.0, 0.0), |(sum_x, sum_y), (&x, &y)| {
            (sum_x + x.ln(), sum_y + y.ln())
        });
    let mean_x = sum_x / n;
    let mean_y = sum_y / n;

    let (num, den) = x
        .iter()
        .zip(y.iter())
        .fold((0.0, 0.0), |(num, den), (&x, &y)| {
            let dx = x.ln() - mean_x;
            let dy = y.ln() - mean_y;
            (num + dx * dy, den + dx * dx)
        });
    num / den
}
