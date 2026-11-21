use super::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs::File, io::Read, path::Path};

const MAX_RECEIPTS: usize = 10_000;
const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessEvidence {
    version: u32,
    installation_id: Uuid,
    receipts: Vec<Value>,
}

pub async fn export_customer_access(
    pool: &PgPool,
    origin: &str,
    installation: Uuid,
) -> Result<Vec<u8>, &'static str> {
    closed_setup(pool, origin).await?;
    let receipts: sqlx::types::Json<Vec<Value>> = sqlx::query_scalar(
        "SELECT coalesce((SELECT jsonb_agg(to_jsonb(c) ORDER BY c.workspace_id,c.expected_revision)
           FROM (SELECT * FROM relay_hosted.customer_access_changes
                 ORDER BY workspace_id,expected_revision LIMIT $3) c),'[]'::jsonb)
         FROM relay_hosted.installations i JOIN relay_hosted.service_control s ON s.installation_id=i.id
         WHERE i.id=$1 AND i.public_origin=$2 AND s.restore_operation IS NULL",
    )
    .bind(installation)
    .bind(origin)
    .bind((MAX_RECEIPTS + 1) as i64)
    .fetch_optional(pool)
    .await
    .map_err(|_| "customer access evidence unavailable")?
    .ok_or("customer access evidence requires the matching unfenced installation")?;
    if receipts.len() > MAX_RECEIPTS {
        return Err("customer access evidence exceeds the export limit");
    }
    let bytes = serde_json::to_vec(&AccessEvidence {
        version: 1,
        installation_id: installation,
        receipts: receipts.0,
    })
    .map_err(|_| "cannot encode customer access evidence")?;
    if bytes.len() >= MAX_BYTES {
        return Err("customer access evidence exceeds the export limit");
    }
    Ok(bytes)
}

pub async fn restore_customer_access(
    pool: &PgPool,
    origin: &str,
    installation: Uuid,
    recovery: Uuid,
    path: &Path,
) -> Result<i32, &'static str> {
    if installation.is_nil() || recovery.is_nil() {
        return Err("customer access restore requires non-nil installation and operation IDs");
    }
    if !installation_schema(pool, origin).await? {
        return Err("customer access restore requires a fenced installation");
    }
    let file = File::open(path).map_err(|_| "cannot open customer access evidence")?;
    if !file
        .metadata()
        .map_err(|_| "cannot inspect customer access evidence")?
        .is_file()
    {
        return Err("customer access evidence must be a regular file");
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read customer access evidence")?;
    if bytes.len() > MAX_BYTES {
        return Err("customer access evidence exceeds the restore limit");
    }
    let evidence: AccessEvidence =
        serde_json::from_slice(&bytes).map_err(|_| "invalid customer access evidence")?;
    if evidence.version != 1
        || evidence.installation_id != installation
        || evidence.receipts.len() > MAX_RECEIPTS
    {
        return Err("customer access evidence version or installation does not match");
    }
    sqlx::query_scalar("SELECT relay_hosted.replay_customer_access($1,$2,$3,$4)")
        .bind(origin)
        .bind(installation)
        .bind(recovery)
        .bind(sqlx::types::Json(evidence.receipts))
        .fetch_one(pool)
        .await
        .map_err(|_| "customer access evidence did not match the restore; keep ingress closed")
}
