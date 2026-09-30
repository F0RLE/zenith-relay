use super::super::sqlite::{db_error, Store};
use super::validation::{
    model_reasoning_allowed_levels_from_metadata, normalize_validated_model_ids,
    validate_quota_request_timeout,
};
#[cfg(test)]
use super::SourcePriceOverrides;
use super::DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS;
#[cfg(test)]
use crate::state::identity_hint;
use rusqlite::{params, TransactionBehavior};
use std::collections::BTreeMap;
#[cfg(test)]
use zenith_relay_core::ApiModelPriceSources;
use zenith_relay_core::{
    normalize_model_ids, normalize_model_price_overrides, normalize_model_reasoning_allowed_levels,
    normalize_model_service_tier_overrides, ApiModelPriceOverride, DefaultServiceTier,
};

impl Store {
    pub fn common_proxy_configured(&self) -> Result<bool, String> {
        Ok(self
            .metadata("common_proxy_configured")?
            .is_some_and(|value| value == "true"))
    }

    pub fn set_common_proxy_configured(&self, configured: bool) -> Result<(), String> {
        self.set_metadata(
            "common_proxy_configured",
            if configured { "true" } else { "false" },
        )
    }

    pub fn common_proxy_id(&self) -> Result<Option<String>, String> {
        Ok(self
            .metadata("common_proxy_id")?
            .filter(|value| !value.is_empty()))
    }

    pub fn set_common_proxy_id(&self, proxy_id: Option<&str>) -> Result<(), String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        for (key, value) in [
            ("common_proxy_id", proxy_id.unwrap_or_default()),
            (
                "common_proxy_configured",
                if proxy_id.is_some() { "true" } else { "false" },
            ),
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

    pub fn account_proxy_required(&self) -> Result<bool, String> {
        Ok(self
            .metadata("account_proxy_required")?
            .is_some_and(|value| value == "true"))
    }

    pub fn set_account_proxy_required(&self, required: bool) -> Result<(), String> {
        self.set_metadata(
            "account_proxy_required",
            if required { "true" } else { "false" },
        )
    }

    pub fn quota_request_timeout_seconds(&self) -> Result<u64, String> {
        let timeout = self.metadata("quota_request_timeout_seconds")?.map_or(
            Ok(DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS),
            |value| {
                value
                    .parse::<u64>()
                    .map_err(|_| "quota request timeout is invalid".to_string())
            },
        )?;
        validate_quota_request_timeout(timeout)?;
        Ok(timeout)
    }

    pub fn hidden_models(&self) -> Result<Vec<String>, String> {
        let value = self
            .metadata("hidden_model_ids")?
            .unwrap_or_else(|| "[]".to_string());
        normalize_validated_model_ids(
            serde_json::from_str(&value).map_err(|_| "hidden model list is invalid".to_string())?,
        )
    }

    pub fn set_hidden_models(&self, models: Vec<String>) -> Result<(), String> {
        let models = normalize_validated_model_ids(models)?;
        self.set_metadata(
            "hidden_model_ids",
            &serde_json::to_string(&models)
                .map_err(|_| "hidden model list serialization failed".to_string())?,
        )
    }

    pub fn model_price_overrides(&self) -> Result<BTreeMap<String, ApiModelPriceOverride>, String> {
        let value = self
            .metadata("model_price_overrides")?
            .unwrap_or_else(|| "{}".to_string());
        normalize_model_price_overrides(
            serde_json::from_str(&value)
                .map_err(|_| "model price overrides are invalid".to_string())?,
        )
        .map_err(str::to_string)
    }

    pub fn set_model_price_overrides(
        &self,
        overrides: BTreeMap<String, ApiModelPriceOverride>,
    ) -> Result<(), String> {
        let overrides = normalize_model_price_overrides(overrides)?;
        self.set_metadata(
            "model_price_overrides",
            &serde_json::to_string(&overrides)
                .map_err(|_| "model price overrides could not be serialized".to_string())?,
        )
    }

    pub fn model_reasoning_allowed_levels(&self) -> Result<BTreeMap<String, Vec<String>>, String> {
        model_reasoning_allowed_levels_from_metadata(
            self.metadata("model_reasoning_allowed_levels")?,
            self.metadata("model_reasoning_overrides")?,
        )
    }

    pub fn set_model_reasoning_allowed_levels(
        &self,
        allowed_levels: BTreeMap<String, Vec<String>>,
    ) -> Result<(), String> {
        let allowed_levels = normalize_model_reasoning_allowed_levels(allowed_levels)?;
        self.set_metadata(
            "model_reasoning_allowed_levels",
            &serde_json::to_string(&allowed_levels).map_err(|_| {
                "model reasoning allowed levels could not be serialized".to_string()
            })?,
        )
    }

    pub fn model_service_tier_overrides(
        &self,
    ) -> Result<BTreeMap<String, DefaultServiceTier>, String> {
        let value = self
            .metadata("model_service_tier_overrides")?
            .unwrap_or_else(|| "{}".to_string());
        normalize_model_service_tier_overrides(
            serde_json::from_str(&value)
                .map_err(|_| "model service tier overrides are invalid".to_string())?,
        )
        .map_err(str::to_string)
    }

    pub fn set_model_service_tier_overrides(
        &self,
        overrides: BTreeMap<String, DefaultServiceTier>,
    ) -> Result<(), String> {
        let overrides =
            normalize_model_service_tier_overrides(overrides).map_err(str::to_string)?;
        self.set_metadata(
            "model_service_tier_overrides",
            &serde_json::to_string(&overrides)
                .map_err(|_| "model service tier overrides could not be serialized".to_string())?,
        )
    }

    pub fn model_display_order(&self) -> Result<Vec<String>, String> {
        let value = self
            .metadata("model_display_order")?
            .unwrap_or_else(|| "[]".to_string());
        Ok(normalize_model_ids(
            serde_json::from_str::<Vec<String>>(&value)
                .map_err(|_| "model display order is invalid".to_string())?,
        ))
    }

    pub fn set_model_display_order(&self, models: Vec<String>) -> Result<(), String> {
        let models = normalize_model_ids(models);
        self.set_metadata(
            "model_display_order",
            &serde_json::to_string(&models)
                .map_err(|_| "model display order could not be serialized".to_string())?,
        )
    }

    #[cfg(test)]
    pub(in crate::store) fn source_price_overrides(&self) -> Result<SourcePriceOverrides, String> {
        self.sources()?
            .into_iter()
            .map(|source| {
                let manual = normalize_model_price_overrides(source.model_price_overrides)?;
                let provider = normalize_model_price_overrides(source.detected_model_prices)?;
                let mut prices = BTreeMap::new();
                for model in manual.keys().chain(provider.keys()) {
                    prices
                        .entry(model.clone())
                        .or_insert_with(|| ApiModelPriceSources {
                            provider: provider.get(model).copied(),
                            manual: manual.get(model).copied(),
                        });
                }
                Ok((identity_hint(&source.id), prices))
            })
            .collect()
    }
}
