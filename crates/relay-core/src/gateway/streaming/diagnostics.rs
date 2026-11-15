use crate::{error_codes, usage::UpstreamErrorDetails};
use serde_json::json;

pub(super) fn incomplete_stream(
    http_status: u16,
    headers: &reqwest::header::HeaderMap,
    body: &[u8],
    inspected: usize,
) -> UpstreamErrorDetails {
    let content_type = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or_default().trim());
    let format = match content_type {
        Some(value) if value.eq_ignore_ascii_case("text/event-stream") => "sse",
        Some(value) if value.eq_ignore_ascii_case("application/json") => "json",
        Some(value) if value.eq_ignore_ascii_case("text/html") => "html",
        Some(_) => "other",
        None => "missing",
    };
    let kind = match body.trim_ascii().first() {
        None => "empty",
        Some(b'{') => "object",
        Some(b'[') => "array_or_marker",
        Some(b'<') => "markup",
        _ => "other",
    };
    UpstreamErrorDetails::from_value(
        Some(http_status),
        &json!({
            "code": error_codes::STREAM_INCOMPLETE,
            "type": "relay_stream_eof",
            "message": format!(
                "Upstream ended before a usable SSE event: content_type={format}; body_bytes={}; framed_bytes={inspected}; pending_bytes={}; kind={kind}",
                body.len(), body.len().saturating_sub(inspected),
            ),
        }),
    )
}

