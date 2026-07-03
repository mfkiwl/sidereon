//! ARAIM Tier-1 tests from the public MHSS equations: Gaussian tail inversion,
//! weight-zeroed gain matrices, closed-form fault-free protection levels, and
//! prior-driven fault-mode enumeration.
//! Tier-2 checks cite Blanch et al. 2015 and WG-C Milestone 3. The WG-C material
//! pins the LPV-200 allocation, ISM parameter families, fault-mode enumeration,
//! and protection-level equations, but it does not provide single-geometry
//! HPL/VPL table values for the synthetic geometries below. The tests therefore
//! pin this implementation's fixed GPS-only and GPS+Galileo HPL/VPL values,
//! monitored fault-mode sets, closed-form fault-free PL, a hand-worked small
//! S-matrix, and fault-mass conservation.

use super::fault_modes::enumerate_fault_modes;
use super::ism::{ConstellationIsm, Ism, SatelliteIsm, SatelliteIsmModel};
use super::protection::{gain_matrix_enu, metric_bias, metric_sigma};
use super::{araim, AraimGeometry, AraimRow, IntegrityAllocation};
use crate::astro::math::least_squares::Status;
use crate::astro::math::special::normal_q_inv;
use crate::dop::{ecef_to_enu_rotation, line_of_sight_from_az_el_deg, LineOfSight};
use crate::frame::{ItrfPositionM, Wgs84Geodetic};
use crate::id::{GnssSatelliteId, GnssSystem};
use crate::quality::{raim_fde_design, RangeFdeOptions, RangeFdeRow};
use crate::spp::{EphemerisSource, ReceiverSolution, SolutionMetadata};

const INV_SQRT_3: f64 = 0.577_350_269_189_625_8;
const WG_C_SIGMA_URA_M: f64 = 0.75;
const WG_C_SIGMA_URE_M: f64 = 0.5;
const WG_C_B_NOM_M: f64 = 0.75;
const WG_C_P_SAT: f64 = 1.0e-5;
const WG_C_P_CONST_GPS: f64 = 0.0;
const WG_C_P_CONST_GAL: f64 = 1.0e-4;

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

#[test]
fn constellation_fault_modes_drop_excluded_clock_and_stay_monitorable() {
    let geometry = reference_gps_galileo_geometry();
    let ism = reference_gps_galileo_ism();
    let allocation = IntegrityAllocation::lpv_200();

    let result = araim(&geometry, &ism, &allocation).expect("GPS plus Galileo ARAIM");

    assert!(result.availability);
    assert!(result.hpl_m.is_finite());
    assert!(result.vpl_m.is_finite());
    assert!(result.p_unmonitored <= allocation.p_threshold_unmonitored);
    let galileo_constellation = result
        .fault_modes
        .iter()
        .find(|mode| mode.excluded_constellation == Some(GnssSystem::Galileo))
        .expect("Galileo constellation mode");
    assert!(galileo_constellation.monitorable);
}

#[test]
fn lpv_200_reference_style_scenarios_are_pinned() {
    let allocation = IntegrityAllocation::lpv_200();
    assert_abs_diff(allocation.phmi_total, 1.0e-7, 0.0);
    assert_abs_diff(allocation.phmi_vert, 1.0e-7, 0.0);
    assert_abs_diff(allocation.phmi_hor, 1.0e-7, 0.0);
    assert_abs_diff(allocation.pfa_vert, 4.0e-6, 0.0);
    assert_abs_diff(allocation.pfa_hor, 4.0e-6, 0.0);

    let gps_result = araim(&reference_gps_geometry(), &reference_gps_ism(), &allocation)
        .expect("GPS-only LPV-200 ARAIM");
    assert!(gps_result.availability);
    assert_eq!(
        monitored_mode_keys(&gps_result),
        (1..=6)
            .map(|prn| sat(GnssSystem::Gps, prn).to_string())
            .collect::<Vec<_>>()
    );
    assert_abs_diff(gps_result.hpl_m, 12.683_570_299_587_462, 1.0e-9);
    assert_abs_diff(gps_result.vpl_m, 31.458_077_322_350_43, 1.0e-9);

    let mixed_result = araim(
        &reference_gps_galileo_geometry(),
        &reference_gps_galileo_ism(),
        &allocation,
    )
    .expect("GPS plus Galileo LPV-200 ARAIM");
    assert!(mixed_result.availability);
    let mut expected_mixed = (1..=6)
        .map(|prn| sat(GnssSystem::Gps, prn).to_string())
        .collect::<Vec<_>>();
    expected_mixed.extend((1..=5).map(|prn| sat(GnssSystem::Galileo, prn).to_string()));
    expected_mixed.push("Galileo*".to_string());
    assert_eq!(monitored_mode_keys(&mixed_result), expected_mixed);
    assert_abs_diff(mixed_result.hpl_m, 8.837_009_847_530_268, 1.0e-9);
    assert_abs_diff(mixed_result.vpl_m, 21.443_928_549_396_453, 1.0e-9);
}

