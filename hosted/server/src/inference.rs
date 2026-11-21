//! Separate, qualification-only ingress. Never mount the Desktop/LAN gateway as a fallback.
mod capture;
mod diagnostics;
mod routing;
mod store;
mod stream;
pub use store::reconcile_due;
pub(crate) use zenith_relay_core::gateway::hosted::response_usage as evidence;

use crate::{
    access,
    auth::browser,
    config::CaptureLimits,
    db, http,
    outbound::{OutboundPolicy, TextRoute},
    vault::{Purpose, Vault},
};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use capture::Milestone;
use diagnostics::Observation;
use ring::{hmac, rand::SystemRandom};
use routing::{Admission, Output, Selection};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use store::Store;
use uuid::Uuid;
use zenith_relay_core::{
    ProviderSource, WireApi,
    gateway::hosted::{HostedBodyBinding, HostedExecutionError, HostedTextRuntime, HostedUnknown},
};

struct Ingress {
    store: Arc<Store>,
    vault: Vault,
    outbound_policy: OutboundPolicy,
    runtimes: tokio::sync::Mutex<HashMap<Uuid, Arc<HostedTextRuntime>>>,
    selection: Selection,
    capacity: Arc<tokio::sync::Semaphore>,
}

pub async fn check_worker(pool: &PgPool, origin: &str) -> Result<(), &'static str> {
    db::closed_setup(pool, origin).await?;
    let ok:bool=sqlx::query_scalar("WITH allowed(id) AS (SELECT unnest(ARRAY[
        'relay_hosted.qualified_key_workspace(text,uuid,bytea)'::regprocedure,
        'relay_hosted.reserve_published_text(text,uuid,bytea,bytea,bytea,bytea,text,integer,integer,boolean)'::regprocedure,
        'relay_hosted.request_text_candidates(text,uuid,uuid,uuid,bytea)'::regprocedure,
        'relay_hosted.claim_text_candidate(text,uuid,uuid,uuid,uuid,uuid,text,bigint,bigint,bigint,bytea,integer,integer,uuid,bigint)'::regprocedure,
        'relay_hosted.record_text_cooldown(uuid,uuid,uuid,bigint)'::regprocedure,
        'relay_hosted.key_text_models(text,uuid,bytea)'::regprocedure,
        'relay_hosted.capture_text_terminal(uuid,uuid,uuid,text,bigint,bigint,bigint,integer)'::regprocedure,
        'relay_hosted.capture_text_terminal(uuid,uuid,uuid,text,bigint,bigint,bigint,integer,integer,bigint)'::regprocedure,
        'relay_hosted.capture_text_stream_usage(uuid,uuid,uuid,bigint,bigint,bigint)'::regprocedure,
        'relay_hosted.record_request_diagnostic(uuid,uuid,uuid,text,smallint,bigint)'::regprocedure,
        'relay_hosted.reconcile_text_attempt(uuid,uuid,uuid,text)'::regprocedure,
        'relay_hosted.due_text()'::regprocedure, 'relay_hosted.key_text_status(text,uuid,bytea,uuid)'::regprocedure]))
        SELECT
        NOT has_schema_privilege('relay_hosted','CREATE') AND NOT has_database_privilege(current_database(),'CREATE')
        AND NOT EXISTS(SELECT FROM pg_proc WHERE pronamespace='relay_hosted'::regnamespace AND has_function_privilege(oid,'EXECUTE')
            AND oid NOT IN (SELECT id FROM allowed)
            AND oid NOT IN ('relay_hosted.observe_reconciliation(boolean,boolean)'::regprocedure,
                'relay_hosted.capture_target(text,integer,integer,integer,integer)'::regprocedure,
                'relay_hosted.append_capture_milestone(uuid,uuid,uuid,relay_hosted.capture_milestone,bigint)'::regprocedure))
        AND NOT EXISTS(SELECT FROM allowed WHERE NOT has_function_privilege(id,'EXECUTE'))
        AND NOT EXISTS(SELECT FROM pg_class WHERE relnamespace='relay_hosted'::regnamespace AND relkind IN ('r','v')
            AND (has_table_privilege(oid,'DELETE,TRUNCATE,TRIGGER') OR has_any_column_privilege(oid,'INSERT,UPDATE,REFERENCES')
            OR (relname NOT IN ('closed_installation','_sqlx_migrations') AND has_any_column_privilege(oid,'SELECT'))))")
        .fetch_one(pool).await.map_err(|_|"cannot verify inference role")?;
    if ok {
        Ok(())
    } else {
        Err("a separate restricted inference role is required")
    }
}

