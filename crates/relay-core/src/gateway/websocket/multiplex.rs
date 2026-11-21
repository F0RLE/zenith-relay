use super::*;
use futures_util::{FutureExt, future::BoxFuture, stream::FuturesUnordered};
use std::collections::BTreeMap;
use tokio::sync::{mpsc, watch};

mod queue;
mod socket;
use queue::{Envelope, Lane, Pending, payload};
pub(super) use socket::LaneSocket;
use socket::{Activity, Inputs, Output};

const MAX_ACTIVE: usize = MAX_ACTIVE_RESPONSES;
const MAX_CONTROL_BYTES: usize = 16 * 1024;

struct Worker {
    id: u64,
    inputs: Inputs,
    active: bool,
}

struct Dispatcher {
    lanes: BTreeMap<Lane, Option<Worker>>,
    pending: Pending,
    workers: FuturesUnordered<BoxFuture<'static, (Lane, u64)>>,
    next_worker: u64,
    output: mpsc::Sender<Output>,
    activity: mpsc::Sender<Activity>,
    cancelled: watch::Receiver<bool>,
}

impl Dispatcher {
    fn active(&self) -> usize {
        self.lanes
            .values()
            .flatten()
            .filter(|worker| worker.active)
            .count()
    }

    fn admit(&mut self, message: Message) -> Result<(), (GatewayFailure, Lane)> {
        let envelope = Envelope::parse(&message)
            .map_err(|failure| (failure, ClientRequest::error_stream_id(payload(&message))))?;
        let lane = envelope.lane;
        if !envelope.create {
            let worker = self
                .lanes
                .get(&lane)
                .and_then(Option::as_ref)
                .filter(|worker| worker.active);
            if payload(&message).len() <= MAX_CONTROL_BYTES
                && worker.is_some_and(|worker| worker.inputs.controls.try_send(message).is_ok())
            {
                return Ok(());
            }
            return Err((
                GatewayFailure::invalid_request(
                    "control has no active response or its queue is full",
                ),
                lane,
            ));
        }
        if lane.is_some()
            && !self.lanes.contains_key(&lane)
            && self.lanes.keys().filter(|lane| lane.is_some()).count() >= MAX_NAMED_STREAMS
        {
            return Err((super::connection_responses::stream_limit_failure(), lane));
        }
        self.pending
            .push(lane.clone(), message)
            .map_err(|failure| (failure, lane.clone()))?;
        self.lanes.entry(lane).or_default();
        Ok(())
    }

    fn schedule(
        &mut self,
        runtime: &Arc<GatewayRuntime>,
        key: &AuthenticatedKey,
        headers: &HeaderMap,
    ) -> bool {
        while self.active() < MAX_ACTIVE {
            let Some(queued) = self.pending.take_ready(|lane| {
                self.lanes
                    .get(lane)
                    .and_then(Option::as_ref)
                    .is_none_or(|worker| !worker.active)
            }) else {
                break;
            };
            if self
                .lanes
                .get(&queued.lane)
                .and_then(Option::as_ref)
                .is_none()
            {
                self.next_worker += 1;
                let id = self.next_worker;
                let (socket, inputs) = LaneSocket::new(
                    id,
                    self.output.clone(),
                    self.activity.clone(),
                    self.cancelled.clone(),
                );
                let (runtime, key, headers, lane) = (
                    runtime.clone(),
                    key.clone(),
                    headers.clone(),
                    queued.lane.clone(),
                );
                self.workers.push(
                    async move {
                        handle_connection(WebSocket::from_lane(socket), runtime, key, headers)
                            .await;
                        (lane, id)
                    }
                    .boxed(),
                );
                self.lanes.insert(
                    queued.lane.clone(),
                    Some(Worker {
                        id,
                        inputs,
                        active: false,
                    }),
                );
            }
            let Some(worker) = self.lanes.get_mut(&queued.lane).and_then(Option::as_mut) else {
                return false;
            };
            match worker.inputs.requests.try_send(queued.message) {
                Ok(()) => worker.active = true,
                Err(mpsc::error::TrySendError::Closed(message)) => {
                    // A retired worker may finish between its ready notification
                    // and the dispatcher observing completion. Keep this request.
                    self.pending.restore_front(queue::Queued {
                        lane: queued.lane.clone(),
                        message,
                    });
                    self.lanes.insert(queued.lane, None);
                }
                Err(mpsc::error::TrySendError::Full(_)) => return false,
            }
        }
        true
    }