#[test]
fn hand_worked_s_matrix_and_fault_free_pl_are_pinned() {
    let geometry = gps_geometry();
    let weights = vec![1.0; geometry.rows.len()];
    let gain = gain_matrix_enu(&geometry, &weights).expect("gain matrix");
    let s = 0.75 * INV_SQRT_3;
    let expected = [vec![-s, s, -s, s], vec![-s, s, s, -s], vec![-s, -s, s, s]];
    for (coord, expected_row) in expected.iter().enumerate() {
        for (idx, &expected_value) in expected_row.iter().enumerate() {
            assert_abs_diff(gain.enu_rows[coord][idx], expected_value, 2.0e-15);
        }
    }

    let sigmas = vec![1.0; geometry.rows.len()];
    let biases = vec![0.25; geometry.rows.len()];
    let expected_sigma = 2.0 * s;
    let expected_bias = s;
    let expected_pl = normal_q_inv(0.5e-7).expect("valid PHMI") * expected_sigma + expected_bias;
    let sigma = metric_sigma(&gain.enu_rows[2], &sigmas);
    let bias = metric_bias(&gain.enu_rows[2], &biases);
    assert_abs_diff(sigma, expected_sigma, 2.0e-15);
    assert_abs_diff(bias, expected_bias, 2.0e-15);
    assert_abs_diff(
        normal_q_inv(0.5e-7).expect("valid PHMI") * sigma + bias,
        expected_pl,
        2.0e-12,
    );
}

#[test]
fn max_fault_order_one_counts_pair_and_higher_mass_unmonitored() {
    let geometry = gps_geometry_with_extra_row();
    let p_sat = 0.01;
    let ism = Ism::new(
        vec![ConstellationIsm::new(
            GnssSystem::Gps,
            0.0,
            SatelliteIsmModel::new(2.0, 1.0, 0.25, p_sat),
        )],
        Vec::new(),
    );
    let allocation = IntegrityAllocation {
        phmi_total: 1.0e-7,
        phmi_vert: 1.0e-7,
        phmi_hor: 1.0e-7,
        pfa_vert: 4.0e-6,
        pfa_hor: 4.0e-6,
        p_threshold_unmonitored: 0.01,
        max_fault_order: 1,
    };

    let result = araim(&geometry, &ism, &allocation).expect("ARAIM with unmonitored tail");
    let expected_pair_plus =
        10.0 * p_sat.powi(2) + 10.0 * p_sat.powi(3) + 5.0 * p_sat.powi(4) + p_sat.powi(5);
    assert_abs_diff(result.p_unmonitored, expected_pair_plus, 2.0e-16);
}

