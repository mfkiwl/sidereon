use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use sidereon_core::data::{dted_cache_relpath, hgt_to_dted, HgtConversionError};
use sidereon_core::terrain::{DtedInterpolation, DtedLookupOptions, DtedTerrain};

const POSTINGS: usize = 3601;
const HGT_LEN: usize = POSTINGS * POSTINGS * 2;
const DTED_LEN: usize = 25_981_042;
const LAT_INDEX: i32 = 36;
const LON_INDEX: i32 = -107;

// Fixture provenance: the HGT payload in these tests is generated in memory
// from the closed-form `synthetic_hgt_sample` function below. No external
// terrain payload is copied. Selected postings pin positive, negative, minimum
// negative, endpoint, and void cases through the public SRTM1 conversion path.

fn temp_root(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{name}-{}-{nonce}", std::process::id()))
}

fn synthetic_hgt_sample(row: usize, col: usize) -> i16 {
    match (row, col) {
        (2366, 2345) => i16::MIN,
        (3500, 200) => -415,
        (1600, 3000) => -1,
        (0, 3600) => 8848,
        _ => (((row as i32 * 37 + col as i32 * 19) % 5000) - 1000) as i16,
    }
}

fn expected_posting(lat_posting: usize, lon_posting: usize) -> i16 {
    let sample = synthetic_hgt_sample(POSTINGS - 1 - lat_posting, lon_posting);
    if sample == i16::MIN {
        0
    } else {
        sample
    }
}

fn generated_hgt() -> Vec<u8> {
    let mut hgt = vec![0u8; HGT_LEN];
    for row in 0..POSTINGS {
        for col in 0..POSTINGS {
            let start = 2 * (row * POSTINGS + col);
            hgt[start..start + 2].copy_from_slice(&synthetic_hgt_sample(row, col).to_be_bytes());
        }
    }
    hgt
}

#[test]
fn hgt_to_dted_round_trips_selected_postings_through_reader() {
    let hgt = generated_hgt();
    let dt2 = hgt_to_dted(LAT_INDEX, LON_INDEX, &hgt).expect("convert HGT to DTED");
    assert_eq!(dt2.len(), DTED_LEN);

    let dt2_again = hgt_to_dted(LAT_INDEX, LON_INDEX, &hgt).expect("convert HGT to DTED again");
    assert!(
        dt2 == dt2_again,
        "same HGT input and tile indices must produce identical DTED bytes"
    );

    let root = temp_root("hgt-to-dted-roundtrip");
    let relpath = dted_cache_relpath(LAT_INDEX, LON_INDEX).expect("DTED cache path");
    let tile_path = root.join(relpath);
    fs::create_dir_all(tile_path.parent().expect("tile parent")).expect("create DTED block dir");
    fs::write(&tile_path, &dt2).expect("write converted DTED tile");

    let mut terrain = DtedTerrain::new(&root);
    let nearest = DtedLookupOptions {
        interpolation: DtedInterpolation::NearestPosting,
    };

    for (lat_posting, lon_posting) in [(0, 0), (100, 200), (1234, 2345), (2000, 3000), (3600, 3600)]
    {
        let lat = f64::from(LAT_INDEX) + lat_posting as f64 / 3600.0;
        let lon = f64::from(LON_INDEX) + lon_posting as f64 / 3600.0;
        let got = terrain
            .height_m_with_options(lon, lat, nearest)
            .expect("read converted DTED posting");
        let expected = f64::from(expected_posting(lat_posting, lon_posting));
        assert_eq!(
            got, expected,
            "posting lat_index={lat_posting} lon_index={lon_posting}"
        );
    }

    fs::remove_dir_all(root).expect("remove temp DTED root");
}

#[test]
fn hgt_to_dted_rejects_bad_length_and_invalid_tile_index() {
    assert_eq!(
        hgt_to_dted(LAT_INDEX, LON_INDEX, &[]),
        Err(HgtConversionError::BadLength {
            expected: HGT_LEN,
            got: 0
        })
    );
    assert_eq!(
        hgt_to_dted(90, LON_INDEX, &[]),
        Err(HgtConversionError::InvalidTileIndex {
            lat_index: 90,
            lon_index: LON_INDEX
        })
    );
}

#[test]
fn missing_dted_tile_reads_as_sea_level() {
    let root = temp_root("dted-empty-root");
    let mut terrain = DtedTerrain::new(&root);
    assert_eq!(
        terrain
            .height_m(f64::from(LON_INDEX) + 0.5, f64::from(LAT_INDEX) + 0.5)
            .expect("missing tile uses terrain sea-level fallback"),
        0.0
    );
}
