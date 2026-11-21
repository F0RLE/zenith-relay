use super::*;
use axum::extract::Path;

pub(super) enum Command {
    Rule(catalog::Change),
    MemberModels(catalog::member_models::Change),
    Rotation(catalog::rotation::Change),
    Limit(purchase_limit::Change),
    Serving(source_serving::Change),
    Intake(source_intake::Approval),
    Check(source_model_check::Change),
}

pub(super) async fn verify(State(s): State<App>, Path(id): Path<String>, req: Request) -> Response {
    let Some(admin) = &s.auth else {
        return denied();
    };
    let (command, ctx, proof) =
        match confirmation::verify(&s, &admin.supply_changes, &id, req).await {
            Ok(verified) => verified,
            Err(error) => return error.response(),
        };
    match command {
        Command::Rule(change) => change.apply(&s, &ctx, &proof).await,
        Command::MemberModels(change) => change.apply(&s, &ctx, &proof).await,
        Command::Rotation(change) => change.apply(&s, &ctx, &proof).await,
        Command::Limit(change) => change.apply(&s, &ctx, &proof).await,
        Command::Serving(change) => change.apply(&s, &ctx, &proof).await,
        Command::Intake(change) => change.apply(&s, &ctx, &proof).await,
        Command::Check(change) => change.apply(&s, &ctx, &proof).await,
    }
}
