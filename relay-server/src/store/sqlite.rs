use crate::state::{GatewayKeyRecord, ServerAccountRecord, SourceRecord, SERVER_SCHEMA_VERSION};
use fs2::FileExt;
use rusqlite::{
    params, params_from_iter, types::Value as SqlValue, Connection, OpenFlags, OptionalExtension,
    TransactionBehavior,
};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};
use zenith_relay_core::{
    automations::{WakeAutomationState, WakeTask},
    estimate_api_equivalent,
    protocol::{UsagePage, UsageQuery, UsageSummary},
    ApiEquivalentSummary, UsageEvent, WireApi,
};

pub const DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS: u64 = 300;
pub const DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS: u64 = 20;
pub const MIN_QUOTA_REFRESH_INTERVAL_SECONDS: u64 = 120;
pub const MAX_QUOTA_REFRESH_INTERVAL_SECONDS: u64 = 3_600;
pub const MIN_QUOTA_REQUEST_TIMEOUT_SECONDS: u64 = 10;
pub const MAX_QUOTA_REQUEST_TIMEOUT_SECONDS: u64 = 20;
pub const DEFAULT_MAX_RETRY_CANDIDATES: u8 = 3;
pub const DEFAULT_SESSION_AFFINITY_TTL_SECONDS: u64 = 3_600;
pub const MIN_SESSION_AFFINITY_TTL_SECONDS: u64 = 60;
pub const MAX_SESSION_AFFINITY_TTL_SECONDS: u64 = 86_400;

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "001_init",
        sql: include_str!("../../migrations/001_init.sql"),
    },
    Migration {
        version: 2,
        name: "002_migration_ledger",
        sql: include_str!("../../migrations/002_migration_ledger.sql"),
    },
    Migration {
        version: 3,
        name: "003_usage_query_indexes",
        sql: include_str!("../../migrations/003_usage_query_indexes.sql"),
    },
    Migration {
        version: 4,
        name: "004_account_proxies",
        sql: include_str!("../../migrations/004_account_proxies.sql"),
    },
    Migration {
        version: 5,
        name: "005_pool_membership",
        sql: include_str!("../../migrations/005_pool_membership.sql"),
    },
    Migration {
        version: 6,
        name: "006_model_rules",
        sql: include_str!("../../migrations/006_model_rules.sql"),
    },
    Migration {
        version: 7,
        name: "007_cached_input_tokens",
        sql: include_str!("../../migrations/007_cached_input_tokens.sql"),
    },
    Migration {
        version: 8,
        name: "008_reasoning_tokens",
        sql: include_str!("../../migrations/008_reasoning_tokens.sql"),
    },
    Migration {
        version: 9,
        name: "009_ttft_ms",
        sql: include_str!("../../migrations/009_ttft_ms.sql"),
    },
    Migration {
        version: 10,
        name: "010_reset_legacy_cooldowns",
        sql: include_str!("../../migrations/010_reset_legacy_cooldowns.sql"),
    },
    Migration {
        version: 11,
        name: "011_request_rotation_default",
        sql: include_str!("../../migrations/011_request_rotation_default.sql"),
    },
];

struct Migration {
    version: u32,
    name: &'static str,
    sql: &'static str,
}

#[derive(Clone, Debug)]
pub struct PendingImport {
    pub id: String,
    pub preview_json: String,
    pub secret_ref: String,
    pub created_at_ms: u64,
}

pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let migration_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(sibling_path(&path, ".migration.lock"))
            .map_err(io_error)?;
        migration_lock
            .try_lock_exclusive()
            .map_err(|_| "database migration is already running".to_string())?;
        recover_interrupted_migration(&path)?;
        let database_existed = path.metadata().is_ok_and(|metadata| metadata.len() > 0);
        let mut connection = Connection::open(&path).map_err(db_error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(db_error)?;
        let current_version = read_schema_version(&connection)?;
        if current_version > SERVER_SCHEMA_VERSION {
            return Err(format!(
                "database schema version {current_version} is newer than supported version {SERVER_SCHEMA_VERSION}"
            ));
        }
        if current_version < SERVER_SCHEMA_VERSION {
            if database_existed {
                prepare_migration_backup(
                    &connection,
                    &path,
                    current_version,
                    SERVER_SCHEMA_VERSION,
                )?;
            }
            apply_migrations(&mut connection, current_version)?;
            if database_existed {
                finish_migration(&path)?;
            }
        }
        validate_migration_ledger(&connection)?;
        drop(migration_lock);
        connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .map_err(db_error)?;
        let store = Self {
            connection: Mutex::new(connection),
        };
        let _ = store.server_id()?;
        Ok(store)
    }

    pub fn server_id(&self) -> Result<String, String> {
        if let Some(value) = self.metadata("server_id")? {
            return Ok(value);
        }
        let value = uuid::Uuid::new_v4().to_string();
        self.set_metadata("server_id", &value)?;
        Ok(value)
    }

    pub fn gateway_enabled(&self) -> Result<bool, String> {
        Ok(self
            .metadata("gateway_enabled")?
            .is_none_or(|value| value == "true"))
    }

    pub fn set_gateway_enabled(&self, enabled: bool) -> Result<(), String> {
        self.set_metadata("gateway_enabled", if enabled { "true" } else { "false" })
    }

    pub fn common_proxy_configured(&self) -> Result<bool, String> {
        Ok(self
            .metadata("common_proxy_configured")?
            .is_some_and(|value| value == "true"))
    }

    pub fn set_common_proxy_configured(&self, configured: bool) -> Result<(), String> {
        self.set_metadata(
            "common_proxy_configured",
            if configured { "true" } else { "false" },
        )
    }

    pub fn account_proxy_required(&self) -> Result<bool, String> {
        Ok(self
            .metadata("account_proxy_required")?
            .is_some_and(|value| value == "true"))
    }

    pub fn set_account_proxy_required(&self, required: bool) -> Result<(), String> {
        self.set_metadata(
            "account_proxy_required",
            if required { "true" } else { "false" },
        )
    }

    pub fn quota_policy(&self) -> Result<(u64, u64, bool), String> {
        let refresh = self.metadata("quota_refresh_interval_seconds")?.map_or(
            Ok(DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS),
            |value| {
                value
                    .parse::<u64>()
                    .map_err(|_| "quota refresh interval is invalid".to_string())
            },
        )?;
        let timeout = self.metadata("quota_request_timeout_seconds")?.map_or(
            Ok(DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS),
            |value| {
                value
                    .parse::<u64>()
                    .map_err(|_| "quota request timeout is invalid".to_string())
            },
        )?;
        validate_quota_policy(refresh, timeout)?;
        let use_free_accounts = self
            .metadata("use_free_accounts")?
            .is_some_and(|value| value == "true");
        Ok((refresh, timeout, use_free_accounts))
    }

    pub fn set_quota_policy(
        &self,
        refresh_interval_seconds: u64,
        request_timeout_seconds: u64,
        use_free_accounts: bool,
    ) -> Result<(), String> {
        validate_quota_policy(refresh_interval_seconds, request_timeout_seconds)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        for (key, value) in [
            (
                "quota_refresh_interval_seconds",
                refresh_interval_seconds.to_string(),
            ),
            (
                "quota_request_timeout_seconds",
                request_timeout_seconds.to_string(),
            ),
            ("use_free_accounts", use_free_accounts.to_string()),
        ] {
            transaction
                .execute(
                    "INSERT INTO metadata(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![key, value],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)
    }

    pub fn routing_policy(&self) -> Result<(u8, bool, u64), String> {
        let max_retry_candidates = self.metadata("max_retry_candidates")?.map_or(
            Ok(DEFAULT_MAX_RETRY_CANDIDATES),
            |value| {
                value
                    .parse::<u8>()
                    .map_err(|_| "max retry candidates is invalid".to_string())
            },
        )?;
        let session_affinity = self
            .metadata("session_affinity")?
            .is_some_and(|value| value == "true");
        let session_affinity_ttl_seconds = self.metadata("session_affinity_ttl_seconds")?.map_or(
            Ok(DEFAULT_SESSION_AFFINITY_TTL_SECONDS),
            |value| {
                value
                    .parse::<u64>()
                    .map_err(|_| "session affinity TTL is invalid".to_string())
            },
        )?;
        validate_routing_policy(max_retry_candidates, session_affinity_ttl_seconds)?;
        Ok((
            max_retry_candidates,
            session_affinity,
            session_affinity_ttl_seconds,
        ))
    }

    pub fn set_routing_policy(
        &self,
        max_retry_candidates: u8,
        session_affinity: bool,
        session_affinity_ttl_seconds: u64,
    ) -> Result<(), String> {
        validate_routing_policy(max_retry_candidates, session_affinity_ttl_seconds)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        for (key, value) in [
            ("max_retry_candidates", max_retry_candidates.to_string()),
            ("session_affinity", session_affinity.to_string()),
            (
                "session_affinity_ttl_seconds",
                session_affinity_ttl_seconds.to_string(),
            ),
        ] {
            transaction
                .execute(
                    "INSERT INTO metadata(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![key, value],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)
    }

    pub fn hidden_models(&self) -> Result<Vec<String>, String> {
        let value = self
            .metadata("hidden_model_ids")?
            .unwrap_or_else(|| "[]".to_string());
        normalize_model_ids(
            serde_json::from_str(&value).map_err(|_| "hidden model list is invalid".to_string())?,
        )
    }

    pub fn set_hidden_models(&self, models: Vec<String>) -> Result<(), String> {
        let models = normalize_model_ids(models)?;
        self.set_metadata(
            "hidden_model_ids",
            &serde_json::to_string(&models)
                .map_err(|_| "hidden model list serialization failed".to_string())?,
        )
    }

    pub fn sources(&self) -> Result<Vec<SourceRecord>, String> {
        self.list_records("sources")
    }

    pub fn save_source(&self, record: &SourceRecord) -> Result<(), String> {
        self.save_record("sources", &record.id, &record.secret_ref, record)
    }

    pub fn delete_source(&self, id: &str) -> Result<Option<SourceRecord>, String> {
        self.delete_record("sources", id)
    }

    pub fn accounts(&self) -> Result<Vec<ServerAccountRecord>, String> {
        self.list_records("accounts")
    }

    pub fn account(&self, id: &str) -> Result<Option<ServerAccountRecord>, String> {
        self.lock()?
            .query_row(
                "SELECT data_json FROM accounts WHERE id = ?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error)?
            .map(|value| parse_json(&value))
            .transpose()
    }

    pub fn save_account(&self, record: &ServerAccountRecord) -> Result<(), String> {
        self.save_record("accounts", &record.id, &record.secret_ref, record)
    }

    pub fn save_accounts(&self, records: &[ServerAccountRecord]) -> Result<(), String> {
        let encoded = records
            .iter()
            .map(|record| Ok((record, to_json(record)?)))
            .collect::<Result<Vec<_>, String>>()?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        {
            let mut statement = transaction
                .prepare(
                    "INSERT INTO accounts(id, data_json, secret_ref) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data_json=excluded.data_json, secret_ref=excluded.secret_ref",
                )
                .map_err(db_error)?;
            for (record, data_json) in encoded {
                statement
                    .execute(params![record.id, data_json, record.secret_ref])
                    .map_err(db_error)?;
            }
        }
        transaction.commit().map_err(db_error)
    }

    pub fn delete_account(&self, id: &str) -> Result<Option<ServerAccountRecord>, String> {
        self.delete_record("accounts", id)
    }

    pub fn replace_pool_membership(
        &self,
        sources: &[(String, bool)],
        accounts: &[(String, bool)],
    ) -> Result<(), String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        for (id, in_pool) in sources {
            let changed = transaction
                .execute(
                    "UPDATE sources SET data_json = json_set(data_json, '$.inPool', json(?1)) WHERE id = ?2",
                    params![if *in_pool { "true" } else { "false" }, id],
                )
                .map_err(db_error)?;
            if changed != 1 {
                return Err("pool source not found".to_string());
            }
        }
        for (id, in_pool) in accounts {
            let changed = transaction
                .execute(
                    "UPDATE accounts SET data_json = json_set(data_json, '$.inPool', json(?1)) WHERE id = ?2",
                    params![if *in_pool { "true" } else { "false" }, id],
                )
                .map_err(db_error)?;
            if changed != 1 {
                return Err("pool account not found".to_string());
            }
        }
        transaction.commit().map_err(db_error)
    }

    pub fn keys(&self) -> Result<Vec<GatewayKeyRecord>, String> {
        self.list_records("gateway_keys")
    }

    pub fn save_key(&self, record: &GatewayKeyRecord) -> Result<(), String> {
        self.save_record("gateway_keys", &record.id, &record.secret_ref, record)
    }

    pub fn delete_key(&self, id: &str) -> Result<Option<GatewayKeyRecord>, String> {
        self.delete_record("gateway_keys", id)
    }

    pub fn save_pending_import(&self, import: &PendingImport) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT INTO pending_imports(id, preview_json, secret_ref, created_at_ms) VALUES (?1, ?2, ?3, ?4)\
                 ON CONFLICT(id) DO UPDATE SET preview_json=excluded.preview_json, secret_ref=excluded.secret_ref, created_at_ms=excluded.created_at_ms",
                params![
                    import.id,
                    import.preview_json,
                    import.secret_ref,
                    import.created_at_ms.min(i64::MAX as u64) as i64
                ],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn pending_import(&self, id: &str) -> Result<Option<PendingImport>, String> {
        self.lock()?
            .query_row(
                "SELECT id, preview_json, secret_ref, created_at_ms FROM pending_imports WHERE id = ?1",
                [id],
                |row| {
                    Ok(PendingImport {
                        id: row.get(0)?,
                        preview_json: row.get(1)?,
                        secret_ref: row.get(2)?,
                        created_at_ms: row.get::<_, i64>(3)?.max(0) as u64,
                    })
                },
            )
            .optional()
            .map_err(db_error)
    }

    pub fn delete_pending_import(&self, id: &str) -> Result<bool, String> {
        Ok(self
            .lock()?
            .execute("DELETE FROM pending_imports WHERE id = ?1", [id])
            .map_err(db_error)?
            > 0)
    }

    pub fn delete_pending_imports_before(&self, cutoff_ms: u64) -> Result<Vec<String>, String> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(db_error)?;
        let secret_refs = {
            let mut statement = transaction
                .prepare("SELECT secret_ref FROM pending_imports WHERE created_at_ms < ?1")
                .map_err(db_error)?;
            let rows = statement
                .query_map([cutoff_ms.min(i64::MAX as u64) as i64], |row| row.get(0))
                .map_err(db_error)?
                .collect::<Result<Vec<String>, _>>()
                .map_err(db_error)?;
            rows
        };
        transaction
            .execute(
                "DELETE FROM pending_imports WHERE created_at_ms < ?1",
                [cutoff_ms.min(i64::MAX as u64) as i64],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(secret_refs)
    }

    pub fn wake_tasks(&self) -> Result<Vec<WakeTask>, String> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT data_json FROM wake_tasks ORDER BY id")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_error)?;
        rows.map(|row| parse_json(&row.map_err(db_error)?))
            .collect()
    }

    pub fn save_wake_task(&self, task: &WakeTask) -> Result<(), String> {
        let json = to_json(task)?;
        self.lock()?
            .execute(
                "INSERT INTO wake_tasks(id, data_json) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET data_json=excluded.data_json",
                params![task.id, json],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn delete_wake_task(&self, id: &str) -> Result<bool, String> {
        Ok(self
            .lock()?
            .execute("DELETE FROM wake_tasks WHERE id = ?1", [id])
            .map_err(db_error)?
            > 0)
    }

    pub fn wake_state(&self) -> Result<WakeAutomationState, String> {
        let json = self
            .lock()?
            .query_row(
                "SELECT data_json FROM wake_state WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error)?;
        match json {
            Some(value) => parse_json(&value),
            None => WakeAutomationState::new(1_024, 256).map_err(|error| error.to_string()),
        }
    }

    pub fn save_wake_state(&self, state: &WakeAutomationState) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT INTO wake_state(singleton, data_json) VALUES (1, ?1) ON CONFLICT(singleton) DO UPDATE SET data_json=excluded.data_json",
                [to_json(state)?],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn record_usage(&self, event: &UsageEvent, created_at_ms: u64) -> Result<(), String> {
        self.record_usage_batch(&[(event, created_at_ms)])
    }

    pub fn record_usage_batch(&self, events: &[(&UsageEvent, u64)]) -> Result<(), String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        {
            let mut statement = transaction
                .prepare(
                    "INSERT INTO usage_events(request_id, local_key_id, candidate_kind, candidate_hint, requested_model, resolved_model, wire_api, success, http_status, error_category, latency_ms, ttft_ms, input_tokens, cached_input_tokens, reasoning_tokens, output_tokens, total_tokens, created_at_ms)\
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                )
                .map_err(db_error)?;
            for (event, created_at_ms) in events {
                let candidate_id = event
                    .account_id
                    .as_deref()
                    .or(event.candidate_id.as_deref())
                    .unwrap_or(&event.source_id);
                let candidate_kind = if event.account_id.is_some() {
                    "account"
                } else {
                    "source"
                };
                let candidate_hint =
                    format!("{:x}", Sha256::digest(candidate_id.as_bytes()))[..12].to_string();
                statement
                    .execute(params![
                        event.request_id,
                        event.local_key_id,
                        candidate_kind,
                        candidate_hint,
                        event.requested_model,
                        event.resolved_model,
                        wire_api_name(event.wire_api),
                        i64::from(event.success),
                        i64::from(event.http_status),
                        event.error_category,
                        event.latency_ms as i64,
                        event.ttft_ms.map(|value| value as i64),
                        event.input_tokens.map(|value| value as i64),
                        event.cached_input_tokens.map(|value| value as i64),
                        event.reasoning_tokens.map(|value| value as i64),
                        event.output_tokens.map(|value| value as i64),
                        event.total_tokens.map(|value| value as i64),
                        *created_at_ms as i64,
                    ])
                    .map_err(db_error)?;
            }
        }
        transaction.commit().map_err(db_error)
    }

    pub fn usage_page(&self, query: &UsageQuery) -> Result<UsagePage, String> {
        let page = query.page.max(1);
        let page_size = if query.page_size == 0 {
            50
        } else {
            query.page_size.clamp(1, 200)
        };
        let connection = self.lock()?;
        let (where_sql, values) = usage_filter(query);
        let count_sql = format!("SELECT COUNT(*) FROM usage_events{where_sql}");
        let total = connection
            .query_row(&count_sql, params_from_iter(values.iter()), |row| {
                row.get::<_, i64>(0)
            })
            .map_err(db_error)?
            .max(0) as u64;
        let offset = u64::from(page.saturating_sub(1)) * u64::from(page_size);
        let sql = format!(
            "SELECT id, request_id, local_key_id, candidate_kind, candidate_hint, requested_model, resolved_model, wire_api, success, http_status, error_category, latency_ms, ttft_ms, input_tokens, cached_input_tokens, reasoning_tokens, output_tokens, total_tokens, created_at_ms \
             FROM usage_events{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?"
        );
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let mut page_values = values;
        page_values.push(SqlValue::Integer(i64::from(page_size)));
        page_values.push(SqlValue::Integer(offset.min(i64::MAX as u64) as i64));
        let rows = statement
            .query_map(params_from_iter(page_values.iter()), |row| {
                let wire_api: String = row.get(7)?;
                Ok(UsageSummary {
                    id: row.get(0)?,
                    request_id: row.get(1)?,
                    local_key_id: row.get(2)?,
                    candidate_kind: row.get(3)?,
                    candidate_hint: row.get(4)?,
                    candidate_label: None,
                    requested_model: row.get(5)?,
                    resolved_model: row.get(6)?,
                    wire_api: parse_wire_api(&wire_api),
                    success: row.get::<_, i64>(8)? != 0,
                    http_status: row.get::<_, i64>(9)?.clamp(0, i64::from(u16::MAX)) as u16,
                    error_category: row.get(10)?,
                    latency_ms: row.get::<_, i64>(11)?.max(0) as u64,
                    ttft_ms: optional_u64(row.get(12)?),
                    input_tokens: optional_u64(row.get(13)?),
                    cached_input_tokens: optional_u64(row.get(14)?),
                    reasoning_tokens: optional_u64(row.get(15)?),
                    output_tokens: optional_u64(row.get(16)?),
                    total_tokens: optional_u64(row.get(17)?),
                    created_at_ms: row.get::<_, i64>(18)?.max(0) as u64,
                })
            })
            .map_err(db_error)?;
        let events = rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?;
        let total_pages = if total == 0 {
            0
        } else {
            total.div_ceil(u64::from(page_size)) as u32
        };
        Ok(UsagePage {
            events,
            total,
            page,
            page_size,
            total_pages,
        })
    }

    pub fn api_equivalents(&self) -> Result<HashMap<String, ApiEquivalentSummary>, String> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT candidate_hint, COALESCE(resolved_model, requested_model),
                    SUM(input_tokens), SUM(cached_input_tokens), SUM(output_tokens), SUM(total_tokens)
                 FROM usage_events
                 GROUP BY candidate_hint, COALESCE(resolved_model, requested_model)",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                let input_tokens: Option<i64> = row.get(2)?;
                let cached_input_tokens: Option<i64> = row.get(3)?;
                let output_tokens: Option<i64> = row.get(4)?;
                let total_tokens: Option<i64> = row.get(5)?;
                Ok((
                    row.get::<_, String>(0)?,
                    estimate_api_equivalent(
                        row.get::<_, Option<String>>(1)?.as_deref(),
                        optional_u64(input_tokens),
                        optional_u64(cached_input_tokens),
                        optional_u64(output_tokens),
                        optional_u64(total_tokens),
                    ),
                ))
            })
            .map_err(db_error)?;
        let mut equivalents = HashMap::<String, ApiEquivalentSummary>::new();
        for row in rows {
            let (candidate_hint, estimate) = row.map_err(db_error)?;
            equivalents
                .entry(candidate_hint)
                .or_default()
                .merge(estimate);
        }
        Ok(equivalents)
    }

    pub fn clear_usage(&self) -> Result<usize, String> {
        self.lock()?
            .execute("DELETE FROM usage_events", [])
            .map_err(db_error)
    }

    pub fn backup_to(&self, destination: &Path) -> Result<(), String> {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        if destination.exists() {
            return Err("backup database destination already exists".to_string());
        }
        self.lock()?
            .backup(rusqlite::MAIN_DB, destination, None)
            .map_err(db_error)
    }

    fn metadata(&self, key: &str) -> Result<Option<String>, String> {
        self.lock()?
            .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(db_error)
    }

    fn set_metadata(&self, key: &str, value: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT INTO metadata(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )
            .map_err(db_error)?;
        Ok(())
    }

    fn list_records<T: DeserializeOwned>(&self, table: &str) -> Result<Vec<T>, String> {
        let sql = format!("SELECT data_json FROM {table} ORDER BY id");
        let connection = self.lock()?;
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_error)?;
        rows.map(|row| parse_json(&row.map_err(db_error)?))
            .collect()
    }

    fn save_record<T: Serialize>(
        &self,
        table: &str,
        id: &str,
        secret_ref: &str,
        value: &T,
    ) -> Result<(), String> {
        let sql = format!(
            "INSERT INTO {table}(id, data_json, secret_ref) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data_json=excluded.data_json, secret_ref=excluded.secret_ref"
        );
        self.lock()?
            .execute(&sql, params![id, to_json(value)?, secret_ref])
            .map_err(db_error)?;
        Ok(())
    }

    fn delete_record<T: DeserializeOwned>(
        &self,
        table: &str,
        id: &str,
    ) -> Result<Option<T>, String> {
        let sql = format!("SELECT data_json FROM {table} WHERE id = ?1");
        let delete = format!("DELETE FROM {table} WHERE id = ?1");
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(db_error)?;
        let json = transaction
            .query_row(&sql, [id], |row| row.get::<_, String>(0))
            .optional()
            .map_err(db_error)?;
        if json.is_some() {
            transaction.execute(&delete, [id]).map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        json.map(|value| parse_json(&value)).transpose()
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.connection
            .lock()
            .map_err(|_| "SQLite lock poisoned".to_string())
    }
}

fn usage_filter(query: &UsageQuery) -> (String, Vec<SqlValue>) {
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    if let Some(value) = query.from_ms {
        clauses.push("created_at_ms >= ?");
        values.push(SqlValue::Integer(value.min(i64::MAX as u64) as i64));
    }
    if let Some(value) = query.to_ms {
        clauses.push("created_at_ms <= ?");
        values.push(SqlValue::Integer(value.min(i64::MAX as u64) as i64));
    }
    if let Some(value) = query.model_query.as_deref() {
        clauses.push("(requested_model LIKE ? ESCAPE '\\' OR resolved_model LIKE ? ESCAPE '\\')");
        let value = SqlValue::Text(like_pattern(value));
        values.push(value.clone());
        values.push(value);
    }
    if let Some(value) = query.source_or_account_query.as_deref() {
        clauses.push("candidate_hint LIKE ? ESCAPE '\\'");
        values.push(SqlValue::Text(like_pattern(value)));
    }
    if let Some(value) = query.local_key_query.as_deref() {
        clauses.push("local_key_id LIKE ? ESCAPE '\\'");
        values.push(SqlValue::Text(like_pattern(value)));
    }
    if let Some(value) = query.wire_api {
        clauses.push("wire_api = ?");
        values.push(SqlValue::Text(wire_api_name(value).to_string()));
    }
    if let Some(value) = query.success {
        clauses.push("success = ?");
        values.push(SqlValue::Integer(i64::from(value)));
    }
    if let Some(value) = query.error_category.as_deref() {
        clauses.push("error_category = ?");
        values.push(SqlValue::Text(value.to_string()));
    }
    if let Some(value) = query.request_id_query.as_deref() {
        clauses.push("request_id LIKE ? ESCAPE '\\'");
        values.push(SqlValue::Text(like_pattern(value)));
    }
    let sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    (sql, values)
}

fn like_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('%');
    for character in value.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}

fn read_schema_version(connection: &Connection) -> Result<u32, String> {
    let has_metadata = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'metadata')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(db_error)?;
    if !has_metadata {
        return Ok(0);
    }
    let value = connection
        .query_row(
            "SELECT value FROM metadata WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?;
    value
        .as_deref()
        .unwrap_or("0")
        .parse::<u32>()
        .map_err(|_| "database schema version is invalid".to_string())
}

fn apply_migrations(connection: &mut Connection, current_version: u32) -> Result<(), String> {
    if MIGRATIONS.last().map(|migration| migration.version) != Some(SERVER_SCHEMA_VERSION)
        || !MIGRATIONS
            .iter()
            .enumerate()
            .all(|(index, migration)| migration.version == index as u32 + 1)
    {
        return Err("database migration registry is invalid".to_string());
    }
    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version > current_version)
    {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        transaction.execute_batch(migration.sql).map_err(db_error)?;
        if migration.version >= 2 {
            transaction
                .execute(
                    "INSERT INTO schema_migrations(version, name, applied_at_ms) VALUES (?1, ?2, ?3)",
                    params![
                        i64::from(migration.version),
                        migration.name,
                        unix_time_ms().min(i64::MAX as u64) as i64
                    ],
                )
                .map_err(db_error)?;
        }
        transaction
            .execute(
                "INSERT INTO metadata(key, value) VALUES ('schema_version', ?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [migration.version.to_string()],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
    }
    Ok(())
}

fn validate_migration_ledger(connection: &Connection) -> Result<(), String> {
    let mut statement = connection
        .prepare("SELECT version, name FROM schema_migrations ORDER BY version")
        .map_err(|_| "database migration ledger is missing".to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    let expected = MIGRATIONS
        .iter()
        .map(|migration| (migration.version, migration.name.to_string()))
        .collect::<Vec<_>>();
    if rows != expected {
        return Err("database migration ledger is invalid".to_string());
    }
    Ok(())
}

fn prepare_migration_backup(
    connection: &Connection,
    path: &Path,
    from_version: u32,
    to_version: u32,
) -> Result<(), String> {
    let backup_path = sibling_path(path, ".pre-migration");
    if backup_path.exists() {
        fs::remove_file(&backup_path).map_err(io_error)?;
    }
    connection
        .backup(rusqlite::MAIN_DB, &backup_path, None)
        .map_err(db_error)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&backup_path)
        .and_then(|file| file.sync_all())
        .map_err(io_error)?;
    if validate_database_file(&backup_path)? != from_version {
        return Err("pre-migration backup version is invalid".to_string());
    }

    let marker_path = sibling_path(path, ".migration-in-progress");
    let mut marker = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(marker_path)
        .map_err(io_error)?;
    writeln!(marker, "{from_version}:{to_version}").map_err(io_error)?;
    marker.sync_all().map_err(io_error)
}

fn finish_migration(path: &Path) -> Result<(), String> {
    let marker_path = sibling_path(path, ".migration-in-progress");
    if marker_path.exists() {
        fs::remove_file(marker_path).map_err(io_error)?;
    }
    Ok(())
}

fn recover_interrupted_migration(path: &Path) -> Result<(), String> {
    let marker_path = sibling_path(path, ".migration-in-progress");
    if !marker_path.exists() {
        return Ok(());
    }
    let backup_path = sibling_path(path, ".pre-migration");
    if !backup_path.is_file() {
        return Err("interrupted database migration backup is missing".to_string());
    }
    let marker = fs::read_to_string(&marker_path).map_err(io_error)?;
    let (from_version, to_version) = marker
        .trim()
        .split_once(':')
        .and_then(|(from, to)| Some((from.parse::<u32>().ok()?, to.parse::<u32>().ok()?)))
        .ok_or_else(|| "database migration marker is invalid".to_string())?;
    if to_version != SERVER_SCHEMA_VERSION
        || from_version >= to_version
        || validate_database_file(&backup_path)? != from_version
    {
        return Err("database migration recovery metadata is invalid".to_string());
    }
    restore_database_file(&backup_path, path)?;
    fs::remove_file(marker_path).map_err(io_error)
}

fn validate_database_file(path: &Path) -> Result<u32, String> {
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(db_error)?;
    let integrity = connection
        .query_row("PRAGMA integrity_check(1)", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(db_error)?;
    if integrity != "ok" {
        return Err("database backup integrity check failed".to_string());
    }
    let version = read_schema_version(&connection)?;
    if version > SERVER_SCHEMA_VERSION {
        return Err("database backup schema is newer than this server".to_string());
    }
    Ok(version)
}

fn restore_database_file(source: &Path, target: &Path) -> Result<(), String> {
    let temporary = sibling_path(target, ".migration-restore.tmp");
    let previous = sibling_path(target, ".failed-migration");
    for path in [&temporary, &previous] {
        if path.exists() {
            fs::remove_file(path).map_err(io_error)?;
        }
    }
    fs::copy(source, &temporary).map_err(io_error)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&temporary)
        .and_then(|file| file.sync_all())
        .map_err(io_error)?;
    for suffix in ["-wal", "-shm"] {
        let path = sibling_path(target, suffix);
        if path.exists() {
            fs::remove_file(path).map_err(io_error)?;
        }
    }
    if target.exists() {
        fs::rename(target, &previous).map_err(io_error)?;
    }
    if let Err(error) = fs::rename(&temporary, target) {
        if previous.exists() {
            let _ = fs::rename(&previous, target);
        }
        return Err(io_error(error));
    }
    if previous.exists() {
        fs::remove_file(previous).map_err(io_error)?;
    }
    Ok(())
}

fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(suffix);
    PathBuf::from(value)
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

fn to_json(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_string(value).map_err(|_| "record serialization failed".to_string())
}

fn parse_json<T: DeserializeOwned>(value: &str) -> Result<T, String> {
    serde_json::from_str(value).map_err(|_| "stored record is invalid".to_string())
}

fn optional_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

fn wire_api_name(value: WireApi) -> &'static str {
    match value {
        WireApi::Responses => "responses",
        WireApi::ChatCompletions => "chat_completions",
        WireApi::Messages => "messages",
    }
}

