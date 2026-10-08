use super::{
    preserved_stream_error, rewrite_bridge_failure, AdapterStreamBridge, MessagesBridgeResponse,
    MessagesStreamBridge, PreservedUpstreamError, UpstreamStream,
};
use axum::body::Bytes;
use futures_util::{stream, Stream, StreamExt};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

struct AdapterBridgeStreamState {
    inner: UpstreamStream,
    bridge: AdapterStreamBridge,
    pending: VecDeque<Bytes>,
    finished: bool,
    completed: Arc<Mutex<Option<MessagesBridgeResponse>>>,
}

/// Translates a native Messages SSE stream into the client-facing Responses
/// SSE contract. The completed bridge response is published before the
/// `response.completed` frame is yielded so the usage callback can persist the
/// continuation without exposing native content outside the local bridge.
pub(super) fn bridge_messages_stream(
    first: Bytes,
    remaining: UpstreamStream,
    bridge: MessagesStreamBridge,
    completed: Arc<Mutex<Option<MessagesBridgeResponse>>>,
) -> impl Stream<Item = Result<Bytes, reqwest::Error>> + Send {
    bridge_adapter_stream(
        first,
        remaining,
        AdapterStreamBridge::Messages(Box::new(bridge)),
        completed,
    )
}

pub(super) fn bridge_adapter_stream(
    first: Bytes,
    remaining: UpstreamStream,
    bridge: AdapterStreamBridge,
    completed: Arc<Mutex<Option<MessagesBridgeResponse>>>,
) -> impl Stream<Item = Result<Bytes, reqwest::Error>> + Send {
    let inner = stream::once(async move { Ok::<Bytes, reqwest::Error>(first) }).chain(remaining);
    stream::unfold(
        AdapterBridgeStreamState {
            inner: Box::pin(inner),
            bridge,
            pending: VecDeque::new(),
            finished: false,
            completed,
        },
        |mut stream_state| async move {
            loop {
                if let Some(bytes) = stream_state.pending.pop_front() {
                    return Some((Ok(bytes), stream_state));
                }
                if stream_state.finished {
                    return None;
                }

                let mut preserved_error = None;
                match stream_state.inner.next().await {
                    Some(Ok(bytes)) => {
                        stream_state.bridge.push(&bytes);
                        preserved_error = stream_state
                            .bridge
                            .take_upstream_error()
                            .and_then(|error| preserved_stream_error(&error));
                    }
                    Some(Err(_)) | None => {
                        stream_state.bridge.finish();
                        stream_state.finished = true;
                    }
                }

                queue_bridge_output(
                    &mut stream_state.pending,
                    || stream_state.bridge.pop_output(),
                    preserved_error.as_ref(),
                );
                if let Some(response) = stream_state.bridge.completed().cloned() {
                    *crate::poison::mutex(&stream_state.completed) = Some(response);
                }
                if stream_state.bridge.is_terminal() {
                    stream_state.finished = true;
                }
            }
        },
    )
}

pub(super) fn bridge_gemini_stream(
    first: Bytes,
    remaining: UpstreamStream,
    bridge: crate::GeminiStreamBridge,
    completed: Arc<Mutex<Option<MessagesBridgeResponse>>>,
) -> impl Stream<Item = Result<Bytes, reqwest::Error>> + Send {
    bridge_adapter_stream(
        first,
        remaining,
        AdapterStreamBridge::Gemini(Box::new(bridge)),
        completed,
    )
}

fn queue_bridge_output(
    pending: &mut VecDeque<Bytes>,
    mut next_output: impl FnMut() -> Option<Vec<u8>>,
    error: Option<&PreservedUpstreamError>,
) {
    while let Some(bytes) = next_output() {
        pending.push_back(Bytes::from(rewrite_bridge_failure(bytes, error)));
    }
}
