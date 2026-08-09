use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Instant,
};
#[cfg(test)]
use zenith_relay_core::ResponseAffinityBinding;
use zenith_relay_core::{
    api_pricing_revision, estimate_api_equivalent_with_price_override,
    protocol::{UsageBucket, UsageGroup, UsageQuery, UsageTotals},
    ApiEquivalentSummary, ApiModelPriceOverride, DefaultServiceTier, RoutingDiagnostics,
    ToolUseDiagnostics, UsageEvent,
};

mod affinity;
mod migrations;
mod queries;
mod record;
mod state;
mod usage;

use migrations::*;
use usage::{
    configured_model_price, rust_u64, sql_u64, usage_buckets, usage_filter, usage_groups,
    usage_log_from_row, usage_model_equivalents, usage_totals,
};

pub type SourcePriceOverrides = BTreeMap<String, BTreeMap<String, ApiModelPriceOverride>>;

pub struct TelemetryDb {
    connection: Mutex<Connection>,
    usage_revision: AtomicU64,
    api_equivalent_cache: Mutex<Option<CachedUsageEquivalents>>,
    open_duration_ms: f64,
}

#[derive(Clone)]
struct CachedUsageEquivalents {
    usage_revision: u64,
    pricing_revision: String,
    value: UsageEquivalents,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLog {
    pub id: i64,
    pub created_at: String,
    pub request_id: String,
    pub attempt: u16,
    pub source_id: String,
    pub candidate_id: Option<String>,
    pub account_id: Option<String>,
    pub routing: Option<RoutingDiagnostics>,
    pub requested_model: Option<String>,
    pub resolved_model: Option<String>,
    pub wire_api: String,
    pub service_tier: DefaultServiceTier,
    pub applied_service_tier: Option<DefaultServiceTier>,
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<String>,
    pub tool_use: Option<ToolUseDiagnostics>,
    pub latency_ms: u64,
    pub ttft_ms: Option<u64>,
    pub generation_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub api_equivalent: ApiEquivalentSummary,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUsagePage {
    pub events: Vec<UsageLog>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub total_pages: u32,
    pub totals: UsageTotals,
    pub models: Vec<UsageGroup>,
    pub pool_members: Vec<UsageGroup>,
    pub buckets: Vec<UsageBucket>,
}

#[derive(Clone, Default)]
pub struct UsageEquivalents {
    pub accounts: HashMap<String, ApiEquivalentSummary>,
    pub sources: HashMap<String, ApiEquivalentSummary>,
}

impl TelemetryDb {
    pub fn open(path: &Path) -> Result<Self> {
        let started = Instant::now();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        let connection = Connection::open(path).map_err(db_error)?;
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(db_error)?;
        if version > LOCAL_DATABASE_SCHEMA_VERSION {
            return Err(LocalPoolError::new(
                ErrorCode::UnsupportedSchema,
                format!(
                    "local database schema {version} is newer than supported schema {LOCAL_DATABASE_SCHEMA_VERSION}"
                ),
            ));
        }
        if version == 0 {
            connection.execute_batch(MIGRATION_001).map_err(db_error)?;
        }
        if version <= 1 {
            connection.execute_batch(MIGRATION_002).map_err(db_error)?;
        }
        if version <= 2 {
            connection.execute_batch(MIGRATION_003).map_err(db_error)?;
        }
        if version <= 3 {
            connection.execute_batch(MIGRATION_004).map_err(db_error)?;
        }
        if version <= 4 {
            connection.execute_batch(MIGRATION_005).map_err(db_error)?;
        }
        if version <= 5 {
            connection.execute_batch(MIGRATION_006).map_err(db_error)?;
        }
        if version <= 6 {
            connection.execute_batch(MIGRATION_007).map_err(db_error)?;
        }
        if version <= 7 {
            connection.execute_batch(MIGRATION_008).map_err(db_error)?;
        }
        if version <= 8 {
            connection.execute_batch(MIGRATION_009).map_err(db_error)?;
        }
        if version <= 9 {
            connection.execute_batch(MIGRATION_010).map_err(db_error)?;
        }
        if version <= 10 {
            connection.execute_batch(MIGRATION_011).map_err(db_error)?;
        }
        if version <= 11 {
            connection.execute_batch(MIGRATION_012).map_err(db_error)?;
        }
        if version <= 12 {
            connection.execute_batch(MIGRATION_013).map_err(db_error)?;
        }
        if version <= 13 {
            connection.execute_batch(MIGRATION_014).map_err(db_error)?;
        }
        if version <= 14 {
            connection.execute_batch(MIGRATION_015).map_err(db_error)?;
        }
        if version <= 15 {
            connection.execute_batch(MIGRATION_016).map_err(db_error)?;
        }
        if version <= 16 {
            connection.execute_batch(MIGRATION_017).map_err(db_error)?;
        }
        if version <= 17 {
            connection.execute_batch(MIGRATION_018).map_err(db_error)?;
        }
        if version <= 18 {
            connection.execute_batch(MIGRATION_019).map_err(db_error)?;
        }
        if version <= 19 {
            connection.execute_batch(MIGRATION_020).map_err(db_error)?;
        }
        if version <= 20 {
            connection.execute_batch(MIGRATION_021).map_err(db_error)?;
        }
        connection
            .execute_batch(ARCHIVE_USAGE_SQL)
            .map_err(db_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
            usage_revision: AtomicU64::new(0),
            api_equivalent_cache: Mutex::new(None),
            open_duration_ms: started.elapsed().as_secs_f64() * 1_000.0,
        })
    }

    pub fn open_duration_ms(&self) -> f64 {
        self.open_duration_ms
    }

    pub fn record_performance(
        &self,
        name: &str,
        duration_ms: f64,
        context: Option<&str>,
    ) -> Result<()> {
        if !valid_performance_name(name)
            || !duration_ms.is_finite()
            || !(0.0..=600_000.0).contains(&duration_ms)
            || context.is_some_and(|value| {
                value.is_empty()
                    || value.len() > 64
                    || !value.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':')
                    })
            })
        {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "performance sample is invalid",
            ));
        }
        self.connection
            .lock()
            .map_err(lock_error)?
            .execute(
                "INSERT INTO performance_samples(name, duration_ms, context) VALUES (?1, ?2, ?3)",
                params![name, duration_ms, context],
            )
            .map_err(db_error)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn api_equivalents(&self) -> Result<UsageEquivalents> {
        self.api_equivalents_with_price_overrides(&BTreeMap::new(), &BTreeMap::new())
    }

    pub fn api_equivalents_with_price_overrides(
        &self,
        price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
        source_price_overrides: &SourcePriceOverrides,
    ) -> Result<UsageEquivalents> {
        let usage_revision = self.usage_revision.load(Ordering::Acquire);
        let pricing_revision = serde_json::to_string(&(
            api_pricing_revision(),
            price_overrides,
            source_price_overrides,
        ))
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("usage pricing revision serialization failed: {error}"),
            )
        })?;
        if let Some(cached) = self
            .api_equivalent_cache
            .lock()
            .map_err(lock_error)?
            .as_ref()
            .filter(|cached| {
                cached.usage_revision == usage_revision
                    && cached.pricing_revision == pricing_revision
            })
        {
            return Ok(cached.value.clone());
        }
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
        let mut statement = connection
            .prepare(
                "SELECT candidate_kind, candidate_id, model,
                    SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens),
                    SUM(output_tokens), SUM(total_tokens), SUM(input_samples),
                    SUM(cached_input_samples), SUM(cache_write_input_samples)
                 FROM (
                    SELECT candidate_kind, candidate_id, model,
                        input_tokens, cached_input_tokens, cache_write_input_tokens,
                        output_tokens, total_tokens, input_samples,
                        cached_input_samples, cache_write_input_samples
                    FROM usage_candidate_rollups
                    UNION ALL
                    SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
                        COALESCE(account_id, source_id),
                        COALESCE(resolved_model, requested_model, ''),
                        COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_input_tokens), 0),
                        COALESCE(SUM(cache_write_input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                        COALESCE(SUM(total_tokens), 0), COUNT(input_tokens),
                        COUNT(cached_input_tokens), COUNT(cache_write_input_tokens)
                    FROM request_logs GROUP BY 1, 2, 3
                 ) GROUP BY candidate_kind, candidate_id, model",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                let input_tokens: Option<i64> = row.get(3)?;
                let cached_input_tokens: Option<i64> = row.get(4)?;
                let cache_write_input_tokens: Option<i64> = row.get(5)?;
                let output_tokens: Option<i64> = row.get(6)?;
                let total_tokens: Option<i64> = row.get(7)?;
                let input_samples: i64 = row.get(8)?;
                let cached_samples: i64 = row.get(9)?;
                let cache_write_samples: i64 = row.get(10)?;
                let model = row.get::<_, Option<String>>(2)?;
                let kind = row.get::<_, String>(0)?;
                let id = row.get::<_, String>(1)?;
                Ok((
                    kind.clone(),
                    id.clone(),
                    estimate_api_equivalent_with_price_override(
                        model.as_deref(),
                        input_tokens.map(rust_u64),
                        (input_samples > 0 && cached_samples == input_samples)
                            .then(|| cached_input_tokens.map(rust_u64))
                            .flatten(),
                        (input_samples > 0 && cache_write_samples == input_samples)
                            .then(|| cache_write_input_tokens.map(rust_u64))
                            .flatten(),
                        output_tokens.map(rust_u64),
                        total_tokens.map(rust_u64),
                        configured_model_price(
                            price_overrides,
                            source_price_overrides,
                            &kind,
                            &id,
                            model.as_deref(),
                        ),
                    ),
                ))
            })
            .map_err(db_error)?;
        let mut equivalents = UsageEquivalents::default();
        for row in rows {
            let (kind, id, estimate) = row.map_err(db_error)?;
            let values = if kind == "account" {
                &mut equivalents.accounts
            } else {
                &mut equivalents.sources
            };
            values.entry(id).or_default().merge(estimate);
        }
        drop(statement);
        drop(connection);
        if self.usage_revision.load(Ordering::Acquire) == usage_revision {
            self.api_equivalent_cache
                .lock()
                .map_err(lock_error)?
                .replace(CachedUsageEquivalents {
                    usage_revision,
                    pricing_revision,
                    value: equivalents.clone(),
                });
        }
        Ok(equivalents)
    }

    pub fn clear(&self) -> Result<()> {
        self.connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?
            .execute_batch("DELETE FROM request_logs; DELETE FROM usage_candidate_rollups;")
            .map_err(db_error)?;
        self.invalidate_usage_cache();
        Ok(())
    }

    fn invalidate_usage_cache(&self) {
        self.usage_revision.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut cached) = self.api_equivalent_cache.lock() {
            *cached = None;
        }
    }
}

