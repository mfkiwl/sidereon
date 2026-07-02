use sidereon::data::{mgex_nav, ops_ultra_sp3, station_obs_url, AnalysisCenter, ProductDate};

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
