use super::ImportedCredentialMaterial;
use zenith_relay_core::accounts::{ImportAuthMode, ParsedImportItem};

pub(super) fn parsed_item_json(
    import_item: &ParsedImportItem,
    auth_mode: ImportAuthMode,
) -> serde_json::Value {
    let mut item_json = serde_json::Map::new();
    item_json.insert(
        "label".into(),
        serde_json::Value::String(import_item.label.clone()),
    );
    item_json.insert(
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
    insert_optional_string(
        &mut item_json,
        "account_id",
        import_item.account_id.as_deref(),
    );
    insert_optional_string(
        &mut item_json,
        "user_id",
        import_item.chatgpt_user_id.as_deref(),
    );
    insert_optional_string(
        &mut item_json,
        "organization_id",
        import_item.organization_id.as_deref(),
    );
    insert_optional_string(&mut item_json, "base_url", import_item.base_url.as_deref());
    insert_optional_string(&mut item_json, "protocol", import_item.protocol.as_deref());
    insert_optional_string(&mut item_json, "email", import_item.email());
    insert_optional_string(&mut item_json, "phone", import_item.phone());
    insert_optional_string(&mut item_json, "password", import_item.password());
    insert_optional_string(&mut item_json, "2fa", import_item.totp_secret());
    if let Some(priority) = import_item.priority {
        item_json.insert("priority".into(), priority.into());
    }
    if import_item.account_is_fedramp {
        item_json.insert("chatgpt_account_is_fedramp".into(), true.into());
    }
    if !import_item.tags.is_empty() {
        item_json.insert(
            "tags".into(),
            serde_json::Value::Array(
                import_item
                    .tags
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    }
    let secrets = import_item.secrets();
    insert_optional_string(&mut item_json, "access_token", secrets.access_token());
    insert_optional_string(&mut item_json, "refresh_token", secrets.refresh_token());
    insert_optional_string(&mut item_json, "id_token", secrets.id_token());
    insert_optional_string(&mut item_json, "api_key", secrets.api_key());
    insert_optional_string(
        &mut item_json,
        "agent_private_key",
        secrets.agent_private_key(),
    );
    insert_optional_string(
        &mut item_json,
        "agent_runtime_id",
        secrets.agent_runtime_id(),
    );
    insert_optional_string(&mut item_json, "task_id", secrets.agent_task_id());
    serde_json::Value::Object(item_json)
}

pub(super) fn parsed_item_json_with_material(
    original_json: serde_json::Value,
    material: &ImportedCredentialMaterial,
) -> serde_json::Value {
    let mut item_json = original_json.as_object().cloned().unwrap_or_default();
    apply_material(&mut item_json, material);
    serde_json::Value::Object(item_json)
}

fn apply_material(
    item_json: &mut serde_json::Map<String, serde_json::Value>,
    material: &ImportedCredentialMaterial,
) {
    insert_optional_string(
        item_json,
        "account_id",
        material.provider_account_id.as_deref(),
    );
    insert_optional_string(item_json, "user_id", material.provider_user_id.as_deref());
    insert_optional_string(
        item_json,
        "organization_id",
        material.organization_id.as_deref(),
    );
    insert_optional_string(item_json, "email", material.email.as_deref());
    insert_optional_string(item_json, "access_token", Some(&material.access_token));
    if let Some(agent) = material.agent_identity.as_ref() {
        insert_optional_string(item_json, "agent_private_key", Some(agent.private_key()));
        insert_optional_string(item_json, "agent_runtime_id", Some(agent.runtime_id()));
        insert_optional_string(item_json, "task_id", agent.task_id());
    }
    insert_optional_string(
        item_json,
        "refresh_token",
        material.refresh_token.as_deref(),
    );
    insert_optional_string(item_json, "id_token", material.id_token.as_deref());
    insert_optional_string(item_json, "plan_type", material.plan_type.as_deref());
    if material.account_is_fedramp {
        item_json.insert("chatgpt_account_is_fedramp".into(), true.into());
    }
    if let Some(expires_at_ms) = material.expires_at_ms {
        item_json.insert("expires_at_ms".into(), expires_at_ms.into());
    }
}

fn insert_optional_string(
    item_json: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    optional_text: Option<&str>,
) {
    if let Some(optional_text) = zenith_relay_core::omit_blank(optional_text) {
        item_json.insert(
            key.into(),
            serde_json::Value::String(optional_text.to_string()),
        );
    }
}
