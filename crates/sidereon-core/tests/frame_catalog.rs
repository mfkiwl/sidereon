//! Epoch-aware terrestrial frame catalog validation.
//!
//! Published values are duplicated here from:
//! - ITRF/IGN `Transfo-ITRF2020_TRFs.txt`, table "Transformation parameters
//!   from ITRF2020 to past ITRFs", equation (1), equation (2).
//! - IERS Technical Note 38, "Analysis and results of ITRF2014", Table 2,
//!   transformation parameters from ITRF2014 to ITRF2008.
//! - EUREF Technical Note 1, Altamimi and Collilieux, release March 4 2024,
//!   Table 2 and Appendix B numerical examples.
//!
//! The exact-bit non-reference epoch constants below are from a scalar
//! expansion of the published position-vector Helmert equation using the
//! decimal table values, with the same left-to-right accumulation order stated
//! in each assertion.

use sidereon_core::{
    catalog, propagate_position, transform, transform_from_epoch, FrameCatalogError,
    HelmertParameters, HelmertRates, TerrestrialFrame, TerrestrialPositionM,
    TerrestrialVelocityMPerYear,
};

#[derive(Clone, Copy)]
struct ExpectedEntry {
    from: TerrestrialFrame,
    to: TerrestrialFrame,
    epoch: f64,
    parameters: HelmertParameters,
    rates: HelmertRates,
}

type ParameterMutator = fn(&mut HelmertParameters);
type RateMutator = fn(&mut HelmertRates);

const EXPECTED_ENTRIES: &[ExpectedEntry] = &[
    ExpectedEntry {
        from: TerrestrialFrame::Itrf2020,
        to: TerrestrialFrame::Itrf2014,
        epoch: 2015.0,
        parameters: HelmertParameters {
            translation_mm: [-1.4, -0.9, 1.4],
            scale_ppb: -0.42,
            rotation_mas: [0.0, 0.0, 0.0],
        },
        rates: HelmertRates {
            translation_mm_per_year: [0.0, -0.1, 0.2],
            scale_ppb_per_year: 0.0,
            rotation_mas_per_year: [0.0, 0.0, 0.0],
        },
    },
    ExpectedEntry {
        from: TerrestrialFrame::Itrf2020,
        to: TerrestrialFrame::Itrf2008,
        epoch: 2015.0,
        parameters: HelmertParameters {
            translation_mm: [0.2, 1.0, 3.3],
            scale_ppb: -0.29,
            rotation_mas: [0.0, 0.0, 0.0],
        },
        rates: HelmertRates {
            translation_mm_per_year: [0.0, -0.1, 0.1],
            scale_ppb_per_year: 0.03,
            rotation_mas_per_year: [0.0, 0.0, 0.0],
        },
    },
    ExpectedEntry {
        from: TerrestrialFrame::Itrf2014,
        to: TerrestrialFrame::Itrf2008,
        epoch: 2010.0,
        parameters: HelmertParameters {
            translation_mm: [1.6, 1.9, 2.4],
            scale_ppb: -0.02,
            rotation_mas: [0.0, 0.0, 0.0],
        },
        rates: HelmertRates {
            translation_mm_per_year: [0.0, 0.0, -0.1],
            scale_ppb_per_year: 0.03,
            rotation_mas_per_year: [0.0, 0.0, 0.0],
        },
    },
    ExpectedEntry {
        from: TerrestrialFrame::Itrf2020,
        to: TerrestrialFrame::Etrf2020,
        epoch: 2015.0,
        parameters: HelmertParameters {
            translation_mm: [0.0, 0.0, 0.0],
            scale_ppb: 0.0,
            rotation_mas: [2.236, 13.494, -19.578],
        },
        rates: HelmertRates {
            translation_mm_per_year: [0.0, 0.0, 0.0],
            scale_ppb_per_year: 0.0,
            rotation_mas_per_year: [0.086, 0.519, -0.753],
        },
    },
];

