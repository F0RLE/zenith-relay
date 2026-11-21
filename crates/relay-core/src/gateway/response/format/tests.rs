use super::*;
use futures_util::stream;
use std::time::Duration;

fn response(body: reqwest::Body, content_type: Option<&str>) -> reqwest::Response {
    let mut response = Response::builder()
        .url("https://synthetic.invalid/v1/responses".parse().unwrap())
        .header("x-synthetic-request", "preserved")
        .body(body)
        .unwrap();
    if let Some(content_type) = content_type {
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            axum::http::HeaderValue::from_str(content_type).unwrap(),
        );
    }
    response.extensions_mut().insert(42_u64);
    response.into()
}

#[test]
fn prefix_detection_is_incremental_and_distinguishes_json() {
    for prefix in [
        b"".as_slice(),
        b"\xef",
        b"\xef\xbb",
        b"\xef\xbb\xbf\r\n",
        b"d",
        b"dat",
        b"event",
    ] {
        assert_eq!(event_stream_prefix(prefix), None, "{prefix:?}");
    }
    for prefix in [
        b"data:".as_slice(),
        b"event:",
        b":",
        b"id:",
        b"retry:",
        b"\xef\xbb\xbf\r\ndata:",
    ] {
        assert_eq!(event_stream_prefix(prefix), Some(true), "{prefix:?}");
    }
    for prefix in [b"{".as_slice(), b" [", b"<html>", b"detail:", b"\xef\xbbx"] {
        assert_eq!(event_stream_prefix(prefix), Some(false), "{prefix:?}");
    }
}

