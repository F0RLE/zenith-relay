use super::super::*;

const AUTH_FIELDS: &[&str] = &["OPENAI_API_KEY", "auth_mode", "tokens", "last_refresh"];

/// Credentials are mutually exclusive; extension fields are not credentials.
/// Callers must verify credential ownership before restoring a saved login.
pub(in crate::local_pool::profiles::codex) fn merge_auth(
    current: Option<&str>,
    credential: Option<&str>,
) -> Result<Option<String>> {
    let mut current = auth_object(current)?;
    let target = auth_object(credential)?;
    let extensions: serde_json::Map<_, _> = current
        .iter()
        .filter(|(key, _)| !AUTH_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let target_extensions: serde_json::Map<_, _> = target
        .iter()
        .filter(|(key, _)| !AUTH_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if extensions == target_extensions {
        return Ok(credential.map(str::to_owned));
    }
    for key in AUTH_FIELDS {
        current.remove(*key);
        if let Some(value) = target.get(*key) {
            current.insert((*key).to_owned(), value.clone());
        }
    }
    if current.is_empty() && credential.is_none() {
        return Ok(None);
    }
    serde_json::to_string_pretty(&current)
        .map(|text| Some(format!("{text}\n")))
        .map_err(LocalPoolError::invalid_state)
}

fn auth_object(content: Option<&str>) -> Result<serde_json::Map<String, Value>> {
    let Some(content) = content.filter(|text| !text.trim().is_empty()) else {
        return Ok(Default::default());
    };
    serde_json::from_str::<Value>(content)
        .ok()
        .and_then(|value| value.as_object().cloned())
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
        let current = r#"{"tokens":{"access_token":"old"},"extension":{"flag":true}}"#;
        let next = merge_auth(
            Some(current),
            Some(r#"{"auth_mode":"apikey","OPENAI_API_KEY":"test"}"#),
        )
        .unwrap()
        .unwrap();
        let next: Value = serde_json::from_str(&next).unwrap();
        assert!(next.get("tokens").is_none());
        assert_eq!(next["extension"]["flag"], true);
    }
}