#[test]
fn published_catalog_values_are_pinned_exactly() {
    let entries = catalog();
    assert_eq!(entries.len(), EXPECTED_ENTRIES.len());

    for expected in EXPECTED_ENTRIES {
        let entry = entries
            .iter()
            .find(|entry| entry.from == expected.from && entry.to == expected.to)
            .expect("published entry present");
        assert_bits(
            "reference epoch",
            entry.reference_epoch_year,
            expected.epoch,
        );
        assert_array_bits(
            "translation mm",
            entry.parameters.translation_mm,
            expected.parameters.translation_mm,
        );
        assert_bits(
            "scale ppb",
            entry.parameters.scale_ppb,
            expected.parameters.scale_ppb,
        );
        assert_array_bits(
            "rotation mas",
            entry.parameters.rotation_mas,
            expected.parameters.rotation_mas,
        );
        assert_array_bits(
            "translation rate mm/year",
            entry.rates.translation_mm_per_year,
            expected.rates.translation_mm_per_year,
        );
        assert_bits(
            "scale rate ppb/year",
            entry.rates.scale_ppb_per_year,
            expected.rates.scale_ppb_per_year,
        );
        assert_array_bits(
            "rotation rate mas/year",
            entry.rates.rotation_mas_per_year,
            expected.rates.rotation_mas_per_year,
        );
        assert!(!entry.provenance.is_empty());
    }
}

#[test]
fn non_reference_epoch_matches_hand_expanded_closed_form_exactly() {
    let position = TerrestrialPositionM::new(4_027_893.675_0, 307_045.906_9, 4_919_475.172_1)
        .expect("finite position");
    let velocity =
        TerrestrialVelocityMPerYear::new(-0.01361, 0.01686, 0.01024).expect("finite velocity");

    let transformed = transform(
        position,
        Some(velocity),
        TerrestrialFrame::Itrf2020,
        TerrestrialFrame::Etrf2020,
        2010.0,
    )
    .expect("published transform");
    let output_velocity = transformed.velocity.expect("velocity transformed");

    let expected_position = [
        f64::from_bits(0x414e_bafa_faaf_96ab),
        f64::from_bits(0x4112_bd96_385a_ba5f),
        f64::from_bits(0x4152_c42c_bd90_ac4e),
    ];
    let expected_velocity = [
        f64::from_bits(0xbf1d_0a95_d661_cf80),
        f64::from_bits(0x3f1b_61ff_f5a1_3200),
        f64::from_bits(0x3f2e_8d9b_41c8_8c40),
    ];

    assert_array_bits(
        "position exact closed form",
        transformed.position.as_array(),
        expected_position,
    );
    assert_array_bits(
        "velocity exact closed form",
        output_velocity.as_array(),
        expected_velocity,
    );
}

#[test]
fn euref_appendix_b_example_matches_published_precision() {
    let position = TerrestrialPositionM::new(4_027_893.675_0, 307_045.906_9, 4_919_475.172_1)
        .expect("finite position");
    let velocity =
        TerrestrialVelocityMPerYear::new(-0.01361, 0.01686, 0.01024).expect("finite velocity");

    let transformed = transform(
        position,
        Some(velocity),
        TerrestrialFrame::Itrf2020,
        TerrestrialFrame::Etrf2020,
        2010.0,
    )
    .expect("published transform");
    let output_velocity = transformed.velocity.expect("velocity transformed");

    assert_close_array(
        transformed.position.as_array(),
        [4_027_893.958_5, 307_045.555_0, 4_919_474.961_9],
        1.0e-4,
    );
    assert_close_array(
        output_velocity.as_array(),
        [-0.00011, 0.00011, 0.00024],
        1.0e-5,
    );
}

#[test]
fn forward_then_inverse_round_trip_is_sub_nanometre() {
    let position = TerrestrialPositionM::new(3_875_112.125, -912_445.875, 4_966_320.25)
        .expect("finite position");
    let velocity =
        TerrestrialVelocityMPerYear::new(-0.0125, 0.019, 0.0045).expect("finite velocity");

    let forward = transform(
        position,
        Some(velocity),
        TerrestrialFrame::Itrf2020,
        TerrestrialFrame::Etrf2020,
        2026.5,
    )
    .expect("forward transform");
    let round_trip = transform(
        forward.position,
        forward.velocity,
        TerrestrialFrame::Etrf2020,
        TerrestrialFrame::Itrf2020,
        2026.5,
    )
    .expect("inverse transform");

    assert_close_array(round_trip.position.as_array(), position.as_array(), 1.0e-9);
    assert_close_array(
        round_trip.velocity.expect("velocity round trip").as_array(),
        velocity.as_array(),
        1.0e-15,
    );
}

