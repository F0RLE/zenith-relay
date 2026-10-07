use super::super::*;
use super::document::{desktop_bool, root_model_reasoning_effort};

const ROUTING_KEYS: &[&str] = &[
    "model",
    "review_model",
    "model_catalog_json",
    "chatgpt_base_url",
    "openai_base_url",
];

pub(in crate::local_pool::profiles::codex) fn attach_config(
    document: &mut DocumentMut,
    base_url: &str,
    local_key: &str,
    model_catalog_path: Option<&str>,
    model_reasoning_effort: Option<&str>,
    supports_websockets: bool,
) {
    clear_relay_routing_overrides(document);
    // Codex reads the active effort from its root config, while the managed
    // catalog supplies the model-specific list of valid levels. Keep both in
    // sync when Relay activates a profile.
    remove_unsupported_reasoning_efforts(document);
    restore_root_string(document, "model_reasoning_effort", model_reasoning_effort);
    document["model_provider"] = value(PROVIDER_ID);
    // A Relay attach without a catalog intentionally clears the native or
    // previous Relay catalog. The old value remains in the recovery backup
    // and is restored only when detaching Relay.
    restore_root_string(document, "model_catalog_json", model_catalog_path);
    if document
        .get("model_providers")
        .and_then(Item::as_table)
        .is_none()
    {
        document["model_providers"] = Item::Table(Table::new());
    }
    document["model_providers"][PROVIDER_ID] = Item::Table(Table::new());
    let relay_provider = &mut document["model_providers"][PROVIDER_ID];
    relay_provider["name"] = value("Zenith Relay Local");
    relay_provider["base_url"] = value(base_url);
    relay_provider["wire_api"] = value("responses");
    relay_provider["requires_openai_auth"] = value(true);
    relay_provider["experimental_bearer_token"] = value(local_key);
    relay_provider["supports_websockets"] = value(supports_websockets);
    // Ultra is an orchestration mode. Codex hides it in the model slider
    // until this desktop switch is on; an absent key means off.
    enable_show_ultra_picker(document);
}

/// Remove route state owned by the previous provider before a Relay attach.
pub(in crate::local_pool::profiles::codex) fn clear_relay_routing_overrides(
    document: &mut DocumentMut,
) {
    clear_relay_provider_tables(document);
    remove_root_keys(document, ROUTING_KEYS);
    clear_named_profile_routing_overrides(document, false);
}

/// Remove route state before a native ChatGPT account attach. Native Codex
/// keeps an explicit `openai` provider; Relay and external providers are
/// removed so stale models and catalogs cannot leak into the account.
pub(in crate::local_pool::profiles::codex) fn clear_account_routing_overrides(
    document: &mut DocumentMut,
) {
    clear_relay_provider_tables(document);
    if root_model_provider(document).is_some_and(|provider| provider != NATIVE_PROVIDER_ID) {
        document.remove("model_provider");
    }
    remove_root_keys(document, ROUTING_KEYS);
    remove_root_keys(document, &["model_reasoning_effort"]);
    clear_named_profile_routing_overrides(document, true);
}

fn remove_root_keys(document: &mut DocumentMut, keys: &[&str]) {
    for key in keys {
        document.remove(key);
    }
}

fn clear_relay_provider_tables(document: &mut DocumentMut) {
    remove_relay_provider_tables(document);
}

pub(in crate::local_pool::profiles::codex) fn remove_relay_provider_tables(
    document: &mut DocumentMut,
) {
    for provider_id in RELAY_PROVIDER_IDS {
        remove_managed_provider(document, provider_id);
    }
}

/// Clear route fields in named profiles while retaining unrelated user
/// settings. Account mode keeps an explicit native `openai` provider; Relay
/// mode clears every profile provider override.
pub(in crate::local_pool::profiles::codex) fn clear_named_profile_routing_overrides(
    document: &mut DocumentMut,
    keep_openai_provider: bool,
) {
    if let Some(profiles) = document
        .get_mut("profiles")
        .and_then(Item::as_table_like_mut)
    {
        for (_, profile) in profiles.iter_mut() {
            let Some(profile) = profile.as_table_like_mut() else {
                continue;
            };
            if !keep_openai_provider
                || profile
                    .get("model_provider")
                    .and_then(Item::as_str)
                    .is_some_and(|provider| provider != NATIVE_PROVIDER_ID)
            {
                profile.remove("model_provider");
            }
            for key in ROUTING_KEYS {
                profile.remove(key);
            }
        }
    }
}

pub(in crate::local_pool::profiles::codex) fn enable_show_ultra_picker(document: &mut DocumentMut) {
    if desktop_bool(document, DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY) == Some(true) {
        return;
    }
    if document.get("desktop").is_some()
        && document
            .get("desktop")
            .and_then(Item::as_table_like)
            .is_none()
    {
        return;
    }
    if document.get("desktop").is_none() {
        document["desktop"] = Item::Table(Table::new());
    }
    document["desktop"][DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY] = value(true);
}

fn restore_show_ultra_picker(document: &mut DocumentMut, previous: Option<bool>) {
    if document.get("desktop").is_some()
        && document
            .get("desktop")
            .and_then(Item::as_table_like)
            .is_none()
    {
        return;
    }
    match previous {
        Some(enabled) => {
            if document.get("desktop").is_none() {
                document["desktop"] = Item::Table(Table::new());
            }
            document["desktop"][DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY] = value(enabled);
        }
        None => {
            let removed_last = {
                let Some(desktop) = document
                    .get_mut("desktop")
                    .and_then(Item::as_table_like_mut)
                else {
                    return;
                };
                desktop.remove(DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY);
                desktop.is_empty()
            };
            if removed_last {
                document.remove("desktop");
            }
        }
    }
}

