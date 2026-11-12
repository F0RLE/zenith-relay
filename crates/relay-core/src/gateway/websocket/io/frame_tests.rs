use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn request_frames_preserve_boundaries_bytes_and_final_flag() {
    use tungstenite::{
        Message,
        protocol::frame::coding::{Data, OpCode},
    };
    for length in [
        0,
        MAX_OUTBOUND_FRAME_BYTES,
        MAX_OUTBOUND_FRAME_BYTES + 1,
        MAX_OUTBOUND_FRAME_BYTES * 2,
    ] {
        let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut socket = Box::pin(futures_util::sink::unfold(
            captured.clone(),
            |captured, message: Message| async move {
                captured.lock().unwrap().push(message);
                Ok::<_, ()>(captured)
            },
        ));
        let payload = "a".repeat(length);
        send_request_message(&mut socket, payload.clone())
            .await
            .unwrap();
        let messages = captured.lock().unwrap();
        if length <= MAX_OUTBOUND_FRAME_BYTES {
            assert_eq!(*messages, vec![Message::Text(payload.into())]);
            continue;
        }
        let mut reconstructed = Vec::new();
        assert_eq!(messages.len(), length.div_ceil(MAX_OUTBOUND_FRAME_BYTES));
        for (index, message) in messages.iter().enumerate() {
            let Message::Frame(frame) = message else {
                panic!("expected a continuation frame")
            };
            assert!(frame.payload().len() <= MAX_OUTBOUND_FRAME_BYTES);
            assert_eq!(
                frame.header().opcode,
                OpCode::Data(if index == 0 {
                    Data::Text
                } else {
                    Data::Continue
                })
            );
            assert_eq!(frame.header().is_final, index + 1 == messages.len());
            reconstructed.extend_from_slice(frame.payload());
        }
        assert_eq!(reconstructed, payload.as_bytes());
    }
}

#[tokio::test(start_paused = true)]
async fn fragmented_write_has_one_deadline_and_never_resends_after_partial_failure() {
    for stall in [false, true] {
        let attempts = Arc::new(AtomicUsize::new(0));
        let mut socket = Box::pin(futures_util::sink::unfold(
            attempts.clone(),
            move |attempts, _: tungstenite::Message| async move {
                if attempts.fetch_add(1, Ordering::SeqCst) > 0 {
                    if stall {
                        std::future::pending::<()>().await;
                    }
                    return Err(());
                }
                tokio::time::sleep(Duration::from_secs(40)).await;
                Ok(attempts)
            },
        ));
        let started = tokio::time::Instant::now();
        assert_eq!(
            send_request_message(&mut socket, "a".repeat(MAX_OUTBOUND_FRAME_BYTES * 3)).await,
            Err(())
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(if stall { 60 } else { 40 })
        );
    }
}
