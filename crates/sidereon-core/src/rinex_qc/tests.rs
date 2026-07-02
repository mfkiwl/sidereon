//! Focused first-slice RINEX QC tests.
//!
//! Broken inputs are deterministic mutations of small in-test products or
//! existing public fixtures under `tests/fixtures`. The QC layer delegates text
//! decoding to the existing RINEX and CRINEX modules; these tests cover the
//! typed rule/action surface added by this module.

use super::*;

fn header_line(body: &str, label: &str) -> String {
    format!("{body:<60}{label}")
}

fn obs_text(headers: &[String], body: &str) -> String {
    let mut lines = vec![
        header_line(
            "     3.05           OBSERVATION DATA    M (MIXED)",
            "RINEX VERSION / TYPE",
        ),
        header_line("QC01", "MARKER NAME"),
        header_line(
            "        0.0000        0.0000        0.0000",
            "ANTENNA: DELTA H/E/N",
        ),
        header_line("G    1 C1C", "SYS / # / OBS TYPES"),
    ];
    lines.extend(headers.iter().cloned());
    lines.push(header_line("", "END OF HEADER"));
    lines.extend(body.lines().map(str::to_string));
    lines.join("\n")
}

fn gps_epoch(minute: u8, second: f64, value: &str) -> String {
    format!("> 2020 01 01 00 {minute:02}{second:11.7}  0  1\nG01{value}")
}

fn finding_codes(report: &LintReport) -> Vec<&'static str> {
    report.findings.iter().map(Finding::code).collect()
}

fn nav_fixture() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/nav/ESBC00DNK_R_20201770000_01D_MN.rnx"
    );
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read NAV fixture {path}: {e}"))
}

#[test]
fn obs_lint_reports_time_interval_order_and_repair_fixes_them() {
    let headers = [
        header_line(
            "  2020    01    01    00    00    1.0000000     GPS",
            "TIME OF FIRST OBS",
        ),
        header_line("    60.000", "INTERVAL"),
    ];
    let body = [
        gps_epoch(1, 0.0, "  20000000.000  "),
        gps_epoch(0, 0.0, "  21000000.000  "),
        gps_epoch(0, 30.0, "  20000000.000  "),
        gps_epoch(0, 0.0, "  22000000.000  "),
    ]
    .join("\n");
    let obs = RinexObs::parse(&obs_text(&headers, &body)).expect("parse OBS");

    let report = lint_obs(&obs);
    let codes = finding_codes(&report);
    assert!(codes.contains(&"OBS-H07"), "{codes:?}");
    assert!(codes.contains(&"OBS-H09"), "{codes:?}");
    assert!(codes.contains(&"OBS-B01"), "{codes:?}");
    assert!(codes.contains(&"OBS-B02"), "{codes:?}");
    assert!(!report.is_clean());

    let repair = repair_obs(
        &obs,
        &RepairOptions {
            set_interval: true,
            ..RepairOptions::default()
        },
    );
    let actions: Vec<_> = repair.actions.iter().map(|action| action.id).collect();
    assert!(actions.contains(&"A3"), "{actions:?}");
    assert!(actions.contains(&"A4"), "{actions:?}");
    assert!(actions.contains(&"A6"), "{actions:?}");
    let remaining = finding_codes(&repair.remaining);
    assert!(!remaining.contains(&"OBS-H07"), "{remaining:?}");
    assert!(!remaining.contains(&"OBS-H09"), "{remaining:?}");
    assert!(!remaining.contains(&"OBS-B01"), "{remaining:?}");
    assert!(!remaining.contains(&"OBS-B02"), "{remaining:?}");

    let second = repair_obs(&repair.repaired, &RepairOptions::default());
    assert!(second.actions.is_empty(), "{:?}", second.actions);
}

#[test]
fn obs_drop_empty_satellite_record_is_opt_in() {
    let headers = [header_line(
        "  2020    01    01    00    00    0.0000000     GPS",
        "TIME OF FIRST OBS",
    )];
    let body = "> 2020 01 01 00 00  0.0000000  0  1\nG01";
    let obs = RinexObs::parse(&obs_text(&headers, body)).expect("parse OBS");

    let report = lint_obs(&obs);
    assert!(finding_codes(&report).contains(&"OBS-B08"));

    let repair = repair_obs(
        &obs,
        &RepairOptions {
            drop_empty_records: true,
            ..RepairOptions::default()
        },
    );
    assert_eq!(
        repair
            .actions
            .iter()
            .map(|action| action.id)
            .collect::<Vec<_>>(),
        vec!["A7"]
    );
    assert!(!finding_codes(&repair.remaining).contains(&"OBS-B08"));
}

#[test]
fn obs_text_lint_decodes_crinex_before_linting() {
    let rnx_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/obs/ESBC00DNK_R_20201770000_01D_30S_MO_trim.rnx"
    );
    let crx_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/obs/ESBC00DNK_R_20201770000_01D_30S_MO_trim.crx"
    );
    let rnx = std::fs::read_to_string(rnx_path).expect("read RINEX fixture");
    let crx = std::fs::read_to_string(crx_path).expect("read CRINEX fixture");

    let rnx_report = lint_obs_text(&rnx);
    let crx_report = lint_obs_text(&crx);
    assert!(!rnx_report.decoded_from_crinex);
    assert!(crx_report.decoded_from_crinex);
    assert_eq!(finding_codes(&crx_report), finding_codes(&rnx_report));
}

#[test]
fn nav_lint_and_repair_identical_duplicates_and_order() {
    let records = parse_nav(&nav_fixture()).expect("parse NAV fixture");
    assert!(records.len() >= 2);
    let mut damaged = vec![records[1], records[0], records[0]];

    let report = LintReport {
        findings: nav_findings(&damaged),
        decoded_from_crinex: false,
    };
    let codes = finding_codes(&report);
    assert!(codes.contains(&"NAV-B02"), "{codes:?}");
    assert!(codes.contains(&"NAV-B03"), "{codes:?}");

    let repair = repair_nav(&damaged, &RepairOptions::default());
    let actions: Vec<_> = repair.actions.iter().map(|action| action.id).collect();
    assert!(actions.contains(&"A11"), "{actions:?}");
    assert!(actions.contains(&"A12"), "{actions:?}");
    damaged.sort_by_key(nav_sort_key);
    assert_eq!(repair.records.len(), 2);
    assert!(!finding_codes(&repair.remaining).contains(&"NAV-B02"));
    assert!(!finding_codes(&repair.remaining).contains(&"NAV-B03"));
}

#[test]
fn nav_text_lint_reports_header_findings_without_file_io() {
    let text = nav_fixture().replace(
        "    18                                                      LEAP SECONDS",
        "",
    );
    let report = lint_nav_text(&text);
    assert!(finding_codes(&report).contains(&"NAV-H02"));
}
