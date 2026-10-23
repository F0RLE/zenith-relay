use super::super::*;

impl TelemetryDb {
    /// Cache activity scoped to a chat, connection, and upstream model.
    ///
    /// A pure miss does not move the touch. The chat start and the latest
    /// earlier reported retention stay available outside the selected period.
    pub fn cache_sessions(&self, query: &UsageQuery) -> Result<Vec<CacheSession>> {
        let connection = self.lock_connection()?;
        let mut clauses = vec![
            "client_context_id IS NOT NULL".to_string(),
            "(COALESCE(cached_input_tokens, 0) > 0 OR COALESCE(cache_write_input_tokens, 0) > 0)"
                .to_string(),
        ];
        let mut sql_parameters = Vec::new();
        if let Some(from_timestamp_ms) = query.from_ms {
            clauses.push("created_at >= datetime(? / 1000, 'unixepoch')".to_string());
            sql_parameters.push(SqlValue::Integer(zenith_relay_core::usage::sql_u64(
                from_timestamp_ms,
            )));
        }
        if let Some(to_timestamp_ms) = query.to_ms {
            clauses.push("created_at <= datetime(? / 1000, 'unixepoch')".to_string());
            sql_parameters.push(SqlValue::Integer(zenith_relay_core::usage::sql_u64(
                to_timestamp_ms,
            )));
        }
        if let Some(account_id) = query
            .source_or_account_query
            .as_deref()
            .map(str::trim)
            .filter(|account_query| !account_query.is_empty())
        {
            clauses.push("account_id = ?".to_string());
            sql_parameters.push(SqlValue::Text(account_id.to_string()));
        }
        let sql = format!(
            "WITH history AS (
                SELECT client_context_id, account_id, source_id, ABS(id) AS id, created_at,
                    cached_input_tokens, cache_write_input_tokens, cache_write_ttl,
                    COALESCE(resolved_model, requested_model) AS model,
                    MIN(created_at) OVER (
                        PARTITION BY client_context_id, account_id, source_id,
                            COALESCE(resolved_model, requested_model)
                    ) AS started_at
                FROM usage_sends
                WHERE client_context_id IS NOT NULL
            ), latest AS (
                SELECT *
                FROM (
                    SELECT *,
                        created_at AS touched_at,
                        ROW_NUMBER() OVER (
                            PARTITION BY client_context_id, account_id, source_id, model
                            ORDER BY created_at DESC, id DESC
                        ) AS rn
                    FROM history
                    WHERE {where_sql}
                )
                WHERE rn = 1
            )
            SELECT latest.client_context_id,
                strftime('%Y-%m-%dT%H:%M:%SZ', latest.started_at),
                strftime('%Y-%m-%dT%H:%M:%SZ', latest.touched_at),
                latest.model,
                (
                    SELECT prior.cache_write_ttl
                    FROM history AS prior
                    WHERE prior.client_context_id = latest.client_context_id
                        AND prior.account_id IS latest.account_id
                        AND prior.source_id = latest.source_id
                        AND prior.model IS latest.model
                        AND (prior.created_at < latest.touched_at
                            OR (prior.created_at = latest.touched_at AND prior.id <= latest.id))
                        AND (COALESCE(prior.cached_input_tokens, 0) > 0
                            OR COALESCE(prior.cache_write_input_tokens, 0) > 0)
                        AND prior.cache_write_ttl IS NOT NULL
                        AND prior.cache_write_ttl <> ''
                    ORDER BY prior.created_at DESC, prior.id DESC
                    LIMIT 1
                ),
                CASE WHEN latest.account_id IS NULL THEN 'source' ELSE 'account' END,
                COALESCE(latest.account_id, latest.source_id)
            FROM latest
            ORDER BY latest.touched_at DESC
            LIMIT 100",
            where_sql = clauses.join(" AND ")
        );
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let sessions = statement
            .query_map(params_from_iter(sql_parameters.iter()), |row| {
                let cache_write_ttl: Option<String> = row.get(4)?;
                Ok(CacheSession {
                    client_context_id: row.get(0)?,
                    candidate_kind: row.get(5)?,
                    candidate_id: row.get(6)?,
                    started_at: row.get(1)?,
                    touched_at: row.get(2)?,
                    model: row.get(3)?,
                    cache_write_ttl: cache_write_ttl
                        .as_deref()
                        .and_then(zenith_relay_core::usage::normalize_reported_cache_ttls),
                })
            })
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(sessions)
    }
}
