//! Customer federation is a separate realm from owner passkeys and inference keys.
mod oidc;
pub use oidc::OidcSettings;

use crate::{auth::browser, config::Config, db, http};
use axum::{
    Router,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use oidc::{Failure, Nonce, PkceCodeVerifier, Provider};
use ring::{hmac, rand::SystemRandom};
use serde_json::json;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use uuid::Uuid;

const SESSION: &str = "__Host-relay_customer";
const ATTEMPT: &str = "__Host-relay_customer_attempt";
const LIFE: Duration = Duration::from_secs(180);
type App = Arc<http::StateData>;

pub struct Customer {
    provider: Provider,
    key: hmac::Key,
    attempts: Mutex<Book>,
}

struct Pending {
    state: browser::Token,
    nonce: Nonce,
    verifier: PkceCodeVerifier,
    until: Instant,
    revision: i64,
    deadline: i64,
}

#[derive(Default)]
struct Book {
    starts: VecDeque<Instant>,
    pending: HashMap<[u8; 32], Pending>,
}

impl Book {
    fn start(&mut self, now: Instant) -> bool {
        self.pending.retain(|_, p| p.until > now);
        while self.starts.front().is_some_and(|t| *t + LIFE <= now) {
            self.starts.pop_front();
        }
        if self.starts.len() >= 64 {
            return false;
        }
        self.starts.push_back(now);
        true
    }

    fn take(&mut self, browser: &browser::Token, state: &browser::Token) -> Option<Pending> {
        let pending = self.pending.get(&**browser)?;
        if !bool::from(pending.state.as_ref().ct_eq(state.as_ref())) {
            return None;
        }
        self.pending
            .remove(&**browser)
            .filter(|p| p.until > Instant::now())
    }
}

impl Customer {
    pub async fn configured(
        c: &Config,
        pool: &sqlx::PgPool,
        dist: Option<&std::path::Path>,
    ) -> Result<Option<Self>, &'static str> {
        let Some(settings) = c.customer_oidc.as_ref() else {
            return Ok(None);
        };
        let file = std::env::var_os("ZENITH_RELAY_HOSTED_DIGEST_KEY_FILE")
            .ok_or("customer identity digest key file required")?;
        let path = std::path::Path::new(&file);
        if let Some(dir) = dist
            && path
                .canonicalize()
                .map_err(|_| "digest key unavailable")?
                .starts_with(dir.canonicalize().map_err(|_| "website unavailable")?)
        {
            return Err("customer digest key must be outside the website");
        }
        let key = crate::vault::read_operator_key(path)
            .map_err(|_| "invalid private customer identity digest key file")?;
        let provider = Provider::new(settings.clone(), &c.server.public_origin, &key)?;
        let matches = sqlx::query_scalar::<_, bool>(
            "SELECT relay_hosted.customer_binding_matches($1,$2,$3,$4)",
        )
        .bind(settings.id)
        .bind(&settings.issuer)
        .bind(&settings.client_id)
        .bind(provider.binding.as_slice())
        .fetch_one(pool)
        .await
        .map_err(|_| "customer registration unavailable")?;
        if !matches {
            return Err(
                "customer identity registration or digest key does not match this installation",
            );
        }
        Ok(Some(Self {
            provider,
            key: hmac::Key::generate(hmac::HMAC_SHA256, &SystemRandom::new())
                .map_err(|_| "system randomness unavailable")?,
            attempts: Mutex::default(),
        }))
    }

    pub(crate) fn provider_id(&self) -> Uuid {
        self.provider.settings.id
    }

    pub(crate) fn binding(&self) -> &[u8] {
        &self.provider.binding
    }

    fn tag(&self, purpose: &[u8], token: &browser::Token) -> hmac::Tag {
        let mut ctx = hmac::Context::with_key(&self.key);
        ctx.update(purpose);
        ctx.update(token.as_ref());
        ctx.sign()
    }

    fn csrf_matches(&self, headers: &HeaderMap, token: &browser::Token) -> bool {
        browser::one(headers, "x-relay-csrf")
            .and_then(browser::decode)
            .is_some_and(|v| {
                bool::from(
                    self.tag(b"customer-csrf-v1", token)
                        .as_ref()
                        .ct_eq(v.as_ref()),
                )
            })
    }
}

pub(crate) struct CustomerCredential<'a> {
    pub provider: Uuid,
    pub binding: &'a [u8],
    pub session: hmac::Tag,
}

