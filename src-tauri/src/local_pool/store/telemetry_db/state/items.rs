use super::*;
use rusqlite::Connection;

fn invalid_items() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        "local inventory observation is invalid",
    )
}

fn inventory_key(key: &str) -> bool {
    matches!(key, "sources" | "accounts" | "keys")
}

pub(super) fn overlay(
    connection: &Connection,
    entries: &mut HashMap<String, String>,
) -> Result<()> {
    if !entries.keys().any(|key| inventory_key(key)) {
        return Ok(());
    }
    let mut statement = connection.prepare("SELECT state_key, position, value_json FROM app_state_items ORDER BY state_key, position")
        .map_err(db_error)?;
    let mut rows = statement.query([]).map_err(db_error)?;
    let mut updated = HashMap::<String, Vec<serde_json::Value>>::new();
    while let Some(row) = rows.next().map_err(db_error)? {
        let key: String = row.get(0).map_err(db_error)?;
        let Some(base) = entries.get(&key) else {
            continue;
        };
        if !updated.contains_key(&key) {
            updated.insert(
                key.clone(),
                serde_json::from_str(base).map_err(|_| invalid_items())?,
            );
        }
        let position = usize::try_from(row.get::<_, i64>(1).map_err(db_error)?)
            .map_err(|_| invalid_items())?;
        let item = updated
            .get_mut(&key)
            .and_then(|items| items.get_mut(position))
            .ok_or_else(invalid_items)?;
        let json: String = row.get(2).map_err(db_error)?;
        *item = serde_json::from_str(&json).map_err(|_| invalid_items())?;
    }
    for (key, values) in updated {
        entries.insert(
            key,
            serde_json::to_string(&values).map_err(|_| invalid_items())?,
        );
    }
    Ok(())
}

pub(super) fn write(connection: &Connection, items: &[(&str, usize, String)]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let mut statement = connection
        .prepare_cached(
            "INSERT INTO app_state_items(state_key, position, value_json)
         SELECT ?1, ?2, ?3 WHERE EXISTS (
             SELECT 1 FROM app_state WHERE key = ?1
         ) ON CONFLICT(state_key, position) DO UPDATE SET value_json = excluded.value_json",
        )
        .map_err(db_error)?;
    for (key, position, json) in items {
        if !inventory_key(key) || json.len() > MAX_STATE_JSON_BYTES {
            return Err(invalid_items());
        }
        let position = i64::try_from(*position).map_err(|_| invalid_items())?;
        let changed = statement
            .execute(params![key, position, json])
            .map_err(db_error)?;
        if changed != 1 {
            return Err(invalid_items());
        }
    }
    Ok(())
}
