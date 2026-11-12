use super::{MAX_WEBSOCKET_MESSAGE_BYTES, UpstreamMessage};
use serde::Deserialize;
use std::borrow::Cow;

// Borrow ordinary identifiers without materializing large text/tool payloads.
#[derive(Deserialize)]
#[serde(transparent)]
struct Text<'a>(#[serde(borrow)] Cow<'a, str>);

#[derive(Deserialize)]
struct ResponseId<'a> {
    #[serde(borrow)]
    id: Option<Text<'a>>,
}

#[derive(Deserialize)]
struct ResponseEnvelope<'a> {
    #[serde(borrow)]
    id: Option<Text<'a>>,
    #[serde(borrow)]
    response: Option<ResponseId<'a>>,
}

#[derive(Deserialize)]
struct ErrorCode<'a> {
    #[serde(borrow)]
    code: Option<Text<'a>>,
}

#[derive(Deserialize)]
pub(super) struct EventScope<'a> {
    #[serde(borrow, rename = "type")]
    kind: Option<Text<'a>>,
    #[serde(borrow)]
    stream_id: Option<Text<'a>>,
    #[serde(borrow)]
    response_id: Option<Text<'a>>,
    #[serde(borrow)]
    response: Option<ResponseEnvelope<'a>>,
    #[serde(borrow)]
    id: Option<Text<'a>>,
    #[serde(borrow)]
    code: Option<Text<'a>>,
    #[serde(borrow)]
    error: Option<ErrorCode<'a>>,
}

impl<'a> EventScope<'a> {
    pub(super) fn stream_id(&self) -> Option<&str> {
        self.stream_id.as_ref().map(|id| id.0.as_ref())
    }

    pub(super) fn is_error(&self) -> bool {
        self.kind.as_ref().is_some_and(|kind| kind.0 == "error")
    }

    pub(super) fn is_timing_notification(&self) -> bool {
        self.kind
            .as_ref()
            .is_some_and(|kind| kind.0 == "responsesapi.websocket_timing")
    }

    pub(super) fn notification_response_id(&self) -> Option<&str> {
        self.response_id
            .as_ref()
            .or_else(|| {
                self.response
                    .as_ref()
                    .and_then(|response| response.id.as_ref())
            })
            .map(|id| id.0.as_ref())
    }

    pub(super) fn is_connection_error(&self, has_unscoped_owner: bool) -> bool {
        self.is_error()
            && self.stream_id.is_none()
            && self.response_id().is_none()
            && (!has_unscoped_owner
                || self
                    .code
                    .iter()
                    .chain(self.error.iter().filter_map(|error| error.code.as_ref()))
                    .any(|code| code.0 == "websocket_connection_limit_reached"))
    }

    pub(super) fn is_response_event(&self) -> bool {
        self.kind
            .as_ref()
            .is_some_and(|kind| kind.0.starts_with("response."))
    }