pub(crate) enum CustomerAccessError {
    SignedOut,
    Forbidden,
    Unavailable,
}
impl CustomerAccessError {
    pub(crate) fn response(self) -> Response {
        match self {
            Self::SignedOut => denied(),
            Self::Forbidden => forbidden(),
            Self::Unavailable => unavailable(),
        }
    }
}

pub(crate) fn credential<'a>(
    s: &'a http::StateData,
    headers: &HeaderMap,
    mutation: bool,
) -> Result<CustomerCredential<'a>, CustomerAccessError> {
    let c = s.customer.as_ref().ok_or(CustomerAccessError::SignedOut)?;
    let token = browser::cookie(headers, SESSION).ok_or(CustomerAccessError::SignedOut)?;
    if mutation && (!browser::same_origin(headers, &s.origin) || !c.csrf_matches(headers, &token)) {
        return Err(CustomerAccessError::Forbidden);
    }
    Ok(CustomerCredential {
        provider: c.provider_id(),
        binding: c.binding(),
        session: c.tag(b"customer-session-v1", &token),
    })
}

pub(crate) fn unavailable() -> Response {
    http::error(
        StatusCode::SERVICE_UNAVAILABLE,
        "customer_unavailable",
        "Customer access unavailable.",
    )
}

pub(crate) fn denied() -> Response {
    http::error(
        StatusCode::UNAUTHORIZED,
        "customer_unauthorized",
        "Customer sign-in required.",
    )
}

fn forbidden() -> Response {
    http::error(
        StatusCode::FORBIDDEN,
        "customer_forbidden",
        "Request not authorized.",
    )
}

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/hosted/v1/auth/customer/{id}/begin", post(begin))
        .route("/api/hosted/v1/auth/customer/{id}/callback", get(callback))
        .route("/api/hosted/v1/auth/customer/logout", post(logout))
        .route("/api/hosted/v1/account", get(account))
}

