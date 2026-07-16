use sidereon::data::{
    dted_cache_relpath, hgt_to_dted, mgex_nav, mgex_sp3, ops_ultra_sp3, parse_skadi_tile_id,
    skadi_archive_url, station_obs_url, terrain_tile_index, ultra_sp3_locations,
    validate_exact_product_set, AnalysisCenter, ProductDate,
};

#[test]
fn facade_reexports_data_catalog_derivation() {
    let product = ops_ultra_sp3(
        AnalysisCenter::IgsUlt,
        ProductDate::new(2024, 9, 3).expect("valid date"),
        None,
        Some("0600"),
    )
    .expect("ultra product");

    assert_eq!(
        product.canonical_filename().expect("filename"),
        "IGS0OPSULT_20242470600_02D_15M_ORB.SP3"
    );
    assert_eq!(
        product.archive_url().expect("url"),
        "https://igs.bkg.bund.de/root_ftp/IGS/products/2330/IGS0OPSULT_20242470600_02D_15M_ORB.SP3.gz"
    );
}

#[test]
fn facade_reexports_exact_product_set_gate() {
    let first = mgex_sp3(
        AnalysisCenter::Cod,
        ProductDate::new(2026, 7, 12).expect("valid date"),
        None,
    )
    .expect("first product")
    .identity()
    .expect("first identity");
    let second = mgex_sp3(
        AnalysisCenter::Cod,
        ProductDate::new(2026, 7, 13).expect("valid date"),
        None,
    )
    .expect("second product")
    .identity()
    .expect("second identity");

    assert_eq!(
        validate_exact_product_set(&[first.clone(), second.clone()], &[second.clone(), first]),
        Ok(())
    );
    assert!(validate_exact_product_set(&[second], &[]).is_err());
}

#[test]
fn facade_reexports_ultra_sp3_fallback_locations() {
    let locations = ultra_sp3_locations(
        AnalysisCenter::EsaUlt,
        ProductDate::new(2026, 7, 13).expect("valid date"),
        "0000",
    )
    .expect("ultra locations");

    assert_eq!(locations[0].pattern, "primary_02D_05M");
    assert_eq!(locations[1].pattern, "alternate_02D_15M");

    let code_locations = ultra_sp3_locations(
        AnalysisCenter::CodUlt,
        ProductDate::new(2026, 7, 14).expect("valid date"),
        "0000",
    )
    .expect("CODE ultra locations");
    assert_eq!(code_locations[0].pattern, "primary_01D_05M");
    assert_eq!(
        code_locations[0].url,
        "https://www.aiub.unibe.ch/download/CODE/COD0OPSULT_20261950000_01D_05M_ORB.SP3"
    );
    assert_eq!(code_locations[1].pattern, "alternate_02D_05M");
    assert_eq!(
        code_locations[1].url,
        "https://www.aiub.unibe.ch/download/CODE/COD0OPSULT_20261950000_02D_05M_ORB.SP3"
    );
    assert_eq!(code_locations[2].pattern, "alias_latest");
    assert_eq!(
        code_locations[2].url,
        "https://www.aiub.unibe.ch/download/CODE/COD0OPSULT.SP3"
    );
}

#[test]
fn facade_reexports_expanded_data_catalog_derivation() {
    let date = ProductDate::new(2020, 6, 25).expect("valid date");
    let nav = mgex_nav(AnalysisCenter::Igs, date, None).expect("nav product");

    assert_eq!(
        nav.canonical_filename().expect("filename"),
        "BRDC00WRD_R_20201770000_01D_MN.rnx"
    );
    assert_eq!(
        station_obs_url("WTZR00DEU", date, "30S").expect("url"),
        "https://igs.bkg.bund.de/root_ftp/IGS/obs/2020/177/WTZR00DEU_R_20201770000_01D_30S_MO.crx.gz"
    );
}

#[test]
fn facade_reexports_terrain_data_derivation_and_conversion() {
    assert_eq!(
        terrain_tile_index(36.75, -106.25).expect("tile index"),
        (36, -107)
    );
    assert_eq!(
        parse_skadi_tile_id("N36W107").expect("parse tile id"),
        (36, -107)
    );
    assert_eq!(
        skadi_archive_url(36, -107).expect("skadi URL"),
        "https://s3.amazonaws.com/elevation-tiles-prod/skadi/N36/N36W107.hgt.gz"
    );
    assert_eq!(
        dted_cache_relpath(36, -107).expect("DTED cache path"),
        "n30_w100/n36_w107_1arc_v3.dt2"
    );
    assert!(hgt_to_dted(36, -107, &[]).is_err());
}
