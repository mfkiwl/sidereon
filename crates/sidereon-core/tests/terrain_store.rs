//! Terrain store fixture provenance: DTED tiles under
//! `tests/fixtures/dted/tiles` and points in `tests/fixtures/dted/dted_points.json`
//! are existing repository fixtures generated from the public DTED
//! UHL/DSI/ACC/data-record layout. The HGT void test uses the synthetic
//! `tests/fixtures/dted/hgt/n36_w107_reference.hgt` fixture already committed
//! for the SRTM1-to-DTED converter. Tests compare terrain heights by
//! `f64::to_bits()` against the crate's DTED reader over the same files.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use sidereon_core::data::hgt_to_dted;
use sidereon_core::geoid::egm96_undulation;
use sidereon_core::terrain::{DtedInterpolation, DtedLookupOptions, DtedTerrain};
use sidereon_core::terrain_store::{
    dted_tree_to_mmap_store, terrain_store_checksum64, Egm96FifteenMinuteGeoid, MmapTerrain,
    OrthometricHeightM, TerrainDatumError, TerrainGeoidModel, VerticalDatum,
};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("dted")
        .join(name)
}

fn temp_path(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{name}-{}-{nonce}", std::process::id()))
}

fn f64_from_hex(input: &str) -> f64 {
    let trimmed = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"))
        .expect("hex string has 0x prefix");
    let bits = u64::from_str_radix(trimmed, 16).expect("valid f64 bits");
    f64::from_bits(bits)
}

fn multi_tile_points() -> Vec<(f64, f64)> {
    let json: Value =
        serde_json::from_slice(&fs::read(fixture_path("dted_points.json")).expect("fixture json"))
            .expect("parse fixture json");
    let mut points: Vec<(f64, f64)> = json["multi_tile_cases"]
        .as_array()
        .expect("multi tile cases")
        .iter()
        .map(|case| {
            (
                f64_from_hex(case["longitude_bits"].as_str().expect("longitude bits")),
                f64_from_hex(case["latitude_bits"].as_str().expect("latitude bits")),
            )
        })
        .collect();
    points.push((-104.5, 36.5));
    points
}

fn assert_height_results_match(
    got: &[sidereon_core::Result<f64>],
    want: &[sidereon_core::Result<f64>],
    context: &str,
) {
    assert_eq!(got.len(), want.len(), "{context} result length");
    for (idx, (got, want)) in got.iter().zip(want).enumerate() {
        match (got, want) {
            (Ok(got), Ok(want)) => assert_eq!(
                got.to_bits(),
                want.to_bits(),
                "{context} index {idx} height bits"
            ),
            (Err(got), Err(want)) => assert_eq!(got, want, "{context} index {idx} error"),
            (got, want) => panic!("{context} index {idx} mismatch: {got:?} != {want:?}"),
        }
    }
}

#[test]
fn mmap_store_matches_dted_reader_over_multi_tile_fixture() {
    let root = fixture_path("tiles");
    let bytes = dted_tree_to_mmap_store(&root).expect("convert DTED tree");
    let mut mmap = MmapTerrain::from_bytes(&bytes).expect("parse terrain store");
    let mut dted = DtedTerrain::new(&root);
    let points = multi_tile_points();

    assert_eq!(mmap.vertical_datum(), VerticalDatum::Egm96MslOrthometric);
    assert_eq!(mmap.tile_index().len(), 2);
    for tile in mmap.tile_index() {
        assert_eq!(tile.vertical_datum, VerticalDatum::Egm96MslOrthometric);
        assert_eq!(tile.data_offset as usize % 4096, 0);
    }

    for interpolation in [
        DtedInterpolation::Bilinear,
        DtedInterpolation::NearestPosting,
    ] {
        let options = DtedLookupOptions { interpolation };
        let got = mmap.height_batch(&points, options);
        let want = dted.height_batch(&points, options);
        assert_height_results_match(&got, &want, &format!("{interpolation:?} batch"));

        for &(longitude_deg, latitude_deg) in &points {
            let got = mmap
                .height_m_with_options(longitude_deg, latitude_deg, options)
                .expect("mmap scalar height");
            let want = DtedTerrain::new(&root)
                .height_m_with_options(longitude_deg, latitude_deg, options)
                .expect("DTED scalar height");
            assert_eq!(got.to_bits(), want.to_bits());

            let typed = mmap
                .orthometric_height_m_with_options(longitude_deg, latitude_deg, options)
                .expect("typed orthometric height");
            assert_eq!(typed.metres().to_bits(), want.to_bits());
        }
    }
}

