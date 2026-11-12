use super::*;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use futures_util::{SinkExt, StreamExt};

pub(in crate::runtime::tests::basis_points) async fn upgrade(
    State(state): State<Arc<AccessState>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !state.websocket_generations.load(Ordering::SeqCst) {
        state.generation_requests.fetch_add(1, Ordering::SeqCst);
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    state.websocket_requests.fetch_add(1, Ordering::SeqCst);
    if let Some(reply) = crate::poison::mutex(&state.websocket_upgrade_reply).as_ref() {
        return (reply.status, Json(reply.body.clone())).into_response();
    }
    upgrade.on_upgrade(move |socket| serve(state, socket))
}

async fn serve(state: Arc<AccessState>, mut socket: WebSocket) {
    let mut previous: Option<(String, Vec<Value>)> = None;
    while let Some(Ok(message)) = socket.recv().await {
        let text = match message {
            Message::Text(text) => text,
            Message::Ping(_) | Message::Pong(_) => {
                if socket.flush().await.is_err() {
                    return;
                }
                continue;
            }
            _ => return,
        };
        let mut completed_output = crate::gateway::NativeReplayCapture::default();
        let mut body: Value = serde_json::from_str(&text).unwrap();
        crate::poison::mutex(&state.websocket_bodies).push(body.clone());
        assert_eq!(body["type"], "response.create");
        assert!(body.get("stream").is_none());
        let object = body.as_object_mut().unwrap();
        object.remove("type");
        if object.remove("generate") == Some(Value::Bool(false)) {
            object
                .entry("prompt_cache_options")
                .or_insert_with(|| json!({}))["prewarm"] = json!(true);
        }
        if let Some(id) = object.remove("previous_response_id") {
            let (prior_id, history) = previous.as_ref().expect("same-connection predecessor");
            assert_eq!(id, *prior_id);
            let mut full = history.clone();
            full.extend(object["input"].as_array().unwrap().iter().cloned());
            object.insert("input".into(), json!(full));
        }
        object.insert("stream".into(), json!(true));
        let input = body["input"].as_array().unwrap().clone();
        let response = generation(State(state.clone()), Json(body)).await;
        let status = response.status();
        let is_sse = status.is_success() && state.stream_generations.load(Ordering::SeqCst);
        if !is_sse {
            let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            let event = if !status.is_success()
                || body.get("error").is_some() && body.get("status").is_none()
            {
                json!({"type":"error","status":status.as_u16(),"error":body["error"]})
            } else {
                json!({"type":format!("response.{}", body["status"].as_str().unwrap_or("failed")),"response":body})
            };
            if !send_event(
                &mut socket,
                event.to_string(),
                &input,
                &mut previous,
                &mut completed_output,
            )
            .await
            {
                return;
            }
            if event["type"] == "response.in_progress" {
                return;
            }
            continue;
        }
        let mut stream = response.into_body().into_data_stream();
        let mut pending = Vec::new();
        let mut terminal = false;
        while let Some(chunk) = stream.next().await {
            pending.extend_from_slice(&chunk.unwrap());
            while let Some(frame) = crate::protocol::take_sse_event(&mut pending) {
                let data = crate::protocol::sse_data(&frame);
                let text = String::from_utf8(data.into_owned()).unwrap();
                if text == "[DONE]" {
                    continue;
                }
                let event: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                terminal = matches!(
                    event["type"].as_str(),
                    Some(
                        "error" | "response.completed" | "response.incomplete" | "response.failed"
                    )
                );
                if !send_event(
                    &mut socket,
                    text,
                    &input,
                    &mut previous,
                    &mut completed_output,
                )
                .await
                {
                    return;
                }
                if terminal {
                    break;
                }
            }
            if terminal {
                break;
            }
        }
        if !terminal {
            return;
        }
    }
}

async fn send_event(
    socket: &mut WebSocket,
    text: String,
    input: &[Value],
    previous: &mut Option<(String, Vec<Value>)>,
    completed_output: &mut crate::gateway::NativeReplayCapture,
) -> bool {
    if let Ok(event) = serde_json::from_str::<Value>(&text) {
        completed_output.observe(&event);
        if event["type"] == "response.completed"
            && let Some(response) =
                std::mem::take(completed_output).finish(event.get("response").cloned(), None)
            && let (Some(id), Some(output)) =
                (response["id"].as_str(), response["output"].as_array())
        {
            let mut history = input.to_vec();
            history.extend(output.iter().cloned());
            *previous = Some((id.to_owned(), history));
        }
    }
    socket.send(Message::Text(text.into())).await.is_ok()
}
