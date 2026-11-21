use super::{
    Ingress, Text, source,
    store::{Claim, Route},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
use zenith_relay_core::{
    gateway::hosted::{
        ClaimError, HostedAttempt, HostedBodyBinding, HostedExecutionError, HostedTextRuntime,
        HostedTextStream,
    },
    scheduler::rotation::{
        AdmissionError, LeaseId, RequestId, RotationCandidate, RotationEngine, RotationRequest,
    },
};

#[derive(Default)]
pub(super) struct Selection {
    current: Mutex<Option<(Uuid, Arc<Mutex<RotationEngine>>)>>,
    sequence: AtomicU64,
}

impl Selection {
    fn for_scope(&self, scope: Uuid) -> Option<Arc<Mutex<RotationEngine>>> {
        let mut current = self.current.lock().ok()?;
        if let Some((_, engine)) = current.as_ref().filter(|(id, _)| *id == scope) {
            return Some(engine.clone());
        }
        let engine = Arc::new(Mutex::new(RotationEngine::default()));
        *current = Some((scope, engine.clone()));
        Some(engine)
    }
}

// A process-local reservation only. PostgreSQL authorizes the physical slot
// across scopes and processes; neither mutex is held while awaiting I/O.
struct Slot {
    engine: Arc<Mutex<RotationEngine>>,
    lease: LeaseId,
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut engine) = self.engine.lock() {
            engine.release_unstarted(self.lease);
        }
    }
}

pub(super) struct RunningStream {
    pub upstream: HostedTextStream,
    _slot: Slot,
}

pub(super) enum Output {
    Json(Value),
    Stream(Box<RunningStream>),
}

pub(super) struct Admission {
    pub rid: Uuid,
    pub aid: Uuid,
    pub space: Uuid,
    pub kid: Uuid,
    pub epoch: i64,
    pub held: i64,
    pub tariff: i64,
    pub binding: HostedBodyBinding,
}

pub(super) async fn dispatch(
    ingress: &Ingress,
    a: Admission,
    text: &Text,
    bytes: &[u8],
) -> Result<Output, HostedExecutionError> {
    use HostedExecutionError::Unavailable;
    let store = &ingress.store;
    let seal = a.binding.commitment(&store.sealing);
    let request = RotationRequest::new(
        RequestId(ingress.selection.sequence.fetch_add(1, Ordering::Relaxed)),
        &text.model,
        &text.model,
    );
    let mut refused = BTreeSet::new();
    loop {
        let candidates = store
            .candidates(a.rid, a.aid, a.kid, seal.as_ref())
            .await
            .map_err(|_| Unavailable)?;
        let engine = ingress
            .selection
            .for_scope(candidates.scope)
            .ok_or(Unavailable)?;
        let mut wait = false;
        let mut routes = BTreeMap::new();
        let mut costs = BTreeMap::new();
        for route in candidates.items {
            if refused.contains(&route.item) {
                continue;
            }
            if route.busy {
                wait = true;
                continue;
            }
            let id = route.item.to_string();
            costs.insert(id.clone(), route.cost);
            routes.insert(id, route);
        }
        while !costs.is_empty() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as u64);
            let selected = {
                // Other holds can carry a different policy. Install this hold's
                // mode and ranks atomically with selection, preserving leases.
                let mut engine = engine.lock().map_err(|_| Unavailable)?;
                engine.set_mode(candidates.mode);
                for id in costs.keys() {
                    let route = &routes[id];
                    if !engine
                        .candidate(id)
                        .is_some_and(|c| c.priority == route.priority)
                    {
                        let mut candidate = RotationCandidate::new(id, &route.model, &route.model)
                            .with_capacity(route.source.to_string(), 1);
                        candidate.priority = route.priority;
                        engine.upsert(candidate).map_err(|_| Unavailable)?;
                    }
                }
                engine.reserve_priced(&request, &costs, now)
            };
            let selected = match selected {
                Ok(selected) => selected,
                Err(AdmissionError::NoEligibleCandidate) => {
                    wait = true;
                    break;
                }
                Err(_) => return Err(Unavailable),
            };
            let slot = Slot {
                engine: engine.clone(),
                lease: selected.lease_id,
            };
            costs.remove(&selected.candidate_id);
            let route = &routes[&selected.candidate_id];
            match execute_route(ingress, &a, route, text, bytes, slot).await {
                // Only a definitive pre-claim rejection can switch supply.
                Err(Unavailable | HostedExecutionError::Claim(ClaimError::Denied)) => {
                    refused.insert(route.item);
                }
                Err(HostedExecutionError::Claim(ClaimError::Busy)) => wait = true,
                result => return result,
            }
        }
        if !wait {
            return Err(Unavailable);
        }
        // Re-read only the original reservation. PostgreSQL owns its claim deadline.
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn execute_route(
    ingress: &Ingress,
    a: &Admission,
    route: &Route,
    text: &Text,
    bytes: &[u8],
    slot: Slot,
) -> Result<Output, HostedExecutionError> {
    let runtime = runtime(ingress, route)
        .await
        .map_err(|_| HostedExecutionError::Unavailable)?;
    let attempt = HostedAttempt {
        workspace_id: a.space.to_string(),
        key_id: a.kid.to_string(),
        request_id: a.rid.to_string(),
        attempt_id: a.aid.to_string(),
        source_id: route.source.to_string(),
        model_id: route.model.clone(),
        authority_revision: a.epoch as u64,
        tariff_revision: a.tariff as u64,
        bound_micro_usd: a.held,
        body_binding: a.binding,
        max_input_bytes: route.input_bytes,
        max_output_tokens: text.max_output_tokens,
    };
    let incoming = axum::http::Request::post("/v1/responses")
        .body(bytes)
        .unwrap();
    let authority = Claim {
        store: &ingress.store,
        route,
    };
    if text.stream {
        let upstream = runtime.start_stream(&attempt, incoming, &authority).await?;
        Ok(Output::Stream(Box::new(RunningStream {
            upstream,
            _slot: slot,
        })))
    } else {
        runtime
            .execute(&attempt, incoming, &authority)
            .await
            .map(Output::Json)
    }
}

pub(super) async fn runtime(
    s: &Ingress,
    route: &Route,
) -> Result<Arc<HostedTextRuntime>, &'static str> {
    let outbound = s
        .outbound_policy
        .text_route(route.source, &route.endpoint, route.uses_proxy)?;
    if route.direct_ipv4 != outbound.direct_ipv4() {
        return Err("outbound approval changed since qualification");
    }
    if let Some(runtime) = s.runtimes.lock().await.get(&route.item).cloned() {
        return Ok(runtime);
    }
    let (src, client) = source(
        &s.vault,
        route.source,
        route.provider_revision,
        route.uses_proxy.then_some(route.proxy_revision),
        &route.model,
        &outbound,
    )
    .await?;
    let runtime =
        Arc::new(HostedTextRuntime::new(src, client).map_err(|_| "invalid qualified route")?);
    let mut cache = s.runtimes.lock().await;
    if cache.len() >= 128 {
        cache.retain(|_, v| Arc::strong_count(v) > 1);
    }
    Ok(cache.entry(route.item).or_insert(runtime).clone())
}