fn remove_unsupported_reasoning_efforts(document: &mut DocumentMut) {
    let Some(desktop) = document
        .get_mut("desktop")
        .and_then(Item::as_table_like_mut)
    else {
        return;
    };
    let Some(efforts) = desktop
        .get_mut("enabled-reasoning-efforts")
        .and_then(Item::as_array_mut)
    else {
        return;
    };
    efforts.retain(|effort| effort.as_str() != Some("persistent"));
}

pub(in crate::local_pool::profiles::codex) fn set_managed_websockets(
    document: &mut DocumentMut,
    provider_id: &str,
    enabled: bool,
) -> bool {
    let Some(provider) = document
        .get_mut("model_providers")
        .and_then(Item::as_table_like_mut)
        .and_then(|providers| providers.get_mut(provider_id))
        .and_then(Item::as_table_like_mut)
    else {
        return false;
    };
    provider.insert("supports_websockets", value(enabled));
    true
}

pub(in crate::local_pool::profiles::codex) fn restore_config(
    document: &mut DocumentMut,
    managed_provider_id: &str,
    previous_model_provider: Option<&str>,
    previous_model_catalog: Option<&str>,
) {
    // A backup can have been created by an older Relay build whose managed
    // provider id is no longer the active one. Remove every known Relay
    // provider, then remove the id recorded in the backup as a final
    // compatibility guard.
    remove_relay_provider_tables(document);
    if !RELAY_PROVIDER_IDS.contains(&managed_provider_id) {
        remove_managed_provider(document, managed_provider_id);
    }
    restore_root_string(document, "model_provider", previous_model_provider);
    restore_root_string(document, "model_catalog_json", previous_model_catalog);
}

pub(in crate::local_pool::profiles::codex) fn restore_local_config(
    document: &mut DocumentMut,
    backup: &ProfileBackup,
    previous_model_catalog: Option<&str>,
    current_model_reasoning_effort: Option<&str>,
) {
    restore_config(
        document,
        &backup.managed_provider_id,
        backup.previous_model_provider.as_deref(),
        previous_model_catalog,
    );
    restore_root_string(document, "model", backup.previous_model.as_deref());
    restore_root_string(
        document,
        "review_model",
        backup.previous_review_model.as_deref(),
    );
    restore_root_string(
        document,
        "chatgpt_base_url",
        backup.previous_chatgpt_base_url.as_deref(),
    );
    restore_root_string(
        document,
        "openai_base_url",
        backup.previous_openai_base_url.as_deref(),
    );
    if backup.managed_model_reasoning_effort_cleared {
        let managed_effort_is_unchanged =
            current_model_reasoning_effort == backup.managed_model_reasoning_effort.as_deref();
        restore_root_string(
            document,
            "model_reasoning_effort",
            if managed_effort_is_unchanged {
                backup.previous_model_reasoning_effort.as_deref()
            } else {
                current_model_reasoning_effort.or(backup.previous_model_reasoning_effort.as_deref())
            },
        );
    }
    // A projection restores this switch itself and keeps a newer user value.
    // Older backups have no projection, so put the recorded value back here.
    if backup.projection_secret_ref.is_none() && backup.managed_show_ultra_picker {
        restore_show_ultra_picker(document, backup.previous_show_ultra_picker);
    }
}

pub(in crate::local_pool::profiles::codex) fn reasoning_effort_for_attach(
    document: &DocumentMut,
    catalog_json: Option<&str>,
) -> Option<String> {
    let current_effort = root_model_reasoning_effort(document);
    let Some(selected_model) = document.get("model").and_then(Item::as_str) else {
        return current_effort;
    };
    let Some(catalog_json) = catalog_json else {
        return current_effort;
    };
    let Ok(catalog) = serde_json::from_str::<Value>(catalog_json) else {
        return current_effort;
    };
    let model_entry = catalog
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| {
            models.iter().find(|model| {
                model
                    .get("slug")
                    .and_then(Value::as_str)
                    .is_some_and(|slug| slug.eq_ignore_ascii_case(selected_model))
            })
        });
    let Some(model_entry) = model_entry else {
        return current_effort;
    };
    let supported_levels = model_entry
        .get("supported_reasoning_levels")
        .and_then(Value::as_array)?;
    let supports_effort = |effort: &str| {
        supported_levels.iter().any(|level| {
            level
                .get("effort")
                .and_then(Value::as_str)
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(effort))
        })
    };
    if let Some(current_effort) = current_effort.filter(|effort| supports_effort(effort)) {
        return Some(current_effort);
    }
    model_entry
        .get("default_reasoning_level")
        .and_then(Value::as_str)
        .filter(|effort| supports_effort(effort))
        .map(ToOwned::to_owned)
}

pub(in crate::local_pool::profiles::codex) fn restore_root_string(
    document: &mut DocumentMut,
    key: &str,
    previous: Option<&str>,
) {
    match previous {
        Some(previous) => document[key] = value(previous),
        None => {
            document.remove(key);
        }
    }
}

pub(in crate::local_pool::profiles::codex) fn remove_managed_provider(
    document: &mut DocumentMut,
    provider_id: &str,
) {
    let providers_empty = {
        let Some(model_providers) = document
            .get_mut("model_providers")
            .and_then(Item::as_table_like_mut)
        else {
            return;
        };
        model_providers.remove(provider_id);
        model_providers.is_empty()
    };
    if providers_empty {
        document.remove("model_providers");
    }
}
