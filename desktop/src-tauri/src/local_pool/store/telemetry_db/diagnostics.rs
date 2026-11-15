use super::{ErrorCode, LocalPoolError, Result, db_error};
use rusqlite::{Transaction, params};
use sha2::{Digest, Sha256};

const BATCH_LIMIT: usize = 256;

#[cfg(test)]
mod tests;

/// Immutable snapshots let a late send correction leave the logical result intact.
pub(super) fn store_json(transaction: &Transaction<'_>, json: Option<&str>) -> Result<Option<i64>> {
    let Some(json) = json else { return Ok(None) };
    let fingerprint = Sha256::digest(json.as_bytes());
    transaction
        .prepare_cached(
            "INSERT OR IGNORE INTO usage_diagnostics(fingerprint, data_json) VALUES (?1, ?2)",
        )
        .map_err(db_error)?
        .execute(params![&fingerprint[..], json])
        .map_err(db_error)?;
    let (id, identical): (i64, bool) = transaction
        .prepare_cached("SELECT id, data_json = ?2 FROM usage_diagnostics WHERE fingerprint = ?1")
        .map_err(db_error)?
        .query_row(params![&fingerprint[..], json], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(db_error)?;
    if !identical {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "usage diagnostic fingerprint mismatch",
        ));
    }
    Ok(Some(id))
}

pub(super) fn discard_unreferenced(
    transaction: &Transaction<'_>,
    routing_id: Option<i64>,
    tool_id: Option<i64>,
) -> Result<()> {
    transaction
        .execute(
            "DELETE FROM usage_diagnostics WHERE refs = 0 AND id IN (?1, ?2)",
            params![routing_id, tool_id],
        )
        .map_err(db_error)?;
    Ok(())
}

/// Convert inline history and expire verbose fields in bounded, indexed batches.
/// Core usage, outcomes, tool counts and request-level timing keep their 30-day lifetime.
pub(super) fn compact(transaction: &Transaction<'_>) -> Result<u64> {
    let mut changed = 0;
    for table in ["request_logs", "request_attempts"] {
        let inline = compact_batch(transaction, table, true, BATCH_LIMIT)?;
        changed += inline as u64;
        changed += compact_batch(transaction, table, false, BATCH_LIMIT - inline)? as u64;
    }
    Ok(changed)
}

fn compact_batch(
    transaction: &Transaction<'_>,
    table: &str,
    inline: bool,
    limit: usize,
) -> Result<usize> {
    if limit == 0 {
        return Ok(0);
    }
    let predicate = if inline {
        "routing_json IS NOT NULL OR tool_use_json IS NOT NULL"
    } else {
        "diagnostics_pruned = 0 AND created_at < datetime('now', '-7 days')
         AND (routing_diagnostics_id IS NOT NULL OR tool_diagnostics_id IS NOT NULL)"
    };
    let rows = {
        let mut statement = transaction.prepare(&format!(
            "SELECT rowid,
                COALESCE(routing_json, (SELECT data_json FROM usage_diagnostics WHERE id = routing_diagnostics_id)),
                COALESCE(tool_use_json, (SELECT data_json FROM usage_diagnostics WHERE id = tool_diagnostics_id)),
                created_at < datetime('now', '-7 days')
             FROM {table} WHERE {predicate} ORDER BY {} LIMIT ?1",
            if inline { "id" } else { "created_at, id" },
        )).map_err(db_error)?;
        statement
            .query_map([limit as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            })
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)?
    };
    for (id, routing, tools, expired) in &rows {
        let routing = retained_json(routing.as_deref(), *expired, true);
        let tools = retained_json(tools.as_deref(), *expired, false);
        let routing_id = store_json(transaction, routing.as_deref())?;
        let tool_id = store_json(transaction, tools.as_deref())?;
        transaction
            .execute(
                &format!(
                    "UPDATE {table} SET routing_json = NULL, tool_use_json = NULL,
                routing_diagnostics_id = ?1, tool_diagnostics_id = ?2,
                diagnostics_pruned = ?3 WHERE rowid = ?4",
                ),
                params![routing_id, tool_id, expired, id],
            )
            .map_err(db_error)?;
    }
    Ok(rows.len())
}

fn retained_json(json: Option<&str>, expired: bool, routing: bool) -> Option<String> {
    json.map(|json| {
        if !expired {
            return json.to_owned();
        }
        // Keep malformed historical evidence intact rather than inventing a replacement.
        let Ok(serde_json::Value::Object(mut object)) = serde_json::from_str(json) else {
            return json.to_owned();
        };
        let fields: &[&str] = if routing {
            &["cacheContext", "cacheAffinity", "routeLatency", "transport"]
        } else {
            &[
                "clientContext",
                "upstreamContext",
                "basisPointsCatalog",
                "clientSchemaBytes",
                "forwardedSchemaBytes",
            ]
        };
        for field in fields {
            object.remove(*field);
        }
        if routing && let Some(serde_json::Value::Object(timing)) = object.get_mut("requestTiming")
        {
            timing.remove("waiting");
        }
        serde_json::Value::Object(object).to_string()
    })
}
