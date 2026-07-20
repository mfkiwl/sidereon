use sidereon_core::data::{mgex_nav, mgex_sp3, AnalysisCenter, ProductDate, ProductType};
use sidereon_core::ephemeris::{
    parse_exact_sp3, validate_exact_sp3, ExactSp3Coverage, ExactSp3Request,
    ExactSp3ValidationError, Sp3,
};

const START: ProductDate = ProductDate {
    year: 2020,
    month: 1,
    day: 1,
};
const P_G01: &str = "PG01  15000.000000 -20000.000000   5000.000000    123.456789\n";
const P_G02: &str = "PG02  16000.000000 -21000.000000   6000.000000    124.456789\n";
const V_G01: &str = "VG01      1.000000      2.000000      3.000000      4.000000\n";
const V_G02: &str = "VG02      5.000000      6.000000      7.000000      8.000000\n";

fn request(sample: &str) -> Result<ExactSp3Request, ExactSp3ValidationError> {
    ExactSp3Request::new(START, Some("0000"), "01D", sample)
}

fn regular_offsets(count: usize, cadence_s: i64) -> Vec<i64> {
    (0..count).map(|index| index as i64 * cadence_s).collect()
}

fn remove_first_line_with_prefix(text: &str, prefix: &str) -> String {
    let mut removed = false;
    text.split_inclusive('\n')
        .filter(|line| {
            if !removed && line.starts_with(prefix) {
                removed = true;
                false
            } else {
                true
            }
        })
        .collect()
}

fn missing_position_record(satellite: &str) -> String {
    format!(
        "P{satellite}{:14.6}{:14.6}{:14.6}{:14.6}\n",
        0.0, 0.0, 0.0, 999_999.999_999
    )
}

fn exact_sp3(
    offsets_s: &[i64],
    declared_count: usize,
    header_cadence: &str,
    declared_day: u8,
) -> String {
    let dt = format!(
        "{:4} {:>2} {:>2} {:>2} {:>2} {:11.8}",
        2020, 1, declared_day, 0, 0, 0.0
    );
    let mut text = format!(
        "#dP{dt} {declared_count:>7} {:<5}{:>6}{:>4} {}\n",
        "ORBIT", "IGS20", "FIT", "TST"
    );
    text.push_str(&format!(
        "## {:>4} {:15.8} {header_cadence:>14} {:>5} {:.13}\n",
        2086, 259_200.0, 58_849, 0.0
    ));
    text.push_str("+    2   G01G02");
    for _ in 2..17 {
        text.push_str("  0");
    }
    text.push('\n');
    for _ in 1..5 {
        text.push_str("+        ");
        for _ in 0..17 {
            text.push_str("  0");
        }
        text.push('\n');
    }
    for _ in 0..5 {
        text.push_str("++       ");
        for _ in 0..17 {
            text.push_str("  0");
        }
        text.push('\n');
    }
    text.push_str("%c M  cc GPS ccc cccc cccc cccc cccc ccccc ccccc ccccc ccccc\n");
    text.push_str("%c cc cc ccc ccc cccc cccc cccc cccc ccccc ccccc ccccc ccccc\n");
    text.push_str("%f  1.2500000  1.025000000  0.00000000000  0.000000000000000\n");
    text.push_str("%f  0.0000000  0.000000000  0.00000000000  0.000000000000000\n");
    text.push_str("%i    0    0    0    0      0      0      0      0         0\n");
    text.push_str("%i    0    0    0    0      0      0      0      0         0\n");
    for _ in 0..4 {
        text.push_str("/* EXACT VALIDATION TEST FIXTURE\n");
    }

    for &offset_s in offsets_s {
        let day_offset = offset_s.div_euclid(86_400);
        let second_of_day = offset_s.rem_euclid(86_400);
        let hour = second_of_day / 3_600;
        let minute = (second_of_day % 3_600) / 60;
        let second = second_of_day % 60;
        text.push_str(&format!(
            "*  {:4} {:>2} {:>2} {:>2} {:>2} {:11.8}\n",
            2020,
            1,
            1 + day_offset,
            hour,
            minute,
            second as f64
        ));
        text.push_str(P_G01);
        text.push_str(P_G02);
    }
    text.push_str("EOF\n");
    text
}

#[test]
fn accepts_regular_24_hour_five_minute_half_open_grid() {
    let text = exact_sp3(&regular_offsets(288, 300), 288, "300.00000000", 1);
    let (product, coverage) = parse_exact_sp3(text.as_bytes(), &request("05M").unwrap()).unwrap();

    assert_eq!(coverage, ExactSp3Coverage::HalfOpen);
    assert_eq!(product.epoch_count(), 288);
    assert_eq!(product.declared_epoch_count(), 288);
}

