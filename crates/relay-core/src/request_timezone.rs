use serde_json::Value;

pub const PROXY_TIMEZONE_MAX_AGE_MS: u64 = 15 * 60_000;

/// Validated timezone observed at the proxy's egress IP.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyTimezone {
    pub timezone: String,
    pub observed_at_ms: u64,
}

impl ProxyTimezone {
    pub fn current(&self, now_ms: u64) -> Option<&str> {
        now_ms
            .checked_sub(self.observed_at_ms)
            .filter(|age| *age < PROXY_TIMEZONE_MAX_AGE_MS)
            .map(|_| self.timezone.as_str())
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
    fn only_current_observed_proxy_timezones_are_available() {
        let evidence = ProxyTimezone {
            timezone: "Europe/Minsk".into(),
            observed_at_ms: 100_000,
        };
        assert_eq!(evidence.current(99_999), None);
        assert_eq!(evidence.current(100_000), Some("Europe/Minsk"));
        assert_eq!(evidence.current(100_000 + PROXY_TIMEZONE_MAX_AGE_MS), None);
        assert!(is_valid_timezone("Etc/GMT-3"));
        assert!(!is_valid_timezone("made/up"));
        assert_eq!(
            normalize_timezone(Some(" Europe/Minsk ".into()))
                .unwrap()
                .as_deref(),
            Some("Europe/Minsk")
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
