use std::process::Command;

use sidereon_core::data::{
    allowed_hosts, archive_url, canonical_filename, catalog, cddis_archive_url, default_sample,
    default_sample_for_date, distribution_location_for_identity, dted_block_dir,
    dted_cache_relpath, dted_tile_filename, gim_date_candidates, latest_ops_ultra_sp3, mgex_clk,
    mgex_ionex, mgex_nav, mgex_sp3, no_open_mirrors, open_mirror_code, ops_ultra_sp3,
    parse_skadi_tile_id, predicted_ionex, product, product_convention, product_solution_class,
    rapid_ionex, skadi_archive_url, skadi_band, skadi_source_entry, skadi_tile_id,
    sp3_content_start_convention, space_weather_archive_url, space_weather_cache_relpath,
    space_weather_filename, space_weather_source_entry, station_obs, station_obs_filename,
    station_obs_protocol, station_obs_url, supported_samples, terrain_tile_index,
    ultra_issue_candidates, ultra_sp3_locations, validate_exact_product_set, AnalysisCenter,
    ArchiveCompression, ArchiveProtocol, DataCatalogError, DistributionSource,
    ExactProductSetError, ProductCampaign, ProductDate, ProductDateTime, ProductFormat,
    ProductPublisher, ProductRequest, ProductType, SolutionClass, Sp3ContentStartConvention,
    SpaceWeatherProduct, UltraIssue,
};

fn date(year: i32, month: u8, day: u8) -> ProductDate {
    ProductDate::new(year, month, day).expect("valid test date")
}

struct UnsupportedCadenceCase<'a> {
    center: AnalysisCenter,
    product_date: ProductDate,
    issue: Option<&'a str>,
    expected_samples: &'a [&'a str],
    rejected_sample: &'a str,
}

#[test]
fn final_sp3_urls_match_binding_catalog_examples() {
    let esa = mgex_sp3(AnalysisCenter::Esa, date(2020, 6, 24), None).expect("ESA SP3 product");
    assert_eq!(
        esa.canonical_filename().expect("filename"),
        "ESA0MGNFIN_20201760000_01D_05M_ORB.SP3"
    );
    assert_eq!(
        esa.archive_url().expect("url"),
        "https://navigation-office.esa.int/products/gnss-products/2111/ESA0MGNFIN_20201760000_01D_05M_ORB.SP3.gz"
    );

    let gfz = mgex_sp3(AnalysisCenter::Gfz, date(2020, 6, 24), None).expect("GFZ SP3 product");
    assert_eq!(
        gfz.canonical_filename().expect("filename"),
        "GFZ0OPSRAP_20201760000_01D_15M_ORB.SP3"
    );
    assert_eq!(
        gfz.archive_url().expect("url"),
        "https://isdc-data.gfz.de/gnss/products/rapid/w2111/GFZ0OPSRAP_20201760000_01D_15M_ORB.SP3.gz"
    );
}

#[test]
fn gfz_rapid_sp3_default_tracks_the_day_138_2021_cadence_transition() {
    let last_15m_date = date(2021, 5, 17);
    let first_5m_date = date(2021, 5, 18);
    assert_eq!(last_15m_date.day_of_year(), 137);
    assert_eq!(first_5m_date.day_of_year(), 138);
    assert_eq!(last_15m_date.gps_week(), Ok(2158));
    assert_eq!(first_5m_date.gps_week(), Ok(2158));

    assert_eq!(
        default_sample(AnalysisCenter::Gfz, ProductType::Sp3),
        Ok("05M")
    );
    assert_eq!(
        default_sample_for_date(AnalysisCenter::Gfz, ProductType::Sp3, last_15m_date),
        Ok("15M")
    );
    assert_eq!(
        default_sample_for_date(AnalysisCenter::Gfz, ProductType::Sp3, first_5m_date),
        Ok("05M")
    );

    let legacy = mgex_sp3(AnalysisCenter::Gfz, last_15m_date, None).expect("last 15M product");
    assert_eq!(
        legacy.canonical_filename().expect("legacy filename"),
        "GFZ0OPSRAP_20211370000_01D_15M_ORB.SP3"
    );
    assert_eq!(
        legacy.archive_url().expect("legacy URL"),
        "https://isdc-data.gfz.de/gnss/products/rapid/w2158/GFZ0OPSRAP_20211370000_01D_15M_ORB.SP3.gz"
    );

    let transition = mgex_sp3(AnalysisCenter::Gfz, first_5m_date, None).expect("first 5M product");
    assert_eq!(
        transition
            .canonical_filename()
            .expect("transition filename"),
        "GFZ0OPSRAP_20211380000_01D_05M_ORB.SP3"
    );
    assert_eq!(
        transition.archive_url().expect("transition URL"),
        "https://isdc-data.gfz.de/gnss/products/rapid/w2158/GFZ0OPSRAP_20211380000_01D_05M_ORB.SP3.gz"
    );
}

#[test]
fn gfz_rapid_sp3_current_default_resolves_the_published_five_minute_url() {
    let current_date = date(2026, 7, 19);
    let current = mgex_sp3(AnalysisCenter::Gfz, current_date, None).expect("current GFZ rapid");

    assert_eq!(
        default_sample_for_date(AnalysisCenter::Gfz, ProductType::Sp3, current_date),
        Ok("05M")
    );
    assert_eq!(
        current.canonical_filename().expect("current filename"),
        "GFZ0OPSRAP_20262000000_01D_05M_ORB.SP3"
    );
    assert_eq!(
        current.archive_url().expect("current URL"),
        "https://isdc-data.gfz.de/gnss/products/rapid/w2428/GFZ0OPSRAP_20262000000_01D_05M_ORB.SP3.gz"
    );

    assert_eq!(
        default_sample_for_date(AnalysisCenter::Gfz, ProductType::Clk, current_date),
        Ok("30S")
    );
}

#[test]
fn igs_final_sp3_derivation_respects_the_week_2238_naming_boundary() {
    for day_of_week in 0..=6 {
        let product_date = ProductDate::from_gps_week_day(2237, day_of_week).expect("legacy date");
        let product = mgex_sp3(AnalysisCenter::Igs, product_date, None).expect("legacy IGS final");
        assert_eq!(
            product.canonical_filename().expect("legacy filename"),
            format!("igs2237{day_of_week}.sp3")
        );
        let identity = product.identity().expect("legacy identity");
        assert_eq!(identity.publisher, ProductPublisher::Igs);
        assert_eq!(identity.solution, SolutionClass::Final);
        assert_eq!(identity.issue.as_deref(), Some("0000"));
        let cddis = product
            .distribution_location(DistributionSource::NasaCddis)
            .expect("legacy CDDIS location");
        assert_eq!(cddis.compression, ArchiveCompression::UnixCompress);
        assert_eq!(
            cddis.original_url.as_deref(),
            Some(format!(
                "https://cddis.nasa.gov/archive/gnss/products/2237/igs2237{day_of_week}.sp3.Z"
            ))
            .as_deref()
        );
        assert_eq!(
            product.distribution_location(DistributionSource::Direct),
            Err(DataCatalogError::UnsupportedDistributionEra {
                source: DistributionSource::Direct,
                center: AnalysisCenter::Igs,
                product_type: ProductType::Sp3,
                date: product_date,
            })
        );
    }

    let current_date = ProductDate::from_gps_week_day(2238, 0).expect("transition date");
    assert_eq!(current_date, date(2022, 11, 27));
    let current = mgex_sp3(AnalysisCenter::Igs, current_date, None).expect("current IGS final");
    assert_eq!(
        current.canonical_filename().expect("current filename"),
        "IGS0OPSFIN_20223310000_01D_15M_ORB.SP3"
    );
    assert_eq!(
        current.archive_url().expect("current BKG URL"),
        "https://igs.bkg.bund.de/root_ftp/IGS/products/2238/\
IGS0OPSFIN_20223310000_01D_15M_ORB.SP3.gz"
    );
    let cddis = current
        .distribution_location(DistributionSource::NasaCddis)
        .expect("current CDDIS location");
    assert_eq!(cddis.compression, ArchiveCompression::Gzip);
    assert_eq!(
        cddis.original_url.as_deref(),
        Some(
            "https://cddis.nasa.gov/archive/gnss/products/2238/\
IGS0OPSFIN_20223310000_01D_15M_ORB.SP3.gz"
        )
    );
}

#[test]
fn igs_final_sp3_starts_at_week_0730_and_cddis_week_directories_are_padded() {
    let before_start = ProductDate::from_gps_week_day(729, 6).expect("pre-IGS date");
    assert_eq!(before_start, date(1994, 1, 1));
    assert_eq!(
        mgex_sp3(AnalysisCenter::Igs, before_start, None),
        Err(DataCatalogError::UnsupportedProductEra {
            center: AnalysisCenter::Igs,
            product_type: ProductType::Sp3,
            date: before_start,
        })
    );

    // The lower bound applies to the combined final orbit family only; the
    // existing IGS broadcast-navigation catalog behavior remains unchanged.
    assert!(mgex_nav(AnalysisCenter::Igs, before_start, None).is_ok());

    let first_date = ProductDate::from_gps_week_day(730, 0).expect("first IGS final date");
    assert_eq!(first_date, date(1994, 1, 2));
    let first = mgex_sp3(AnalysisCenter::Igs, first_date, None).expect("first IGS final");
    assert_eq!(
        first.canonical_filename().expect("first filename"),
        "igs07300.sp3"
    );
    assert_eq!(
        first
            .distribution_location(DistributionSource::NasaCddis)
            .expect("first CDDIS location")
            .original_url
            .as_deref(),
        Some("https://cddis.nasa.gov/archive/gnss/products/0730/igs07300.sp3.Z")
    );

    let week_999_date = ProductDate::from_gps_week_day(999, 0).expect("week 999 date");
    let week_999 = mgex_sp3(AnalysisCenter::Igs, week_999_date, None).expect("week 999 IGS final");
    assert_eq!(
        week_999
            .distribution_location(DistributionSource::NasaCddis)
            .expect("week 999 CDDIS location")
            .original_url
            .as_deref(),
        Some("https://cddis.nasa.gov/archive/gnss/products/0999/igs09990.sp3.Z")
    );
}

