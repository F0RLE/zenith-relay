use super::sqlite::{db_error, io_error, unix_time_ms};
use crate::state::SERVER_SCHEMA_VERSION;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

mod catalog;
use catalog::MIGRATIONS;

pub(super) fn read_schema_version(connection: &Connection) -> Result<u32, String> {
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
    let schema_version_text = connection
        .query_row(
            "SELECT value FROM metadata WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?;
    schema_version_text
        .as_deref()
        .unwrap_or("0")
        .parse::<u32>()
        .map_err(|_| "database schema version is invalid".to_string())
}

pub(super) fn apply_migrations(
    connection: &mut Connection,
    current_version: u32,
) -> Result<(), String> {
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
                        zenith_relay_core::usage::sql_u64(unix_time_ms())
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

pub(super) fn validate_migration_ledger(connection: &Connection) -> Result<(), String> {
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

pub(super) fn prepare_migration_backup(
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

pub(super) fn finish_migration(path: &Path) -> Result<(), String> {
    let marker_path = sibling_path(path, ".migration-in-progress");
    if marker_path.exists() {
        fs::remove_file(marker_path).map_err(io_error)?;
    }
    Ok(())
}

pub(super) fn recover_interrupted_migration(path: &Path) -> Result<(), String> {
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
    let failed_migration_path = sibling_path(target, ".failed-migration");
    for path in [&temporary, &failed_migration_path] {
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
        fs::rename(target, &failed_migration_path).map_err(io_error)?;
    }
    if let Err(error) = fs::rename(&temporary, target) {
        if failed_migration_path.exists() {
            let _ = fs::rename(&failed_migration_path, target);
        }
        return Err(io_error(error));
    }
    if failed_migration_path.exists() {
        fs::remove_file(failed_migration_path).map_err(io_error)?;
    }
    Ok(())
}

pub(super) use zenith_relay_core::path_with_suffix as sibling_path;

#[cfg(test)]
mod tests;