#[test]
fn accepts_regular_24_hour_five_minute_inclusive_grid() {
    let text = exact_sp3(&regular_offsets(289, 300), 289, "300.00000000", 1);
    let (_, coverage) = parse_exact_sp3(text.as_bytes(), &request("05M").unwrap()).unwrap();

    assert_eq!(coverage, ExactSp3Coverage::Inclusive);
}

#[test]
fn rejects_shorter_and_longer_regular_grids() {
    for (count, expected_half_open, expected_inclusive) in [(287, 288, 289), (290, 288, 289)] {
        let text = exact_sp3(&regular_offsets(count, 300), count, "300.00000000", 1);
        let error = parse_exact_sp3(text.as_bytes(), &request("05M").unwrap()).unwrap_err();
        assert_eq!(
            error,
            ExactSp3ValidationError::SpanMismatch {
                parsed: count,
                half_open: expected_half_open,
                inclusive: expected_inclusive,
            }
        );
    }
}

#[test]
fn rejects_irregular_or_nonascending_epoch_grid() {
    let mut irregular = regular_offsets(288, 300);
    irregular[100] += 1;
    let text = exact_sp3(&irregular, 288, "300.00000000", 1);
    let error = parse_exact_sp3(text.as_bytes(), &request("05M").unwrap()).unwrap_err();
    assert_eq!(
        error,
        ExactSp3ValidationError::IrregularEpochGrid {
            epoch_index: 100,
            requested_s: 300.0,
            actual_s: 301.0,
        }
    );

    let mut nonascending = regular_offsets(288, 300);
    nonascending[100] = nonascending[99];
    let text = exact_sp3(&nonascending, 288, "300.00000000", 1);
    assert!(matches!(
        parse_exact_sp3(text.as_bytes(), &request("05M").unwrap()),
        Err(ExactSp3ValidationError::IrregularEpochGrid {
            epoch_index: 100,
            actual_s: 0.0,
            ..
        })
    ));
}

#[test]
fn rejects_zero_nonfinite_out_of_range_and_mismatched_header_cadence() {
    let offsets = regular_offsets(288, 300);
    let cases = [
        (
            "0.00000000",
            ExactSp3ValidationError::NonPositiveHeaderCadence { actual_s: 0.0 },
        ),
        (
            "-300.0000000",
            ExactSp3ValidationError::NonPositiveHeaderCadence { actual_s: -300.0 },
        ),
        ("NaN", ExactSp3ValidationError::NonFiniteHeaderCadence),
        ("inf", ExactSp3ValidationError::NonFiniteHeaderCadence),
        (
            "100000.000000",
            ExactSp3ValidationError::UnsupportedHeaderCadence {
                actual_s: 100_000.0,
            },
        ),
        (
            "900.00000000",
            ExactSp3ValidationError::CadenceMismatch {
                requested_s: 300.0,
                header_s: 900.0,
            },
        ),
    ];

    for (header_cadence, expected) in cases {
        let text = exact_sp3(&offsets, 288, header_cadence, 1);
        assert_eq!(
            parse_exact_sp3(text.as_bytes(), &request("05M").unwrap()).unwrap_err(),
            expected
        );
    }
}

#[test]
fn rejects_zero_unknown_and_unsupported_sample_tokens() {
    for sample in ["00M", "00U", "05X", "1M", "01W", "05m", "99Q"] {
        assert_eq!(
            request(sample).unwrap_err(),
            ExactSp3ValidationError::UnsupportedSampleToken {
                token: sample.to_owned(),
            }
        );
    }
}

#[test]
fn request_from_identity_uses_exact_igs_final_fields_and_rejects_nav() {
    let current = ProductDate::new(2022, 11, 27).unwrap();
    let identity = mgex_sp3(AnalysisCenter::Igs, current, None)
        .unwrap()
        .identity()
        .unwrap();
    let request = ExactSp3Request::from_identity(&identity).unwrap();

    assert_eq!(request.date(), current);
    assert_eq!(request.issue(), Some("0000"));
    assert_eq!(request.span(), "01D");
    assert_eq!(request.sample(), "15M");
    assert_eq!(request.expected_agency(), Some("IGS"));

    let nav_identity = mgex_nav(AnalysisCenter::Igs, current, None)
        .unwrap()
        .identity()
        .unwrap();
    assert_eq!(
        ExactSp3Request::from_identity(&nav_identity).unwrap_err(),
        ExactSp3ValidationError::WrongProductFamily {
            actual: ProductType::Nav,
        }
    );
}

