//! In-process execution of an admitted native Responses text request.
//! The host owns claims and recovery. This entry is not mounted on the LAN router.

mod stream;
pub use stream::{
    HostedStreamFrame, HostedStreamObservation, HostedTerminalKind, HostedTextStream,
};

use crate::protocol::completion::ResponseCompletion;
use crate::runtime::{
    AuthorizationDispatch, AuthorizationIdentityPolicy, AuthorizedRequestError, CandidateLease,
};
use crate::scheduler::rotation::SharedRequestBudget;
use crate::{AdapterRequestContext, GatewayRuntime, ProviderSource, WireApi};
use axum::http::{Method, Request};
use futures_util::future::BoxFuture;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime},
};

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_REPLY_BYTES: usize = 1024 * 1024;

/// Created by the trusted host after admission, not deserialized from HTTP.
/// The bound and tariff revision are inputs; this path never prices or debits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostedAttempt {
    pub workspace_id: String,
    pub key_id: String,
    pub request_id: String,
    pub attempt_id: String,
    pub source_id: String,
    pub model_id: String,
    pub authority_revision: u64,
    pub tariff_revision: u64,
    pub bound_micro_usd: i64,
    pub body_binding: HostedBodyBinding,
    pub max_input_bytes: usize,
    pub max_output_tokens: u32,
}

/// In-process check of the exact admitted bytes, not a persisted idempotency digest.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct HostedBodyBinding([u8; 32]);

impl HostedBodyBinding {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// Only a keyed commitment may cross the host's durable dispatch boundary.
    pub fn commitment(&self, key: &ring::hmac::Key) -> ring::hmac::Tag {
        ring::hmac::sign(key, &self.0)
    }
}

impl std::fmt::Debug for HostedBodyBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostedBodyBinding(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimError {
    Denied,
    /// The source is busy and no dispatch permit committed.
    Busy,
    AlreadyClaimed,
    /// The host may have committed. Recover this attempt instead of reissuing it.
    Unconfirmed,
}

pub trait HostedDispatchAuthority: Send + Sync {
    /// Atomically match the complete admission, current grant and unused attempt.
    /// Only the first successful claim may return Ok, never a replay or a lookup.
    fn claim<'a>(&'a self, attempt: &'a HostedAttempt) -> BoxFuture<'a, Result<(), ClaimError>>;
}

pub(crate) struct SendFence<'a> {
    pub(crate) attempt: &'a HostedAttempt,
    authority: &'a dyn HostedDispatchAuthority,
    confirmed: &'a AtomicBool,
}

impl SendFence<'_> {
    pub(crate) async fn claim(self) -> Result<(), ClaimError> {
        tokio::time::timeout(Duration::from_secs(5), self.authority.claim(self.attempt))
            .await
            .map_err(|_| ClaimError::Unconfirmed)??;
        self.confirmed.store(true, Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostedExecutionError {
    InvalidRequest,
    /// Known local rejection before a dispatch permit committed.
    Unavailable,
    Claim(ClaimError),
    /// A status is an observation, not permission to repeat the request.
    UpstreamStatus(u16),
    /// Explicit provider wait in milliseconds, not execution evidence.
    Cooldown(u64),
    OutcomeUnknown(HostedUnknown),
}

/// The local observation that stopped execution, not proof that upstream execution failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostedUnknown {
    SendUnconfirmed,
    ReadInterrupted,
    ReadTimeout,
    MissingTerminal,
    ResponseLimit,
    InvalidResponse,
}

impl HostedUnknown {
    fn read_error(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::ReadTimeout
        } else {
            Self::ReadInterrupted
        }
    }
}

/// One transport and local capacity gate for a qualified static source route.
pub struct HostedTextRuntime {
    core: GatewayRuntime,
    client: reqwest::Client,
}

struct Reply {
    response: reqwest::Response,
    lease: CandidateLease,
}