async fn begin(State(s): State<App>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    let Some(c) = s
        .customer
        .as_ref()
        .filter(|c| c.provider_id().to_string() == id)
    else {
        return http::not_found().await;
    };
    if !browser::same_origin(&headers, &s.origin) {
        return forbidden();
    }
    let now = Instant::now();
    if !c.attempts.lock().unwrap().start(now) {
        return http::error(
            StatusCode::TOO_MANY_REQUESTS,
            "customer_rate_limited",
            "Too many sign-in attempts.",
        );
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let Ok(Some((revision, deadline))) =
        sqlx::query_as::<_, (i64, i64)>("SELECT * FROM relay_hosted.customer_login_context($1)")
            .bind(&s.origin)
            .fetch_optional(&s.pool)
            .await
    else {
        return unavailable();
    };
    let (Some(key), Some(state)) = (browser::random(), browser::random()) else {
        return unavailable();
    };
    let nonce = Nonce::new_random();
    let (url, verifier) = c.provider.authorize(&hex::encode(state.as_ref()), &nonce);
    let mut book = c.attempts.lock().unwrap();
    if let Some(old) = browser::cookie(&headers, ATTEMPT) {
        book.pending.remove(&*old);
    }
    book.pending.insert(
        *key,
        Pending {
            state,
            nonce,
            verifier,
            until: now + LIFE,
            revision,
            deadline,
        },
    );
    let mut response = http::success(json!({"authorization_url":url.as_str()}));
    browser::set_policy(
        &mut response,
        ATTEMPT,
        Some(&key),
        LIFE.as_secs() as u32,
        "Lax",
    );
    response
}

#[derive(serde::Deserialize)]
struct Callback {
    state: String,
    code: Option<String>,
    iss: Option<String>,
    error: Option<String>,
}

async fn callback(
    State(s): State<App>,
    Path(id): Path<String>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let Some(c) = s
        .customer
        .as_ref()
        .filter(|c| c.provider_id().to_string() == id)
    else {
        return http::not_found().await;
    };
    let failure = || Redirect::to("/login#denied").into_response();
    let Some(query) = query.filter(|q| q.len() <= 8192) else {
        return failure();
    };
    let Ok(cb) = serde_urlencoded::from_str::<Callback>(&query) else {
        return failure();
    };
    let (Some(key), Some(state)) = (
        browser::cookie(&headers, ATTEMPT),
        browser::decode(&cb.state),
    ) else {
        return failure();
    };
    let pending = c.attempts.lock().unwrap().take(&key, &state);
    let Some(p) = pending else {
        return failure();
    };
    let response = match cb.code {
        Some(code)
            if cb.error.is_none()
                && cb
                    .iss
                    .as_ref()
                    .is_none_or(|iss| iss == &c.provider.settings.issuer)
                && (1..=2048).contains(&code.len())
                && code.bytes().all(|b| b.is_ascii_graphic()) =>
        {
            finish(&s, c, p, &code).await
        }
        _ => failure(),
    };
    let mut response = response;
    browser::set_policy(&mut response, ATTEMPT, None, 0, "Lax");
    response
}

async fn finish(s: &App, c: &Customer, p: Pending, code: &str) -> Response {
    let failed = || Redirect::to("/login#unavailable").into_response();
    let denied = || Redirect::to("/login#denied").into_response();
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return failed();
    }
    let subject = match c.provider.subject(code, p.verifier, p.nonce).await {
        Ok(subject) if Instant::now() < p.until => subject,
        Err(Failure::Unavailable) => return failed(),
        _ => return denied(),
    };
    let Some(token) = browser::random() else {
        return failed();
    };
    let registration = &c.provider.settings;
    let result = sqlx::query_as::<_, (String, Option<Uuid>)>(
        "SELECT * FROM relay_hosted.finish_customer_login($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(&s.origin)
    .bind(c.provider_id())
    .bind(&registration.issuer)
    .bind(&registration.client_id)
    .bind(c.binding())
    .bind(subject.as_slice())
    .bind(p.revision)
    .bind(p.deadline)
    .bind(c.tag(b"customer-session-v1", &token).as_ref())
    .fetch_optional(&s.pool)
    .await;
    match result {
        Ok(Some((kind, Some(id)))) if kind == "review" => {
            Redirect::to(&format!("/login#review={id}")).into_response()
        }
        Ok(Some((kind, None))) if kind == "signed_in" => {
            let mut r = Redirect::to("/app/overview").into_response();
            browser::set(&mut r, SESSION, Some(&token), 8 * 3600);
            r
        }
        Ok(_) => denied(),
        Err(_) => failed(),
    }
}

async fn account(State(s): State<App>, headers: HeaderMap) -> Response {
    let (Some(c), Some(token)) = (&s.customer, browser::cookie(&headers, SESSION)) else {
        return denied();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = sqlx::query_as::<_, (Uuid, i64, i64, i64)>(
        "SELECT * FROM relay_hosted.customer_wallet($1,$2,$3,$4)",
    )
    .bind(&s.origin)
    .bind(c.provider_id())
    .bind(c.binding())
    .bind(c.tag(b"customer-session-v1", &token).as_ref())
    .fetch_optional(&s.pool)
    .await;
    match result {
        Ok(Some((id, available, reserved, refund))) => http::success(json!({
            "workspace_id":id,"csrf_token":hex::encode(c.tag(b"customer-csrf-v1",&token)),
            "wallet":{"currency":"USD","precision":6,"available_micro_usd":available.to_string(),
                "reserved_micro_usd":reserved.to_string(),"refund_reserved_micro_usd":refund.to_string(),
                "inference_reserved_micro_usd":(reserved-refund).to_string()}
        })),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

async fn logout(State(s): State<App>, headers: HeaderMap) -> Response {
    let (Some(c), Some(token)) = (&s.customer, browser::cookie(&headers, SESSION)) else {
        return denied();
    };
    if !browser::same_origin(&headers, &s.origin) || !c.csrf_matches(&headers, &token) {
        return forbidden();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let ended = sqlx::query_scalar::<_, bool>("SELECT relay_hosted.end_customer_session($1)")
        .bind(c.tag(b"customer-session-v1", &token).as_ref())
        .fetch_one(&s.pool)
        .await;
    match ended {
        Ok(ok) => {
            let mut r = if ok {
                http::success(json!({"signed_out":true}))
            } else {
                denied()
            };
            browser::set(&mut r, SESSION, None, 0);
            browser::set_policy(&mut r, ATTEMPT, None, 0, "Lax");
            if let Some(attempt) = browser::cookie(&headers, ATTEMPT) {
                c.attempts.lock().unwrap().pending.remove(&*attempt);
            }
            r
        }
        Err(_) => unavailable(),
    }
}
