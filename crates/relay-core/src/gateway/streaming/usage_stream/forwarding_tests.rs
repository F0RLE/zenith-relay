use super::*;
use crate::gateway::test_support::test_usage_event;
use futures_util::stream;
use std::{convert::Infallible, hint::black_box};

fn forwarder() -> UsageStream<stream::Empty<Result<Bytes, Infallible>>> {
    UsageStream::new(
        stream::empty(),
        Arc::new(|_| {}),
        test_usage_event(),
        Instant::now(),
        Arc::new(|_, _, _| {}),
    )
}

fn frame(event: Value, ending: &str) -> Vec<u8> {
    format!(
        "event: {}{ending}data: {event}{ending}{ending}",
        event["type"].as_str().unwrap()
    )
    .into_bytes()
}

fn delta(ending: &str) -> Vec<u8> {
    frame(
        json!({"type":"response.output_text.delta","delta":"synthetic ".repeat(24)}),
        ending,
    )
}

fn terminal(ending: &str, text_bytes: usize) -> Vec<u8> {
    frame(
        json!({"type":"response.completed","response":{"id":"resp_synthetic",
            "status":"completed","output":[{"type":"message","role":"assistant",
            "content":[{"type":"output_text","text":"x".repeat(text_bytes)}]}]}}),
        ending,
    )
}

#[tokio::test]
async fn coalesced_and_fragmented_forwarding_preserve_bytes() {
    for ending in ["\n", "\r\n", "\r"] {
        let mut wire = delta(ending).repeat(32);
        wire.extend(terminal(ending, 4096));
        for chunk_size in [1, 7, 1024, wire.len()] {
            let mut stream = forwarder();
            let mut output = Vec::new();
            for chunk in wire.chunks(chunk_size) {
                output.extend(stream.forward_sse_bytes(chunk, crate::ErrorOrigin::Account));
            }
            assert_eq!(output, wire, "ending={ending:?}, chunk={chunk_size}");
            assert!(stream.sse_forward_pending.is_empty());
            if chunk_size == wire.len() {
                assert_eq!(stream.sse_forward_pending.capacity(), 0);
            }
        }
    }
}

#[tokio::test]
async fn incomplete_terminal_stays_buffered_while_text_is_immediate() {
    for ending in ["\n", "\r\n", "\r"] {
        let completed = terminal(ending, 64 * 1024);
        let prefix = &completed[..completed.len() - ending.len() * 2];
        let mut stream = forwarder();
        let text = delta(ending);
        assert_eq!(
            stream.forward_sse_bytes(&text, crate::ErrorOrigin::Account),
            text
        );
        for chunk in prefix.chunks(1024) {
            assert!(
                stream
                    .forward_sse_bytes(chunk, crate::ErrorOrigin::Account)
                    .is_empty()
            );
        }
        assert_eq!(
            stream.forward_sse_bytes(&completed[prefix.len()..], crate::ErrorOrigin::Account),
            completed
        );
        assert!(stream.sse_forward_pending.is_empty());
    }
}

#[tokio::test]
async fn terminal_stops_a_coalesced_chunk_and_preserves_failure_origin() {
    for event in [
        json!({"type":"response.completed","response":{"status":"completed","output":[]}}),
        json!({"type":"error","code":"server_error","message":"Synthetic failure"}),
        json!({"type":"response.incomplete","response":{"status":"incomplete",
            "incomplete_details":{"reason":"max_output_tokens"},"output":[]}}),
    ] {
        let text = delta("\n").repeat(128);
        let end = frame(event.clone(), "\n");
        let expected_end = forwarder().forward_sse_bytes(&end, crate::ErrorOrigin::Account);
        let mut wire = text.clone();
        wire.extend(&end);
        wire.extend_from_slice(b"event: unexpected\ndata: must_not_escape\n\n");
        let mut stream = forwarder();
        let output = stream.forward_sse_bytes(&wire, crate::ErrorOrigin::Account);
        assert_eq!(output, [text, expected_end].concat());
        assert!(stream.sse_forward_pending.is_empty());
        if event["type"] == "error" {
            let parsed = parse_sse_event(&output[delta("\n").len() * 128..]);
            assert!(
                parsed.event_payload.unwrap()["message"]
                    .as_str()
                    .unwrap()
                    .contains("Account")
            );
        }
    }
}

#[tokio::test]
#[ignore = "manual synthetic SSE forwarding benchmark; no provider requests"]
async fn measure_sse_forwarding_latency() {
    let coalesced = [delta("\n").repeat(4096), terminal("\n", 0)].concat();
    let fragmented = terminal("\r\n", 1024 * 1024);
    for (label, wire, chunk_size) in [
        ("coalesced", &coalesced, coalesced.len()),
        ("network_chunks", &coalesced, 16 * 1024),
        ("fragmented_terminal", &fragmented, 1024),
    ] {
        let mut samples = Vec::new();
        for trial in 0..9 {
            let mut stream = forwarder();
            let mut output = Vec::with_capacity(wire.len());
            let started = Instant::now();
            for chunk in wire.chunks(chunk_size) {
                output.extend(
                    stream.forward_sse_bytes(black_box(chunk), crate::ErrorOrigin::Account),
                );
            }
            let elapsed = started.elapsed();
            assert_eq!(&output, wire);
            assert!(stream.sse_forward_pending.is_empty());
            if trial >= 2 {
                samples.push(elapsed);
            }
        }
        samples.sort_unstable();
        eprintln!(
            "SSE forwarding {label}: bytes={}, chunk={chunk_size}, median={:?}, min={:?}, max={:?}",
            wire.len(),
            samples[3],
            samples[0],
            samples[6]
        );
    }
}
