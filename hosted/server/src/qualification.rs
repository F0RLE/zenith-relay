//! Explicit operator check. Reading the catalog or publishing a draft never runs this operation.
mod stream;

use crate::{db, inference, outbound::OutboundPolicy, vault::Vault};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::{future::Future, net::Ipv4Addr, path::Path, pin::Pin, time::Duration};
use uuid::Uuid;
use zenith_relay_core::ProviderSource;
use zenith_relay_core::gateway::hosted::{
    ClaimError, HostedAttempt, HostedBodyBinding, HostedDispatchAuthority, HostedTextRuntime,
};

const CHECK_INPUT: &str = "Reply with OK.";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    id: Uuid,
    scope: Uuid,
    source: Uuid,
    source_label: String,
    endpoint: String,
    model: String,
    provider_revision: i64,
    proxy_revision: i64,
    uses_proxy: bool,
    max_input_bytes: i32,
    max_output_tokens: i32,
    tokens_per_byte: i32,
    overhead_tokens: i32,
    close_after_s: i32,
    valid_for_s: i32,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CheckProfile {
    Stream(stream::Profile),
    Json(Profile),
}

#[derive(Serialize)]
struct BoundProfile<'a> {
    #[serde(flatten)]
    profile: &'a Profile,
    #[serde(skip_serializing_if = "Option::is_none")]
    direct_ipv4: Option<Ipv4Addr>,
}

struct Authorization {
    pool: PgPool,
    origin: String,
    profile: Profile,
    direct_ipv4: Option<Ipv4Addr>,
    web: bool,
}
impl Authorization {
    async fn step(&self, step: &str) -> Result<String, &'static str> {
        let p = &self.profile;
        let binding = sqlx::types::Json(BoundProfile {
            profile: p,
            direct_ipv4: self.direct_ipv4,
        });
        if self.web {
            return sqlx::query_scalar("SELECT relay_hosted.step_web_model_check($1,$2,$3)")
                .bind(&self.origin)
                .bind(binding)
                .bind(step)
                .fetch_one(&self.pool)
                .await
                .map_err(|_| "qualification store unavailable");
        }
        sqlx::query_scalar("SELECT relay_hosted.text_check_step($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(&self.origin)
            .bind(p.scope)
            .bind(p.id)
            .bind(p.source)
            .bind(p.provider_revision)
            .bind(p.proxy_revision)
            .bind(p.uses_proxy)
            .bind(binding)
            .bind(step)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| "qualification store unavailable")
    }
}

struct CatalogClaim<'a> {
    authority: &'a Authorization,
    source: &'a ProviderSource,
    client: &'a reqwest::Client,
}

impl HostedDispatchAuthority for CatalogClaim<'_> {
    fn claim<'a>(
        &'a self,
        a: &'a HostedAttempt,
    ) -> Pin<Box<dyn Future<Output = Result<(), ClaimError>> + Send + 'a>> {
        Box::pin(async move {
            let authority = self.authority;
            if a.request_id != authority.profile.id.to_string()
                || a.model_id != authority.profile.model
                || a.source_id != authority.profile.source.to_string()
            {
                return Err(ClaimError::Denied);
            }
            db::closed_setup(&authority.pool, &authority.origin)
                .await
                .map_err(|_| ClaimError::Unconfirmed)?;
            match authority.step("claim").await.as_deref() {
                Ok("claimed") => {}
                Ok("busy") => return Err(ClaimError::Busy),
                Ok(_) => return Err(ClaimError::Denied),
                Err(_) => return Err(ClaimError::Unconfirmed),
            }
            // Include the upstream catalog read in the same bounded check permit.
            if !matches!(tokio::time::timeout(Duration::from_secs(25),
                zenith_relay_core::observed_responses_models(self.client, self.source)).await,
                Ok(Ok(models)) if models.contains(&authority.profile.model))
            {
                let _ = authority.step("release").await;
                return Err(ClaimError::Denied);
            }
            Ok(())
        })
    }
}
pub struct Scope<'a> {
    pool: &'a PgPool,
    origin: &'a str,
    id: Uuid,
}
impl Scope<'_> {
    pub fn id(&self) -> Uuid {
        self.id
    }
}

