//! Explicit local probe, never a provider request or a CI speed threshold.
use super::*;
use axum::{
    body::Bytes,
    extract::{Request as ServerRequest, State},
    response::{sse::Event, IntoResponse, Sse},
    routing::post,
    Router,
};
use futures_util::{stream, StreamExt};
use std::{convert::Infallible, time::Duration};

const WARMUPS: usize = 2;
const SAMPLES: usize = 9;
const OUTPUT_DELAY: Duration = Duration::from_millis(5);

#[derive(Clone)]
struct MockUpstream {
    expected: Bytes,
    bytes_per_second: Option<f64>,
}

async fn upstream(State(state): State<MockUpstream>, request: ServerRequest) -> impl IntoResponse {
    let compressed = request
        .headers()
        .get(header::CONTENT_ENCODING)
        .is_some_and(|v| v == "zstd");
    let started = tokio::time::Instant::now();
    let mut incoming = request.into_body().into_data_stream();
    let mut encoded = Vec::new();
    while let Some(chunk) = incoming.next().await {
        encoded.extend_from_slice(&chunk.unwrap());
        assert!(encoded.len() <= MAX_COMPRESS_BYTES);
        if let Some(rate) = state.bytes_per_second {
            // Cumulative pacing avoids adding one artificial timer delay per chunk.
            tokio::time::sleep_until(
                started + Duration::from_secs_f64(encoded.len() as f64 / rate),
            )
            .await;
        }
    }
    let upload_us = started.elapsed().as_micros();
    let decode_started = Instant::now();
    let decoded = if compressed {
        tokio::task::spawn_blocking(move || zstd::stream::decode_all(encoded.as_slice()).unwrap())
            .await
            .unwrap()
    } else {
        encoded
    };
    let decode_us = decode_started.elapsed().as_micros();
    assert_eq!(decoded.as_slice(), state.expected.as_ref());
    let events = stream::once(async {
        Ok::<_, Infallible>(Event::default().data(r#"{"type":"response.created"}"#))
    })
    .chain(stream::once(async {
        tokio::time::sleep(OUTPUT_DELAY).await;
        Ok::<_, Infallible>(
            Event::default().data(r#"{"type":"response.output_text.delta","delta":"synthetic"}"#),
        )
    }));
    let mut response = Sse::new(events).into_response();
    response
        .headers_mut()
        .insert("x-probe-upload-us", upload_us.to_string().parse().unwrap());
    response
        .headers_mut()
        .insert("x-probe-decode-us", decode_us.to_string().parse().unwrap());
    response
}

fn fixture(target_bytes: usize, opaque: bool) -> Bytes {
    let mut text = String::with_capacity(target_bytes);
    let mut state = 0x1234_5678_u64;
    while text.len() < target_bytes {
        if !opaque {
            text.push_str(
                "Synthetic tool result: fn example(value: usize) -> usize { value + 1 } // ",
            );
        }
        for _ in 0..32 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            text.push(
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
                    [(state & 63) as usize] as char,
            );
        }
    }
    serde_json::to_vec(&serde_json::json!({"model":"synthetic","stream":true,"input":text}))
        .unwrap()
        .into()
}

#[derive(Default)]
struct Samples {
    encode_us: Vec<u64>,
    upload_us: Vec<u64>,
    decode_us: Vec<u64>,
    headers_us: Vec<u64>,
    first_output_us: Vec<u64>,
    body_bytes: usize,
}

fn percentile(values: &[u64], percentile: usize) -> f64 {
    // Microsecond samples become milliseconds; nanosecond handoffs become microseconds.
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted[(sorted.len() * percentile).div_ceil(100).saturating_sub(1)] as f64 / 1000.0
}

async fn sample(
    client: &reqwest::Client,
    url: &url::Url,
    payload: &Bytes,
    compress: bool,
    samples: Option<&mut Samples>,
) {
    // Encode with the real allowlist; only this test then redirects the unsent
    // synthetic request to loopback. Production endpoint policy is unchanged.
    let request = client
        .post("https://chatgpt.com/backend-api/codex/responses")
        .header(header::CONTENT_TYPE, "application/json")
        .body(payload.clone())
        .build()
        .unwrap();
    let start = Instant::now();
    let (mut request, _) = encode(request, compress, None).await;
    let encode_us = start.elapsed().as_micros() as u64;
    let body_bytes = request.body().unwrap().as_bytes().unwrap().len();
    assert_eq!(
        request.headers().contains_key(header::CONTENT_ENCODING),
        compress
    );
    *request.url_mut() = url.clone();
    let send_started = Instant::now();
    let response = client
        .execute(request)
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let headers_us = send_started.elapsed().as_micros() as u64;
    let upload_us = response.headers()["x-probe-upload-us"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let decode_us = response.headers()["x-probe-decode-us"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let mut output = response.bytes_stream();
    let mut frames = Vec::new();
    let mut first_output_us = None;
    while let Some(chunk) = output.next().await {
        frames.extend_from_slice(&chunk.unwrap());
        if first_output_us.is_none()
            && frames.ends_with(b"\n\n")
            && String::from_utf8_lossy(&frames).contains(r#""delta":"synthetic""#)
        {
            first_output_us = Some(start.elapsed().as_micros() as u64);
        }
    }
    if let Some(samples) = samples {
        samples.encode_us.push(encode_us);
        samples.upload_us.push(upload_us);
        samples.decode_us.push(decode_us);
        samples.headers_us.push(headers_us);
        samples
            .first_output_us
            .push(first_output_us.expect("lifecycle events are not tokens"));
        samples.body_bytes = body_bytes;
    }
}

#[tokio::test]
#[ignore = "local paced HTTP/SSE benchmark; no provider traffic or performance assertions"]
async fn measure_request_compression_latency() {
    let client = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    println!(
        "compression_probe warmups={WARMUPS} samples={SAMPLES} model_delay_ms={} profile={}",
        OUTPUT_DELAY.as_millis(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    for (bytes, opaque) in [
        (64 * 1024, false),
        (512 * 1024, false),
        (2 * 1024 * 1024, false),
        (512 * 1024, true),
    ] {
        let payload = fixture(bytes, opaque);
        for mbps in [0, 10, 100] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url: url::Url = format!("http://{}/responses", listener.local_addr().unwrap())
                .parse()
                .unwrap();
            let app = Router::new()
                .route("/responses", post(upstream))
                .with_state(MockUpstream {
                    expected: payload.clone(),
                    bytes_per_second: (mbps != 0).then_some(mbps as f64 * 1_000_000.0 / 8.0),
                });
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let mut samples = [Samples::default(), Samples::default()];
            for trial in 0..WARMUPS + SAMPLES {
                // Alternate order within each pair; keep one warmed HTTP connection.
                for index in if trial % 2 == 0 { [0, 1] } else { [1, 0] } {
                    sample(
                        &client,
                        &url,
                        &payload,
                        index == 1,
                        (trial >= WARMUPS).then_some(&mut samples[index]),
                    )
                    .await;
                }
            }
            for (index, sample) in samples.iter().enumerate() {
                println!("compression_probe kind={} json_bytes={} encoding={} body_bytes={} link_mbps={} encode_p50_ms={:.3} encode_p95_ms={:.3} upload_p50_ms={:.3} decode_p50_ms={:.3} send_headers_p50_ms={:.3} first_output_p50_ms={:.3} first_output_p95_ms={:.3}",
                    if opaque { "opaque" } else { "text" }, payload.len(), if index == 0 { "identity" } else { "zstd" }, sample.body_bytes, mbps,
                    percentile(&sample.encode_us, 50), percentile(&sample.encode_us, 95), percentile(&sample.upload_us, 50), percentile(&sample.decode_us, 50), percentile(&sample.headers_us, 50), percentile(&sample.first_output_us, 50), percentile(&sample.first_output_us, 95));
            }
            server.abort();
            let _ = server.await;
        }
    }
}

#[test]
#[ignore = "paired local CPU/allocation probe, not provider latency"]
fn measure_request_body_handoff() {
    use std::hint::black_box;
    const ITERATIONS: u32 = 200;
    let client = reqwest::Client::new();
    for size in [64 * 1024, 512 * 1024, 2 * 1024 * 1024, MAX_COMPRESS_BYTES] {
        let request = client
            .post("https://chatgpt.com/backend-api/codex/responses")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-probe", "synthetic")
            .body(vec![b'x'; size])
            .build()
            .unwrap();
        let mut timings = [Vec::new(), Vec::new()];
        for trial in 0..12 {
            for index in if trial % 2 == 0 { [0, 1] } else { [1, 0] } {
                let started = Instant::now();
                for _ in 0..ITERATIONS {
                    if index == 0 {
                        black_box(request.body().unwrap().as_bytes().unwrap().to_vec());
                    } else {
                        black_box(shared_body(&request).unwrap());
                    }
                }
                if trial >= 3 {
                    timings[index]
                        .push(started.elapsed().as_nanos() as u64 / u64::from(ITERATIONS));
                }
            }
        }
        println!(
            "body_handoff bytes={size} copied_p50_us={:.3} shared_p50_us={:.3} removed_payload_copy_bytes={size}",
            percentile(&timings[0], 50),
            percentile(&timings[1], 50)
        );
    }
}
