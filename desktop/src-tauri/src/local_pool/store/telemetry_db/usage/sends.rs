/// Match the physical-send view without carrying diagnostic JSON through an
/// aggregate, DISTINCT or page sort. Filters are pushed into both branches;
/// their positional parameters must therefore be bound twice, in branch order.
pub(in crate::local_pool::store::telemetry_db) fn send_projection(
    columns: &str,
    where_sql: &str,
) -> String {
    let conjunction = if where_sql.is_empty() {
        " WHERE "
    } else {
        " AND "
    };
    format!(
        "SELECT {columns} FROM request_attempts{where_sql}
         UNION ALL SELECT {columns} FROM request_logs{where_sql}{conjunction}{LEGACY_SEND_PREDICATE}"
    )
}

pub(in crate::local_pool::store::telemetry_db) const LEGACY_SEND_PREDICATE: &str =
    "NOT EXISTS (SELECT 1 FROM request_attempts
     WHERE request_attempts.request_id = request_logs.request_id
         AND request_attempts.attempt = request_logs.attempt)";
