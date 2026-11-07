use super::{GatewayFailure, MAX_NAMED_STREAMS};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

struct CachedResponse {
    identity: [u8; 32],
    transient_affinity: Option<String>,
}

/// Latest response per lane on one physical upstream connection. Only opaque
/// identifiers are fingerprinted here; replay data stays with its existing owner.
pub(super) struct ConnectionResponses {
    lanes: BTreeMap<Option<String>, Option<CachedResponse>>,
}

impl ConnectionResponses {
    pub(super) fn new(stream_id: Option<String>) -> Self {
        Self {
            lanes: BTreeMap::from([(stream_id, None)]),
        }
    }

    pub(super) fn register(&mut self, stream_id: Option<&str>) -> Result<(), GatewayFailure> {
        let lane = stream_id.map(str::to_owned);
        if lane.is_some()
            && !self.lanes.contains_key(&lane)
            && self.lanes.keys().filter(|lane| lane.is_some()).count() >= MAX_NAMED_STREAMS
        {
            return Err(stream_limit_failure());
        }
        self.lanes.entry(lane).or_default();
        Ok(())
    }

    fn response(&self, response_id: &str) -> Option<&CachedResponse> {
        let identity: [u8; 32] = Sha256::digest(response_id.as_bytes()).into();
        self.lanes
            .values()
            .flatten()
            .find(|response| response.identity == identity)
    }

    pub(super) fn contains(&self, response_id: &str) -> bool {
        self.response(response_id).is_some()
    }

    pub(super) fn contains_on_lane(&self, lane: Option<&str>, response_id: &str) -> bool {
        let identity: [u8; 32] = Sha256::digest(response_id.as_bytes()).into();
        self.lanes
            .get(&lane.map(str::to_owned))
            .and_then(Option::as_ref)
            .is_some_and(|response| response.identity == identity)
    }

    pub(super) fn transient_affinity(&self, response_id: &str) -> Option<&str> {
        self.response(response_id)?.transient_affinity.as_deref()
    }

    pub(super) fn retire_failed_continuation(
        &mut self,
        stream_id: Option<&str>,
        previous_response_id: Option<&str>,
    ) -> Option<String> {
        let identity: [u8; 32] = Sha256::digest(previous_response_id?.as_bytes()).into();
        let slot = self.lanes.get_mut(&stream_id.map(str::to_owned))?;
        if slot.as_ref()?.identity != identity {
            return None;
        }
        slot.take()?.transient_affinity
    }

    /// Returns the retired volatile binding for removal by the runtime owner.
    pub(super) fn record(
        &mut self,
        stream_id: Option<&str>,
        response_id: Option<&str>,
        transient_affinity: Option<String>,
    ) -> Option<String> {
        let Some(slot) = self.lanes.get_mut(&stream_id.map(str::to_owned)) else {
            return transient_affinity;
        };
        let retired = slot.take().and_then(|response| response.transient_affinity);
        *slot = response_id.map(|id| CachedResponse {
            identity: Sha256::digest(id.as_bytes()).into(),
            transient_affinity,
        });
        retired
    }

    /// Socket replacement drops cached identities, but named-lane admission is
    /// bounded for the entire client connection, including its reconnects.
    pub(super) fn clear(&mut self) -> Vec<String> {
        self.lanes
            .values_mut()
            .filter_map(Option::take)
            .filter_map(|response| response.transient_affinity)
            .collect()
    }
}

pub(super) fn stream_limit_failure() -> GatewayFailure {
    let mut failure = GatewayFailure::invalid_request(
        "maximum number of distinct WebSocket stream IDs reached; reuse a stream_id or reconnect",
    );
    failure.category = "websocket_stream_limit_reached";
    failure
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_responses_keep_source_and_fork_lineage_separate() {
        let mut responses = ConnectionResponses::new(Some("source".into()));
        assert!(responses.register(Some("fork")).is_ok());
        assert!(responses.register(None).is_ok());
        assert_eq!(
            responses.record(
                Some("source"),
                Some("parent"),
                Some("binding-source".into())
            ),
            None
        );
        assert_eq!(
            responses.record(Some("fork"), Some("child"), Some("binding-fork".into())),
            None
        );
        assert_eq!(
            responses.transient_affinity("parent"),
            Some("binding-source")
        );
        assert!(responses.contains("child"));
        assert!(responses.contains_on_lane(Some("source"), "parent"));
        assert!(!responses.contains_on_lane(Some("fork"), "parent"));
        assert!(!responses.contains_on_lane(None, "parent"));
        assert!(!responses.contains("unknown"));
        assert_eq!(
            responses.record(Some("fork"), Some("next-child"), None),
            Some("binding-fork".into())
        );
        assert!(!responses.contains("child"));
        assert!(responses.contains("parent"));
        assert!(responses.contains("next-child"));
        assert_eq!(responses.clear(), ["binding-source"]);
        assert!(!responses.contains("parent"));
        assert!(!responses.contains("next-child"));
        assert!(responses.clear().is_empty());
    }

    #[test]
    fn connection_responses_bound_named_lanes_across_socket_replacement() {
        let mut responses = ConnectionResponses::new(None);
        for index in 0..MAX_NAMED_STREAMS {
            assert!(responses.register(Some(&format!("lane-{index}"))).is_ok());
        }
        responses.clear();
        assert!(responses.register(None).is_ok());
        assert!(responses.register(Some("lane-0")).is_ok());
        assert_eq!(
            responses.register(Some("extra")).err().unwrap().category,
            "websocket_stream_limit_reached"
        );
        assert_eq!(
            responses.record(Some("extra"), Some("response"), Some("unowned".into())),
            Some("unowned".into())
        );
        assert!(!responses.contains("response"));
    }

    #[test]
    fn connection_responses_evict_failed_same_lane_but_keep_cross_lane_parents() {
        let mut responses = ConnectionResponses::new(Some("source".into()));
        assert!(responses.register(Some("fork")).is_ok());
        responses.record(Some("source"), Some("parent"), Some("volatile".into()));
        assert_eq!(
            responses.retire_failed_continuation(Some("fork"), Some("parent")),
            None
        );
        assert!(responses.contains("parent"));
        assert_eq!(
            responses.retire_failed_continuation(Some("source"), Some("other")),
            None
        );
        assert!(responses.contains("parent"));
        assert_eq!(
            responses.retire_failed_continuation(Some("source"), Some("parent")),
            Some("volatile".into())
        );
        assert!(!responses.contains("parent"));
        assert!(responses.clear().is_empty());
    }
}
