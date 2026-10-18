use super::{
    preserved_stream_error, rewrite_bridge_failure, PreservedUpstreamError, TranslatedResponse,
    UpstreamStream,
};
use crate::AdapterStreamBridge;
use axum::body::Bytes;
use futures_util::{stream, Stream, StreamExt};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

const UPSTREAM_END_TIMEOUT: Duration = Duration::from_secs(30);

struct AdapterBridgeStreamState {
    inner: UpstreamStream,
    bridge: AdapterStreamBridge,
    pending: VecDeque<Bytes>,
    finished: bool,
    end_deadline: Option<Instant>,
    completed: Arc<Mutex<Option<TranslatedResponse>>>,
}

pub(super) fn bridge_adapter_stream(
    first: Bytes,
    remaining: UpstreamStream,
    bridge: AdapterStreamBridge,
    completed: Arc<Mutex<Option<TranslatedResponse>>>,
) -> impl Stream<Item = Result<Bytes, reqwest::Error>> + Send {
    let inner = stream::once(async move { Ok::<Bytes, reqwest::Error>(first) }).chain(remaining);
    stream::unfold(
        AdapterBridgeStreamState {
            inner: Box::pin(inner),
            bridge,
            pending: VecDeque::new(),
            finished: false,
            end_deadline: None,
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
                // Once Gemini declares its finish reason, drain late usage until
                // EOF. This deadline does not limit active model generation.
                if stream_state.bridge.awaiting_upstream_end()
                    && stream_state.end_deadline.is_none()
                {
                    stream_state.end_deadline = Some(Instant::now() + UPSTREAM_END_TIMEOUT);
                }
                let next = if let Some(deadline) = stream_state.end_deadline {
                    tokio::time::timeout_at(deadline, stream_state.inner.next()).await
                } else {
                    Ok(stream_state.inner.next().await)
                };
                match next {
                    Ok(Some(Ok(bytes))) => {
                        stream_state.bridge.push(&bytes);
                        preserved_error = stream_state
                            .bridge
                            .take_upstream_error()
                            .and_then(|error| preserved_stream_error(&error));
                    }
                    Ok(None) => {
                        stream_state.bridge.finish();
                        stream_state.finished = true;
                    }
                    Ok(Some(Err(_))) | Err(_) => {
                        stream_state.bridge.abort();
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

fn queue_bridge_output(
    pending: &mut VecDeque<Bytes>,
    mut next_output: impl FnMut() -> Option<Vec<u8>>,
    error: Option<&PreservedUpstreamError>,
) {
    while let Some(bytes) = next_output() {
        pending.push_back(Bytes::from(rewrite_bridge_failure(bytes, error)));
    }
}
