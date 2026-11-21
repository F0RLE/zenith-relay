use super::{device_denied, device_proof};
use crate::{auth::App, db, http};
use axum::{
    extract::{RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use serde_json::Value;
use zenith_relay_hosted_device::{UsageFilter, valid_hours};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OverviewFilter {
    hours: i32,
}

enum Read {
    Access,
    Overview { hours: i32, metrics: bool },
    Usage(UsageFilter),
}

pub(in crate::auth) async fn access(
    State(s): State<App>,
    h: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    observe(s, h, q, |q| q.is_empty().then_some(Read::Access)).await
}

pub(in crate::auth) async fn overview(
    State(s): State<App>,
    h: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    read_overview(s, h, q, false).await
}

pub(in crate::auth) async fn overview_metrics(
    State(s): State<App>,
    h: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    read_overview(s, h, q, true).await
}

async fn read_overview(s: App, h: HeaderMap, q: Option<String>, metrics: bool) -> Response {
    observe(s, h, q, |q| {
        let f: OverviewFilter = serde_urlencoded::from_str(q).ok()?;
        valid_hours(f.hours).then_some(Read::Overview {
            hours: f.hours,
            metrics,
        })
    })
    .await
}

pub(in crate::auth) async fn usage(
    State(s): State<App>,
    h: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    observe(s, h, q, |q| {
        let f: UsageFilter = serde_urlencoded::from_str(q).ok()?;
        f.valid().then_some(Read::Usage(f))
    })
    .await
}

async fn observe(
    s: App,
    h: HeaderMap,
    q: Option<String>,
    parse: impl FnOnce(&str) -> Option<Read>,
) -> Response {
    let Some((iid, device, proof)) = device_proof(&s, &h) else {
        return device_denied();
    };
    let invalid = || {
        http::error(
            StatusCode::BAD_REQUEST,
            "invalid_device_filter",
            "Invalid filter or expired page. Refresh to start again.",
        )
    };
    let q = q.as_deref().unwrap_or_default();
    if q.len() > 512 {
        return invalid();
    }
    let Some(read) = parse(q) else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return super::unavailable();
    }
    let sql = match read {
        Read::Access => "SELECT relay_hosted.device_access($1,$2,$3,$4)",
        Read::Overview { metrics: false, .. } => {
            "SELECT relay_hosted.device_overview($1,$2,$3,$4,$5)"
        }
        Read::Overview { metrics: true, .. } => {
            "SELECT relay_hosted.device_overview_metrics($1,$2,$3,$4,$5)"
        }
        Read::Usage(_) => "SELECT relay_hosted.device_usage($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    };
    let query = sqlx::query_scalar::<_, Option<Value>>(sql)
        .bind(&s.origin)
        .bind(iid)
        .bind(device)
        .bind(proof.as_slice());
    let query = match read {
        Read::Access => query,
        Read::Overview { hours, .. } => query.bind(hours),
        Read::Usage(f) => query
            .bind(f.hours)
            .bind(f.model)
            .bind(f.execution.map(|e| e.as_str()))
            .bind(f.as_of_ms)
            .bind(f.before),
    };
    match query.fetch_one(&s.pool).await {
        Ok(Some(v)) if v.get("error").is_none() => http::success(v),
        Ok(Some(_)) => invalid(),
        Ok(None) => device_denied(),
        Err(_) => super::unavailable(),
    }
}