#[test]
fn mmap_store_matches_dted_reader_for_hgt_void_collapsed_to_zero() {
    let hgt = fs::read(fixture_path("hgt/n36_w107_reference.hgt")).expect("read HGT fixture");
    let dt2 = hgt_to_dted(36, -107, &hgt).expect("convert HGT fixture");
    let root = temp_path("terrain-store-hgt-void");
    fs::create_dir_all(&root).expect("create temp DTED root");
    fs::write(root.join("n36_w107_1arc_v3.dt2"), dt2).expect("write converted DTED tile");

    let bytes = dted_tree_to_mmap_store(&root).expect("convert DTED tree");
    let mut mmap = MmapTerrain::from_bytes(&bytes).expect("parse terrain store");
    let mut dted = DtedTerrain::new(&root);
    let options = DtedLookupOptions {
        interpolation: DtedInterpolation::NearestPosting,
    };
    let latitude_deg = 36.0 + 1234.0 / 3600.0;
    let longitude_deg = -107.0 + 2345.0 / 3600.0;

    let got = mmap
        .height_m_with_options(longitude_deg, latitude_deg, options)
        .expect("mmap void height");
    let want = dted
        .height_m_with_options(longitude_deg, latitude_deg, options)
        .expect("DTED void height");
    assert_eq!(got.to_bits(), want.to_bits());
    assert_eq!(got.to_bits(), 0.0f64.to_bits());

    fs::remove_dir_all(root).expect("remove temp DTED root");
}

#[test]
fn dted_tree_conversion_is_byte_stable() {
    let root = fixture_path("tiles");
    let first = dted_tree_to_mmap_store(&root).expect("first conversion");
    let second = dted_tree_to_mmap_store(&root).expect("second conversion");
    assert_eq!(first, second);
    assert_eq!(
        terrain_store_checksum64(&first),
        terrain_store_checksum64(&second)
    );

    let parsed = MmapTerrain::from_bytes(&first).expect("parse store");
    let reserialized = parsed.to_bytes();
    assert_eq!(
        terrain_store_checksum64(&first),
        terrain_store_checksum64(&reserialized)
    );
    assert_eq!(first, reserialized);
}

#[test]
fn orthometric_to_ellipsoidal_uses_pinned_egm96_one_degree_grid() {
    let orthometric = OrthometricHeightM::new(123.5);
    let latitude_deg = 37.0;
    let longitude_deg = -122.0;
    let got = orthometric
        .to_ellipsoidal_height_deg(
            latitude_deg,
            longitude_deg,
            TerrainGeoidModel::Egm96OneDegree,
        )
        .expect("convert terrain height");
    let expected = orthometric.metres()
        + egm96_undulation(latitude_deg.to_radians(), longitude_deg.to_radians());
    assert_eq!(got.metres().to_bits(), expected.to_bits());
}

#[test]
fn missing_egm96_fifteen_minute_grid_returns_typed_error() {
    let root = temp_path("missing-egm96-dac");
    fs::create_dir_all(&root).expect("create temp dir");
    let missing_path = root.join("WW15MGH.DAC");
    let err = Egm96FifteenMinuteGeoid::from_ww15mgh_dac_path(&missing_path)
        .expect_err("missing DAC must error");
    match err {
        TerrainDatumError::MissingEgm96Dac { path, remediation } => {
            assert_eq!(path, missing_path);
            assert!(remediation.contains("WW15MGH.DAC"));
            assert!(remediation.contains("from_ww15mgh_dac_bytes"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
    fs::remove_dir_all(root).expect("remove temp dir");
}

#[test]
fn store_file_round_trips_through_path_reader() {
    let root = fixture_path("tiles");
    let bytes = dted_tree_to_mmap_store(&root).expect("convert DTED tree");
    let store_path = temp_path("terrain-store-file").with_extension("bin");
    fs::write(&store_path, &bytes).expect("write terrain store");

    let mmap = MmapTerrain::from_path(&store_path).expect("read terrain store");
    assert_eq!(mmap.as_bytes(), bytes.as_slice());
    assert_eq!(mmap.to_bytes(), bytes);

    fs::remove_file(store_path).expect("remove temp store");
}