#[test]
fn receiver_solution_adapter_rebuilds_geometry() {
    let source_geometry = gps_geometry();
    let receiver_ecef = [6_378_137.0, 0.0, 0.0];
    let positions = source_geometry
        .rows
        .iter()
        .map(|row| {
            (
                row.id,
                [
                    receiver_ecef[0] + 20_200_000.0 * row.line_of_sight.e_x,
                    receiver_ecef[1] + 20_200_000.0 * row.line_of_sight.e_y,
                    receiver_ecef[2] + 20_200_000.0 * row.line_of_sight.e_z,
                ],
            )
        })
        .collect();
    let eph = StaticEphemeris { positions };
    let solution = receiver_solution(
        source_geometry.rows.iter().map(|row| row.id).collect(),
        vec![GnssSystem::Gps],
    );

    let rebuilt =
        AraimGeometry::from_receiver_solution(&solution, &eph).expect("rebuilt ARAIM geometry");

    assert_eq!(rebuilt.clock_systems, vec![GnssSystem::Gps]);
    assert_eq!(rebuilt.rows.len(), source_geometry.rows.len());
    for (rebuilt, source) in rebuilt.rows.iter().zip(source_geometry.rows.iter()) {
        assert_eq!(rebuilt.id, source.id);
        assert_eq!(rebuilt.system, source.system);
        assert_abs_diff(rebuilt.line_of_sight.e_x, source.line_of_sight.e_x, 2.0e-16);
        assert_abs_diff(rebuilt.line_of_sight.e_y, source.line_of_sight.e_y, 2.0e-16);
        assert_abs_diff(rebuilt.line_of_sight.e_z, source.line_of_sight.e_z, 2.0e-16);
        assert!(rebuilt.elevation_rad.is_finite());
    }
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

fn reference_gps_geometry() -> AraimGeometry {
    AraimGeometry {
        rows: vec![
            row_az_el(GnssSystem::Gps, 1, 0.0, 65.0),
            row_az_el(GnssSystem::Gps, 2, 60.0, 35.0),
            row_az_el(GnssSystem::Gps, 3, 120.0, 55.0),
            row_az_el(GnssSystem::Gps, 4, 190.0, 30.0),
            row_az_el(GnssSystem::Gps, 5, 250.0, 50.0),
            row_az_el(GnssSystem::Gps, 6, 310.0, 40.0),
        ],
        receiver: receiver(),
        clock_systems: vec![GnssSystem::Gps],
    }
}

fn reference_gps_galileo_geometry() -> AraimGeometry {
    let mut geometry = reference_gps_geometry();
    geometry.rows.extend([
        row_az_el(GnssSystem::Galileo, 1, 20.0, 55.0),
        row_az_el(GnssSystem::Galileo, 2, 95.0, 35.0),
        row_az_el(GnssSystem::Galileo, 3, 170.0, 60.0),
        row_az_el(GnssSystem::Galileo, 4, 245.0, 40.0),
        row_az_el(GnssSystem::Galileo, 5, 320.0, 50.0),
    ]);
    geometry.clock_systems = vec![GnssSystem::Gps, GnssSystem::Galileo];
    geometry
}

fn reference_gps_ism() -> Ism {
    Ism::new(
        vec![ConstellationIsm::new(
            GnssSystem::Gps,
            WG_C_P_CONST_GPS,
            reference_sat_model(),
        )],
        Vec::new(),
    )
}

fn reference_gps_galileo_ism() -> Ism {
    Ism::new(
        vec![
            ConstellationIsm::new(GnssSystem::Gps, WG_C_P_CONST_GPS, reference_sat_model()),
            ConstellationIsm::new(GnssSystem::Galileo, WG_C_P_CONST_GAL, reference_sat_model()),
        ],
        Vec::new(),
    )
}

const fn reference_sat_model() -> SatelliteIsmModel {
    SatelliteIsmModel::new(WG_C_SIGMA_URA_M, WG_C_SIGMA_URE_M, WG_C_B_NOM_M, WG_C_P_SAT)
}

fn row(system: GnssSystem, prn: u8, los: [f64; 3]) -> AraimRow {
    AraimRow {
        id: sat(system, prn),
        line_of_sight: LineOfSight::new(los[0], los[1], los[2]),
        system,
        elevation_rad: core::f64::consts::FRAC_PI_2,
    }
}

fn row_az_el(system: GnssSystem, prn: u8, az_deg: f64, el_deg: f64) -> AraimRow {
    AraimRow {
        id: sat(system, prn),
        line_of_sight: line_of_sight_from_az_el_deg(az_deg, el_deg, receiver())
            .expect("valid az/el"),
        system,
        elevation_rad: el_deg.to_radians(),
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

fn monitored_mode_keys(result: &super::AraimResult) -> Vec<String> {
    result
        .fault_modes
        .iter()
        .skip(1)
        .filter(|mode| mode.monitorable)
        .map(|mode| {
            if let Some(system) = mode.excluded_constellation {
                format!("{system:?}*")
            } else {
                mode.excluded
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("+")
            }
        })
        .collect()
}

fn receiver_solution(
    used_sats: Vec<GnssSatelliteId>,
    systems: Vec<GnssSystem>,
) -> ReceiverSolution {
    let used_count = used_sats.len();
    let redundancy = used_count as isize - (3 + systems.len()) as isize;
    ReceiverSolution {
        position: ItrfPositionM::new(6_378_137.0, 0.0, 0.0).expect("valid receiver"),
        geodetic: Some(receiver()),
        rx_clock_s: 0.0,
        system_clocks_s: systems.iter().map(|&system| (system, 0.0)).collect(),
        dop: None,
        system_tdops: Vec::new(),
        residuals_m: vec![0.0; used_count],
        used_sats,
        rejected_sats: Vec::new(),
        metadata: SolutionMetadata {
            iterations: 1,
            converged: true,
            status: Status::StepTolerance,
            ionosphere_applied: false,
            troposphere_applied: false,
            outer_iterations: 0,
            final_robust_scale_m: None,
            used_count,
            systems,
            redundancy,
            raim_checkable: redundancy >= 1,
        },
    }
}

struct StaticEphemeris {
    positions: Vec<(GnssSatelliteId, [f64; 3])>,
}

impl EphemerisSource for StaticEphemeris {
    fn position_clock_at_j2000_s(
        &self,
        sat: GnssSatelliteId,
        _t_j2000_s: f64,
    ) -> Option<([f64; 3], f64)> {
        self.positions
            .iter()
            .find(|(id, _)| *id == sat)
            .map(|&(_, position)| (position, 0.0))
    }
}

fn assert_abs_diff(left: f64, right: f64, tol: f64) {
    let diff = (left - right).abs();
    assert!(
        diff <= tol,
        "left={left} right={right} diff={diff} tol={tol}"
    );
}
