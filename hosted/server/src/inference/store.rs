use super::{
    capture::{Capture, Milestone},
    diagnostics::{DiagnosticSink, Observation, RequestIds},
};
use crate::config::CaptureLimits;
use ring::hmac;
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;
use std::{future::Future, net::Ipv4Addr, pin::Pin, time::Instant};
use uuid::Uuid;
use zenith_relay_core::{
    gateway::hosted::{ClaimError, HostedAttempt, HostedDispatchAuthority, HostedGeneration},
    scheduler::rotation::RotationMode,
};

#[derive(Deserialize)]
pub(super) struct Route {
    pub item: Uuid,
    pub source: Uuid,
    pub model: String,
    pub endpoint: String,
    pub direct_ipv4: Option<Ipv4Addr>,
    pub proxy_revision: i64,
    pub uses_proxy: bool,
    pub provider_revision: i64,
    pub input_bytes: usize,
    pub rate_revision: i64,
    pub cost: u64,
    pub busy: bool,
    pub priority: i32,
}

#[derive(Deserialize)]
pub(super) struct Candidates {
    pub scope: Uuid,
    pub mode: RotationMode,
    pub items: Vec<Route>,
}

pub(super) struct Claim<'a> {
    pub store: &'a Store,
    pub route: &'a Route,
}

pub(super) struct Store {
    pub pool: PgPool,
    pub origin: String,
    pub sealing: hmac::Key,
    pub diagnostics: DiagnosticSink,
    capture: Option<Capture>,
}

pub(super) type Reserved = (
    String,
    Option<Uuid>,
    Option<Uuid>,
    Option<Uuid>,
    Option<i64>,
    Option<i64>,
    Option<Uuid>,
    Option<i64>,
);
impl Store {
    pub fn new(
        pool: PgPool,
        origin: String,
        sealing: hmac::Key,
        limits: Option<CaptureLimits>,
    ) -> Self {
        let diagnostics = DiagnosticSink::start(pool.clone());
        let capture = limits.map(|limits| Capture::start(&pool, origin.clone(), limits));
        Self {
            pool,
            origin,
            sealing,
            diagnostics,
            capture,
        }
    }

    pub fn milestone(&self, request: Uuid, attempt: Uuid, milestone: Milestone) {
        if let Some(capture) = &self.capture {
            capture.record(request, attempt, milestone);
        }
    }