fn parse_wire_api(value: &str) -> WireApi {
    match value {
        "chat_completions" => WireApi::ChatCompletions,
        "messages" => WireApi::Messages,
        _ => WireApi::Responses,
    }
}

fn validate_quota_policy(
    refresh_interval_seconds: u64,
    request_timeout_seconds: u64,
) -> Result<(), String> {
    if !(MIN_QUOTA_REFRESH_INTERVAL_SECONDS..=MAX_QUOTA_REFRESH_INTERVAL_SECONDS)
        .contains(&refresh_interval_seconds)
    {
        return Err("quota refresh interval is invalid".to_string());
    }
    if !(MIN_QUOTA_REQUEST_TIMEOUT_SECONDS..=MAX_QUOTA_REQUEST_TIMEOUT_SECONDS)
        .contains(&request_timeout_seconds)
    {
        return Err("quota request timeout is invalid".to_string());
    }
    Ok(())
}

fn validate_routing_policy(
    max_retry_candidates: u8,
    session_affinity_ttl_seconds: u64,
) -> Result<(), String> {
    if !(1..=8).contains(&max_retry_candidates) {
        return Err("max retry candidates is invalid".to_string());
    }
    if !(MIN_SESSION_AFFINITY_TTL_SECONDS..=MAX_SESSION_AFFINITY_TTL_SECONDS)
        .contains(&session_affinity_ttl_seconds)
    {
        return Err("session affinity TTL is invalid".to_string());
    }
    Ok(())
}