#[test]
fn cataloged_sp3_series_enforce_their_first_published_date() {
    let cases = [
        (
            AnalysisCenter::Esa,
            date(2014, 1, 4),
            date(2014, 1, 5),
            None,
            "https://navigation-office.esa.int/products/gnss-products/1774/\
ESA0MGNFIN_20140050000_01D_05M_ORB.SP3.gz",
        ),
        (
            AnalysisCenter::Gfz,
            date(2020, 5, 12),
            date(2020, 5, 13),
            None,
            "https://isdc-data.gfz.de/gnss/products/rapid/w2105/\
GFZ0OPSRAP_20201340000_01D_15M_ORB.SP3.gz",
        ),
        (
            AnalysisCenter::IgsUlt,
            date(2022, 11, 26),
            date(2022, 11, 27),
            Some("0000"),
            "https://igs.bkg.bund.de/root_ftp/IGS/products/2238/\
IGS0OPSULT_20223310000_02D_15M_ORB.SP3.gz",
        ),
        (
            AnalysisCenter::EsaUlt,
            date(2022, 10, 3),
            date(2022, 10, 4),
            Some("0000"),
            "https://navigation-office.esa.int/products/gnss-products/2230/\
ESA0OPSULT_20222770000_02D_15M_ORB.SP3.gz",
        ),
        (
            AnalysisCenter::GfzUlt,
            date(2020, 10, 5),
            date(2020, 10, 6),
            Some("0000"),
            "https://isdc-data.gfz.de/gnss/products/ultra/w2126/\
GFZ0OPSULT_20202800000_02D_15M_ORB.SP3.gz",
        ),
    ];

    for (center, before, first, issue, first_url) in cases {
        assert_eq!(
            sidereon_core::data::product(center, ProductType::Sp3, before, None, issue),
            Err(DataCatalogError::UnsupportedProductEra {
                center,
                product_type: ProductType::Sp3,
                date: before,
            })
        );
        assert_eq!(
            sidereon_core::data::product(center, ProductType::Sp3, first, None, issue)
                .expect("first product")
                .archive_url()
                .expect("first direct URL"),
            first_url
        );
    }

    let last_pretransition =
        ProductDate::from_gps_week_day(2237, 6).expect("last pre-transition date");
    assert_eq!(
        ops_ultra_sp3(
            AnalysisCenter::CodUlt,
            last_pretransition,
            None,
            Some("0000")
        ),
        Err(DataCatalogError::UnsupportedProductEra {
            center: AnalysisCenter::CodUlt,
            product_type: ProductType::Sp3,
            date: last_pretransition,
        })
    );
    let first_long_date = ProductDate::from_gps_week_day(2238, 0).expect("week 2238");
    assert_eq!(
        ops_ultra_sp3(AnalysisCenter::CodUlt, first_long_date, None, Some("0000"))
            .expect("first modeled CODE ultra product")
            .canonical_filename()
            .expect("CODE ultra filename"),
        "COD0OPSULT_20223310000_01D_05M_ORB.SP3"
    );
}

#[test]
fn cataloged_clock_series_share_their_verified_orbit_family_floors() {
    for (center, before, first, first_url) in [
        (
            AnalysisCenter::Esa,
            date(2014, 1, 4),
            date(2014, 1, 5),
            "https://navigation-office.esa.int/products/gnss-products/1774/\
ESA0MGNFIN_20140050000_01D_30S_CLK.CLK.gz",
        ),
        (
            AnalysisCenter::Gfz,
            date(2020, 5, 12),
            date(2020, 5, 13),
            "https://isdc-data.gfz.de/gnss/products/rapid/w2105/\
GFZ0OPSRAP_20201340000_01D_30S_CLK.CLK.gz",
        ),
    ] {
        assert_eq!(
            mgex_clk(center, before, None),
            Err(DataCatalogError::UnsupportedProductEra {
                center,
                product_type: ProductType::Clk,
                date: before,
            })
        );
        assert_eq!(
            mgex_clk(center, first, None)
                .expect("first clock product")
                .archive_url()
                .expect("first direct clock URL"),
            first_url
        );
    }
}

#[test]
fn ultra_issue_candidates_stop_at_each_product_series_floor() {
    for (center, before, first) in [
        (
            AnalysisCenter::IgsUlt,
            date(2022, 11, 26),
            date(2022, 11, 27),
        ),
        (AnalysisCenter::EsaUlt, date(2022, 10, 3), date(2022, 10, 4)),
        (AnalysisCenter::GfzUlt, date(2020, 10, 5), date(2020, 10, 6)),
    ] {
        let target = ProductDateTime::new(first, 1, 0, 0).expect("target on first day");
        let candidates = ultra_issue_candidates(center, target).expect("first-day candidates");
        assert_eq!(
            candidates,
            vec![UltraIssue::new(first, "0000").expect("first issue")]
        );

        let before_target = ProductDateTime::new(before, 23, 59, 0).expect("pre-floor target");
        assert_eq!(
            ultra_issue_candidates(center, before_target),
            Err(DataCatalogError::UnsupportedProductEra {
                center,
                product_type: ProductType::Sp3,
                date: before,
            })
        );
    }
}

#[test]
fn cddis_rejects_pretransition_long_sp3_names_but_keeps_igs_legacy() {
    let last_legacy_date = ProductDate::from_gps_week_day(2237, 0).expect("week 2237");
    let long_name_identities = [
        mgex_sp3(AnalysisCenter::Esa, last_legacy_date, None)
            .expect("valid direct ESA product")
            .identity()
            .expect("ESA identity"),
        mgex_sp3(AnalysisCenter::Gfz, last_legacy_date, None)
            .expect("valid direct GFZ product")
            .identity()
            .expect("GFZ identity"),
        ops_ultra_sp3(AnalysisCenter::EsaUlt, last_legacy_date, None, Some("0000"))
            .expect("valid direct ESA ultra product")
            .identity()
            .expect("ESA ultra identity"),
        ops_ultra_sp3(AnalysisCenter::GfzUlt, last_legacy_date, None, Some("0000"))
            .expect("valid direct GFZ ultra product")
            .identity()
            .expect("GFZ ultra identity"),
    ];
    for identity in long_name_identities {
        let expected = DataCatalogError::UnsupportedDistributionEra {
            source: DistributionSource::NasaCddis,
            center: identity.analysis_center,
            product_type: ProductType::Sp3,
            date: last_legacy_date,
        };
        assert_eq!(cddis_archive_url(&identity), Err(expected.clone()));
        assert_eq!(
            distribution_location_for_identity(&identity, DistributionSource::NasaCddis),
            Err(expected)
        );
    }

    let legacy_igs = mgex_sp3(AnalysisCenter::Igs, last_legacy_date, None)
        .expect("IGS legacy product")
        .identity()
        .expect("IGS legacy identity");
    assert_eq!(
        cddis_archive_url(&legacy_igs).expect("legacy CDDIS URL"),
        "https://cddis.nasa.gov/archive/gnss/products/2237/igs22370.sp3.Z"
    );

    let first_long_date = ProductDate::from_gps_week_day(2238, 0).expect("week 2238");
    let igs_long = mgex_sp3(AnalysisCenter::Igs, first_long_date, None)
        .expect("first IGS long-name date")
        .identity()
        .expect("IGS identity");
    assert_eq!(
        cddis_archive_url(&igs_long).expect("long-name CDDIS URL"),
        "https://cddis.nasa.gov/archive/gnss/products/2238/\
IGS0OPSFIN_20223310000_01D_15M_ORB.SP3.gz"
    );
}

#[test]
fn igs_product_solution_class_is_family_aware_and_preserves_legacy_api() {
    assert_eq!(
        AnalysisCenter::Igs.solution_class(),
        SolutionClass::Broadcast
    );
    assert_eq!(
        product_solution_class(AnalysisCenter::Igs, ProductType::Nav),
        Ok(SolutionClass::Broadcast)
    );
    assert_eq!(
        product_solution_class(AnalysisCenter::Igs, ProductType::Sp3),
        Ok(SolutionClass::Final)
    );
    assert_eq!(
        product_solution_class(AnalysisCenter::Igs, ProductType::Clk),
        Err(DataCatalogError::UnsupportedProduct {
            center: AnalysisCenter::Igs,
            product_type: ProductType::Clk,
        })
    );
}

#[test]
fn pretransition_igs_long_trial_is_not_an_alias_for_the_legacy_final() {
    let product =
        mgex_sp3(AnalysisCenter::Igs, date(2022, 11, 17), None).expect("legacy operational final");
    let mut identity = product.identity().expect("legacy identity");
    identity.official_filename = "IGS0OPSFIN_20223210000_01D_15M_ORB.SP3".to_string();
    assert_eq!(
        identity.validate(),
        Err(DataCatalogError::InconsistentProductIdentity {
            field: "official_filename",
        })
    );
}

