use super::*;

pub(super) fn write_configuration(
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
        record.pricing_provider = rule.pricing_provider.clone();
        record.official_provider_family = rule.official_provider_family.clone();
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
    let default_service_tier = match settings.routing.default_service_tier {
        DefaultServiceTier::Standard => "standard",
        DefaultServiceTier::Fast => "fast",
        DefaultServiceTier::Ultrafast => "ultrafast",
    };
    let metadata = [
        (
            "pool_routing",
            to_json(&settings.routing.pool_routing).map_err(ConfigurationReplaceError::Store)?,
        ),
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
        (
            "tool_policy",
            to_json(&settings.routing.tool_policy.clone().unwrap_or_default())
                .map_err(ConfigurationReplaceError::Store)?,
        ),
        (
            "basis_points_enabled",
            settings.routing.basis_points_enabled.to_string(),
        ),
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
            "hidden_model_ids",
            to_json(&settings.hidden_models).map_err(ConfigurationReplaceError::Store)?,
        ),
        (
            "model_price_overrides",
            to_json(&settings.model_price_overrides).map_err(ConfigurationReplaceError::Store)?,
        ),
        (
            "model_reasoning_allowed_levels",
            to_json(&settings.model_reasoning_allowed_levels)
                .map_err(ConfigurationReplaceError::Store)?,
        ),
        (
            "model_service_tier_overrides",
            to_json(&settings.model_service_tier_overrides)
                .map_err(ConfigurationReplaceError::Store)?,
        ),
        (
            "model_display_order",
            to_json(&settings.model_display_order).map_err(ConfigurationReplaceError::Store)?,
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

pub(super) fn update_record(
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

pub(super) fn list_records_from<T: DeserializeOwned>(
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

pub(super) fn metadata_from(connection: &Connection, key: &str) -> Result<Option<String>, String> {
    connection
        .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(db_error)
}

pub(super) fn routing_policy_from_connection(
    connection: &Connection,
) -> Result<PresetRoutingPolicy, String> {
    let max_retry_candidates = metadata_from(connection, "max_retry_candidates")?.map_or(
        Ok(DEFAULT_MAX_RETRY_CANDIDATES),
        |value| {
            value
                .parse::<u8>()
                .map_err(|_| "max retry candidates is invalid".to_string())
        },
    )?;
    let pool_routing: Option<zenith_relay_core::PoolRoutingPolicy> =
        metadata_from(connection, "pool_routing")?
            .map(|value| {
                serde_json::from_str(&value)
                    .map_err(|_| "pool routing policy is invalid".to_string())
            })
            .transpose()?
            .flatten();
    if let Some(pool) = &pool_routing {
        pool.validate().map_err(str::to_string)?;
    }
    let default_service_tier = match metadata_from(connection, "default_service_tier")?.as_deref() {
        None | Some("standard") => DefaultServiceTier::Standard,
        Some("fast") | Some("priority") => DefaultServiceTier::Fast,
        Some("ultrafast") => DefaultServiceTier::Ultrafast,
        Some(_) => return Err("default service tier is invalid".to_string()),
    };
    let image_base_model =
        normalize_image_base_model(metadata_from(connection, "image_base_model")?)
            .map_err(|error| error.to_string())?;
    let basis_points_enabled =
        metadata_from(connection, "basis_points_enabled")?.is_some_and(|value| value == "true");
    validate_routing_policy(max_retry_candidates)?;
    Ok(PresetRoutingPolicy {
        tool_policy: Some(
            metadata_from(connection, "tool_policy")?
                .map(|value| {
                    serde_json::from_str::<zenith_relay_core::ToolPolicy>(&value)
                        .map_err(|_| "stored tool policy is invalid".to_string())
                })
                .transpose()?
                .unwrap_or_default()
                .normalized()
                .map_err(str::to_string)?,
        ),
        pool_routing,
        basis_points_enabled,
        max_retry_candidates,
        default_service_tier,
        image_base_model,
    })
}

pub(super) fn configuration_replace_message(error: ConfigurationReplaceError) -> String {
    match error {
        ConfigurationReplaceError::Stale { current_revision } => {
            format!("configuration revision is stale: {current_revision}")
        }
        ConfigurationReplaceError::Invalid(message) | ConfigurationReplaceError::Store(message) => {
            message
        }
    }
}
