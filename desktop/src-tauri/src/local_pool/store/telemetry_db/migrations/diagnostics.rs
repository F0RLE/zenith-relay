use super::super::{Result, db_error};
use rusqlite::{Connection, TransactionBehavior};

/// Install the dictionary without rewriting retained history at startup.
pub(in crate::local_pool::store::telemetry_db) fn migrate_046(
    connection: &mut Connection,
) -> Result<()> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_diagnostics (
            id INTEGER PRIMARY KEY,
            fingerprint BLOB NOT NULL UNIQUE,
            data_json TEXT NOT NULL,
            refs INTEGER NOT NULL DEFAULT 0 CHECK(refs >= 0)
        );",
        )
        .map_err(db_error)?;
    for table in ["request_logs", "request_attempts"] {
        for (column, definition) in [
            ("routing_diagnostics_id", "INTEGER"),
            ("tool_diagnostics_id", "INTEGER"),
            ("diagnostics_pruned", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            let exists: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
                    [table, column],
                    |row| row.get(0),
                )
                .map_err(db_error)?;
            if !exists {
                transaction
                    .execute_batch(&format!(
                        "ALTER TABLE {table} ADD COLUMN {column} {definition}"
                    ))
                    .map_err(db_error)?;
            }
        }
        transaction
            .execute_batch(&format!(
                "CREATE INDEX IF NOT EXISTS {table}_inline_diagnostics ON {table}(id)
                WHERE routing_json IS NOT NULL OR tool_use_json IS NOT NULL;
             CREATE INDEX IF NOT EXISTS {table}_diagnostic_expiry ON {table}(created_at, id)
                WHERE diagnostics_pruned = 0
                    AND (routing_diagnostics_id IS NOT NULL OR tool_diagnostics_id IS NOT NULL);
             CREATE TRIGGER IF NOT EXISTS {table}_diagnostics_insert AFTER INSERT ON {table}
             BEGIN
                UPDATE usage_diagnostics SET refs = refs + 1
                WHERE id IN (NEW.routing_diagnostics_id, NEW.tool_diagnostics_id);
             END;
             CREATE TRIGGER IF NOT EXISTS {table}_diagnostics_update
             AFTER UPDATE OF routing_diagnostics_id, tool_diagnostics_id ON {table}
             WHEN OLD.routing_diagnostics_id IS NOT NEW.routing_diagnostics_id
                OR OLD.tool_diagnostics_id IS NOT NEW.tool_diagnostics_id
             BEGIN
                UPDATE usage_diagnostics SET refs = refs + 1
                WHERE id IN (NEW.routing_diagnostics_id, NEW.tool_diagnostics_id);
                UPDATE usage_diagnostics SET refs = refs - 1
                WHERE id IN (OLD.routing_diagnostics_id, OLD.tool_diagnostics_id);
                DELETE FROM usage_diagnostics WHERE refs = 0
                    AND id IN (OLD.routing_diagnostics_id, OLD.tool_diagnostics_id);
             END;
             CREATE TRIGGER IF NOT EXISTS {table}_diagnostics_delete AFTER DELETE ON {table}
             BEGIN
                UPDATE usage_diagnostics SET refs = refs - 1
                WHERE id IN (OLD.routing_diagnostics_id, OLD.tool_diagnostics_id);
                DELETE FROM usage_diagnostics WHERE refs = 0
                    AND id IN (OLD.routing_diagnostics_id, OLD.tool_diagnostics_id);
             END;"
            ))
            .map_err(db_error)?;
    }
    transaction
        .pragma_update(None, "user_version", 46)
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)
}
