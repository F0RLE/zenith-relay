use super::GatewayRuntime;
use crate::DefaultServiceTier;

impl GatewayRuntime {
    pub fn model_supports_service_tier(&self, model: &str, tier: DefaultServiceTier) -> bool {
        self.model_supported_service_tiers(model).contains(&tier)
    }

    pub(crate) fn model_supported_service_tiers(
        &self,
        model: &str,
    ) -> &'static [DefaultServiceTier] {
        self.model_metadata_catalog.as_ref().map_or_else(
            || crate::catalog::model_service_tiers(model, None),
            |catalog| catalog.snapshot().service_tiers_for(model),
        )
    }

    /// Applying a pool preference is a model policy. Retrying on another
    /// account or source never downgrades the selected mode.
    pub(crate) fn project_service_tier_for_model(
        &self,
        model: &str,
        requested: DefaultServiceTier,
    ) -> DefaultServiceTier {
        if self.model_supports_service_tier(model, requested) {
            requested
        } else {
            DefaultServiceTier::Standard
        }
    }
}
