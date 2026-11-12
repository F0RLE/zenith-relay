use super::super::Message;
use futures_util::Sink;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::{mpsc, oneshot, watch};

pub(super) struct Output {
    pub(super) worker: u64,
    pub(super) message: Message,
    pub(super) acknowledged: oneshot::Sender<Result<(), ()>>,
}

pub(super) struct Inputs {
    pub(super) requests: mpsc::Sender<Message>,
    pub(super) controls: mpsc::Sender<Message>,
}

pub(super) enum Activity {
    Ready(u64),
    Prepared,
}

pub(in crate::gateway::websocket) struct LaneSocket {
    worker: u64,
    requests: mpsc::Receiver<Message>,
    controls: mpsc::Receiver<Message>,
    ready: mpsc::Sender<Activity>,
    cancelled: watch::Receiver<bool>,
    active: bool,
    output: Pin<Box<dyn Sink<Message, Error = axum::Error> + Send>>,
}

fn closed() -> axum::Error {
    axum::Error::new(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
}

impl LaneSocket {
    pub(super) fn new(
        worker: u64,
        output: mpsc::Sender<Output>,
        ready: mpsc::Sender<Activity>,
        cancelled: watch::Receiver<bool>,
    ) -> (Self, Inputs) {
        let (requests, requests_rx) = mpsc::channel(1);
        let (controls, controls_rx) = mpsc::channel(8);
        let output = futures_util::sink::unfold(output, move |output, message| async move {
            let (acknowledged, acknowledgment) = oneshot::channel();
            output
                .send(Output {
                    worker,
                    message,
                    acknowledged,
                })
                .await
                .map_err(|_| closed())?;
            acknowledgment
                .await
                .map_err(|_| closed())?
                .map_err(|()| closed())?;
            Ok::<_, axum::Error>(output)
        });
        (
            Self {
                worker,
                requests: requests_rx,
                controls: controls_rx,
                ready,
                cancelled,
                active: false,
                output: Box::pin(output),
            },
            Inputs { requests, controls },
        )
    }

    pub(in crate::gateway::websocket) async fn recv(
        &mut self,
    ) -> Option<Result<Message, axum::Error>> {
        if self.active {
            self.active = false;
            self.ready.send(Activity::Ready(self.worker)).await.ok()?;
        }
        if *self.cancelled.borrow() {
            return None;
        }
        tokio::select! {
            biased;
            _ = self.cancelled.changed() => None,
            message = self.requests.recv() => {
                self.active = message.is_some();
                message.map(Ok)
            }
            message = self.controls.recv() => message.map(Ok),
        }
    }

    pub(in crate::gateway::websocket) async fn note_prepared(&mut self) -> Result<(), ()> {
        self.ready.send(Activity::Prepared).await.map_err(|_| ())
    }

    pub(in crate::gateway::websocket) async fn recv_live(
        &mut self,
    ) -> Option<Result<Message, axum::Error>> {
        if *self.cancelled.borrow() {
            return None;
        }
        tokio::select! {
            biased;
            _ = self.cancelled.changed() => None,
            message = self.controls.recv() => message.map(Ok),
        }
    }
}

impl Sink<Message> for LaneSocket {
    type Error = axum::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.output.as_mut().poll_ready(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, message: Message) -> Result<(), Self::Error> {
        self.output.as_mut().start_send(message)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.output.as_mut().poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.output.as_mut().poll_close(cx)
    }
}
