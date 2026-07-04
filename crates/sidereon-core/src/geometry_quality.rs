//! Geometry observability and residual-validation classification.

/// Observability and validation tier for an estimation geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservabilityTier {
    /// The design rank is below the parameter count, so at least one parameter
    /// is not observable.
    RankDeficient,
    /// The design is full rank, but has no residual degrees of freedom.
    ZeroRedundancy,
    /// The design is full rank with residual degrees of freedom, but exceeds a
    /// configured condition-number or GDOP cutoff.
    Weak,
    /// The design is full rank and does not exceed the configured cutoffs.
    Nominal,
}

/// Geometry observability and covariance-validation diagnostics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeometryQuality {
    /// Tier assigned from rank, redundancy, condition number, GDOP, and prior
    /// availability.
    pub tier: ObservabilityTier,
    /// Observation redundancy, defined as `n_obs - n_params`.
    pub redundancy: i32,
    /// Rank of the design matrix used by the solve.
    pub rank: usize,
    /// Condition number of the design matrix, computed as `sigma_max /
    /// sigma_min` from its singular values.
    pub condition_number: f64,
    /// Geometric dilution of precision for the solved state.
    pub gdop: f64,
    /// Whether residual-based RAIM can test the solve.
    pub raim_checkable: bool,
    /// Whether residuals or a valid propagated prior can validate the
    /// covariance bound.
    pub covariance_validated: bool,
}

/// Configurable cutoffs for [`classify`].
///
/// The default uses `cond_cutoff = 1.0e8` and `gdop_cutoff = 10.0`.
/// The condition-number cutoff follows the standard first-order linear-system
/// error amplifier, where `kappa(H) * eps` approximates relative numerical
/// sensitivity. In `f64`, `1.0e8` is far above ordinary scaling noise but still
/// below the singular-value rank threshold used by the least-squares covariance
/// path. The GDOP cutoff sits in the common GNSS screening band around 6 to 20;
/// `10.0` marks a geometry before a DOP-only projection becomes very large.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeometryQualityThresholds {
    /// Maximum accepted singular-value condition number for a full-rank solve
    /// with positive redundancy.
    pub cond_cutoff: f64,
    /// Maximum accepted GDOP for a full-rank solve with positive redundancy.
    pub gdop_cutoff: f64,
}

impl Default for GeometryQualityThresholds {
    fn default() -> Self {
        Self {
            cond_cutoff: 1.0e8,
            gdop_cutoff: 10.0,
        }
    }
}

/// Classify geometry observability and covariance validation from scalar
/// diagnostics.
///
/// `rank` is compared to `n_params`. `redundancy` is `n_obs - n_params`.
/// `condition_number` must be the singular-value ratio of the design matrix,
/// not the condition number of the normal matrix. A non-finite condition number,
/// GDOP, or cutoff is treated as exceeding the corresponding cutoff for
/// full-rank positive-redundancy cases.
pub fn classify(
    rank: usize,
    n_params: usize,
    redundancy: i32,
    condition_number: f64,
    gdop: f64,
    has_valid_prior: bool,
    thresholds: GeometryQualityThresholds,
) -> GeometryQuality {
    let (tier, raim_checkable, covariance_validated) = if rank < n_params {
        (ObservabilityTier::RankDeficient, false, false)
    } else if redundancy == 0 {
        (ObservabilityTier::ZeroRedundancy, false, has_valid_prior)
    } else if redundancy >= 1
        && (exceeds_cutoff(condition_number, thresholds.cond_cutoff)
            || exceeds_cutoff(gdop, thresholds.gdop_cutoff))
    {
        (ObservabilityTier::Weak, true, true)
    } else {
        let raim_checkable = redundancy >= 1;
        (
            ObservabilityTier::Nominal,
            raim_checkable,
            raim_checkable || has_valid_prior,
        )
    };

    GeometryQuality {
        tier,
        redundancy,
        rank,
        condition_number,
        gdop,
        raim_checkable,
        covariance_validated,
    }
}

fn exceeds_cutoff(value: f64, cutoff: f64) -> bool {
    !value.is_finite() || !cutoff.is_finite() || value > cutoff
}

