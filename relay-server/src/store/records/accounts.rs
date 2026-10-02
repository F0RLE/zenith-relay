use super::super::sqlite::{db_error, parse_json, to_json, Store};
use crate::state::ServerAccountRecord;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use zenith_relay_core::scheduler::{account_member_key, refresh::RefreshIdentity};

impl Store {
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

    pub fn save_account_and_consume_pending_import(
        &self,
        record: &ServerAccountRecord,
        pending_import_id: &str,
    ) -> Result<bool, String> {
        let data_json = to_json(record)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let existed = transaction
            .query_row(
                "SELECT 1 FROM accounts WHERE id = ?1",
                [record.id.as_str()],
                |_| Ok(()),
            )
            .optional()
            .map_err(db_error)?
            .is_some();
        transaction
            .execute(
                "INSERT INTO accounts(id, data_json, secret_ref) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data_json=excluded.data_json, secret_ref=excluded.secret_ref",
                params![record.id, data_json, record.secret_ref],
            )
            .map_err(db_error)?;
        if transaction
            .execute(
                "DELETE FROM pending_imports WHERE id = ?1",
                [pending_import_id],
            )
            .map_err(db_error)?
            != 1
        {
            return Err("pending import no longer exists".to_string());
        }
        transaction.commit().map_err(db_error)?;
        self.notify_refresh_changed();
        Ok(!existed)
    }

    /// Updates the latest stored account in one transaction so background
    /// observations cannot overwrite an operator's concurrent configuration.
    pub fn update_account<T>(
        &self,
        id: &str,
        update: impl FnOnce(&mut ServerAccountRecord) -> Result<T, String>,
    ) -> Result<Option<T>, String> {
        self.update_account_with_refresh_identity(id, update)
            .map(|updated| updated.map(|(value, _)| value))
    }

    /// Return the durable identity from the same transaction as this usage
    /// observation. A subsequent configuration edit must not make a late
    /// passive quota or Retry-After hint target the replacement registration.
    pub(crate) fn update_account_with_refresh_identity<T>(
        &self,
        id: &str,
        update: impl FnOnce(&mut ServerAccountRecord) -> Result<T, String>,
    ) -> Result<Option<(T, RefreshIdentity)>, String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let snapshot = transaction
            .query_row(
                "SELECT data_json, refresh_revision, (SELECT CAST(value AS INTEGER) FROM metadata WHERE key = 'refresh_config_revision') FROM accounts WHERE id = ?1",
                [id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
            )
            .optional()
            .map_err(db_error)?;
        let Some((json, revision, configuration_revision)) = snapshot else {
            transaction.commit().map_err(db_error)?;
            self.notify_refresh_changed();
            return Ok(None);
        };
        let identity = RefreshIdentity::new(
            account_member_key(id),
            u64::try_from(revision).map_err(|_| "account refresh revision is invalid")?,
            u64::try_from(configuration_revision)
                .map_err(|_| "refresh configuration revision is invalid")?,
        );
        let mut record: ServerAccountRecord = parse_json(&json)?;
        let previous_monitoring = (
            record.enabled,
            record.auth_state,
            record.secret_ref.clone(),
            record.source_id.clone(),
            record.proxy_id.clone(),
            record.bypass_common_proxy,
            record.created_at_ms,
        );
        let value = update(&mut record)?;
        let monitoring_changed = previous_monitoring
            != (
                record.enabled,
                record.auth_state,
                record.secret_ref.clone(),
                record.source_id.clone(),
                record.proxy_id.clone(),
                record.bypass_common_proxy,
                record.created_at_ms,
            );
        transaction
            .execute(
                "UPDATE accounts SET data_json = ?1, secret_ref = ?2 WHERE id = ?3",
                params![to_json(&record)?, record.secret_ref, id],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        // Usage counts/passive quota do not trigger an inventory-wide scan on
        // each completed request. Runtime activity directly updates cadence.
        if monitoring_changed {
            self.notify_refresh_changed();
        }
        Ok(Some((value, identity)))
    }

    /// Persists Team breaker sibling state without recording synthetic usage.
    pub fn block_accounts_for_team(&self, account_ids: &[String]) -> Result<bool, String> {
        let mut changed = false;
        for account_id in account_ids {
            let updated = self.update_account(account_id, |account| {
                let needs_update = account.health
                    != zenith_relay_core::accounts::AccountHealthState::Blocked
                    || account.last_error_code.as_deref() != Some("deactivated_workspace");
                if needs_update {
                    account.health = zenith_relay_core::accounts::AccountHealthState::Blocked;
                    account.last_error_code = Some("deactivated_workspace".to_string());
                }
                Ok(needs_update)
            })?;
            changed |= updated.unwrap_or(false);
        }
        Ok(changed)
    }

    pub fn save_accounts(&self, records: &[ServerAccountRecord]) -> Result<(), String> {
        self.save_batch_records(records)
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
                    zenith_relay_core::usage::DELETE_ACCOUNT_CANDIDATE_ROLLUPS_SQL,
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
        self.notify_refresh_changed();
        json.map(|value| parse_json(&value)).transpose()
    }
}
