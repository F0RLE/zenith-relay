use super::super::gemini::{self, GeminiBridgeRequest, GeminiBridgeResponse};
use super::super::messages;
use super::super::stream::{AdapterStreamBridge, GeminiStreamBridge, MessagesStreamBridge};
use super::{
    AdapterError, AdapterResult, MessagesBridgeRequest, MessagesBridgeResponse, MessagesBridgeState,
};
use crate::tool_policy::{apply_tool_policy, ToolPolicyResult};
use crate::ToolPolicy;
use serde_json::Value;
/// A translated response from any non-native source binding.
#[derive(Clone, Debug)]
pub enum AdapterResponse {
    Messages(MessagesBridgeResponse),
    Gemini(GeminiBridgeResponse),
    Translated(MessagesBridgeResponse),
}

impl AdapterResponse {
    pub fn response_body(&self) -> &Value {
        match self {
            Self::Messages(response) | Self::Translated(response) => &response.response_body,
            Self::Gemini(response) => &response.response_body,
        }
    }

    pub fn response_id(&self) -> &str {
        match self {
            Self::Messages(response) | Self::Translated(response) => &response.response_id,
            Self::Gemini(response) => &response.response_id,
        }
    }

    pub fn messages_continuation(&self) -> Option<&MessagesBridgeResponse> {
        match self {
            Self::Messages(response) | Self::Translated(response) => Some(response),
            Self::Gemini(_) => None,
        }
    }

    /// Returns the local continuation payload for either bridge. The method
    /// keeps the legacy name above source-compatible while making persistence
    /// protocol-agnostic.
    pub fn continuation(&self) -> Option<(&str, &MessagesBridgeState)> {
        match self {
            Self::Messages(response) | Self::Translated(response) => {
                Some((&response.response_id, &response.continuation))
            }
            Self::Gemini(response) => Some((&response.response_id, &response.continuation)),
        }
    }
}

/// A source-agnostic prepared protocol route.
///
/// It pairs the exact upstream payload with the inverse translation required
/// when the response returns. The gateway only transports this value; it does
/// not need special branches for a provider or model family.
#[derive(Clone, Debug)]
pub enum PreparedAdapterRequest {
    Native {
        upstream_body: Value,
    },
    ResponsesToMessages {
        request: Box<MessagesBridgeRequest>,
    },
    ResponsesToGemini {
        request: Box<GeminiBridgeRequest>,
    },
    Translated {
        request: Box<super::super::translation::TranslationRequest>,
    },
}

impl PreparedAdapterRequest {
    pub fn upstream_body(&self) -> &Value {
        match self {
            Self::Native { upstream_body } => upstream_body,
            Self::ResponsesToMessages { request } => request.upstream_body(),
            Self::ResponsesToGemini { request } => request.upstream_body(),
            Self::Translated { request } => request.upstream_body(),
        }
    }

    /// Only native routes permit local request normalization after adapter
    /// preparation. Bridge bodies are already a complete upstream contract.
    pub fn native_upstream_body_mut(&mut self) -> Option<&mut Value> {
        match self {
            Self::Native { upstream_body } => Some(upstream_body),
            Self::ResponsesToMessages { .. }
            | Self::ResponsesToGemini { .. }
            | Self::Translated { .. } => None,
        }
    }

    pub fn upstream_body_mut(&mut self) -> &mut Value {
        match self {
            Self::Native { upstream_body } => upstream_body,
            Self::ResponsesToMessages { request } => &mut request.upstream_body,
            Self::ResponsesToGemini { request } => &mut request.upstream_body,
            Self::Translated { request } => &mut request.upstream_body,
        }
    }

    pub const fn is_passthrough(&self) -> bool {
        matches!(self, Self::Native { .. })
    }

    pub const fn requires_bridge_headers(&self) -> bool {
        matches!(
            self,
            Self::ResponsesToMessages { .. }
                | Self::ResponsesToGemini { .. }
                | Self::Translated { .. }
        )
    }

    pub const fn uses_messages_continuation(&self) -> bool {
        matches!(
            self,
            Self::ResponsesToMessages { .. }
                | Self::ResponsesToGemini { .. }
                | Self::Translated { .. }
        )
    }

    /// Translates a completed upstream response only when the selected route
    /// is a bridge. Native bytes remain untouched and can be proxied directly.
    pub fn translate_response_bytes(self, bytes: &[u8]) -> AdapterResult<Option<AdapterResponse>> {
        match self {
            Self::Native { .. } => Ok(None),
            Self::Translated { request } => {
                let upstream = serde_json::from_slice::<Value>(bytes)
                    .map_err(|_| AdapterError::upstream_response_invalid())?;
                request
                    .translate(&upstream)
                    .map(AdapterResponse::Translated)
                    .map(Some)
            }
            Self::ResponsesToMessages { request } => {
                let upstream = serde_json::from_slice::<Value>(bytes)
                    .map_err(|_| AdapterError::upstream_response_invalid())?;
                messages::translate_messages_response(*request, &upstream)
                    .map(AdapterResponse::Messages)
                    .map(Some)
            }
            Self::ResponsesToGemini { request } => {
                let upstream = serde_json::from_slice::<Value>(bytes)
                    .map_err(|_| AdapterError::upstream_response_invalid())?;
                gemini::translate_gemini_response(*request, &upstream)
                    .map(AdapterResponse::Gemini)
                    .map(Some)
            }
        }
    }

    /// Returns the stream transformer for a bridged route. A native route
    /// intentionally returns `None` so its stream stays byte-for-byte
    /// passthrough.
    pub fn into_stream_bridge(self) -> Option<AdapterStreamBridge> {
        match self {
            Self::Native { .. } => None,
            Self::Translated { request } => Some(AdapterStreamBridge::Translated(Box::new(
                super::super::translation::TranslationStream::new(*request),
            ))),
            Self::ResponsesToMessages { request } => Some(AdapterStreamBridge::Messages(Box::new(
                MessagesStreamBridge::new(*request),
            ))),
            Self::ResponsesToGemini { request } => Some(AdapterStreamBridge::Gemini(Box::new(
                GeminiStreamBridge::new(*request),
            ))),
        }
    }
}

impl PreparedAdapterRequest {
    /// Apply the non-destructive policy at the final catalog boundary, after
    /// input/history translation and before any upstream I/O.
    pub(crate) fn apply_tool_policy(
        &mut self,
        policy: &ToolPolicy,
    ) -> Result<ToolPolicyResult, &'static str> {
        apply_tool_policy(self.upstream_body_mut(), policy)
    }
}