impl HostedTextRuntime {
    /// The host supplies the same restricted transport used to qualify the source.
    pub fn new(source: ProviderSource, client: reqwest::Client) -> crate::Result<Self> {
        if source.wire_api != WireApi::Responses || source.models.len() != 1 {
            return Err(crate::Error::Validation(
                "expected one native Responses model".into(),
            ));
        }
        Ok(Self {
            core: GatewayRuntime::hosted_text_source(source)?,
            client,
        })
    }

    pub async fn execute(
        &self,
        attempt: &HostedAttempt,
        incoming: Request<&[u8]>,
        authority: &dyn HostedDispatchAuthority,
    ) -> Result<Value, HostedExecutionError> {
        use HostedExecutionError::OutcomeUnknown;
        let invalid = OutcomeUnknown(HostedUnknown::InvalidResponse);
        let Reply { response, lease } = self.send(attempt, incoming, authority, false).await?;
        // Hold the slot through the response body, not merely through its headers.
        let bytes = crate::transport::collect_limited(response, MAX_REPLY_BYTES)
            .await
            .map_err(|error| {
                OutcomeUnknown(match error {
                    crate::Error::Upstream(error) => HostedUnknown::read_error(error),
                    crate::Error::UpstreamBodyTooLarge => HostedUnknown::ResponseLimit,
                    _ => HostedUnknown::ReadInterrupted,
                })
            })?;
        let value = crate::strict_json::parse(std::str::from_utf8(&bytes).map_err(|_| invalid)?)
            .map_err(|_| invalid)?;
        super::response::validate_upstream_response(&value, &bytes, Some(&attempt.model_id))
            .map_err(|_| invalid)?;
        if value.get("model").and_then(Value::as_str) != Some(&attempt.model_id) {
            return Err(invalid);
        }
        match ResponseCompletion::for_protocol(WireApi::Responses, &value) {
            ResponseCompletion::Completed => {
                lease.settle_rotation_success_with_latency(super::now_ms(), None);
            }
            ResponseCompletion::Incomplete => (),
            ResponseCompletion::Invalid => return Err(invalid),
        }
        Ok(value)
    }

    pub async fn start_stream(
        &self,
        attempt: &HostedAttempt,
        incoming: Request<&[u8]>,
        authority: &dyn HostedDispatchAuthority,
    ) -> Result<HostedTextStream, HostedExecutionError> {
        let reply = self.send(attempt, incoming, authority, true).await?;
        HostedTextStream::new(reply, attempt.model_id.clone())
    }

