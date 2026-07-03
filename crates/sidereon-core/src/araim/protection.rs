use crate::astro::math::linear::invert_symmetric_pd;
use crate::astro::math::special::{normal_q, normal_q_inv};
use crate::dop::ecef_to_enu_rotation;
use crate::id::GnssSystem;

use super::{clock_system_for_row, validate_probability, AraimError, AraimGeometry};

/// Satellite error model consumed by protection-level solvers.
pub trait ProtectionModel {
    /// Number of satellite rows.
    fn len(&self) -> usize;
    /// Integrity sigma for row `index`, meters.
    fn sigma_int_m(&self, index: usize) -> f64;
    /// Accuracy sigma for row `index`, meters.
    fn sigma_acc_m(&self, index: usize) -> f64;
    /// Nominal bias bound for row `index`, meters.
    fn b_nom_m(&self, index: usize) -> f64;
    /// Returns true when there are no satellite rows.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GainMatrix {
    pub enu_rows: [Vec<f64>; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ProtectionEquationTerm {
    pub prior: f64,
    pub sigma_m: f64,
    pub bias_m: f64,
    pub threshold_m: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ProtectionLevelSolution {
    pub value_m: f64,
    pub converged: bool,
}

pub(crate) fn gain_matrix_enu(
    geometry: &AraimGeometry,
    weights: &[f64],
) -> Result<GainMatrix, AraimError> {
    gain_matrix_enu_for_clock_systems(geometry, weights, &geometry.clock_systems)
}

pub(crate) fn gain_matrix_enu_for_clock_systems(
    geometry: &AraimGeometry,
    weights: &[f64],
    clock_systems: &[GnssSystem],
) -> Result<GainMatrix, AraimError> {
    if weights.len() != geometry.rows.len() || clock_systems.is_empty() {
        return Err(AraimError::InsufficientGeometry);
    }
    let n_state = 3 + clock_systems.len();
    if weights.iter().filter(|&&weight| weight > 0.0).count() < n_state {
        return Err(AraimError::InsufficientGeometry);
    }

    let mut normal = vec![vec![0.0_f64; n_state]; n_state];
    let mut design_rows: Vec<Vec<f64>> = Vec::with_capacity(geometry.rows.len());
    for (row, &weight) in geometry.rows.iter().zip(weights) {
        if !weight.is_finite() || weight < 0.0 {
            return Err(AraimError::NumericalFailure);
        }
        let design = if weight > 0.0 {
            design_row(clock_systems, row)?
        } else {
            vec![0.0; n_state]
        };
        if weight > 0.0 {
            for i in 0..n_state {
                for j in 0..n_state {
                    normal[i][j] += design[i] * weight * design[j];
                }
            }
        }
        design_rows.push(design);
    }

    let inverse = invert_symmetric_pd(&normal).ok_or(AraimError::InsufficientGeometry)?;
    let mut ecef_rows = [
        vec![0.0; geometry.rows.len()],
        vec![0.0; geometry.rows.len()],
        vec![0.0; geometry.rows.len()],
    ];
    for (measurement_idx, (design, &weight)) in design_rows.iter().zip(weights).enumerate() {
        if weight == 0.0 {
            continue;
        }
        for state_idx in 0..3 {
            let mut value = 0.0;
            for col in 0..n_state {
                value += inverse[state_idx][col] * design[col] * weight;
            }
            ecef_rows[state_idx][measurement_idx] = value;
        }
    }

    let r = ecef_to_enu_rotation(geometry.receiver.lat_rad, geometry.receiver.lon_rad);
    let mut enu_rows = [
        vec![0.0; geometry.rows.len()],
        vec![0.0; geometry.rows.len()],
        vec![0.0; geometry.rows.len()],
    ];
    for coord in 0..3 {
        for measurement_idx in 0..geometry.rows.len() {
            enu_rows[coord][measurement_idx] = r[coord][0] * ecef_rows[0][measurement_idx]
                + r[coord][1] * ecef_rows[1][measurement_idx]
                + r[coord][2] * ecef_rows[2][measurement_idx];
        }
    }

    Ok(GainMatrix { enu_rows })
}

pub(crate) fn metric_sigma(gain_row: &[f64], sigmas_m: &[f64]) -> f64 {
    gain_row
        .iter()
        .zip(sigmas_m)
        .map(|(&s, &sigma)| s * s * sigma * sigma)
        .sum::<f64>()
        .sqrt()
}

pub(crate) fn metric_bias(gain_row: &[f64], biases_m: &[f64]) -> f64 {
    gain_row
        .iter()
        .zip(biases_m)
        .map(|(&s, &bias)| s.abs() * bias)
        .sum()
}

pub(crate) fn separation_sigma(
    gain_row: &[f64],
    fault_free_gain_row: &[f64],
    sigmas_m: &[f64],
) -> f64 {
    gain_row
        .iter()
        .zip(fault_free_gain_row)
        .zip(sigmas_m)
        .map(|((&sk, &s0), &sigma)| {
            let ds = sk - s0;
            ds * ds * sigma * sigma
        })
        .sum::<f64>()
        .sqrt()
}

pub(crate) fn k_false_alert(pfa: f64, n_fault_modes: usize) -> Result<f64, AraimError> {
    if n_fault_modes == 0 {
        return Ok(0.0);
    }
    if !validate_probability(pfa, false) {
        return Err(AraimError::InvalidAllocation);
    }
    normal_q_inv(pfa / (2.0 * n_fault_modes as f64)).ok_or(AraimError::InvalidAllocation)
}

pub(crate) fn solve_protection_level(
    fault_free: ProtectionEquationTerm,
    fault_terms: &[ProtectionEquationTerm],
    phmi: f64,
) -> Result<ProtectionLevelSolution, AraimError> {
    if !validate_probability(phmi, false) {
        return Err(AraimError::InvalidAllocation);
    }
    validate_term(fault_free)?;
    for term in fault_terms {
        validate_term(*term)?;
    }

    let target = phmi * 0.5;
    if fault_terms.is_empty() {
        let value_m = normal_q_inv(target).ok_or(AraimError::InvalidAllocation)?
            * fault_free.sigma_m
            + fault_free.bias_m;
        return Ok(ProtectionLevelSolution {
            value_m,
            converged: true,
        });
    }

    let mut lo = 0.0;
    if protection_lhs(lo, fault_free, fault_terms) <= target {
        return Ok(ProtectionLevelSolution {
            value_m: 0.0,
            converged: true,
        });
    }

    let mut hi = normal_q_inv(target).ok_or(AraimError::InvalidAllocation)? * fault_free.sigma_m
        + fault_free.bias_m;
    if hi <= lo || !hi.is_finite() {
        hi = 1.0;
    }
    let mut expanded = 0usize;
    while protection_lhs(hi, fault_free, fault_terms) > target {
        hi *= 2.0;
        expanded += 1;
        if !hi.is_finite() || expanded > 100 {
            return Err(AraimError::NumericalFailure);
        }
    }

    let mut converged = false;
    for _ in 0..80 {
        let mid = 0.5 * (lo + hi);
        if protection_lhs(mid, fault_free, fault_terms) > target {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo <= 1.0e-4 {
            converged = true;
            break;
        }
    }
    Ok(ProtectionLevelSolution {
        value_m: hi,
        converged,
    })
}

fn protection_lhs(
    y_m: f64,
    fault_free: ProtectionEquationTerm,
    fault_terms: &[ProtectionEquationTerm],
) -> f64 {
    let mut value = normal_q((y_m - fault_free.bias_m) / fault_free.sigma_m);
    for term in fault_terms {
        value += term.prior * normal_q((y_m - term.threshold_m - term.bias_m) / term.sigma_m);
    }
    value
}

fn validate_term(term: ProtectionEquationTerm) -> Result<(), AraimError> {
    if term.prior.is_finite()
        && term.prior >= 0.0
        && term.sigma_m.is_finite()
        && term.sigma_m > 0.0
        && term.bias_m.is_finite()
        && term.bias_m >= 0.0
        && term.threshold_m.is_finite()
        && term.threshold_m >= 0.0
    {
        Ok(())
    } else {
        Err(AraimError::NumericalFailure)
    }
}

fn design_row(clock_systems: &[GnssSystem], row: &super::AraimRow) -> Result<Vec<f64>, AraimError> {
    let mut design = vec![0.0_f64; 3 + clock_systems.len()];
    let los = row.line_of_sight;
    if !los.e_x.is_finite()
        || !los.e_y.is_finite()
        || !los.e_z.is_finite()
        || !row.elevation_rad.is_finite()
    {
        return Err(AraimError::InsufficientGeometry);
    }
    design[0] = -los.e_x;
    design[1] = -los.e_y;
    design[2] = -los.e_z;
    let clock_system = clock_system_for_row(row.system);
    let clock_idx = clock_systems
        .iter()
        .position(|&system| system == clock_system)
        .ok_or(AraimError::InsufficientGeometry)?;
    design[3 + clock_idx] = 1.0;
    Ok(design)
}