fn normalize_model_ids(models: Vec<String>) -> Result<Vec<String>, String> {
    if models.len() > 4_096 {
        return Err("model list exceeds the supported limit".to_string());
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for model in models {
        let model = model.trim();
        if model.is_empty() {
            continue;
        }
        if model.len() > 256 || model.chars().any(char::is_control) {
            return Err("model id is invalid".to_string());
        }
        if seen.insert(model.to_ascii_lowercase()) {
            normalized.push(model.to_string());
        }
    }
    Ok(normalized)
}

fn db_error(error: rusqlite::Error) -> String {
    format!("SQLite operation failed: {error}")
}

fn io_error(error: std::io::Error) -> String {
    format!("store I/O failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_membership_migration_defaults_existing_records_outside_pool() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(MIGRATIONS[0].sql).unwrap();
        connection
            .execute(
                "INSERT INTO sources(id, data_json, secret_ref) VALUES ('source_1', '{\"id\":\"source_1\",\"name\":\"Preserved\"}', 'source:1')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO accounts(id, data_json, secret_ref) VALUES ('account_1', '{\"id\":\"account_1\",\"label\":\"Preserved\"}', 'account:1')",
                [],
            )
            .unwrap();
        connection.execute_batch(MIGRATIONS[4].sql).unwrap();
        let source: bool = connection
            .query_row(
                "SELECT json_extract(data_json, '$.inPool') FROM sources WHERE id = 'source_1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let account: bool = connection
            .query_row(
                "SELECT json_extract(data_json, '$.inPool') FROM accounts WHERE id = 'account_1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!source);
        assert!(!account);
    }

    #[test]
    fn cooldown_migration_clears_legacy_long_delays() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(MIGRATIONS[0].sql).unwrap();
        connection
            .execute(
                "INSERT INTO accounts(id, data_json, secret_ref) VALUES ('account_1', '{\"id\":\"account_1\",\"cooldowns\":{\"*\":1784000000000,\"gpt-5.6-luna\":1784001013965},\"consecutiveFailures\":7}', 'account:1')",
                [],
            )
            .unwrap();
        connection.execute_batch(MIGRATIONS[9].sql).unwrap();
        let (cooldowns, failures): (String, u32) = connection
            .query_row(
                "SELECT json_extract(data_json, '$.cooldowns'), json_extract(data_json, '$.consecutiveFailures') FROM accounts WHERE id = 'account_1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(cooldowns, "{}");
        assert_eq!(failures, 0);
    }

    #[test]
    fn pool_membership_batch_rolls_back_when_one_record_is_missing() {
        let root = test_root("pool-membership-rollback");
        let store = Store::open(root.join("relay.sqlite")).unwrap();
        {
            let connection = store.lock().unwrap();
            connection
                .execute(
                    "INSERT INTO sources(id, data_json, secret_ref) VALUES ('source_1', '{\"id\":\"source_1\",\"inPool\":false}', 'source:1')",
                    [],
                )
                .unwrap();
        }

        assert!(store
            .replace_pool_membership(
                &[
                    ("source_1".to_string(), true),
                    ("missing".to_string(), true)
                ],
                &[],
            )
            .is_err());
        let in_pool: bool = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT json_extract(data_json, '$.inPool') FROM sources WHERE id = 'source_1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!in_pool);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quota_policy_is_validated_and_persists() {
        let root = test_root("quota-policy");
        let path = root.join("relay.sqlite");
        let store = Store::open(path.clone()).unwrap();
        assert_eq!(store.quota_policy().unwrap(), (300, 20, false));
        assert!(store.set_quota_policy(119, 20, false).is_err());
        assert!(store.set_quota_policy(120, 9, false).is_err());
        store.set_quota_policy(120, 10, true).unwrap();
        drop(store);

        let reopened = Store::open(path).unwrap();
        assert_eq!(reopened.quota_policy().unwrap(), (120, 10, true));
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn routing_policy_is_validated_and_persists() {
        let root = test_root("routing-policy");
        let path = root.join("relay.sqlite");
        let store = Store::open(path.clone()).unwrap();
        assert_eq!(store.routing_policy().unwrap(), (3, false, 3_600));
        assert!(store.set_routing_policy(0, true, 3_600).is_err());
        assert!(store.set_routing_policy(3, true, 59).is_err());
        store.set_routing_policy(5, false, 300).unwrap();
        drop(store);

        let reopened = Store::open(path).unwrap();
        assert_eq!(reopened.routing_policy().unwrap(), (5, false, 300));
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hidden_models_are_validated_deduplicated_and_persisted() {
        let root = test_root("hidden-models");
        let path = root.join("relay.sqlite");
        let store = Store::open(path.clone()).unwrap();
        assert!(store.hidden_models().unwrap().is_empty());
        store
            .set_hidden_models(vec![" gpt-5.4 ".into(), "GPT-5.4".into()])
            .unwrap();
        assert!(store.set_hidden_models(vec!["x\nunsafe".into()]).is_err());
        drop(store);

        let reopened = Store::open(path).unwrap();
        assert_eq!(reopened.hidden_models().unwrap(), ["gpt-5.4"]);
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn store_migrates_and_preserves_server_identity() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-store-{}", uuid::Uuid::new_v4()));
        let path = root.join("relay.sqlite");
        let first = Store::open(path.clone()).unwrap();
        let server_id = first.server_id().unwrap();
        assert!(first.gateway_enabled().unwrap());
        assert!(!first.common_proxy_configured().unwrap());
        assert!(!first.account_proxy_required().unwrap());
        first.set_common_proxy_configured(true).unwrap();
        first.set_account_proxy_required(true).unwrap();
        assert_eq!(
            first.metadata("schema_version").unwrap(),
            Some(SERVER_SCHEMA_VERSION.to_string())
        );
        drop(first);
        let second = Store::open(path).unwrap();
        assert_eq!(second.server_id().unwrap(), server_id);
        assert!(second.common_proxy_configured().unwrap());
        assert!(second.account_proxy_required().unwrap());
        drop(second);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn v1_migration_creates_backup_and_ordered_ledger() {
        let root = test_root("v1-migration");
        let path = root.join("relay.sqlite");
        create_v1_database(&path, "stable-server-id");

        let store = Store::open(path.clone()).unwrap();
        assert_eq!(store.server_id().unwrap(), "stable-server-id");
        assert_eq!(
            store.metadata("schema_version").unwrap(),
            Some(SERVER_SCHEMA_VERSION.to_string())
        );
        let ledger = {
            let connection = store.lock().unwrap();
            let mut statement = connection
                .prepare("SELECT version, name FROM schema_migrations ORDER BY version")
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            ledger,
            vec![
                (1, "001_init".to_string()),
                (2, "002_migration_ledger".to_string()),
                (3, "003_usage_query_indexes".to_string()),
                (4, "004_account_proxies".to_string()),
                (5, "005_pool_membership".to_string()),
                (6, "006_model_rules".to_string()),
                (7, "007_cached_input_tokens".to_string()),
                (8, "008_reasoning_tokens".to_string()),
                (9, "009_ttft_ms".to_string()),
                (10, "010_reset_legacy_cooldowns".to_string()),
                (11, "011_request_rotation_default".to_string())
            ]
        );
        drop(store);

        let backup = Connection::open(sibling_path(&path, ".pre-migration")).unwrap();
        assert_eq!(read_schema_version(&backup).unwrap(), 1);
        drop(backup);
        assert!(!sibling_path(&path, ".migration-in-progress").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn newer_schema_is_rejected_without_rewriting_database() {
        let root = test_root("newer-schema");
        let path = root.join("relay.sqlite");
        create_v1_database(&path, "future-server-id");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE metadata SET value = '99' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();

        let error = Store::open(path.clone()).err().unwrap();
        assert!(error.contains("newer than supported"));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!sibling_path(&path, ".pre-migration").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_migration_restores_backup_before_retry() {
        let root = test_root("interrupted-migration");
        let path = root.join("relay.sqlite");
        create_v1_database(&path, "original-server-id");
        let connection = Connection::open(&path).unwrap();
        connection
            .backup(
                rusqlite::MAIN_DB,
                sibling_path(&path, ".pre-migration"),
                None,
            )
            .unwrap();
        connection
            .execute(
                "UPDATE metadata SET value = 'corrupted-server-id' WHERE key = 'server_id'",
                [],
            )
            .unwrap();
        drop(connection);
        fs::write(
            sibling_path(&path, ".migration-in-progress"),
            format!("1:{SERVER_SCHEMA_VERSION}\n"),
        )
        .unwrap();

        let store = Store::open(path.clone()).unwrap();
        assert_eq!(store.server_id().unwrap(), "original-server-id");
        assert_eq!(
            store.metadata("schema_version").unwrap(),
            Some(SERVER_SCHEMA_VERSION.to_string())
        );
        assert!(!sibling_path(&path, ".migration-in-progress").exists());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_interrupted_backup_never_replaces_live_database() {
        let root = test_root("corrupt-migration-backup");
        let path = root.join("relay.sqlite");
        create_v1_database(&path, "live-server-id");
        let before = fs::read(&path).unwrap();
        fs::write(sibling_path(&path, ".pre-migration"), b"not a database").unwrap();
        fs::write(sibling_path(&path, ".migration-in-progress"), b"1:3\n").unwrap();

        assert!(Store::open(path.clone()).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_filters_paginate_escape_wildcards_and_clear() {
        use zenith_relay_core::protocol::UsageRange;

        let root = test_root("usage-query");
        let store = Store::open(root.join("relay.sqlite")).unwrap();
        for (index, success, model, error) in [
            (1, true, "gpt-5.4", None),
            (2, false, "gpt%literal", Some("quota_exhausted")),
            (3, true, "gpt-test", None),
        ] {
            store
                .record_usage(
                    &UsageEvent {
                        request_id: format!("req_{index}"),
                        attempt: 1,
                        local_key_id: "key_alpha".to_string(),
                        source_id: "source_alpha".to_string(),
                        candidate_id: Some("source_alpha".to_string()),
                        account_id: None,
                        requested_model: Some(model.to_string()),
                        resolved_model: Some(model.to_string()),
                        wire_api: WireApi::Responses,
                        success,
                        http_status: if success { 200 } else { 429 },
                        error_category: error.map(str::to_string),
                        cooldown_scope: None,
                        retry_at_ms: None,
                        consecutive_failures: None,
                        latency_ms: 10,
                        ttft_ms: Some(4),
                        input_tokens: Some(1),
                        cached_input_tokens: Some(1),
                        reasoning_tokens: Some(1),
                        output_tokens: Some(1),
                        total_tokens: Some(2),
                    },
                    2_000 + index,
                )
                .unwrap();
        }

        let page = store
            .usage_page(&UsageQuery {
                page: 1,
                page_size: 1,
                range: Some(UsageRange::Custom),
                from_ms: Some(2_000),
                to_ms: Some(3_000),
                model_query: Some("%".to_string()),
                success: Some(false),
                error_category: Some("quota_exhausted".to_string()),
                request_id_query: Some("req_2".to_string()),
                ..UsageQuery::default()
            })
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.total_pages, 1);
        assert_eq!(page.events[0].request_id, "req_2");
        assert_eq!(page.events[0].ttft_ms, Some(4));
        let hint = format!("{:x}", Sha256::digest(b"source_alpha"))[..12].to_string();
        assert_eq!(
            store.api_equivalents().unwrap().get(&hint),
            Some(&ApiEquivalentSummary {
                micro_usd: 15,
                priced_tokens: 2,
                unpriced_tokens: 4,
            })
        );
        assert_eq!(store.clear_usage().unwrap(), 3);
        assert_eq!(store.usage_page(&UsageQuery::default()).unwrap().total, 0);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    fn create_v1_database(path: &Path, server_id: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(include_str!("../../migrations/001_init.sql"))
            .unwrap();
        connection
            .execute(
                "INSERT INTO metadata(key, value) VALUES ('server_id', ?1)",
                [server_id],
            )
            .unwrap();
    }

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "zenith-relay-store-{name}-{}",
            uuid::Uuid::new_v4()
        ))
    }
}