async fn check_role(pool: &PgPool, origin: &str, web: bool) -> Result<(), &'static str> {
    db::closed_setup(pool, origin).await?;
    let permitted:bool=sqlx::query_scalar("SELECT NOT has_schema_privilege('relay_hosted','CREATE') AND NOT has_database_privilege(current_database(),'CREATE')
        AND has_function_privilege('relay_hosted.text_check_step(text,uuid,uuid,uuid,bigint,bigint,boolean,jsonb,text)','EXECUTE')
        AND has_function_privilege('relay_hosted.record_text_check(text,uuid,uuid,uuid,bigint,bigint,boolean,text,text,text,integer,integer,integer,integer,integer,integer,inet)','EXECUTE')
        AND has_function_privilege('relay_hosted.text_stream_check(text,uuid,uuid,uuid,text)','EXECUTE')
        AND (NOT $1 OR (has_function_privilege('relay_hosted.take_web_model_check(text)','EXECUTE')
        AND has_function_privilege('relay_hosted.step_web_model_check(text,jsonb,text)','EXECUTE')
        AND has_function_privilege('relay_hosted.finish_web_model_check(text,uuid)','EXECUTE')))
        AND NOT EXISTS(SELECT FROM pg_proc WHERE pronamespace='relay_hosted'::regnamespace AND has_function_privilege(oid,'EXECUTE')
        AND proname NOT IN ('record_text_check','text_check_step','text_check_scope','text_stream_check',
        'take_web_model_check','step_web_model_check','finish_web_model_check'))
        AND NOT EXISTS(SELECT FROM pg_class WHERE relnamespace='relay_hosted'::regnamespace AND relkind IN ('r','v')
        AND (has_table_privilege(oid,'DELETE,TRUNCATE,TRIGGER') OR has_any_column_privilege(oid,'INSERT,UPDATE,REFERENCES')
        OR (relname NOT IN ('closed_installation','_sqlx_migrations') AND has_any_column_privilege(oid,'SELECT'))))")
        .bind(web).fetch_one(pool).await.map_err(|_|"cannot verify qualification role")?;
    if !permitted || !crate::inference::local_origin(origin) {
        return Err("restricted local qualification role required");
    }
    Ok(())
}

pub async fn scope<'a>(pool: &'a PgPool, origin: &'a str) -> Result<Scope<'a>, &'static str> {
    check_role(pool, origin, false).await?;
    let id = sqlx::query_scalar::<_, Option<Uuid>>("SELECT relay_hosted.text_check_scope($1)")
        .bind(origin)
        .fetch_one(pool)
        .await
        .map_err(|_| "qualification scope unavailable")?
        .ok_or("active tester scope required")?;
    Ok(Scope { pool, origin, id })
}
pub async fn check(
    scope: Scope<'_>,
    vault: &Vault,
    file: &Path,
    outbound_policy: &OutboundPolicy,
) -> Result<(), &'static str> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .and_then(|f| f.take(8193).read_to_end(&mut bytes))
        .map_err(|_| "cannot read qualification profile")?;
    if bytes.len() > 8192 {
        return Err("invalid qualification profile");
    }
    let spec = match zenith_relay_core::bounded_json::from_slice(&bytes)
        .map_err(|_| "invalid qualification profile")?
    {
        CheckProfile::Stream(profile) => {
            return stream::check(scope, vault, profile, outbound_policy).await;
        }
        CheckProfile::Json(profile) => profile,
    };
    check_json(scope, vault, spec, outbound_policy, false).await
}

// Claim once; a dead worker never puts a possibly sent check back on the queue.
pub async fn watch(
    pool: &PgPool,
    origin: &str,
    vault: &Vault,
    outbound_policy: &OutboundPolicy,
) -> Result<(), &'static str> {
    if !origin.starts_with("https://") {
        return Err("web model checks require local HTTPS");
    }
    check_role(pool, origin, true).await?;
    loop {
        let profile: Option<sqlx::types::Json<Profile>> =
            sqlx::query_scalar("SELECT relay_hosted.take_web_model_check($1)")
                .bind(origin)
                .fetch_one(pool)
                .await
                .map_err(|_| "model check queue unavailable")?;
        let Some(sqlx::types::Json(profile)) = profile else {
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        };
        let id = profile.id;
        let scope = Scope {
            pool,
            origin,
            id: profile.scope,
        };
        let _ = check_json(scope, vault, profile, outbound_policy, true).await;
        sqlx::query("SELECT relay_hosted.finish_web_model_check($1,$2)")
            .bind(origin)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|_| "cannot acknowledge model check")?;
    }
}

