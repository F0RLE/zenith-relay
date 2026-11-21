use super::*;
use axum::extract::Path;
use confirmation::{Approval, Context};
use serde::Deserialize;
use serde_json::{Value, json};
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
struct Command {
    operation_id: Uuid,
    expected_revision: i32,
    command: Action,
}

pub(super) struct Change {
    id: Uuid,
    command: Command,
}

impl Change {
    async fn parse(raw_id: &str, req: Request) -> Option<Self> {
        let id = raw_id.parse::<Uuid>().ok()?;
        let mut command = crate::setup::credential::<Command>(req).await?;
        if id.is_nil()
            || command.operation_id.is_nil()
            || !(0..i32::MAX).contains(&command.expected_revision)
        {
            return None;
        }
        match &mut command.command {
            Action::Save { title, message } => {
                *title = plain(title, 120, false)?.into();
                *message = plain(message, 4000, true)?.into();
            }
            _ if command.expected_revision == 0 => return None,
            _ => {}
        }
        Some(Self { id, command })
    }

    async fn apply(
        &self,
        s: &App,
        tag: &hmac::Tag,
        approval: Option<(&Context, &Approval)>,
    ) -> sqlx::Result<String> {
        let (verb, title, message) = match &self.command.command {
            Action::Save { title, message } => ("save", Some(title), Some(message)),
            Action::Publish {} => ("publish", None, None),
            Action::Discard {} => ("discard", None, None),
        };
        let query = sqlx::query_scalar(
            "SELECT relay_hosted.change_service_notice($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
        )
        .bind(&s.origin)
        .bind(tag.as_ref())
        .bind(self.id)
        .bind(self.command.operation_id)
        .bind(self.command.expected_revision)
        .bind(verb)
        .bind(title)
        .bind(message);
        confirmation::bind_proof(query, approval)
            .fetch_one(&s.pool)
            .await
    }
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
    prepare(&s, &id, req, false).await
}

pub(super) async fn options(
    State(s): State<App>,
    Path(id): Path<String>,
    req: Request,
) -> Response {
    prepare(&s, &id, req, true).await
}

async fn prepare(s: &App, id: &str, req: Request, issue_options: bool) -> Response {
    let tag = match owner_mutation(s, req.headers()) {
        Ok(tag) => tag,
        Err(error) => return error.response(),
    };
    let Some(change) = Change::parse(id, req).await else {
        return invalid();
    };
    if issue_options && !matches!(change.command.command, Action::Publish {}) {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = change.apply(s, &tag, None).await;
    if issue_options && matches!(result.as_deref(), Ok("fresh_required")) {
        let admin = s.auth.as_ref().expect("authenticated");
        return confirmation::start(s, &admin.notice_publications, tag, change).await;
    }
    respond(result, change.command.operation_id)
}

pub(super) async fn verify(State(s): State<App>, Path(id): Path<String>, req: Request) -> Response {
    let Some(admin) = &s.auth else {
        return denied();
    };
    let (change, ctx, proof) =
        match confirmation::verify(&s, &admin.notice_publications, &id, req).await {
            Ok(verified) => verified,
            Err(error) => return error.response(),
        };
    let result = change.apply(&s, &ctx.tag, Some((&ctx, &proof))).await;
    respond(result, change.command.operation_id)
}

fn respond(result: sqlx::Result<String>, operation_id: Uuid) -> Response {
    match result.as_deref() {
        Ok("applied") => http::success(json!({"applied": true, "operation_id": operation_id})),
        Ok("denied") => denied(),
        Ok("invalid") => invalid(),
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
        Ok(code) => confirmation::error(code).unwrap_or_else(unavailable),
        Err(_) => unavailable(),
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
                serde_json::from_value::<Command>(json!({
                    "expected_revision":1,"operation_id":Uuid::new_v4(),"command":payload
                }))
                .is_err()
            );
        }
    }
}
