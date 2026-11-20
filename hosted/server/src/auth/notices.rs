use super::*;
use axum::extract::Path;
use serde::Deserialize;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Save { title: String, message: String },
    Publish {},
    Discard {},
}

// Keep unknown-field rejection on the tagged command rather than a flattened struct.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    operation_id: Uuid,
    expected_revision: i32,
    command: Action,
}

fn plain(value: &str, limit: usize, multiline: bool) -> Option<&str> {
    let value = value.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    (!value.is_empty()
        && value.encode_utf16().count() <= limit
        && !value
            .chars()
            .any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\t'))))
    .then_some(value)
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_notice",
        "Invalid notification.",
    )
}

pub(super) async fn list(State(s): State<App>, headers: HeaderMap) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&headers, SESSION)) else {
        return denied();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>("SELECT relay_hosted.owner_service_notices($1,$2)")
        .bind(&s.origin)
        .bind(admin.tag(b"session-v1", &token).as_ref())
        .fetch_one(&s.pool)
        .await
    {
        Ok(Some(items)) => http::success(items),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

pub(super) async fn change(State(s): State<App>, Path(id): Path<String>, req: Request) -> Response {
    let headers = req.headers();
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(headers, SESSION)) else {
        return denied();
    };
    if !browser::same_origin(headers, &s.origin)
        || !browser::one(headers, "x-relay-csrf")
            .and_then(browser::decode)
            .is_some_and(|v| bool::from(admin.tag(b"csrf-v1", &token).as_ref().ct_eq(v.as_ref())))
    {
        return forbidden();
    }
    let Ok(id) = id.parse::<Uuid>() else {
        return invalid();
    };
    let Some(cmd) = crate::setup::credential::<Change>(req).await else {
        return invalid();
    };
    if id.is_nil() || cmd.operation_id.is_nil() || !(0..i32::MAX).contains(&cmd.expected_revision) {
        return invalid();
    }
    let (action, title, message) = match &cmd.command {
        Action::Save { title, message } => {
            let (Some(title), Some(message)) =
                (plain(title, 120, false), plain(message, 4000, true))
            else {
                return invalid();
            };
            ("save", Some(title), Some(message))
        }
        Action::Publish {} if cmd.expected_revision > 0 => ("publish", None, None),
        Action::Discard {} if cmd.expected_revision > 0 => ("discard", None, None),
        _ => return invalid(),
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = sqlx::query_scalar::<_, String>(
        "SELECT relay_hosted.change_service_notice($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .bind(id)
    .bind(cmd.operation_id)
    .bind(cmd.expected_revision)
    .bind(action)
    .bind(title)
    .bind(message)
    .fetch_one(&s.pool)
    .await;
    match result.as_deref() {
        Ok("applied") => http::success(json!({"applied": true, "operation_id": cmd.operation_id})),
        Ok("denied") => denied(),
        Ok("invalid") => invalid(),
        Ok("fresh_required") => http::error(
            StatusCode::FORBIDDEN,
            "fresh_passkey_required",
            "Verify the owner passkey again.",
        ),
        Ok("conflict") => http::error(
            StatusCode::CONFLICT,
            "notice_changed",
            "Review the current notification.",
        ),
        Ok("no_audience") => http::error(
            StatusCode::CONFLICT,
            "notice_no_audience",
            "No eligible customers.",
        ),
        Ok("capacity") => http::error(
            StatusCode::CONFLICT,
            "notice_capacity",
            "Notification limit reached.",
        ),
        _ => unavailable(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_text_but_rejects_control_characters_and_foreign_fields() {
        assert_eq!(plain(" 标题 ", 2, false), Some("标题"));
        assert_eq!(plain("a\nb", 3, true), Some("a\nb"));
        for bad in [" ", "\n\n", "\u{feff}", "a\u{0}b", "a\nb", "🇺🇸"] {
            assert_eq!(plain(bad, 3, false), None);
        }
        for payload in [
            json!({"action":"save","title":"valid","message":"valid","recipient":"irrelevant"}),
            json!({"action":"publish","title":"never published directly"}),
            json!({"action":"discard","workspace":"not a caller field"}),
        ] {
            assert!(
                serde_json::from_value::<Change>(json!({
                    "expected_revision":1,"operation_id":Uuid::new_v4(),"command":payload
                }))
                .is_err()
            );
        }
    }
}
