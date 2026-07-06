use crate::astro::error::PropagationError;
use crate::astro::frames::orientation::EarthOrientationProvider;
use crate::constants::SECONDS_PER_HOUR;
use std::sync::Arc;

/// Per-evaluation context shared with force models.
///
/// The default context is intentionally empty. A caller that wants a body-fixed
/// force to use the precise Earth-fixed frame can attach an
/// [`EarthOrientationProvider`], while existing force models and default
/// propagation remain bit-identical.
#[derive(Clone, Default)]
pub struct PropagationContext {
    body_fixed_frame_provider: Option<Arc<dyn EarthOrientationProvider>>,
}

impl core::fmt::Debug for PropagationContext {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PropagationContext")
            .field(
                "body_fixed_frame_provider",
                &self.body_fixed_frame_provider.is_some(),
            )
            .finish()
    }
}

impl PropagationContext {
    /// Build an empty propagation context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a body-fixed frame provider.
    pub fn with_body_fixed_frame_provider(
        mut self,
        provider: Arc<dyn EarthOrientationProvider>,
    ) -> Self {
        self.body_fixed_frame_provider = Some(provider);
        self
    }

    /// Return the body-fixed frame provider, if one was attached.
    pub fn body_fixed_frame_provider(&self) -> Option<&dyn EarthOrientationProvider> {
        self.body_fixed_frame_provider
            .as_deref()
            .map(|provider| provider as &dyn EarthOrientationProvider)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntegratorOptions {
    pub abs_tol: f64,
    pub rel_tol: f64,
    pub min_step: f64,
    pub max_step: f64,
    pub initial_step: f64,
    pub max_steps: u32,
    pub dense_output: bool,
}

impl Default for IntegratorOptions {
    fn default() -> Self {
        Self {
            abs_tol: 1e-9,
            rel_tol: 1e-12,
            min_step: 1e-6,
            max_step: SECONDS_PER_HOUR,
            initial_step: 60.0,
            max_steps: 1_000_000,
            dense_output: false,
        }
    }
}

pub(crate) fn validate_integrator_options(
    opts: &IntegratorOptions,
) -> Result<(), PropagationError> {
    validate_step_options(opts)
}

pub(crate) fn validate_adaptive_integrator_options(
    opts: &IntegratorOptions,
) -> Result<(), PropagationError> {
    validate_step_options(opts)?;
    crate::validate::finite_positive(opts.abs_tol, "abs_tol").map_err(map_field_error)?;
    crate::validate::finite_positive(opts.rel_tol, "rel_tol").map_err(map_field_error)?;
    Ok(())
}

pub(crate) fn validate_integrator_epoch(
    value: f64,
    field: &'static str,
) -> Result<(), PropagationError> {
    crate::validate::finite(value, field)
        .map(|_| ())
        .map_err(map_field_error)
}

fn validate_step_options(opts: &IntegratorOptions) -> Result<(), PropagationError> {
    crate::validate::positive_step(opts.initial_step, "initial_step").map_err(map_field_error)?;
    crate::validate::positive_step(opts.min_step, "min_step").map_err(map_field_error)?;
    crate::validate::positive_step(opts.max_step, "max_step").map_err(map_field_error)?;
    Ok(())
}

fn map_field_error(error: crate::validate::FieldError) -> PropagationError {
    PropagationError::InvalidInput(format!("{} {}", error.field(), error.reason()))
}
