//! Opt-in live checks for official padded SP3 terminal records.
//!
//! The deterministic default suite is network-free. Run these checks with:
//! `cargo test -p sidereon --test sp3_terminal_record_live -- --ignored`.

use std::io::Read;
use std::process::Command;

use sidereon_core::data::{mgex_sp3, AnalysisCenter, ProductDate};
use sidereon_core::ephemeris::{parse_exact_sp3, ExactSp3Request};

fn date(year: i32, month: u8, day: u8) -> ProductDate {
    ProductDate::new(year, month, day).expect("valid test date")
}

fn fetch(url: &str) -> Vec<u8> {
    let response = Command::new("curl")
        .args([
            "--http1.1",
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--retry",
            "2",
            url,
        ])
        .output()
        .expect("run curl");
    assert!(
        response.status.success(),
        "curl failed for {url}: {}",
        String::from_utf8_lossy(&response.stderr)
    );
    response.stdout
}

fn gunzip(archive: &[u8]) -> Vec<u8> {
    let mut product = Vec::new();
    flate2::read::MultiGzDecoder::new(archive)
        .read_to_end(&mut product)
        .expect("gzip CRC, size, and stream completion are valid");
    product
}

fn terminal_record(product: &[u8]) -> &str {
    std::str::from_utf8(product)
        .expect("SP3 is ASCII")
        .lines()
        .next_back()
        .expect("product has a record")
}

fn assert_supported_terminal_record(record: &str) {
    let padding = record
        .strip_prefix("EOF")
        .unwrap_or_else(|| panic!("terminal record is not anchored EOF: {record:?}"));
    assert!(record.len() <= 80, "terminal record exceeds policy width");
    assert!(
        padding.bytes().all(|byte| byte == b' '),
        "terminal padding is not ASCII spaces"
    );
}

#[test]
#[ignore = "network test for ESA's official MGEX final SP3 archive"]
fn live_esa_final_padded_terminal_record_passes_the_exact_gate() {
    let product_date = date(2025, 7, 15);
    let spec = mgex_sp3(AnalysisCenter::Esa, product_date, None).expect("ESA final SP3");
    let url = spec.archive_url().expect("archive URL");
    assert_eq!(
        url,
        "https://navigation-office.esa.int/products/gnss-products/2375/\
ESA0MGNFIN_20251960000_01D_05M_ORB.SP3.gz"
    );

    let archive = fetch(&url);
    assert_eq!(archive.len(), 966_204);
    let product = gunzip(&archive);
    assert_eq!(product.len(), 2_740_975);
    assert_eq!(terminal_record(&product).len(), 80);
    assert_supported_terminal_record(terminal_record(&product));
    assert!(!product.contains(&b'\r'), "official product is LF-only");

    let request =
        ExactSp3Request::from_identity(&spec.identity().expect("identity")).expect("exact request");
    let (parsed, _) = parse_exact_sp3(&product, &request)
        .expect("official padded terminal record must pass the exact gate");
    assert_eq!(parsed.header.agency.trim(), "ESOC");
    assert_eq!(parsed.epoch_count(), 289);
    assert_eq!(parsed.header.epoch_interval_s, 300.0);
}

#[test]
#[ignore = "network test for GFZ's official rapid SP3 archive"]
fn live_gfz_rapid_padded_terminal_record_passes_the_exact_gate() {
    let spec = mgex_sp3(AnalysisCenter::Gfz, date(2026, 7, 19), None).expect("GFZ rapid SP3");
    let product = gunzip(&fetch(&spec.archive_url().expect("archive URL")));

    assert_eq!(terminal_record(&product).len(), 40);
    assert_supported_terminal_record(terminal_record(&product));

    let request =
        ExactSp3Request::from_identity(&spec.identity().expect("identity")).expect("exact request");
    parse_exact_sp3(&product, &request)
        .expect("official GFZ padded terminal record must pass the exact gate");
}
