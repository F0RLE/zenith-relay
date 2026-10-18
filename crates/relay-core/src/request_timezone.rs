use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const DEFAULT_REQUEST_TIMEZONE: &str = "America/New_York";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountTimezoneSource {
    #[default]
    Account,
    Proxy,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestTimezonePolicy {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub account_timezones: BTreeMap<String, String>,
    /// Optional per-account override. Missing entries inherit `enabled`.
    #[serde(default)]
    pub account_timezone_bindings: BTreeMap<String, bool>,
    /// Optional source for an account's effective timezone. Missing entries
    /// preserve the legacy account/global fallback behavior.
    #[serde(default)]
    pub account_timezone_sources: BTreeMap<String, AccountTimezoneSource>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestTimezoneUpdate {
    pub policy: RequestTimezonePolicy,
    pub expected_policy: RequestTimezonePolicy,
}

impl RequestTimezonePolicy {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.account_timezones.len() > 10_000 {
            return Err("too many account timezones");
        }
        if self.account_timezone_bindings.len() > 10_000 {
            return Err("too many account timezone bindings");
        }
        if self.account_timezone_sources.len() > 10_000 {
            return Err("too many account timezone sources");
        }
        for (account_id, zone) in &self.account_timezones {
            if !valid_account_id(account_id) {
                return Err("invalid account id for request timezone");
            }
            if !is_valid_timezone(zone) {
                return Err("request timezone must be a known IANA timezone");
            }
        }
        for account_id in self.account_timezone_bindings.keys() {
            if !valid_account_id(account_id) {
                return Err("invalid account id for request timezone");
            }
        }
        for account_id in self.account_timezone_sources.keys() {
            if !valid_account_id(account_id) {
                return Err("invalid account id for request timezone");
            }
        }
        Ok(())
    }

    /// Resolves the timezone written into one account's requests.
    ///
    /// An explicit per-account "off" always wins. Without an explicit choice
    /// and with the global rule off, the account follows its assigned proxy
    /// and sends nothing when there is no proxy timezone.
    pub fn timezone_for(&self, account_id: &str, proxy_timezone: Option<&str>) -> Option<String> {
        let binding = self.account_timezone_bindings.get(account_id).copied();
        if binding == Some(false) {
            return None;
        }
        if !binding.unwrap_or(self.enabled) {
            return proxy_timezone.map(str::to_owned);
        }
        if self.account_timezone_sources.get(account_id) == Some(&AccountTimezoneSource::Proxy) {
            return proxy_timezone.map(str::to_owned);
        }
        if !self.account_timezone_sources.contains_key(account_id)
            && !self.account_timezones.contains_key(account_id)
        {
            if let Some(timezone) = proxy_timezone {
                return Some(timezone.to_owned());
            }
        }
        Some(
            self.account_timezones
                .get(account_id)
                .cloned()
                .unwrap_or_else(|| DEFAULT_REQUEST_TIMEZONE.to_string()),
        )
    }

    pub fn binding_for(&self, account_id: &str) -> AccountTimezoneBinding {
        self.account_timezone_bindings
            .get(account_id)
            .copied()
            .map(AccountTimezoneBinding::from)
            .unwrap_or(AccountTimezoneBinding::Inherit)
    }
}

pub fn is_valid_timezone(value: &str) -> bool {
    !value.trim().is_empty() && value.parse::<chrono_tz::Tz>().is_ok()
}

pub fn normalize_timezone(value: Option<String>) -> Result<Option<String>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if !is_valid_timezone(value) {
        return Err("request timezone must be a known IANA timezone");
    }
    Ok(Some(value.to_string()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountTimezoneBinding {
    Inherit,
    Enabled,
    Disabled,
}

impl From<bool> for AccountTimezoneBinding {
    fn from(enabled: bool) -> Self {
        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }
}

fn valid_account_id(account_id: &str) -> bool {
    !account_id.is_empty()
        && account_id.len() <= 256
        && account_id.trim() == account_id
        && !account_id.chars().any(char::is_control)
}

pub fn request_timezone_catalog() -> Vec<String> {
    let mut zones = chrono_tz::TZ_VARIANTS
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    zones.sort();
    zones
}

/// Rewrite only structured user environment fields and existing search-tool
/// location fields. The caller supplies an account-specific upstream body.
pub(crate) fn rewrite_request_timezone(body: &mut Value, timezone: &str) {
    if let Some(input) = body.get_mut("input") {
        match input {
            Value::String(text) => rewrite_environment(text, timezone),
            Value::Array(items) => {
                for item in items {
                    if item.get("role").and_then(Value::as_str) != Some("user") {
                        continue;
                    }
                    match item.get_mut("content") {
                        Some(Value::String(text)) => rewrite_environment(text, timezone),
                        Some(Value::Array(content)) => {
                            for part in content {
                                if matches!(
                                    part.get("type").and_then(Value::as_str),
                                    Some("input_text" | "text")
                                ) {
                                    if let Some(Value::String(text)) = part.get_mut("text") {
                                        rewrite_environment(text, timezone);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(Value::Array(tools)) = body.get_mut("tools") {
        for tool in tools {
            if matches!(
                tool.get("type").and_then(Value::as_str),
                Some("web_search" | "web_search_preview" | "web_search_preview_2025_03_11")
            ) {
                if let Some(Value::String(zone)) = tool.pointer_mut("/user_location/timezone") {
                    *zone = timezone.to_string();
                }
            }
        }
    }
}

fn rewrite_environment(text: &mut String, timezone: &str) {
    const OPEN: &str = "<environment_context>";
    const CLOSE: &str = "</environment_context>";
    const ZONE_OPEN: &str = "<timezone>";
    const ZONE_CLOSE: &str = "</timezone>";
    let mut cursor = 0;
    while let Some(start) = text[cursor..].find(OPEN).map(|offset| cursor + offset) {
        let content_start = start + OPEN.len();
        let Some(end) = text[content_start..]
            .find(CLOSE)
            .map(|offset| content_start + offset)
        else {
            break;
        };
        let block = &text[content_start..end];
        // Nested, duplicate, or incomplete blocks are ambiguous and stay intact.
        let valid = !block.contains(OPEN)
            && block.matches(ZONE_OPEN).count() == 1
            && block.matches(ZONE_CLOSE).count() == 1;
        if valid {
            let zone_start = content_start + block.find(ZONE_OPEN).unwrap() + ZONE_OPEN.len();
            let zone_end = content_start + block.find(ZONE_CLOSE).unwrap();
            if zone_start <= zone_end && !text[zone_start..zone_end].contains('<') {
                let previous_len = zone_end - zone_start;
                text.replace_range(zone_start..zone_end, timezone);
                cursor = end - previous_len + timezone.len() + CLOSE.len();
                continue;
            }
        }
        cursor = end + CLOSE.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn policy_is_disabled_by_default_and_validates_iana_zones() {
        let mut policy = RequestTimezonePolicy::default();
        assert_eq!(policy.timezone_for("a", None), None);
        assert_eq!(policy.binding_for("a"), AccountTimezoneBinding::Inherit);
        policy.enabled = true;
        assert_eq!(
            policy.timezone_for("a", None),
            Some("America/New_York".to_string())
        );
        policy
            .account_timezones
            .insert("a".into(), "Etc/GMT-3".into());
        assert!(policy.validate().is_ok());
        assert_eq!(
            policy.timezone_for("a", None),
            Some("Etc/GMT-3".to_string())
        );
        policy.enabled = false;
        assert_eq!(policy.timezone_for("a", None), None);
        policy.account_timezone_bindings.insert("a".into(), true);
        assert_eq!(
            policy.timezone_for("a", None),
            Some("Etc/GMT-3".to_string())
        );
        assert_eq!(policy.binding_for("a"), AccountTimezoneBinding::Enabled);
        policy.account_timezone_bindings.insert("a".into(), false);
        assert_eq!(policy.timezone_for("a", None), None);
        assert_eq!(policy.binding_for("a"), AccountTimezoneBinding::Disabled);
        policy
            .account_timezones
            .insert("a".into(), "made/up".into());
        assert!(policy.validate().is_err());
        assert!(request_timezone_catalog().contains(&"Etc/GMT-3".into()));
    }

    #[test]
    fn proxy_source_uses_only_the_assigned_proxy_timezone() {
        let mut policy = RequestTimezonePolicy {
            enabled: true,
            account_timezones: BTreeMap::from([("a".into(), "Etc/GMT+3".into())]),
            account_timezone_bindings: BTreeMap::new(),
            account_timezone_sources: BTreeMap::from([("a".into(), AccountTimezoneSource::Proxy)]),
        };
        assert_eq!(
            policy.timezone_for("a", Some("Etc/GMT-3")),
            Some("Etc/GMT-3".to_string())
        );
        assert_eq!(policy.timezone_for("a", None), None);
        policy.account_timezone_sources.clear();
        assert_eq!(
            policy.timezone_for("a", None),
            Some("Etc/GMT+3".to_string())
        );
    }

    #[test]
    fn default_follows_the_assigned_proxy_and_explicit_off_wins() {
        let mut policy = RequestTimezonePolicy::default();
        assert_eq!(policy.timezone_for("a", None), None);
        assert_eq!(
            policy.timezone_for("a", Some("Etc/GMT-3")),
            Some("Etc/GMT-3".to_string())
        );
        policy.account_timezone_bindings.insert("a".into(), false);
        assert_eq!(policy.timezone_for("a", Some("Etc/GMT-3")), None);
    }

    #[test]
    fn global_enable_still_inherits_proxy_without_overriding_an_explicit_account_choice() {
        let mut policy = RequestTimezonePolicy {
            enabled: true,
            ..Default::default()
        };
        assert_eq!(
            policy.timezone_for("a", Some("Asia/Tokyo")),
            Some("Asia/Tokyo".into())
        );
        policy
            .account_timezone_sources
            .insert("a".into(), AccountTimezoneSource::Account);
        assert_eq!(
            policy.timezone_for("a", Some("Asia/Tokyo")),
            Some(DEFAULT_REQUEST_TIMEZONE.into())
        );
        policy
            .account_timezones
            .insert("a".into(), "Europe/Amsterdam".into());
        assert_eq!(
            policy.timezone_for("a", Some("Asia/Tokyo")),
            Some("Europe/Amsterdam".into())
        );
    }

    #[test]
    fn rewrite_preserves_dates_other_roles_and_missing_tool_fields() {
        let environment = "<environment_context><current_date>2026-10-10</current_date><timezone>UTC</timezone></environment_context>";
        let mut body = json!({
            "input": [
                {"role":"system","content":environment},
                {"role":"assistant","content":environment},
                {"role":"user","content":[{"type":"input_text","text":environment}]},
                {"role":"user","content":"ordinary UTC text"}
            ],
            "tools":[
                {"type":"web_search","user_location":{"timezone":"UTC","city":"Synthetic"}},
                {"type":"web_search_preview","user_location":{"city":"Synthetic"}},
                {"type":"function","user_location":{"timezone":"UTC"}}
            ]
        });
        rewrite_request_timezone(&mut body, "Etc/GMT-3");
        assert_eq!(body["input"][0]["content"], environment);
        assert_eq!(body["input"][1]["content"], environment);
        assert_eq!(
            body["input"][2]["content"][0]["text"],
            environment.replace("UTC", "Etc/GMT-3")
        );
        assert_eq!(body["input"][3]["content"], "ordinary UTC text");
        assert_eq!(body["tools"][0]["user_location"]["timezone"], "Etc/GMT-3");
        assert!(body["tools"][1]["user_location"].get("timezone").is_none());
        assert_eq!(body["tools"][2]["user_location"]["timezone"], "UTC");
    }

    #[test]
    fn ambiguous_environment_blocks_are_not_changed() {
        for text in [
            "<timezone>UTC</timezone>",
            "<environment_context><timezone>UTC</timezone>",
            "<environment_context><timezone>UTC</environment_context>",
            "<environment_context><timezone>UTC</timezone><timezone>UTC</timezone></environment_context>",
            "<environment_context><environment_context><timezone>UTC</timezone></environment_context>",
            "<environment_context></timezone><timezone>UTC</environment_context>",
        ] {
            let mut body = json!({"input":text});
            rewrite_request_timezone(&mut body, "Etc/GMT-3");
            assert_eq!(body["input"], text);
        }
    }
}
