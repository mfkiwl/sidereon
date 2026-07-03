use std::collections::BTreeSet;

use crate::quality::{pseudorange_variance, PseudorangeVarianceOptions};

use super::{
    validate_nonneg_finite, validate_positive_finite, validate_probability, AraimError, AraimRow,
};
use crate::id::{GnssSatelliteId, GnssSystem};

/// Per-satellite integrity and accuracy model without an identity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SatelliteIsmModel {
    /// Integrity one-sigma SIS range error, meters.
    pub sigma_ura_m: f64,
    /// Accuracy and continuity one-sigma SIS range error, meters.
    pub sigma_ure_m: f64,
    /// Nominal SIS bias bound, meters.
    pub b_nom_m: f64,
    /// Prior probability for a satellite fault.
    pub p_sat: f64,
}

impl SatelliteIsmModel {
    /// Construct a per-satellite model.
    pub const fn new(sigma_ura_m: f64, sigma_ure_m: f64, b_nom_m: f64, p_sat: f64) -> Self {
        Self {
            sigma_ura_m,
            sigma_ure_m,
            b_nom_m,
            p_sat,
        }
    }
}

/// Per-satellite ISM override.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SatelliteIsm {
    /// Satellite identity for this override.
    pub id: GnssSatelliteId,
    /// Integrity one-sigma SIS range error, meters.
    pub sigma_ura_m: f64,
    /// Accuracy and continuity one-sigma SIS range error, meters.
    pub sigma_ure_m: f64,
    /// Nominal SIS bias bound, meters.
    pub b_nom_m: f64,
    /// Prior probability for a satellite fault.
    pub p_sat: f64,
}

impl SatelliteIsm {
    /// Construct a satellite-specific model.
    pub const fn new(
        id: GnssSatelliteId,
        sigma_ura_m: f64,
        sigma_ure_m: f64,
        b_nom_m: f64,
        p_sat: f64,
    ) -> Self {
        Self {
            id,
            sigma_ura_m,
            sigma_ure_m,
            b_nom_m,
            p_sat,
        }
    }

    pub(crate) const fn model(self) -> SatelliteIsmModel {
        SatelliteIsmModel {
            sigma_ura_m: self.sigma_ura_m,
            sigma_ure_m: self.sigma_ure_m,
            b_nom_m: self.b_nom_m,
            p_sat: self.p_sat,
        }
    }
}

/// Per-constellation fault prior and default satellite model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConstellationIsm {
    /// Constellation identity.
    pub system: GnssSystem,
    /// Prior probability for a constellation-wide fault.
    pub p_const: f64,
    /// Default satellite model for rows in this constellation.
    pub default_sat: SatelliteIsmModel,
}

impl ConstellationIsm {
    /// Construct a per-constellation model.
    pub const fn new(system: GnssSystem, p_const: f64, default_sat: SatelliteIsmModel) -> Self {
        Self {
            system,
            p_const,
            default_sat,
        }
    }
}

/// Parsed integrity support message used by ARAIM.
#[derive(Debug, Clone, PartialEq)]
pub struct Ism {
    /// Per-constellation defaults and constellation-wide fault priors.
    pub constellations: Vec<ConstellationIsm>,
    /// Per-satellite overrides.
    pub satellites: Vec<SatelliteIsm>,
}

impl Ism {
    /// Construct an ISM from parsed records.
    pub fn new(constellations: Vec<ConstellationIsm>, satellites: Vec<SatelliteIsm>) -> Self {
        Self {
            constellations,
            satellites,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), AraimError> {
        if self.constellations.is_empty() {
            return Err(AraimError::InvalidIsm);
        }

        let mut systems = BTreeSet::new();
        for constellation in &self.constellations {
            if !systems.insert(constellation.system) {
                return Err(AraimError::InvalidIsm);
            }
            if !validate_probability(constellation.p_const, true)
                || !validate_sat_model(constellation.default_sat)
            {
                return Err(AraimError::InvalidIsm);
            }
        }

        let mut satellites = BTreeSet::new();
        for sat in &self.satellites {
            if !satellites.insert(sat.id) || !validate_sat_model(sat.model()) {
                return Err(AraimError::InvalidIsm);
            }
            if self.constellation(sat.id.system).is_none() {
                return Err(AraimError::InvalidIsm);
            }
        }
        Ok(())
    }

    pub(crate) fn constellation(&self, system: GnssSystem) -> Option<&ConstellationIsm> {
        self.constellations
            .iter()
            .find(|constellation| constellation.system == system)
    }

    pub(crate) fn model_for(&self, id: GnssSatelliteId) -> Option<SatelliteIsmModel> {
        self.satellites
            .iter()
            .find(|sat| sat.id == id)
            .map(|sat| sat.model())
            .or_else(|| self.constellation(id.system).map(|c| c.default_sat))
    }

    pub(crate) fn effective_for(&self, row: &AraimRow) -> Result<EffectiveIsm, AraimError> {
        let model = self.model_for(row.id).ok_or(AraimError::InvalidIsm)?;
        let elevation_deg = row.elevation_rad.to_degrees();
        let local_variance_m2 =
            pseudorange_variance(elevation_deg, PseudorangeVarianceOptions::default())
                .map_err(|_| AraimError::InvalidIsm)?;
        let sigma_int_m2 = model.sigma_ura_m * model.sigma_ura_m + local_variance_m2;
        let sigma_acc_m2 = model.sigma_ure_m * model.sigma_ure_m + local_variance_m2;
        if !validate_positive_finite(sigma_int_m2) || !validate_positive_finite(sigma_acc_m2) {
            return Err(AraimError::InvalidIsm);
        }
        Ok(EffectiveIsm {
            sigma_int_m: sigma_int_m2.sqrt(),
            sigma_acc_m: sigma_acc_m2.sqrt(),
            b_nom_m: model.b_nom_m,
            p_sat: model.p_sat,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EffectiveIsm {
    pub sigma_int_m: f64,
    pub sigma_acc_m: f64,
    pub b_nom_m: f64,
    pub p_sat: f64,
}

fn validate_sat_model(model: SatelliteIsmModel) -> bool {
    validate_positive_finite(model.sigma_ura_m)
        && validate_positive_finite(model.sigma_ure_m)
        && model.sigma_ure_m <= model.sigma_ura_m
        && validate_nonneg_finite(model.b_nom_m)
        && validate_probability(model.p_sat, true)
}
