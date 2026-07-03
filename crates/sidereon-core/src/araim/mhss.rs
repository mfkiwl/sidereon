use std::collections::BTreeSet;

use super::fault_modes::{enumerate_fault_modes_checked, FaultHypothesis};
use super::ism::Ism;
use super::protection::{
    gain_matrix_enu, k_false_alert, metric_bias, metric_sigma, separation_sigma,
    solve_protection_level, ProtectionEquationTerm,
};
use super::{validate_probability, AraimError, AraimGeometry, IntegrityAllocation};
use crate::id::{GnssSatelliteId, GnssSystem};

/// Per-hypothesis ARAIM monitor data.
#[derive(Debug, Clone, PartialEq)]
pub struct FaultMode {
    /// Satellites excluded by this mode.
    pub excluded: Vec<GnssSatelliteId>,
    /// Constellation excluded by this mode, if any.
    pub excluded_constellation: Option<GnssSystem>,
    /// Fault prior probability for this mode.
    pub prior: f64,
    /// Integrity sigma in local `[east, north, up]`, meters.
    pub sigma_int_enu_m: [f64; 3],
    /// Nominal bias bound in local `[east, north, up]`, meters.
    pub bias_enu_m: [f64; 3],
    /// Separation monitor threshold in local `[east, north, up]`, meters.
    pub threshold_enu_m: [f64; 3],
    /// True when the subset geometry is full-rank.
    pub monitorable: bool,
}

/// ARAIM protection-level result.
#[derive(Debug, Clone, PartialEq)]
pub struct AraimResult {
    /// Horizontal protection level, meters.
    pub hpl_m: f64,
    /// Vertical protection level, meters.
    pub vpl_m: f64,
    /// All-in-view horizontal accuracy sigma, meters.
    pub sigma_acc_h_m: f64,
    /// All-in-view vertical accuracy sigma, meters.
    pub sigma_acc_v_m: f64,
    /// Effective monitor threshold, meters.
    pub emt_m: f64,
    /// Per-mode monitor data, including the fault-free mode first.
    pub fault_modes: Vec<FaultMode>,
    /// Unenumerated plus unmonitorable fault probability mass.
    pub p_unmonitored: f64,
    /// True when the solve met the allocation and all PL roots converged.
    pub availability: bool,
}

