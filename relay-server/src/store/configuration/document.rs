use super::super::sqlite::{db_error, parse_json, to_json, Store};
use super::validation::{
    model_reasoning_allowed_levels_from_metadata, normalize_validated_model_ids,
    validate_configuration_settings, validate_quota_request_timeout, validate_routing_policy,
};
use super::{
    ConfigurationReplaceError, ConfigurationReplacement, DEFAULT_MAX_RETRY_CANDIDATES,
    DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS,
};
use crate::state::{ServerAccountRecord, SourceRecord};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use zenith_relay_core::{
    normalize_image_base_model, normalize_model_ids, normalize_model_price_overrides,
    normalize_model_service_tier_overrides,
    protocol::{
        AccountPresetRule, ConfigurationPresetSettings, PresetQuotaPolicy, PresetRoutingPolicy,
        SourcePresetRule,
    },
    DefaultServiceTier,
};

impl Store {
    pub fn routing_policy(&self) -> Result<PresetRoutingPolicy, String> {
        let connection = self.lock()?;
        persist::routing_policy_from_connection(&connection)
    }

    pub fn set_routing_policy(&self, policy: &PresetRoutingPolicy) -> Result<(), String> {
        let tool_policy = policy
            .tool_policy
            .clone()
            .unwrap_or(self.routing_policy()?.tool_policy.unwrap_or_default())
            .normalized()
            .map_err(str::to_string)?;
        let tool_policy = serde_json::to_string(&tool_policy)
            .map_err(|_| "tool policy could not be serialized")?;
        if let Some(pool) = &policy.pool_routing {
            pool.validate().map_err(str::to_string)?;
        }
        let pool_routing = serde_json::to_string(&policy.pool_routing)
            .map_err(|_| "pool routing policy is invalid")?;
        validate_routing_policy(policy.max_retry_candidates)?;
        let image_base_model = normalize_image_base_model(policy.image_base_model.clone())
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        for (key, value) in [
            ("tool_policy", tool_policy),
            ("pool_routing", pool_routing),
            (
                "basis_points_enabled",
                policy.basis_points_enabled.to_string(),
            ),
            (
                "max_retry_candidates",
                policy.max_retry_candidates.to_string(),
            ),
            (
                "default_service_tier",
                match policy.default_service_tier {
                    DefaultServiceTier::Standard => "standard".to_string(),
                    DefaultServiceTier::Fast => "fast".to_string(),
                    DefaultServiceTier::Ultrafast => "ultrafast".to_string(),
                },
            ),
            ("image_base_model", image_base_model),
        ] {
            transaction
                .execute(
                    "INSERT INTO metadata(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![key, value],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        self.notify_refresh_changed();
        Ok(())
    }

    pub fn configuration_settings(&self) -> Result<ConfigurationPresetSettings, String> {
        let connection = self.lock()?;
        configuration_settings_from_connection(&connection)
    }

    pub fn replace_configuration_if_revision(
        &self,
        expected_revision: &str,
        settings: &ConfigurationPresetSettings,
    ) -> Result<ConfigurationReplacement, ConfigurationReplaceError> {
        let mut connection = self.lock().map_err(ConfigurationReplaceError::Store)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)
            .map_err(ConfigurationReplaceError::Store)?;
        let previous = configuration_settings_from_connection(&transaction)
            .map_err(ConfigurationReplaceError::Store)?;
        let previous_revision =
            configuration_revision(&previous).map_err(ConfigurationReplaceError::Store)?;
        if previous_revision != expected_revision {
            return Err(ConfigurationReplaceError::Stale {
                current_revision: previous_revision,
            });
        }
        persist::write_configuration(&transaction, settings)?;
        transaction
            .commit()
            .map_err(db_error)
            .map_err(ConfigurationReplaceError::Store)?;
        self.notify_refresh_changed();
        Ok(ConfigurationReplacement {
            previous,
            previous_revision,
            revision: configuration_revision(settings).map_err(ConfigurationReplaceError::Store)?,
        })
    }

    pub fn restore_configuration(
        &self,
        settings: &ConfigurationPresetSettings,
    ) -> Result<(), String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        persist::write_configuration(&transaction, settings)
            .map_err(persist::configuration_replace_message)?;
        transaction.commit().map_err(db_error)?;
        self.notify_refresh_changed();
        Ok(())
    }
}

