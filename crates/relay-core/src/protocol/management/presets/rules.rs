use std::collections::{BTreeMap, BTreeSet};

pub(super) fn validate_unique_preset_member_ids<T, F>(
    members: &[T],
    id: F,
    kind: &str,
) -> Result<(), String>
where
    F: Fn(&T) -> &String,
{
    let unique_count = members.iter().map(id).collect::<BTreeSet<_>>().len();
    if unique_count != members.len() {
        return Err(format!(
            "configuration preset resolves multiple {kind} rules to one local {kind}"
        ));
    }
    Ok(())
}

pub(super) fn replace_preset_members<T, F>(
    existing_members: &mut [T],
    requested: &[T],
    id: F,
    kind: &str,
) -> Result<(), String>
where
    T: Clone,
    F: Fn(&T) -> &String,
{
    let indexes = existing_members
        .iter()
        .enumerate()
        .map(|(index, rule)| (id(rule).clone(), index))
        .collect::<BTreeMap<_, _>>();
    for rule in requested {
        let member_id = id(rule);
        let index = indexes
            .get(member_id)
            .copied()
            .ok_or_else(|| format!("referenced {kind} {member_id} does not exist"))?;
        existing_members[index] = rule.clone();
    }
    Ok(())
}

pub(super) fn normalize_source_preset_rules(
    rules: &mut [super::SourcePresetRule],
) -> Result<(), String> {
    if rules.len() > super::MAX_PRESET_MEMBERS {
        return Err("configuration preset contains too many sources".into());
    }
    let mut source_ids = BTreeSet::new();
    for rule in rules.iter_mut() {
        validate_preset_reference(&rule.id, "source")?;
        rule.name = rule.name.trim().to_string();
        rule.base_url = rule.base_url.trim().trim_end_matches('/').to_string();
        let valid_url =
            url::Url::parse(&rule.base_url).is_ok_and(|url| crate::is_http_endpoint(&url));
        if !source_ids.insert(rule.id.clone())
            || rule.weight == 0
            || rule.recovery_delay_seconds > crate::MAX_SOURCE_RECOVERY_DELAY_SECONDS
            || rule.name.is_empty()
            || rule.name.len() > 256
            || rule.name.chars().any(char::is_control)
            || !valid_url
        {
            return Err("configuration preset source rule is invalid".into());
        }
        rule.allowed_models = normalize_preset_model_ids(std::mem::take(&mut rule.allowed_models))?;
        rule.excluded_models =
            normalize_preset_model_ids(std::mem::take(&mut rule.excluded_models))?;
        rule.model_price_overrides =
            super::normalize_model_price_overrides(std::mem::take(&mut rule.model_price_overrides))
                .map_err(|message| format!("configuration preset {message}"))?;
        if rule
            .legacy_protocol_mode
            .as_deref()
            .is_some_and(|mode| !matches!(mode, "auto" | "manual"))
        {
            return Err("configuration preset protocol mode is invalid".into());
        }
        if !rule.protocol_bindings.is_empty() {
            rule.protocol_bindings = super::normalize_source_protocol_bindings(
                std::mem::take(&mut rule.protocol_bindings),
                rule.wire_api,
                &[],
            )
            .map_err(|error| error.to_string())?;
        }
    }
    rules.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(())
}

pub(super) fn normalize_account_preset_rules(
    rules: &mut [super::AccountPresetRule],
) -> Result<(), String> {
    if rules.len() > super::MAX_PRESET_MEMBERS {
        return Err("configuration preset contains too many accounts".into());
    }
    let mut account_ids = BTreeSet::new();
    for rule in rules.iter_mut() {
        validate_preset_reference(&rule.id, "account")?;
        if !account_ids.insert(rule.id.clone())
            || rule.weight == 0
            || invalid_preset_reference(&rule.identity_hint)
            || rule
                .proxy_id
                .as_deref()
                .is_some_and(invalid_preset_reference)
            || rule.proxy_id.is_some() && rule.bypass_common_proxy
        {
            return Err("configuration preset account rule is invalid".into());
        }
        rule.allowed_models = normalize_preset_model_ids(std::mem::take(&mut rule.allowed_models))?;
        rule.excluded_models =
            normalize_preset_model_ids(std::mem::take(&mut rule.excluded_models))?;
    }
    rules.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(())
}

fn validate_preset_reference(reference_value: &str, kind: &str) -> Result<(), String> {
    if invalid_preset_reference(reference_value) {
        return Err(format!("configuration preset {kind} reference is invalid"));
    }
    Ok(())
}

fn invalid_preset_reference(reference_value: &str) -> bool {
    reference_value.is_empty()
        || reference_value.len() > 128
        || !reference_value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(super) fn normalize_preset_model_ids(models: Vec<String>) -> Result<Vec<String>, String> {
    crate::normalize_bounded_model_ids(models, crate::MAX_MODEL_LIST_LEN).map_err(|error| {
        match error {
            crate::ModelIdListError::TooLarge => "configuration preset model list is too large",
            crate::ModelIdListError::InvalidId => "configuration preset model id is invalid",
        }
        .into()
    })
}
