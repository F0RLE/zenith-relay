use super::super::*;

pub(in crate::local_pool::store::telemetry_db) fn migrate_047(
    connection: &mut Connection,
) -> Result<()> {
    let transaction = connection.transaction().map_err(db_error)?;
    let exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='usage_image_price_rollups')", [], |row| row.get(0)
    ).map_err(db_error)?;
    if !exists {
        transaction.execute_batch(
            "CREATE TABLE usage_image_price_rollups (
             candidate_kind TEXT NOT NULL,candidate_id TEXT NOT NULL,model TEXT NOT NULL,
             component TEXT NOT NULL,price_model TEXT NOT NULL,price_class TEXT NOT NULL,context_band TEXT NOT NULL,
             tokens INTEGER NOT NULL,images INTEGER NOT NULL,
             PRIMARY KEY(candidate_kind,candidate_id,model,component,price_model,price_class,context_band));"
        ).map_err(db_error)?;
        let mut statement = transaction.prepare(
            "SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
             COALESCE(account_id,source_id),COALESCE(resolved_model,requested_model,''),
             input_tokens,cached_input_tokens,cache_write_input_tokens,cache_write_ttl,
             output_tokens,total_tokens,applied_service_tier,image_usage_json,usage_aggregate_recorded
             FROM usage_sends WHERE image_usage_json IS NOT NULL"
        ).map_err(db_error)?;
        let mut rows = statement.query([]).map_err(db_error)?;
        while let Some(row) = rows.next().map_err(db_error)? {
            let aggregate = super::super::record::UsageAggregate::from_row_current(row, 0)
                .map_err(db_error)?
                .with_image(
                    row.get::<_, Option<String>>(10)
                        .map_err(db_error)?
                        .and_then(|json| serde_json::from_str(&json).ok()),
                );
            if row.get::<_, bool>(11).map_err(db_error)? {
                aggregate.apply_image_delta(&transaction, 1)?;
            } else {
                super::super::record::apply_aggregate_delta(&transaction, &aggregate, 1)?;
            }
        }
        drop(rows);
        drop(statement);
        transaction.execute_batch(
            "UPDATE request_logs SET usage_aggregate_recorded=1 WHERE image_usage_json IS NOT NULL;
             UPDATE request_attempts SET usage_aggregate_recorded=1 WHERE image_usage_json IS NOT NULL;"
        ).map_err(db_error)?;
    }
    transaction
        .execute_batch("PRAGMA user_version=47;")
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)
}
