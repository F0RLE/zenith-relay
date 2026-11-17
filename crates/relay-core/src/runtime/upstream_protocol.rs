//! Per-principal upstream selection; client wire format and transport stay separate.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamProtocolPolicy {
    #[default]
    Auto,
    Responses,
    ChatCompletions,
    Messages,
    Gemini,
}

impl UpstreamProtocolPolicy {
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::Responses,
        Self::ChatCompletions,
        Self::Messages,
        Self::Gemini,
    ];

    pub fn protocol(self) -> Option<WireApi> {
        match self {
            Self::Auto => None,
            Self::Responses => Some(WireApi::Responses),
            Self::ChatCompletions => Some(WireApi::ChatCompletions),
            Self::Messages => Some(WireApi::Messages),
            Self::Gemini => Some(WireApi::Gemini),
        }
    }

    pub fn allows(self, upstream: WireApi) -> bool {
        self.protocol().is_none_or(|selected| selected == upstream)
    }
}

impl GatewayRuntime {
    /// Each request snapshots this preference, including requests on an existing
    /// WebSocket. In-flight work retains its choice; scope revocation stays live.
    pub fn set_key_upstream_protocol(&self, key_id: &str, policy: UpstreamProtocolPolicy) -> bool {
        let Some(key) = self.keys.iter().find(|key| key.id == key_id) else {
            return false;
        };
        *crate::poison::write(&key.upstream_protocol) = policy;
        true
    }

    pub(crate) fn snapshot_request_key(&self, key: &AuthenticatedKey) -> AuthenticatedKey {
        let mut snapshot = key.clone();
        if let Some(current) = self.keys.iter().find(|current| current.id == key.id) {
            snapshot.upstream_protocol = *crate::poison::read(&current.upstream_protocol);
        }
        snapshot
    }

    pub(crate) fn validate_key_upstream_protocol(
        &self,
        key: &AuthenticatedKey,
        route: &ExecutorRoute,
        request: &Value,
        now_ms: u64,
    ) -> crate::AdapterResult<()> {
        if key.upstream_protocol.allows(
            route
                .adapter
                .upstream_protocol(route.client_wire_api)
                .wire_api(),
        ) || self.has_retained_continuation_route(route, &key.id, request, now_ms)
        {
            Ok(())
        } else {
            Err(crate::AdapterError::upstream_protocol_unavailable())
        }
    }
}
