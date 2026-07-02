use sidereon::sgp4::{FitConfig, Loss as Sgp4Loss, XScale as Sgp4XScale};
use sidereon::{Loss, XScale};

#[test]
fn facade_reexports_fit_loss_and_x_scale_types() {
    let config = FitConfig {
        loss: Loss::Huber,
        x_scale: Some(XScale::Unit),
        ..FitConfig::default()
    };

    assert_eq!(config.loss, Sgp4Loss::Huber);
    assert_eq!(config.x_scale, Some(Sgp4XScale::Unit));
}
