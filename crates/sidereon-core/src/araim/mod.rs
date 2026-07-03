//! Advanced RAIM multi-hypothesis snapshot integrity.
//!
//! This module is sans-IO: callers provide line-of-sight geometry plus an
//! externally supplied integrity support message, and the solver returns
//! protection levels without reading products, global state, or residuals.

pub mod fault_modes;
pub mod ism;
mod mhss;
pub mod protection;

#[cfg(test)]
mod tests;

pub use fault_modes::{enumerate_fault_modes, FaultHypothesis};
pub use ism::{ConstellationIsm, Ism, SatelliteIsm, SatelliteIsmModel};
pub use mhss::{araim, AraimResult, FaultMode};

use crate::dop::LineOfSight;
use crate::frame::Wgs84Geodetic;
use crate::id::{GnssSatelliteId, GnssSystem};
use crate::spp::{EphemerisSource, ReceiverSolution};

/// One satellite row in an ARAIM geometry snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AraimRow {
    /// Satellite identity used for ISM lookup and satellite-fault modes.
    pub id: GnssSatelliteId,
    /// Receiver-to-satellite ECEF unit vector.
    pub line_of_sight: LineOfSight,
    /// Constellation owning the signal and constellation-fault mode.
    pub system: GnssSystem,
    /// Elevation angle at the receiver, radians.
    pub elevation_rad: f64,
}

/// A snapshot geometry and clock-column convention for ARAIM.
#[derive(Debug, Clone, PartialEq)]
pub struct AraimGeometry {
    /// Satellite rows, index-aligned through all gain matrices.
    pub rows: Vec<AraimRow>,
    /// Receiver geodetic position for ENU rotation.
    pub receiver: Wgs84Geodetic,
    /// Receiver-clock columns, in the same order as the SPP state.
    pub clock_systems: Vec<GnssSystem>,
}

impl AraimGeometry {
    /// Build ARAIM geometry from an SPP solution.
    ///
    /// The current SPP solution type does not retain receive epoch or measured
    /// pseudoranges, so this adapter cannot reconstruct transmit-time satellite
    /// positions without adding inputs that are outside this signature. Callers
    /// should pass explicit [`AraimRow`] values through [`AraimGeometry`] until
    /// the solve record carries the required epoch data.
    pub fn from_receiver_solution(
        solution: &ReceiverSolution,
        eph: &dyn EphemerisSource,
    ) -> Result<Self, AraimError> {
        let _ = (solution, eph);
        Err(AraimError::InsufficientGeometry)
    }
}

/// Integrity and continuity risk allocation for one ARAIM solve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntegrityAllocation {
    /// Total probability of hazardous misleading information.
    pub phmi_total: f64,
    /// Vertical PHMI allocation.
    pub phmi_vert: f64,
    /// Horizontal PHMI allocation.
    pub phmi_hor: f64,
    /// Vertical false-alert allocation.
    pub pfa_vert: f64,
    /// Horizontal false-alert allocation.
    pub pfa_hor: f64,
    /// Maximum acceptable unmonitored fault probability mass.
    pub p_threshold_unmonitored: f64,
    /// Maximum enumerated satellite-fault order. Zero keeps only fault-free.
    pub max_fault_order: usize,
}

impl IntegrityAllocation {
    /// LPV-200 style allocation commonly used by public ARAIM examples.
    pub const fn lpv_200() -> Self {
        Self {
            phmi_total: 1.0e-7,
            phmi_vert: 1.0e-7,
            phmi_hor: 1.0e-7,
            pfa_vert: 4.0e-6,
            pfa_hor: 4.0e-6,
            p_threshold_unmonitored: 1.0e-8,
            max_fault_order: 2,
        }
    }
}

/// ARAIM input or numerical failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AraimError {
    /// The full or subset geometry does not have enough independent rows.
    #[error("insufficient ARAIM geometry")]
    InsufficientGeometry,
    /// The unmonitorable fault probability exceeds the allocation.
    #[error("unmonitorable ARAIM fault probability exceeds allocation")]
    UnmonitorableFaultMass,
    /// A matrix operation or root solve failed.
    #[error("ARAIM numerical failure")]
    NumericalFailure,
    /// The ISM is missing, non-finite, or outside its valid domain.
    #[error("invalid ARAIM ISM")]
    InvalidIsm,
    /// The integrity allocation is missing, non-finite, or outside its domain.
    #[error("invalid ARAIM allocation")]
    InvalidAllocation,
}

pub(crate) fn clock_system_for_row(system: GnssSystem) -> GnssSystem {
    match system {
        GnssSystem::Sbas => GnssSystem::Gps,
        other => other,
    }
}

pub(crate) fn validate_probability(value: f64, allow_zero: bool) -> bool {
    value.is_finite()
        && if allow_zero {
            (0.0..1.0).contains(&value) || value == 0.0
        } else {
            (0.0..1.0).contains(&value)
        }
}

pub(crate) fn validate_nonneg_finite(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

pub(crate) fn validate_positive_finite(value: f64) -> bool {
    value.is_finite() && value > 0.0
}
