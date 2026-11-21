use super::*;
use axum::extract::RawQuery;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    after: Option<Uuid>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum InventoryView {
    #[default]
    All,
    Api,
    Trash,
}

impl InventoryView {
    fn as_str(&self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Api => "api",
            Self::Trash => "trash",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerQuery {
    after: Option<Uuid>,
    #[serde(default)]
    view: InventoryView,
}

pub(super) fn parse_query<'a, T: Deserialize<'a>>(
    q: Option<&'a str>,
    limit: usize,
) -> Result<T, ()> {
    let q = q.unwrap_or_default();
    if q.len() > limit {
        return Err(());
    }
    serde_urlencoded::from_str(q).map_err(|_| ())
}

pub(super) fn cursor(q: Option<&str>) -> Result<Option<Uuid>, ()> {
    parse_query::<Cursor>(q, 64).map(|v| v.after)
}

pub(super) async fn read(State(s): State<App>, h: HeaderMap, RawQuery(q): RawQuery) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&h, SESSION)) else {
        return denied();
    };
    let Ok(OwnerQuery { after, view }) = parse_query(q.as_deref(), 96) else {
        return http::error(
            StatusCode::BAD_REQUEST,
            "invalid_cursor",
            "Invalid connection query.",
        );
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.owner_connections($1,$2,$3,$4)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .bind(after)
    .bind(view.as_str())
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(page)) => http::success(page),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}
