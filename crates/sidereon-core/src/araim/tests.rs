//! ARAIM Tier-1 tests from the public MHSS equations: Gaussian tail inversion,
//! weight-zeroed gain matrices, closed-form fault-free protection levels, and
//! prior-driven fault-mode enumeration.

use super::fault_modes::enumerate_fault_modes;
use super::ism::{ConstellationIsm, Ism, SatelliteIsm, SatelliteIsmModel};
use super::protection::gain_matrix_enu;
use super::{araim, AraimGeometry, AraimRow, IntegrityAllocation};
use crate::astro::math::special::normal_q_inv;
use crate::dop::{ecef_to_enu_rotation, LineOfSight};
use crate::frame::Wgs84Geodetic;
use crate::id::{GnssSatelliteId, GnssSystem};
use crate::quality::{raim_fde_design, RangeFdeOptions, RangeFdeRow};

const INV_SQRT_3: f64 = 0.577_350_269_189_625_8;

#[test]
fn fault_free_only_reduces_to_closed_form_pl() {
    let geometry = gps_geometry();
    let ism = Ism::new(
        vec![ConstellationIsm::new(
            GnssSystem::Gps,
            0.0,
            SatelliteIsmModel::new(2.0, 1.0, 0.25, 0.0),
        )],
        Vec::new(),
    );
    let allocation = IntegrityAllocation {
        phmi_total: 1.0e-7,
        phmi_vert: 1.0e-7,
        phmi_hor: 1.0e-7,
        pfa_vert: 4.0e-6,
        pfa_hor: 4.0e-6,
        p_threshold_unmonitored: 0.0,
        max_fault_order: 0,
    };

    let result = araim(&geometry, &ism, &allocation).expect("fault-free ARAIM");
    let fault_free = &result.fault_modes[0];
    let k = normal_q_inv(allocation.phmi_vert * 0.5).expect("valid PHMI");
    let expected_vpl = k * fault_free.sigma_int_enu_m[2] + fault_free.bias_enu_m[2];
    assert_abs_diff(result.vpl_m, expected_vpl, 1.0e-9);

    let k_h = normal_q_inv(allocation.phmi_hor * 0.5).expect("valid PHMI");
    let expected_e = k_h * fault_free.sigma_int_enu_m[0] + fault_free.bias_enu_m[0];
    let expected_n = k_h * fault_free.sigma_int_enu_m[1] + fault_free.bias_enu_m[1];
    let expected_hpl = (expected_e * expected_e + expected_n * expected_n).sqrt();
    assert_abs_diff(result.hpl_m, expected_hpl, 1.0e-9);
    assert_eq!(result.emt_m, 0.0);
    assert!(result.availability);
}

#[test]
fn weight_zeroed_gain_matches_row_deleted_wls() {
    let geometry = gps_geometry_with_extra_row();
    let weights = vec![0.0, 1.0, 1.0, 1.0, 1.0];
    let gain_zeroed = gain_matrix_enu(&geometry, &weights).expect("zeroed gain");

    let deleted_geometry = AraimGeometry {
        rows: geometry.rows[1..].to_vec(),
        receiver: geometry.receiver,
        clock_systems: geometry.clock_systems.clone(),
    };
    let deleted_weights = vec![1.0; deleted_geometry.rows.len()];
    let gain_deleted = gain_matrix_enu(&deleted_geometry, &deleted_weights).expect("deleted gain");

    for coord in 0..3 {
        assert_eq!(gain_zeroed.enu_rows[coord][0], 0.0);
        for idx in 1..geometry.rows.len() {
            assert_abs_diff(
                gain_zeroed.enu_rows[coord][idx],
                gain_deleted.enu_rows[coord][idx - 1],
                2.0e-12,
            );
        }
    }

    let residuals = [0.0, 0.4, -0.2, 0.7, -0.1];
    let enu_from_gain = [
        dot(&gain_zeroed.enu_rows[0], &residuals),
        dot(&gain_zeroed.enu_rows[1], &residuals),
        dot(&gain_zeroed.enu_rows[2], &residuals),
    ];
    let fde_rows: Vec<RangeFdeRow> = geometry.rows[1..]
        .iter()
        .zip(residuals[1..].iter())
        .map(|(row, &residual_m)| RangeFdeRow {
            id: row.id.to_string(),
            residual_m,
            design_row: vec![
                -row.line_of_sight.e_x,
                -row.line_of_sight.e_y,
                -row.line_of_sight.e_z,
                1.0,
            ],
            weight: 1.0,
        })
        .collect();
    let fit = raim_fde_design(
        &fde_rows,
        &RangeFdeOptions {
            max_exclusions: 0,
            ..RangeFdeOptions::default()
        },
    )
    .expect("row-deleted WLS");
    let r = ecef_to_enu_rotation(geometry.receiver.lat_rad, geometry.receiver.lon_rad);
    let dx = &fit.state_correction;
    let enu_from_wls = [
        r[0][0] * dx[0] + r[0][1] * dx[1] + r[0][2] * dx[2],
        r[1][0] * dx[0] + r[1][1] * dx[1] + r[1][2] * dx[2],
        r[2][0] * dx[0] + r[2][1] * dx[1] + r[2][2] * dx[2],
    ];
    for coord in 0..3 {
        assert_abs_diff(enu_from_gain[coord], enu_from_wls[coord], 2.0e-12);
    }
}

