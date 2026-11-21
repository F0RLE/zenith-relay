use serde_json::Value;

/// Codex sometimes uses generation event names for edits. Only the account edit
/// route enables this normalization; API-source frames otherwise remain native.
pub(super) fn codex_edit_frame(
    frame: &[u8],
    data: &[u8],
    kind: &str,
    event_limit: usize,
) -> Option<Vec<u8>> {
    let mut payload: Value = serde_json::from_slice(data).ok()?;
    payload
        .as_object_mut()?
        .insert("type".into(), Value::String(kind.to_owned()));
    let data = serde_json::to_vec(&payload).ok()?;
    let mut output = Vec::with_capacity(frame.len());
    let mut wrote_data = false;
    for line in crate::protocol::sse::lines(frame) {
        if line.starts_with(b"data:") {
            if !wrote_data {
                output.extend_from_slice(b"data: ");
                output.extend_from_slice(&data);
                output.push(b'\n');
                wrote_data = true;
            }
        } else if let Some(event) = line.strip_prefix(b"event:")
            && event
                .strip_prefix(b" ")
                .unwrap_or(event)
                .starts_with(b"image_generation.")
        {
            output.extend_from_slice(b"event: ");
            output.extend_from_slice(kind.as_bytes());
            output.push(b'\n');
        } else if !line.is_empty() {
            output.extend_from_slice(line);
            output.push(b'\n');
        }
    }
    output.push(b'\n');
    (wrote_data && output.len() <= event_limit).then_some(output)
}
