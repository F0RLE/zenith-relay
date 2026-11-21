use super::*;
use axum::extract::Path;
use serde_json::json;
use uuid::Uuid;

pub(super) enum Command {
    Device(devices::Change),
    Copy(source_copy::Approval),
    Pool(devices::pool::Approval),
}

pub(super) fn receipt(id: Uuid) -> Response {
    http::success(json!({"operation_id":id,"applied":true}))
}

pub(super) async fn verify(State(s): State<App>, Path(id): Path<String>, req: Request) -> Response {
    let Some(admin) = &s.auth else {
        return denied();
    };
    let (command, ctx, proof) =
        match confirmation::verify(&s, &admin.desktop_changes, &id, req).await {
            Ok(verified) => verified,
            Err(error) => return error.response(),
        };
    match command {
        Command::Device(change) => change.apply(&s, &ctx, &proof).await,
        Command::Copy(change) => change.apply(&s, &ctx, &proof).await,
        Command::Pool(change) => change.apply(&s, &ctx, &proof).await,
    }
}
