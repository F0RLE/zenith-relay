use sqlx::PgPool;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use uuid::Uuid;
use zenith_relay_core::{
    error_codes as core,
    gateway::hosted::{ClaimError, HostedExecutionError, HostedUnknown},
};

const QUEUE: usize = 128;
const WRITE_BUDGET: Duration = Duration::from_millis(250);
const LOG_INTERVAL_SECS: u64 = 60;

pub(super) type RequestIds = (Uuid, Uuid, Uuid); // offer, logical request, reserved attempt

#[derive(Clone, Copy)]
pub(super) enum Observation {
    Execution(HostedExecutionError, bool),
    ExecutionDeadline,
    StreamDeadline,
    DeliveryClosed,
    DeliveryBackpressure,
    UpstreamFailed,
    EvidenceUnavailable,
    SettlementDeferred,
}

impl Observation {
    fn fields(self) -> (&'static str, Option<i16>) {
        use HostedExecutionError as E;
        let code = match self {
            Self::Execution(E::Cooldown(_), _) => return (core::UPSTREAM_STATUS, Some(429)),
            Self::Execution(E::UpstreamStatus(status), _) => {
                return (
                    core::UPSTREAM_STATUS,
                    (100..=599).contains(&status).then_some(status as i16),
                );
            }
            Self::Execution(E::Claim(ClaimError::Unconfirmed | ClaimError::AlreadyClaimed), _) => {
                "hosted_dispatch_unconfirmed"
            }
            Self::Execution(E::Claim(_) | E::Unavailable, _) => "hosted_dispatch_unavailable",
            Self::Execution(E::InvalidRequest, _) => core::ADAPTER_INVALID_REQUEST,
            Self::Execution(E::OutcomeUnknown(unknown), streaming) => match unknown {
                HostedUnknown::SendUnconfirmed => core::UPSTREAM_TRANSPORT_REQUEST,
                HostedUnknown::ReadInterrupted => core::UPSTREAM_TRANSPORT_BODY,
                HostedUnknown::ReadTimeout => core::UPSTREAM_TRANSPORT_TIMEOUT,
                HostedUnknown::MissingTerminal => "hosted_terminal_missing",
                HostedUnknown::ResponseLimit => "hosted_response_limit",
                HostedUnknown::InvalidResponse if streaming => {
                    core::ADAPTER_UPSTREAM_STREAM_INVALID
                }
                HostedUnknown::InvalidResponse => core::ADAPTER_UPSTREAM_RESPONSE_INVALID,
            },
            Self::ExecutionDeadline => "hosted_execution_deadline",
            Self::StreamDeadline => "hosted_stream_deadline",
            Self::DeliveryClosed => "hosted_delivery_closed",
            Self::DeliveryBackpressure => "hosted_delivery_backpressure",
            Self::UpstreamFailed => core::UPSTREAM_TERMINAL,
            Self::EvidenceUnavailable => "hosted_evidence_unavailable",
            Self::SettlementDeferred => "hosted_settlement_deferred",
        };
        (code, None)
    }
}

struct Pending {
    ids: RequestIds,
    observation: Observation,
    occurred_ms: Option<i64>,
}

pub(super) struct DropNotice {
    start: Instant,
    after: AtomicU64,
    message: &'static str,
}

impl DropNotice {
    pub fn new(message: &'static str) -> Self {
        Self {
            start: Instant::now(),
            after: AtomicU64::new(0),
            message,
        }
    }

    pub fn emit(&self) {
        let now = self.start.elapsed().as_secs();
        let next = self.after.load(Ordering::Relaxed);
        if now >= next
            && self
                .after
                .compare_exchange(
                    next,
                    now.saturating_add(LOG_INTERVAL_SECS),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
        {
            eprintln!("relay-hosted: {}", self.message);
        }
    }
}

/// One bounded best-effort writer per ingress. It never owns a request,
/// route permit, or accounting transaction.
pub(super) struct DiagnosticSink {
    queue: mpsc::Sender<Pending>,
    drops: Arc<DropNotice>,
}

impl DiagnosticSink {
    pub fn start(pool: PgPool) -> Self {
        let (queue, mut pending) = mpsc::channel::<Pending>(QUEUE);
        let drops = Arc::new(DropNotice::new("optional request diagnostics incomplete"));
        let notice = drops.clone();
        tokio::spawn(async move {
            while let Some(event) = pending.recv().await {
                let (code, status) = event.observation.fields();
                let (offer, rid, aid) = event.ids;
                let write = sqlx::query_scalar::<_, bool>(
                    "SELECT relay_hosted.record_request_diagnostic($1,$2,$3,$4,$5,$6)",
                )
                .bind(offer)
                .bind(rid)
                .bind(aid)
                .bind(code)
                .bind(status)
                .bind(event.occurred_ms)
                .fetch_one(&pool);
                if !matches!(
                    tokio::time::timeout(WRITE_BUDGET, write).await,
                    Ok(Ok(true))
                ) {
                    notice.emit();
                }
            }
        });
        Self { queue, drops }
    }

    pub fn record(&self, ids: RequestIds, observation: Observation) {
        let occurred_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok());
        if self
            .queue
            .try_send(Pending {
                ids,
                observation,
                occurred_ms,
            })
            .is_err()
        {
            self.drops.emit();
        }
    }
}
