use super::sqlite::{db_error, parse_json, to_json, Store};
use crate::state::{GatewayKeyRecord, ServerAccountRecord, ServerProxyRecord, SourceRecord};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};

impl Store {
    pub fn gateway_enabled(&self) -> Result<bool, String> {
        Ok(self
            .metadata("gateway_enabled")?
            .is_none_or(|value| value == "true"))
    }

    pub fn set_gateway_enabled(&self, enabled: bool) -> Result<(), String> {
        self.set_metadata("gateway_enabled", if enabled { "true" } else { "false" })
    }

    pub fn sources(&self) -> Result<Vec<SourceRecord>, String> {
        self.list_records("sources")
    }

    pub fn save_source(&self, record: &SourceRecord) -> Result<(), String> {
        self.save_record("sources", &record.id, &record.secret_ref, record)
    }

    pub fn save_sources(&self, records: &[SourceRecord]) -> Result<(), String> {
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
                    "INSERT INTO sources(id, data_json, secret_ref) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data_json=excluded.data_json, secret_ref=excluded.secret_ref",
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

    pub fn reset_quota_economics_learning(&self) -> Result<(), String> {
        let mut accounts = self.accounts()?;
        for account in &mut accounts {
            account.economics.reset_learning();
            account
                .economics
                .set_value_revision(zenith_relay_core::quota::quota_valuation_revision());
        }
        self.save_accounts(&accounts)
    }

    pub fn delete_account(&self, id: &str) -> Result<Option<ServerAccountRecord>, String> {
        let candidate_hint = hex::encode(Sha256::digest(id.as_bytes()))[..12].to_string();
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let json = transaction
            .query_row(
                "SELECT data_json FROM accounts WHERE id = ?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error)?;
        if json.is_some() {
            transaction
                .execute("DELETE FROM accounts WHERE id = ?1", [id])
                .map_err(db_error)?;
            transaction
                .execute(
                    "INSERT INTO usage_request_tombstones(request_id, archived_at_ms)
                     SELECT request_id, created_at_ms FROM usage_events
                     WHERE candidate_kind = 'account' AND candidate_hint = ?1
                     ON CONFLICT(request_id) DO UPDATE SET
                        archived_at_ms = MAX(usage_request_tombstones.archived_at_ms, excluded.archived_at_ms)",
                    [&candidate_hint],
                )
                .map_err(db_error)?;
            transaction
                .execute(
                    "DELETE FROM usage_events
                     WHERE candidate_kind = 'account' AND candidate_hint = ?1",
                    [&candidate_hint],
                )
                .map_err(db_error)?;
            transaction
                .execute(
                    "DELETE FROM usage_candidate_rollups
                     WHERE candidate_kind = 'account' AND candidate_id = ?1",
                    [&candidate_hint],
                )
                .map_err(db_error)?;
            transaction
                .execute(
                    "DELETE FROM response_affinity WHERE candidate_id = ?1",
                    [id],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        json.map(|value| parse_json(&value)).transpose()
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
        transaction.commit().map_err(db_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::test_root;
    use std::fs;

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
}
