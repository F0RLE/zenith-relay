use super::*;
use axum::extract::Path;

pub(super) enum Command {
    Checkout(checkout::Change),
    Refund(refunds::Decision),
}

pub(super) async fn verify(State(s): State<App>, Path(id): Path<String>, req: Request) -> Response {
    let Some(admin) = &s.auth else {
        return denied();
    };
    let (command, ctx, approval) =
        match confirmation::verify(&s, &admin.payment_changes, &id, req).await {
            Ok(verified) => verified,
            Err(error) => return error.response(),
        };
    let proof = Some((&ctx, &approval));
    match command {
        Command::Checkout(change) => change.apply(&s, &ctx.tag, proof).await,
        Command::Refund(decision) => decision.apply(&s, &ctx.tag, proof).await,
    }
}