#[test]
fn fault_modes_are_prior_ordered_and_pinned() {
    let geometry = mixed_geometry_for_enumeration();
    let ism = Ism::new(
        vec![
            ConstellationIsm::new(
                GnssSystem::Gps,
                1.0e-3,
                SatelliteIsmModel::new(2.0, 1.0, 0.0, 0.01),
            ),
            ConstellationIsm::new(
                GnssSystem::Galileo,
                2.0e-3,
                SatelliteIsmModel::new(2.0, 1.0, 0.0, 0.03),
            ),
        ],
        vec![SatelliteIsm::new(
            sat(GnssSystem::Gps, 2),
            2.0,
            1.0,
            0.0,
            0.02,
        )],
    );
    let allocation = IntegrityAllocation {
        phmi_total: 1.0e-7,
        phmi_vert: 1.0e-7,
        phmi_hor: 1.0e-7,
        pfa_vert: 4.0e-6,
        pfa_hor: 4.0e-6,
        p_threshold_unmonitored: 0.0,
        max_fault_order: 2,
    };

    let modes = enumerate_fault_modes(&geometry, &ism, &allocation);
    assert_eq!(modes.len(), 9);
    assert_eq!(modes[0].excluded, Vec::<GnssSatelliteId>::new());
    assert_eq!(modes[1].excluded, vec![sat(GnssSystem::Gps, 1)]);
    assert_eq!(modes[2].excluded, vec![sat(GnssSystem::Gps, 2)]);
    assert_eq!(modes[3].excluded, vec![sat(GnssSystem::Galileo, 1)]);
    assert_eq!(modes[4].excluded_constellation, Some(GnssSystem::Gps));
    assert_eq!(modes[5].excluded_constellation, Some(GnssSystem::Galileo));
    assert_eq!(
        modes[6].excluded,
        vec![sat(GnssSystem::Gps, 2), sat(GnssSystem::Galileo, 1)]
    );
    assert_eq!(
        modes[7].excluded,
        vec![sat(GnssSystem::Gps, 1), sat(GnssSystem::Galileo, 1)]
    );
    assert_eq!(
        modes[8].excluded,
        vec![sat(GnssSystem::Gps, 1), sat(GnssSystem::Gps, 2)]
    );
    assert_abs_diff(modes[6].prior, 6.0e-4, 0.0);
    assert_abs_diff(modes[7].prior, 3.0e-4, 0.0);
    assert_abs_diff(modes[8].prior, 2.0e-4, 0.0);
}

fn gps_geometry() -> AraimGeometry {
    AraimGeometry {
        rows: vec![
            row(GnssSystem::Gps, 1, [INV_SQRT_3, INV_SQRT_3, INV_SQRT_3]),
            row(GnssSystem::Gps, 2, [INV_SQRT_3, -INV_SQRT_3, -INV_SQRT_3]),
            row(GnssSystem::Gps, 3, [-INV_SQRT_3, INV_SQRT_3, -INV_SQRT_3]),
            row(GnssSystem::Gps, 4, [-INV_SQRT_3, -INV_SQRT_3, INV_SQRT_3]),
        ],
        receiver: receiver(),
        clock_systems: vec![GnssSystem::Gps],
    }
}

fn gps_geometry_with_extra_row() -> AraimGeometry {
    let mut geometry = gps_geometry();
    geometry
        .rows
        .push(row(GnssSystem::Gps, 5, [0.2, 0.6, 0.774_596_669_241_483_4]));
    geometry
}

fn mixed_geometry_for_enumeration() -> AraimGeometry {
    AraimGeometry {
        rows: vec![
            row(GnssSystem::Gps, 1, [INV_SQRT_3, INV_SQRT_3, INV_SQRT_3]),
            row(GnssSystem::Gps, 2, [INV_SQRT_3, -INV_SQRT_3, -INV_SQRT_3]),
            row(
                GnssSystem::Galileo,
                1,
                [-INV_SQRT_3, INV_SQRT_3, -INV_SQRT_3],
            ),
        ],
        receiver: receiver(),
        clock_systems: vec![GnssSystem::Gps, GnssSystem::Galileo],
    }
}

fn row(system: GnssSystem, prn: u8, los: [f64; 3]) -> AraimRow {
    AraimRow {
        id: sat(system, prn),
        line_of_sight: LineOfSight::new(los[0], los[1], los[2]),
        system,
        elevation_rad: core::f64::consts::FRAC_PI_2,
    }
}

fn sat(system: GnssSystem, prn: u8) -> GnssSatelliteId {
    GnssSatelliteId::new(system, prn).expect("valid satellite")
}

fn receiver() -> Wgs84Geodetic {
    Wgs84Geodetic::new(0.0, 0.0, 0.0).expect("valid receiver")
}

fn dot(row: &[f64], residuals: &[f64]) -> f64 {
    row.iter().zip(residuals).map(|(a, b)| a * b).sum()
}

fn assert_abs_diff(left: f64, right: f64, tol: f64) {
    let diff = (left - right).abs();
    assert!(
        diff <= tol,
        "left={left} right={right} diff={diff} tol={tol}"
    );
}
