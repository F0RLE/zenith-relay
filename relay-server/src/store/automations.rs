use super::sqlite::{db_error, parse_json, to_json, Store};
use rusqlite::{params, OptionalExtension};
use zenith_relay_core::automations::{WakeAutomationState, WakeTask};

impl Store {
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
}
