//! The normal owner realm has no setup, customer or recovery authority.
mod analytics;
pub(crate) mod browser;
mod catalog;
mod checkout;
mod connections;
mod devices;
mod diagnostic_capture;
mod notices;
mod operations;
mod purchase_limit;
mod refunds;
mod service;
mod signin;
mod source_copy;
mod usage;

use crate::{config::Config, db, http};
use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use browser::{ATTEMPT, SESSION, Token};
use ring::{hmac, rand::SystemRandom};
use std::{sync::Arc, time::Duration};
use webauthn_rs::prelude::{Url, Webauthn, WebauthnBuilder};

type App = Arc<http::StateData>;

pub(crate) fn webauthn(c: &Config) -> Result<Option<Webauthn>, &'static str> {
    let Some(rp) = &c.auth else { return Ok(None) };
    let url = Url::parse(&c.server.public_origin).map_err(|_| "invalid public origin")?;
    WebauthnBuilder::new(&rp.admin_rp_id, &url)
        .and_then(|w| w.rp_name(&c.branding.name).build())
        .map(Some)
        .map_err(|_| "invalid passkey origin or RP ID")
}

pub struct Admin {
    webauthn: Webauthn,
    key: hmac::Key,
    attempts: signin::Attempts,
}

impl Admin {
    pub fn configured(c: &Config) -> Result<Option<Self>, &'static str> {
        let Some(webauthn) = webauthn(c)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            webauthn,
            key: hmac::Key::generate(hmac::HMAC_SHA256, &SystemRandom::new())
                .map_err(|_| "system randomness unavailable")?,
            attempts: signin::Attempts::default(),
        }))
    }

    fn tag(&self, purpose: &[u8], token: &Token) -> hmac::Tag {
        let mut context = hmac::Context::with_key(&self.key);
        context.update(purpose);
        context.update(token.as_ref());
        context.sign()
    }
}

fn unavailable() -> Response {
    http::error(
        StatusCode::SERVICE_UNAVAILABLE,
        "admin_unavailable",
        "Owner access unavailable.",
    )
}

fn denied() -> Response {
    http::error(
        StatusCode::UNAUTHORIZED,
        "admin_unauthorized",
        "Owner authentication required.",
    )
}

fn forbidden() -> Response {
    http::error(
        StatusCode::FORBIDDEN,
        "admin_forbidden",
        "Request not authorized.",
    )
}

enum OwnerMutationError {
    SignedOut,
    Forbidden,
}

impl OwnerMutationError {
    fn response(self) -> Response {
        match self {
            Self::SignedOut => denied(),
            Self::Forbidden => forbidden(),
        }
    }
}

fn owner_mutation(s: &App, headers: &HeaderMap) -> Result<hmac::Tag, OwnerMutationError> {
    use subtle::ConstantTimeEq;
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(headers, SESSION)) else {
        return Err(OwnerMutationError::SignedOut);
    };
    if !browser::same_origin(headers, &s.origin)
        || !browser::one(headers, "x-relay-csrf")
            .and_then(browser::decode)
            .is_some_and(|v| bool::from(admin.tag(b"csrf-v1", &token).as_ref().ct_eq(v.as_ref())))
    {
        return Err(OwnerMutationError::Forbidden);
    }
    Ok(admin.tag(b"session-v1", &token))
}

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route(
            "/api/hosted/v1/auth/admin/sign-in/options",
            post(signin::options),
        )
        .route(
            "/api/hosted/v1/auth/admin/sign-in/verify",
            post(signin::verify),
        )
        .route("/api/hosted/v1/auth/admin/logout", post(logout))
        .route("/api/hosted/v1/admin/overview", get(overview))
        .route(
            "/api/hosted/v1/admin/overview/usage",
            get(analytics::overview),
        )
        .route("/api/hosted/v1/admin/usage/requests", get(usage::requests))
        .route("/api/hosted/v1/admin/usage/errors", get(usage::errors))
        .route(
            "/api/hosted/v1/admin/usage/requests/{id}",
            get(usage::detail),
        )
        .route(
            "/api/hosted/v1/admin/payment-methods",
            get(checkout::read).post(checkout::change),
        )
        .route("/api/hosted/v1/admin/payments", get(refunds::list))
        .route(
            "/api/hosted/v1/admin/payments/refunds/{id}",
            post(refunds::review),
        )
        .route("/api/hosted/v1/admin/notifications", get(notices::list))
        .route(
            "/api/hosted/v1/admin/notifications/{id}",
            post(notices::change),
        )
        .route("/api/hosted/v1/admin/connections", get(connections::read))
        .route(
            "/api/hosted/v1/admin/connections/{id}/purchase-limit",
            get(purchase_limit::read).post(purchase_limit::change),
        )
        .route(
            "/api/hosted/v1/admin/devices",
            get(devices::list).post(devices::change),
        )
        .route("/api/hosted/v1/device/connections", get(devices::inventory))
        .route(
            "/api/hosted/v1/admin/source-copy",
            post(source_copy::review),
        )
        .route("/api/hosted/v1/admin/pools/model-rules", get(catalog::read))
        .route(
            "/api/hosted/v1/admin/pools/model-rules/{id}",
            post(catalog::change),
        )
        .route(
            "/api/hosted/v1/admin/operations/service",
            get(service::read),
        )
        .route(
            "/api/hosted/v1/admin/operations/status",
            get(operations::status),
        )
        .route(
            "/api/hosted/v1/admin/operations/events",
            get(operations::events),
        )
        .route(
            "/api/hosted/v1/admin/operations/diagnostic-sessions",
            get(diagnostic_capture::list).post(diagnostic_capture::start),
        )
        .route(
            "/api/hosted/v1/admin/operations/diagnostic-sessions/{id}",
            get(diagnostic_capture::detail),
        )
        .route(
            "/api/hosted/v1/admin/operations/diagnostic-sessions/{id}/stop",
            post(diagnostic_capture::stop),
        )
        .route(
            "/api/hosted/v1/admin/operations/service/transitions",
            post(service::transition),
        )
}

