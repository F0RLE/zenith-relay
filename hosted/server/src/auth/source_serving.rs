use super::*;
use axum::extract::Path;
use confirmation::{Approval, Context};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Pause,
    Resume,
    Park,
    Trash,
    Restore,
    Remove,
}

impl Action {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Park => "park",
            Self::Trash => "trash",
            Self::Restore => "restore",
            Self::Remove => "remove",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestChange {
    operation_id: Uuid,
    expected_revision: String,
    action: Action,
}

pub(super) struct Change {
    source: Uuid,
    operation_id: Uuid,
    revision: i64,
    action: Action,
}

impl Change {
    async fn execute(
        &self,
        s: &App,
        tag: &hmac::Tag,
        proof: Option<(&Context, &Approval)>,
    ) -> sqlx::Result<String> {
        let query = sqlx::query_scalar(
            "SELECT relay_hosted.change_source_serving($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        )
        .bind(&s.origin)
        .bind(tag.as_ref())
        .bind(self.source)
        .bind(self.operation_id)
        .bind(self.revision)
        .bind(self.action.as_str());
        confirmation::bind_proof(query, proof)
            .fetch_one(&s.pool)
            .await
    }

    pub(super) async fn apply(&self, s: &App, ctx: &Context, proof: &Approval) -> Response {
        respond(
            self.execute(s, &ctx.tag, Some((ctx, proof))).await,
            self.operation_id,
        )
        .await
    }
}

pub(super) async fn receipt(State(s): State<App>, Path(id): Path<Uuid>, req: Request) -> Response {
    submit(s, id, req, false).await
}

pub(super) async fn options(State(s): State<App>, Path(id): Path<Uuid>, req: Request) -> Response {
    submit(s, id, req, true).await
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_source_serving",
        "Invalid source action.",
    )
}

async fn submit(s: App, source: Uuid, req: Request, prepare: bool) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    let Some(cmd) = crate::setup::credential::<RequestChange>(req).await else {
        return invalid();
    };
    let Ok(revision) = cmd.expected_revision.parse::<i64>() else {
        return invalid();
    };
    if source.is_nil()
        || cmd.operation_id.is_nil()
        || !(0..i64::MAX).contains(&revision)
        || revision.to_string() != cmd.expected_revision
    {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let change = Change {
        source,
        operation_id: cmd.operation_id,
        revision,
        action: cmd.action,
    };
    let result = change.execute(&s, &tag, None).await;
    if prepare && matches!(result.as_deref(), Ok("fresh_required")) {
        let admin = s.auth.as_ref().expect("authenticated");
        let command = supply_confirmation::Command::Serving(change);
        return confirmation::start(&s, &admin.supply_changes, tag, command).await;
    }
    respond(result, change.operation_id).await
}

async fn respond(result: sqlx::Result<String>, operation_id: Uuid) -> Response {
    match result.as_deref() {
        Ok("applied") => http::success(json!({"applied":true,"operation_id":operation_id})),
        Ok("denied") => denied(),
        Ok("missing") => http::not_found().await,
        Ok("invalid") => invalid(),
        Ok("conflict") => http::error(
            StatusCode::CONFLICT,
            "source_serving_changed",
            "Source serving state changed.",
        ),
        Ok("draining") => http::error(
            StatusCode::CONFLICT,
            "source_draining",
            "Source still has work in flight.",
        ),
        Ok(code) => confirmation::error(code).unwrap_or_else(unavailable),
        Err(_) => unavailable(),
    }
}
