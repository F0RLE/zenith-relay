//! Normal owner key management never grants local recovery authority.
use super::*;
use axum::extract::Path;
use confirmation::{Approval, Context, Verification};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::types::Json;
use std::time::Instant;
use webauthn_rs::prelude::{
    Passkey, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, Uuid,
};

const KEY_LIMIT: usize = 8;

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Intent {
    Add { label: Option<String> },
    Revoke { passkey_id: Uuid },
}

pub(super) enum Step {
    Verify(Intent, Verification),
    Create(Option<String>, Approval, PasskeyRegistration),
    Confirm(
        Option<String>,
        Approval,
        Box<Passkey>,
        PasskeyAuthentication,
    ),
}

fn passkey_error(status: StatusCode, code: &'static str) -> Response {
    let message = match code {
        "invalid_passkey_change" => "Invalid passkey change.",
        "invalid_credential" => "Passkey verification failed.",
        "last_passkey" => "Keep at least one owner passkey.",
        "passkey_limit" => "Owner passkey limit reached.",
        "passkey_changed" => "Passkeys changed. Try again.",
        "passkey_rate_limited" => "Too many passkey attempts.",
        _ => "Passkey ceremony expired or already used.",
    };
    http::error(status, code, message)
}

fn expired() -> Response {
    passkey_error(StatusCode::CONFLICT, "passkey_ceremony_expired")
}

fn invalid() -> Response {
    passkey_error(StatusCode::BAD_REQUEST, "invalid_credential")
}

fn status_error(code: &'static str) -> Response {
    passkey_error(StatusCode::CONFLICT, code)
}

#[derive(Serialize)]
struct Challenge<T> {
    ceremony_id: Uuid,
    step: &'static str,
    options: T,
}

fn challenge<T: Serialize>(admin: &Admin, ctx: Context, pending: Step, options: T) -> Response {
    let step = match pending {
        Step::Verify(..) => "verify",
        Step::Create(..) => "create",
        Step::Confirm(..) => "confirm",
    };
    let ceremony_id = admin.passkeys.issue(ctx, pending);
    http::success(Challenge {
        ceremony_id,
        step,
        options,
    })
}

