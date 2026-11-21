mod csv;

use super::App;
use crate::{db, http, identity};
use axum::{
    Router,
    extract::{Path, RawQuery, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use ring::hmac;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

pub(super) fn routes() -> Router<App> {
    Router::new()
        .route("/api/hosted/v1/usage/reports", post(start))
        .route("/api/hosted/v1/usage/reports/{id}", get(summary))
        .route("/api/hosted/v1/usage/reports/{id}/csv", get(export))
        .route("/api/hosted/v1/usage/requests", get(page))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Period {
    hours: i32,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Page {
    report: Option<Uuid>,
    model: Option<String>,
    bucket: Option<i32>,
    key_id: Option<Uuid>,
    execution: Option<String>,
    before: Option<String>,
    id: Option<Uuid>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Overview,
    Page,
    Export,
}
impl View {
    fn as_str(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Page => "page",
            Self::Export => "export",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Series {
    model: Option<String>,
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_filter",
        "Invalid usage filter.",
    )
}

fn result(data: Result<Option<Value>, sqlx::Error>) -> Result<Value, fn() -> Response> {
    let payload = match data {
        Ok(Some(Value::Object(v))) => Value::Object(v),
        Ok(None) => return Err(identity::denied),
        _ => return Err(identity::unavailable),
    };
    if let Some(code) = payload["error"].as_str() {
        return Err(match code {
            "invalid_filter" => invalid,
            "report_not_found" => || {
                http::error(
                    StatusCode::NOT_FOUND,
                    "report_not_found",
                    "Usage report not available.",
                )
            },
            "report_busy" => || {
                http::error(
                    StatusCode::TOO_MANY_REQUESTS,
                    "report_busy",
                    "Too many active usage reports.",
                )
            },
            "report_too_large" => || {
                http::error(
                    StatusCode::TOO_MANY_REQUESTS,
                    "report_too_large",
                    "Too many requests in this period.",
                )
            },
            _ => identity::unavailable,
        });
    }
    Ok(payload)
}

async fn start(State(s): State<App>, req: Request) -> Response {
    let a = match identity::credential(&s, req.headers(), true) {
        Ok(a) => a,
        Err(e) => return e.response(),
    };
    let Some(p) = crate::setup::credential::<Period>(req).await else {
        return invalid();
    };
    if !matches!(p.hours, 24 | 168 | 720) {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return identity::unavailable();
    }
    let v = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.start_usage_report($1,$2,$3,$4,$5)",
    )
    .bind(&s.origin)
    .bind(a.provider)
    .bind(a.binding)
    .bind(a.session.as_ref())
    .bind(p.hours)
    .fetch_one(&s.pool)
    .await;
    result(v).map_or_else(|r| r(), http::success)
}

async fn summary(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let (Ok(id), Some(q)) = (id.parse(), query::<Series>(raw.as_deref())) else {
        return invalid();
    };
    read(
        s,
        h,
        Page {
            report: Some(id),
            model: q.model,
            ..Page::default()
        },
        View::Overview,
    )
    .await
}

async fn page(State(s): State<App>, h: HeaderMap, RawQuery(raw): RawQuery) -> Response {
    let Some(q) = query::<Page>(raw.as_deref()) else {
        return invalid();
    };
    if (q.report.is_none() && (q.id.is_none() || q.has_filter() || q.before.is_some()))
        || (q.id.is_some() && q.before.is_some())
    {
        return invalid();
    }
    read(s, h, q, View::Page).await
}

async fn export(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let (Ok(id), Some(mut q)) = (id.parse(), query::<Page>(raw.as_deref())) else {
        return invalid();
    };
    if q.report.is_some() || q.before.is_some() || q.id.is_some() {
        return invalid();
    }
    q.report = Some(id);
    read(s, h, q, View::Export).await
}

fn query<T: serde::de::DeserializeOwned>(raw: Option<&str>) -> Option<T> {
    let raw = raw.unwrap_or("");
    (raw.len() <= 640)
        .then(|| serde_urlencoded::from_str(raw).ok())
        .flatten()
}

fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 128
        && model.as_bytes()[0].is_ascii_alphanumeric()
        && model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/-".contains(&b))
}

impl Page {
    fn has_filter(&self) -> bool {
        self.model.is_some()
            || self.bucket.is_some()
            || self.key_id.is_some()
            || self.execution.is_some()
    }

    // Model IDs cannot contain the separators. The session proof never leaves the server.
    fn cursor_input(&self, id: Uuid) -> String {
        format!(
            "usage-cursor-v2|{}|{}|{}|{}|{}|{id}",
            self.report.expect("report cursor"),
            self.model.as_deref().unwrap_or(""),
            self.bucket.unwrap_or(-1),
            self.key_id.map(|v| v.to_string()).unwrap_or_default(),
            self.execution.as_deref().unwrap_or("")
        )
    }
    fn decode(&self, key: &hmac::Key, cursor: &str) -> Option<Uuid> {
        let (id, tag) = cursor.split_once('.')?;
        if id.len() != 36 || tag.len() != 64 {
            return None;
        }
        let id = id.parse().ok()?;
        let mut bytes = [0; 32];
        hex::decode_to_slice(tag, &mut bytes).ok()?;
        hmac::verify(key, self.cursor_input(id).as_bytes(), &bytes).ok()?;
        Some(id)
    }
    fn encode(&self, key: &hmac::Key, id: Uuid) -> String {
        format!(
            "{id}.{}",
            hex::encode(hmac::sign(key, self.cursor_input(id).as_bytes()).as_ref())
        )
    }
}

async fn read(s: App, h: HeaderMap, q: Page, view: View) -> Response {
    let a = match identity::credential(&s, &h, false) {
        Ok(a) => a,
        Err(e) => return e.response(),
    };
    if q.model.as_deref().is_some_and(|m| !valid_model(m))
        || q.bucket.is_some_and(|i| !(0..30).contains(&i))
        || q.execution.as_deref().is_some_and(|state| {
            !matches!(
                state,
                "reserved" | "dispatched" | "completed" | "incomplete" | "failed" | "uncertain"
            )
        })
    {
        return invalid();
    }
    let key = hmac::Key::new(hmac::HMAC_SHA256, a.session.as_ref());
    let before = match q.before.as_deref() {
        Some(raw) => match q.decode(&key, raw) {
            Some(id) => Some(id),
            None => return invalid(),
        },
        None => None,
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return identity::unavailable();
    }
    static EXPORT_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let _export_slot = if view == View::Export {
        match EXPORT_SLOTS.try_acquire() {
            Ok(permit) => Some(permit),
            Err(_) => {
                return http::error(
                    StatusCode::TOO_MANY_REQUESTS,
                    "export_busy",
                    "Too many downloads in progress.",
                );
            }
        }
    } else {
        None
    };
    let v = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.read_usage_report($1,$2,$3,$4,$5,
            ROW($6::text,$7::integer,$8::uuid,$9::text)::relay_hosted.usage_filter,$10,$11,$12)",
    )
    .bind(&s.origin)
    .bind(a.provider)
    .bind(a.binding)
    .bind(a.session.as_ref())
    .bind(q.report)
    .bind(&q.model)
    .bind(q.bucket)
    .bind(q.key_id)
    .bind(&q.execution)
    .bind(before)
    .bind(q.id)
    .bind(view.as_str())
    .fetch_one(&s.pool)
    .await;
    let mut payload = match result(v) {
        Ok(payload) => payload,
        Err(r) => return r(),
    };
    if view == View::Export {
        let Some(bytes) = csv::render(payload) else {
            return identity::unavailable();
        };
        use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
        return (
            [
                (CONTENT_TYPE, "text/csv; charset=utf-8"),
                (
                    CONTENT_DISPOSITION,
                    "attachment; filename=\"relay-usage.csv\"",
                ),
            ],
            bytes,
        )
            .into_response();
    }
    if view == View::Page {
        let Some(rows) = payload["items"].as_array_mut() else {
            return identity::unavailable();
        };
        let next = if rows.len() > 32 {
            rows.truncate(32);
            let Some(id) = rows[31]["request_id"].as_str().and_then(|i| i.parse().ok()) else {
                return identity::unavailable();
            };
            Some(q.encode(&key, id))
        } else {
            None
        };
        payload["next_before"] = json!(next);
    }
    http::success(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_only_works_in_its_session_report_and_filter() {
        let key = hmac::Key::new(hmac::HMAC_SHA256, &[1; 32]);
        let other = hmac::Key::new(hmac::HMAC_SHA256, &[2; 32]);
        let mut p = Page {
            report: Some(Uuid::new_v4()),
            model: Some("m".into()),
            bucket: Some(0),
            key_id: Some(Uuid::new_v4()),
            execution: Some("completed".into()),
            ..Page::default()
        };
        let id = Uuid::new_v4();
        let cursor = p.encode(&key, id);
        assert_eq!(p.decode(&key, &cursor), Some(id));
        assert_eq!(p.decode(&other, &cursor), None);
        p.bucket = None;
        assert_eq!(p.decode(&key, &cursor), None);
        p.bucket = Some(0);
        p.model = None;
        assert_eq!(p.decode(&key, &cursor), None);
        p.model = Some("m".into());
        let key_id = p.key_id;
        p.key_id = Some(Uuid::new_v4());
        assert_eq!(p.decode(&key, &cursor), None);
        p.key_id = key_id;
        p.execution = Some("failed".into());
        assert_eq!(p.decode(&key, &cursor), None);
        p.execution = Some("completed".into());
        p.report = Some(Uuid::new_v4());
        assert_eq!(p.decode(&key, &cursor), None);
    }
}
