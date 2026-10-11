use super::sqlite::{db_error, parse_json, to_json, Store};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use zenith_relay_core::{
    automations::{AccountSelector, WakeAutomationState, WakeCoordinator, WakeTask},
    quota::QuotaWindowKind,
};

impl Store {
    pub fn wake_tasks(&self) -> Result<Vec<WakeTask>, String> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT data_json FROM wake_tasks ORDER BY id")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_error)?;
        let mut tasks = rows
            .map(|row| parse_json(&row.map_err(db_error)?))
            .collect::<Result<Vec<WakeTask>, String>>()?;
        for task in &mut tasks {
            task.window_kinds = [QuotaWindowKind::Primary].into();
        }
        Ok(tasks)
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

    pub fn save_wake_task_and_state(
        &self,
        task: &WakeTask,
        state: &WakeAutomationState,
    ) -> Result<(), String> {
        let task_json = to_json(task)?;
        let state_json = to_json(state)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        transaction
            .execute(
                "INSERT INTO wake_tasks(id, data_json) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET data_json=excluded.data_json",
                params![task.id, task_json],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "INSERT INTO wake_state(singleton, data_json) VALUES (1, ?1) ON CONFLICT(singleton) DO UPDATE SET data_json=excluded.data_json",
                [state_json],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)
    }

    pub fn delete_wake_task_and_save_state(
        &self,
        id: &str,
        state: &WakeAutomationState,
    ) -> Result<bool, String> {
        let state_json = to_json(state)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let deleted = transaction
            .execute("DELETE FROM wake_tasks WHERE id = ?1", [id])
            .map_err(db_error)?
            > 0;
        if deleted {
            transaction
                .execute(
                    "INSERT INTO wake_state(singleton, data_json) VALUES (1, ?1) ON CONFLICT(singleton) DO UPDATE SET data_json=excluded.data_json",
                    [state_json],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        Ok(deleted)
    }

    pub fn remove_account_from_wake_tasks(
        &self,
        account_id: &str,
        completed_at_ms: u64,
    ) -> Result<(), String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        prune_account_wake_tasks(&transaction, account_id, completed_at_ms)?;
        transaction.commit().map_err(db_error)
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
}

/// Used inside the inventory transaction so a failed trash operation cannot
/// leave its account's wake selectors and pending work partially removed.
pub(super) fn prune_account_wake_tasks(
    connection: &Connection,
    account_id: &str,
    completed_at_ms: u64,
) -> Result<(), String> {
    let mut statement = connection
        .prepare("SELECT data_json FROM wake_tasks ORDER BY id")
        .map_err(db_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error)?;
    let mut tasks = rows
        .map(|row| parse_json(&row.map_err(db_error)?))
        .collect::<Result<Vec<WakeTask>, String>>()?;
    tasks.retain_mut(|task| {
        task.window_kinds = [QuotaWindowKind::Primary].into();
        let AccountSelector::AccountIds(account_ids) = &mut task.account_selector else {
            return true;
        };
        if !account_ids.remove(account_id) {
            return true;
        }
        task.updated_at_ms = completed_at_ms;
        !account_ids.is_empty()
    });
    let state_json = connection
        .query_row(
            "SELECT data_json FROM wake_state WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?;
    let wake_state = match state_json {
        Some(json) => parse_json(&json)?,
        None => WakeAutomationState::new(1_024, 256).map_err(|error| error.to_string())?,
    };
    let mut coordinator =
        WakeCoordinator::from_state(wake_state).map_err(|error| error.to_string())?;
    coordinator.remove_pending_for_account(account_id, completed_at_ms);
    connection
        .execute("DELETE FROM wake_tasks", [])
        .map_err(db_error)?;
    for task in &tasks {
        connection
            .execute(
                "INSERT INTO wake_tasks(id, data_json) VALUES (?1, ?2)",
                params![task.id, to_json(task)?],
            )
            .map_err(db_error)?;
    }
    connection.execute(
        "INSERT INTO wake_state(singleton, data_json) VALUES (1, ?1) ON CONFLICT(singleton) DO UPDATE SET data_json=excluded.data_json",
        [to_json(coordinator.state())?],
    ).map_err(db_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::test_root;
    use std::collections::BTreeSet;
    use zenith_relay_core::{automations::AccountSelector, quota::QuotaWindowKind};

    fn task() -> WakeTask {
        crate::test_fixtures::wake_task()
    }

    #[test]
    fn task_changes_commit_with_wake_state() {
        let root = test_root("wake-task-state");
        let store = Store::open(root.join("relay.sqlite")).unwrap();
        let mut stored_task = task();
        stored_task.window_kinds.insert(QuotaWindowKind::Secondary);
        let mut normalized_task = stored_task.clone();
        normalized_task.window_kinds = [QuotaWindowKind::Primary].into();
        let updated_state = WakeAutomationState::new(8, 4).unwrap();

        store
            .save_wake_task_and_state(&stored_task, &updated_state)
            .unwrap();
        assert_eq!(store.wake_tasks().unwrap(), vec![normalized_task.clone()]);
        assert_eq!(store.wake_state().unwrap(), updated_state);

        let deleted_state = WakeAutomationState::new(4, 2).unwrap();
        assert!(store
            .delete_wake_task_and_save_state(&normalized_task.id, &deleted_state)
            .unwrap());
        assert!(store.wake_tasks().unwrap().is_empty());
        assert_eq!(store.wake_state().unwrap(), deleted_state);

        let all_accounts = task();
        let mut selected_accounts = task();
        selected_accounts.id = "wake_selected".into();
        selected_accounts.account_selector =
            AccountSelector::AccountIds(BTreeSet::from(["account_a".into(), "account_b".into()]));
        let mut removed_account = task();
        removed_account.id = "wake_removed".into();
        removed_account.account_selector =
            AccountSelector::AccountIds(BTreeSet::from(["account_a".into()]));
        store.save_wake_task(&all_accounts).unwrap();
        store.save_wake_task(&selected_accounts).unwrap();
        store.save_wake_task(&removed_account).unwrap();
        store
            .remove_account_from_wake_tasks("account_a", 50)
            .unwrap();
        let tasks = store.wake_tasks().unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().any(|task| task.id == all_accounts.id));
        let selected = tasks
            .iter()
            .find(|task| task.id == selected_accounts.id)
            .unwrap();
        assert_eq!(
            selected.account_selector,
            AccountSelector::AccountIds(BTreeSet::from(["account_b".into()]))
        );
        assert_eq!(selected.updated_at_ms, 50);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