    fn ready(&mut self, id: u64) {
        if let Some(worker) = self
            .lanes
            .values_mut()
            .flatten()
            .find(|worker| worker.id == id)
        {
            worker.active = false;
        }
    }

    fn finished(&mut self, lane: Lane, id: u64) {
        if let Some(slot) = self.lanes.get_mut(&lane)
            && slot.as_ref().is_some_and(|worker| worker.id == id)
        {
            *slot = None;
        }
    }
}

/// One physical reader/writer; each lane retains the existing HTTP request owner.
pub(super) async fn serve_http(
    mut downstream: WebSocket,
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    headers: HeaderMap,
) {
    let (output, mut outputs) = mpsc::channel(16);
    let (activity, mut activities) = mpsc::channel(64);
    let (cancel, cancelled) = watch::channel(false);
    let mut dispatcher = Dispatcher {
        lanes: BTreeMap::new(),
        pending: Pending::default(),
        workers: FuturesUnordered::new(),
        next_worker: 0,
        output,
        activity,
        cancelled,
    };
    let initial_deadline = TokioInstant::now() + INITIAL_MESSAGE_TIMEOUT;
    let mut prepared = false;
    let mut last_activity = TokioInstant::now();
    let mut heartbeat = interval_at(
        last_activity + WEBSOCKET_HEARTBEAT_INTERVAL,
        WEBSOCKET_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        if !dispatcher.schedule(&runtime, &key, &headers) {
            break;
        }
        tokio::select! {
            _ = sleep_until(initial_deadline), if !prepared => {
                send_gateway_error(&mut downstream, &GatewayFailure::request_timeout(), None, None).await;
                break;
            }
            _ = sleep_until(last_activity + WEBSOCKET_IDLE_TIMEOUT), if dispatcher.active() == 0 => break,
            _ = heartbeat.tick() => {
                if send_message(&mut downstream, Message::Ping(Default::default())).await.is_err() { break; }
            }
            incoming = downstream.recv_live() => {
                last_activity = TokioInstant::now();
                match incoming {
                    Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                        if let Err((failure, lane)) = dispatcher.admit(message)
                            && failure::send_request_error(&mut downstream, &failure, None, lane.as_deref()).await.is_err()
                        { break; }
                    }
                    Some(Ok(Message::Ping(bytes))) => {
                        if send_message(&mut downstream, Message::Pong(bytes)).await.is_err() { break; }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        let _ = io::send_close_message(&mut downstream, Message::Close(frame)).await;
                        break;
                    }
                    _ => break,
                }
            }
            Some(output) = outputs.recv() => {
                if output.acknowledged.is_closed() { continue; }
                let known = dispatcher.lanes.values().flatten().any(|worker| worker.id == output.worker);
                if !known {
                    let _ = output.acknowledged.send(Err(()));
                    continue;
                }
                let result = match output.message {
                    Message::Close(_) | Message::Ping(_) | Message::Pong(_) => Ok(()),
                    message => send_message(&mut downstream, message).await,
                };
                last_activity = TokioInstant::now();
                let _ = output.acknowledged.send(result);
                if result.is_err() { break; }
            }
            Some(activity) = activities.recv() => match activity {
                Activity::Ready(id) => dispatcher.ready(id),
                Activity::Prepared => prepared = true,
            },
            Some((lane, id)) = dispatcher.workers.next(), if !dispatcher.workers.is_empty() => {
                dispatcher.finished(lane, id);
            }
        }
    }
    // Let each request owner record cancellation and release its own lease.
    // No queued request is dispatched after this point.
    let _ = cancel.send(true);
    dispatcher.pending = Pending::default();
    while !dispatcher.workers.is_empty() {
        tokio::select! {
            _ = dispatcher.workers.next() => {}
            Some(output) = outputs.recv() => { let _ = output.acknowledged.send(Err(())); }
            _ = activities.recv() => {}
        }
    }
}
