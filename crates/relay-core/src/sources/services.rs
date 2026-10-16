use super::WireApi;
use url::Url;

#[derive(Clone, Copy)]
pub(super) enum Service {
    Zenith,
    OpenAi,
    OpenRouter,
    Anthropic,
    Gemini,
    Deepseek,
    Groq,
    Mistral,
    Moonshot,
    KimiCode,
    MiniMax,
    SiliconFlow,
    Xai,
}

impl Service {
    pub(super) fn protocols(self, url: &Url) -> Vec<WireApi> {
        match self {
            // This service publishes all four generation endpoints. The model's
            // native contract is selected separately from the client's wire API.
            Self::Zenith => WireApi::ALL.to_vec(),
            _ => self.protocol(url).into_iter().collect(),
        }
    }

    pub(super) fn from_host(host: &str) -> Option<Self> {
        match host {
            "api.zenithmarket.dev" => Some(Self::Zenith),
            "api.openai.com" => Some(Self::OpenAi),
            "openrouter.ai" => Some(Self::OpenRouter),
            "api.anthropic.com" => Some(Self::Anthropic),
            "generativelanguage.googleapis.com" => Some(Self::Gemini),
            "api.deepseek.com" => Some(Self::Deepseek),
            "api.groq.com" => Some(Self::Groq),
            "api.mistral.ai" => Some(Self::Mistral),
            "api.moonshot.ai" | "api.moonshot.cn" => Some(Self::Moonshot),
            "api.kimi.ai" | "api.kimi.com" => Some(Self::KimiCode),
            "api.minimax.io" | "api.minimaxi.com" => Some(Self::MiniMax),
            "api.siliconflow.cn" | "api.siliconflow.com" => Some(Self::SiliconFlow),
            "api.x.ai" => Some(Self::Xai),
            _ => None,
        }
    }

    pub(super) fn protocol(self, url: &Url) -> Option<WireApi> {
        let path = url.path().trim_end_matches('/');
        match self {
            Self::Zenith
            | Self::OpenAi
            | Self::OpenRouter
            | Self::Deepseek
            | Self::Groq
            | Self::Moonshot
            | Self::Xai => Some(WireApi::Responses),
            Self::Anthropic => Some(WireApi::Messages),
            Self::Gemini => Some(WireApi::Gemini),
            Self::KimiCode if path == "/coding" || path.starts_with("/coding/") => {
                Some(WireApi::Messages)
            }
            Self::KimiCode => None,
            Self::MiniMax if path == "/anthropic" || path.starts_with("/anthropic/") => {
                Some(WireApi::Messages)
            }
            Self::MiniMax | Self::Mistral => Some(WireApi::ChatCompletions),
            Self::SiliconFlow => None,
        }
    }

    /// SDKs append `/v1` to these published roots; Relay joins endpoints itself.
    pub(super) fn normalize_api_root(self, url: &mut Url) {
        let path = match (self, url.path().trim_end_matches('/')) {
            (Self::KimiCode, "/coding") => "/coding/v1/",
            (Self::MiniMax, "/anthropic") => "/anthropic/v1/",
            _ => return,
        };
        url.set_path(path);
    }

    /// Published Gemini endpoints can use a different root from OpenAI routes.
    /// Keep custom roots and explicitly selected Gemini versions intact.
    pub(super) fn normalize_gemini_api_root(self, url: &mut Url) {
        let path = match (self, url.path().trim_end_matches('/')) {
            (Self::Zenith, "" | "/v1") => "/v1beta/",
            (Self::Gemini, "" | "/v1beta/openai") => "/v1beta/",
            _ => return,
        };
        url.set_path(path);
    }
}
