use crate::{Error, Result};
use futures_util::StreamExt;
use std::time::Duration;

mod headers;

pub(crate) use headers::{
    is_connection_header, is_credential_header, retry_after_ms, single_header,
};

#[cfg(test)]
mod header_tests;

pub(crate) const MAX_MODEL_CATALOG_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Optional native HTTP deadline from connect through response headers.
pub(crate) fn http_headers_timeout() -> Option<Duration> {
    parse_http_headers_timeout(
        std::env::var("ZENITH_RELAY_HTTP_HEADERS_TIMEOUT_SECONDS")
            .ok()
            .as_deref(),
    )
}

fn parse_http_headers_timeout(value: Option<&str>) -> Option<Duration> {
    value?
        .parse::<u64>()
        .ok()
        .filter(|seconds| (1..=1800).contains(seconds))
        .map(Duration::from_secs)
}

#[cfg(test)]
pub(crate) async fn execute_with_headers_deadline(
    client: &reqwest::Client,
    request: reqwest::Request,
    deadline: tokio::time::Instant,
) -> std::result::Result<reqwest::Response, crate::runtime::AuthorizedRequestError> {
    if deadline <= tokio::time::Instant::now() {
        return Err(crate::runtime::AuthorizedRequestError::HeadersTimeout);
    }
    tokio::time::timeout_at(deadline, client.execute(request))
        .await
        .map_err(|_| crate::runtime::AuthorizedRequestError::HeadersTimeout)?
        .map_err(crate::runtime::AuthorizedRequestError::Transport)
}

/// Defaults to a BPS-only read-progress deadline. Native generation has no
/// total or idle deadline. Zero explicitly disables the BPS guard.
pub(crate) fn basis_points_progress_timeout() -> Option<Duration> {
    let seconds = std::env::var("ZENITH_RELAY_BPS_PROGRESS_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds == 0 || (30..=300).contains(seconds))
        .unwrap_or(120);
    (seconds != 0).then(|| Duration::from_secs(seconds))
}

pub(crate) struct BodyReadFailure {
    pub(crate) bytes: Vec<u8>,
    pub(crate) timed_out: bool,
}

#[derive(Default)]
pub(crate) struct SseProgress {
    line_started: bool,
    comment: bool,
}

impl SseProgress {
    pub(crate) fn observe(&mut self, bytes: &[u8]) -> bool {
        let mut progress = false;
        for &byte in bytes {
            if matches!(byte, b'\r' | b'\n') {
                self.line_started = false;
                self.comment = false;
            } else {
                if !self.line_started {
                    self.comment = byte == b':';
                    self.line_started = true;
                }
                progress |= !self.comment && !byte.is_ascii_whitespace();
            }
        }
        progress
    }
}

/// Count only time waiting for upstream bytes. SSE heartbeats do not reset the
/// budget; fragmented data does. Dropping this future drops the upstream body.
/// The caller owns terminal interpretation and safe retry classification.
pub(crate) async fn collect_with_progress(
    response: reqwest::Response,
    idle_timeout: Option<Duration>,
    stop_at_event: impl FnMut(&[u8]) -> bool,
) -> std::result::Result<Vec<u8>, BodyReadFailure> {
    collect_with_progress_limit(response, idle_timeout, usize::MAX, stop_at_event).await
}

pub(crate) async fn collect_with_progress_limit(
    response: reqwest::Response,
    idle_timeout: Option<Duration>,
    max_bytes: usize,
    mut stop_at_event: impl FnMut(&[u8]) -> bool,
) -> std::result::Result<Vec<u8>, BodyReadFailure> {
    let sse = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        });
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    let mut progress = SseProgress::default();
    let mut remaining = idle_timeout;
    let mut inspected = 0;
    loop {
        let started = tokio::time::Instant::now();
        let chunk = match remaining {
            Some(timeout) => match tokio::time::timeout(timeout, stream.next()).await {
                Ok(chunk) => chunk,
                Err(_) => {
                    return Err(BodyReadFailure {
                        bytes,
                        timed_out: true,
                    });
                }
            },
            None => stream.next().await,
        };
        let Some(chunk) = chunk else {
            return Ok(bytes);
        };
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(_) => {
                return Err(BodyReadFailure {
                    bytes,
                    timed_out: false,
                });
            }
        };
        let count = chunk.len().min(max_bytes.saturating_sub(bytes.len()));
        // A complete terminal within the budget wins over any trailing bytes
        // in the same network chunk. Never allocate or parse that excess tail.
        let retained = &chunk[..count];
        let advanced = if sse {
            progress.observe(retained)
        } else {
            !retained.is_empty()
        };
        remaining = if advanced {
            idle_timeout
        } else {
            remaining.map(|time| time.saturating_sub(started.elapsed()))
        };
        // The old incomplete frame has already been scanned. Revisit only its
        // final byte for a split CRLF instead of rescanning its whole prefix.
        let mut scan_from = bytes.len().saturating_sub(inspected).saturating_sub(1);
        bytes.extend_from_slice(retained);
        if sse {
            while let Some(end) =
                crate::protocol::sse::event_end_from(&bytes[inspected..], scan_from)
            {
                let terminal = stop_at_event(&bytes[inspected..inspected + end]);
                inspected += end;
                scan_from = 0;
                if terminal {
                    bytes.truncate(inspected);
                    return Ok(bytes);
                }
            }
        }
        if count < chunk.len() {
            return Err(BodyReadFailure {
                bytes,
                timed_out: false,
            });
        }
    }
}