    async fn send(
        &self,
        attempt: &HostedAttempt,
        incoming: Request<&[u8]>,
        authority: &dyn HostedDispatchAuthority,
        stream: bool,
    ) -> Result<Reply, HostedExecutionError> {
        use HostedExecutionError::{InvalidRequest, OutcomeUnknown, Unavailable};
        let body = text_body(attempt, incoming, stream).ok_or(InvalidRequest)?;
        let key = self
            .core
            .hosted_source_scope(
                &attempt.workspace_id,
                &attempt.key_id,
                &attempt.source_id,
                &attempt.model_id,
            )
            .ok_or(Unavailable)?;
        let budget = SharedRequestBudget::for_incoming_request(1);
        budget.retain_input_bytes(super::request_body::retained_request_bytes(&body));
        let (selected, lease) = self
            .core
            .select_and_reserve_with_budget(
                &key,
                &attempt.model_id,
                &[WireApi::Responses],
                &HashSet::new(),
                (None, None),
                super::now_ms(),
                &budget,
            )
            .await
            .ok_or(Unavailable)?;
        let route = self
            .core
            .executor_route(
                &selected.candidate_id,
                &attempt.model_id,
                &key.scope_snapshot(),
                &[WireApi::Responses],
                stream,
            )
            .filter(|route| {
                route.adapter == crate::SourceAdapter::Native
                    && route.source_model == attempt.model_id
            })
            .ok_or(Unavailable)?;
        let prepared = route
            .adapter
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Responses,
                request: &body,
                model: &route.source_model,
                stream,
                reasoning_mode: route.reasoning_mode,
                cache_write_ttl: route.cache_write_ttl,
                previous: None,
                response_scope: &key.id,
                response_id_seed: &attempt.request_id,
            })
            .map_err(|_| InvalidRequest)?;
        let request = self
            .client
            .post(route.upstream_url.clone())
            .headers(route.upstream_headers.clone())
            .timeout(Duration::from_secs(30))
            .json(prepared.upstream_body());
        let confirmed = AtomicBool::new(false);
        let response = self
            .core
            .send_hosted_text(
                &route,
                request,
                AuthorizationDispatch {
                    client_version: None,
                    identity_policy: AuthorizationIdentityPolicy::PreserveUpstream,
                    turn_scope: None,
                    budget: Some(&budget),
                    lease: Some(&lease),
                    usage_event: None,
                },
                SendFence {
                    attempt,
                    authority,
                    confirmed: &confirmed,
                },
            )
            .await
            .map_err(|error| match error {
                AuthorizedRequestError::HostedClaim(reason) => HostedExecutionError::Claim(reason),
                _ if !confirmed.load(Ordering::Relaxed)
                    && error.execution_certainty()
                        == crate::RotationExecutionCertainty::NotSent =>
                {
                    Unavailable
                }
                _ => OutcomeUnknown(HostedUnknown::SendUnconfirmed),
            })?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS
            && let Some(delay) =
                crate::transport::retry_after_ms(response.headers(), SystemTime::now())
        {
            return Err(HostedExecutionError::Cooldown(delay));
        }
        if !status.is_success() {
            return Err(HostedExecutionError::UpstreamStatus(status.as_u16()));
        }
        Ok(Reply { response, lease })
    }
}

pub fn response_usage(v: &Value) -> [Option<i64>; 3] {
    let usage = &v["usage"];
    let count = |v: &Value| v.as_i64().filter(|n| *n >= 0);
    let input = count(&usage["input_tokens"]);
    let output = count(&usage["output_tokens"]);
    let cached = count(&usage["input_tokens_details"]["cached_tokens"]);
    if cached.zip(input).is_some_and(|(c, i)| c > i)
        || input
            .zip(output)
            .zip(count(&usage["total_tokens"]))
            .is_some_and(|((i, o), total)| i.checked_add(o) != Some(total))
    {
        return [None; 3];
    }
    [input, cached.filter(|_| input.is_some()), output]
}

fn text_body(attempt: &HostedAttempt, request: Request<&[u8]>, stream: bool) -> Option<Value> {
    if request.method() != Method::POST
        || request.uri().path_and_query()?.as_str() != "/v1/responses"
        || request.body().len() > MAX_REQUEST_BYTES
        || attempt.authority_revision == 0
        || attempt.tariff_revision == 0
        || attempt.bound_micro_usd < 0
        || HostedBodyBinding::from_bytes(request.body()) != attempt.body_binding
        || [
            &attempt.workspace_id,
            &attempt.key_id,
            &attempt.request_id,
            &attempt.attempt_id,
        ]
        .into_iter()
        .any(|id| !crate::is_ascii_token(id, 128))
    {
        return None;
    }
    let mut body = crate::strict_json::parse(std::str::from_utf8(request.body()).ok()?).ok()?;
    let fields = body.as_object_mut()?;
    if !fields.keys().all(|k| {
        matches!(
            k.as_str(),
            "model" | "input" | "max_output_tokens" | "stream" | "store"
        )
    }) || fields.get("model")?.as_str()? != attempt.model_id
        || !(1..=attempt.max_input_bytes).contains(&fields.get("input")?.as_str()?.len())
        || !(1..=u64::from(attempt.max_output_tokens))
            .contains(&fields.get("max_output_tokens")?.as_u64()?)
        || fields.get("stream").map_or(Some(false), Value::as_bool) != Some(stream)
        || fields
            .get("store")
            .is_some_and(|value| value.as_bool() != Some(false))
    {
        return None;
    }
    fields.insert("store".into(), Value::Bool(false));
    Some(body)
}
