use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireApi {
    Responses,
    ChatCompletions,
    Messages,
    Gemini,
}

impl WireApi {
    pub const ALL: [Self; 4] = [
        Self::Responses,
        Self::ChatCompletions,
        Self::Messages,
        Self::Gemini,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::ChatCompletions => "chat_completions",
            Self::Messages => "messages",
            Self::Gemini => "gemini",
        }
    }

    /// Parses canonical and legacy persisted values without accepting a new
    /// user-facing protocol spelling.
    pub fn from_storage_value(stored_wire_api: &str) -> Option<Self> {
        match stored_wire_api {
            "responses" => Some(Self::Responses),
            "chat_completions" | "chatcompletions" => Some(Self::ChatCompletions),
            "messages" => Some(Self::Messages),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }
}

/// Explicit Anthropic prompt-cache write lifetime for a Messages upstream.
/// `Provider` preserves the request as supplied; Relay never chooses a TTL
/// unless the source owner selected one.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum CacheWriteTtl {
    #[default]
    #[serde(rename = "provider")]
    Provider,
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "1h")]
    OneHour,
}

impl CacheWriteTtl {
    pub const fn is_provider(&self) -> bool {
        matches!(self, Self::Provider)
    }

    pub const fn anthropic_ttl(self) -> Option<&'static str> {
        match self {
            Self::Provider => None,
            Self::FiveMinutes => Some("5m"),
            Self::OneHour => Some("1h"),
        }
    }

    pub fn from_anthropic_ttl(ttl_text: &str) -> Option<Self> {
        match ttl_text.trim() {
            "5m" => Some(Self::FiveMinutes),
            "1h" => Some(Self::OneHour),
            _ => None,
        }
    }
}
