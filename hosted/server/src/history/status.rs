use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Window {
    hours: Option<i32>,
}

pub(super) async fn read(State(s): State<App>, h: HeaderMap, RawQuery(raw): RawQuery) -> Response {
    let a = match identity::credential(&s, &h, false) {
        Ok(a) => a,
        Err(e) => return e.response(),
    };
    let bad = || {
        http::error(
            StatusCode::BAD_REQUEST,
            "invalid_filter",
            "Invalid status period.",
        )
    };
    let query = raw.as_deref().unwrap_or("");
    if query.len() > 32 {
        return bad();
    }
    let Ok(window) = serde_urlencoded::from_str::<Window>(query) else {
        return bad();
    };
    let hours = window.hours.unwrap_or(24);
    if !matches!(hours, 24 | 168) {
        return bad();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return identity::unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.customer_model_status($1,$2,$3,$4,$5)",
    )
    .bind(&s.origin)
    .bind(a.provider)
    .bind(a.binding)
    .bind(a.session.as_ref())
    .bind(hours)
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(data)) if data.get("items").is_some() => http::success(data),
        Ok(None) => identity::denied(),
        _ => identity::unavailable(),
    }
}