#[test]
fn caller_identity_cannot_invent_an_unpublished_daily_issue() {
    let mut identity = mgex_sp3(AnalysisCenter::Igs, date(2026, 4, 30), None)
        .expect("IGS final")
        .identity()
        .expect("identity");
    identity.issue = Some("1200".to_string());
    identity.official_filename = "IGS0OPSFIN_20261201200_01D_15M_ORB.SP3".to_string();
    assert_eq!(
        identity.validate(),
        Err(DataCatalogError::InconsistentProductIdentity { field: "issue" })
    );
}

#[test]
fn igs_final_catalog_rejects_unpublished_span_and_cadence_variants() {
    let product_date = date(2026, 4, 30);
    assert_eq!(
        mgex_sp3(AnalysisCenter::Igs, product_date, Some("05M")),
        Err(DataCatalogError::UnsupportedSample {
            center: AnalysisCenter::Igs,
            product_type: ProductType::Sp3,
            sample: "05M".to_string(),
        })
    );

    let mut identity = mgex_sp3(AnalysisCenter::Igs, product_date, None)
        .expect("IGS final")
        .identity()
        .expect("identity");
    identity.span = "02D".to_string();
    identity.official_filename = "IGS0OPSFIN_20261200000_02D_15M_ORB.SP3".to_string();
    assert_eq!(
        identity.validate(),
        Err(DataCatalogError::InconsistentProductIdentity { field: "span" })
    );
}

#[test]
fn period_token_syntax_is_distinct_from_cataloged_publication_support() {
    let product_date = date(2026, 4, 30);
    for sample in ["60S", "60M", "24H"] {
        assert_eq!(
            mgex_sp3(AnalysisCenter::Esa, product_date, Some(sample)),
            Err(DataCatalogError::InvalidSample(sample.to_string()))
        );
    }
    for sample in ["07D", "12L"] {
        assert_eq!(
            mgex_sp3(AnalysisCenter::Esa, product_date, Some(sample)),
            Err(DataCatalogError::UnsupportedSample {
                center: AnalysisCenter::Esa,
                product_type: ProductType::Sp3,
                sample: sample.to_string(),
            }),
            "a valid period spelling is not evidence ESA publishes that cadence",
        );
    }

    assert!(mgex_clk(AnalysisCenter::Cod, product_date, Some("30S")).is_ok());
    assert!(mgex_sp3(AnalysisCenter::Cod, product_date, Some("05M")).is_ok());
    assert!(mgex_ionex(AnalysisCenter::Cod, product_date, Some("01H")).is_ok());
    assert!(mgex_nav(AnalysisCenter::Igs, product_date, Some("01D")).is_ok());
}

#[test]
fn code_https_routes_are_verified_per_current_product_family() {
    let product_date = date(2026, 4, 30);
    assert_eq!(
        mgex_sp3(AnalysisCenter::Cod, product_date, None)
            .expect("CODE MGEX SP3")
            .archive_url()
            .expect("SP3 URL"),
        "https://www.aiub.unibe.ch/download/CODE_MGEX/CODE/2026/\
COD0MGXFIN_20261200000_01D_05M_ORB.SP3.gz"
    );
    assert_eq!(
        mgex_clk(AnalysisCenter::Cod, product_date, None)
            .expect("CODE MGEX clock")
            .archive_url()
            .expect("clock URL"),
        "https://www.aiub.unibe.ch/download/CODE_MGEX/CODE/2026/\
COD0MGXFIN_20261200000_01D_30S_CLK.CLK.gz"
    );
    assert_eq!(
        mgex_ionex(AnalysisCenter::Cod, product_date, None)
            .expect("CODE final IONEX")
            .archive_url()
            .expect("IONEX URL"),
        "https://www.aiub.unibe.ch/download/CODE/2026/\
COD0OPSFIN_20261200000_01D_01H_GIM.INX.gz"
    );
    assert_eq!(
        rapid_ionex(product_date, None)
            .expect("CODE rapid IONEX")
            .archive_url()
            .expect("rapid URL"),
        "https://www.aiub.unibe.ch/download/CODE/\
COD0OPSRAP_20261200000_01D_01H_GIM.INX.gz"
    );

    for center in [AnalysisCenter::Cod, AnalysisCenter::CodRap] {
        let entry = catalog()
            .iter()
            .find(|entry| entry.center == center)
            .expect("CODE catalog entry");
        assert_eq!(entry.protocol, ArchiveProtocol::Https);
        assert_eq!(entry.host, "www.aiub.unibe.ch");
        assert_eq!(entry.root_url, "https://www.aiub.unibe.ch/download");
    }
}

#[test]
fn code_pretransition_dates_are_not_misnamed_with_current_long_names() {
    let legacy_date = date(2022, 11, 26);
    for (product_type, result) in [
        (
            ProductType::Sp3,
            mgex_sp3(AnalysisCenter::Cod, legacy_date, None),
        ),
        (
            ProductType::Clk,
            mgex_clk(AnalysisCenter::Cod, legacy_date, None),
        ),
        (
            ProductType::Ionex,
            mgex_ionex(AnalysisCenter::Cod, legacy_date, None),
        ),
    ] {
        assert_eq!(
            result,
            Err(DataCatalogError::UnsupportedProductEra {
                center: AnalysisCenter::Cod,
                product_type,
                date: legacy_date,
            })
        );
    }
}

#[test]
fn ionex_urls_match_binding_catalog_examples() {
    let esa = mgex_ionex(AnalysisCenter::Esa, date(2024, 6, 24), None).expect("ESA IONEX product");
    assert_eq!(
        esa.canonical_filename().expect("filename"),
        "ESA0OPSFIN_20241760000_01D_02H_GIM.INX"
    );
    assert_eq!(
        esa.archive_url().expect("url"),
        "https://navigation-office.esa.int/products/gnss-products/2320/ESA0OPSFIN_20241760000_01D_02H_GIM.INX.gz"
    );

    let rapid = rapid_ionex(date(2026, 6, 13), None).expect("rapid IONEX product");
    assert_eq!(
        rapid.canonical_filename().expect("filename"),
        "COD0OPSRAP_20261640000_01D_01H_GIM.INX"
    );
    assert_eq!(
        rapid.archive_url().expect("url"),
        "https://www.aiub.unibe.ch/download/CODE/COD0OPSRAP_20261640000_01D_01H_GIM.INX.gz"
    );
}

#[test]
fn product_identity_is_independent_of_distributor() {
    let product =
        mgex_sp3(AnalysisCenter::Esa, date(2024, 6, 24), None).expect("ESA final SP3 product");
    let identity = product.identity().expect("identity");

    assert_eq!(identity.family, ProductType::Sp3);
    assert_eq!(identity.publisher, ProductPublisher::Esa);
    assert_eq!(identity.solution, SolutionClass::Final);
    assert_eq!(identity.campaign, ProductCampaign::MultiGnss);
    assert_eq!(identity.format, ProductFormat::Sp3);
    assert_eq!(identity.version, 0);
    assert_eq!(identity.span, "01D");
    assert_eq!(identity.sample, "05M");
    assert_eq!(
        identity.official_filename,
        "ESA0MGNFIN_20241760000_01D_05M_ORB.SP3"
    );

    let direct = product
        .distribution_location(DistributionSource::Direct)
        .expect("direct location");
    let in_memory = product
        .distribution_location(DistributionSource::InMemory)
        .expect("in-memory location");
    assert_eq!(product.identity().expect("identity"), identity);
    assert_eq!(direct.source, DistributionSource::Direct);
    assert_eq!(in_memory.source, DistributionSource::InMemory);
    assert_eq!(direct.compression, ArchiveCompression::Gzip);
    assert_eq!(in_memory.compression, ArchiveCompression::None);
    assert_eq!(in_memory.original_url, None);
}

#[test]
fn cddis_does_not_substitute_for_esa_mgex_final_sp3() {
    let product =
        mgex_sp3(AnalysisCenter::Esa, date(2024, 6, 24), None).expect("ESA final SP3 product");
    let identity = product.identity().expect("identity");
    let expected = DataCatalogError::UnsupportedDistributionEra {
        source: DistributionSource::NasaCddis,
        center: AnalysisCenter::Esa,
        product_type: ProductType::Sp3,
        date: date(2024, 6, 24),
    };
    assert_eq!(cddis_archive_url(&identity), Err(expected.clone()));
    assert_eq!(
        product.distribution_location(DistributionSource::NasaCddis),
        Err(expected)
    );
}

#[test]
fn cddis_rejects_unmodeled_pretransition_long_ionex_names() {
    let pretransition = ProductDate::from_gps_week_day(2237, 6).expect("week 2237");
    let identity = mgex_ionex(AnalysisCenter::Esa, pretransition, None)
        .expect("ESA direct IONEX")
        .identity()
        .expect("ESA IONEX identity");
    let expected = DataCatalogError::UnsupportedDistributionEra {
        source: DistributionSource::NasaCddis,
        center: AnalysisCenter::Esa,
        product_type: ProductType::Ionex,
        date: pretransition,
    };
    assert_eq!(cddis_archive_url(&identity), Err(expected.clone()));
    assert_eq!(
        distribution_location_for_identity(&identity, DistributionSource::NasaCddis),
        Err(expected)
    );
}

