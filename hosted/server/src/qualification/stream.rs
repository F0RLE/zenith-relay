use super::{CHECK_INPUT, Scope, valid_usage};
use crate::{db, inference, outbound::OutboundPolicy, vault::Vault};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{future::Future, net::Ipv4Addr, pin::Pin};
use uuid::Uuid;
use zenith_relay_core::gateway::hosted::{
    ClaimError, HostedAttempt, HostedBodyBinding, HostedDispatchAuthority, HostedStreamObservation,
    HostedTerminalKind, HostedTextRuntime,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Profile {
    id: Uuid,
    scope: Uuid,
    stream_inventory: Uuid,
}

#[derive(Deserialize)]
struct Binding {
    source: Uuid,
    endpoint: String,
    model: String,
    provider_revision: i64,
    proxy_revision: i64,
    uses_proxy: bool,
    direct_ipv4: Option<Ipv4Addr>,
    max_input_bytes: usize,
    max_output_tokens: i32,
    tokens_per_byte: i32,
    overhead_tokens: i32,
}

struct Check<'a> {
    scope: Scope<'a>,
    profile: Profile,
}

impl Check<'_> {
    async fn step(&self, phase: &str) -> Result<Value, &'static str> {
        let query = sqlx::query_scalar("SELECT relay_hosted.text_stream_check($1,$2,$3,$4,$5)")
            .bind(self.scope.origin)
            .bind(self.scope.id)
            .bind(self.profile.id)
            .bind(self.profile.stream_inventory)
            .bind(phase);
        let value: Option<Value> = query
            .fetch_one(self.scope.pool)
            .await
            .map_err(|_| "stream qualification store unavailable")?;
        value.ok_or("stream check unavailable for this inventory")
    }
}

impl HostedDispatchAuthority for Check<'_> {
    fn claim<'a>(
        &'a self,
        attempt: &'a HostedAttempt,
    ) -> Pin<Box<dyn Future<Output = Result<(), ClaimError>> + Send + 'a>> {
        Box::pin(async move {
            if attempt.attempt_id != self.profile.id.to_string() {
                return Err(ClaimError::Denied);
            }
            db::closed_setup(self.scope.pool, self.scope.origin)
                .await
                .map_err(|_| ClaimError::Unconfirmed)?;
            let result = self
                .step("claim")
                .await
                .map_err(|_| ClaimError::Unconfirmed)?;
            match result["state"].as_str() {
                Some("claimed") => Ok(()),
                Some("busy") => Err(ClaimError::Busy),
                _ => Err(ClaimError::Denied),
            }
        })
    }
}

pub(super) async fn check(
    scope: Scope<'_>,
    vault: &Vault,
    profile: Profile,
    policy: &OutboundPolicy,
) -> Result<(), &'static str> {
    if scope.id != profile.scope || profile.id.is_nil() || profile.stream_inventory.is_nil() {
        return Err("invalid stream qualification profile");
    }
    let check = Check { scope, profile };
    let prepared = check.step("prepare").await?;
    match prepared["state"].as_str() {
        Some("done") => return Ok(()),
        Some("prepared") => (),
        _ => return Err("stream check unavailable or previous send unconfirmed"),
    }
    let b: Binding =
        serde_json::from_value(prepared).map_err(|_| "invalid stream route binding")?;
    let route = policy.text_route(b.source, &b.endpoint, b.uses_proxy)?;
    if route.direct_ipv4() != b.direct_ipv4 {
        return Err("outbound approval changed since qualification");
    }
    let (source, client) = inference::source(
        vault,
        b.source,
        b.provider_revision,
        b.uses_proxy.then_some(b.proxy_revision),
        &b.model,
        &route,
    )
    .await?;
    let runtime = HostedTextRuntime::new(source, client).map_err(|_| "invalid stream route")?;
    let cap = b.max_output_tokens.min(64);
    let body = serde_json::to_vec(&json!({
        "model":b.model,"input":CHECK_INPUT,"max_output_tokens":cap,"store":false,"stream":true
    }))
    .unwrap();
    let id = check.profile.id.to_string();
    let attempt = HostedAttempt {
        workspace_id: check.scope.id.to_string(),
        key_id: id.clone(),
        request_id: id.clone(),
        attempt_id: id,
        source_id: b.source.to_string(),
        model_id: b.model,
        authority_revision: 1,
        tariff_revision: 1,
        bound_micro_usd: 0,
        body_binding: HostedBodyBinding::from_bytes(&body),
        max_input_bytes: b.max_input_bytes,
        max_output_tokens: cap as u32,
    };
    let mut stream = runtime
        .start_stream(
            &attempt,
            axum::http::Request::post("/v1/responses")
                .body(body.as_slice())
                .unwrap(),
            &check,
        )
        .await
        .map_err(|_| "stream model check not confirmed")?;
    while let Some(frame) = stream
        .next_frame()
        .await
        .map_err(|_| "stream model check not confirmed")?
    {
        if let HostedStreamObservation::Terminal {
            kind,
            usage,
            text_delta,
            ..
        } = frame.observation
        {
            if kind == HostedTerminalKind::Completed
                && text_delta
                && valid_usage(usage, b.tokens_per_byte, b.overhead_tokens, cap)
                && check.step("record").await?["state"] == "done"
            {
                return Ok(());
            }
            break;
        }
    }
    Err("stream model check did not yield text and verified final usage")
}
