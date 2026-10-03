use super::super::*;

impl TelemetryDb {
    /// Chats with a cache write or read in the selected period.
    ///
    /// A pure miss does not move the touch. The chat start and the latest
    /// reported retention stay available even when they fall outside the
    /// period or the selected account.
    pub fn cache_sessions(&self, query: &UsageQuery) -> Result<Vec<CacheSession>> {
        let connection = self.lock_connection()?;
        let mut clauses = vec![
            "client_context_id IS NOT NULL".to_string(),
            "(COALESCE(cached_input_tokens, 0) > 0 OR COALESCE(cache_write_input_tokens, 0) > 0)"
                .to_string(),
        ];
        let mut values = Vec::new();
        if let Some(value) = query.from_ms {
            clauses.push("created_at >= datetime(? / 1000, 'unixepoch')".to_string());
            values.push(SqlValue::Integer(zenith_relay_core::usage::sql_u64(value)));
        }
        if let Some(value) = query.to_ms {
            clauses.push("created_at <= datetime(? / 1000, 'unixepoch')".to_string());
            values.push(SqlValue::Integer(zenith_relay_core::usage::sql_u64(value)));
        }
        if let Some(account_id) = query
            .source_or_account_query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            clauses.push("account_id = ?".to_string());
            values.push(SqlValue::Text(account_id.to_string()));
        }
        let sql = format!(
            "WITH latest AS (
                SELECT client_context_id, touched_at, model
                FROM (
                    SELECT client_context_id,
                        created_at AS touched_at,
                        COALESCE(resolved_model, requested_model) AS model,
                        ROW_NUMBER() OVER (
                            PARTITION BY client_context_id
                            ORDER BY created_at DESC, id DESC
                        ) AS rn
                    FROM request_logs
                    WHERE {where_sql}
                )
                WHERE rn = 1
            ),
            bounds AS (
                SELECT client_context_id, MIN(created_at) AS started_at
                FROM request_logs
                WHERE client_context_id IN (SELECT client_context_id FROM latest)
                GROUP BY client_context_id
            ),
            reported AS (
                SELECT client_context_id, cache_write_ttl
                FROM (
                    SELECT client_context_id,
                        cache_write_ttl,
                        ROW_NUMBER() OVER (
                            PARTITION BY client_context_id
                            ORDER BY created_at DESC, id DESC
                        ) AS rn
                    FROM request_logs
                    WHERE client_context_id IN (SELECT client_context_id FROM latest)
                        AND cache_write_ttl IS NOT NULL
                        AND cache_write_ttl <> ''
                )
                WHERE rn = 1
            )
            SELECT latest.client_context_id,
                strftime('%Y-%m-%dT%H:%M:%SZ', bounds.started_at),
                strftime('%Y-%m-%dT%H:%M:%SZ', latest.touched_at),
                latest.model,
                reported.cache_write_ttl
            FROM latest
            JOIN bounds ON bounds.client_context_id = latest.client_context_id
            LEFT JOIN reported ON reported.client_context_id = latest.client_context_id
            ORDER BY latest.touched_at DESC
            LIMIT 100",
            where_sql = clauses.join(" AND ")
        );
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let sessions = statement
            .query_map(params_from_iter(values.iter()), |row| {
                let cache_write_ttl: Option<String> = row.get(4)?;
                Ok(CacheSession {
                    client_context_id: row.get(0)?,
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
