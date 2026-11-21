use futures_util::{Sink, SinkExt};
use std::time::Duration;

const WRITE_TIMEOUT: Duration = Duration::from_secs(60);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
pub(super) const MAX_OUTBOUND_FRAME_BYTES: usize = 1024 * 1024;

pub(super) async fn send_request_message<S>(socket: &mut S, payload: String) -> Result<(), ()>
where
    S: Sink<tungstenite::Message> + Unpin,
{
    use tungstenite::protocol::frame::{
        Frame,
        coding::{Data, OpCode},
    };
    let write = async {
        if payload.len() <= MAX_OUTBOUND_FRAME_BYTES {
            return socket
                .send(tungstenite::Message::Text(payload.into()))
                .await
                .map_err(|_| ());
        }
        let bytes = reqwest_websocket::Bytes::from(payload.into_bytes());
        for start in (0..bytes.len()).step_by(MAX_OUTBOUND_FRAME_BYTES) {
            let end = start
                .saturating_add(MAX_OUTBOUND_FRAME_BYTES)
                .min(bytes.len());
            let opcode = OpCode::Data(if start == 0 {
                Data::Text
            } else {
                Data::Continue
            });
            socket
                .send(tungstenite::Message::Frame(Frame::message(
                    bytes.slice(start..end),
                    opcode,
                    end == bytes.len(),
                )))
                .await
                .map_err(|_| ())?;
        }
        Ok(())
    };
    tokio::time::timeout(WRITE_TIMEOUT, write)
        .await
        .map_err(|_| ())?
}

pub(super) async fn send_close_message<S, M>(socket: &mut S, message: M) -> Result<(), ()>
where
    S: Sink<M> + Unpin,
{
    match tokio::time::timeout(CLOSE_TIMEOUT, socket.send(message)).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(()),
    }
}

pub(super) async fn close_downstream(socket: &mut super::WebSocket, message: super::Message) {
    if send_close_message(socket, message).await.is_err() || socket.is_http_lane() {
        return;
    }
    // Drain the close handshake before dropping TCP: unread control frames
    // can otherwise turn an already-sent terminal error into a reset.
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, async {
        while let Some(Ok(message)) = socket.recv_live().await {
            if matches!(message, super::Message::Close(_)) {
                break;
            }
        }
    })
    .await;
}

pub(super) async fn send_message<S, M>(socket: &mut S, message: M) -> Result<(), ()>
where
    S: Sink<M> + Unpin,
{
    // A peer that stops reading must not hold a request lease indefinitely.
    match tokio::time::timeout(WRITE_TIMEOUT, socket.send(message)).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(()),
    }
}

#[cfg(test)]
mod frame_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn stalled_socket(attempts: Arc<AtomicUsize>) -> impl Sink<u8, Error = ()> + Unpin {
        Box::pin(futures_util::sink::unfold(
            attempts,
            |attempts, _: u8| async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                std::future::pending::<Result<Arc<AtomicUsize>, ()>>().await
            },
        ))
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_close_does_not_use_the_data_write_timeout_or_resend() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let mut socket = stalled_socket(attempts.clone());
        let started = tokio::time::Instant::now();
        assert_eq!(send_close_message(&mut socket, 1).await, Err(()));
        assert_eq!(started.elapsed(), CLOSE_TIMEOUT);
        assert!(started.elapsed() < WRITE_TIMEOUT);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_write_times_out_without_resending() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let mut socket = stalled_socket(attempts.clone());
        let started = tokio::time::Instant::now();
        assert_eq!(send_message(&mut socket, 1).await, Err(()));
        assert_eq!(started.elapsed(), WRITE_TIMEOUT);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn writes_complete_or_fail_without_waiting_for_the_deadline() {
        for success in [true, false] {
            let mut socket = Box::pin(futures_util::sink::unfold(
                (),
                move |(), _: u8| async move { if success { Ok(()) } else { Err(()) } },
            ));
            let started = tokio::time::Instant::now();
            assert_eq!(send_message(&mut socket, 1).await.is_ok(), success);
            assert!(started.elapsed().is_zero());
        }
    }
}