    pub(super) fn parse(message: &'a UpstreamMessage) -> Option<Self> {
        let bytes = match message {
            UpstreamMessage::Text(text) => text.as_bytes(),
            UpstreamMessage::Binary(bytes) => bytes.as_ref(),
            _ => return None,
        };
        if bytes.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
            return None;
        }
        serde_json::from_slice(bytes).ok()
    }

    pub(super) fn response_id(&self) -> Option<&str> {
        let kind = self.kind.as_ref()?.0.as_ref();
        if !kind.starts_with("response.") && kind != "error" {
            return None;
        }
        self.response
            .as_ref()
            .and_then(|response| {
                response
                    .response
                    .as_ref()
                    .and_then(|response| response.id.as_ref())
                    .or(response.id.as_ref())
            })
            .or(self.response_id.as_ref())
            .or_else(|| {
                kind.starts_with("response.")
                    .then_some(self.id.as_ref())
                    .flatten()
            })
            .map(|id| id.0.trim())
            .filter(|id| !id.is_empty())
    }

    pub(super) fn matches(&self, stream_id: Option<&str>, response_id: Option<&str>) -> bool {
        if self
            .stream_id
            .as_ref()
            .is_some_and(|id| Some(id.0.as_ref()) != stream_id)
        {
            return false;
        }
        !self
            .response_id()
            .zip(response_id)
            .is_some_and(|(observed, expected)| observed != expected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_lane_and_response_identifiers_must_match_the_request() {
        let message = UpstreamMessage::Text(
            r#"{"type":"response.output_text.delta","stream_id":"ma\u0069n","response_id":"resp-1","delta":"synthetic"}"#.into(),
        );
        let scope = EventScope::parse(&message).unwrap();
        assert!(scope.matches(Some("main"), Some("resp-1")));
        assert!(!scope.matches(None, Some("resp-1")));
        assert!(!scope.matches(Some("other"), Some("resp-1")));
        assert!(!scope.matches(Some("main"), Some("resp-2")));
        assert_eq!(scope.response_id(), Some("resp-1"));
    }

    #[test]
    fn unscoped_legacy_events_and_transport_notifications_keep_their_existing_path() {
        for json in [
            r#"{"type":"response.output_text.delta","delta":"synthetic"}"#,
            r#"{"type":"error","id":"event-id","error":{"code":"server_error"}}"#,
            r#"{"type":"responsesapi.websocket_timing","response_id":"prior-response"}"#,
        ] {
            let message = UpstreamMessage::Binary(json.as_bytes().to_vec().into());
            let scope = EventScope::parse(&message).unwrap();
            assert!(scope.matches(Some("main"), Some("current-response")));
            assert_eq!(scope.response_id(), None);
        }
        assert!(EventScope::parse(&UpstreamMessage::Text("not-json".into())).is_none());
    }

    #[test]
    fn nested_response_identity_and_request_errors_are_scoped() {
        for json in [
            r#"{"type":"response.completed","response":{"id":"resp-1"}}"#,
            r#"{"type":"response.completed","response":{"response":{"id":"resp-1"}}}"#,
            r#"{"type":"error","response_id":"resp-1","error":{"code":"usage_limit_reached"}}"#,
        ] {
            let message = UpstreamMessage::Text(json.into());
            let scope = EventScope::parse(&message).unwrap();
            assert!(scope.matches(None, Some("resp-1")));
            assert!(!scope.matches(None, Some("resp-2")));
        }
    }

    #[test]
    fn timing_identity_does_not_claim_response_ownership() {
        for json in [
            r#"{"type":"responsesapi.websocket_timing","response_id":"resp-1"}"#,
            r#"{"type":"responsesapi.websocket_timing","response":{"id":"resp-1"}}"#,
        ] {
            let message = UpstreamMessage::Text(json.into());
            let scope = EventScope::parse(&message).unwrap();
            assert!(scope.is_timing_notification());
            assert!(!scope.is_response_event());
            assert_eq!(scope.notification_response_id(), Some("resp-1"));
            assert_eq!(scope.response_id(), None);
        }
        let message = UpstreamMessage::Text(
            r#"{"type":"response.future_event","response_id":"resp-1"}"#.into(),
        );
        assert!(
            !EventScope::parse(&message)
                .unwrap()
                .is_timing_notification()
        );
    }

    #[test]
    fn unscoped_request_errors_use_the_owner_but_connection_limits_do_not() {
        for json in [
            r#"{"type":"error","code":"invalid_request_error"}"#,
            r#"{"type":"error","error":{"code":"invalid_request_error"}}"#,
        ] {
            let message = UpstreamMessage::Text(json.into());
            let scope = EventScope::parse(&message).unwrap();
            assert!(scope.is_error());
            assert!(!scope.is_connection_error(true));
            assert!(scope.is_connection_error(false));
        }
        for json in [
            r#"{"type":"error","code":"websocket_connection_limit_reached"}"#,
            r#"{"type":"error","error":{"code":"websocket_connection_limit_reached"}}"#,
        ] {
            let message = UpstreamMessage::Text(json.into());
            let scope = EventScope::parse(&message).unwrap();
            assert!(scope.is_connection_error(true));
            assert!(scope.is_connection_error(false));
        }
    }
}
