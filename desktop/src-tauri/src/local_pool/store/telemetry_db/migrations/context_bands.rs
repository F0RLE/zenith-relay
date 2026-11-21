use super::super::{
    Result, db_error,
    record::{UsageAggregate, apply_aggregate_delta},
};
use rusqlite::{Connection, TransactionBehavior};

/// Reclassify retained requests using their original input counts. Archived
/// tokens have no per-request sizes left; keep their existing bands intact.
pub(in crate::local_pool::store::telemetry_db) fn migrate_032(
    connection: &mut Connection,
) -> Result<()> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    {
        let mut statement = transaction
            .prepare(
                "SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
                    COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
                    input_tokens, cached_input_tokens, cache_write_input_tokens, cache_write_ttl,
                    output_tokens, total_tokens, applied_service_tier
                 FROM request_logs
                 WHERE usage_aggregate_recorded = 1
                    AND ((input_tokens > 100000 AND input_tokens <= 200000)
                        OR input_tokens > 512000)",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                let aggregate = UsageAggregate::from_row(row, 0)?;
                let tier: Option<String> = row.get(9)?;
                let aggregate = if tier
                    .as_deref()
                    .is_some_and(|tier| tier.trim().eq_ignore_ascii_case("ultrafast"))
                {
                    aggregate.with_price_class("standard")
                } else {
                    aggregate
                };
                Ok((aggregate, row.get::<_, i64>(3)?))
            })
            .map_err(db_error)?;
        for row in rows {
            let (updated, input_tokens) = row.map_err(db_error)?;
            let old_band = if input_tokens > 272_000 {
                "above_272k"
            } else {
                "base"
            };
            let previous = updated.clone().with_context_band(old_band);
            apply_aggregate_delta(&transaction, &previous, -1)?;
            apply_aggregate_delta(&transaction, &updated, 1)?;
        }
    }
    transaction
        .pragma_update(None, "user_version", 32)
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)
}