pub(super) fn transport_error(error: &(dyn std::error::Error + 'static)) -> UpstreamErrorDetails {
    let mut source = Some(error);
    let mut timeout = false;
    let mut body = false;
    let mut decode = false;
    let mut io = "unknown";
    let mut http2 = "none";
    let mut http2_reason = "none";
    // Error Display strings and URLs may contain credentials or provider data.
    // Inspect a bounded source chain for fixed transport categories only.
    for _ in 0..8 {
        let Some(current) = source else { break };
        if let Some(error) = current.downcast_ref::<reqwest::Error>() {
            timeout |= error.is_timeout();
            body |= error.is_body();
            decode |= error.is_decode();
        }
        if let Some(error) = current.downcast_ref::<h2::Error>() {
            http2 = if error.is_reset() {
                "reset"
            } else if error.is_go_away() {
                "go_away"
            } else {
                "other"
            };
            http2_reason = match error.reason() {
                None => "none",
                Some(h2::Reason::NO_ERROR) => "no_error",
                Some(h2::Reason::PROTOCOL_ERROR) => "protocol_error",
                Some(h2::Reason::INTERNAL_ERROR) => "internal_error",
                Some(h2::Reason::FLOW_CONTROL_ERROR) => "flow_control_error",
                Some(h2::Reason::SETTINGS_TIMEOUT) => "settings_timeout",
                Some(h2::Reason::STREAM_CLOSED) => "stream_closed",
                Some(h2::Reason::FRAME_SIZE_ERROR) => "frame_size_error",
                Some(h2::Reason::REFUSED_STREAM) => "refused_stream",
                Some(h2::Reason::CANCEL) => "cancel",
                Some(h2::Reason::COMPRESSION_ERROR) => "compression_error",
                Some(h2::Reason::CONNECT_ERROR) => "connect_error",
                Some(h2::Reason::ENHANCE_YOUR_CALM) => "enhance_your_calm",
                Some(h2::Reason::INADEQUATE_SECURITY) => "inadequate_security",
                Some(h2::Reason::HTTP_1_1_REQUIRED) => "http_1_1_required",
                Some(_) => "unknown",
            };
        }
        if let Some(error) = current.downcast_ref::<std::io::Error>() {
            timeout |= error.kind() == std::io::ErrorKind::TimedOut;
            io = match error.kind() {
                std::io::ErrorKind::TimedOut => "timed_out",
                std::io::ErrorKind::ConnectionReset => "connection_reset",
                std::io::ErrorKind::ConnectionAborted => "connection_aborted",
                std::io::ErrorKind::UnexpectedEof => "unexpected_eof",
                std::io::ErrorKind::BrokenPipe => "broken_pipe",
                _ => "other",
            };
        }
        source = current
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .map(|inner| inner as &(dyn std::error::Error + 'static))
            .or_else(|| current.source());
    }
    UpstreamErrorDetails::from_value(
        None,
        &json!({
            "code": error_codes::UPSTREAM_STREAM,
            "type": "relay_stream_transport",
            "message": format!("Relay stream transport: timeout={timeout}; body={body}; decode={decode}; io={io}; http2={http2}; http2_reason={http2_reason}"),
        }),
    )
}

pub(super) fn invalid_event(
    frame: &[u8],
    sse_data: &[u8],
    error: &serde_json::Error,
) -> UpstreamErrorDetails {
    let mut data_lines = 0;
    let mut event_fields = 0;
    for line in crate::protocol::sse_lines(frame) {
        data_lines += usize::from(line.starts_with(b"data:"));
        event_fields += usize::from(line.starts_with(b"event:"));
    }
    let event_field = event_fields > 0;
    let cr_only = frame
        .iter()
        .enumerate()
        .filter(|(index, byte)| **byte == b'\r' && frame.get(index + 1) != Some(&b'\n'))
        .count();
    let data_kind = match sse_data.trim_ascii().first() {
        Some(b'{') => "object",
        Some(b'[') => "array_or_marker",
        Some(b'<') => "markup",
        _ => "other",
    };
    // serde's Display and the upstream event name can contain arbitrary data.
    // Persist only fixed labels, counts and positions, never payload fragments.
    let message = format!(
        "Relay SSE parser: invalid JSON; category={:?}; line={}; column={}; frame_bytes={}; data_bytes={}; data_lines={data_lines}; event_field={event_field}; utf8={}; kind={data_kind}; event_fields={event_fields}; cr_only={cr_only}",
        error.classify(),
        error.line(),
        error.column(),
        frame.len(),
        sse_data.len(),
        std::str::from_utf8(sse_data).is_ok(),
    );
    UpstreamErrorDetails::from_value(
        None,
        &json!({
            "code": error_codes::STREAM_INVALID,
            "type": "relay_stream_parser",
            "message": message,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::super::parse_sse_event;
    use super::*;

    #[test]
    fn transport_diagnostics_keep_categories_without_error_text() {
        for (kind, label) in [
            (std::io::ErrorKind::ConnectionReset, "connection_reset"),
            (std::io::ErrorKind::UnexpectedEof, "unexpected_eof"),
            (std::io::ErrorKind::TimedOut, "timed_out"),
        ] {
            let error = std::io::Error::new(kind, "synthetic-private-url-and-body");
            let details = transport_error(&error);
            assert_eq!(details.code.as_deref(), Some(error_codes::UPSTREAM_STREAM));
            assert_eq!(
                details.error_type.as_deref(),
                Some("relay_stream_transport")
            );
            assert!(
                details
                    .message
                    .as_ref()
                    .unwrap()
                    .contains(&format!("io={label}"))
            );
            assert!(
                !serde_json::to_string(&details)
                    .unwrap()
                    .contains("synthetic-private")
            );
        }
    }

    #[test]
    fn http2_diagnostics_use_typed_reasons_not_untrusted_error_text() {
        for (reason, label) in [
            (h2::Reason::CANCEL, "cancel"),
            (h2::Reason::REFUSED_STREAM, "refused_stream"),
            (h2::Reason::ENHANCE_YOUR_CALM, "enhance_your_calm"),
        ] {
            let error = std::io::Error::other(h2::Error::from(reason));
            let details = transport_error(&error);
            assert_eq!(details.code.as_deref(), Some(error_codes::UPSTREAM_STREAM));
            assert!(
                details
                    .message
                    .unwrap()
                    .contains(&format!("http2_reason={label}"))
            );
        }
        let forged = std::io::Error::other(
            "stream error: stream ID 1; CANCEL; received from peer; synthetic-private",
        );
        let details = transport_error(&forged);
        let message = details.message.unwrap();
        assert!(message.contains("http2=none; http2_reason=none"));
        assert!(!message.contains("synthetic-private"));
    }

    #[test]
    fn malformed_frames_record_structure_without_payload_or_event_names() {
        for (data, expected_category) in [
            ("{\"synthetic-private\":", "Eof"),
            ("{\"synthetic-private\":1} {\"second\":2}", "Syntax"),
            ("synthetic-private", "Syntax"),
        ] {
            let frame = format!("event: synthetic-private\ndata: {data}\n\n");
            let event = parse_sse_event(frame.as_bytes());
            assert!(event.has_data && !event.valid);
            let details = event.upstream_error.unwrap();
            let serialized = serde_json::to_string(&details).unwrap();
            assert!(!serialized.contains("synthetic-private"));
            assert!(!serialized.contains("second"));
            assert_eq!(details.code.as_deref(), Some(error_codes::STREAM_INVALID));
            let message = details.message.as_ref().unwrap();
            assert!(message.contains(&format!("category={expected_category}")));
            assert!(message.contains("data_lines=1; event_field=true; utf8=true"));
            let restored: UpstreamErrorDetails = serde_json::from_str(&serialized).unwrap();
            assert_eq!(restored, details);
        }
    }

    #[test]
    fn multline_and_non_utf8_failures_remain_distinguishable() {
        let event = parse_sse_event(b"data: {}\ndata: {}\n\n");
        let details = event.upstream_error.unwrap();
        let message = details.message.unwrap();
        assert!(message.contains("category=Syntax; line=2; column=1"));
        assert!(message.contains("data_lines=2; event_field=false"));
        let event = parse_sse_event(b"data: {\"text\":\"\xff\"}\n\n");
        assert!(
            event
                .upstream_error
                .unwrap()
                .message
                .unwrap()
                .contains("utf8=false")
        );
    }

    #[test]
    fn missing_event_separators_are_reported_without_guessing_boundaries() {
        let event =
            parse_sse_event(b"event: private-one\rdata: {}\revent: private-two\rdata: {}\r\r");
        assert!(event.has_data && !event.valid);
        let message = event.upstream_error.unwrap().message.unwrap();
        assert!(message.contains("data_lines=2"));
        assert!(message.contains("event_fields=2; cr_only=5"));
        assert!(!message.contains("private"));
    }
}
