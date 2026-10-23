use super::super::{persistence, LocalPoolStore, STATE_GATEWAY, STATE_REFRESH_REVISIONS};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::{AutomationRecords, GatewaySettings},
};
use zenith_relay_core::normalize_image_base_model;

impl LocalPoolStore {
    pub fn replace_gateway(&mut self, mut gateway: GatewaySettings) -> Result<()> {
        gateway.hidden_models = crate::local_pool::models::normalized_values(gateway.hidden_models);
        gateway.model_price_overrides = gateway
            .model_price_overrides
            .into_iter()
            .map(|(model, price)| (zenith_relay_core::model_id_key(&model), price))
            .collect();
        gateway.model_reasoning_allowed_levels =
            zenith_relay_core::normalize_model_reasoning_allowed_levels(
                gateway.model_reasoning_allowed_levels,
            )
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        gateway.model_service_tier_overrides =
            zenith_relay_core::normalize_model_service_tier_overrides(
                gateway.model_service_tier_overrides,
            )
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        gateway.model_context_policies =
            zenith_relay_core::normalize_model_context_policies(gateway.model_context_policies)
                .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        gateway.model_display_order =
            zenith_relay_core::normalize_model_ids(gateway.model_display_order);
        gateway.image_base_model = normalize_image_base_model(gateway.image_base_model)
            .map_err(LocalPoolError::invalid_state)?;
        if gateway == self.gateway {
            return Ok(());
        }
        gateway
            .validate()
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        let revisions = self
            .refresh_revisions
            .with_gateway(&self.gateway, &gateway)?;
        let mut state_entries = vec![(STATE_GATEWAY, persistence::serialize_state(&gateway)?)];
        if revisions != self.refresh_revisions {
            state_entries.push((
                STATE_REFRESH_REVISIONS,
                persistence::serialize_state(&revisions)?,
            ));
        }
        self.database.replace_state_json(&state_entries)?;
        let refresh_changed = revisions != self.refresh_revisions;
        self.gateway = gateway;
        self.refresh_revisions = revisions;
        if refresh_changed {
            self.notify_refresh_changed();
        }
        Ok(())
    }

    pub fn set_gateway_enabled(&mut self, enabled: bool) -> Result<()> {
        let mut updated_gateway = self.gateway.clone();
        updated_gateway.enabled = enabled;
        self.replace_gateway(updated_gateway)
    }

    pub fn reset_local_records(&mut self) -> Result<()> {
        let trashed_ids = self
            .account_trash
            .iter()
            .map(|entry| entry.record.account.id.clone())
            .collect::<Vec<_>>();
        if !trashed_ids.is_empty() {
            self.purge_trashed_accounts(&trashed_ids)?;
        }
        self.replace_gateway(GatewaySettings::default())?;
        self.replace_all_records(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            AutomationRecords::default(),
        )
    }
}
