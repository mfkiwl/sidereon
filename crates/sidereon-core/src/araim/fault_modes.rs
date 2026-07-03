use std::cmp::Ordering;

use super::{AraimError, AraimGeometry, IntegrityAllocation};
use crate::id::{GnssSatelliteId, GnssSystem};

use super::ism::Ism;

/// One ARAIM fault hypothesis.
#[derive(Debug, Clone, PartialEq)]
pub struct FaultHypothesis {
    /// Satellites excluded by this hypothesis.
    pub excluded: Vec<GnssSatelliteId>,
    /// Constellation excluded by this hypothesis, if any.
    pub excluded_constellation: Option<GnssSystem>,
    /// Prior probability mass for this hypothesis.
    pub prior: f64,
}

impl FaultHypothesis {
    pub(crate) fn fault_free() -> Self {
        Self {
            excluded: Vec::new(),
            excluded_constellation: None,
            prior: 1.0,
        }
    }

    pub(crate) fn excludes_satellite(&self, id: GnssSatelliteId, system: GnssSystem) -> bool {
        self.excluded.contains(&id) || self.excluded_constellation == Some(system)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FaultEnumeration {
    pub modes: Vec<FaultHypothesis>,
    pub p_unenumerated: f64,
}

/// Enumerate ARAIM fault modes in the order used by the MHSS solve.
pub fn enumerate_fault_modes(
    geometry: &AraimGeometry,
    ism: &Ism,
    allocation: &IntegrityAllocation,
) -> Vec<FaultHypothesis> {
    enumerate_fault_modes_checked(geometry, ism, allocation)
        .map(|enumeration| enumeration.modes)
        .unwrap_or_default()
}

pub(crate) fn enumerate_fault_modes_checked(
    geometry: &AraimGeometry,
    ism: &Ism,
    allocation: &IntegrityAllocation,
) -> Result<FaultEnumeration, AraimError> {
    ism.validate()?;
    let mut modes = vec![FaultHypothesis::fault_free()];
    let mut p_unenumerated = 0.0;

    let mut sat_priors: Vec<(GnssSatelliteId, f64)> = Vec::with_capacity(geometry.rows.len());
    for row in &geometry.rows {
        let model = ism.effective_for(row)?;
        sat_priors.push((row.id, model.p_sat));
    }
    let satellite_order_mass = satellite_order_masses(&sat_priors);

    if allocation.max_fault_order >= 1 {
        for &(id, prior) in &sat_priors {
            if prior > 0.0 {
                modes.push(FaultHypothesis {
                    excluded: vec![id],
                    excluded_constellation: None,
                    prior,
                });
            }
        }

        let mut systems: Vec<GnssSystem> = geometry.rows.iter().map(|row| row.system).collect();
        systems.sort_unstable();
        systems.dedup();
        for system in systems {
            let constellation = ism.constellation(system).ok_or(AraimError::InvalidIsm)?;
            if constellation.p_const > 0.0 {
                modes.push(FaultHypothesis {
                    excluded: Vec::new(),
                    excluded_constellation: Some(system),
                    prior: constellation.p_const,
                });
            }
        }
    } else {
        for &(_, prior) in &sat_priors {
            p_unenumerated += prior;
        }
        let mut systems: Vec<GnssSystem> = geometry.rows.iter().map(|row| row.system).collect();
        systems.sort_unstable();
        systems.dedup();
        for system in systems {
            p_unenumerated += ism
                .constellation(system)
                .ok_or(AraimError::InvalidIsm)?
                .p_const;
        }
    }

    let max_sat_order = sat_priors.len();
    let max_enumerated_sat_order = allocation.max_fault_order.min(max_sat_order);
    if max_enumerated_sat_order >= 2 {
        let mut candidates = Vec::new();
        for order in 2..=max_enumerated_sat_order {
            collect_satellite_combinations(
                &sat_priors,
                order,
                0,
                &mut Vec::with_capacity(order),
                1.0,
                &mut candidates,
            );
        }
        candidates.sort_by(compare_candidates);

        let mut remaining: f64 = candidates.iter().map(|candidate| candidate.prior).sum();
        for candidate in candidates {
            if remaining < allocation.p_threshold_unmonitored {
                break;
            }
            remaining -= candidate.prior;
            modes.push(FaultHypothesis {
                excluded: candidate.ids,
                excluded_constellation: None,
                prior: candidate.prior,
            });
        }
        p_unenumerated += remaining;
        p_unenumerated += satellite_order_mass
            .iter()
            .skip(max_enumerated_sat_order + 1)
            .sum::<f64>();
    } else {
        p_unenumerated += satellite_order_mass.iter().skip(2).sum::<f64>();
    }

    let monitored_fault_mass: f64 = modes.iter().skip(1).map(|mode| mode.prior).sum();
    let total_fault_mass = monitored_fault_mass + p_unenumerated;
    if !total_fault_mass.is_finite() || total_fault_mass > 1.0 + 1.0e-12 {
        return Err(AraimError::InvalidIsm);
    }
    modes[0].prior = (1.0 - total_fault_mass).max(0.0);

    debug_assert!(p_unenumerated >= 0.0);
    debug_assert!((monitored_fault_mass + p_unenumerated + modes[0].prior - 1.0).abs() <= 1.0e-10);
    Ok(FaultEnumeration {
        modes,
        p_unenumerated,
    })
}

#[derive(Debug, Clone, PartialEq)]
struct Candidate {
    ids: Vec<GnssSatelliteId>,
    prior: f64,
}

fn satellite_order_masses(sat_priors: &[(GnssSatelliteId, f64)]) -> Vec<f64> {
    let mut masses = vec![0.0; sat_priors.len() + 1];
    masses[0] = 1.0;
    for (seen, &(_, prior)) in sat_priors.iter().enumerate() {
        for order in (1..=seen + 1).rev() {
            masses[order] += masses[order - 1] * prior;
        }
    }
    masses
}

fn collect_satellite_combinations(
    sat_priors: &[(GnssSatelliteId, f64)],
    order: usize,
    start: usize,
    selected: &mut Vec<GnssSatelliteId>,
    prior: f64,
    out: &mut Vec<Candidate>,
) {
    if selected.len() == order {
        if prior > 0.0 {
            out.push(Candidate {
                ids: selected.clone(),
                prior,
            });
        }
        return;
    }

    let remaining_slots = order - selected.len();
    let max_start = sat_priors.len().saturating_sub(remaining_slots);
    for idx in start..=max_start {
        let (id, p_sat) = sat_priors[idx];
        selected.push(id);
        collect_satellite_combinations(sat_priors, order, idx + 1, selected, prior * p_sat, out);
        selected.pop();
    }
}

fn compare_candidates(a: &Candidate, b: &Candidate) -> Ordering {
    b.prior
        .partial_cmp(&a.prior)
        .unwrap_or(Ordering::Equal)
        .then_with(|| a.ids.cmp(&b.ids))
}