#[tokio::test]
async fn missing_or_mislabeled_sse_preserves_every_chunk_and_metadata() {
    let body = "\u{feff}\r\nevent: response.output_text.delta\r\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\r\n\r\n";
    for content_type in [None, Some("application/json"), Some("text/plain")] {
        let chunks: Vec<_> = body
            .as_bytes()
            .iter()
            .map(|byte| Ok::<_, std::io::Error>(Bytes::copy_from_slice(&[*byte])))
            .collect();
        let upstream = response(
            reqwest::Body::wrap_stream(stream::iter(chunks)),
            content_type,
        );
        let normalized = normalize_event_stream(upstream, Instant::now(), None, None).await;
        assert_eq!(
            normalized.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        assert_eq!(normalized.headers()["x-synthetic-request"], "preserved");
        assert_eq!(
            normalized.url().as_str(),
            "https://synthetic.invalid/v1/responses"
        );
        assert_eq!(normalized.extensions().get::<u64>(), Some(&42));
        assert_eq!(normalized.bytes().await.unwrap().as_ref(), body.as_bytes());
    }
}

#[tokio::test]
async fn recognition_does_not_wait_for_json_payload_or_upstream_completion() {
    let chunks = stream::iter(
        b"data:"
            .iter()
            .map(|byte| Ok::<_, std::io::Error>(Bytes::copy_from_slice(&[*byte])))
            .collect::<Vec<_>>(),
    );
    let body = reqwest::Body::wrap_stream(chunks.chain(stream::pending()));
    let mut normalized = tokio::time::timeout(
        Duration::from_secs(1),
        normalize_event_stream(response(body, None), Instant::now(), None, None),
    )
    .await
    .expect("SSE detection must stop at the field prefix");
    let mut restored = Vec::new();
    while restored.len() < 5 {
        restored.extend_from_slice(
            &tokio::time::timeout(Duration::from_secs(1), normalized.chunk())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        );
    }
    assert_eq!(restored, b"data:");
}

#[tokio::test]
async fn json_and_html_bodies_remain_unchanged() {
    for (body, mime) in [
        ("{\"error\":\"synthetic\"}", "application/json"),
        ("<html>synthetic</html>", "text/html"),
    ] {
        let upstream = response(reqwest::Body::from(body), Some(mime));
        let normalized = normalize_event_stream(upstream, Instant::now(), None, None).await;
        assert_eq!(normalized.headers()[header::CONTENT_TYPE], mime);
        assert_eq!(normalized.text().await.unwrap(), body);
    }
}

#[tokio::test]
async fn prefix_storage_is_bounded_without_dropping_a_large_chunk() {
    let chunk = Bytes::from(vec![b' '; MAX_PREFIX_BYTES * 2]);
    let body = reqwest::Body::wrap_stream(
        stream::iter([Ok::<_, std::io::Error>(chunk.clone())]).chain(stream::pending()),
    );
    let mut normalized = tokio::time::timeout(
        Duration::from_secs(1),
        normalize_event_stream(response(body, None), Instant::now(), None, None),
    )
    .await
    .expect("whitespace cannot grow the detection prefix indefinitely");
    assert!(normalized.headers().get(header::CONTENT_TYPE).is_none());
    assert_eq!(normalized.chunk().await.unwrap().unwrap(), chunk);
}

#[tokio::test]
async fn declared_sse_and_encoded_or_failed_responses_are_not_probed() {
    for (mime, encoded, status) in [
        (Some("Text/Event-Stream; charset=utf-8"), false, 200),
        (None, true, 200),
        (None, false, 403),
    ] {
        let body = reqwest::Body::wrap_stream(stream::pending::<Result<Bytes, std::io::Error>>());
        let upstream = response(body, mime);
        let mut raw: Response<reqwest::Body> = upstream.into();
        *raw.status_mut() = axum::http::StatusCode::from_u16(status).unwrap();
        let mut upstream = reqwest::Response::from(raw);
        if encoded {
            upstream.headers_mut().insert(
                header::CONTENT_ENCODING,
                axum::http::HeaderValue::from_static("gzip"),
            );
        }
        let normalized = tokio::time::timeout(
            Duration::from_secs(1),
            normalize_event_stream(upstream, Instant::now(), None, None),
        )
        .await
        .expect("known response formats must not wait for a body");
        assert_eq!(normalized.status().as_u16(), status);
    }
}

#[tokio::test]
async fn an_error_during_detection_remains_a_body_error() {
    let body = reqwest::Body::wrap_stream(stream::iter([
        Ok(Bytes::from_static(b"da")),
        Err(std::io::Error::other("synthetic interrupted body")),
    ]));
    let normalized = normalize_event_stream(response(body, None), Instant::now(), None, None).await;
    let error = normalized.bytes().await.unwrap_err();
    assert!(error.is_body() || error.is_decode());
}

#[tokio::test(start_paused = true)]
async fn bps_progress_guard_still_bounds_detection_of_a_silent_body() {
    let body = reqwest::Body::wrap_stream(stream::pending::<Result<Bytes, std::io::Error>>());
    let normalized = normalize_event_stream(
        response(body, None),
        Instant::now(),
        None,
        Some(Duration::from_secs(1)),
    )
    .await;
    assert_eq!(
        normalized.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    let text = normalized.text().await.unwrap();
    assert!(text.contains(crate::error_codes::STREAM_IDLE_TIMEOUT));
}

#[tokio::test]
async fn only_recognized_sse_adds_real_body_observations() {
    for (body, sse) in [("data: {}\n\n", true), ("{\"synthetic\":true}", false)] {
        let mut transport: crate::usage::TransportDiagnostics =
            serde_json::from_value(serde_json::json!({"upstream":"http","mode":"native"})).unwrap();
        let normalized = normalize_event_stream(
            response(reqwest::Body::from(body), None),
            Instant::now(),
            Some(&mut transport),
            None,
        )
        .await;
        assert_eq!(normalized.bytes().await.unwrap().as_ref(), body.as_bytes());
        assert_eq!(transport.stream_response.is_some(), sse);
        if let Some(timing) = transport.stream_response {
            assert!(timing.decoded_body_first_ms.is_some());
            assert_eq!(timing.sse_json_first_ms, None);
            assert_eq!(timing.upstream_reasoning_first_ms, None);
        }
    }
}

#[tokio::test]
async fn cancelling_detection_drops_the_owned_body() {
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
    let (observed, first_read) = tokio::sync::oneshot::channel();
    let stream = stream::once(async move {
        let _ = observed.send(());
        std::future::pending::<Result<Bytes, std::io::Error>>().await
    })
    .chain(stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|chunk| (chunk, receiver))
    }));
    let body = reqwest::Body::wrap_stream(stream);
    let task = tokio::spawn(normalize_event_stream(
        response(body, None),
        Instant::now(),
        None,
        None,
    ));
    first_read.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        sender.is_closed(),
        "no detached reader may retain the upstream body"
    );
}