#[test]
fn cddis_ionex_path_uses_current_year_and_day_of_year_layout() {
    let product = rapid_ionex(date(2026, 6, 13), None).expect("CODE rapid IONEX");
    let identity = product.identity().expect("identity");
    assert_eq!(identity.publisher, ProductPublisher::Code);
    assert_eq!(identity.solution, SolutionClass::Rapid);
    assert_eq!(identity.campaign, ProductCampaign::Operational);
    assert_eq!(identity.format, ProductFormat::Ionex);
    assert_eq!(
        cddis_archive_url(&identity).expect("CDDIS URL"),
        "https://cddis.nasa.gov/archive/gnss/products/ionex/2026/164/\
COD0OPSRAP_20261640000_01D_01H_GIM.INX.gz"
    );
}

#[test]
fn exact_request_and_cache_key_keep_source_selection_explicit() {
    let product = ops_ultra_sp3(AnalysisCenter::IgsUlt, date(2024, 9, 3), None, Some("0600"))
        .expect("IGS ultra product");
    let identity = product.identity().expect("identity");
    let request = ProductRequest::new(
        identity.clone(),
        vec![DistributionSource::NasaCddis, DistributionSource::Direct],
    )
    .expect("request");
    assert_eq!(request.identity, identity);
    assert_eq!(
        request.distributors,
        vec![DistributionSource::NasaCddis, DistributionSource::Direct]
    );
    assert_ne!(
        request
            .identity
            .cache_relpath(DistributionSource::NasaCddis)
            .expect("CDDIS cache path"),
        request
            .identity
            .cache_relpath(DistributionSource::Direct)
            .expect("direct cache path")
    );
    assert_eq!(
        ProductRequest::new(identity, vec![]),
        Err(DataCatalogError::NoDistributionSources)
    );
}

#[test]
fn exact_identity_key_is_stable_across_interfaces() {
    let identity = mgex_sp3(AnalysisCenter::Cod, date(2026, 7, 12), None)
        .expect("CODE product")
        .identity()
        .expect("identity");
    assert_eq!(identity.analysis_center, AnalysisCenter::Cod);
    assert_eq!(identity.format_version, None);
    assert_eq!(
        identity.key().expect("cache key"),
        "cod-final-a91258c21fa4860c34ce"
    );
}

#[test]
fn exact_product_set_requires_every_declared_identity_before_processing() {
    let first = mgex_sp3(AnalysisCenter::Cod, date(2026, 7, 12), None)
        .expect("first product")
        .identity()
        .expect("first identity");
    let second = mgex_sp3(AnalysisCenter::Cod, date(2026, 7, 13), None)
        .expect("second product")
        .identity()
        .expect("second identity");

    assert_eq!(
        validate_exact_product_set(
            &[first.clone(), second.clone()],
            &[second.clone(), first.clone()]
        ),
        Ok(())
    );
    assert_eq!(
        validate_exact_product_set(&[], &[]),
        Err(ExactProductSetError::EmptyExpected)
    );
    assert_eq!(
        validate_exact_product_set(
            &[first.clone(), second.clone()],
            std::slice::from_ref(&first),
        ),
        Err(ExactProductSetError::Mismatch {
            missing: vec![second],
            unexpected: vec![],
            duplicate_expected: vec![],
            duplicate_available: vec![],
        })
    );
}

#[test]
fn exact_product_set_rejects_duplicates_and_undeclared_products() {
    let first = mgex_sp3(AnalysisCenter::Cod, date(2026, 7, 12), None)
        .expect("first product")
        .identity()
        .expect("first identity");
    let second = mgex_sp3(AnalysisCenter::Cod, date(2026, 7, 13), None)
        .expect("second product")
        .identity()
        .expect("second identity");

    assert_eq!(
        validate_exact_product_set(
            &[first.clone(), first.clone()],
            &[first.clone(), second.clone(), second.clone()]
        ),
        Err(ExactProductSetError::Mismatch {
            missing: vec![],
            unexpected: vec![second.clone()],
            duplicate_expected: vec![first],
            duplicate_available: vec![second],
        })
    );
}

#[test]
fn exact_product_set_compares_prediction_metadata_not_only_filenames() {
    let predicted_one_day = predicted_ionex(AnalysisCenter::CodPrd1, date(2026, 7, 15), None)
        .expect("one-day prediction")
        .identity()
        .expect("one-day identity");
    let predicted_two_day = predicted_ionex(AnalysisCenter::CodPrd2, date(2026, 7, 14), None)
        .expect("two-day prediction")
        .identity()
        .expect("two-day identity");
    assert_eq!(
        predicted_one_day.official_filename,
        predicted_two_day.official_filename
    );

    assert_eq!(
        validate_exact_product_set(
            std::slice::from_ref(&predicted_one_day),
            std::slice::from_ref(&predicted_two_day),
        ),
        Err(ExactProductSetError::Mismatch {
            missing: vec![predicted_one_day],
            unexpected: vec![predicted_two_day],
            duplicate_expected: vec![],
            duplicate_available: vec![],
        })
    );
}

#[test]
fn caller_constructed_identity_must_agree_with_its_official_filename() {
    let product = mgex_sp3(AnalysisCenter::Cod, date(2026, 7, 12), None).expect("CODE SP3");
    let identity = product.identity().expect("identity");
    assert_eq!(identity.campaign, ProductCampaign::MultiGnssExperiment);
    assert_eq!(identity.validate(), Ok(()));

    let mut inconsistent = identity.clone();
    inconsistent.publisher = ProductPublisher::Esa;
    assert_eq!(
        inconsistent.validate(),
        Err(DataCatalogError::InconsistentProductIdentity {
            field: "official_filename",
        })
    );
    assert_eq!(
        cddis_archive_url(&inconsistent),
        Err(DataCatalogError::InconsistentProductIdentity {
            field: "official_filename",
        })
    );
    assert_eq!(
        ProductRequest::new(inconsistent.clone(), vec![DistributionSource::NasaCddis]),
        Err(DataCatalogError::InconsistentProductIdentity {
            field: "official_filename",
        })
    );
    assert_eq!(
        inconsistent.cache_relpath(DistributionSource::NasaCddis),
        Err(DataCatalogError::InconsistentProductIdentity {
            field: "official_filename",
        })
    );

    let mut unsafe_identity = identity;
    unsafe_identity.official_filename = "../escape.SP3".to_string();
    assert_eq!(
        unsafe_identity.cache_relpath(DistributionSource::NasaCddis),
        Err(DataCatalogError::InvalidOfficialFilename(
            "../escape.SP3".to_string()
        ))
    );
}

#[test]
fn caller_constructed_unsupported_center_product_is_rejected_before_location() {
    let rapid_ionex = rapid_ionex(date(2026, 6, 13), None)
        .expect("rapid IONEX")
        .identity()
        .expect("identity");
    let mut unsupported = rapid_ionex;
    unsupported.family = ProductType::Sp3;
    unsupported.format = ProductFormat::Sp3;
    unsupported.sample = "05M".to_string();
    unsupported.official_filename = "COD0OPSRAP_20261640000_01D_05M_ORB.SP3".to_string();

    let expected = DataCatalogError::UnsupportedProduct {
        center: AnalysisCenter::CodRap,
        product_type: ProductType::Sp3,
    };
    assert_eq!(unsupported.validate(), Err(expected.clone()));
    assert_eq!(
        sidereon_core::data::distribution_location_for_identity(
            &unsupported,
            DistributionSource::Direct,
        ),
        Err(expected)
    );
}

#[test]
fn broadcast_navigation_identity_validates_fields_not_encoded_in_filename() {
    let product = mgex_nav(AnalysisCenter::Igs, date(2026, 7, 12), None).expect("IGS NAV");
    let identity = product.identity().expect("identity");
    assert_eq!(identity.validate(), Ok(()));

    let mut inconsistent = identity;
    inconsistent.sample = "30S".to_string();
    assert_eq!(
        inconsistent.validate(),
        Err(DataCatalogError::UnsupportedSample {
            center: AnalysisCenter::Igs,
            product_type: ProductType::Nav,
            sample: "30S".to_string(),
        })
    );
}

#[test]
fn local_sources_have_no_network_url_and_cddis_does_not_expand_family_scope() {
    let sp3 = mgex_sp3(AnalysisCenter::Cod, date(2026, 4, 30), None).expect("SP3");
    let local = sp3
        .distribution_location(DistributionSource::LocalFile)
        .expect("local location");
    assert_eq!(local.original_url, None);
    assert_eq!(local.compression, ArchiveCompression::None);

    let nav = mgex_nav(AnalysisCenter::Igs, date(2020, 6, 25), None).expect("NAV");
    assert_eq!(
        nav.distribution_location(DistributionSource::NasaCddis),
        Err(DataCatalogError::UnsupportedDistribution {
            source: DistributionSource::NasaCddis,
            product_type: ProductType::Nav,
        })
    );
}

#[test]
fn clock_and_broadcast_nav_urls_match_binding_catalog_examples() {
    let clk = mgex_clk(AnalysisCenter::Gfz, date(2020, 6, 24), None).expect("GFZ clock product");
    assert_eq!(
        clk.canonical_filename().expect("filename"),
        "GFZ0OPSRAP_20201760000_01D_30S_CLK.CLK"
    );
    assert_eq!(
        clk.archive_url().expect("url"),
        "https://isdc-data.gfz.de/gnss/products/rapid/w2111/GFZ0OPSRAP_20201760000_01D_30S_CLK.CLK.gz"
    );

    let nav =
        mgex_nav(AnalysisCenter::Igs, date(2020, 6, 25), None).expect("IGS broadcast nav product");
    assert_eq!(
        nav.canonical_filename().expect("filename"),
        "BRDC00WRD_R_20201770000_01D_MN.rnx"
    );
    assert_eq!(
        nav.archive_url().expect("url"),
        "https://igs.bkg.bund.de/root_ftp/IGS/BRDC/2020/177/BRDC00WRD_R_20201770000_01D_MN.rnx.gz"
    );
}

