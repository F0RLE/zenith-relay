use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::{path::Path, sync::Mutex};
use zenith_relay_core::UsageEvent;

const MIGRATION_001: &str = r#"
CREATE TABLE IF NOT EXISTS request_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    request_id TEXT NOT NULL,
    local_key_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    requested_model TEXT,
    resolved_model TEXT,
    wire_api TEXT NOT NULL,
    success INTEGER NOT NULL,
    http_status INTEGER NOT NULL,
    error_category TEXT,
    latency_ms INTEGER NOT NULL,
    ttft_ms INTEGER,
    input_tokens INTEGER,
    output_tokens INTEGER,
    total_tokens INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS request_logs_request_id_idx ON request_logs(request_id);
CREATE INDEX IF NOT EXISTS request_logs_created_at_idx ON request_logs(created_at);
CREATE INDEX IF NOT EXISTS request_logs_source_created_idx ON request_logs(source_id, created_at);
CREATE INDEX IF NOT EXISTS request_logs_key_created_idx ON request_logs(local_key_id, created_at);
PRAGMA user_version = 1;
"#;

const MIGRATION_002: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE request_logs ADD COLUMN attempt INTEGER NOT NULL DEFAULT 1;
DROP INDEX IF EXISTS request_logs_request_id_idx;
CREATE UNIQUE INDEX request_logs_request_attempt_idx ON request_logs(request_id, attempt);
PRAGMA user_version = 2;
COMMIT;
"#;

pub struct TelemetryDb {
    connection: Mutex<Connection>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLog {
    pub id: i64,
    pub created_at: String,
    pub request_id: String,
    pub attempt: u16,
    pub local_key_id: String,
    pub source_id: String,
    pub requested_model: Option<String>,
    pub resolved_model: Option<String>,
    pub wire_api: String,
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<String>,
    pub latency_ms: u64,
    pub ttft_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

impl TelemetryDb {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        let connection = Connection::open(path).map_err(db_error)?;
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(db_error)?;
        if version > 2 {
            return Err(LocalPoolError::new(
                ErrorCode::UnsupportedSchema,
                format!("usage database schema {version} is newer than supported schema 2"),
            ));
        }
        if version == 0 {
            connection.execute_batch(MIGRATION_001).map_err(db_error)?;
        }
        if version <= 1 {
            connection.execute_batch(MIGRATION_002).map_err(db_error)?;
        }
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn record(&self, event: &UsageEvent) -> Result<()> {
        if event.attempt == 0 {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "usage attempt must be at least one",
            ));
        }
        self.connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?
            .execute(
                "INSERT OR IGNORE INTO request_logs (
                    request_id, attempt, local_key_id, source_id, requested_model, resolved_model,
                    wire_api, success, http_status, error_category, latency_ms, ttft_ms,
                    input_tokens, output_tokens, total_tokens
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    event.request_id,
                    event.attempt,
                    event.local_key_id,
                    event.source_id,
                    event.requested_model,
                    event.resolved_model,
                    format!("{:?}", event.wire_api).to_lowercase(),
                    event.success,
                    event.http_status,
                    event.error_category,
                    sql_u64(event.latency_ms),
                    event.ttft_ms.map(sql_u64),
                    event.input_tokens.map(sql_u64),
                    event.output_tokens.map(sql_u64),
                    event.total_tokens.map(sql_u64),
                ],
            )
            .map(|_| ())
            .map_err(db_error)
    }

    pub fn list(&self, limit: u16) -> Result<Vec<UsageLog>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
        let mut statement = connection
            .prepare(
                "SELECT id, strftime('%Y-%m-%dT%H:%M:%SZ', created_at), request_id, attempt, local_key_id, source_id, requested_model,
                    resolved_model, wire_api, success, http_status, error_category, latency_ms,
                    ttft_ms, input_tokens, output_tokens, total_tokens
                 FROM request_logs ORDER BY id DESC LIMIT ?1",
            )
            .map_err(db_error)?;
        let logs = statement
            .query_map([limit.clamp(1, 500)], |row| {
                let latency_ms: i64 = row.get(12)?;
                let ttft_ms: Option<i64> = row.get(13)?;
                let input_tokens: Option<i64> = row.get(14)?;
                let output_tokens: Option<i64> = row.get(15)?;
                let total_tokens: Option<i64> = row.get(16)?;
                Ok(UsageLog {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    request_id: row.get(2)?,
                    attempt: row.get(3)?,
                    local_key_id: row.get(4)?,
                    source_id: row.get(5)?,
                    requested_model: row.get(6)?,
                    resolved_model: row.get(7)?,
                    wire_api: row.get(8)?,
                    success: row.get(9)?,
                    http_status: row.get(10)?,
                    error_category: row.get(11)?,
                    latency_ms: rust_u64(latency_ms),
                    ttft_ms: ttft_ms.map(rust_u64),
                    input_tokens: input_tokens.map(rust_u64),
                    output_tokens: output_tokens.map(rust_u64),
                    total_tokens: total_tokens.map(rust_u64),
                })
            })
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(logs)
    }
}