#[cfg(test)]
mod tests {
    //! Clean-room tests derived from estimation-theory classification rules.
    //! Redundancy, rank, condition number, GDOP, and prior availability are
    //! explicit scalar inputs; expected tiers do not come from a solve.

    use super::*;

    fn thresholds() -> GeometryQualityThresholds {
        GeometryQualityThresholds {
            cond_cutoff: 100.0,
            gdop_cutoff: 10.0,
        }
    }

    #[test]
    fn zero_redundancy_without_prior_is_not_validated() {
        let quality = classify(4, 4, 0, 3.0, 2.0, false, thresholds());

        assert_eq!(
            quality,
            GeometryQuality {
                tier: ObservabilityTier::ZeroRedundancy,
                redundancy: 0,
                rank: 4,
                condition_number: 3.0,
                gdop: 2.0,
                raim_checkable: false,
                covariance_validated: false,
            }
        );
    }

    #[test]
    fn zero_redundancy_with_prior_is_validated() {
        let quality = classify(4, 4, 0, 3.0, 2.0, true, thresholds());

        assert_eq!(quality.tier, ObservabilityTier::ZeroRedundancy);
        assert!(!quality.raim_checkable);
        assert!(quality.covariance_validated);
    }

    #[test]
    fn rank_deficient_disables_raim_and_covariance_validation() {
        let quality = classify(3, 4, 2, 2.0e12, 30.0, true, thresholds());

        assert_eq!(quality.tier, ObservabilityTier::RankDeficient);
        assert_eq!(quality.rank, 3);
        assert_eq!(quality.redundancy, 2);
        assert!(!quality.raim_checkable);
        assert!(!quality.covariance_validated);
    }

    #[test]
    fn weak_when_condition_number_exceeds_cutoff() {
        let quality = classify(4, 4, 1, 100.0 + 1.0e-9, 2.0, false, thresholds());

        assert_eq!(quality.tier, ObservabilityTier::Weak);
        assert!(quality.raim_checkable);
        assert!(quality.covariance_validated);
    }

    #[test]
    fn weak_when_gdop_exceeds_cutoff() {
        let quality = classify(4, 4, 1, 3.0, 10.0 + 1.0e-12, false, thresholds());

        assert_eq!(quality.tier, ObservabilityTier::Weak);
        assert!(quality.raim_checkable);
        assert!(quality.covariance_validated);
    }

    #[test]
    fn nominal_with_full_rank_and_positive_redundancy() {
        let quality = classify(4, 4, 2, 20.0, 4.0, false, thresholds());

        assert_eq!(
            quality,
            GeometryQuality {
                tier: ObservabilityTier::Nominal,
                redundancy: 2,
                rank: 4,
                condition_number: 20.0,
                gdop: 4.0,
                raim_checkable: true,
                covariance_validated: true,
            }
        );
    }

    #[test]
    fn condition_cutoff_boundary_is_strict() {
        let at_cutoff = classify(4, 4, 1, 100.0, 2.0, false, thresholds());
        let below_cutoff = classify(4, 4, 1, 100.0 - 1.0e-9, 2.0, false, thresholds());
        let above_cutoff = classify(4, 4, 1, 100.0 + 1.0e-9, 2.0, false, thresholds());

        assert_eq!(at_cutoff.tier, ObservabilityTier::Nominal);
        assert_eq!(below_cutoff.tier, ObservabilityTier::Nominal);
        assert_eq!(above_cutoff.tier, ObservabilityTier::Weak);
    }

    #[test]
    fn gdop_cutoff_boundary_is_strict() {
        let at_cutoff = classify(4, 4, 1, 3.0, 10.0, false, thresholds());
        let below_cutoff = classify(4, 4, 1, 3.0, 10.0 - 1.0e-12, false, thresholds());
        let above_cutoff = classify(4, 4, 1, 3.0, 10.0 + 1.0e-12, false, thresholds());

        assert_eq!(at_cutoff.tier, ObservabilityTier::Nominal);
        assert_eq!(below_cutoff.tier, ObservabilityTier::Nominal);
        assert_eq!(above_cutoff.tier, ObservabilityTier::Weak);
    }
}