#[test]
fn station_observation_derivation_matches_binding_catalog_examples() {
    assert_eq!(
        station_obs_filename("ESBC00DNK", date(2020, 6, 25), "30S").expect("filename"),
        "ESBC00DNK_R_20201770000_01D_30S_MO.crx"
    );
    assert_eq!(
        station_obs_url("WTZR00DEU", date(2020, 6, 25), "30S").expect("url"),
        "https://igs.bkg.bund.de/root_ftp/IGS/obs/2020/177/WTZR00DEU_R_20201770000_01D_30S_MO.crx.gz"
    );
    assert_eq!(station_obs_protocol(), ArchiveProtocol::Https);

    let obs = station_obs("WTZR00DEU", date(2020, 6, 25), None).expect("station obs product");
    assert_eq!(
        obs.canonical_filename().expect("filename"),
        "WTZR00DEU_R_20201770000_01D_30S_MO.crx"
    );
    assert_eq!(
        obs.archive_url().expect("url"),
        "https://igs.bkg.bund.de/root_ftp/IGS/obs/2020/177/WTZR00DEU_R_20201770000_01D_30S_MO.crx.gz"
    );
}

#[test]
fn mirror_gating_matches_binding_catalog() {
    let err = product_convention(AnalysisCenter::Igs, ProductType::Ionex)
        .expect_err("IGS IONEX is mirror gated");
    assert_eq!(
        err,
        DataCatalogError::NoOpenMirror {
            center: "igs".to_string(),
            product_type: "ionex".to_string()
        }
    );

    assert!(no_open_mirrors()
        .iter()
        .any(|entry| entry.center == "grg" && entry.product_type == "sp3"));
    assert_eq!(
        open_mirror_code("grg_ult", "clk"),
        Err(DataCatalogError::NoOpenMirror {
            center: "grg_ult".to_string(),
            product_type: "clk".to_string()
        })
    );
    assert!(open_mirror_code("igs", "nav").is_ok());
}

#[test]
fn predicted_ionex_aliases_apply_the_existing_date_offset() {
    let prd1 = predicted_ionex(AnalysisCenter::CodPrd1, date(2026, 6, 14), None).expect("prd1");
    assert_eq!(
        prd1.canonical_filename().expect("filename"),
        "COD0OPSPRD_20261650000_01D_01H_GIM.INX"
    );

    let prd2 = predicted_ionex(AnalysisCenter::CodPrd2, date(2026, 6, 14), None).expect("prd2");
    assert_eq!(
        prd2.canonical_filename().expect("filename"),
        "COD0OPSPRD_20261660000_01D_01H_GIM.INX"
    );

    let same_file_prd1 =
        predicted_ionex(AnalysisCenter::CodPrd1, date(2026, 6, 15), None).expect("prd1");
    assert_eq!(
        same_file_prd1.canonical_filename().expect("filename"),
        prd2.canonical_filename().expect("filename")
    );
    assert_ne!(
        same_file_prd1.identity().expect("identity").key(),
        prd2.identity().expect("identity").key(),
        "prediction horizon must remain part of the normalized cache identity"
    );
    assert_ne!(
        same_file_prd1
            .identity()
            .expect("P1 identity")
            .cache_relpath(DistributionSource::Direct)
            .expect("P1 cache path"),
        prd2.identity()
            .expect("P2 identity")
            .cache_relpath(DistributionSource::Direct)
            .expect("P2 cache path"),
        "P1 and P2 must not collide even when their official filenames match"
    );
}

#[test]
fn predicted_ionex_direct_urls_use_exact_aiub_tier_and_identity_year() {
    let p1 = predicted_ionex(AnalysisCenter::CodPrd1, date(2026, 7, 15), None)
        .expect("P1 predicted IONEX");
    assert_eq!(
        p1.archive_url().expect("P1 direct URL"),
        "https://www.aiub.unibe.ch/download/CODE/IONO/P1/2026/\
COD0OPSPRD_20261960000_01D_01H_GIM.INX.gz"
    );

    let p2 = predicted_ionex(AnalysisCenter::CodPrd2, date(2026, 7, 15), None)
        .expect("P2 predicted IONEX");
    assert_eq!(
        p2.archive_url().expect("P2 direct URL"),
        "https://www.aiub.unibe.ch/download/CODE/IONO/P2/2026/\
COD0OPSPRD_20261970000_01D_01H_GIM.INX.gz"
    );

    let boundary = predicted_ionex(AnalysisCenter::CodPrd2, date(2026, 12, 31), None)
        .expect("year-boundary P2 predicted IONEX");
    assert_eq!(boundary.date, date(2027, 1, 1));
    assert_eq!(
        boundary.archive_url().expect("year-boundary P2 URL"),
        "https://www.aiub.unibe.ch/download/CODE/IONO/P2/2027/\
COD0OPSPRD_20270010000_01D_01H_GIM.INX.gz"
    );
}

#[test]
fn ultra_rapid_sp3_urls_match_binding_catalog_examples() {
    let igs = ops_ultra_sp3(AnalysisCenter::IgsUlt, date(2024, 9, 3), None, Some("0600"))
        .expect("IGS ultra SP3 product");
    assert_eq!(
        igs.canonical_filename().expect("filename"),
        "IGS0OPSULT_20242470600_02D_15M_ORB.SP3"
    );
    assert_eq!(
        igs.archive_url().expect("url"),
        "https://igs.bkg.bund.de/root_ftp/IGS/products/2330/IGS0OPSULT_20242470600_02D_15M_ORB.SP3.gz"
    );

    let esa = ops_ultra_sp3(AnalysisCenter::EsaUlt, date(2024, 9, 3), None, Some("0600"))
        .expect("ESA ultra SP3 product");
    assert_eq!(
        esa.archive_url().expect("url"),
        "https://navigation-office.esa.int/products/gnss-products/2330/ESA0OPSULT_20242470600_02D_15M_ORB.SP3.gz"
    );

    let cod = ops_ultra_sp3(AnalysisCenter::CodUlt, date(2026, 7, 14), None, None)
        .expect("CODE ultra SP3 product");
    assert_eq!(
        cod.canonical_filename().expect("filename"),
        "COD0OPSULT_20261950000_01D_05M_ORB.SP3"
    );
    assert_eq!(
        cod.archive_url().expect("url"),
        "https://www.aiub.unibe.ch/download/CODE/COD0OPSULT_20261950000_01D_05M_ORB.SP3"
    );
}

#[test]
fn ultra_rapid_sp3_locations_include_only_evidenced_dated_products() {
    let esa = ultra_sp3_locations(AnalysisCenter::EsaUlt, date(2026, 7, 12), "1800")
        .expect("ESA candidates");
    assert_eq!(esa.len(), 1);
    assert_eq!(esa[0].pattern, "primary_02D_05M");
    assert!(esa[0].filename.ends_with("_02D_05M_ORB.SP3"));

    let igs = ultra_sp3_locations(AnalysisCenter::IgsUlt, date(2026, 7, 12), "1800")
        .expect("IGS candidates");
    assert_eq!(igs.len(), 1);
    assert_eq!(igs[0].pattern, "primary_02D_15M");

    let code = ultra_sp3_locations(AnalysisCenter::CodUlt, date(2026, 7, 14), "0000")
        .expect("CODE candidates");
    assert_eq!(code.len(), 1);
    assert_eq!(code[0].pattern, "primary_01D_05M");
    assert_eq!(code[0].filename, "COD0OPSULT_20261950000_01D_05M_ORB.SP3");
    assert_eq!(
        code[0].url,
        "https://www.aiub.unibe.ch/download/CODE/COD0OPSULT_20261950000_01D_05M_ORB.SP3"
    );
    assert!(code.iter().all(|candidate| candidate
        .url
        .starts_with("https://www.aiub.unibe.ch/download/CODE/")));
    assert!(allowed_hosts().contains(&"www.aiub.unibe.ch"));

    let catalog_entry = catalog()
        .iter()
        .find(|entry| entry.center == AnalysisCenter::CodUlt)
        .expect("CODE ultra catalog entry");
    assert_eq!(catalog_entry.protocol, ArchiveProtocol::Https);
    assert_eq!(catalog_entry.host, "www.aiub.unibe.ch");
    assert_eq!(catalog_entry.root_url, "https://www.aiub.unibe.ch/download");

    let gfz_target = ProductDateTime::new(date(2026, 7, 12), 10, 0, 0).expect("target");
    let gfz_issues =
        sidereon_core::data::ultra_issue_candidates(AnalysisCenter::GfzUlt, gfz_target)
            .expect("GFZ issues");
    assert_eq!(gfz_issues[0].issue, "0900");
}

