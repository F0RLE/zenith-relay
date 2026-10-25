use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireApi {
    Responses,
    ChatCompletions,
    Messages,
    Gemini,
    /// Native-only Interactions; never inferred from Generate Content support.
    GeminiInteractions,
}

impl WireApi {
    /// The interoperable conversion matrix, retained for existing callers.
    /// Native-only extensions must be enumerated with `ALL_NATIVE`.
    pub const ALL: [Self; 4] = [
        Self::Responses,
        Self::ChatCompletions,
        Self::Messages,
        Self::Gemini,
    ];

    pub const ALL_NATIVE: [Self; 5] = [
        Self::Responses,
        Self::ChatCompletions,
        Self::Messages,
        Self::Gemini,
        Self::GeminiInteractions,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::ChatCompletions => "chat_completions",
            Self::Messages => "messages",
            Self::Gemini => "gemini",
            Self::GeminiInteractions => "gemini_interactions",
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
            "gemini_interactions" => Some(Self::GeminiInteractions),
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
}
