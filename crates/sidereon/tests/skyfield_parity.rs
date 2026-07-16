//! Independent Skyfield 1.49 frame-transform oracle.
//!
//! These vectors were captured from Skyfield 1.49 with Python `float.hex()`.
//! They are deliberately not emitted by Sidereon: this test must remain an
//! external numerical gate for the public Rust interface.

use sidereon::astro::frames::transforms::{
    gcrs_to_itrs_compute, teme_to_gcrs_compute, TemeStateKm,
};
use sidereon::astro::time::TimeScales;

const TEME_POSITION_KM: [f64; 3] = [
    f64::from_bits(0x40ac_e86c_23df_fb6b),
    f64::from_bits(0x409f_7fa6_1c81_cb47),
    f64::from_bits(0x40b4_bd83_5915_9cde),
];
const TEME_VELOCITY_KM_S: [f64; 3] = [
    f64::from_bits(0xc00b_2ffb_7cf9_ad7d),
    f64::from_bits(0x401b_7a87_51f7_fc4a),
    f64::from_bits(0xbfce_b369_25f0_7cb4),
];
const GCRS_POSITION_KM: [f64; 3] = [
    f64::from_bits(0x40ad_0bd9_1937_13e1),
    f64::from_bits(0x409f_41a3_b207_3733),
    f64::from_bits(0x40b4_b6ff_ad12_89d1),
];
const GCRS_VELOCITY_KM_S: [f64; 3] = [
    f64::from_bits(0xc00a_f690_723d_6cb1),
    f64::from_bits(0x401b_88e0_6212_f969),
    f64::from_bits(0xbfcd_e857_5471_eaf0),
];
const ITRS_POSITION_KM: [f64; 3] = [
    f64::from_bits(0xc092_d5d3_2b31_9db8),
    f64::from_bits(0x40af_8b3b_3a72_2474),
    f64::from_bits(0x40b4_bd83_5915_9cdb),
];

fn assert_bits_eq(label: &str, actual: [f64; 3], expected: [f64; 3]) {
    for axis in 0..3 {
        assert_eq!(
            actual[axis].to_bits(),
            expected[axis].to_bits(),
            "{label}[{axis}]: actual={:#018x} expected={:#018x}",
            actual[axis].to_bits(),
            expected[axis].to_bits()
        );
    }
}

fn reference_epoch() -> TimeScales {
    TimeScales::from_utc(2018, 7, 4, 0, 0, 0.0).expect("valid reference epoch")
}

#[test]
fn teme_to_gcrs_matches_skyfield_1_49_at_zero_ulp() {
    let state = TemeStateKm {
        position_km: TEME_POSITION_KM,
        velocity_km_s: TEME_VELOCITY_KM_S,
    };
    let (position, velocity) =
        teme_to_gcrs_compute(&state, &reference_epoch(), true).expect("valid transform");

    assert_bits_eq(
        "position",
        [position.0, position.1, position.2],
        GCRS_POSITION_KM,
    );
    assert_bits_eq(
        "velocity",
        [velocity.0, velocity.1, velocity.2],
        GCRS_VELOCITY_KM_S,
    );
}

#[test]
fn gcrs_to_itrs_matches_skyfield_1_49_at_zero_ulp() {
    let (x, y, z) = gcrs_to_itrs_compute(
        GCRS_POSITION_KM[0],
        GCRS_POSITION_KM[1],
        GCRS_POSITION_KM[2],
        &reference_epoch(),
        true,
    )
    .expect("valid transform");

    assert_bits_eq("position", [x, y, z], ITRS_POSITION_KM);
}