#[test]
fn expected_agency_is_optional_but_terminal_when_requested() {
    let text = exact_sp3(&regular_offsets(288, 300), 288, "300.00000000", 1);
    let required = request("05M").unwrap().with_expected_agency("IGS").unwrap();
    assert_eq!(required.expected_agency(), Some("IGS"));
    assert_eq!(
        parse_exact_sp3(text.as_bytes(), &required).unwrap_err(),
        ExactSp3ValidationError::AgencyMismatch {
            expected: "IGS".to_owned(),
            actual: "TST".to_owned(),
        }
    );

    let matching = text.replacen(" FIT TST\n", " FIT IGS\n", 1);
    assert!(parse_exact_sp3(matching.as_bytes(), &required).is_ok());
    assert!(matches!(
        request("05M").unwrap().with_expected_agency("igs"),
        Err(ExactSp3ValidationError::InvalidExpectedAgency { .. })
    ));
}

#[test]
fn rejects_noncanonical_fixed_duration_tokens() {
    for (sample, canonical) in [("60S", "01M"), ("60M", "01H"), ("24H", "01D")] {
        assert_eq!(
            request(sample).unwrap_err(),
            ExactSp3ValidationError::NonCanonicalSampleToken {
                token: sample.to_owned(),
                canonical: canonical.to_owned(),
            }
        );
    }
    assert!(ExactSp3Request::new(START, None, "07D", "05M").is_ok());
    for sample in ["30S", "05M", "01D"] {
        assert!(request(sample).is_ok());
    }
}

#[test]
fn rejects_missing_mandatory_structure_and_accepts_missing_position_sentinels() {
    let base = exact_sp3(&regular_offsets(288, 300), 288, "300.00000000", 1);
    let no_eof = base.strip_suffix("EOF\n").unwrap();
    assert_eq!(
        parse_exact_sp3(no_eof.as_bytes(), &request("05M").unwrap()).unwrap_err(),
        ExactSp3ValidationError::MissingEof
    );

    let trailing = format!("{base}*  2020  1  2  0  0  0.00000000\n{P_G01}{P_G02}");
    assert_eq!(
        parse_exact_sp3(trailing.as_bytes(), &request("05M").unwrap()).unwrap_err(),
        ExactSp3ValidationError::TrailingContentAfterEof
    );

    let missing_accuracy = remove_first_line_with_prefix(&base, "++");
    assert_eq!(
        parse_exact_sp3(missing_accuracy.as_bytes(), &request("05M").unwrap()).unwrap_err(),
        ExactSp3ValidationError::MandatoryHeaderRecordCount {
            record: "++",
            expected: 5,
            actual: 4,
        }
    );

    let missing_float = remove_first_line_with_prefix(&base, "%f");
    assert_eq!(
        parse_exact_sp3(missing_float.as_bytes(), &request("05M").unwrap()).unwrap_err(),
        ExactSp3ValidationError::MandatoryHeaderRecordCount {
            record: "%f",
            expected: 2,
            actual: 1,
        }
    );

    let missing_comment = remove_first_line_with_prefix(&base, "/*").replacen(
        "EOF\n",
        "/* BODY COMMENT DOES NOT COUNT AS HEADER\nEOF\n",
        1,
    );
    assert_eq!(
        parse_exact_sp3(missing_comment.as_bytes(), &request("05M").unwrap()).unwrap_err(),
        ExactSp3ValidationError::MandatoryHeaderRecordCount {
            record: "/*",
            expected: 4,
            actual: 3,
        }
    );

    let no_satellites = base
        .replacen("+    2", "+    0", 1)
        .replacen("G01G02", "  0  0", 1)
        .lines()
        .filter(|line| !line.starts_with('P'))
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    assert_eq!(
        parse_exact_sp3(no_satellites.as_bytes(), &request("05M").unwrap()).unwrap_err(),
        ExactSp3ValidationError::NoDeclaredSatellites
    );

    let empty_epochs = base
        .replace(P_G01, &missing_position_record("G01"))
        .replace(P_G02, &missing_position_record("G02"));
    assert_eq!(
        parse_exact_sp3(empty_epochs.as_bytes(), &request("05M").unwrap())
            .unwrap()
            .1,
        ExactSp3Coverage::HalfOpen
    );
}