pub(crate) async fn collect(response: reqwest::Response) -> Result<Vec<u8>> {
    let mut response_bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        response_bytes.extend_from_slice(&chunk?);
    }
    Ok(response_bytes)
}

pub async fn collect_limited(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(Error::UpstreamBodyTooLarge);
    }
    let mut response_bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if response_bytes.len().saturating_add(chunk.len()) > limit {
            return Err(Error::UpstreamBodyTooLarge);
        }
        response_bytes.extend_from_slice(&chunk);
    }
    Ok(response_bytes)
}

#[cfg(test)]
mod progress_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn native_http_headers_timeout_is_opt_in_and_bounded() {
        for value in [None, Some("0"), Some("-1"), Some("invalid"), Some("1801")] {
            assert_eq!(parse_http_headers_timeout(value), None);
        }
        assert_eq!(
            parse_http_headers_timeout(Some("1")),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            parse_http_headers_timeout(Some("1800")),
            Some(Duration::from_secs(1800))
        );
    }

    async fn headers_request(
        headers_delay: Duration,
        body_delay: Duration,
    ) -> (reqwest::Client, reqwest::Request, AbortServer) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            tokio::time::sleep(headers_delay).await;
            if socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n")
                .await
                .is_err()
            {
                return;
            }
            tokio::time::sleep(body_delay).await;
            let _ = socket.write_all(b"hello").await;
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let request = client.get(format!("http://{address}/")).build().unwrap();
        (client, request, AbortServer(task))
    }

    #[tokio::test]
    async fn http_headers_deadline_stops_silent_headers_but_not_a_long_response_body() {
        let (client, request, _server) =
            headers_request(Duration::ZERO, Duration::from_millis(700)).await;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        let response = execute_with_headers_deadline(&client, request, deadline)
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "hello");
        assert!(tokio::time::Instant::now() > deadline);

        let (client, request, _silent_server) =
            headers_request(Duration::from_secs(5), Duration::ZERO).await;
        let error = execute_with_headers_deadline(
            &client,
            request,
            tokio::time::Instant::now() + Duration::from_millis(50),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            crate::runtime::AuthorizedRequestError::HeadersTimeout
        ));
    }

    #[tokio::test]
    async fn expired_http_headers_deadline_does_not_submit_another_request() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let request = client
            .get(format!("http://{}/", listener.local_addr().unwrap()))
            .build()
            .unwrap();
        let error = execute_with_headers_deadline(&client, request, tokio::time::Instant::now())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            crate::runtime::AuthorizedRequestError::HeadersTimeout
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
    }

    struct AbortServer(tokio::task::JoinHandle<()>);

    impl Drop for AbortServer {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    async fn stream_response(
        chunks: Vec<(Duration, &'static str)>,
    ) -> (reqwest::Response, AbortServer) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n",
                )
                .await
                .unwrap();
            for (delay, chunk) in chunks {
                tokio::time::sleep(delay).await;
                if socket
                    .write_all(format!("{:x}\r\n{chunk}\r\n", chunk.len()).as_bytes())
                    .await
                    .is_err()
                {
                    return;
                }
            }
            std::future::pending::<()>().await;
        });
        let server = AbortServer(task);
        let response = reqwest::get(format!("http://{address}/")).await.unwrap();
        (response, server)
    }

    #[tokio::test]
    async fn heartbeat_only_wait_expires_and_retains_the_partial_response() {
        let initial = "data: {\"type\":\"response.created\",\"response\":{\"usage\":{\"input_tokens\":2}}}\n\n";
        let mut chunks = vec![(Duration::ZERO, initial)];
        chunks.extend((0..30).map(|_| (Duration::from_millis(20), ": heartbeat\n\n")));
        let (response, _server) = stream_response(chunks).await;
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            collect_with_progress(response, Some(Duration::from_millis(150)), |_| false),
        )
        .await
        .expect("heartbeats must not hold the generation open");
        let failure = result.expect_err("missing output must time out");
        assert!(failure.timed_out);
        assert!(failure.bytes.starts_with(initial.as_bytes()));
        assert!(failure.bytes.len() < initial.len() + 30 * ": heartbeat\n\n".len());
    }

    #[tokio::test]
    async fn fragmented_data_extends_the_deadline_until_a_complete_terminal_event() {
        let chunks = vec![
            (Duration::ZERO, "data: {\"type\":\"response."),
            (Duration::from_millis(60), "completed\",\"response\":"),
            (Duration::from_millis(60), "{\"output\":"),
            (Duration::from_millis(60), "[]}}\n\n"),
        ];
        let expected = chunks.iter().map(|(_, bytes)| *bytes).collect::<String>();
        let (response, _server) = stream_response(chunks).await;
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            collect_with_progress(response, Some(Duration::from_millis(150)), |_| true),
        )
        .await
        .unwrap()
        .map_err(|failure| failure.timed_out)
        .expect("fragmented data read failed");
        assert_eq!(result, expected.as_bytes());
    }

    #[test]
    fn heartbeats_and_blank_lines_do_not_count_as_progress_even_when_fragmented() {
        let mut progress = SseProgress::default();
        for chunk in [
            b": hear".as_slice(),
            b"tbeat\r",
            b"\n\r\n",
            b": more",
            b"\n",
        ] {
            assert!(!progress.observe(chunk));
        }
        assert!(progress.observe(b"da"));
        assert!(progress.observe(b"ta: {\"type\":"));
        assert!(progress.observe(b"\"response.created\"}\n\n"));
        assert!(!progress.observe(b":heartbeat\n\n"));
    }

    #[tokio::test]
    async fn bounded_sse_stops_before_an_over_limit_tail() {
        for (prefix, chunk, expected) in [
            ("", "data: done\n\ndata: ignored\n\n", "data: done\n\n"),
            ("data: ", "done\n\ndata: ignored\n\n", "data: done\n\n"),
            ("", "data: done\r\rdata: ignored\r\r", "data: done\r\r"),
            (
                "data: done\r",
                "\n\r\ndata: ignored\r\n\r\n",
                "data: done\r\n\r",
            ),
        ] {
            let mut chunks = Vec::new();
            if !prefix.is_empty() {
                chunks.push((Duration::ZERO, prefix));
            }
            chunks.push((Duration::from_millis(5), chunk));
            let (response, _server) = stream_response(chunks).await;
            let mut events = 0;
            let result = collect_with_progress_limit(
                response,
                Some(Duration::from_secs(1)),
                expected.len(),
                |event| {
                    events += 1;
                    assert_eq!(crate::protocol::sse::event_data(event).as_ref(), b"done");
                    true
                },
            )
            .await
            .map_err(|failure| (failure.timed_out, failure.bytes.len()))
            .expect("a complete terminal inside the budget must win over its trailing bytes");
            assert_eq!(result, expected.as_bytes());
            assert_eq!(events, 1);
        }
    }

    #[tokio::test]
    async fn bounded_sse_keeps_usage_before_an_oversized_event() {
        let complete = "data: {\"usage\":{\"input_tokens\":7}}\n\n";
        let (response, _server) = stream_response(vec![(
            Duration::ZERO,
            "data: {\"usage\":{\"input_tokens\":7}}\n\ndata: unfinished oversized event",
        )])
        .await;
        let limit = complete.len() + 6;
        let mut events = 0;
        let failure =
            collect_with_progress_limit(response, Some(Duration::from_secs(1)), limit, |event| {
                events += 1;
                assert_eq!(event, complete.as_bytes());
                false
            })
            .await
            .expect_err("an oversized response without a terminal must fail");
        assert!(!failure.timed_out);
        assert_eq!(events, 1);
        assert_eq!(failure.bytes.len(), limit);
        assert!(failure.bytes.starts_with(complete.as_bytes()));
    }

    fn synthetic_response(mime: &str, chunks: &[&[u8]]) -> reqwest::Response {
        let chunks = chunks
            .iter()
            .map(|chunk| Ok::<_, std::io::Error>(axum::body::Bytes::copy_from_slice(chunk)))
            .collect::<Vec<_>>();
        axum::http::Response::builder()
            .header(reqwest::header::CONTENT_TYPE, mime)
            .body(reqwest::Body::wrap_stream(futures_util::stream::iter(
                chunks,
            )))
            .unwrap()
            .into()
    }

    #[tokio::test]
    async fn bounded_sse_preserves_every_fragmented_terminal_boundary() {
        for ending in ["\n\n", "\r\r", "\r\n\r\n", "\n\r\n", "\r\n\n"] {
            let prefix = "data: first\n\n";
            let terminal = format!("data: done{ending}");
            let complete = format!("{prefix}{terminal}");
            let wire = format!("{complete}data: must not be parsed\n\n");
            for split in 0..=wire.len() {
                let mut events = Vec::new();
                let response = synthetic_response(
                    "text/event-stream",
                    &[&wire.as_bytes()[..split], &wire.as_bytes()[split..]],
                );
                let bytes = collect_with_progress_limit(response, None, complete.len(), |event| {
                    let data = crate::protocol::sse::event_data(event);
                    events.push(data.to_vec());
                    data.as_ref() == b"done"
                })
                .await
                .map_err(|failure| (failure.timed_out, failure.bytes.len()))
                .expect("terminal completion must not depend on chunk boundaries");
                assert_eq!(events, [b"first".to_vec(), b"done".to_vec()]);
                assert!(
                    bytes == complete.as_bytes() || {
                        complete.ends_with("\r\n")
                            && bytes == complete.as_bytes()[..complete.len() - 1]
                    }
                );
            }
        }
    }

    #[tokio::test]
    async fn bounded_json_keeps_its_byte_limit_without_sse_interpretation() {
        for (body, limit, accepted) in [
            (b"{}".as_slice(), 2, true),
            (b"{} ", 2, false),
            (b"", 0, true),
            (b"{}", 0, false),
            (b"data: done\n\nignored", 12, false),
        ] {
            let response = synthetic_response("application/json", &[body]);
            let mut callback_called = false;
            let result = collect_with_progress_limit(response, None, limit, |_| {
                callback_called = true;
                false
            })
            .await;
            assert!(!callback_called, "JSON body invoked the SSE callback");
            assert_eq!(result.is_ok(), accepted);
            match result {
                Ok(bytes) => assert_eq!(bytes, body),
                Err(failure) => {
                    assert!(!failure.timed_out);
                    assert_eq!(failure.bytes, body[..limit]);
                }
            }
        }
    }
}
