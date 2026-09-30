use crate::{error_codes, usage::UpstreamErrorDetails};
use serde_json::json;

pub(super) fn invalid_event(
    frame: &[u8],
    data: &[u8],
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
    let kind = match data.trim_ascii().first() {
        Some(b'{') => "object",
        Some(b'[') => "array_or_marker",
        Some(b'<') => "markup",
        _ => "other",
    };
    // serde's Display and the upstream event name can contain arbitrary data.
    // Persist only fixed labels, counts and positions, never payload fragments.
    let message = format!(
        "Relay SSE parser: invalid JSON; category={:?}; line={}; column={}; frame_bytes={}; data_bytes={}; data_lines={data_lines}; event_field={event_field}; utf8={}; kind={kind}; event_fields={event_fields}; cr_only={cr_only}",
        error.classify(), error.line(), error.column(), frame.len(), data.len(),
        std::str::from_utf8(data).is_ok(),
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
        assert!(event
            .upstream_error
            .unwrap()
            .message
            .unwrap()
            .contains("utf8=false"));
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
