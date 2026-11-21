use super::super::*;

const AUTH_FIELDS: &[&str] = &[
    "OPENAI_API_KEY",
    "auth_mode",
    "tokens",
    "last_refresh",
    "agent_identity",
    "personal_access_token",
    "bedrock_api_key",
    "bedrock_access_keys",
];

/// Credentials are mutually exclusive; extension fields are not credentials.
/// Callers must verify credential ownership before restoring a saved login.
pub(in crate::local_pool::profiles::codex) fn merge_auth(
    existing_auth_text: Option<&str>,
    credential: Option<&str>,
) -> Result<Option<String>> {
    let mut existing_auth = auth_object(existing_auth_text)?;
    let target = auth_object(credential)?;
    let extensions: serde_json::Map<_, _> = existing_auth
        .iter()
        .filter(|(key, _)| !AUTH_FIELDS.contains(&key.as_str()))
        .map(|(field_name, field_value)| (field_name.clone(), field_value.clone()))
        .collect();
    let target_extensions: serde_json::Map<_, _> = target
        .iter()
        .filter(|(key, _)| !AUTH_FIELDS.contains(&key.as_str()))
        .map(|(field_name, field_value)| (field_name.clone(), field_value.clone()))
        .collect();
    if extensions == target_extensions {
        return Ok(credential.map(str::to_owned));
    }
    for key in AUTH_FIELDS {
        existing_auth.remove(*key);
        if let Some(field_value) = target.get(*key) {
            existing_auth.insert((*key).to_owned(), field_value.clone());
        }
    }
    if existing_auth.is_empty() && credential.is_none() {
        return Ok(None);
    }
    serde_json::to_string_pretty(&existing_auth)
        .map(|text| Some(format!("{text}\n")))
        .map_err(LocalPoolError::invalid_state)
}

fn auth_object(content: Option<&str>) -> Result<serde_json::Map<String, Value>> {
    let Some(content) = content.filter(|text| !text.trim().is_empty()) else {
        return Ok(Default::default());
    };
    serde_json::from_str::<Value>(content)
        .ok()
        .and_then(|auth_document| auth_document.as_object().cloned())
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "Profile auth must be a JSON object",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_switch_preserves_extensions_but_not_old_tokens() {
        let previous_auth_json = r#"{"tokens":{"access_token":"old"},"extension":{"flag":true}}"#;
        let switched_auth = merge_auth(
            Some(previous_auth_json),
            Some(r#"{"auth_mode":"apikey","OPENAI_API_KEY":"test"}"#),
        )
        .unwrap()
        .unwrap();
        let switched_auth: Value = serde_json::from_str(&switched_auth).unwrap();
        assert!(switched_auth.get("tokens").is_none());
        assert_eq!(switched_auth["extension"]["flag"], true);
    }
}
