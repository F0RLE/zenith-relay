use super::super::{
    Result, db_error,
    record::{UsageAggregate, apply_aggregate_delta},
};
use rusqlite::{Connection, TransactionBehavior};

/// Move retained, explicitly unknown tiers out of their legacy Standard bucket.
/// Archived aggregates have lost that evidence; preserve their classification.
pub(in crate::local_pool::store::telemetry_db) fn migrate_035(
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
                    AND lower(trim(COALESCE(applied_service_tier, ''))) NOT IN
                        ('', 'standard', 'default', 'flex', 'priority', 'fast', 'ultrafast')",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| UsageAggregate::from_row_current(row, 0))
            .map_err(db_error)?;
        for row in rows {
            let updated = row.map_err(db_error)?;
            let previous = updated.clone().with_price_class("standard");
            apply_aggregate_delta(&transaction, &previous, -1)?;
            apply_aggregate_delta(&transaction, &updated, 1)?;
        }
    }
    transaction
        .pragma_update(None, "user_version", 35)
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)
}
