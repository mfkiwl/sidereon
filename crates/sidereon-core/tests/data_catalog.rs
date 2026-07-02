use sidereon_core::data::{
    archive_url, canonical_filename, gim_date_candidates, latest_ops_ultra_sp3, mgex_clk,
    mgex_ionex, mgex_nav, mgex_sp3, no_open_mirrors, open_mirror_code, ops_ultra_sp3,
    predicted_ionex, product_convention, rapid_ionex, station_obs, station_obs_filename,
    station_obs_protocol, station_obs_url, AnalysisCenter, ArchiveProtocol, DataCatalogError,
    ProductDate, ProductDateTime, ProductType, UltraIssue,
};

fn date(year: i32, month: u8, day: u8) -> ProductDate {
    ProductDate::new(year, month, day).expect("valid test date")
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
        "http://ftp.aiub.unibe.ch/CODE/COD0OPSRAP_20261640000_01D_01H_GIM.INX.gz"
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

    let cod = ops_ultra_sp3(AnalysisCenter::CodUlt, date(2026, 6, 11), None, None)
        .expect("CODE ultra SP3 product");
    assert_eq!(
        cod.canonical_filename().expect("filename"),
        "COD0OPSULT_20261620000_01D_05M_ORB.SP3"
    );
    assert_eq!(
        cod.archive_url().expect("url"),
        "http://ftp.aiub.unibe.ch/CODE/COD0OPSULT_20261620000_01D_05M_ORB.SP3"
    );
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
