//! Private registration of the exact sandbox capabilities a worker supports.
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Serialize)]
pub struct SandboxMethod<'a> {
    pub code: &'a str,
    pub label: &'a str,
    pub min_credit_minor: u32,
    pub max_credit_minor: u32,
    pub fee_minor: u32,
    pub custom_amount: bool,
    pub merchant_pays: bool,
    pub customer_pays: bool,
}

/// Attach capabilities to a provisioned sandbox merchant binding. Repeating
/// the exact registration succeeds; a different profile never rewrites it.
pub async fn register_sandbox_profile(
    worker: &PgPool,
    origin: &str,
    binding: Uuid,
    label: &str,
    methods: &[SandboxMethod<'_>],
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT relay_hosted.register_checkout_profile($1,$2,$3,$4)")
        .bind(origin)
        .bind(binding)
        .bind(label)
        .bind(sqlx::types::Json(methods))
        .fetch_one(worker)
        .await
}
