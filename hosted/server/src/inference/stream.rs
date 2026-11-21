use super::{
    capture::Milestone,
    diagnostics::{Observation, RequestIds},
    request_header,
    routing::RunningStream,
    store::Store,
};
use axum::{
    body::{Body, Bytes},
    http::{HeaderValue, header},
    response::Response,
};
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::{
    sync::{OwnedSemaphorePermit, mpsc},
    time::{Instant, timeout_at},
};
use tokio_stream::wrappers::ReceiverStream;
use zenith_relay_core::gateway::hosted::{
    HostedExecutionError, HostedStreamObservation, HostedTerminalKind, HostedUnknown,
};

const SEND_BUDGET: Duration = Duration::from_secs(5);
const BODY_BUDGET: Duration = Duration::from_secs(30);

pub(super) fn respond(
    store: Arc<Store>,
    ids: RequestIds,
    running: Box<RunningStream>,
    permit: OwnedSemaphorePermit,
    started: std::time::Instant,
) -> Response {
    // Backpressure is bounded even when the caller keeps the connection but stops reading.
    let (tx, rx) = mpsc::channel(1);
    tokio::spawn(async move {
        let _permit = permit;
        pump(&store, ids, *running, tx, started).await;
    });
    let mut response = Response::new(Body::from_stream(ReceiverStream::new(rx)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    request_header(&mut response, ids.1);
    response
}

type Sender = mpsc::Sender<Result<Bytes, Infallible>>;

async fn pump(
    store: &Store,
    ids: RequestIds,
    mut stream: RunningStream,
    tx: Sender,
    started: std::time::Instant,
) {
    let deadline = Instant::now() + BODY_BUDGET;
    let (offer, rid, aid) = ids;
    let mut first = true;
    let stop = loop {
        // If a terminal is already available, capture it even if the client just left.
        let next = tokio::select! {
            biased;
            result = timeout_at(deadline, stream.upstream.next_frame()) => result,
            _ = tx.closed() => break Observation::DeliveryClosed,
        };
        let frame = match next {
            Ok(Ok(Some(frame))) => frame,
            Ok(Ok(None)) => {
                break Observation::Execution(
                    HostedExecutionError::OutcomeUnknown(HostedUnknown::MissingTerminal),
                    true,
                );
            }
            Ok(Err(error)) => break Observation::Execution(error, true),
            Err(_) => break Observation::StreamDeadline,
        };
        if std::mem::take(&mut first) {
            store.milestone(rid, aid, Milestone::FirstStreamFrame);
        }
        match frame.observation {
            HostedStreamObservation::None => (),
            HostedStreamObservation::Usage(usage) => {
                stream.upstream.mark_read_pause();
                if store.stream_usage(offer, rid, aid, usage).await.is_err() {
                    break Observation::EvidenceUnavailable;
                }
            }
            HostedStreamObservation::Terminal {
                kind,
                usage,
                generation,
                ..
            } => {
                if kind == HostedTerminalKind::Failed {
                    store.diagnostics.record(ids, Observation::UpstreamFailed);
                }
                let applied = store
                    .apply_terminal(ids, kind.as_str(), usage, started, generation)
                    .await;
                let sequence = stream.upstream.next_sequence_number();
                let bytes = match (applied, kind) {
                    (false, _) => error_frame("relay_outcome_pending", sequence),
                    (true, HostedTerminalKind::Failed) => {
                        error_frame("relay_upstream_failed", sequence)
                    }
                    (true, _) => frame.bytes,
                };
                if let Err(stop) = send(&tx, deadline, bytes).await {
                    store.diagnostics.record(ids, stop);
                }
                return;
            }
        }
        // Backpressure can delay reading the next upstream chunk.
        if tx.capacity() == 0 {
            stream.upstream.mark_read_pause();
        }
        if let Err(stop) = send(&tx, deadline, frame.bytes).await {
            break stop;
        }
    };
    // Closing our HTTP body cancels local reads. It does not prove what the supplier did.
    let sequence = stream.upstream.next_sequence_number();
    drop(stream);
    store.milestone(rid, aid, Milestone::RequestInterrupted);
    store.diagnostics.record(ids, stop);
    let _ = store.reconcile(offer, rid, aid, "unknown").await;
    let _ = send(
        &tx,
        deadline,
        error_frame("relay_outcome_pending", sequence),
    )
    .await;
}

async fn send(tx: &Sender, deadline: Instant, bytes: Bytes) -> Result<(), Observation> {
    match timeout_at(
        deadline.min(Instant::now() + SEND_BUDGET),
        tx.send(Ok(bytes)),
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(Observation::DeliveryClosed),
        Err(_) => Err(Observation::DeliveryBackpressure),
    }
}

fn error_frame(code: &'static str, sequence: u64) -> Bytes {
    let data = serde_json::json!({"type":"error","code":code,"message":code,"param":null,"sequence_number":sequence});
    Bytes::from(format!("event: error\ndata: {data}\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_stalled_receiver_cannot_queue_a_second_frame() {
        let (tx, mut rx) = mpsc::channel(1);
        let deadline = Instant::now() + Duration::from_millis(40);
        assert!(
            send(&tx, deadline, Bytes::from_static(b"first"))
                .await
                .is_ok()
        );
        assert!(matches!(
            send(&tx, deadline, Bytes::from_static(b"extra")).await,
            Err(Observation::DeliveryBackpressure)
        ));
        assert_eq!(rx.try_recv().unwrap().unwrap(), "first");
        assert!(rx.try_recv().is_err());
        drop(rx);
        assert!(matches!(
            send(&tx, Instant::now() + SEND_BUDGET, Bytes::new()).await,
            Err(Observation::DeliveryClosed)
        ));
    }
}
