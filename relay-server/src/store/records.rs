use super::sqlite::{db_error, parse_json, to_json, Store};
use crate::state::{
    identity_hint, GatewayKeyRecord, ServerAccountRecord, ServerProxyRecord, SourceRecord,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use zenith_relay_core::{
    normalize_image_base_model, normalize_model_price_overrides, normalize_subscription_plan_order,
    protocol::{
        AccountPresetRule, ConfigurationPresetSettings, PresetQuotaPolicy, PresetRoutingPolicy,
        SourcePresetRule,
    },
    ApiModelPriceOverride, DefaultServiceTier, RoutingStrategy, DEFAULT_COOLDOWN_AFTER_FAILURES,
    DEFAULT_KEEP_LAST_CANDIDATE_AVAILABLE,
};

pub const DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS: u64 = 20;

pub const MIN_QUOTA_REQUEST_TIMEOUT_SECONDS: u64 = 10;

pub const MAX_QUOTA_REQUEST_TIMEOUT_SECONDS: u64 = 20;

pub const DEFAULT_MAX_RETRY_CANDIDATES: u8 = 3;

pub(super) type SourcePriceOverrides = BTreeMap<String, BTreeMap<String, ApiModelPriceOverride>>;

pub type RoutingPolicy = (
    u8,
    RoutingStrategy,
    DefaultServiceTier,
    Option<String>,
    Vec<String>,
    u8,
    bool,
);

#[derive(Debug)]
pub enum ConfigurationReplaceError {
    Stale { current_revision: String },
    Invalid(String),
    Store(String),
}

pub struct ConfigurationReplacement {
    pub previous: ConfigurationPresetSettings,
    pub previous_revision: String,
    pub revision: String,
}

impl Store {
    pub fn gateway_enabled(&self) -> Result<bool, String> {
        Ok(self
            .metadata("gateway_enabled")?
            .is_none_or(|value| value == "true"))
    }

    pub fn set_gateway_enabled(&self, enabled: bool) -> Result<(), String> {
        self.set_metadata("gateway_enabled", if enabled { "true" } else { "false" })
    }

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
        transaction.commit().map_err(db_error)
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

    pub fn routing_policy(&self) -> Result<RoutingPolicy, String> {
        let max_retry_candidates = self.metadata("max_retry_candidates")?.map_or(
            Ok(DEFAULT_MAX_RETRY_CANDIDATES),
            |value| {
                value
                    .parse::<u8>()
                    .map_err(|_| "max retry candidates is invalid".to_string())
            },
        )?;
        let routing_strategy = match self.metadata("routing_strategy")?.as_deref() {
            None | Some("adaptive") => RoutingStrategy::Adaptive,
            Some("quota_highest") => RoutingStrategy::QuotaHighest,
            Some("subscription_expiry") => RoutingStrategy::SubscriptionExpiry,
            Some("subscription_plan") => RoutingStrategy::SubscriptionPlan,
            Some(_) => return Err("routing strategy is invalid".to_string()),
        };
        let default_service_tier = match self.metadata("default_service_tier")?.as_deref() {
            None | Some("standard") => DefaultServiceTier::Standard,
            Some("fast") => DefaultServiceTier::Fast,
            Some(_) => return Err("default service tier is invalid".to_string()),
        };
        let image_base_model = normalize_image_base_model(self.metadata("image_base_model")?)
            .map_err(|error| error.to_string())?;
        let subscription_plan_order =
            self.metadata("subscription_plan_order")?
                .map_or(Ok(Vec::new()), |value| {
                    serde_json::from_str::<Vec<String>>(&value)
                        .map_err(|_| "subscription plan order is invalid".to_string())
                })?;
        let subscription_plan_order =
            normalize_subscription_plan_order(subscription_plan_order).map_err(str::to_string)?;
        let cooldown_after_failures = self.metadata("cooldown_after_failures")?.map_or(
            Ok(DEFAULT_COOLDOWN_AFTER_FAILURES),
            |value| {
                value
                    .parse::<u8>()
                    .map_err(|_| "cooldown after failures is invalid".to_string())
            },
        )?;
        let keep_last_candidate_available = self
            .metadata("keep_last_candidate_available")?
            .map_or(DEFAULT_KEEP_LAST_CANDIDATE_AVAILABLE, |value| {
                value == "true"
            });
        validate_routing_policy(max_retry_candidates, cooldown_after_failures)?;
        Ok((
            max_retry_candidates,
            routing_strategy,
            default_service_tier,
            image_base_model,
            subscription_plan_order,
            cooldown_after_failures,
            keep_last_candidate_available,
        ))
    }

    pub fn set_routing_policy(&self, policy: &PresetRoutingPolicy) -> Result<(), String> {
        validate_routing_policy(policy.max_retry_candidates, policy.cooldown_after_failures)?;
        let image_base_model = normalize_image_base_model(policy.image_base_model.clone())
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        let subscription_plan_order =
            normalize_subscription_plan_order(policy.subscription_plan_order.clone())
                .map_err(str::to_string)?;
        let subscription_plan_order = serde_json::to_string(&subscription_plan_order)
            .map_err(|_| "subscription plan order is invalid".to_string())?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        for (key, value) in [
            (
                "max_retry_candidates",
                policy.max_retry_candidates.to_string(),
            ),
            (
                "routing_strategy",
                match policy.routing_strategy {
                    RoutingStrategy::Adaptive => "adaptive".to_string(),
                    RoutingStrategy::QuotaHighest => "quota_highest".to_string(),
                    RoutingStrategy::SubscriptionExpiry => "subscription_expiry".to_string(),
                    RoutingStrategy::SubscriptionPlan => "subscription_plan".to_string(),
                },
            ),
            (
                "default_service_tier",
                match policy.default_service_tier {
                    DefaultServiceTier::Standard => "standard".to_string(),
                    DefaultServiceTier::Fast => "fast".to_string(),
                },
            ),
            ("image_base_model", image_base_model),
            ("subscription_plan_order", subscription_plan_order),
            (
                "cooldown_after_failures",
                policy.cooldown_after_failures.to_string(),
            ),
            (
                "keep_last_candidate_available",
                policy.keep_last_candidate_available.to_string(),
            ),
        ] {
            transaction
                .execute(
                    "INSERT INTO metadata(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![key, value],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)
    }

    pub fn hidden_models(&self) -> Result<Vec<String>, String> {
        let value = self
            .metadata("hidden_model_ids")?
            .unwrap_or_else(|| "[]".to_string());
        normalize_model_ids(
            serde_json::from_str(&value).map_err(|_| "hidden model list is invalid".to_string())?,
        )
    }

    pub fn set_hidden_models(&self, models: Vec<String>) -> Result<(), String> {
        let models = normalize_model_ids(models)?;
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

    pub fn sources(&self) -> Result<Vec<SourceRecord>, String> {
        self.list_records("sources")
    }

    pub(super) fn source_price_overrides(&self) -> Result<SourcePriceOverrides, String> {
        self.sources()?
            .into_iter()
            .map(|source| {
                Ok((
                    identity_hint(&source.id),
                    normalize_model_price_overrides(source.model_price_overrides)?,
                ))
            })
            .collect()
    }

    pub fn save_source(&self, record: &SourceRecord) -> Result<(), String> {
        self.save_record("sources", &record.id, &record.secret_ref, record)
    }

    pub fn delete_source(&self, id: &str) -> Result<Option<SourceRecord>, String> {
        self.delete_record("sources", id)
    }

    pub fn accounts(&self) -> Result<Vec<ServerAccountRecord>, String> {
        self.list_records("accounts")
    }

    pub fn account(&self, id: &str) -> Result<Option<ServerAccountRecord>, String> {
        self.lock()?
            .query_row(
                "SELECT data_json FROM accounts WHERE id = ?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error)?
            .map(|value| parse_json(&value))
            .transpose()
    }

    pub fn save_account(&self, record: &ServerAccountRecord) -> Result<(), String> {
        self.save_record("accounts", &record.id, &record.secret_ref, record)
    }

    pub fn save_accounts(&self, records: &[ServerAccountRecord]) -> Result<(), String> {
        let encoded = records
            .iter()
            .map(|record| Ok((record, to_json(record)?)))
            .collect::<Result<Vec<_>, String>>()?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        {
            let mut statement = transaction
                .prepare(
                    "INSERT INTO accounts(id, data_json, secret_ref) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data_json=excluded.data_json, secret_ref=excluded.secret_ref",
                )
                .map_err(db_error)?;
            for (record, data_json) in encoded {
                statement
                    .execute(params![record.id, data_json, record.secret_ref])
                    .map_err(db_error)?;
            }
        }
        transaction.commit().map_err(db_error)
    }

    pub fn reset_quota_economics_learning(&self) -> Result<(), String> {
        let mut accounts = self.accounts()?;
        for account in &mut accounts {
            account.economics.reset_learning();
            account
                .economics
                .set_value_revision(zenith_relay_core::quota::quota_valuation_revision());
        }
        self.save_accounts(&accounts)
    }

    pub fn delete_account(&self, id: &str) -> Result<Option<ServerAccountRecord>, String> {
        let candidate_hint = hex::encode(Sha256::digest(id.as_bytes()))[..12].to_string();
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let json = transaction
            .query_row(
                "SELECT data_json FROM accounts WHERE id = ?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error)?;
        if json.is_some() {
            transaction
                .execute("DELETE FROM accounts WHERE id = ?1", [id])
                .map_err(db_error)?;
            transaction
                .execute(
                    "INSERT INTO usage_request_tombstones(request_id, archived_at_ms)
                     SELECT request_id, created_at_ms FROM usage_events
                     WHERE candidate_kind = 'account' AND candidate_hint = ?1
                     ON CONFLICT(request_id) DO UPDATE SET
                        archived_at_ms = MAX(usage_request_tombstones.archived_at_ms, excluded.archived_at_ms)",
                    [&candidate_hint],
                )
                .map_err(db_error)?;
            transaction
                .execute(
                    "DELETE FROM usage_events
                     WHERE candidate_kind = 'account' AND candidate_hint = ?1",
                    [&candidate_hint],
                )
                .map_err(db_error)?;
            transaction
                .execute(
                    "DELETE FROM usage_candidate_rollups
                     WHERE candidate_kind = 'account' AND candidate_id = ?1",
                    [&candidate_hint],
                )
                .map_err(db_error)?;
            transaction
                .execute(
                    "DELETE FROM response_affinity WHERE candidate_id = ?1",
                    [id],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        json.map(|value| parse_json(&value)).transpose()
    }

    pub fn proxies(&self) -> Result<Vec<ServerProxyRecord>, String> {
        self.list_records("proxies")
    }

    pub fn proxy(&self, id: &str) -> Result<Option<ServerProxyRecord>, String> {
        self.lock()?
            .query_row("SELECT data_json FROM proxies WHERE id = ?1", [id], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .map_err(db_error)?
            .map(|value| parse_json(&value))
            .transpose()
    }

    pub fn save_proxy(&self, record: &ServerProxyRecord) -> Result<(), String> {
        self.save_record("proxies", &record.id, &record.secret_ref, record)
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
        write_configuration(&transaction, settings)?;
        transaction
            .commit()
            .map_err(db_error)
            .map_err(ConfigurationReplaceError::Store)?;
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
        write_configuration(&transaction, settings).map_err(configuration_replace_message)?;
        transaction.commit().map_err(db_error)
    }

    pub fn replace_pool_membership(
        &self,
        sources: &[(String, bool)],
        accounts: &[(String, bool)],
    ) -> Result<(), String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        for (id, in_pool) in sources {
            let changed = transaction
                .execute(
                    "UPDATE sources SET data_json = json_set(data_json, '$.inPool', json(?1)) WHERE id = ?2",
                    params![if *in_pool { "true" } else { "false" }, id],
                )
                .map_err(db_error)?;
            if changed != 1 {
                return Err("pool source not found".to_string());
            }
        }
        for (id, in_pool) in accounts {
            let changed = transaction
                .execute(
                    "UPDATE accounts SET data_json = json_set(data_json, '$.inPool', json(?1)) WHERE id = ?2",
                    params![if *in_pool { "true" } else { "false" }, id],
                )
                .map_err(db_error)?;
            if changed != 1 {
                return Err("pool account not found".to_string());
            }
        }
        transaction.commit().map_err(db_error)
    }

    pub fn keys(&self) -> Result<Vec<GatewayKeyRecord>, String> {
        self.list_records("gateway_keys")
    }

    pub fn save_key(&self, record: &GatewayKeyRecord) -> Result<(), String> {
        self.save_record("gateway_keys", &record.id, &record.secret_ref, record)
    }

    pub fn delete_key(&self, id: &str) -> Result<Option<GatewayKeyRecord>, String> {
        self.delete_record("gateway_keys", id)
    }

    pub fn delete_keys(&self, ids: &[String]) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        for id in ids {
            transaction
                .execute("DELETE FROM gateway_keys WHERE id = ?1", [id])
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)
    }
}

pub fn configuration_revision(settings: &ConfigurationPresetSettings) -> Result<String, String> {
    let encoded = serde_json::to_vec(settings)
        .map_err(|_| "configuration revision could not be calculated".to_string())?;
    Ok(format!("cfg_{}", hex::encode(Sha256::digest(encoded))))
}

fn configuration_settings_from_connection(
    connection: &Connection,
) -> Result<ConfigurationPresetSettings, String> {
    let sources = list_records_from::<SourceRecord>(connection, "sources")?
        .into_iter()
        .map(|record| SourcePresetRule {
            id: record.id,
            name: record.name,
            base_url: record.base_url,
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
    let accounts = list_records_from::<ServerAccountRecord>(connection, "accounts")?
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
    let request_timeout_seconds = metadata_from(connection, "quota_request_timeout_seconds")?
        .map_or(Ok(DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS), |value| {
            value
                .parse::<u64>()
                .map_err(|_| "quota request timeout is invalid".to_string())
        })?;
    validate_quota_request_timeout(request_timeout_seconds)?;
    let max_retry_candidates = metadata_from(connection, "max_retry_candidates")?.map_or(
        Ok(DEFAULT_MAX_RETRY_CANDIDATES),
        |value| {
            value
                .parse::<u8>()
                .map_err(|_| "max retry candidates is invalid".to_string())
        },
    )?;
    let cooldown_after_failures = metadata_from(connection, "cooldown_after_failures")?.map_or(
        Ok(DEFAULT_COOLDOWN_AFTER_FAILURES),
        |value| {
            value
                .parse::<u8>()
                .map_err(|_| "cooldown after failures is invalid".to_string())
        },
    )?;
    let keep_last_candidate_available = metadata_from(connection, "keep_last_candidate_available")?
        .map_or(DEFAULT_KEEP_LAST_CANDIDATE_AVAILABLE, |value| {
            value == "true"
        });
    validate_routing_policy(max_retry_candidates, cooldown_after_failures)?;
    let routing_strategy = match metadata_from(connection, "routing_strategy")?.as_deref() {
        None | Some("adaptive") => RoutingStrategy::Adaptive,
        Some("quota_highest") => RoutingStrategy::QuotaHighest,
        Some("subscription_expiry") => RoutingStrategy::SubscriptionExpiry,
        Some("subscription_plan") => RoutingStrategy::SubscriptionPlan,
        Some(_) => return Err("routing strategy is invalid".to_string()),
    };
    let default_service_tier = match metadata_from(connection, "default_service_tier")?.as_deref() {
        None | Some("standard") => DefaultServiceTier::Standard,
        Some("fast") => DefaultServiceTier::Fast,
        Some(_) => return Err("default service tier is invalid".to_string()),
    };
    let image_base_model =
        normalize_image_base_model(metadata_from(connection, "image_base_model")?)
            .map_err(|error| error.to_string())?;
    let subscription_plan_order =
        metadata_from(connection, "subscription_plan_order")?.map_or(Ok(Vec::new()), |value| {
            serde_json::from_str::<Vec<String>>(&value)
                .map_err(|_| "subscription plan order is invalid".to_string())
        })?;
    let subscription_plan_order =
        normalize_subscription_plan_order(subscription_plan_order).map_err(str::to_string)?;
    let hidden_models = normalize_model_ids(
        metadata_from(connection, "hidden_model_ids")?.map_or(Ok(Vec::new()), |value| {
            serde_json::from_str(&value).map_err(|_| "hidden model list is invalid".to_string())
        })?,
    )?;
    let model_price_overrides = normalize_model_price_overrides(
        metadata_from(connection, "model_price_overrides")?.map_or(
            Ok(BTreeMap::new()),
            |value| {
                serde_json::from_str(&value)
                    .map_err(|_| "model price overrides are invalid".to_string())
            },
        )?,
    )?;
    Ok(ConfigurationPresetSettings {
        sources,
        accounts,
        routing: PresetRoutingPolicy {
            max_retry_candidates,
            cooldown_after_failures,
            keep_last_candidate_available,
            routing_strategy,
            subscription_plan_order,
            default_service_tier,
            image_base_model,
        },
        quota: PresetQuotaPolicy {
            request_timeout_seconds,
            account_proxy_required: metadata_from(connection, "account_proxy_required")?
                .is_some_and(|value| value == "true"),
            common_proxy_id: metadata_from(connection, "common_proxy_id")?
                .filter(|value| !value.is_empty()),
        },
        hidden_models,
        model_price_overrides,
    })
}

fn write_configuration(
    transaction: &Transaction<'_>,
    settings: &ConfigurationPresetSettings,
) -> Result<(), ConfigurationReplaceError> {
    validate_configuration_settings(settings).map_err(ConfigurationReplaceError::Invalid)?;
    let mut sources = list_records_from::<SourceRecord>(transaction, "sources")
        .map_err(ConfigurationReplaceError::Store)?;
    let mut accounts = list_records_from::<ServerAccountRecord>(transaction, "accounts")
        .map_err(ConfigurationReplaceError::Store)?;
    let source_rules = settings
        .sources
        .iter()
        .map(|rule| (rule.id.as_str(), rule))
        .collect::<HashMap<_, _>>();
    let account_rules = settings
        .accounts
        .iter()
        .map(|rule| (rule.id.as_str(), rule))
        .collect::<HashMap<_, _>>();
    if source_rules.len() != sources.len()
        || account_rules.len() != accounts.len()
        || sources
            .iter()
            .any(|record| !source_rules.contains_key(record.id.as_str()))
        || accounts
            .iter()
            .any(|record| !account_rules.contains_key(record.id.as_str()))
    {
        return Err(ConfigurationReplaceError::Invalid(
            "configuration preset object set is incomplete".to_string(),
        ));
    }
    for proxy_id in settings
        .accounts
        .iter()
        .filter_map(|rule| rule.proxy_id.as_deref())
        .chain(settings.quota.common_proxy_id.as_deref())
    {
        let exists = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM proxies WHERE id = ?1)",
                [proxy_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(db_error)
            .map_err(ConfigurationReplaceError::Store)?;
        if !exists {
            return Err(ConfigurationReplaceError::Invalid(format!(
                "referenced proxy {proxy_id} does not exist"
            )));
        }
    }
    for record in &mut sources {
        let rule = source_rules[record.id.as_str()];
        record.enabled = rule.enabled;
        record.in_pool = rule.in_pool;
        record.protocol_bindings = rule.protocol_bindings.clone();
        record.allowed_models = rule.allowed_models.clone();
        record.excluded_models = rule.excluded_models.clone();
        record.priority = rule.priority;
        record.weight = rule.weight;
        record.recovery_delay_seconds = rule.recovery_delay_seconds;
        record.model_price_overrides = rule.model_price_overrides.clone();
        update_record(transaction, "sources", &record.id, record)?;
    }
    for record in &mut accounts {
        let rule = account_rules[record.id.as_str()];
        record.enabled = rule.enabled;
        record.in_pool = rule.in_pool;
        record.allowed_models = rule.allowed_models.clone();
        record.excluded_models = rule.excluded_models.clone();
        record.priority = rule.priority;
        record.weight = rule.weight;
        record.proxy_id = rule.proxy_id.clone();
        record.bypass_common_proxy = rule.bypass_common_proxy;
        update_record(transaction, "accounts", &record.id, record)?;
    }
    let routing_strategy = match settings.routing.routing_strategy {
        RoutingStrategy::Adaptive => "adaptive",
        RoutingStrategy::QuotaHighest => "quota_highest",
        RoutingStrategy::SubscriptionExpiry => "subscription_expiry",
        RoutingStrategy::SubscriptionPlan => "subscription_plan",
    };
    let default_service_tier = match settings.routing.default_service_tier {
        DefaultServiceTier::Standard => "standard",
        DefaultServiceTier::Fast => "fast",
    };
    let metadata = [
        (
            "quota_request_timeout_seconds",
            settings.quota.request_timeout_seconds.to_string(),
        ),
        (
            "account_proxy_required",
            settings.quota.account_proxy_required.to_string(),
        ),
        (
            "common_proxy_id",
            settings.quota.common_proxy_id.clone().unwrap_or_default(),
        ),
        (
            "common_proxy_configured",
            settings.quota.common_proxy_id.is_some().to_string(),
        ),
        (
            "max_retry_candidates",
            settings.routing.max_retry_candidates.to_string(),
        ),
        ("routing_strategy", routing_strategy.to_string()),
        ("default_service_tier", default_service_tier.to_string()),
        (
            "image_base_model",
            settings
                .routing
                .image_base_model
                .clone()
                .unwrap_or_default(),
        ),
        (
            "subscription_plan_order",
            to_json(&settings.routing.subscription_plan_order)
                .map_err(ConfigurationReplaceError::Store)?,
        ),
        (
            "cooldown_after_failures",
            settings.routing.cooldown_after_failures.to_string(),
        ),
        (
            "keep_last_candidate_available",
            settings.routing.keep_last_candidate_available.to_string(),
        ),
        (
            "hidden_model_ids",
            to_json(&settings.hidden_models).map_err(ConfigurationReplaceError::Store)?,
        ),
        (
            "model_price_overrides",
            to_json(&settings.model_price_overrides).map_err(ConfigurationReplaceError::Store)?,
        ),
    ];
    for (key, value) in metadata {
        transaction
            .execute(
                "INSERT INTO metadata(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )
            .map_err(db_error)
            .map_err(ConfigurationReplaceError::Store)?;
    }
    Ok(())
}

fn validate_configuration_settings(settings: &ConfigurationPresetSettings) -> Result<(), String> {
    validate_quota_request_timeout(settings.quota.request_timeout_seconds)?;
    validate_routing_policy(
        settings.routing.max_retry_candidates,
        settings.routing.cooldown_after_failures,
    )?;
    if normalize_subscription_plan_order(settings.routing.subscription_plan_order.clone())
        .map_err(str::to_string)?
        != settings.routing.subscription_plan_order
        || normalize_image_base_model(settings.routing.image_base_model.clone())
            .map_err(|error| error.to_string())?
            != settings.routing.image_base_model
        || normalize_model_ids(settings.hidden_models.clone())? != settings.hidden_models
        || normalize_model_price_overrides(settings.model_price_overrides.clone())?
            != settings.model_price_overrides
    {
        return Err("configuration preset is not normalized".to_string());
    }
    let mut ids = HashSet::new();
    for rule in &settings.sources {
        if rule.id.is_empty()
            || !ids.insert(("source", rule.id.as_str()))
            || rule.weight == 0
            || rule.recovery_delay_seconds > 24 * 60 * 60
            || rule.name.is_empty()
            || rule.name.len() > 256
            || rule.name.chars().any(char::is_control)
            || url::Url::parse(&rule.base_url).is_err()
            || normalize_model_ids(rule.allowed_models.clone())? != rule.allowed_models
            || normalize_model_ids(rule.excluded_models.clone())? != rule.excluded_models
            || normalize_model_price_overrides(rule.model_price_overrides.clone())?
                != rule.model_price_overrides
        {
            return Err("source preset rule is invalid".to_string());
        }
    }
    for rule in &settings.accounts {
        if rule.id.is_empty()
            || !ids.insert(("account", rule.id.as_str()))
            || rule.weight == 0
            || rule.identity_hint.is_empty()
            || rule.identity_hint.len() > 128
            || rule.identity_hint.chars().any(char::is_control)
            || normalize_model_ids(rule.allowed_models.clone())? != rule.allowed_models
            || normalize_model_ids(rule.excluded_models.clone())? != rule.excluded_models
        {
            return Err("account preset rule is invalid".to_string());
        }
    }
    Ok(())
}

fn update_record(
    transaction: &Transaction<'_>,
    table: &str,
    id: &str,
    record: &impl Serialize,
) -> Result<(), ConfigurationReplaceError> {
    let changed = transaction
        .execute(
            &format!("UPDATE {table} SET data_json = ?1 WHERE id = ?2"),
            params![
                to_json(record).map_err(ConfigurationReplaceError::Store)?,
                id
            ],
        )
        .map_err(db_error)
        .map_err(ConfigurationReplaceError::Store)?;
    if changed != 1 {
        return Err(ConfigurationReplaceError::Invalid(
            "referenced configuration object does not exist".to_string(),
        ));
    }
    Ok(())
}

fn list_records_from<T: DeserializeOwned>(
    connection: &Connection,
    table: &str,
) -> Result<Vec<T>, String> {
    let sql = format!("SELECT data_json FROM {table} ORDER BY id");
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error)?;
    rows.map(|row| parse_json(&row.map_err(db_error)?))
        .collect()
}

fn metadata_from(connection: &Connection, key: &str) -> Result<Option<String>, String> {
    connection
        .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(db_error)
}

fn configuration_replace_message(error: ConfigurationReplaceError) -> String {
    match error {
        ConfigurationReplaceError::Stale { current_revision } => {
            format!("configuration revision is stale: {current_revision}")
        }
        ConfigurationReplaceError::Invalid(message) | ConfigurationReplaceError::Store(message) => {
            message
        }
    }
}

fn validate_quota_request_timeout(request_timeout_seconds: u64) -> Result<(), String> {
    if !(MIN_QUOTA_REQUEST_TIMEOUT_SECONDS..=MAX_QUOTA_REQUEST_TIMEOUT_SECONDS)
        .contains(&request_timeout_seconds)
    {
        return Err("quota request timeout is invalid".to_string());
    }
    Ok(())
}

fn validate_routing_policy(
    max_retry_candidates: u8,
    cooldown_after_failures: u8,
) -> Result<(), String> {
    if !(1..=8).contains(&max_retry_candidates) {
        return Err("max retry candidates is invalid".to_string());
    }
    if !(1..=8).contains(&cooldown_after_failures) {
        return Err("cooldown after failures is invalid".to_string());
    }
    Ok(())
}

fn normalize_model_ids(models: Vec<String>) -> Result<Vec<String>, String> {
    if models.len() > 4_096 {
        return Err("model list exceeds the supported limit".to_string());
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for model in models {
        let model = model.trim();
        if model.is_empty() {
            continue;
        }
        if model.len() > 256 || model.chars().any(char::is_control) {
            return Err("model id is invalid".to_string());
        }
        if seen.insert(model.to_ascii_lowercase()) {
            normalized.push(model.to_string());
        }
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::test_root;
    use std::fs;

    #[test]
    fn pool_membership_batch_rolls_back_when_one_record_is_missing() {
        let root = test_root("pool-membership-rollback");
        let store = Store::open(root.join("relay.sqlite")).unwrap();
        {
            let connection = store.lock().unwrap();
            connection
                .execute(
                    "INSERT INTO sources(id, data_json, secret_ref) VALUES ('source_1', '{\"id\":\"source_1\",\"inPool\":false}', 'source:1')",
                    [],
                )
                .unwrap();
        }

        assert!(store
            .replace_pool_membership(
                &[
                    ("source_1".to_string(), true),
                    ("missing".to_string(), true)
                ],
                &[],
            )
            .is_err());
        let in_pool: bool = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT json_extract(data_json, '$.inPool') FROM sources WHERE id = 'source_1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!in_pool);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quota_request_timeout_is_validated_and_persists() {
        let root = test_root("quota-policy");
        let path = root.join("relay.sqlite");
        let store = Store::open(path.clone()).unwrap();
        assert_eq!(store.quota_request_timeout_seconds().unwrap(), 20);
        store
            .set_metadata("quota_request_timeout_seconds", "9")
            .unwrap();
        assert!(store.quota_request_timeout_seconds().is_err());
        store
            .set_metadata("quota_request_timeout_seconds", "10")
            .unwrap();
        drop(store);

        let reopened = Store::open(path).unwrap();
        assert_eq!(reopened.quota_request_timeout_seconds().unwrap(), 10);
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn routing_policy_is_validated_and_persists() {
        let root = test_root("routing-policy");
        let path = root.join("relay.sqlite");
        let store = Store::open(path.clone()).unwrap();
        assert_eq!(
            store.routing_policy().unwrap(),
            (
                3,
                RoutingStrategy::Adaptive,
                DefaultServiceTier::Standard,
                None,
                Vec::new(),
                DEFAULT_COOLDOWN_AFTER_FAILURES,
                DEFAULT_KEEP_LAST_CANDIDATE_AVAILABLE,
            )
        );
        assert!(store
            .set_routing_policy(&PresetRoutingPolicy {
                max_retry_candidates: 0,
                cooldown_after_failures: DEFAULT_COOLDOWN_AFTER_FAILURES,
                keep_last_candidate_available: DEFAULT_KEEP_LAST_CANDIDATE_AVAILABLE,
                routing_strategy: RoutingStrategy::Adaptive,
                subscription_plan_order: Vec::new(),
                default_service_tier: DefaultServiceTier::Standard,
                image_base_model: None,
            })
            .is_err());
        store
            .set_routing_policy(&PresetRoutingPolicy {
                max_retry_candidates: 5,
                cooldown_after_failures: 5,
                keep_last_candidate_available: false,
                routing_strategy: RoutingStrategy::SubscriptionPlan,
                subscription_plan_order: vec!["business".into(), "plus".into()],
                default_service_tier: DefaultServiceTier::Fast,
                image_base_model: Some("gpt-5.4-mini".into()),
            })
            .unwrap();
        drop(store);

        let reopened = Store::open(path).unwrap();
        assert_eq!(
            reopened.routing_policy().unwrap(),
            (
                5,
                RoutingStrategy::SubscriptionPlan,
                DefaultServiceTier::Fast,
                Some("gpt-5.4-mini".into()),
                vec!["business".into(), "plus".into()],
                5,
                false,
            )
        );
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hidden_models_are_validated_deduplicated_and_persisted() {
        let root = test_root("hidden-models");
        let path = root.join("relay.sqlite");
        let store = Store::open(path.clone()).unwrap();
        assert!(store.hidden_models().unwrap().is_empty());
        store
            .set_hidden_models(vec![" gpt-5.4 ".into(), "GPT-5.4".into()])
            .unwrap();
        assert!(store.set_hidden_models(vec!["x\nunsafe".into()]).is_err());
        drop(store);

        let reopened = Store::open(path).unwrap();
        assert_eq!(reopened.hidden_models().unwrap(), ["gpt-5.4"]);
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn model_price_overrides_are_validated_normalized_and_persisted() {
        let root = test_root("model-prices");
        let path = root.join("relay.sqlite");
        let store = Store::open(path.clone()).unwrap();
        let price = ApiModelPriceOverride {
            input_micro_usd_per_million: 1_000_000,
            cached_input_micro_usd_per_million: Some(100_000),
            cache_write_5m_micro_usd_per_million: Some(1_250_000),
            cache_write_1h_micro_usd_per_million: Some(2_500_000),
            output_micro_usd_per_million: 2_000_000,
        };
        store
            .set_model_price_overrides(BTreeMap::from([(" GPT-Test ".into(), price)]))
            .unwrap();
        assert!(store
            .set_model_price_overrides(BTreeMap::from([("unsafe\nmodel".into(), price)]))
            .is_err());
        drop(store);

        let reopened = Store::open(path).unwrap();
        assert_eq!(
            reopened.model_price_overrides().unwrap().get("gpt-test"),
            Some(&price)
        );
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }
}
