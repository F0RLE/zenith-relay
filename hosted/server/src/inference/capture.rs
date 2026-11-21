use super::diagnostics::DropNotice;
use crate::config::CaptureLimits;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::mpsc, time};
use uuid::Uuid;

const POLL: Duration = Duration::from_millis(500);
const BUDGET: Duration = Duration::from_millis(200);

#[derive(Clone, Copy)]
pub(super) enum Milestone {
    DispatchCommitted,
    UpstreamResponse,
    FirstStreamFrame,
    TerminalObserved,
    EvidenceCommitted,
    AccountingSettled,
    AccountingPending,
    RequestInterrupted,
}

impl Milestone {
    fn code(self) -> &'static str {
        match self {
            Self::DispatchCommitted => "dispatch_committed",
            Self::UpstreamResponse => "upstream_response",
            Self::FirstStreamFrame => "first_stream_frame",
            Self::TerminalObserved => "terminal_observed",
            Self::EvidenceCommitted => "evidence_committed",
            Self::AccountingSettled => "accounting_settled",
            Self::AccountingPending => "accounting_pending",
            Self::RequestInterrupted => "request_interrupted",
        }
    }
}

#[derive(Clone, Copy)]
struct Target {
    session: Uuid,
    request: Uuid,
}

struct Pending {
    target: Target,
    attempt: Uuid,
    milestone: Milestone,
    at: i64,
}

pub(super) struct Capture {
    target: Arc<RwLock<Option<Target>>>,
    queue: mpsc::Sender<Pending>,
    drops: Arc<DropNotice>,
}

impl Capture {
    pub fn start(pool: &PgPool, origin: String, limits: CaptureLimits) -> Self {
        // The optional collector cannot consume a connection from the dispatch/settlement pool.
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(BUDGET)
            .connect_lazy_with(pool.connect_options().as_ref().clone());
        let target = Arc::new(RwLock::new(None));
        let drops = Arc::new(DropNotice::new("temporary diagnostic capture incomplete"));
        let (queue, receiver) = mpsc::channel(32);
        tokio::spawn(collect(
            pool,
            origin,
            limits,
            target.clone(),
            receiver,
            drops.clone(),
        ));
        Self {
            target,
            queue,
            drops,
        }
    }

    pub fn record(&self, request: Uuid, attempt: Uuid, milestone: Milestone) {
        let Some(target) = self.target.try_read().ok().and_then(|value| *value) else {
            return;
        };
        if target.request != request {
            return;
        }
        let Some(at) = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|v| i64::try_from(v.as_millis()).ok())
        else {
            self.drops.emit();
            return;
        };
        if self
            .queue
            .try_send(Pending {
                target,
                attempt,
                milestone,
                at,
            })
            .is_err()
        {
            self.drops.emit();
        }
    }
}

async fn collect(
    pool: PgPool,
    origin: String,
    limits: CaptureLimits,
    active: Arc<RwLock<Option<Target>>>,
    mut queue: mpsc::Receiver<Pending>,
    drops: Arc<DropNotice>,
) {
    let mut poll = time::interval(POLL);
    poll.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = poll.tick() => {
                let query = sqlx::query_as::<_, (Uuid, Uuid)>(
                    "SELECT * FROM relay_hosted.capture_target($1,$2,$3,$4,$5)",
                )
                .bind(&origin)
                .bind(limits.max_seconds)
                .bind(limits.max_events)
                .bind(limits.max_bytes)
                .bind(limits.max_events_per_second)
                .fetch_optional(&pool);
                let next = match time::timeout(BUDGET, query).await {
                    Ok(Ok(row)) => row.map(|(session, request)| Target { session, request }),
                    _ => {
                        drops.emit();
                        None
                    }
                };
                if let Ok(mut target) = active.write() {
                    *target = next;
                }
            }
            event = queue.recv() => {
                let Some(event) = event else { break };
                let write = sqlx::query_scalar::<_, bool>(
                    "SELECT relay_hosted.append_capture_milestone($1,$2,$3,$4::text::relay_hosted.capture_milestone,$5)",
                )
                .bind(event.target.session)
                .bind(event.target.request)
                .bind(event.attempt)
                .bind(event.milestone.code())
                .bind(event.at)
                .fetch_one(&pool);
                if !matches!(time::timeout(BUDGET, write).await, Ok(Ok(true))) {
                    drops.emit();
                }
            }
        }
    }
}