fn sql_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn rust_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn db_error(error: rusqlite::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, format!("usage database error: {error}"))
}

fn io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::WireApi;

    #[test]
    fn usage_survives_database_reopen() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-usage-{}", uuid::Uuid::new_v4()));
        let path = root.join("usage.sqlite");
        let event = UsageEvent {
            request_id: "req_1".into(),
            attempt: 1,
            local_key_id: "key_1".into(),
            source_id: "source_1".into(),
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            wire_api: WireApi::Responses,
            success: true,
            http_status: 200,
            error_category: None,
            latency_ms: 12,
            ttft_ms: None,
            input_tokens: Some(2),
            output_tokens: Some(3),
            total_tokens: Some(5),
        };
        TelemetryDb::open(&path).unwrap().record(&event).unwrap();
        let logs = TelemetryDb::open(&path).unwrap().list(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].created_at.ends_with('Z'));
        assert_eq!(logs[0].total_tokens, Some(5));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_keeps_each_fallback_attempt() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-attempts-{}",
            uuid::Uuid::new_v4()
        ));
        let path = root.join("usage.sqlite");
        let database = TelemetryDb::open(&path).unwrap();
        let mut event = UsageEvent {
            request_id: "req_fallback".into(),
            attempt: 1,
            local_key_id: "key_1".into(),
            source_id: "source_1".into(),
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            wire_api: WireApi::Responses,
            success: false,
            http_status: 503,
            error_category: Some("upstream_unavailable".into()),
            latency_ms: 5,
            ttft_ms: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
        };
        database.record(&event).unwrap();
        event.attempt = 2;
        event.source_id = "source_2".into();
        event.success = true;
        event.http_status = 200;
        event.error_category = None;
        database.record(&event).unwrap();
        let logs = database.list(10).unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].attempt, 2);
        assert_eq!(logs[1].attempt, 1);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_v1_migrates_existing_rows_to_attempt_one() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-usage-v1-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("usage.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(MIGRATION_001).unwrap();
        connection
            .execute(
                "INSERT INTO request_logs (
                    request_id, local_key_id, source_id, wire_api, success, http_status, latency_ms
                ) VALUES ('req_old', 'key_1', 'source_1', 'responses', 1, 200, 3)",
                [],
            )
            .unwrap();
        drop(connection);

        let database = TelemetryDb::open(&path).unwrap();
        let logs = database.list(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].attempt, 1);
        let version: u32 = database
            .connection
            .lock()
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_schema_has_no_secret_or_body_columns() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-schema-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        let connection = database.connection.lock().unwrap();
        let mut statement = connection
            .prepare("SELECT name FROM pragma_table_info('request_logs')")
            .unwrap();
        let columns = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert!(!columns.iter().any(|column| {
            let column = column.to_lowercase();
            column.contains("secret")
                || column.contains("prompt")
                || column.contains("request_body")
                || column.contains("response_body")
        }));
        drop(statement);
        drop(connection);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn newer_usage_schema_is_rejected_without_rewrite() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-future-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("usage.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection.pragma_update(None, "user_version", 3).unwrap();
        drop(connection);

        assert!(matches!(
            TelemetryDb::open(&path).err().unwrap().code,
            ErrorCode::UnsupportedSchema
        ));
        let version: u32 = Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 3);
        std::fs::remove_dir_all(root).unwrap();
    }
}
