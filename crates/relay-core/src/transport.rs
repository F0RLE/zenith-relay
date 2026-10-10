use crate::{Error, Result};
use futures_util::StreamExt;
use std::time::Duration;

pub(crate) const MAX_MODEL_CATALOG_BODY_BYTES: usize = 4 * 1024 * 1024;

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
struct SseProgress {
    line_started: bool,
    comment: bool,
}

impl SseProgress {
    fn observe(&mut self, bytes: &[u8]) -> bool {
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
                    })
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
                })
            }
        };
        let advanced = if sse {
            progress.observe(&chunk)
        } else {
            !chunk.is_empty()
        };
        remaining = if advanced {
            idle_timeout
        } else {
            remaining.map(|time| time.saturating_sub(started.elapsed()))
        };
        bytes.extend_from_slice(&chunk);
        if sse {
            while let Some(end) = crate::protocol::sse::event_end(&bytes[inspected..]) {
                let terminal = stop_at_event(&bytes[inspected..inspected + end]);
                inspected += end;
                if terminal {
                    bytes.truncate(inspected);
                    return Ok(bytes);
                }
            }
        }
    }
}

/// A relative floor measured at receipt. Consumers must preserve the floor
/// when converting it to a monotonic deadline; do not cap a long provider hint
/// to a shorter polling interval.
pub(crate) fn retry_after_ms(
    headers: &reqwest::header::HeaderMap,
    now: std::time::SystemTime,
) -> Option<u64> {
    let retry_after_header = headers.get("retry-after")?.to_str().ok()?.trim();
    if let Ok(seconds) = retry_after_header.parse::<u64>() {
        return Some(seconds.saturating_mul(1_000));
    }
    Some(
        httpdate::parse_http_date(retry_after_header)
            .ok()?
            .duration_since(now)
            .ok()?
            .as_millis()
            .min(u128::from(u64::MAX)) as u64,
    )
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
}