#[test]
fn transform_composition_matches_direct_published_link_tightly() {
    let position = TerrestrialPositionM::new(1_234_567.25, -4_321_987.5, 4_876_543.75)
        .expect("finite position");
    let velocity = TerrestrialVelocityMPerYear::new(0.011, -0.017, 0.006).expect("finite velocity");
    let epoch = 2012.25;

    let direct = transform(
        position,
        Some(velocity),
        TerrestrialFrame::Itrf2020,
        TerrestrialFrame::Itrf2008,
        epoch,
    )
    .expect("direct transform");
    let via_2014 = transform(
        position,
        Some(velocity),
        TerrestrialFrame::Itrf2020,
        TerrestrialFrame::Itrf2014,
        epoch,
    )
    .and_then(|state| {
        transform(
            state.position,
            state.velocity,
            TerrestrialFrame::Itrf2014,
            TerrestrialFrame::Itrf2008,
            epoch,
        )
    })
    .expect("composed transform");

    assert_ulps_array(
        "composition position",
        direct.position.as_array(),
        via_2014.position.as_array(),
        1,
    );
    assert_close_array(
        direct.velocity.expect("direct velocity").as_array(),
        via_2014.velocity.expect("composed velocity").as_array(),
        6.0e-14,
    );
}

#[test]
fn station_velocity_propagates_position_to_requested_epoch() {
    let position = TerrestrialPositionM::new(1.0, 2.0, 3.0).expect("finite position");
    let velocity = TerrestrialVelocityMPerYear::new(0.01, -0.02, 0.03).expect("finite velocity");

    let propagated =
        propagate_position(position, velocity, 2010.0, 2012.5).expect("propagated position");
    assert_array_bits(
        "propagated position",
        propagated.as_array(),
        [1.025, 1.95, 3.075],
    );

    let transformed_from_epoch = transform_from_epoch(
        position,
        velocity,
        2010.0,
        TerrestrialFrame::Itrf2020,
        TerrestrialFrame::Itrf2014,
        2012.5,
    )
    .expect("propagate and transform");
    let transformed_after_manual_propagation = transform(
        propagated,
        Some(velocity),
        TerrestrialFrame::Itrf2020,
        TerrestrialFrame::Itrf2014,
        2012.5,
    )
    .expect("manual propagation then transform");

    assert_array_bits(
        "transform_from_epoch position",
        transformed_from_epoch.position.as_array(),
        transformed_after_manual_propagation.position.as_array(),
    );
    assert_array_bits(
        "transform_from_epoch velocity",
        transformed_from_epoch
            .velocity
            .expect("transformed velocity")
            .as_array(),
        transformed_after_manual_propagation
            .velocity
            .expect("manual transformed velocity")
            .as_array(),
    );
}

#[test]
fn identity_transform_rejects_non_finite_public_fields() {
    let invalid_position = TerrestrialPositionM {
        x_m: f64::NAN,
        y_m: 0.0,
        z_m: 0.0,
    };
    let position_error = transform(
        invalid_position,
        None,
        TerrestrialFrame::Itrf2020,
        TerrestrialFrame::Itrf2020,
        2010.0,
    )
    .expect_err("invalid position rejected");
    assert_eq!(
        position_error,
        FrameCatalogError::InvalidInput {
            field: "x_m",
            reason: "must be finite"
        }
    );

    let valid_position = TerrestrialPositionM::new(1.0, 2.0, 3.0).expect("finite position");
    let invalid_velocity = TerrestrialVelocityMPerYear {
        vx_m_per_year: 0.0,
        vy_m_per_year: f64::INFINITY,
        vz_m_per_year: 0.0,
    };
    let velocity_error = transform(
        valid_position,
        Some(invalid_velocity),
        TerrestrialFrame::Itrf2014,
        TerrestrialFrame::Itrf2014,
        2010.0,
    )
    .expect_err("invalid velocity rejected");
    assert_eq!(
        velocity_error,
        FrameCatalogError::InvalidInput {
            field: "vy_m_per_year",
            reason: "must be finite"
        }
    );
}

