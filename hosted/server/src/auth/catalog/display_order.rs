use super::*;
use crate::qualification::valid_model;

pub(in crate::auth) async fn read(
    State(s): State<App>,
    h: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&h, SESSION)) else {
        return denied();
    };
    if query.is_some() {
        return invalid_order();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    respond(
        sqlx::query_scalar("SELECT relay_hosted.owner_model_display_order($1,$2)")
            .bind(&s.origin)
            .bind(admin.tag(b"session-v1", &token).as_ref())
            .fetch_one(&s.pool)
            .await,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Order {
    expected_revision: String,
    model_display_order: Vec<String>,
}

pub(in crate::auth) async fn change(
    State(s): State<App>,
    RawQuery(query): RawQuery,
    req: Request,
) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    let Some(order) = crate::setup::credential::<Order>(req).await else {
        return invalid_order();
    };
    if query.is_some()
        || order.expected_revision.len() != 64
        || !order
            .expected_revision
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || order.model_display_order.len() > 128
        || !order.model_display_order.iter().all(|id| valid_model(id))
    {
        return invalid_order();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    respond(
        sqlx::query_scalar("SELECT relay_hosted.change_model_display_order($1,$2,$3,$4)")
            .bind(&s.origin)
            .bind(tag.as_ref())
            .bind(order.expected_revision)
            .bind(order.model_display_order)
            .fetch_one(&s.pool)
            .await,
    )
}

fn invalid_order() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_model_order",
        "Invalid model display order.",
    )
}

fn respond(result: sqlx::Result<Option<Value>>) -> Response {
    match result {
        Ok(Some(view)) => match view.get("error").and_then(Value::as_str) {
            Some("conflict") => http::error(
                StatusCode::CONFLICT,
                "model_order_changed",
                "Reload the latest catalog order.",
            ),
            Some("invalid") => invalid_order(),
            None => http::success(view),
            _ => unavailable(),
        },
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}
