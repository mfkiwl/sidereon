use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_sidereon")
}

fn fixture(parts: &[&str]) -> PathBuf {
    let mut path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../sidereon-core/tests/fixtures");
    for part in parts {
        path.push(part);
    }
    path
}

fn run(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("run sidereon binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout utf8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr utf8")
}

#[test]
fn metrics_prints_expected_bounds() {
    let output = run(&["metrics", "--enu-cov", "4,0,0,0,9,0,0,0,16"]);
    assert!(
        output.status.success(),
        "status {:?}\nstderr:\n{}",
        output.status.code(),
        stderr(&output)
    );
    let stdout = stdout(&output);
    assert!(stdout.contains("CEP"));
    assert!(stdout.contains("R95"));
    assert!(stdout.contains("V(0.950)"));
}

#[test]
fn inspect_observation_fixture_reports_structure() {
    let obs = fixture(&["obs", "ESBC00DNK_R_20201770000_01D_30S_MO_trim.rnx"]);
    let output = run(&["inspect", obs.to_str().expect("fixture path utf8")]);
    assert!(
        output.status.success(),
        "status {:?}\nstderr:\n{}",
        output.status.code(),
        stderr(&output)
    );
    let stdout = stdout(&output);
    assert!(stdout.contains("type: RINEX OBS"));
    assert!(stdout.contains("epochs: 2"));
    assert!(stdout.contains("satellites:"));
    assert!(stdout.contains("G05"));
}

#[test]
fn qc_json_includes_lint_counts_and_qc_report() {
    let obs = fixture(&["obs", "ESBC00DNK_R_20201770000_01D_30S_MO_120epoch.rnx"]);
    let output = run(&[
        "qc",
        "--obs",
        obs.to_str().expect("fixture path utf8"),
        "--json",
    ]);
    assert!(
        output.status.success(),
        "status {:?}\nstderr:\n{}",
        output.status.code(),
        stderr(&output)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("qc JSON");
    assert!(json["lint"]["counts"]["fatal"].as_u64().is_some());
    assert!(
        json["qc"]["total_epoch_records"]
            .as_u64()
            .expect("total epochs")
            >= 2
    );
}

#[test]
fn solve_json_reports_successful_epochs_and_metrics() {
    let obs = fixture(&["obs", "ESBC00DNK_R_20201770000_01D_30S_MO_trim.rnx"]);
    let nav = fixture(&["nav", "ESBC00DNK_R_20201770000_01D_MN.rnx"]);
    let sp3 = fixture(&["sp3", "COD0MGXFIN_20201770000_01D_05M_ORB.SP3"]);
    let output = run(&[
        "solve",
        "--obs",
        obs.to_str().expect("fixture path utf8"),
        "--nav",
        nav.to_str().expect("fixture path utf8"),
        "--sp3",
        sp3.to_str().expect("fixture path utf8"),
        "--json",
    ]);
    assert!(
        output.status.success(),
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout(&output),
        stderr(&output)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("solve JSON");
    assert_eq!(json["summary"]["assembled_epochs"].as_u64(), Some(2));
    assert!(json["summary"]["solved_count"].as_u64().expect("solved") >= 1);

    let first = json["epochs"]
        .as_array()
        .expect("epochs array")
        .iter()
        .find(|epoch| epoch["solved"].as_bool() == Some(true))
        .expect("successful epoch");
    let lat = first["lat_deg"].as_f64().expect("lat");
    let lon = first["lon_deg"].as_f64().expect("lon");
    let height = first["height_m"].as_f64().expect("height");
    let cep = first["metrics"]["cep_m"].as_f64().expect("CEP");
    assert!((50.0..60.0).contains(&lat), "lat {lat}");
    assert!((5.0..15.0).contains(&lon), "lon {lon}");
    assert!((-100.0..500.0).contains(&height), "height {height}");
    assert!(cep.is_finite() && cep >= 0.0, "CEP {cep}");
}
