use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use sqlx::PgPool;
use std::{path::Path, sync::Arc};
use tower_http::services::{ServeDir, ServeFile};
use uuid::Uuid;

pub struct StateData {
    pub pool: PgPool,
    pub origin: String,
    pub display_name: String,
    pub policies: crate::config::PolicyLinks,
    pub capture: Option<crate::config::CaptureLimits>,
    pub auth: Option<crate::auth::Admin>,
    pub customer: Option<crate::identity::Customer>,
}

#[derive(Serialize)]
struct Meta {
    api_version: &'static str,
    request_id: Uuid,
}

#[derive(Serialize)]
struct PublicConfig<'a> {
    display_name: &'a str,
    login_methods: Vec<Uuid>,
    registration_open: bool,
    studio_enabled: bool,
    #[serde(skip_serializing_if = "crate::config::PolicyLinks::is_empty")]
    policy_links: &'a crate::config::PolicyLinks,
}

#[derive(Serialize)]
struct Envelope<T> {
    data: T,
    meta: Meta,
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: &'static str,
    request_id: Uuid,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<serde_json::Value>,
}

pub(crate) fn error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    error_details(status, code, message, None)
}

pub(crate) fn error_details(
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    details: Option<serde_json::Value>,
) -> Response {
    (
        status,
        Json(ErrorBody {
            error: ErrorDetail {
                code,
                message,
                request_id: Uuid::new_v4(),
                retryable: status == StatusCode::SERVICE_UNAVAILABLE,
                details,
            },
        }),
    )
        .into_response()
}

pub(crate) fn success<T: Serialize>(data: T) -> Response {
    Json(Envelope {
        data,
        meta: Meta {
            api_version: "1",
            request_id: Uuid::new_v4(),
        },
    })
    .into_response()
}

async fn public_config(State(state): State<Arc<StateData>>) -> Response {
    if crate::db::closed_setup(&state.pool, &state.origin)
        .await
        .is_err()
    {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Service unavailable.",
        );
    }
    success(PublicConfig {
        display_name: &state.display_name,
        login_methods: state.customer.iter().map(|c| c.provider_id()).collect(),
        registration_open: false,
        studio_enabled: false,
        policy_links: &state.policies,
    })
}

pub(crate) async fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", "Not found.")
}

pub(crate) async fn wrong_method() -> Response {
    error(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "Method not allowed.",
    )
}

pub(crate) async fn response_headers(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    resp.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    resp.headers_mut().insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'self'"));
    resp
}

pub(crate) fn website<S: Clone + Send + Sync + 'static>(
    mut app: Router<S>,
    dist: &Path,
    pages: &[&str],
) -> Result<Router<S>, &'static str> {
    let index = dist.join("index.html");
    if !index.is_file() || !dist.join("assets").is_dir() {
        return Err("expected the built Hosted web directory");
    }
    let index = ServeFile::new(index);
    // No catch-all SPA fallback: unknown API/inference routes never return HTML.
    for &path in pages {
        app = app.route_service(path, index.clone());
    }
    app = app
        .nest_service(
            "/assets",
            ServeDir::new(dist.join("assets")).append_index_html_on_directories(false),
        )
        .nest_service(
            "/fonts",
            ServeDir::new(dist.join("fonts")).append_index_html_on_directories(false),
        );
    Ok(app)
}

pub fn router(state: StateData, web_dist: Option<&Path>) -> Result<Router, &'static str> {
    let state = Arc::new(state);
    let mut app = crate::auth::routes()
        .merge(crate::identity::routes())
        .merge(crate::access::routes())
        .merge(crate::history::routes())
        .merge(crate::notifications::routes())
        .merge(crate::payments::routes())
        .route("/api/hosted/v1/public/config", get(public_config));
    if let Some(dist) = web_dist {
        app = website(
            app,
            dist,
            &["/", "/docs", "/legal", "/app", "/app/{*page}", "/login"],
        )?;
        app = app.merge(crate::auth::pages(state.clone(), dist));
    }
    Ok(app
        .fallback(not_found)
        .method_not_allowed_fallback(wrong_method)
        .with_state(state)
        .layer(middleware::from_fn(response_headers)))
}
