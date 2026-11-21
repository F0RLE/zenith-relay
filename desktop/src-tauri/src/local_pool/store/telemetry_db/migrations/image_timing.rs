use super::super::*;

/// Keep modality in the compact report index instead of reading image JSON
/// for every timing aggregate. The writer derives the flag from the same JSON.
pub(in crate::local_pool::store::telemetry_db) fn migrate_048(
    connection: &mut Connection,
) -> Result<()> {
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    for table in ["request_logs", "request_attempts"] {
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name='is_image_usage')",
                [table],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if !exists {
            transaction
                .execute_batch(&format!(
                    "ALTER TABLE {table} ADD COLUMN is_image_usage INTEGER NOT NULL DEFAULT 0
                     CHECK(is_image_usage IN (0, 1));"
                ))
                .map_err(db_error)?;
        }
        transaction
            .execute_batch(&format!(
                "UPDATE {table} SET is_image_usage=(image_usage_json IS NOT NULL)
                    WHERE is_image_usage != (image_usage_json IS NOT NULL);
                 DROP INDEX IF EXISTS {table}_usage_reads;
                 CREATE INDEX {table}_usage_reads ON {table}(
                    created_at DESC, ABS(id) DESC, request_id, attempt,
                    wire_api, transport, error_category, requested_model, resolved_model,
                    account_id, source_id, success, latency_ms, ttft_ms, generation_ms,
                    input_tokens, cached_input_tokens, cache_write_input_tokens,
                    reasoning_tokens, output_tokens, total_tokens,
                    applied_service_tier, cache_write_ttl, is_image_usage
                 );"
            ))
            .map_err(db_error)?;
    }
    transaction
        .pragma_update(None, "user_version", 48)
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)
}