#[test]
fn ultra_sp3_defaults_and_candidate_order_follow_issue_cadence_eras() {
    let esa_transition = date(2025, 2, 2);
    assert_eq!(
        default_sample_for_date(AnalysisCenter::EsaUlt, ProductType::Sp3, esa_transition),
        Ok("15M")
    );
    let esa_0600 = ops_ultra_sp3(AnalysisCenter::EsaUlt, esa_transition, None, Some("0600"))
        .expect("last ESA 15M issue");
    assert_eq!(esa_0600.sample, "15M");
    assert_eq!(
        esa_0600.canonical_filename().expect("0600 filename"),
        "ESA0OPSULT_20250330600_02D_15M_ORB.SP3"
    );
    let esa_1200 = ops_ultra_sp3(AnalysisCenter::EsaUlt, esa_transition, None, Some("1200"))
        .expect("first ESA 05M issue");
    assert_eq!(esa_1200.sample, "05M");
    assert_eq!(
        esa_1200.canonical_filename().expect("1200 filename"),
        "ESA0OPSULT_20250331200_02D_05M_ORB.SP3"
    );

    let esa_0600_locations = ultra_sp3_locations(AnalysisCenter::EsaUlt, esa_transition, "0600")
        .expect("ESA 0600 candidates");
    assert_eq!(esa_0600_locations.len(), 1);
    assert_eq!(esa_0600_locations[0].pattern, "primary_02D_15M");
    let esa_1200_locations = ultra_sp3_locations(AnalysisCenter::EsaUlt, esa_transition, "1200")
        .expect("ESA 1200 candidates");
    assert_eq!(esa_1200_locations.len(), 1);
    assert_eq!(esa_1200_locations[0].pattern, "primary_02D_05M");

    let gfz_last_15m = date(2021, 5, 15);
    let gfz_first_5m = date(2021, 5, 16);
    assert_eq!(
        default_sample_for_date(AnalysisCenter::GfzUlt, ProductType::Sp3, gfz_last_15m),
        Ok("15M")
    );
    assert_eq!(
        default_sample_for_date(AnalysisCenter::GfzUlt, ProductType::Sp3, gfz_first_5m),
        Ok("05M")
    );
    assert_eq!(
        ops_ultra_sp3(AnalysisCenter::GfzUlt, gfz_last_15m, None, Some("2100"))
            .expect("last GFZ 15M default")
            .sample,
        "15M"
    );
    assert_eq!(
        ops_ultra_sp3(AnalysisCenter::GfzUlt, gfz_first_5m, None, Some("0000"))
            .expect("first GFZ 05M default")
            .sample,
        "05M"
    );
    let gfz_legacy_locations = ultra_sp3_locations(AnalysisCenter::GfzUlt, gfz_last_15m, "2100")
        .expect("GFZ legacy candidates");
    assert_eq!(gfz_legacy_locations.len(), 1);
    assert_eq!(gfz_legacy_locations[0].pattern, "primary_02D_15M");
    let gfz_current_locations = ultra_sp3_locations(AnalysisCenter::GfzUlt, gfz_first_5m, "0000")
        .expect("GFZ current candidates");
    assert_eq!(gfz_current_locations.len(), 1);
    assert_eq!(gfz_current_locations[0].pattern, "primary_02D_05M");

    let gfz_overlap = ultra_sp3_locations(AnalysisCenter::GfzUlt, gfz_last_15m, "0000")
        .expect("GFZ cataloged overlap");
    assert_eq!(gfz_overlap.len(), 2);
    assert_eq!(gfz_overlap[0].pattern, "primary_02D_15M");
    assert_eq!(gfz_overlap[1].pattern, "alternate_02D_05M");
    assert!(gfz_overlap.iter().all(|location| location.span == "02D"));

    // Candidate enumeration must round-trip through the same single-product
    // catalog constructor at every evidenced cadence boundary.
    for (center, product_date, issue) in [
        (AnalysisCenter::IgsUlt, date(2026, 7, 12), "1800"),
        (AnalysisCenter::CodUlt, date(2026, 7, 14), "0000"),
        (AnalysisCenter::EsaUlt, esa_transition, "0600"),
        (AnalysisCenter::EsaUlt, esa_transition, "1200"),
        (AnalysisCenter::GfzUlt, gfz_last_15m, "0000"),
        (AnalysisCenter::GfzUlt, gfz_last_15m, "2100"),
        (AnalysisCenter::GfzUlt, gfz_first_5m, "0000"),
    ] {
        for candidate in
            ultra_sp3_locations(center, product_date, issue).expect("cataloged candidates")
        {
            let spec = ops_ultra_sp3(center, product_date, Some(&candidate.sample), Some(issue))
                .expect("single-product constructor");
            let identity = spec.identity().expect("identity");
            assert_eq!(candidate.filename, spec.canonical_filename().expect("name"));
            assert_eq!(candidate.url, spec.archive_url().expect("URL"));
            assert_eq!(candidate.span, identity.span);
            assert_eq!(candidate.sample, identity.sample);
        }
    }
}

#[test]
fn sp3_content_start_convention_is_product_and_issue_aware() {
    assert_eq!(
        sp3_content_start_convention(AnalysisCenter::GfzUlt, date(2022, 9, 6), Some("2100")),
        Ok(Sp3ContentStartConvention::FilenameEpochMinusOneDay)
    );
    assert_eq!(
        sp3_content_start_convention(AnalysisCenter::GfzUlt, date(2022, 9, 8), Some("0600")),
        Ok(Sp3ContentStartConvention::FilenameEpochMinusOneDay)
    );
    assert_eq!(
        sp3_content_start_convention(AnalysisCenter::GfzUlt, date(2022, 9, 8), Some("0900")),
        Ok(Sp3ContentStartConvention::FilenameEpoch)
    );
    assert_eq!(
        sp3_content_start_convention(AnalysisCenter::GfzUlt, date(2022, 9, 9), Some("0000")),
        Ok(Sp3ContentStartConvention::FilenameEpoch)
    );
    assert_eq!(
        Sp3ContentStartConvention::FilenameEpochMinusOneDay.content_start_offset_s(),
        -86_400
    );
    assert_eq!(
        Sp3ContentStartConvention::FilenameEpochMinusOneDay.code(),
        "filename_epoch_minus_one_day"
    );

    assert!(matches!(
        sp3_content_start_convention(AnalysisCenter::GfzUlt, date(2022, 9, 8), Some("0130")),
        Err(DataCatalogError::UnsupportedIssue { .. })
    ));
    assert_eq!(
        sp3_content_start_convention(AnalysisCenter::Gfz, date(2022, 9, 8), Some("0000")),
        Err(DataCatalogError::UnexpectedIssue {
            center: AnalysisCenter::Gfz,
        })
    );
}

#[test]
fn unsupported_cadence_is_rejected_before_filename_or_url_derivation() {
    let cases = [
        UnsupportedCadenceCase {
            center: AnalysisCenter::Igs,
            product_date: date(2026, 6, 15),
            issue: None,
            expected_samples: &["15M"],
            rejected_sample: "05M",
        },
        UnsupportedCadenceCase {
            center: AnalysisCenter::Esa,
            product_date: date(2026, 6, 15),
            issue: None,
            expected_samples: &["05M"],
            rejected_sample: "15M",
        },
        UnsupportedCadenceCase {
            center: AnalysisCenter::Cod,
            product_date: date(2026, 6, 15),
            issue: None,
            expected_samples: &["05M"],
            rejected_sample: "15M",
        },
        UnsupportedCadenceCase {
            center: AnalysisCenter::Gfz,
            product_date: date(2026, 6, 15),
            issue: None,
            expected_samples: &["05M"],
            rejected_sample: "15M",
        },
        UnsupportedCadenceCase {
            center: AnalysisCenter::IgsUlt,
            product_date: date(2026, 7, 19),
            issue: Some("1200"),
            expected_samples: &["15M"],
            rejected_sample: "05M",
        },
        UnsupportedCadenceCase {
            center: AnalysisCenter::CodUlt,
            product_date: date(2026, 7, 19),
            issue: Some("0000"),
            expected_samples: &["05M"],
            rejected_sample: "15M",
        },
        UnsupportedCadenceCase {
            center: AnalysisCenter::EsaUlt,
            product_date: date(2026, 7, 19),
            issue: Some("1200"),
            expected_samples: &["05M"],
            rejected_sample: "15M",
        },
        UnsupportedCadenceCase {
            center: AnalysisCenter::GfzUlt,
            product_date: date(2026, 7, 19),
            issue: Some("0300"),
            expected_samples: &["05M"],
            rejected_sample: "15M",
        },
        // Boundary controls from the live matrix: the GFZ rapid cadence
        // transition and the one overlapping GFZ-ultra issue must be exact.
        UnsupportedCadenceCase {
            center: AnalysisCenter::Gfz,
            product_date: date(2021, 5, 17),
            issue: None,
            expected_samples: &["15M"],
            rejected_sample: "05M",
        },
        UnsupportedCadenceCase {
            center: AnalysisCenter::GfzUlt,
            product_date: date(2021, 5, 15),
            issue: Some("0300"),
            expected_samples: &["15M"],
            rejected_sample: "05M",
        },
        UnsupportedCadenceCase {
            center: AnalysisCenter::EsaUlt,
            product_date: date(2025, 2, 2),
            issue: Some("0600"),
            expected_samples: &["15M"],
            rejected_sample: "05M",
        },
        UnsupportedCadenceCase {
            center: AnalysisCenter::EsaUlt,
            product_date: date(2025, 2, 2),
            issue: Some("1200"),
            expected_samples: &["05M"],
            rejected_sample: "15M",
        },
    ];

    for case in cases {
        assert_eq!(
            supported_samples(case.center, ProductType::Sp3, case.product_date, case.issue,),
            Ok(case.expected_samples),
        );
        let expected = DataCatalogError::UnsupportedSample {
            center: case.center,
            product_type: ProductType::Sp3,
            sample: case.rejected_sample.to_string(),
        };
        let construction = if let Some(issue) = case.issue {
            ops_ultra_sp3(
                case.center,
                case.product_date,
                Some(case.rejected_sample),
                Some(issue),
            )
        } else {
            mgex_sp3(case.center, case.product_date, Some(case.rejected_sample))
        };
        assert_eq!(construction.unwrap_err(), expected);
        assert_eq!(
            canonical_filename(
                case.center,
                ProductType::Sp3,
                case.product_date,
                Some(case.rejected_sample),
                case.issue,
            )
            .unwrap_err(),
            expected
        );
        assert_eq!(
            archive_url(
                case.center,
                ProductType::Sp3,
                case.product_date,
                Some(case.rejected_sample),
                case.issue,
            )
            .unwrap_err(),
            expected
        );
    }
}