fn valid_performance_name(name: &str) -> bool {
    matches!(
        name,
        "native_startup"
            | "vault"
            | "sqlite"
            | "window"
            | "first_frame"
            | "interactive"
            | "full_snapshot"
            | "full_snapshot_native"
            | "mode_switch"
            | "page_open"
    )
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, "local database lock poisoned")
}

fn db_error(error: rusqlite::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, format!("local database error: {error}"))
}

fn io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::{SelectionReason, TerminalOutputKind, ToolChoiceMode, WireApi};

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
            candidate_id: Some("account_1".into()),
            account_id: Some("account_1".into()),
            routing: Some(RoutingDiagnostics {
                reason: SelectionReason::QuotaHeadroom,
                eligible_candidates: 4,
                quota_remaining_basis_points: Some(6_300),
                in_flight_before: 0,
                dispatches_before: 3,
            }),
            requested_model: Some("gpt-5.4".into()),
            resolved_model: Some("gpt-5.4".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Fast,
            applied_service_tier: Some(DefaultServiceTier::Standard),
            success: true,
            http_status: 200,
            error_category: None,
            tool_use: ToolUseDiagnostics {
                client_tool_count: 3,
                forwarded_tool_count: 3,
                tool_choice: ToolChoiceMode::Auto,
                tool_call_count: 1,
                text_output: false,
                terminal_output: TerminalOutputKind::ToolCall,
            },
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 12,
            ttft_ms: Some(4),
            generation_ms: Some(8),
            input_tokens: Some(2),
            cached_input_tokens: Some(1),
            cache_write_input_tokens: Some(1),
            reasoning_tokens: Some(2),
            output_tokens: Some(3),
            total_tokens: Some(5),
            quota_snapshot: None,
        };
        TelemetryDb::open(&path).unwrap().record(&event).unwrap();
        let database = TelemetryDb::open(&path).unwrap();
        let logs = database.list(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].created_at.ends_with('Z'));
        assert_eq!(logs[0].candidate_id.as_deref(), Some("account_1"));
        assert_eq!(logs[0].ttft_ms, Some(4));
        assert_eq!(logs[0].cached_input_tokens, Some(1));
        assert_eq!(logs[0].cache_write_input_tokens, Some(1));
        assert_eq!(logs[0].reasoning_tokens, Some(2));
        assert_eq!(
            logs[0]
                .tool_use
                .as_ref()
                .map(|tool_use| tool_use.tool_call_count),
            Some(1)
        );
        assert_eq!(logs[0].service_tier, DefaultServiceTier::Fast);
        assert_eq!(
            logs[0].applied_service_tier,
            Some(DefaultServiceTier::Standard)
        );
        assert_eq!(
            logs[0].routing.as_ref().map(|routing| routing.reason),
            Some(SelectionReason::QuotaHeadroom)
        );
        let page = database.usage_page(&UsageQuery::default()).unwrap();
        // The event carries a measured value and the totals are that same value
        // merged once, so the relation holds regardless of catalog prices.
        assert!(page.events[0].api_equivalent.micro_usd > 0);
        assert_eq!(page.totals.api_equivalent, page.events[0].api_equivalent);
        let default_page = database
            .usage_page(&UsageQuery {
                page: 0,
                page_size: 0,
                ..UsageQuery::default()
            })
            .unwrap();
        assert_eq!(default_page.page, 1);
        assert_eq!(default_page.page_size, 50);
        assert_eq!(default_page.total_pages, 1);
        let cached = database.api_equivalents().unwrap();
        assert_eq!(
            database.api_equivalents().unwrap().accounts,
            cached.accounts
        );
        let mut second = event;
        second.request_id = "req_2".into();
        second.input_tokens = Some(20);
        second.total_tokens = Some(23);
        database.record(&second).unwrap();
        assert!(
            database.api_equivalents().unwrap().accounts["account_1"].micro_usd
                > cached.accounts["account_1"].micro_usd
        );
        database
            .record_performance("first_frame", 12.5, Some("startup"))
            .unwrap();
        assert!(database
            .record_performance("unknown_metric", 1.0, None)
            .is_err());
        let performance_samples: i64 = database
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM performance_samples", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(performance_samples, 1);
        database.clear().unwrap();
        assert!(database.list(10).unwrap().is_empty());
        assert_eq!(logs[0].total_tokens, Some(5));
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn response_affinity_survives_reopen_and_expires() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-affinity-{}", uuid::Uuid::new_v4()));
        let path = root.join("usage.sqlite");
        let binding = ResponseAffinityBinding {
            key: "hashed-response".into(),
            candidate_id: "account-1".into(),
            expires_at_ms: 200,
        };
        TelemetryDb::open(&path)
            .unwrap()
            .upsert_affinity(&binding, 100)
            .unwrap();
        let database = TelemetryDb::open(&path).unwrap();
        assert_eq!(
            database.find_affinity(&binding.key, 199).unwrap(),
            Some(binding)
        );
        assert!(database.affinity_bindings(200).unwrap().is_empty());
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleting_account_data_removes_usage_rollups_and_affinity() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-account-delete-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        database
            .connection
            .lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO request_logs(
                    request_id, local_key_id, source_id, candidate_id, account_id,
                    wire_api, success, http_status, latency_ms
                 ) VALUES ('request-delete', 'key', 'codex', 'account-delete',
                    'account-delete', 'responses', 1, 200, 1);
                 INSERT INTO usage_candidate_rollups(candidate_kind, candidate_id, model)
                 VALUES ('account', 'account-delete', 'gpt-test');
                 INSERT INTO response_affinity(
                    response_key, candidate_id, expires_at_ms, updated_at_ms
                 ) VALUES ('response-delete', 'account-delete', 1000, 1);",
            )
            .unwrap();

        database
            .replace_state_json_and_delete_account_data(
                &[("accounts", "[]".to_string())],
                "account-delete",
            )
            .unwrap();

        let remaining: i64 = database
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM request_logs WHERE account_id = 'account-delete') +
                    (SELECT COUNT(*) FROM usage_candidate_rollups
                     WHERE candidate_kind = 'account' AND candidate_id = 'account-delete') +
                    (SELECT COUNT(*) FROM response_affinity
                     WHERE candidate_id = 'account-delete')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
        assert_eq!(
            database.state_json("accounts").unwrap().as_deref(),
            Some("[]")
        );
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_state_batch_does_not_partially_save_or_purge_account_data() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-account-delete-rollback-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        database
            .replace_state_json(&[("accounts", "[\"before\"]".to_string())])
            .unwrap();
        database
            .connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO request_logs(
                    request_id, local_key_id, source_id, candidate_id, account_id,
                    wire_api, success, http_status, latency_ms
                 ) VALUES ('request-rollback', 'key', 'codex', 'account-rollback',
                    'account-rollback', 'responses', 1, 200, 1)",
                [],
            )
            .unwrap();

        let error = database
            .replace_state_json_and_delete_account_data(
                &[
                    ("accounts", "[]".to_string()),
                    ("invalid-key", "{}".to_string()),
                ],
                "account-rollback",
            )
            .unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidState);
        assert_eq!(
            database.state_json("accounts").unwrap().as_deref(),
            Some("[\"before\"]")
        );
        let remaining: i64 = database
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM request_logs WHERE account_id = 'account-rollback'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 1);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn response_affinity_storage_matches_the_runtime_capacity() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-affinity-capacity-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        database
            .connection
            .lock()
            .unwrap()
            .execute_batch(&format!(
                "WITH RECURSIVE entries(value) AS (
                    SELECT 0 UNION ALL SELECT value + 1 FROM entries WHERE value < {limit}
                 )
                 INSERT INTO response_affinity(
                    response_key, candidate_id, expires_at_ms, updated_at_ms
                 )
                 SELECT printf('response-%05d', value), 'account-1', 999999, value
                 FROM entries;",
                limit = MAX_RESPONSE_AFFINITY_ROWS + 1
            ))
            .unwrap();

        let bindings = database.affinity_bindings(0).unwrap();
        assert_eq!(bindings.len(), MAX_RESPONSE_AFFINITY_ROWS);
        assert_eq!(
            bindings.first().map(|binding| binding.key.as_str()),
            Some("response-04097")
        );
        assert_eq!(
            bindings.last().map(|binding| binding.key.as_str()),
            Some("response-00002")
        );
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_page_aggregates_the_full_filtered_range_not_only_the_page() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-usage-page-{}", uuid::Uuid::new_v4()));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        let mut event = UsageEvent {
            request_id: "req_page_1".into(),
            attempt: 1,
            local_key_id: "key_1".into(),
            source_id: "openai-codex".into(),
            candidate_id: Some("account_1".into()),
            account_id: Some("account_1".into()),
            routing: None,
            requested_model: Some("gpt-5.4".into()),
            resolved_model: Some("gpt-5.4".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: true,
            http_status: 200,
            error_category: None,
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 428,
            ttft_ms: Some(128),
            generation_ms: Some(300),
            input_tokens: Some(20),
            cached_input_tokens: Some(12),
            cache_write_input_tokens: None,
            reasoning_tokens: Some(5),
            output_tokens: Some(8),
            total_tokens: Some(28),
            quota_snapshot: None,
        };
        database.record(&event).unwrap();
        event.request_id = "req_page_2".into();
        event.candidate_id = Some("account_2".into());
        event.account_id = Some("account_2".into());
        event.wire_api = WireApi::ChatCompletions;
        event.latency_ms = 500;
        event.ttft_ms = Some(100);
        event.input_tokens = Some(10);
        event.cached_input_tokens = Some(0);
        event.reasoning_tokens = Some(0);
        event.output_tokens = Some(20);
        event.total_tokens = Some(30);
        database.record(&event).unwrap();
        event.request_id = "req_page_3".into();
        event.success = false;
        event.http_status = 502;
        event.error_category = Some("upstream_websocket_closed".into());
        event.generation_ms = Some(5_000);
        event.input_tokens = Some(0);
        event.output_tokens = Some(100);
        event.total_tokens = Some(100);
        database.record(&event).unwrap();

        let page = database
            .usage_page(&UsageQuery {
                page: 1,
                page_size: 1,
                from_ms: Some(0),
                bucket_ms: Some(3_600_000),
                ..UsageQuery::default()
            })
            .unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.total, 3);
        assert_eq!(page.total_pages, 3);
        assert_eq!(page.totals.requests, 3);
        assert_eq!(page.totals.total_tokens, 158);
        assert_eq!(page.totals.generation_output_tokens, 23);
        assert_eq!(page.totals.generation_ms, 600);
        assert_eq!(page.totals.generation_samples, 2);
        assert_eq!(page.totals.speed_output_tokens, 23);
        assert_eq!(page.totals.speed_duration_ms, 928);
        assert_eq!(page.totals.api_equivalent.priced_tokens, 158);
        assert_eq!(page.models.len(), 1);
        assert_eq!(page.pool_members.len(), 2);
        assert_eq!(page.buckets.len(), 1);
        assert_eq!(page.buckets[0].totals.total_tokens, 158);
        assert_eq!(
            page.buckets[0].totals.api_equivalent,
            page.totals.api_equivalent
        );
        assert_eq!(page.events[0].wire_api, "chat_completions");
        assert_eq!(page.events[0].service_tier, DefaultServiceTier::Standard);
        assert!(page.events[0].tool_use.is_none());

        let chat = database
            .usage_page(&UsageQuery {
                wire_api: Some(WireApi::ChatCompletions),
                ..UsageQuery::default()
            })
            .unwrap();
        assert_eq!(chat.total, 2);
        assert_eq!(chat.events[0].request_id, "req_page_3");
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_keeps_only_the_terminal_fallback_attempt() {
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
            candidate_id: Some("source_1".into()),
            account_id: None,
            routing: None,
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: false,
            http_status: 503,
            error_category: Some("upstream_unavailable".into()),
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: Some("*".into()),
            retry_at_ms: Some(60_000),
            consecutive_failures: Some(1),
            latency_ms: 5,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_tokens: None,
            output_tokens: None,
            total_tokens: None,
            quota_snapshot: None,
        };
        database.record(&event).unwrap();
        event.attempt = 2;
        event.source_id = "source_2".into();
        event.candidate_id = Some("source_2".into());
        event.success = true;
        event.http_status = 200;
        event.error_category = None;
        event.cooldown_scope = None;
        event.retry_at_ms = None;
        event.consecutive_failures = Some(0);
        database.record(&event).unwrap();

        event.attempt = 1;
        event.source_id = "source_stale".into();
        event.success = false;
        event.http_status = 503;
        event.error_category = Some("upstream_unavailable".into());
        database.record(&event).unwrap();

        let logs = database.list(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].attempt, 2);
        assert!(logs[0].success);
        assert_eq!(logs[0].source_id, "source_2");
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_keeps_only_the_last_failure_when_all_attempts_fail() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-failed-attempts-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        let mut event = UsageEvent {
            request_id: "req_failed".into(),
            attempt: 1,
            local_key_id: "key_1".into(),
            source_id: "source_1".into(),
            candidate_id: Some("source_1".into()),
            account_id: None,
            routing: None,
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: false,
            http_status: 503,
            error_category: Some("upstream_unavailable".into()),
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: Some("*".into()),
            retry_at_ms: Some(60_000),
            consecutive_failures: Some(1),
            latency_ms: 5,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_tokens: None,
            output_tokens: None,
            total_tokens: None,
            quota_snapshot: None,
        };
        database.record(&event).unwrap();
        event.attempt = 2;
        event.source_id = "source_2".into();
        event.candidate_id = Some("source_2".into());
        event.http_status = 429;
        event.error_category = Some("upstream_rate_limited".into());
        database.record(&event).unwrap();

        let logs = database.list(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].attempt, 2);
        assert!(!logs[0].success);
        assert_eq!(logs[0].http_status, 429);
        assert_eq!(logs[0].source_id, "source_2");
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn api_equivalents_group_priced_and_unknown_usage_by_candidate() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-equivalent-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        let event = UsageEvent {
            request_id: "req_equivalent".into(),
            attempt: 1,
            local_key_id: "key_1".into(),
            source_id: "source_1".into(),
            candidate_id: Some("account_1".into()),
            account_id: Some("account_1".into()),
            routing: None,
            requested_model: Some("gpt-5.4".into()),
            resolved_model: Some("gpt-5.4".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: true,
            http_status: 200,
            error_category: None,
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 12,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: Some(20),
            cached_input_tokens: Some(10),
            cache_write_input_tokens: None,
            reasoning_tokens: Some(3),
            output_tokens: Some(8),
            total_tokens: Some(28),
            quota_snapshot: None,
        };
        database.record(&event).unwrap();
        let equivalents = database.api_equivalents().unwrap();
        assert_eq!(
            equivalents.accounts.get("account_1"),
            Some(&ApiEquivalentSummary {
                micro_usd: 148,
                priced_tokens: 28,
                unpriced_tokens: 0,
            })
        );
        assert!(equivalents.sources.is_empty());
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn custom_price_revalues_existing_unknown_model_usage() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-custom-price-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        database
            .record(&UsageEvent {
                request_id: "req_custom_price".into(),
                attempt: 1,
                local_key_id: "key_1".into(),
                source_id: "source_1".into(),
                candidate_id: Some("source_1".into()),
                account_id: None,
                routing: None,
                requested_model: Some("private-model".into()),
                resolved_model: Some("private-model".into()),
                wire_api: WireApi::Responses,
                service_tier: DefaultServiceTier::Standard,
                applied_service_tier: None,
                success: true,
                http_status: 200,
                error_category: None,
                tool_use: ToolUseDiagnostics::default(),
                cooldown_scope: None,
                retry_at_ms: None,
                consecutive_failures: Some(0),
                latency_ms: 100,
                ttft_ms: Some(10),
                generation_ms: Some(90),
                input_tokens: Some(1_000_000),
                cached_input_tokens: Some(0),
                cache_write_input_tokens: Some(0),
                reasoning_tokens: Some(0),
                output_tokens: Some(100_000),
                total_tokens: Some(1_100_000),
                quota_snapshot: None,
            })
            .unwrap();

        assert_eq!(
            database
                .usage_page(&UsageQuery::default())
                .unwrap()
                .totals
                .api_equivalent
                .unpriced_tokens,
            1_100_000
        );
        let prices = BTreeMap::from([(
            "private-model".into(),
            ApiModelPriceOverride {
                input_micro_usd_per_million: 2_000_000,
                cached_input_micro_usd_per_million: Some(200_000),
                cache_write_5m_micro_usd_per_million: None,
                cache_write_1h_micro_usd_per_million: None,
                output_micro_usd_per_million: 10_000_000,
            },
        )]);
        let page = database
            .usage_page_with_price_overrides(&UsageQuery::default(), &prices, &BTreeMap::new())
            .unwrap();
        assert_eq!(page.totals.api_equivalent.micro_usd, 3_000_000);
        assert_eq!(page.totals.api_equivalent.priced_tokens, 1_100_000);
        assert_eq!(page.totals.api_equivalent.unpriced_tokens, 0);
        assert_eq!(page.events[0].api_equivalent, page.totals.api_equivalent);
        assert_eq!(
            database
                .api_equivalents_with_price_overrides(&prices, &BTreeMap::new())
                .unwrap()
                .sources
                .get("source_1")
                .map(|summary| summary.micro_usd),
            Some(3_000_000)
        );
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_prices_are_applied_before_same_model_usage_is_merged() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-source-prices-{}",
            uuid::Uuid::new_v4()
        ));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        let mut event = UsageEvent {
            request_id: "req_source_cheap".into(),
            attempt: 1,
            local_key_id: "key_1".into(),
            source_id: "source_cheap".into(),
            candidate_id: Some("source_cheap".into()),
            account_id: None,
            routing: None,
            requested_model: Some("private-model".into()),
            resolved_model: Some("private-model".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: true,
            http_status: 200,
            error_category: None,
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 100,
            ttft_ms: Some(10),
            generation_ms: Some(90),
            input_tokens: Some(1_000_000),
            cached_input_tokens: Some(0),
            cache_write_input_tokens: Some(0),
            reasoning_tokens: Some(0),
            output_tokens: Some(100_000),
            total_tokens: Some(1_100_000),
            quota_snapshot: None,
        };
        database.record(&event).unwrap();
        event.request_id = "req_source_expensive".into();
        event.source_id = "source_expensive".into();
        event.candidate_id = Some("source_expensive".into());
        database.record(&event).unwrap();
        event.request_id = "req_account".into();
        event.source_id = "codex".into();
        event.candidate_id = Some("account_1".into());
        event.account_id = Some("account_1".into());
        database.record(&event).unwrap();

        let price = |input, output| ApiModelPriceOverride {
            input_micro_usd_per_million: input,
            cached_input_micro_usd_per_million: Some(input / 10),
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: output,
        };
        let source_prices = BTreeMap::from([
            (
                "source_cheap".into(),
                BTreeMap::from([("private-model".into(), price(1_000_000, 2_000_000))]),
            ),
            (
                "source_expensive".into(),
                BTreeMap::from([("private-model".into(), price(2_000_000, 4_000_000))]),
            ),
        ]);
        let page = database
            .usage_page_with_price_overrides(
                &UsageQuery::default(),
                &BTreeMap::new(),
                &source_prices,
            )
            .unwrap();
        assert_eq!(page.totals.api_equivalent.micro_usd, 3_600_000);
        assert_eq!(page.totals.api_equivalent.unpriced_tokens, 1_100_000);
        assert_eq!(page.models[0].totals.api_equivalent.micro_usd, 3_600_000);
        let event_value = |request_id: &str| {
            page.events
                .iter()
                .find(|event| event.request_id == request_id)
                .unwrap()
                .api_equivalent
        };
        assert_eq!(event_value("req_source_cheap").micro_usd, 1_200_000);
        assert_eq!(event_value("req_source_expensive").micro_usd, 2_400_000);
        assert_eq!(event_value("req_account").unpriced_tokens, 1_100_000);

        let equivalents = database
            .api_equivalents_with_price_overrides(&BTreeMap::new(), &source_prices)
            .unwrap();
        assert_eq!(equivalents.sources["source_cheap"].micro_usd, 1_200_000);
        assert_eq!(equivalents.sources["source_expensive"].micro_usd, 2_400_000);
        assert_eq!(equivalents.accounts["account_1"].micro_usd, 0);
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
        assert_eq!(version, LOCAL_DATABASE_SCHEMA_VERSION);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_v14_migration_keeps_the_latest_attempt_per_request() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-usage-v14-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("usage.sqlite");
        let connection = Connection::open(&path).unwrap();
        for migration in [
            MIGRATION_001,
            MIGRATION_002,
            MIGRATION_003,
            MIGRATION_004,
            MIGRATION_005,
            MIGRATION_006,
            MIGRATION_007,
            MIGRATION_008,
            MIGRATION_009,
            MIGRATION_010,
            MIGRATION_011,
            MIGRATION_012,
            MIGRATION_013,
            MIGRATION_014,
        ] {
            connection.execute_batch(migration).unwrap();
        }
        connection
            .execute(
                "INSERT INTO request_logs (
                    request_id, attempt, local_key_id, source_id, wire_api, success,
                    http_status, latency_ms
                ) VALUES ('req_duplicate', 1, 'key', 'source_1', 'responses', 0, 503, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO request_logs (
                    request_id, attempt, local_key_id, source_id, wire_api, success,
                    http_status, latency_ms
                ) VALUES ('req_duplicate', 2, 'key', 'source_2', 'responses', 1, 200, 2)",
                [],
            )
            .unwrap();
        drop(connection);

        let database = TelemetryDb::open(&path).unwrap();
        let logs = database.list(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].attempt, 2);
        assert!(logs[0].success);
        assert_eq!(logs[0].source_id, "source_2");
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
        let future_version = LOCAL_DATABASE_SCHEMA_VERSION + 1;
        connection
            .pragma_update(None, "user_version", future_version)
            .unwrap();
        drop(connection);

        assert!(matches!(
            TelemetryDb::open(&path).err().unwrap().code,
            ErrorCode::UnsupportedSchema
        ));
        let version: u32 = Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, future_version);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_retention_prunes_old_rows_on_open_and_periodically() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-usage-retention-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("usage.sqlite");
        drop(TelemetryDb::open(&path).unwrap());
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO request_logs (
                    request_id, attempt, local_key_id, source_id, candidate_id, account_id,
                    requested_model, resolved_model, wire_api, success, http_status, latency_ms,
                    input_tokens, cached_input_tokens, output_tokens, total_tokens, created_at
                ) VALUES ('old-open', 1, 'key', 'source', 'account', 'account',
                    'gpt-5.4', 'gpt-5.4', 'responses', 1, 200, 1, 20, 10, 8, 28,
                    datetime('now', '-31 days'))",
                [],
            )
            .unwrap();
        drop(connection);

        let database = TelemetryDb::open(&path).unwrap();
        assert!(database.list(10).unwrap().is_empty());
        assert_eq!(
            database.api_equivalents().unwrap().accounts.get("account"),
            Some(&ApiEquivalentSummary {
                micro_usd: 148,
                priced_tokens: 28,
                unpriced_tokens: 0,
            })
        );
        database
            .connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO request_logs (
                    id, request_id, attempt, local_key_id, source_id, candidate_id,
                    requested_model, resolved_model, wire_api, success, http_status, latency_ms,
                    input_tokens, cached_input_tokens, output_tokens, total_tokens, created_at
                ) VALUES (255, 'old-trigger', 1, 'key', 'source', 'source',
                    'gpt-5.4', 'gpt-5.4', 'responses', 1, 200, 1, 20, 10, 8, 28,
                    datetime('now', '-31 days'))",
                [],
            )
            .unwrap();
        database
            .record(&UsageEvent {
                request_id: "trigger-256".into(),
                attempt: 1,
                local_key_id: "key".into(),
                source_id: "source".into(),
                candidate_id: None,
                account_id: None,
                routing: None,
                requested_model: None,
                resolved_model: None,
                wire_api: WireApi::Responses,
                service_tier: DefaultServiceTier::Standard,
                applied_service_tier: None,
                success: true,
                http_status: 200,
                error_category: None,
                tool_use: ToolUseDiagnostics::default(),
                cooldown_scope: None,
                retry_at_ms: None,
                consecutive_failures: None,
                latency_ms: 1,
                ttft_ms: None,
                generation_ms: None,
                input_tokens: None,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                reasoning_tokens: None,
                output_tokens: None,
                total_tokens: None,
                quota_snapshot: None,
            })
            .unwrap();
        assert_eq!(database.list(10).unwrap().len(), 1);
        assert_eq!(
            database.api_equivalents().unwrap().sources.get("source"),
            Some(&ApiEquivalentSummary {
                micro_usd: 148,
                priced_tokens: 28,
                unpriced_tokens: 0,
            })
        );
        database.clear().unwrap();
        let equivalents = database.api_equivalents().unwrap();
        assert!(equivalents.accounts.is_empty());
        assert!(equivalents.sources.is_empty());
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }
}
