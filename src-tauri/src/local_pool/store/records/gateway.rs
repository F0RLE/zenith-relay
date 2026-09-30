use super::super::{
    persistence, LocalPoolStore, STATE_ACCOUNTS, STATE_GATEWAY, STATE_KEYS,
    STATE_OWNERSHIP_OPERATION, STATE_REFRESH_REVISIONS, STATE_REMOTE_TARGET,
};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::{
        AutomationRecords, GatewaySettings, LocalAccountRecord, LocalGatewayKeyRecord,
        OwnershipOperationRecord, RemoteTargetRecord, MAX_LOCAL_ACCOUNTS,
    },
};
use zenith_relay_core::normalize_image_base_model;

impl LocalPoolStore {
    pub fn replace_gateway(&mut self, mut gateway: GatewaySettings) -> Result<()> {
        gateway.tool_policy = gateway
            .tool_policy
            .normalized()
            .map_err(LocalPoolError::invalid_state)?;
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
        let mut values = vec![(STATE_GATEWAY, persistence::serialize_state(&gateway)?)];
        if revisions != self.refresh_revisions {
            values.push((
                STATE_REFRESH_REVISIONS,
                persistence::serialize_state(&revisions)?,
            ));
        }
        self.database.replace_state_json(&values)?;
        let refresh_changed = revisions != self.refresh_revisions;
        self.gateway = gateway;
        self.refresh_revisions = revisions;
        if refresh_changed {
            self.notify_refresh_changed();
        }
        Ok(())
    }

    pub fn replace_remote_target(&mut self, target: Option<RemoteTargetRecord>) -> Result<()> {
        if target == self.remote_target {
            return Ok(());
        }
        self.database
            .replace_state_json(&[(STATE_REMOTE_TARGET, persistence::serialize_state(&target)?)])?;
        self.remote_target = target;
        Ok(())
    }

    pub fn replace_ownership_operation(
        &mut self,
        operation: Option<OwnershipOperationRecord>,
    ) -> Result<()> {
        if let Some(operation) = &operation {
            operation
                .validate()
                .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        }
        if operation == self.ownership_operation {
            return Ok(());
        }
        self.database.replace_state_json(&[(
            STATE_OWNERSHIP_OPERATION,
            persistence::serialize_state(&operation)?,
        )])?;
        self.ownership_operation = operation;
        Ok(())
    }

    pub fn replace_accounts_keys_and_ownership_operation(
        &mut self,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
        operation: Option<OwnershipOperationRecord>,
    ) -> Result<()> {
        if accounts.len() > MAX_LOCAL_ACCOUNTS {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("local account count exceeds the supported limit of {MAX_LOCAL_ACCOUNTS}"),
            ));
        }
        if let Some(operation) = &operation {
            operation
                .validate()
                .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        }
        let revisions = self
            .refresh_revisions
            .with_accounts(&self.accounts, &accounts)?;
        self.database.replace_state_json(&[
            (STATE_ACCOUNTS, persistence::serialize_state(&accounts)?),
            (STATE_KEYS, persistence::serialize_state(&keys)?),
            (
                STATE_OWNERSHIP_OPERATION,
                persistence::serialize_state(&operation)?,
            ),
            (
                STATE_REFRESH_REVISIONS,
                persistence::serialize_state(&revisions)?,
            ),
        ])?;
        let refresh_changed = self.refresh_registration_changed(&revisions, &accounts);
        self.accounts = accounts;
        self.keys = keys;
        self.ownership_operation = operation;
        self.refresh_revisions = revisions;
        if refresh_changed {
            self.notify_refresh_changed();
        }
        Ok(())
    }

    pub fn set_gateway_enabled(&mut self, enabled: bool) -> Result<()> {
        let mut next = self.gateway.clone();
        next.enabled = enabled;
        self.replace_gateway(next)
    }

    pub fn reset_local_records(&mut self) -> Result<()> {
        self.replace_gateway(GatewaySettings::default())?;
        self.replace_all_records(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            AutomationRecords::default(),
        )
    }
}