pub async fn router(
    pool: PgPool,
    origin: &str,
    vault: Vault,
    outbound_policy: OutboundPolicy,
    limits: Option<CaptureLimits>,
) -> Result<Router, &'static str> {
    check_worker(&pool, origin).await?;
    if !local_origin(origin) {
        return Err("qualification requires a loopback installation");
    }
    let store = Arc::new(Store::new(
        pool,
        origin.into(),
        hmac::Key::generate(hmac::HMAC_SHA256, &SystemRandom::new())
            .map_err(|_| "system randomness unavailable")?,
        limits,
    ));
    Ok(Router::new()
        .route("/v1/responses", post(generate))
        .route("/v1/models", get(models))
        .route("/api/hosted/v1/requests/{id}/status", get(status))
        .fallback(http::not_found)
        .method_not_allowed_fallback(http::wrong_method)
        .with_state(Arc::new(Ingress {
            store,
            vault,
            outbound_policy,
            runtimes: Default::default(),
            selection: Selection::default(),
            capacity: Arc::new(tokio::sync::Semaphore::new(32)),
        }))
        .layer(middleware::from_fn(http::response_headers)))
}

pub(crate) fn local_origin(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|u| match u.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    })
}

pub(crate) async fn source(
    vault: &Vault,
    id: Uuid,
    revision: i64,
    proxy: Option<i64>,
    model: &str,
    route: &TextRoute<'_>,
) -> Result<(ProviderSource, reqwest::Client), &'static str> {
    let endpoint = route.endpoint();
    vault
        .check_saved_route(id, endpoint, route.uses_proxy())
        .await
        .map_err(|_| "source differs from the saved route")?;
    let raw = vault
        .read(id, Purpose::Provider, revision)
        .await
        .map_err(|_| "qualified credential unavailable")?;
    let key = std::str::from_utf8(&raw)
        .ok()
        .filter(|t| (1..=1024).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_graphic()))
        .ok_or("invalid source credential")?;
    let proxy_url = if let Some(revision) = proxy {
        let raw = vault
            .read(id, Purpose::Proxy, revision)
            .await
            .map_err(|_| "qualified proxy unavailable")?;
        Some(
            std::str::from_utf8(&raw)
                .map_err(|_| "invalid proxy")?
                .to_owned(),
        )
    } else {
        None
    };
    let client = route.client(proxy_url.as_deref())?;
    let source = ProviderSource {
        id: id.to_string(),
        name: model.into(),
        base_url: endpoint.into(),
        api_key: key.into(),
        wire_api: WireApi::Responses,
        models: vec![model.into()],
        proxy_url,
    };
    Ok((source, client))
}
async fn models(State(s): State<Arc<Ingress>>, h: HeaderMap) -> Response {
    let noauth = || err(401, "relay_invalid_key", None);
    let Some((kid, proof)) = bearer(&h).and_then(access::proof) else {
        return noauth();
    };
    if db::closed_setup(&s.store.pool, &s.store.origin)
        .await
        .is_err()
    {
        return err(503, "relay_unavailable", None);
    }
    match sqlx::query_scalar::<_,Option<Value>>("SELECT relay_hosted.key_text_models($1,$2,$3)")
        .bind(&s.store.origin).bind(kid).bind(proof.as_ref()).fetch_one(&s.store.pool).await{
        Ok(Some(Value::Array(ids)))=>Json(json!({"object":"list","data":ids.into_iter().map(|id|json!({"id":id,"object":"model"})).collect::<Vec<_>>()})).into_response(),
        Ok(None)=>noauth(),_=>err(503,"relay_unavailable",None)
    }
}

