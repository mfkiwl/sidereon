#[test]
fn core_domain_modules_are_reachable_through_facade() {
    assert_eq!(
        sidereon::constellation::gnss_sp3_id(sidereon::GnssSystem::Gps, 3),
        "G03"
    );

    assert!(sidereon::ppp_corrections::PppCorrections::default()
        .diagnostics
        .warnings
        .is_empty());

    let double_difference = sidereon::rtk::DoubleDifference {
        satellite_id: "G03".to_string(),
        reference_satellite_id: "G01".to_string(),
        ambiguity_id: "G03-G01".to_string(),
        code_m: 0.0,
        phase_m: 0.0,
    };
    assert_eq!(double_difference.reference_satellite_id, "G01");

    assert_eq!(
        sidereon::staleness::StalenessPolicy::default().max_staleness_s,
        3.0 * sidereon::constants::SECONDS_PER_DAY
    );

    assert_eq!(
        sidereon::tides::TideInputErrorKind::Missing.to_string(),
        "missing"
    );

    assert!(sidereon::ils::lambda_ils_search(&[1.2], &[vec![1.0]], 3.0).is_ok());

    assert_eq!(
        sidereon::terrain::DtedLookupOptions::default().interpolation,
        sidereon::terrain::DtedInterpolation::Bilinear
    );
}
