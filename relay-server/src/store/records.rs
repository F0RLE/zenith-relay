use super::sqlite::{db_error, parse_json, to_json, Store};
use crate::state::{GatewayKeyRecord, ServerAccountRecord, ServerProxyRecord, SourceRecord};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::Serialize;
mod accounts;

trait BatchRecord: Serialize {
    const UPSERT_SQL: &'static str;

    fn id(&self) -> &str;
    fn secret_ref(&self) -> &str;
}

impl BatchRecord for SourceRecord {
    const UPSERT_SQL: &'static str =
        "INSERT INTO sources(id, data_json, secret_ref) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data_json=excluded.data_json, secret_ref=excluded.secret_ref";

    fn id(&self) -> &str {
        &self.id
    }

    fn secret_ref(&self) -> &str {
        &self.secret_ref
    }
}

impl BatchRecord for ServerAccountRecord {
    const UPSERT_SQL: &'static str =
        "INSERT INTO accounts(id, data_json, secret_ref) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data_json=excluded.data_json, secret_ref=excluded.secret_ref";

    fn id(&self) -> &str {
        &self.id
    }

    fn secret_ref(&self) -> &str {
        &self.secret_ref
    }
}

impl Store {
    fn save_batch_records<T: BatchRecord>(&self, records: &[T]) -> Result<(), String> {
        let encoded = records
            .iter()
            .map(|record| Ok((record, to_json(record)?)))
            .collect::<Result<Vec<_>, String>>()?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        {
            let mut statement = transaction.prepare(T::UPSERT_SQL).map_err(db_error)?;
            for (record, data_json) in encoded {
                statement
                    .execute(params![record.id(), data_json, record.secret_ref()])
                    .map_err(db_error)?;
            }
        }
        transaction.commit().map_err(db_error)?;
        self.notify_refresh_changed();
        Ok(())
    }
}

impl Store {
    pub fn keys(&self) -> Result<Vec<GatewayKeyRecord>, String> {
        self.list_records("gateway_keys")
    }

    pub fn save_key(&self, record: &GatewayKeyRecord) -> Result<(), String> {
        self.save_record("gateway_keys", &record.id, &record.secret_ref, record)
    }

    pub fn delete_key(&self, id: &str) -> Result<Option<GatewayKeyRecord>, String> {
        self.delete_record("gateway_keys", id)
    }

    pub fn delete_keys(&self, ids: &[String]) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        for id in ids {
            transaction
                .execute("DELETE FROM gateway_keys WHERE id = ?1", [id])
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        self.notify_refresh_changed();
        Ok(())
    }

    pub fn proxies(&self) -> Result<Vec<ServerProxyRecord>, String> {
        self.list_records("proxies")
    }

    pub fn proxy(&self, id: &str) -> Result<Option<ServerProxyRecord>, String> {
        self.lock()?
            .query_row("SELECT data_json FROM proxies WHERE id = ?1", [id], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .map_err(db_error)?
            .map(|value| parse_json(&value))
            .transpose()
    }

    pub fn save_proxy(&self, record: &ServerProxyRecord) -> Result<(), String> {
        self.save_record("proxies", &record.id, &record.secret_ref, record)
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
        transaction.commit().map_err(db_error)?;
        self.notify_refresh_changed();
        Ok(())
    }

    pub fn sources(&self) -> Result<Vec<SourceRecord>, String> {
        self.list_records("sources")
    }

    pub fn save_source(&self, record: &SourceRecord) -> Result<(), String> {
        self.save_record("sources", &record.id, &record.secret_ref, record)
    }

    pub fn save_sources(&self, records: &[SourceRecord]) -> Result<(), String> {
        self.save_batch_records(records)
    }

    pub fn delete_source(&self, id: &str) -> Result<Option<SourceRecord>, String> {
        self.delete_record("sources", id)
    }

    pub fn weekly_reset_was_applied(
        &self,
        account_id: &str,
        fingerprint: &str,
    ) -> Result<bool, String> {
        let key = format!("weekly_reset:{account_id}:{fingerprint}");
        Ok(self.metadata(&key)?.is_some_and(|value| value == "1"))
    }

    pub fn mark_weekly_reset_applied(
        &self,
        account_id: &str,
        fingerprint: &str,
    ) -> Result<(), String> {
        let key = format!("weekly_reset:{account_id}:{fingerprint}");
        self.set_metadata(&key, "1")
    }

    pub fn gateway_enabled(&self) -> Result<bool, String> {
        self.metadata_enabled("gateway_enabled", true)
    }

    pub fn set_gateway_enabled(&self, enabled: bool) -> Result<(), String> {
        self.set_metadata_enabled("gateway_enabled", enabled)
    }

    pub fn codex_websockets_enabled(&self) -> Result<bool, String> {
        self.metadata_enabled("codex_websockets_enabled", true)
    }

    pub fn set_codex_websockets_enabled(&self, enabled: bool) -> Result<(), String> {
        self.set_metadata_enabled("codex_websockets_enabled", enabled)
    }

    pub fn codex_background_tasks_enabled(&self) -> Result<bool, String> {
        self.metadata_enabled("codex_background_tasks_enabled", true)
    }

    pub fn set_codex_background_tasks_enabled(&self, enabled: bool) -> Result<(), String> {
        self.set_metadata_enabled("codex_background_tasks_enabled", enabled)
    }

    pub fn chatgpt_retry_until_available(&self) -> Result<bool, String> {
        self.metadata_enabled("chatgpt_retry_until_available", false)
    }

    pub fn set_chatgpt_retry_until_available(&self, enabled: bool) -> Result<(), String> {
        self.set_metadata_enabled("chatgpt_retry_until_available", enabled)
    }

    fn metadata_enabled(&self, key: &str, default_enabled: bool) -> Result<bool, String> {
        Ok(match self.metadata(key)? {
            Some(value) => value == "true",
            None => default_enabled,
        })
    }

    fn set_metadata_enabled(&self, key: &str, enabled: bool) -> Result<(), String> {
        self.set_metadata(key, if enabled { "true" } else { "false" })
    }
}
#[cfg(test)]
mod tests;