// Deserializing into a struct rejects duplicate fields. Canonical serialization
// binds effective defaults; whitespace, field order and explicit false are equivalent.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    model: String,
    input: String,
    max_output_tokens: u32,
    #[serde(default)]
    stream: bool,
    #[serde(default)]
    store: bool,
}

fn err(status: u16, code: &'static str, id: Option<Uuid>) -> Response {
    let mut r = (
        StatusCode::from_u16(status).unwrap(),
        Json(json!({"error":{"type":"relay_error","code":code,"message":code}})),
    )
        .into_response();
    r.headers_mut()
        .insert("x-should-retry", HeaderValue::from_static("false"));
    if let Some(id) = id {
        request_header(&mut r, id)
    }
    r
}
fn request_header(r: &mut Response, id: Uuid) {
    r.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(&id.to_string()).unwrap(),
    );
}
fn bearer(h: &HeaderMap) -> Option<&str> {
    browser::one(h, "authorization")?.strip_prefix("Bearer ")
}
fn tag(key: &hmac::Key, domain: &[u8], bytes: &[u8]) -> hmac::Tag {
    let mut c = hmac::Context::with_key(key);
    c.update(domain);
    c.update(bytes);
    c.sign()
}
async fn generate(State(s): State<Arc<Ingress>>, req: Request) -> Response {
    let noauth = || err(401, "relay_invalid_key", None);
    let invalid = || err(400, "relay_invalid_request", None);
    let unavailable = || err(503, "relay_unavailable", None);
    let Ok(permit) = s.capacity.clone().try_acquire_owned() else {
        return err(429, "relay_capacity", None);
    };
    let Some(raw) = bearer(req.headers()) else {
        return noauth();
    };
    let Some((kid, proof)) = access::proof(raw) else {
        return noauth();
    };
    if db::closed_setup(&s.store.pool, &s.store.origin)
        .await
        .is_err()
    {
        return unavailable();
    }
    match access::qualified_workspace(&s.store.pool, &s.store.origin, raw).await {
        Ok(Some(_)) => (),
        Ok(None) => return noauth(),
        Err(_) => return unavailable(),
    }
    if req.uri().query().is_some()
        || browser::one(req.headers(), "content-type").and_then(|v| v.split(';').next())
            != Some("application/json")
    {
        return invalid();
    }
    let key = hmac::Key::new(hmac::HMAC_SHA256, raw.as_bytes());
    let op = if req.headers().contains_key("idempotency-key") {
        let Some(v) = browser::one(req.headers(), "idempotency-key")
            .filter(|v| (1..=128).contains(&v.len()) && v.bytes().all(|c| c.is_ascii_graphic()))
        else {
            return invalid();
        };
        Some(tag(&key, b"responses-operation-v1\0", v.as_bytes()))
    } else {
        None
    };
    let Ok(Ok(bytes)) =
        tokio::time::timeout(Duration::from_secs(5), to_bytes(req.into_body(), 65536)).await
    else {
        return invalid();
    };
    let Ok(text) = serde_json::from_slice::<Text>(&bytes) else {
        return invalid();
    };
    if text.store
        || text.input.is_empty()
        || text.max_output_tokens == 0
        || text.max_output_tokens > 1000000
        || text.model.len() > 128
    {
        return invalid();
    }
    let Ok(canonical) = serde_json::to_vec(&text) else {
        return invalid();
    };
    if canonical.len() > 65536 {
        return invalid();
    }
    let stable = tag(&key, b"responses-body-v1\0", &canonical);
    let binding = HostedBodyBinding::from_bytes(&canonical);
    let seal = binding.commitment(&s.store.sealing);
    let reserved = s
        .store
        .reserve(
            kid,
            proof.as_ref(),
            op.as_ref().map(AsRef::as_ref),
            stable.as_ref(),
            seal.as_ref(),
            &text.model,
            text.input.len() as i32,
            text.max_output_tokens as i32,
            text.stream,
        )
        .await;
    let (rid, aid, space, epoch, held, offer, tariff) = match reserved {
        Ok((k, Some(r), Some(a), Some(w), Some(e), Some(h), Some(o), Some(t))) if k == "new" => {
            (r, a, w, e, h, o, t)
        }
        Ok((k, id, _, _, _, _, _, _)) => {
            return match k.as_str() {
                "denied" => noauth(),
                "unoffered" => err(403, "relay_model_unavailable", None),
                "funds" => err(402, "relay_insufficient_balance", None),
                "capacity" => err(429, "relay_capacity", None),
                "conflict" => err(409, "relay_idempotency_conflict", None),
                "in_progress" => err(409, "relay_request_in_progress", id),
                "finished" => err(409, "relay_request_result_unavailable", id),
                _ => unavailable(),
            };
        }
        Err(_) => return unavailable(),
    };
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        routing::dispatch(
            &s,
            Admission {
                rid,
                aid,
                space,
                kid,
                epoch,
                held,
                tariff,
                binding,
            },
            &text,
            &canonical,
        ),
    )
    .await;
    let ids = (offer, rid, aid);
    let value = match result {
        Ok(Ok(Output::Json(v))) => {
            s.store.milestone(rid, aid, Milestone::UpstreamResponse);
            v
        }
        Ok(Ok(Output::Stream(running))) => {
            s.store.milestone(rid, aid, Milestone::UpstreamResponse);
            return stream::respond(s.store.clone(), ids, running, permit, started);
        }
        other => {
            s.store.milestone(rid, aid, Milestone::RequestInterrupted);
            match other {
                Ok(Err(error)) => {
                    s.store
                        .diagnostics
                        .record(ids, Observation::Execution(error, text.stream));
                    if let HostedExecutionError::Cooldown(delay_ms) = error {
                        let _ = s.store.cooldown(offer, rid, aid, delay_ms).await;
                    }
                }
                Err(_) => s
                    .store
                    .diagnostics
                    .record(ids, Observation::ExecutionDeadline),
                Ok(Ok(_)) => unreachable!(),
            }
            // If a permit committed, this cannot release even on a timeout or dropped acknowledgement.
            return match s
                .store
                .reconcile(offer, rid, aid, "not_sent")
                .await
                .as_deref()
            {
                Ok("released") => err(503, "relay_not_dispatched", Some(rid)),
                _ => err(503, "relay_outcome_pending", Some(rid)),
            };
        }
    };
    let outcome = match value["status"].as_str() {
        Some("completed") => "completed",
        Some("incomplete") => "incomplete",
        _ => {
            s.store.milestone(rid, aid, Milestone::RequestInterrupted);
            s.store.diagnostics.record(
                ids,
                Observation::Execution(
                    HostedExecutionError::OutcomeUnknown(HostedUnknown::InvalidResponse),
                    false,
                ),
            );
            let _ = s.store.reconcile(offer, rid, aid, "unknown").await;
            return err(503, "relay_outcome_pending", Some(rid));
        }
    };
    if !s
        .store
        .apply_terminal(ids, outcome, evidence(&value), started, None)
        .await
    {
        return err(503, "relay_outcome_pending", Some(rid));
    }
    let mut response = Json(value).into_response();
    request_header(&mut response, rid);
    response
}

async fn status(State(s): State<Arc<Ingress>>, Path(id): Path<String>, h: HeaderMap) -> Response {
    let Some((kid, proof)) = bearer(&h).and_then(access::proof) else {
        return err(401, "relay_invalid_key", None);
    };
    let Ok(id) = Uuid::parse_str(&id) else {
        return http::not_found().await;
    };
    if db::closed_setup(&s.store.pool, &s.store.origin)
        .await
        .is_err()
    {
        return err(503, "relay_unavailable", None);
    }
    match sqlx::query_scalar::<_, Option<Value>>("SELECT relay_hosted.key_text_status($1,$2,$3,$4)")
        .bind(&s.store.origin)
        .bind(kid)
        .bind(proof.as_ref())
        .bind(id)
        .fetch_one(&s.store.pool)
        .await
    {
        Ok(Some(v)) => http::success(v),
        Ok(None) => http::not_found().await,
        Err(_) => err(503, "relay_unavailable", None),
    }
}

#[cfg(test)]
mod outbound_tests;
#[cfg(test)]
mod tests;