pub fn configuration_revision(settings: &ConfigurationPresetSettings) -> Result<String, String> {
    let encoded = serde_json::to_vec(settings)
        .map_err(|_| "configuration revision could not be calculated".to_string())?;
    Ok(format!("cfg_{}", hex::encode(Sha256::digest(encoded))))
}

pub(super) fn configuration_settings_from_connection(
    connection: &Connection,
) -> Result<ConfigurationPresetSettings, String> {
    let sources = persist::list_records_from::<SourceRecord>(connection, "sources")?
        .into_iter()
        .map(|record| SourcePresetRule {
            legacy_protocol_mode: None,
            id: record.id,
            name: record.name,
            base_url: record.base_url,
            pricing_provider: record.pricing_provider,
            official_provider_family: record.official_provider_family,
            wire_api: record.wire_api,
            protocol_bindings: record.protocol_bindings,
            enabled: record.enabled,
            in_pool: record.in_pool,
            allowed_models: record.allowed_models,
            excluded_models: record.excluded_models,
            priority: record.priority,
            weight: record.weight,
            recovery_delay_seconds: record.recovery_delay_seconds,
            model_price_overrides: record.model_price_overrides,
        })
        .collect();
    let accounts = persist::list_records_from::<ServerAccountRecord>(connection, "accounts")?
        .into_iter()
        .map(|record| AccountPresetRule {
            id: record.id,
            identity_hint: record.identity_hint,
            enabled: record.enabled,
            in_pool: record.in_pool,
            allowed_models: record.allowed_models,
            excluded_models: record.excluded_models,
            priority: record.priority,
            weight: record.weight,
            proxy_id: record.proxy_id,
            bypass_common_proxy: record.bypass_common_proxy,
        })
        .collect();
    let request_timeout_seconds = persist::metadata_from(
        connection,
        "quota_request_timeout_seconds",
    )?
    .map_or(Ok(DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS), |value| {
        value
            .parse::<u64>()
            .map_err(|_| "quota request timeout is invalid".to_string())
    })?;
    validate_quota_request_timeout(request_timeout_seconds)?;
    let routing = persist::routing_policy_from_connection(connection)?;
    let hidden_models = normalize_validated_model_ids(
        persist::metadata_from(connection, "hidden_model_ids")?.map_or(
            Ok(Vec::new()),
            |value| {
                serde_json::from_str(&value).map_err(|_| "hidden model list is invalid".to_string())
            },
        )?,
    )?;
    let model_price_overrides = normalize_model_price_overrides(
        persist::metadata_from(connection, "model_price_overrides")?.map_or(
            Ok(BTreeMap::new()),
            |value| {
                serde_json::from_str(&value)
                    .map_err(|_| "model price overrides are invalid".to_string())
            },
        )?,
    )?;
    let model_reasoning_allowed_levels = model_reasoning_allowed_levels_from_metadata(
        persist::metadata_from(connection, "model_reasoning_allowed_levels")?,
        persist::metadata_from(connection, "model_reasoning_overrides")?,
    )?;
    Ok(ConfigurationPresetSettings {
        sources,
        accounts,
        routing,
        quota: PresetQuotaPolicy {
            request_timeout_seconds,
            account_proxy_required: persist::metadata_from(connection, "account_proxy_required")?
                .is_some_and(|value| value == "true"),
            common_proxy_id: persist::metadata_from(connection, "common_proxy_id")?
                .filter(|value| !value.is_empty()),
        },
        hidden_models,
        model_price_overrides,
        model_reasoning_allowed_levels,
        model_reasoning_allowed_levels_present: true,
        model_service_tier_overrides: normalize_model_service_tier_overrides(
            persist::metadata_from(connection, "model_service_tier_overrides")?.map_or(
                Ok(BTreeMap::new()),
                |value| {
                    serde_json::from_str(&value)
                        .map_err(|_| "model service tier overrides are invalid".to_string())
                },
            )?,
        )
        .map_err(str::to_string)?,
        model_display_order: normalize_model_ids(
            persist::metadata_from(connection, "model_display_order")?.map_or(
                Ok(Vec::<String>::new()),
                |value| {
                    serde_json::from_str(&value)
                        .map_err(|_| "model display order is invalid".to_string())
                },
            )?,
        ),
        model_service_tier_overrides_present: true,
        model_display_order_present: true,
    })
}

mod persist;