pub(super) async fn list(State(s): State<App>, headers: HeaderMap) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&headers, SESSION)) else {
        return denied();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>("SELECT relay_hosted.owner_passkeys($1,$2)")
        .bind(&s.origin)
        .bind(admin.tag(b"session-v1", &token).as_ref())
        .fetch_one(&s.pool)
        .await
    {
        Ok(Some(projection)) => http::success(projection),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

pub(super) async fn start(State(s): State<App>, req: Request) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    let admin = s
        .auth
        .as_ref()
        .expect("owner mutation requires configured passkeys");
    let Some(intent) = crate::setup::credential::<Intent>(req).await else {
        return passkey_error(StatusCode::BAD_REQUEST, "invalid_passkey_change");
    };
    let now = Instant::now();
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let Ok(mut rows) = confirmation::candidates(&s, &tag).await else {
        return unavailable();
    };
    let Some(first) = rows.first() else {
        return denied();
    };
    let exclude = match &intent {
        Intent::Add { label } => {
            if label.as_ref().is_some_and(|v| {
                v.is_empty()
                    || v.trim() != v
                    || v.chars().count() > 48
                    || v.chars().any(char::is_control)
            }) {
                return passkey_error(StatusCode::BAD_REQUEST, "invalid_passkey_change");
            }
            if rows.len() >= KEY_LIMIT {
                return status_error("passkey_limit");
            }
            None
        }
        Intent::Revoke { passkey_id } => {
            if !rows.iter().any(|row| row.passkey_id == *passkey_id) {
                return status_error("passkey_changed");
            }
            if rows.len() < 2 {
                return status_error("last_passkey");
            }
            Some(*passkey_id)
        }
    };
    if !admin.passkeys.reserve(now) {
        return passkey_error(StatusCode::TOO_MANY_REQUESTS, "passkey_rate_limited");
    }
    let ctx = Context::new(tag, first, now);
    rows.retain(|row| Some(row.passkey_id) != exclude);
    let Some((mut options, auth)) = Verification::start(&admin.webauthn, rows) else {
        return unavailable();
    };
    let Some(timeout) = ctx.timeout() else {
        return expired();
    };
    options.public_key.timeout = Some(timeout);
    challenge(admin, ctx, Step::Verify(intent, auth), options)
}

pub(super) async fn advance(
    State(s): State<App>,
    Path(id): Path<String>,
    req: Request,
) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    let admin = s
        .auth
        .as_ref()
        .expect("owner mutation requires configured passkeys");
    let Ok(id) = Uuid::parse_str(&id) else {
        return expired();
    };
    let pending = admin.passkeys.take(id, &tag, Instant::now());
    let Some((ctx, step)) = pending else {
        return expired();
    };
    match access(&s, req.headers()).await {
        Ok(Some(_)) => (),
        Ok(None) => return denied(),
        Err(()) => return unavailable(),
    };
    match step {
        Step::Verify(intent, state) => {
            let Some(assertion) = crate::setup::credential::<PublicKeyCredential>(req).await else {
                return invalid();
            };
            let Some(approval) = state.verify(&admin.webauthn, &ctx, &assertion) else {
                return invalid();
            };
            match intent {
                Intent::Revoke { passkey_id } => {
                    change(&s, ctx, approval, Change::Revoke(passkey_id)).await
                }
                Intent::Add { label } => {
                    let exclude = state.credential_ids();
                    let Some((mut options, state)) =
                        enrollment::options(&admin.webauthn, ctx.owner, Some(exclude))
                    else {
                        return unavailable();
                    };
                    let Some(timeout) = ctx.timeout() else {
                        return expired();
                    };
                    options.public_key.timeout = Some(timeout);
                    challenge(admin, ctx, Step::Create(label, approval, state), options)
                }
            }
        }
        Step::Create(label, approval, state) => {
            let Some(registration) =
                crate::setup::credential::<RegisterPublicKeyCredential>(req).await
            else {
                return invalid();
            };
            let Some((mut options, proof, key)) =
                enrollment::register(&admin.webauthn, &state, &registration)
            else {
                return invalid();
            };
            let Some(timeout) = ctx.timeout() else {
                return expired();
            };
            options.public_key.timeout = Some(timeout);
            challenge(
                admin,
                ctx,
                Step::Confirm(label, approval, Box::new(key), proof),
                options,
            )
        }
        Step::Confirm(label, approval, key, state) => {
            let Some(assertion) = crate::setup::credential::<PublicKeyCredential>(req).await else {
                return invalid();
            };
            let Some(key) = enrollment::prove(&admin.webauthn, ctx.owner, &state, *key, &assertion)
            else {
                return invalid();
            };
            change(&s, ctx, approval, Change::Add(&key, label.as_deref())).await
        }
    }
}

enum Change<'a> {
    Add(&'a Passkey, Option<&'a str>),
    Revoke(Uuid),
}

async fn change(s: &App, ctx: Context, approval: Approval, change: Change<'_>) -> Response {
    if ctx.timeout().is_none() {
        return expired();
    }
    let (revoke, key, label) = match change {
        Change::Add(key, label) => (None, Some(key), label),
        Change::Revoke(id) => (Some(id), None, None),
    };
    let result = sqlx::query_scalar::<_, String>(
        "SELECT relay_hosted.change_owner_passkey($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
    )
    .bind(&s.origin)
    .bind(ctx.tag.as_ref())
    .bind(ctx.owner)
    .bind(approval.proof.cred_id().as_slice())
    .bind(approval.revision)
    .bind(i64::from(approval.proof.counter()))
    .bind(approval.proof.backup_state())
    .bind(approval.proof.backup_eligible())
    .bind(ctx.deadline_ms)
    .bind(revoke)
    .bind(key.map(|k| k.cred_id().as_slice()))
    .bind(key.map(Json))
    .bind(label)
    .fetch_one(&s.pool)
    .await;
    match result.as_deref() {
        Ok("applied") => http::success(json!({
            "step": if revoke.is_some() { "revoked" } else { "added" }, "session_ended": false
        })),
        Ok("signed_out") => {
            let mut r = http::success(json!({"step":"revoked","session_ended":true}));
            browser::set(&mut r, SESSION, None, 0);
            r
        }
        Ok("denied") => denied(),
        Ok("expired") => expired(),
        Ok("last_passkey") => status_error("last_passkey"),
        Ok("passkey_limit") => status_error("passkey_limit"),
        Ok("changed") => status_error("passkey_changed"),
        _ => unavailable(),
    }
}