#[test]
fn public_helmert_transform_fields_are_validated() {
    let base = *catalog()
        .iter()
        .find(|entry| {
            entry.from == TerrestrialFrame::Itrf2020 && entry.to == TerrestrialFrame::Itrf2014
        })
        .expect("published entry present");

    let mut invalid_reference_epoch = base;
    invalid_reference_epoch.reference_epoch_year = f64::NAN;
    assert_invalid_field(
        invalid_reference_epoch.parameters_at(2010.0),
        "reference_epoch_year",
    );

    let parameter_cases: &[(ParameterMutator, &'static str)] = &[
        (
            |parameters| parameters.translation_mm[0] = f64::NAN,
            "translation_mm[0]",
        ),
        (
            |parameters| parameters.translation_mm[1] = f64::INFINITY,
            "translation_mm[1]",
        ),
        (
            |parameters| parameters.translation_mm[2] = f64::NEG_INFINITY,
            "translation_mm[2]",
        ),
        (|parameters| parameters.scale_ppb = f64::NAN, "scale_ppb"),
        (
            |parameters| parameters.rotation_mas[0] = f64::INFINITY,
            "rotation_mas[0]",
        ),
        (
            |parameters| parameters.rotation_mas[1] = f64::NEG_INFINITY,
            "rotation_mas[1]",
        ),
        (
            |parameters| parameters.rotation_mas[2] = f64::NAN,
            "rotation_mas[2]",
        ),
    ];
    for (mutate, field) in parameter_cases {
        let mut invalid_parameter = base;
        mutate(&mut invalid_parameter.parameters);
        assert_invalid_field(invalid_parameter.parameters_at(2010.0), field);
    }

    let rate_cases: &[(RateMutator, &'static str)] = &[
        (
            |rates| rates.translation_mm_per_year[0] = f64::NAN,
            "translation_mm_per_year[0]",
        ),
        (
            |rates| rates.translation_mm_per_year[1] = f64::INFINITY,
            "translation_mm_per_year[1]",
        ),
        (
            |rates| rates.translation_mm_per_year[2] = f64::NEG_INFINITY,
            "translation_mm_per_year[2]",
        ),
        (
            |rates| rates.scale_ppb_per_year = f64::NAN,
            "scale_ppb_per_year",
        ),
        (
            |rates| rates.rotation_mas_per_year[0] = f64::INFINITY,
            "rotation_mas_per_year[0]",
        ),
        (
            |rates| rates.rotation_mas_per_year[1] = f64::NEG_INFINITY,
            "rotation_mas_per_year[1]",
        ),
        (
            |rates| rates.rotation_mas_per_year[2] = f64::NAN,
            "rotation_mas_per_year[2]",
        ),
    ];
    for (mutate, field) in rate_cases {
        let mut invalid_rate = base;
        mutate(&mut invalid_rate.rates);
        assert_invalid_field(invalid_rate.parameters_at(2010.0), field);
    }

    let mut overflowing_epoch_delta = base;
    overflowing_epoch_delta.reference_epoch_year = -f64::MAX;
    assert_invalid_field(
        overflowing_epoch_delta.parameters_at(f64::MAX),
        "epoch_delta_years",
    );
}

fn assert_bits(label: &str, got: f64, expected: f64) {
    assert_eq!(got.to_bits(), expected.to_bits(), "{label}");
}

fn assert_array_bits(label: &str, got: [f64; 3], expected: [f64; 3]) {
    for axis in 0..3 {
        assert_eq!(
            got[axis].to_bits(),
            expected[axis].to_bits(),
            "{label} axis {axis}"
        );
    }
}

fn assert_close_array(got: [f64; 3], expected: [f64; 3], tolerance: f64) {
    for axis in 0..3 {
        let error = (got[axis] - expected[axis]).abs();
        assert!(
            error <= tolerance,
            "axis {axis}: got {}, expected {}, error {}, tolerance {}",
            got[axis],
            expected[axis],
            error,
            tolerance
        );
    }
}

fn assert_ulps_array(label: &str, got: [f64; 3], expected: [f64; 3], max_ulps: u128) {
    for axis in 0..3 {
        let error_ulps = ulp_distance(got[axis], expected[axis]);
        assert!(
            error_ulps <= max_ulps,
            "{label} axis {axis}: got {}, expected {}, error {} ulps, max {} ulps",
            got[axis],
            expected[axis],
            error_ulps,
            max_ulps
        );
    }
}

fn assert_invalid_field<T: core::fmt::Debug>(
    result: Result<T, FrameCatalogError>,
    field: &'static str,
) {
    let error = result.expect_err("invalid field rejected");
    assert!(
        matches!(
            error,
            FrameCatalogError::InvalidInput {
                field: actual,
                reason: "must be finite"
            } if actual == field
        ),
        "expected invalid field {field}, got {error:?}"
    );
}

fn ulp_distance(a: f64, b: f64) -> u128 {
    let a = ordered_f64_bits(a);
    let b = ordered_f64_bits(b);
    a.abs_diff(b)
}

fn ordered_f64_bits(value: f64) -> u128 {
    let bits = value.to_bits();
    if bits & (1_u64 << 63) == 0 {
        bits as u128
    } else {
        (!bits) as u128
    }
}