#[test]
fn rejects_raw_satellite_count_and_position_record_sequence_defects() {
    let base = exact_sp3(&regular_offsets(288, 300), 288, "300.00000000", 1);

    let wrong_count = base.replacen("+    2", "+    1", 1);
    assert_eq!(
        parse_exact_sp3(wrong_count.as_bytes(), &request("05M").unwrap()).unwrap_err(),
        ExactSp3ValidationError::DeclaredSatelliteCountMismatch {
            declared: 1,
            tokens: 2,
        }
    );

    let omitted = remove_first_line_with_prefix(&base, "PG02");
    assert_eq!(
        parse_exact_sp3(omitted.as_bytes(), &request("05M").unwrap()).unwrap_err(),
        ExactSp3ValidationError::SatelliteRecordSequenceMismatch {
            record: "P",
            epoch_index: 0,
            expected: vec!["G01".to_owned(), "G02".to_owned()],
            actual: vec!["G01".to_owned()],
        }
    );

    let reordered = base.replacen(&format!("{P_G01}{P_G02}"), &format!("{P_G02}{P_G01}"), 1);
    assert_eq!(
        parse_exact_sp3(reordered.as_bytes(), &request("05M").unwrap()).unwrap_err(),
        ExactSp3ValidationError::SatelliteRecordSequenceMismatch {
            record: "P",
            epoch_index: 0,
            expected: vec!["G01".to_owned(), "G02".to_owned()],
            actual: vec!["G02".to_owned(), "G01".to_owned()],
        }
    );

    let duplicate = base.replacen(P_G02, P_G01, 1);
    assert_eq!(
        parse_exact_sp3(duplicate.as_bytes(), &request("05M").unwrap()).unwrap_err(),
        ExactSp3ValidationError::SatelliteRecordSequenceMismatch {
            record: "P",
            epoch_index: 0,
            expected: vec!["G01".to_owned(), "G02".to_owned()],
            actual: vec!["G01".to_owned(), "G01".to_owned()],
        }
    );
}

#[test]
fn velocity_products_require_matching_velocity_records() {
    let position_only = exact_sp3(&regular_offsets(288, 300), 288, "300.00000000", 1);
    let declared_velocity = position_only.replacen("#dP", "#dV", 1);

    assert_eq!(
        parse_exact_sp3(declared_velocity.as_bytes(), &request("05M").unwrap()).unwrap_err(),
        ExactSp3ValidationError::SatelliteRecordSequenceMismatch {
            record: "V",
            epoch_index: 0,
            expected: vec!["G01".to_owned(), "G02".to_owned()],
            actual: vec![],
        }
    );

    let paired = declared_velocity
        .replace(P_G01, &format!("{P_G01}{V_G01}"))
        .replace(P_G02, &format!("{P_G02}{V_G02}"));
    assert!(parse_exact_sp3(paired.as_bytes(), &request("05M").unwrap()).is_ok());

    let first_paired = format!("{P_G01}{V_G01}{P_G02}{V_G02}");
    let grouped = paired.replacen(&first_paired, &format!("{P_G01}{P_G02}{V_G01}{V_G02}"), 1);
    assert_eq!(
        parse_exact_sp3(grouped.as_bytes(), &request("05M").unwrap()).unwrap_err(),
        ExactSp3ValidationError::BodyRecordInterleavingMismatch {
            epoch_index: 0,
            expected: vec![
                "PG01".to_owned(),
                "VG01".to_owned(),
                "PG02".to_owned(),
                "VG02".to_owned(),
            ],
            actual: vec![
                "PG01".to_owned(),
                "PG02".to_owned(),
                "VG01".to_owned(),
                "VG02".to_owned(),
            ],
        }
    );

    let velocity_before_position =
        paired.replacen(&first_paired, &format!("{V_G01}{P_G01}{P_G02}{V_G02}"), 1);
    assert_eq!(
        parse_exact_sp3(
            velocity_before_position.as_bytes(),
            &request("05M").unwrap()
        )
        .unwrap_err(),
        ExactSp3ValidationError::BodyRecordInterleavingMismatch {
            epoch_index: 0,
            expected: vec![
                "PG01".to_owned(),
                "VG01".to_owned(),
                "PG02".to_owned(),
                "VG02".to_owned(),
            ],
            actual: vec![
                "VG01".to_owned(),
                "PG01".to_owned(),
                "PG02".to_owned(),
                "VG02".to_owned(),
            ],
        }
    );
}

#[test]
fn rejects_declared_count_mismatch_without_tightening_base_parser() {
    let text = exact_sp3(&regular_offsets(288, 300), 287, "300.00000000", 1);
    let product = Sp3::parse(text.as_bytes()).expect("base parser remains permissive");

    assert_eq!(product.epoch_count(), 288);
    assert_eq!(product.declared_epoch_count(), 287);
    assert_eq!(
        validate_exact_sp3(&product, &request("05M").unwrap()).unwrap_err(),
        ExactSp3ValidationError::DeclaredEpochCountMismatch {
            declared: 287,
            parsed: 288,
        }
    );
}