/// Run the ARAIM MHSS protection-level algorithm.
pub fn araim(
    geometry: &AraimGeometry,
    ism: &Ism,
    allocation: &IntegrityAllocation,
) -> Result<AraimResult, AraimError> {
    validate_geometry(geometry)?;
    validate_allocation(allocation)?;
    ism.validate()?;

    let effective = geometry
        .rows
        .iter()
        .map(|row| ism.effective_for(row))
        .collect::<Result<Vec<_>, _>>()?;
    let sigma_int_m: Vec<f64> = effective.iter().map(|model| model.sigma_int_m).collect();
    let sigma_acc_m: Vec<f64> = effective.iter().map(|model| model.sigma_acc_m).collect();
    let bias_m: Vec<f64> = effective.iter().map(|model| model.b_nom_m).collect();
    let weights_int: Vec<f64> = sigma_int_m
        .iter()
        .map(|sigma| 1.0 / (sigma * sigma))
        .collect();
    let weights_acc: Vec<f64> = sigma_acc_m
        .iter()
        .map(|sigma| 1.0 / (sigma * sigma))
        .collect();

    let enumeration = enumerate_fault_modes_checked(geometry, ism, allocation)?;
    let n_fault_modes = enumeration.modes.len().saturating_sub(1);
    let k_h = k_false_alert(allocation.pfa_hor, n_fault_modes)?;
    let k_v = k_false_alert(allocation.pfa_vert, n_fault_modes)?;

    let fault_free_int = gain_matrix_enu(geometry, &weights_int)?;
    let fault_free_acc = gain_matrix_enu(geometry, &weights_acc)?;
    let sigma_acc_e = metric_sigma(&fault_free_acc.enu_rows[0], &sigma_acc_m);
    let sigma_acc_n = metric_sigma(&fault_free_acc.enu_rows[1], &sigma_acc_m);
    let sigma_acc_u = metric_sigma(&fault_free_acc.enu_rows[2], &sigma_acc_m);

    let mut p_unmonitored = enumeration.p_unenumerated;
    let mut fault_modes = Vec::with_capacity(enumeration.modes.len());
    for (mode_idx, hypothesis) in enumeration.modes.iter().enumerate() {
        let mode = if mode_idx == 0 {
            compute_monitorable_mode(
                hypothesis,
                MonitorInputs {
                    gain_int: &fault_free_int,
                    gain_acc: &fault_free_acc,
                    fault_free_acc: &fault_free_acc,
                    sigma_int_m: &sigma_int_m,
                    sigma_acc_m: &sigma_acc_m,
                    bias_m: &bias_m,
                    k: [0.0, 0.0, 0.0],
                },
            )
        } else {
            let weights_int_k = zeroed_weights(geometry, &weights_int, hypothesis);
            let weights_acc_k = zeroed_weights(geometry, &weights_acc, hypothesis);
            match (
                gain_matrix_enu(geometry, &weights_int_k),
                gain_matrix_enu(geometry, &weights_acc_k),
            ) {
                (Ok(gain_int), Ok(gain_acc)) => compute_monitorable_mode(
                    hypothesis,
                    MonitorInputs {
                        gain_int: &gain_int,
                        gain_acc: &gain_acc,
                        fault_free_acc: &fault_free_acc,
                        sigma_int_m: &sigma_int_m,
                        sigma_acc_m: &sigma_acc_m,
                        bias_m: &bias_m,
                        k: [k_h, k_h, k_v],
                    },
                ),
                _ => {
                    p_unmonitored += hypothesis.prior;
                    unmonitorable_mode(hypothesis)
                }
            }
        };
        fault_modes.push(mode);
    }

    if p_unmonitored > allocation.p_threshold_unmonitored {
        return Err(AraimError::UnmonitorableFaultMass);
    }

    let pl_e = solve_coord_pl(&fault_modes, 0, allocation.phmi_hor)?;
    let pl_n = solve_coord_pl(&fault_modes, 1, allocation.phmi_hor)?;
    let pl_u = solve_coord_pl(&fault_modes, 2, allocation.phmi_vert)?;
    let emt_m = fault_modes
        .iter()
        .filter(|mode| mode.monitorable)
        .flat_map(|mode| mode.threshold_enu_m)
        .fold(0.0_f64, f64::max);

    Ok(AraimResult {
        hpl_m: (pl_e * pl_e + pl_n * pl_n).sqrt(),
        vpl_m: pl_u,
        sigma_acc_h_m: (sigma_acc_e * sigma_acc_e + sigma_acc_n * sigma_acc_n).sqrt(),
        sigma_acc_v_m: sigma_acc_u,
        emt_m,
        fault_modes,
        p_unmonitored,
        availability: true,
    })
}

struct MonitorInputs<'a> {
    gain_int: &'a super::protection::GainMatrix,
    gain_acc: &'a super::protection::GainMatrix,
    fault_free_acc: &'a super::protection::GainMatrix,
    sigma_int_m: &'a [f64],
    sigma_acc_m: &'a [f64],
    bias_m: &'a [f64],
    k: [f64; 3],
}

fn compute_monitorable_mode(hypothesis: &FaultHypothesis, inputs: MonitorInputs<'_>) -> FaultMode {
    let mut sigma_int_enu_m = [0.0_f64; 3];
    let mut bias_enu_m = [0.0_f64; 3];
    let mut threshold_enu_m = [0.0_f64; 3];
    for coord in 0..3 {
        sigma_int_enu_m[coord] = metric_sigma(&inputs.gain_int.enu_rows[coord], inputs.sigma_int_m);
        bias_enu_m[coord] = metric_bias(&inputs.gain_int.enu_rows[coord], inputs.bias_m);
        threshold_enu_m[coord] = inputs.k[coord]
            * separation_sigma(
                &inputs.gain_acc.enu_rows[coord],
                &inputs.fault_free_acc.enu_rows[coord],
                inputs.sigma_acc_m,
            );
    }

    FaultMode {
        excluded: hypothesis.excluded.clone(),
        excluded_constellation: hypothesis.excluded_constellation,
        prior: hypothesis.prior,
        sigma_int_enu_m,
        bias_enu_m,
        threshold_enu_m,
        monitorable: true,
    }
}