    pub async fn candidates(
        &self,
        rid: Uuid,
        aid: Uuid,
        kid: Uuid,
        seal: &[u8],
    ) -> Result<Candidates, &'static str> {
        let v: Option<Value> =
            sqlx::query_scalar("SELECT relay_hosted.request_text_candidates($1,$2,$3,$4,$5)")
                .bind(&self.origin)
                .bind(rid)
                .bind(aid)
                .bind(kid)
                .bind(seal)
                .fetch_one(&self.pool)
                .await
                .map_err(|_| "qualification database unavailable")?;
        serde_json::from_value(v.ok_or("reservation no longer dispatchable")?)
            .map_err(|_| "invalid supplier projection")
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn reserve(
        &self,
        key: Uuid,
        proof: &[u8],
        op: Option<&[u8]>,
        stable: &[u8],
        seal: &[u8],
        model: &str,
        input_len: i32,
        max_out: i32,
        stream: bool,
    ) -> Result<Reserved, sqlx::Error> {
        sqlx::query_as(
            "SELECT * FROM relay_hosted.reserve_published_text($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(&self.origin)
        .bind(key)
        .bind(proof)
        .bind(op)
        .bind(stable)
        .bind(seal)
        .bind(model)
        .bind(input_len)
        .bind(max_out)
        .bind(stream)
        .fetch_one(&self.pool)
        .await
    }

    async fn capture(
        &self,
        (offer, rid, aid): RequestIds,
        state: &str,
        counts: [Option<i64>; 3],
        elapsed_ms: Option<i32>,
        generation: Option<HostedGeneration>,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT relay_hosted.capture_text_terminal($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(offer)
        .bind(rid)
        .bind(aid)
        .bind(state)
        .bind(counts[0])
        .bind(counts[1])
        .bind(counts[2])
        .bind(elapsed_ms)
        .bind(generation.map(|v| v.duration_ms))
        .bind(generation.map(|v| v.output_tokens))
        .fetch_one(&self.pool)
        .await
    }

    pub async fn apply_terminal(
        &self,
        ids: RequestIds,
        state: &str,
        counts: [Option<i64>; 3],
        started: Instant,
        generation: Option<HostedGeneration>,
    ) -> bool {
        let (offer, rid, aid) = ids;
        let elapsed_ms = if state == "completed" {
            i32::try_from(started.elapsed().as_millis()).ok()
        } else {
            None
        };
        self.milestone(rid, aid, Milestone::TerminalObserved);
        // Mandatory recovery evidence commits before accounting, independently of optional diagnostics.
        if !matches!(
            self.capture(ids, state, counts, elapsed_ms, generation)
                .await,
            Ok(true)
        ) {
            self.diagnostics
                .record(ids, Observation::EvidenceUnavailable);
            return false;
        }
        self.milestone(rid, aid, Milestone::EvidenceCommitted);
        match self.reconcile(offer, rid, aid, "deadline").await.as_deref() {
            Ok("settled") => self.milestone(rid, aid, Milestone::AccountingSettled),
            Ok("pending") => self.milestone(rid, aid, Milestone::AccountingPending),
            _ => {
                self.diagnostics
                    .record(ids, Observation::SettlementDeferred);
                return false;
            }
        }
        true
    }

    pub async fn stream_usage(
        &self,
        offer: Uuid,
        rid: Uuid,
        aid: Uuid,
        counts: [Option<i64>; 3],
    ) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT relay_hosted.capture_text_stream_usage($1,$2,$3,$4,$5,$6)")
            .bind(offer)
            .bind(rid)
            .bind(aid)
            .bind(counts[0])
            .bind(counts[1])
            .bind(counts[2])
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn reconcile(
        &self,
        offer: Uuid,
        rid: Uuid,
        aid: Uuid,
        action: &str,
    ) -> Result<String, sqlx::Error> {
        reconcile(&self.pool, offer, rid, aid, action).await
    }

    pub async fn cooldown(
        &self,
        offer: Uuid,
        rid: Uuid,
        aid: Uuid,
        delay_ms: u64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT relay_hosted.record_text_cooldown($1,$2,$3,$4)")
            .bind(offer)
            .bind(rid)
            .bind(aid)
            .bind(i64::try_from(delay_ms).unwrap_or(i64::MAX))
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

impl HostedDispatchAuthority for Claim<'_> {
    fn claim<'a>(
        &'a self,
        a: &'a HostedAttempt,
    ) -> Pin<Box<dyn Future<Output = Result<(), ClaimError>> + Send + 'a>> {
        Box::pin(async move {
            let store = self.store;
            crate::db::closed_setup(&store.pool, &store.origin)
                .await
                .map_err(|_| ClaimError::Unconfirmed)?;
            let id = |s: &str| Uuid::parse_str(s).map_err(|_| ClaimError::Denied);
            let rid = id(&a.request_id)?;
            let aid = id(&a.attempt_id)?;
            let seal = a.body_binding.commitment(&store.sealing);
            let result: String = sqlx::query_scalar(
                "SELECT relay_hosted.claim_text_candidate($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)",
            )
            .bind(&store.origin)
            .bind(rid)
            .bind(aid)
            .bind(id(&a.workspace_id)?)
            .bind(id(&a.key_id)?)
            .bind(id(&a.source_id)?)
            .bind(&a.model_id)
            .bind(i64::try_from(a.authority_revision).map_err(|_| ClaimError::Denied)?)
            .bind(i64::try_from(a.tariff_revision).map_err(|_| ClaimError::Denied)?)
            .bind(a.bound_micro_usd)
            .bind(seal.as_ref())
            .bind(a.max_input_bytes as i32)
            .bind(a.max_output_tokens as i32)
            .bind(self.route.item)
            .bind(self.route.rate_revision)
            .fetch_one(&store.pool)
            .await
            .map_err(|_| ClaimError::Unconfirmed)?;
            match result.as_str() {
                "claimed" => {
                    store.milestone(rid, aid, Milestone::DispatchCommitted);
                    Ok(())
                }
                "busy" => Err(ClaimError::Busy),
                "denied" => Err(ClaimError::Denied),
                _ => Err(ClaimError::Unconfirmed),
            }
        })
    }
}

async fn reconcile(
    pool: &PgPool,
    offer: Uuid,
    rid: Uuid,
    aid: Uuid,
    action: &str,
) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("SELECT relay_hosted.reconcile_text_attempt($1,$2,$3,$4)")
        .bind(offer)
        .bind(rid)
        .bind(aid)
        .bind(action)
        .fetch_one(pool)
        .await
}

/// One bounded pass, independent of current service mode, originator, or published offer.
pub async fn reconcile_due(pool: &PgPool) -> Result<usize, sqlx::Error> {
    let due: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as("SELECT * FROM relay_hosted.due_text()")
        .fetch_all(pool)
        .await?;
    for (oid, rid, aid) in &due {
        reconcile(pool, *oid, *rid, *aid, "deadline").await?;
    }
    Ok(due.len())
}