#[test]
fn rejects_declared_or_parsed_start_mismatch() {
    let offsets = regular_offsets(288, 300);
    let wrong_declared = exact_sp3(&offsets, 288, "300.00000000", 2);
    assert!(matches!(
        parse_exact_sp3(wrong_declared.as_bytes(), &request("05M").unwrap()),
        Err(ExactSp3ValidationError::DeclaredStartMismatch { .. })
    ));

    let parsed_late = regular_offsets(288, 300)
        .into_iter()
        .map(|seconds| seconds + 300)
        .collect::<Vec<_>>();
    let text = exact_sp3(&parsed_late, 288, "300.00000000", 1);
    assert!(matches!(
        parse_exact_sp3(text.as_bytes(), &request("05M").unwrap()),
        Err(ExactSp3ValidationError::FirstEpochMismatch { .. })
    ));
}

#[test]
fn rejects_inconsistent_line_two_week_sow_and_mjd_start_metadata() {
    let base = exact_sp3(&regular_offsets(288, 300), 288, "300.00000000", 1);
    let cases = [
        (
            base.replacen("## 2086", "## 2085", 1),
            ExactSp3ValidationError::HeaderStartMetadataMismatch {
                field: "gps_week",
                requested: 2086.0,
                actual: 2085.0,
            },
        ),
        (
            base.replacen("259200.00000000", "259201.00000000", 1),
            ExactSp3ValidationError::HeaderStartMetadataMismatch {
                field: "seconds_of_week",
                requested: 259_200.0,
                actual: 259_201.0,
            },
        ),
        (
            base.replacen("259200.00000000", "            NaN", 1),
            ExactSp3ValidationError::NonFiniteHeaderStartMetadata {
                field: "seconds_of_week",
            },
        ),
        (
            base.replacen("259200.00000000", "    -1.00000000", 1),
            ExactSp3ValidationError::InvalidHeaderStartMetadata {
                field: "seconds_of_week",
                actual: -1.0,
            },
        ),
        (
            base.replacen("259200.00000000", "604800.00000000", 1),
            ExactSp3ValidationError::InvalidHeaderStartMetadata {
                field: "seconds_of_week",
                actual: 604_800.0,
            },
        ),
        (
            base.replacen(" 58849 ", " 58848 ", 1),
            ExactSp3ValidationError::HeaderStartMetadataMismatch {
                field: "mjd",
                requested: 58_849.0,
                actual: 58_848.0,
            },
        ),
        (
            base.replacen("0.0000000000000", "0.5000000000000", 1),
            ExactSp3ValidationError::HeaderStartMetadataMismatch {
                field: "mjd",
                requested: 58_849.0,
                actual: 58_849.5,
            },
        ),
        (
            base.replacen("0.0000000000000", "1.0000000000000", 1),
            ExactSp3ValidationError::InvalidHeaderStartMetadata {
                field: "mjd_fraction",
                actual: 1.0,
            },
        ),
    ];

    for (text, expected) in cases {
        assert_eq!(
            parse_exact_sp3(text.as_bytes(), &request("05M").unwrap()).unwrap_err(),
            expected
        );
    }
}

#[test]
fn line_two_start_metadata_uses_the_declared_file_time_system_coordinate() {
    let text = exact_sp3(&regular_offsets(288, 300), 288, "300.00000000", 1).replacen(
        "%c M  cc GPS",
        "%c M  cc UTC",
        1,
    );

    // SP3-d says every time field uses the file's declared time system. The
    // line-1 civil date, line-2 week/MJD, and epoch records therefore agree
    // directly; inserting a GPS-versus-UTC leap offset here would be wrong.
    let (_, coverage) = parse_exact_sp3(text.as_bytes(), &request("05M").unwrap()).unwrap();
    assert_eq!(coverage, ExactSp3Coverage::HalfOpen);
}

#[test]
fn rejects_span_not_divisible_by_sample() {
    let request = ExactSp3Request::new(START, None, "01H", "07M").unwrap();
    let text = exact_sp3(&regular_offsets(9, 420), 9, "420.00000000", 1);

    assert_eq!(
        parse_exact_sp3(text.as_bytes(), &request).unwrap_err(),
        ExactSp3ValidationError::SpanNotMultipleOfCadence {
            span_s: 3_600,
            cadence_s: 420,
        }
    );
}
