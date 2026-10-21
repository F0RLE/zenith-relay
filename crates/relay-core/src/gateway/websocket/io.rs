use futures_util::{Sink, SinkExt};
use std::time::Duration;

const WRITE_TIMEOUT: Duration = Duration::from_secs(60);

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
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test(start_paused = true)]
    async fn stalled_write_times_out_without_resending() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let mut socket = Box::pin(futures_util::sink::unfold(
            attempts.clone(),
            |attempts, _: u8| async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                std::future::pending::<Result<Arc<AtomicUsize>, ()>>().await
            },
        ));
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
                move |(), _: u8| async move {
                    if success {
                        Ok(())
                    } else {
                        Err(())
                    }
                },
            ));
            let started = tokio::time::Instant::now();
            assert_eq!(send_message(&mut socket, 1).await.is_ok(), success);
            assert!(started.elapsed().is_zero());
        }
    }
}
