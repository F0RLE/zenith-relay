use super::super::stream::AdapterStreamBridge;
use super::{AdapterContinuationState, AdapterError, AdapterResult, TranslatedResponse};
use crate::tool_policy::{apply_tool_policy, ToolPolicyResult};
use crate::ToolPolicy;
use serde_json::Value;

/// One response codec result, independent of the selected upstream API.
#[derive(Clone, Debug)]
pub enum AdapterResponse {
    Translated(TranslatedResponse),
}

impl AdapterResponse {
    pub fn response_body(&self) -> &Value {
        let Self::Translated(response) = self;
        &response.response_body
    }

    pub fn response_id(&self) -> &str {
        let Self::Translated(response) = self;
        &response.response_id
    }

    pub fn translated_response(&self) -> &TranslatedResponse {
        let Self::Translated(response) = self;
        response
    }

    pub fn continuation(&self) -> Option<(&str, &AdapterContinuationState)> {
        let Self::Translated(response) = self;
        response
            .continuation
            .is_reusable()
            .then_some((response.response_id.as_str(), &response.continuation))
    }
}

/// Exactly two execution paths: native forwarding or the shared codecs.
#[derive(Clone, Debug)]
pub enum PreparedAdapterRequest {
    Native {
        upstream_body: Value,
    },
    Translated {
        request: Box<super::super::translation::TranslationRequest>,
    },
}

impl PreparedAdapterRequest {
    pub fn upstream_body(&self) -> &Value {
        match self {
            Self::Native { upstream_body } => upstream_body,
            Self::Translated { request } => request.upstream_body(),
        }
    }

    pub fn upstream_body_mut(&mut self) -> &mut Value {
        match self {
            Self::Native { upstream_body } => upstream_body,
            Self::Translated { request } => &mut request.upstream_body,
        }
    }

    pub const fn is_passthrough(&self) -> bool {
        matches!(self, Self::Native { .. })
    }

    pub const fn requires_bridge_headers(&self) -> bool {
        !self.is_passthrough()
    }
    pub const fn uses_local_continuation_state(&self) -> bool {
        !self.is_passthrough()
    }

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
        }
    }

    pub fn into_stream_bridge(self) -> Option<AdapterStreamBridge> {
        match self {
            Self::Native { .. } => None,
            Self::Translated { request } => Some(AdapterStreamBridge::Translated(Box::new(
                super::super::translation::TranslationStream::new(*request),
            ))),
        }
    }

    /// Policy is applied once, after conversion and before upstream I/O.
    pub(crate) fn apply_tool_policy(
        &mut self,
        policy: &ToolPolicy,
        diagnostics_enabled: bool,
    ) -> Result<ToolPolicyResult, &'static str> {
        apply_tool_policy(self.upstream_body_mut(), policy, diagnostics_enabled)
    }
}
