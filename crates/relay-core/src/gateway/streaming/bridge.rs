use super::{
    preserved_stream_error, rewrite_bridge_failure, PreservedUpstreamError, UpstreamStream,
};
use crate::{AdapterStreamBridge, TranslatedResponse};
use axum::body::Bytes;
use futures_util::{stream, Stream, StreamExt};
use std::collections::VecDeque;
use std::time::Duration;
use tokio::time::Instant;

const UPSTREAM_END_TIMEOUT: Duration = Duration::from_secs(30);

struct AdapterBridgeStreamState<F> {
    inner: UpstreamStream,
    bridge: AdapterStreamBridge,
    pending: VecDeque<Bytes>,
    finished: bool,
    end_deadline: Option<Instant>,
    on_completed: Option<F>,
}

pub(super) fn bridge_adapter_stream(
    first: Bytes,
    remaining: UpstreamStream,
    bridge: AdapterStreamBridge,
    on_completed: impl FnOnce(TranslatedResponse) + Send + 'static,
) -> impl Stream<Item = Result<Bytes, reqwest::Error>> + Send {
    let inner = stream::once(async move { Ok::<Bytes, reqwest::Error>(first) }).chain(remaining);
    stream::unfold(
        AdapterBridgeStreamState {
            inner: Box::pin(inner),
            bridge,
            pending: VecDeque::new(),
            finished: false,
            end_deadline: None,
            on_completed: Some(on_completed),
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
                if let Some(response) = stream_state.bridge.take_completed() {
                    if let Some(on_completed) = stream_state.on_completed.take() {
                        on_completed(response);
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AdapterRequestContext, CacheWriteTtl, MessagesReasoningMode, SourceAdapter, WireApi,
    };
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn completed_bridge_response_moves_body_and_continuation_without_cloning() {
        let request = json!({"input":"Synthetic input"});
        let mut bridge = SourceAdapter::between(WireApi::Responses, WireApi::Messages)
            .unwrap()
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Responses,
                request: &request,
                model: "test",
                stream: true,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: None,
                response_scope: "synthetic-source",
                response_id_seed: "synthetic-request",
            })
            .unwrap()
            .into_stream_bridge()
            .unwrap();
        assert!(bridge.take_completed().is_none());
        for event in [
            json!({"type":"message_start","message":{"id":"msg_test","content":[],"usage":{"input_tokens":3}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"Synthetic reasoning","signature":"synthetic-signature"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":"Hi"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
            json!({"type":"message_stop"}),
        ] {
            bridge.push(format!("data: {event}\n\n").as_bytes());
        }
        let response = bridge.completed().expect("synthetic stream completes");
        let body_pointer = response.response_body["output"][1]["content"][0]["text"]
            .as_str()
            .unwrap()
            .as_ptr();
        let continuation_pointer = response.continuation.model().as_ptr();
        let completed = Arc::new(Mutex::new(None));
        let captured = completed.clone();
        let forwarded = bridge_adapter_stream(
            Bytes::new(),
            Box::pin(stream::pending()),
            bridge,
            move |response| *crate::poison::mutex(&captured) = Some(response),
        );
        futures_util::pin_mut!(forwarded);
        let first = forwarded.next().await.unwrap();
        assert!(
            crate::poison::mutex(&completed).is_some(),
            "continuation must be saved before client frames are returned"
        );
        let mut frames = vec![first];
        frames.extend(
            tokio::time::timeout(Duration::from_secs(5), forwarded.collect::<Vec<_>>())
                .await
                .expect("terminal bridge must not poll the remaining upstream"),
        );
        let wire = String::from_utf8(
            frames
                .into_iter()
                .flat_map(|frame| frame.unwrap())
                .collect(),
        )
        .unwrap();
        assert_eq!(wire.matches("event: response.completed\n").count(), 1);
        let response = crate::poison::mutex(&completed).take().unwrap();
        assert_eq!(response.response_body["usage"]["input_tokens"], 3);
        assert_eq!(response.response_body["usage"]["output_tokens"], 2);
        assert_eq!(
            response.response_body["output"][1]["content"][0]["text"]
                .as_str()
                .unwrap()
                .as_ptr(),
            body_pointer
        );
        assert_eq!(response.continuation.model().as_ptr(), continuation_pointer);
        assert_eq!(response.continuation.model(), "test");
        assert_eq!(response.continuation.upstream_wire_api(), WireApi::Messages);
        assert!(response.continuation.is_reusable());
        assert!(crate::poison::mutex(&completed).is_none());
    }
}