fn unmonitorable_mode(hypothesis: &FaultHypothesis) -> FaultMode {
    FaultMode {
        excluded: hypothesis.excluded.clone(),
        excluded_constellation: hypothesis.excluded_constellation,
        prior: hypothesis.prior,
        sigma_int_enu_m: [f64::INFINITY; 3],
        bias_enu_m: [f64::INFINITY; 3],
        threshold_enu_m: [f64::INFINITY; 3],
        monitorable: false,
    }
}

fn zeroed_weights(
    geometry: &AraimGeometry,
    weights: &[f64],
    hypothesis: &FaultHypothesis,
) -> Vec<f64> {
    geometry
        .rows
        .iter()
        .zip(weights)
        .map(|(row, &weight)| {
            if hypothesis.excludes_satellite(row.id, row.system) {
                0.0
            } else {
                weight
            }
        })
        .collect()
}

fn solve_coord_pl(modes: &[FaultMode], coord: usize, phmi: f64) -> Result<f64, AraimError> {
    let fault_free = modes.first().ok_or(AraimError::NumericalFailure)?;
    if !fault_free.monitorable {
        return Err(AraimError::InsufficientGeometry);
    }
    let fault_free_term = ProtectionEquationTerm {
        prior: 1.0,
        sigma_m: fault_free.sigma_int_enu_m[coord],
        bias_m: fault_free.bias_enu_m[coord],
        threshold_m: 0.0,
    };
    let fault_terms: Vec<ProtectionEquationTerm> = modes
        .iter()
        .skip(1)
        .filter(|mode| mode.monitorable)
        .map(|mode| ProtectionEquationTerm {
            prior: mode.prior,
            sigma_m: mode.sigma_int_enu_m[coord],
            bias_m: mode.bias_enu_m[coord],
            threshold_m: mode.threshold_enu_m[coord],
        })
        .collect();
    solve_protection_level(fault_free_term, &fault_terms, phmi)
}

fn validate_geometry(geometry: &AraimGeometry) -> Result<(), AraimError> {
    if geometry.clock_systems.is_empty() {
        return Err(AraimError::InsufficientGeometry);
    }
    let n_state = 3 + geometry.clock_systems.len();
    if geometry.rows.len() < n_state {
        return Err(AraimError::InsufficientGeometry);
    }
    let mut clock_systems = BTreeSet::new();
    for &system in &geometry.clock_systems {
        if !clock_systems.insert(system) {
            return Err(AraimError::InsufficientGeometry);
        }
    }

    let mut ids = BTreeSet::new();
    for row in &geometry.rows {
        if row.id.system != row.system || !ids.insert(row.id) {
            return Err(AraimError::InsufficientGeometry);
        }
        if !(-core::f64::consts::FRAC_PI_2..=core::f64::consts::FRAC_PI_2)
            .contains(&row.elevation_rad)
        {
            return Err(AraimError::InsufficientGeometry);
        }
        let los = row.line_of_sight;
        let norm = (los.e_x * los.e_x + los.e_y * los.e_y + los.e_z * los.e_z).sqrt();
        if !norm.is_finite() || (norm - 1.0).abs() > 1.0e-3 {
            return Err(AraimError::InsufficientGeometry);
        }
    }
    Ok(())
}

fn validate_allocation(allocation: &IntegrityAllocation) -> Result<(), AraimError> {
    let valid = validate_probability(allocation.phmi_total, false)
        && validate_probability(allocation.phmi_vert, false)
        && validate_probability(allocation.phmi_hor, false)
        && validate_probability(allocation.pfa_vert, false)
        && validate_probability(allocation.pfa_hor, false)
        && validate_probability(allocation.p_threshold_unmonitored, true)
        && allocation.phmi_vert <= allocation.phmi_total
        && allocation.phmi_hor <= allocation.phmi_total;
    if valid {
        Ok(())
    } else {
        Err(AraimError::InvalidAllocation)
    }
}