#[test]
fn supported_samples_are_date_and_issue_aware_for_every_sp3_line() {
    let cases: &[(AnalysisCenter, ProductDate, Option<&str>, &[&str])] = &[
        (AnalysisCenter::Igs, date(2026, 6, 15), None, &["15M"]),
        (AnalysisCenter::Esa, date(2026, 6, 15), None, &["05M"]),
        (AnalysisCenter::Cod, date(2026, 6, 15), None, &["05M"]),
        (
            AnalysisCenter::IgsUlt,
            date(2026, 7, 19),
            Some("1200"),
            &["15M"],
        ),
        (
            AnalysisCenter::CodUlt,
            date(2026, 7, 19),
            Some("0000"),
            &["05M"],
        ),
        (AnalysisCenter::Gfz, date(2021, 5, 17), None, &["15M"]),
        (AnalysisCenter::Gfz, date(2021, 5, 18), None, &["05M"]),
        (
            AnalysisCenter::EsaUlt,
            date(2025, 2, 2),
            Some("0600"),
            &["15M"],
        ),
        (
            AnalysisCenter::EsaUlt,
            date(2025, 2, 2),
            Some("1200"),
            &["05M"],
        ),
        (
            AnalysisCenter::GfzUlt,
            date(2021, 5, 14),
            Some("0000"),
            &["15M"],
        ),
        (
            AnalysisCenter::GfzUlt,
            date(2021, 5, 15),
            Some("0000"),
            &["15M", "05M"],
        ),
        (
            AnalysisCenter::GfzUlt,
            date(2021, 5, 15),
            Some("2100"),
            &["15M"],
        ),
        (
            AnalysisCenter::GfzUlt,
            date(2021, 5, 16),
            Some("0000"),
            &["05M"],
        ),
    ];
    for (center, product_date, issue, expected) in cases {
        assert_eq!(
            supported_samples(*center, ProductType::Sp3, *product_date, *issue),
            Ok(*expected),
            "{center:?} {product_date:?} issue {issue:?}",
        );
    }

    assert_eq!(
        ops_ultra_sp3(
            AnalysisCenter::GfzUlt,
            date(2021, 5, 15),
            Some("05M"),
            Some("2100"),
        ),
        Err(DataCatalogError::UnsupportedSample {
            center: AnalysisCenter::GfzUlt,
            product_type: ProductType::Sp3,
            sample: "05M".to_string(),
        })
    );
}

#[test]
fn supported_samples_cover_every_current_catalog_product_family() {
    let product_date = date(2026, 7, 19);
    for entry in catalog() {
        let issue = entry.issues.first().copied();
        for convention in entry.products {
            assert_eq!(
                supported_samples(entry.center, convention.product_type, product_date, issue,),
                Ok(&[convention.default_sample][..]),
                "{:?} {:?}",
                entry.center,
                convention.product_type,
            );

            let unsupported = if convention.default_sample == "05M" {
                "15M"
            } else {
                "05M"
            };
            assert_eq!(
                canonical_filename(
                    entry.center,
                    convention.product_type,
                    product_date,
                    Some(unsupported),
                    issue,
                ),
                Err(DataCatalogError::UnsupportedSample {
                    center: entry.center,
                    product_type: convention.product_type,
                    sample: unsupported.to_string(),
                }),
                "{:?} {:?}",
                entry.center,
                convention.product_type,
            );

            let mut identity = product(
                entry.center,
                convention.product_type,
                product_date,
                None,
                issue,
            )
            .expect("catalog product")
            .identity()
            .expect("catalog identity");
            let unsupported_span = if convention.span == "01D" {
                "02D"
            } else {
                "01D"
            };
            identity.span = unsupported_span.to_string();
            identity.official_filename = identity.official_filename.replacen(
                &format!("_{}_", convention.span),
                &format!("_{unsupported_span}_"),
                1,
            );
            assert_eq!(
                identity.validate(),
                Err(DataCatalogError::InconsistentProductIdentity { field: "span" }),
                "{:?} {:?}",
                entry.center,
                convention.product_type,
            );
        }
    }
}

#[test]
#[ignore = "network test for AIUB's current CODE ultra-rapid object store"]
fn live_aiub_code_ultra_day_195_downloads_and_parses_as_sp3() {
    let candidate = ultra_sp3_locations(AnalysisCenter::CodUlt, date(2026, 7, 14), "0000")
        .expect("CODE candidates")
        .remove(0);
    assert_eq!(candidate.pattern, "primary_01D_05M");

    let response = Command::new("curl")
        .args([
            "--http1.1",
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--retry",
            "2",
            &candidate.url,
        ])
        .output()
        .expect("run curl");
    assert!(
        response.status.success(),
        "curl failed for {}: {}",
        candidate.url,
        String::from_utf8_lossy(&response.stderr)
    );
    assert!(response.stdout.starts_with(b"#dP"));
    assert_eq!(response.stdout.len(), 1_473_962);

    let sp3 = sidereon_core::ephemeris::Sp3::parse(&response.stdout)
        .expect("downloaded CODE object parses as SP3");
    assert_eq!(sp3.epoch_count(), 289);
    assert_eq!(sp3.header.epoch_interval_s, 300.0);
}

#[test]
fn free_functions_derive_string_identical_names_and_urls() {
    let name = canonical_filename(
        AnalysisCenter::GfzUlt,
        ProductType::Sp3,
        date(2024, 9, 3),
        None,
        Some("1200"),
    )
    .expect("filename");
    assert_eq!(name, "GFZ0OPSULT_20242471200_02D_05M_ORB.SP3");

    let url = archive_url(
        AnalysisCenter::GfzUlt,
        ProductType::Sp3,
        date(2024, 9, 3),
        None,
        Some("1200"),
    )
    .expect("url");
    assert_eq!(
        url,
        "https://isdc-data.gfz.de/gnss/products/ultra/w2330/GFZ0OPSULT_20242471200_02D_05M_ORB.SP3.gz"
    );
}

#[test]
fn date_from_gps_week_day_can_drive_product_derivation() {
    let date = ProductDate::from_gps_week_day(2111, 3).expect("week/day date");
    assert_eq!(date, ProductDate::new(2020, 6, 24).expect("date"));

    let name = canonical_filename(AnalysisCenter::Esa, ProductType::Sp3, date, None, None)
        .expect("filename");
    assert_eq!(name, "ESA0MGNFIN_20201760000_01D_05M_ORB.SP3");
}

#[test]
fn pure_issue_and_ionex_candidate_selection_matches_bindings() {
    let target = ProductDateTime::new(date(2024, 9, 3), 13, 0, 0).expect("target");
    let available = [
        UltraIssue::new(date(2024, 9, 3), "0000").expect("issue"),
        UltraIssue::new(date(2024, 9, 3), "0600").expect("issue"),
    ];
    let selected = latest_ops_ultra_sp3(AnalysisCenter::GfzUlt, target, None, Some(&available))
        .expect("latest available product");
    assert_eq!(
        selected.canonical_filename().expect("filename"),
        "GFZ0OPSULT_20242470600_02D_05M_ORB.SP3"
    );

    let candidates =
        gim_date_candidates(AnalysisCenter::CodPrd1, date(2026, 6, 14), 1).expect("candidates");
    assert_eq!(candidates, vec![date(2026, 6, 14), date(2026, 6, 13)]);
}

#[test]
fn skadi_source_entry_and_host_allowlist_are_cataloged() {
    let source = skadi_source_entry();
    assert_eq!(source.protocol, ArchiveProtocol::Https);
    assert_eq!(source.host, "s3.amazonaws.com");
    assert_eq!(source.compression, ArchiveCompression::Gzip);
    assert_eq!(source.compression.as_str(), "gzip");
    assert_eq!(
        source.root_url,
        "https://s3.amazonaws.com/elevation-tiles-prod"
    );
    assert!(allowed_hosts().contains(&"s3.amazonaws.com"));
}