async fn check_json(
    scope: Scope<'_>,
    vault: &Vault,
    spec: Profile,
    outbound_policy: &OutboundPolicy,
    web: bool,
) -> Result<(), &'static str> {
    let Scope {
        pool,
        origin,
        id: approved,
    } = scope;
    let p = &spec;
    if p.scope != approved
        || [p.id, p.source].iter().any(Uuid::is_nil)
        || !valid_model(&p.model)
        || p.source_label.trim() != p.source_label
        || p.source_label.is_empty()
        || if web {
            p.source_label.encode_utf16().count() > 80
        } else {
            p.source_label.len() > 96
        }
        || p.source_label.chars().any(char::is_control)
        || p.provider_revision < 1
        || p.proxy_revision < 0
        || p.uses_proxy && p.proxy_revision == 0
        || !valid_bounds(
            p.max_input_bytes,
            p.max_output_tokens,
            p.tokens_per_byte,
            p.overhead_tokens,
        )
        || !(65..=3600).contains(&p.close_after_s)
        || !(60..=86400).contains(&p.valid_for_s)
    {
        return Err("invalid qualification profile");
    }
    let direct_ipv4 = outbound_policy
        .text_route(p.source, &p.endpoint, p.uses_proxy)?
        .direct_ipv4();
    let authority = Authorization {
        pool: pool.clone(),
        origin: origin.into(),
        profile: spec,
        direct_ipv4,
        web,
    };
    match authority.step("prepare").await?.as_str() {
        "done" => return Ok(()),
        "prepared" => (),
        _ => return Err("check unavailable or previous send unconfirmed"),
    }
    let p = &authority.profile;
    let route = outbound_policy.text_route(p.source, &p.endpoint, p.uses_proxy)?;
    let (source, client) = inference::source(
        vault,
        p.source,
        p.provider_revision,
        p.uses_proxy.then_some(p.proxy_revision),
        &p.model,
        &route,
    )
    .await?;
    let engine = HostedTextRuntime::new(source.clone(), client.clone())
        .map_err(|_| "invalid qualification route")?;
    let output_cap = p.max_output_tokens.min(64);
    let body=serde_json::to_vec(&serde_json::json!({"model":p.model,"input":CHECK_INPUT,"max_output_tokens":output_cap,"store":false,"stream":false})).unwrap();
    let attempt = HostedAttempt {
        workspace_id: p.scope.to_string(),
        key_id: p.id.to_string(),
        request_id: p.id.to_string(),
        attempt_id: p.id.to_string(),
        source_id: p.source.to_string(),
        model_id: p.model.clone(),
        authority_revision: 1,
        tariff_revision: 1,
        bound_micro_usd: 0,
        body_binding: HostedBodyBinding::from_bytes(&body),
        max_input_bytes: p.max_input_bytes as usize,
        max_output_tokens: output_cap as u32,
    };
    let value = engine
        .execute(
            &attempt,
            axum::http::Request::post("/v1/responses")
                .body(body.as_slice())
                .unwrap(),
            &CatalogClaim {
                authority: &authority,
                source: &source,
                client: &client,
            },
        )
        .await
        .map_err(|_| "model check not confirmed")?;
    if value["status"] != "completed"
        || !valid_usage(
            inference::evidence(&value),
            p.tokens_per_byte,
            p.overhead_tokens,
            output_cap,
        )
        || !value["output"].as_array().is_some_and(|items| {
            items.iter().any(|i| {
                i["type"] == "message"
                    && i["role"] == "assistant"
                    && i["content"].as_array().is_some_and(|content| {
                        content.iter().any(|c| {
                            c["type"] == "output_text"
                                && c["text"].as_str().is_some_and(|s| !s.trim().is_empty())
                        })
                    })
            })
        })
    {
        return Err("model check outside approved profile");
    }
    let ok:bool=sqlx::query_scalar("SELECT relay_hosted.record_text_check($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17::text::inet)")
        .bind(origin).bind(p.scope).bind(p.id).bind(p.source).bind(p.provider_revision).bind(p.proxy_revision).bind(p.uses_proxy)
        .bind(&p.endpoint).bind(&p.model).bind(&p.source_label).bind(p.max_input_bytes).bind(p.max_output_tokens)
        .bind(p.tokens_per_byte).bind(p.overhead_tokens).bind(p.close_after_s).bind(p.valid_for_s)
        .bind(direct_ipv4.map(|ip| ip.to_string())).fetch_one(pool).await.map_err(|_|"cannot record route evidence")?;
    if ok {
        Ok(())
    } else {
        Err("scope, source state or credential changed during check")
    }
}

fn valid_usage(
    counts: [Option<i64>; 3],
    tokens_per_byte: i32,
    overhead: i32,
    output_cap: i32,
) -> bool {
    let [Some(input), Some(_), Some(output)] = counts else {
        return false;
    };
    (1..=i64::from(CHECK_INPUT.len() as i32 * tokens_per_byte + overhead)).contains(&input)
        && (1..=i64::from(output_cap)).contains(&output)
}

pub(crate) fn valid_model(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/-".contains(&b))
}

pub(crate) fn valid_bounds(bytes: i32, output: i32, multiplier: i32, overhead: i32) -> bool {
    (CHECK_INPUT.len() as i32..=60000).contains(&bytes)
        && (1..=1_000_000).contains(&output)
        && (1..=16).contains(&multiplier)
        && (0..=65536).contains(&overhead)
}
