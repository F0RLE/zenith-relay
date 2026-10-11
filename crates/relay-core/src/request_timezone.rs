use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const DEFAULT_REQUEST_TIMEZONE: &str = "America/New_York";

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestTimezonePolicy {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub account_timezones: BTreeMap<String, String>,
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
        for (account_id, zone) in &self.account_timezones {
            if account_id.is_empty()
                || account_id.len() > 256
                || account_id.trim() != account_id
                || account_id.chars().any(char::is_control)
            {
                return Err("invalid account id for request timezone");
            }
            if zone.parse::<chrono_tz::Tz>().is_err() {
                return Err("request timezone must be a known IANA timezone");
            }
        }
        Ok(())
    }

    pub fn timezone_for(&self, account_id: &str) -> Option<&str> {
        self.enabled.then(|| {
            self.account_timezones
                .get(account_id)
                .map(String::as_str)
                .unwrap_or(DEFAULT_REQUEST_TIMEZONE)
        })
    }
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
        assert_eq!(policy.timezone_for("a"), None);
        policy.enabled = true;
        assert_eq!(policy.timezone_for("a"), Some("America/New_York"));
        policy
            .account_timezones
            .insert("a".into(), "Europe/Minsk".into());
        assert!(policy.validate().is_ok());
        assert_eq!(policy.timezone_for("a"), Some("Europe/Minsk"));
        policy
            .account_timezones
            .insert("a".into(), "made/up".into());
        assert!(policy.validate().is_err());
        assert!(request_timezone_catalog().contains(&"Europe/Minsk".into()));
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
        rewrite_request_timezone(&mut body, "Europe/Minsk");
        assert_eq!(body["input"][0]["content"], environment);
        assert_eq!(body["input"][1]["content"], environment);
        assert_eq!(
            body["input"][2]["content"][0]["text"],
            environment.replace("UTC", "Europe/Minsk")
        );
        assert_eq!(body["input"][3]["content"], "ordinary UTC text");
        assert_eq!(
            body["tools"][0]["user_location"]["timezone"],
            "Europe/Minsk"
        );
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
            rewrite_request_timezone(&mut body, "Europe/Minsk");
            assert_eq!(body["input"], text);
        }
    }
}
