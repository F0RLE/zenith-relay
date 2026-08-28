use super::ImportedCredentialMaterial;
use zenith_relay_core::accounts::{ImportAuthMode, ParsedImportItem};

pub(super) fn parsed_item_value(
    item: &ParsedImportItem,
    auth_mode: ImportAuthMode,
) -> serde_json::Value {
    let mut value = serde_json::Map::new();
    value.insert(
        "label".into(),
        serde_json::Value::String(item.label.clone()),
    );
    value.insert(
        "auth_mode".into(),
        serde_json::Value::String(
            match auth_mode {
                ImportAuthMode::OAuth => "oauth",
                ImportAuthMode::AgentIdentity => "agent_identity",
                ImportAuthMode::ApiKey => "api_key",
                ImportAuthMode::ImportedToken => "imported_token",
                ImportAuthMode::Unknown => "unknown",
            }
            .into(),
        ),
    );
    insert_optional_string(&mut value, "account_id", item.account_id.as_deref());
    insert_optional_string(&mut value, "user_id", item.chatgpt_user_id.as_deref());
    insert_optional_string(
        &mut value,
        "organization_id",
        item.organization_id.as_deref(),
    );
    insert_optional_string(&mut value, "base_url", item.base_url.as_deref());
    insert_optional_string(&mut value, "protocol", item.protocol.as_deref());
    insert_optional_string(&mut value, "email", item.email());
    if let Some(priority) = item.priority {
        value.insert("priority".into(), priority.into());
    }
    let secrets = item.secrets();
    insert_optional_string(&mut value, "access_token", secrets.access_token());
    insert_optional_string(&mut value, "refresh_token", secrets.refresh_token());
    insert_optional_string(&mut value, "id_token", secrets.id_token());
    insert_optional_string(&mut value, "api_key", secrets.api_key());
    insert_optional_string(&mut value, "agent_private_key", secrets.agent_private_key());
    insert_optional_string(&mut value, "agent_runtime_id", secrets.agent_runtime_id());
    insert_optional_string(&mut value, "task_id", secrets.agent_task_id());
    serde_json::Value::Object(value)
}

pub(super) fn parsed_item_value_from_material(
    original: serde_json::Value,
    material: &ImportedCredentialMaterial,
) -> serde_json::Value {
    let mut value = original.as_object().cloned().unwrap_or_default();
    apply_material(&mut value, material);
    serde_json::Value::Object(value)
}

fn apply_material(
    value: &mut serde_json::Map<String, serde_json::Value>,
    material: &ImportedCredentialMaterial,
) {
    insert_optional_string(value, "account_id", material.provider_account_id.as_deref());
    insert_optional_string(value, "user_id", material.provider_user_id.as_deref());
    insert_optional_string(
        value,
        "organization_id",
        material.organization_id.as_deref(),
    );
    insert_optional_string(value, "email", material.email.as_deref());
    insert_optional_string(value, "access_token", Some(&material.access_token));
    if let Some(agent) = material.agent_identity.as_ref() {
        insert_optional_string(value, "agent_private_key", Some(agent.private_key()));
        insert_optional_string(value, "agent_runtime_id", Some(agent.runtime_id()));
        insert_optional_string(value, "task_id", agent.task_id());
    }
    insert_optional_string(value, "refresh_token", material.refresh_token.as_deref());
    insert_optional_string(value, "id_token", material.id_token.as_deref());
    insert_optional_string(value, "plan_type", material.plan_type.as_deref());
    if let Some(expires_at_ms) = material.expires_at_ms {
        value.insert("expires_at_ms".into(), expires_at_ms.into());
    }
}

fn insert_optional_string(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<&str>,
) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        object.insert(key.into(), serde_json::Value::String(value.to_string()));
    }
}