async fn access(s: &App, headers: &HeaderMap) -> Result<Option<(String, String)>, ()> {
    let Some(token) = browser::cookie(headers, SESSION) else {
        return Ok(None);
    };
    let admin = s.auth.as_ref().ok_or(())?;
    db::closed_setup(&s.pool, &s.origin).await.map_err(|_| ())?;
    let mode =
        sqlx::query_scalar::<_, Option<String>>("SELECT relay_hosted.read_admin_overview($1, $2)")
            .bind(&s.origin)
            .bind(admin.tag(b"session-v1", &token).as_ref())
            .fetch_one(&s.pool)
            .await
            .map_err(|_| ())?;
    Ok(mode.map(|mode| (mode, hex::encode(admin.tag(b"csrf-v1", &token).as_ref()))))
}

async fn overview(State(s): State<App>, headers: HeaderMap) -> Response {
    match access(&s, &headers).await {
        Ok(Some((mode, csrf))) => http::success(serde_json::json!({
            "service_mode": mode, "csrf_token": csrf,
        })),
        Ok(None) => denied(),
        Err(()) => unavailable(),
    }
}

async fn logout(State(s): State<App>, headers: HeaderMap) -> Response {
    use subtle::ConstantTimeEq;
    if !browser::same_origin(&headers, &s.origin) {
        return forbidden();
    }
    let Some(token) = browser::cookie(&headers, SESSION) else {
        return denied();
    };
    let Some(admin) = &s.auth else {
        return unavailable();
    };
    let Some(proof) = browser::one(&headers, "x-relay-csrf").and_then(browser::decode) else {
        return forbidden();
    };
    if !bool::from(admin.tag(b"csrf-v1", &token).as_ref().ct_eq(&*proof)) {
        return forbidden();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let ended = sqlx::query_scalar::<_, bool>("SELECT relay_hosted.end_admin_session($1)")
        .bind(admin.tag(b"session-v1", &token).as_ref())
        .fetch_one(&s.pool)
        .await;
    match ended {
        Ok(result) => {
            let mut r = if result {
                http::success(serde_json::json!({"signed_out": true}))
            } else {
                denied()
            };
            browser::set(&mut r, SESSION, None, 0);
            browser::set(&mut r, ATTEMPT, None, 0);
            r
        }
        Err(_) => unavailable(),
    }
}

pub(crate) fn pages(state: App, dist: &std::path::Path) -> Router<App> {
    let index = tower_http::services::ServeFile::new(dist.join("index.html"));
    Router::new()
        .route_service("/admin", index.clone())
        .route_service("/admin/login", index.clone())
        .route_service("/admin/connections", index.clone())
        .route_service("/admin/pools", index.clone())
        .route_service("/admin/notifications", index.clone())
        .route_service("/admin/payment-methods", index.clone())
        .route_service("/admin/payments", index.clone())
        .route_service("/admin/usage", index.clone())
        .route_service("/admin/usage/requests/{id}", index.clone())
        .route_service("/admin/operations", index.clone())
        .route_service("/admin/overview", index)
        .route_layer(middleware::from_fn_with_state(state, page_gate))
}

async fn page_gate(State(s): State<App>, req: Request, next: Next) -> Response {
    let page = req.uri().path();
    let target = match access(&s, req.headers()).await {
        Ok(Some(_))
            if matches!(
                page,
                "/admin/pools"
                    | "/admin/connections"
                    | "/admin/notifications"
                    | "/admin/payment-methods"
                    | "/admin/payments"
                    | "/admin/usage"
                    | "/admin/operations"
            ) || page.starts_with("/admin/usage/requests/") =>
        {
            page
        }
        Ok(Some(_)) => "/admin/overview",
        Ok(None) => "/admin/login",
        Err(()) => return unavailable(),
    };
    if page == target {
        next.run(req).await
    } else {
        Redirect::to(target).into_response()
    }
}
