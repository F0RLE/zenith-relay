use super::*;

fn upstream() -> (
    tokio::sync::mpsc::UnboundedSender<Result<Bytes, reqwest::Error>>,
    UpstreamStream,
) {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let stream = stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|chunk| (chunk, receiver))
    });
    (sender, Box::pin(stream))
}

#[tokio::test]
async fn terminal_releases_reader_before_the_client_drains_queued_output() {
    for terminal in [
        completed(json!([call("inspect")])),
        json!({"type":"response.incomplete","response":{"status":"incomplete","output":[]}}),
        json!({"type":"response.failed","response":{"status":"failed","output":[],"error":{"code":"server_error"}}}),
        json!({"type":"response.cancelled","response":{"status":"cancelled","output":[]}}),
        json!({"type":"error","code":"server_error","message":"Synthetic failure"}),
        json!({"type":"response.completed","response":{"status":"in_progress","output":[]}}),
    ] {
        for in_first in [false, true] {
            let (sender, inner) = upstream();
            let first = if in_first {
                frame(terminal.clone())
            } else {
                sender.send(Ok(frame(terminal.clone()))).unwrap();
                Bytes::new()
            };
            let mut body = translate_stream(first, inner, request().into());
            let first_output = body.next().await.unwrap().unwrap();
            assert!(
                sender.is_closed(),
                "terminal {} must release its reader before downstream drain, in_first={in_first}",
                terminal["type"]
            );
            let mut wire = first_output.to_vec();
            while let Some(chunk) = body.next().await {
                wire.extend_from_slice(&chunk.unwrap());
            }
            let wire = String::from_utf8(wire).unwrap();
            assert_eq!(
                wire.matches("event: response.completed\n").count(),
                usize::from(terminal["response"]["status"] == "completed")
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn idle_timeout_releases_reader_before_returning_its_error() {
    let (sender, inner) = upstream();
    let mut body = guard_progress(inner, Some(Duration::from_secs(3)));
    let started = tokio::time::Instant::now();
    let error = body.next().await.unwrap().unwrap();
    assert_eq!(started.elapsed(), Duration::from_secs(3));
    assert!(String::from_utf8_lossy(&error).contains(crate::error_codes::STREAM_IDLE_TIMEOUT));
    assert!(
        sender.is_closed(),
        "timeout must close the stalled reader immediately"
    );
    assert!(body.next().await.is_none());
}

#[tokio::test]
async fn transport_error_releases_reader_without_another_poll() {
    for translate in [false, true] {
        let (sender, inner) = upstream();
        let response: reqwest::Response = axum::http::Response::builder()
            .status(502)
            .body("")
            .unwrap()
            .into();
        sender
            .send(Err(response.error_for_status().unwrap_err()))
            .unwrap();
        let mut body = if translate {
            translate_stream(Bytes::new(), inner, request().into())
        } else {
            guard_progress(inner, None)
        };
        assert!(body.next().await.unwrap().is_err());
        assert!(
            sender.is_closed(),
            "error must release the reader, translate={translate}"
        );
        assert!(body.next().await.is_none());
    }
}

#[tokio::test]
async fn nonterminal_text_keeps_reader_until_client_cancellation() {
    let (sender, inner) = upstream();
    sender
        .send(Ok(frame(
            json!({"type":"response.output_text.delta","output_index":0,"delta":"Working"}),
        )))
        .unwrap();
    let mut body = translate_stream(Bytes::new(), inner, request().into());
    assert!(body.next().await.unwrap().is_ok());
    assert!(!sender.is_closed());
    drop(body);
    assert!(sender.is_closed());
}
