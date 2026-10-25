use super::*;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientWireApi {
    Responses,
    ChatCompletions,
    Messages,
    Gemini,
    GeminiInteractions,
    Images,
}

/// Client protocols accepted by a local gateway key.
/// Image generation uses Chat Completions, so the legacy `images` scope is omitted.
pub fn local_gateway_client_wire_apis() -> Vec<ClientWireApi> {
    vec![
        ClientWireApi::Responses,
        ClientWireApi::Messages,
        ClientWireApi::ChatCompletions,
        ClientWireApi::Gemini,
        ClientWireApi::GeminiInteractions,
    ]
}
