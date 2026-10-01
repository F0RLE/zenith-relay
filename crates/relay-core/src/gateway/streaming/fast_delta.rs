use serde::de::IgnoredAny;
use serde::Deserialize;

/// Ordinary Responses token delta.
///
/// `output_index` is absent when the upstream did not number the output.
/// `nonempty_text` is the same text signal the full parser uses for time to
/// first token.
pub(in crate::gateway) struct FastResponseDelta {
    pub(in crate::gateway) output_index: Option<u64>,
    pub(in crate::gateway) nonempty_text: bool,
}

/// Classifies one complete JSON payload without building a `serde_json::Value`.
///
/// Streaming time is dominated by `response.*.delta` frames. Usage, terminal
/// status, identifiers, tool items, and anything else that can change
/// settlement stay on the full parser. A `None` result is not an error.
pub(in crate::gateway) fn fast_response_delta_json(payload: &[u8]) -> Option<FastResponseDelta> {
    if !payload.windows(6).any(|window| window == b".delta") {
        return None;
    }
    let parsed: FastDeltaBody<'_> = serde_json::from_slice(payload).ok()?;
    if parsed.has_slow_fields() {
        return None;
    }
    let kind = parsed.kind?;
    // Same text-delta names as `has_output_delta`. Other `response.*.delta`
    // events, including compaction, can change route ownership or replay and
    // stay on the full parser.
    if !is_fast_output_delta(kind) {
        return None;
    }
    Some(FastResponseDelta {
        output_index: parsed.output_index,
        nonempty_text: parsed.delta.is_some_and(|delta| !delta.is_empty()),
    })
}

/// `frame` must be one complete SSE event, including its blank-line terminator.
pub(in crate::gateway::streaming) fn fast_response_delta(
    frame: &[u8],
) -> Option<FastResponseDelta> {
    fast_response_delta_json(single_json_data(frame)?)
}

#[derive(Deserialize)]
struct FastDeltaBody<'a> {
    #[serde(borrow, default, rename = "type")]
    kind: Option<&'a str>,
    #[serde(borrow, default)]
    delta: Option<&'a str>,
    #[serde(default)]
    output_index: Option<u64>,
    #[serde(default)]
    usage: Option<IgnoredAny>,
    #[serde(default, rename = "usageMetadata")]
    usage_metadata: Option<IgnoredAny>,
    #[serde(default)]
    error: Option<IgnoredAny>,
    #[serde(default)]
    response: Option<IgnoredAny>,
    #[serde(default)]
    message: Option<IgnoredAny>,
    #[serde(default)]
    body: Option<IgnoredAny>,
    #[serde(default)]
    service_tier: Option<IgnoredAny>,
    #[serde(default)]
    choices: Option<IgnoredAny>,
    #[serde(default)]
    candidates: Option<IgnoredAny>,
    #[serde(default)]
    content_block: Option<IgnoredAny>,
    #[serde(default)]
    id: Option<IgnoredAny>,
    #[serde(default)]
    item: Option<IgnoredAny>,
    #[serde(default)]
    status: Option<IgnoredAny>,
}

impl FastDeltaBody<'_> {
    fn has_slow_fields(&self) -> bool {
        self.usage.is_some()
            || self.usage_metadata.is_some()
            || self.error.is_some()
            || self.response.is_some()
            || self.message.is_some()
            || self.body.is_some()
            || self.service_tier.is_some()
            || self.choices.is_some()
            || self.candidates.is_some()
            || self.content_block.is_some()
            || self.id.is_some()
            || self.item.is_some()
            || self.status.is_some()
    }
}

fn is_fast_output_delta(kind: &str) -> bool {
    matches!(
        kind,
        "response.output_text.delta"
            | "response.reasoning_text.delta"
            | "response.reasoning_summary_text.delta"
            | "response.refusal.delta"
            | "response.function_call_arguments.delta"
            | "response.custom_tool_call_input.delta"
            | "response.mcp_call_arguments.delta"
            | "response.code_interpreter_call_code.delta"
    )
}

fn single_json_data(frame: &[u8]) -> Option<&[u8]> {
    let mut payload = None;
    for line in crate::protocol::sse_lines(frame) {
        if line.is_empty() || line.first() == Some(&b':') {
            continue;
        }
        if let Some(value) = line.strip_prefix(b"data:") {
            if payload.is_some() {
                return None;
            }
            payload = Some(value.strip_prefix(b" ").unwrap_or(value));
            continue;
        }
        match line.split(|byte| *byte == b':').next() {
            Some(b"event" | b"id" | b"retry") => {}
            _ => return None,
        }
    }
    let payload = trim_ascii(payload?);
    payload
        .first()
        .is_some_and(|byte| *byte == b'{')
        .then_some(payload)
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

#[cfg(test)]
mod tests {
    use super::{fast_response_delta, fast_response_delta_json};

    #[test]
    fn recognizes_a_flushed_response_text_delta() {
        let frame = b"event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"delta\":\"hi\"}\n\n";
        let delta = fast_response_delta(frame).unwrap();
        assert_eq!(delta.output_index, Some(0));
        assert!(delta.nonempty_text);

        let crlf = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"\"}\r\n\r\n";
        assert!(!fast_response_delta(crlf).unwrap().nonempty_text);

        let json =
            br#"{"type":"response.function_call_arguments.delta","output_index":2,"delta":"{"}"#;
        assert_eq!(
            fast_response_delta_json(json).unwrap().output_index,
            Some(2)
        );
    }

    #[test]
    fn leaves_usage_terminal_and_split_data_on_the_full_parser() {
        assert!(fast_response_delta(
            b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1}}}\n\n"
        )
        .is_none());
        assert!(fast_response_delta(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\",\"usage\":{\"input_tokens\":4}}\n\n"
        )
        .is_none());
        assert!(fast_response_delta(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\",\"id\":\"resp_1\"}\n\n"
        )
        .is_none());
        assert!(fast_response_delta(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"a\"}\ndata: {\"delta\":\"b\"}\n\n"
        )
        .is_none());
        assert!(fast_response_delta(b"data: [DONE]\n\n").is_none());
        assert!(fast_response_delta_json(br#"{"type":"response.created"}"#).is_none());
        assert!(fast_response_delta_json(
            br#"{"type":"response.compaction.delta","delta":"opaque"}"#
        )
        .is_none());
    }
}
