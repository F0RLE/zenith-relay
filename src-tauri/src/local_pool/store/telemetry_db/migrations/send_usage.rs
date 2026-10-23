use super::super::{
    db_error,
    record::{apply_aggregate_delta, UsageAggregate},
    Result,
};
use rusqlite::{Connection, TransactionBehavior};

/// v35 rollups include only the last send of each request. Add the retained
/// earlier sends once, without rebuilding or guessing already archived usage.
pub(in crate::local_pool::store::telemetry_db) fn migrate_036(
    connection: &mut Connection,
) -> Result<()> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    transaction
        .execute_batch(
            "UPDATE request_attempts SET usage_aggregate_recorded = COALESCE((
            SELECT usage_aggregate_recorded FROM request_logs
            WHERE request_logs.request_id = request_attempts.request_id
                AND request_logs.attempt = request_attempts.attempt
        ), 0);
         CREATE VIEW usage_sends AS
             SELECT * FROM request_attempts
             UNION ALL
             SELECT * FROM request_logs WHERE NOT EXISTS (
                 SELECT 1 FROM request_attempts
                 WHERE request_attempts.request_id = request_logs.request_id
                     AND request_attempts.attempt = request_logs.attempt
             );",
        )
        .map_err(db_error)?;
    {
        let mut statement = transaction
            .prepare(
                "SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
                COALESCE(account_id, source_id), COALESCE(resolved_model, requested_model, ''),
                input_tokens, cached_input_tokens, cache_write_input_tokens, cache_write_ttl,
                output_tokens, total_tokens, applied_service_tier
             FROM usage_sends WHERE usage_aggregate_recorded = 0",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| UsageAggregate::from_row_current(row, 0))
            .map_err(db_error)?;
        for row in rows {
            apply_aggregate_delta(&transaction, &row.map_err(db_error)?, 1)?;
        }
    }
    transaction.execute_batch(
        "UPDATE request_attempts SET usage_aggregate_recorded = 1;
         UPDATE request_logs SET usage_aggregate_recorded = 1;
         CREATE INDEX request_attempts_created_at ON request_attempts(created_at);
         CREATE INDEX request_attempts_account_created ON request_attempts(account_id, created_at);"
    ).map_err(db_error)?;
    transaction
        .pragma_update(None, "user_version", 36)
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)
}