#[test]
fn celestrak_space_weather_catalog_entry_and_paths_are_stable() {
    let source = space_weather_source_entry();
    assert_eq!(source.protocol, ArchiveProtocol::Https);
    assert_eq!(source.host, "celestrak.org");
    assert_eq!(source.compression, ArchiveCompression::None);
    assert_eq!(source.compression.as_str(), "none");
    assert_eq!(source.root_url, "https://celestrak.org/SpaceData");
    assert!(allowed_hosts().contains(&"celestrak.org"));

    assert_eq!(SpaceWeatherProduct::All.code(), "sw_all");
    assert_eq!(
        SpaceWeatherProduct::from_code("sw_last5"),
        Some(SpaceWeatherProduct::Last5Years)
    );
    assert_eq!("sw_all".parse(), Ok(SpaceWeatherProduct::All));
    assert_eq!(
        "bad".parse::<SpaceWeatherProduct>(),
        Err(DataCatalogError::UnknownProductType("bad".to_string()))
    );
    assert_eq!(
        space_weather_filename(SpaceWeatherProduct::All),
        "SW-All.csv"
    );
    assert_eq!(
        space_weather_filename(SpaceWeatherProduct::Last5Years),
        "SW-Last5Years.csv"
    );
    assert_eq!(
        space_weather_archive_url(SpaceWeatherProduct::All),
        "https://celestrak.org/SpaceData/SW-All.csv"
    );
    assert_eq!(
        space_weather_archive_url(SpaceWeatherProduct::Last5Years),
        "https://celestrak.org/SpaceData/SW-Last5Years.csv"
    );
    assert_eq!(
        space_weather_cache_relpath(SpaceWeatherProduct::All),
        "space-weather/SW-All.csv"
    );
    assert_eq!(
        space_weather_cache_relpath(SpaceWeatherProduct::Last5Years),
        "space-weather/SW-Last5Years.csv"
    );
}

#[test]
fn skadi_tile_and_dted_derivation_match_known_tile_ids() {
    assert_eq!(skadi_tile_id(36, -107).expect("tile id"), "N36W107");
    assert_eq!(skadi_band(36).expect("band"), "N36");
    assert_eq!(
        skadi_archive_url(36, -107).expect("url"),
        "https://s3.amazonaws.com/elevation-tiles-prod/skadi/N36/N36W107.hgt.gz"
    );
    assert_eq!(
        dted_tile_filename(36, -107).expect("filename"),
        "n36_w107_1arc_v3.dt2"
    );
    assert_eq!(dted_block_dir(36, -107).expect("block"), "n30_w100");
    assert_eq!(
        dted_cache_relpath(36, -107).expect("relative path"),
        "n30_w100/n36_w107_1arc_v3.dt2"
    );

    assert_eq!(skadi_tile_id(-1, 10).expect("tile id"), "S01E010");
    assert_eq!(skadi_band(-1).expect("band"), "S01");
    assert_eq!(dted_block_dir(-1, 10).expect("block"), "s00_e010");
    assert_eq!(
        skadi_archive_url(-1, 10).expect("url"),
        "https://s3.amazonaws.com/elevation-tiles-prod/skadi/S01/S01E010.hgt.gz"
    );

    assert_eq!(dted_block_dir(32, -117).expect("block"), "n30_w110");
    assert_eq!(dted_block_dir(43, -112).expect("block"), "n40_w110");
    assert_eq!(dted_block_dir(20, -103).expect("block"), "n20_w100");
}

#[test]
fn southern_and_western_hemisphere_tiles_floor_to_sw_corner() {
    // A fractional coordinate names the tile at its floored (south-west) integer
    // corner. DTED block directories use the signed tile index hemisphere and
    // truncate the absolute index magnitude to a ten-degree bucket.

    // Southern latitude, western longitude.
    assert_eq!(
        terrain_tile_index(-32.83, -117.12).expect("index"),
        (-33, -118)
    );
    assert_eq!(skadi_tile_id(-33, -118).expect("tile id"), "S33W118");
    assert_eq!(skadi_band(-33).expect("band"), "S33");
    assert_eq!(
        skadi_archive_url(-33, -118).expect("url"),
        "https://s3.amazonaws.com/elevation-tiles-prod/skadi/S33/S33W118.hgt.gz"
    );
    assert_eq!(
        dted_tile_filename(-33, -118).expect("filename"),
        "s33_w118_1arc_v3.dt2"
    );
    assert_eq!(dted_block_dir(-33, -118).expect("block"), "s30_w110");
    assert_eq!(
        dted_cache_relpath(-33, -118).expect("relpath"),
        "s30_w110/s33_w118_1arc_v3.dt2"
    );

    // Southern latitude, eastern longitude.
    assert_eq!(terrain_tile_index(-33.92, 18.42).expect("index"), (-34, 18));
    assert_eq!(skadi_tile_id(-34, 18).expect("tile id"), "S34E018");
    assert_eq!(dted_block_dir(-34, 18).expect("block"), "s30_e010");
    assert_eq!(
        dted_cache_relpath(-34, 18).expect("relpath"),
        "s30_e010/s34_e018_1arc_v3.dt2"
    );

    // Just south and west of the origin: the floored corner is -1, not 0.
    assert_eq!(terrain_tile_index(-0.5, -0.5).expect("index"), (-1, -1));
    assert_eq!(skadi_tile_id(-1, -1).expect("tile id"), "S01W001");
    assert_eq!(dted_block_dir(-1, -1).expect("block"), "s00_w000");

    // Northern and eastern control: the same flooring rule, no sign flip.
    assert_eq!(terrain_tile_index(45.5, 10.5).expect("index"), (45, 10));
    assert_eq!(skadi_tile_id(45, 10).expect("tile id"), "N45E010");
    assert_eq!(dted_block_dir(45, 10).expect("block"), "n40_e010");
}

#[test]
fn dted_block_dir_truncates_signed_indices_to_observed_buckets() {
    assert_eq!(dted_block_dir(32, -110).expect("block"), "n30_w110");
    assert_eq!(dted_block_dir(32, -111).expect("block"), "n30_w110");
    assert_eq!(dted_block_dir(32, -1).expect("block"), "n30_w000");
    assert_eq!(dted_block_dir(32, -10).expect("block"), "n30_w010");
    assert_eq!(dted_block_dir(1, 1).expect("block"), "n00_e000");
    assert_eq!(dted_block_dir(-1, 1).expect("block"), "s00_e000");
    assert_ne!(
        dted_block_dir(1, 1).expect("north block"),
        dted_block_dir(-1, 1).expect("south block")
    );
}

#[test]
fn dted_block_dir_matches_observed_block_layout_fixture() {
    let mut checked = 0usize;
    for (line_index, line) in include_str!("fixtures/dted/observed_block_layout.txt")
        .lines()
        .enumerate()
    {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut fields = line.split_whitespace();
        let tile_stem = fields.next().expect("observed tile stem");
        let observed_block = fields.next().expect("observed block dir");
        assert!(
            fields.next().is_none(),
            "unexpected field in observed layout line {}",
            line_index + 1
        );

        let (lat_index, lon_index) = parse_observed_dted_tile_stem(tile_stem);
        let derived = dted_block_dir(lat_index, lon_index).expect("derived block dir");
        assert_eq!(
            derived,
            observed_block,
            "observed layout line {}",
            line_index + 1
        );
        checked += 1;
    }
    assert_eq!(checked, 888);
}

fn parse_observed_dted_tile_stem(stem: &str) -> (i32, i32) {
    let mut parts = stem.split('_');
    let lat = parts.next().expect("latitude token");
    let lon = parts.next().expect("longitude token");
    assert_eq!(parts.next(), Some("1arc"));
    assert_eq!(parts.next(), Some("v3"));
    assert_eq!(parts.next(), None);

    (
        parse_signed_index(lat, 'n', 's'),
        parse_signed_index(lon, 'e', 'w'),
    )
}

fn parse_signed_index(token: &str, positive: char, negative: char) -> i32 {
    let mut chars = token.chars();
    let hemi = chars.next().expect("hemisphere");
    let magnitude = chars.as_str().parse::<i32>().expect("index magnitude");
    match hemi {
        h if h == positive => magnitude,
        h if h == negative => -magnitude,
        _ => panic!("unexpected hemisphere token {token}"),
    }
}

#[test]
fn parse_skadi_tile_id_validates_format_and_range() {
    assert_eq!(
        parse_skadi_tile_id("N36W107").expect("parsed tile"),
        (36, -107)
    );
    assert_eq!(
        parse_skadi_tile_id("S01E010").expect("parsed tile"),
        (-1, 10)
    );
    assert_eq!(
        parse_skadi_tile_id("N90E000"),
        Err(DataCatalogError::InvalidTileIndex {
            lat_index: 90,
            lon_index: 0
        })
    );
    assert_eq!(
        parse_skadi_tile_id("S00E010"),
        Err(DataCatalogError::InvalidTileId("S00E010".to_string()))
    );
    assert_eq!(
        parse_skadi_tile_id("n36w107"),
        Err(DataCatalogError::InvalidTileId("n36w107".to_string()))
    );
}

#[test]
fn terrain_tile_index_matches_reader_grid_and_clamps_upper_edges() {
    assert_eq!(
        terrain_tile_index(36.75, -106.25).expect("tile index"),
        (36, -107)
    );
    assert_eq!(
        terrain_tile_index(-0.25, 10.9).expect("tile index"),
        (-1, 10)
    );
    assert_eq!(
        terrain_tile_index(90.0, 180.0).expect("upper edge tile index"),
        (89, 179)
    );
    assert_eq!(
        terrain_tile_index(-90.0, -180.0).expect("lower edge tile index"),
        (-90, -180)
    );
    assert_eq!(
        terrain_tile_index(f64::NAN, -106.5),
        Err(DataCatalogError::InvalidCoordinate {
            lat_deg_bits: f64::NAN.to_bits(),
            lon_deg_bits: (-106.5f64).to_bits()
        })
    );
    assert_eq!(
        skadi_tile_id(90, 0),
        Err(DataCatalogError::InvalidTileIndex {
            lat_index: 90,
            lon_index: 0
        })
    );
}
