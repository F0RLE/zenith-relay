use super::*;

pub(in crate::local_pool::profiles::repair) fn create_backup(
    directory: &Path,
    backup_id: &str,
    snapshot: &RepairSnapshot,
) -> Result<RepairManifest, String> {
    let mut backup_entries = Vec::new();
    for (index, rollout_file) in snapshot.rollout_files.iter().enumerate() {
        let relative = PathBuf::from("rollouts").join(format!("{index}.jsonl"));
        let target = directory.join(&relative);
        fs::create_dir_all(target.parent().unwrap()).map_err(io_error)?;
        fs::copy(&rollout_file.path, &target).map_err(io_error)?;
        sync_file(&target)?;
        backup_entries.push(BackupEntry {
            original_path: rollout_file.path.clone(),
            backup_path: path_string(&relative),
            sqlite: false,
        });
    }
    for (index, database_snapshot) in snapshot.databases.iter().enumerate() {
        let relative = PathBuf::from("databases").join(format!("{index}.sqlite"));
        let target = directory.join(&relative);
        fs::create_dir_all(target.parent().unwrap()).map_err(io_error)?;
        Connection::open(&database_snapshot.path)
            .map_err(db_error)?
            .backup(rusqlite::MAIN_DB, &target, None)
            .map_err(db_error)?;
        sync_file(&target)?;
        backup_entries.push(BackupEntry {
            original_path: database_snapshot.path.clone(),
            backup_path: path_string(&relative),
            sqlite: true,
        });
    }
    let manifest = RepairManifest {
        version: SNAPSHOT_VERSION,
        backup_id: backup_id.to_string(),
        profile_roots: snapshot.profile_roots.clone(),
        entries: backup_entries,
        created_at_ms: now_ms(),
    };
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|_| "repair backup manifest serialization failed".to_string())?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(directory.join("manifest.json"))
        .map_err(io_error)?;
    file.write_all(&bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    Ok(manifest)
}

pub(in crate::local_pool::profiles::repair) fn apply_snapshot(
    snapshot: &RepairSnapshot,
) -> Result<(), String> {
    for rollout_file in &snapshot.rollout_files {
        rewrite_rollout(
            Path::new(&rollout_file.path),
            &snapshot.target_provider,
            rollout_file.records,
        )?;
    }
    for database_snapshot in &snapshot.databases {
        let mut connection = Connection::open(&database_snapshot.path).map_err(db_error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(db_error)?;
        let thread_columns = super::scan::table_columns(&connection, "threads")?;
        let catalog_columns = super::scan::table_columns(&connection, "local_thread_catalog")?;
        let catalog_metadata_columns =
            super::scan::table_columns(&connection, "local_thread_catalog_metadata")?;
        if !database_snapshot.threads.is_empty()
            && !["id", "model_provider", "rollout_path"]
                .iter()
                .all(|column| thread_columns.contains(*column))
        {
            return Err("ChatGPT history database schema changed during repair".to_string());
        }
        if !database_snapshot.catalog_threads.is_empty()
            && !["host_id", "thread_id", "model_provider"]
                .iter()
                .all(|column| catalog_columns.contains(*column))
        {
            return Err("ChatGPT history database schema changed during repair".to_string());
        }
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;

        let mut changed = 0_usize;
        for thread in &database_snapshot.threads {
            let count = transaction
                .execute(
                    "UPDATE threads \
                     SET model_provider = ?1 \
                     WHERE id = ?2 \
                       AND rollout_path = ?3 \
                       AND COALESCE(model_provider, '') <> ?1",
                    rusqlite::params![snapshot.target_provider, thread.id, thread.rollout_path],
                )
                .map_err(db_error)?;
            changed = changed.saturating_add(count);
        }
        let mut catalog_changed = 0_usize;
        if !database_snapshot.catalog_threads.is_empty() {
            let changed_predicate = if catalog_columns.contains("missing_candidate") {
                "(COALESCE(model_provider, '') <> ?1 OR COALESCE(missing_candidate, 0) <> 0)"
            } else {
                "COALESCE(model_provider, '') <> ?1"
            };
            let query = format!(
                "UPDATE local_thread_catalog \
                 SET model_provider = ?1{} \
                 WHERE COALESCE(host_id, '') = ?2 \
                   AND thread_id = ?3 \
                   AND {changed_predicate}",
                if catalog_columns.contains("missing_candidate") {
                    ", missing_candidate = 0"
                } else {
                    ""
                }
            );
            for thread in &database_snapshot.catalog_threads {
                let count = transaction
                    .execute(
                        &query,
                        rusqlite::params![
                            snapshot.target_provider,
                            thread.host_id,
                            thread.thread_id
                        ],
                    )
                    .map_err(db_error)?;
                catalog_changed = catalog_changed.saturating_add(count);
            }
            changed = changed.saturating_add(catalog_changed);
            bump_local_thread_catalog_revision(
                &transaction,
                &catalog_metadata_columns,
                catalog_changed,
            )?;
        }
        let expected = database_snapshot
            .threads
            .len()
            .saturating_add(database_snapshot.catalog_threads.len());
        if changed != expected || expected != database_snapshot.rows {
            return Err("ChatGPT history database changed during repair".to_string());
        }
        transaction.commit().map_err(db_error)?;
    }
    Ok(())
}

fn bump_local_thread_catalog_revision(
    transaction: &rusqlite::Transaction<'_>,
    metadata_columns: &HashSet<String>,
    changed_rows: usize,
) -> Result<(), String> {
    if changed_rows == 0 || !metadata_columns.contains("catalog_revision") {
        return Ok(());
    }
    let updated = transaction
        .execute(
            "UPDATE local_thread_catalog_metadata \
             SET catalog_revision = COALESCE(catalog_revision, 0) + ?1",
            [changed_rows as i64],
        )
        .map_err(db_error)?;
    if updated == 0 && metadata_columns.contains("id") {
        transaction
            .execute(
                "INSERT INTO local_thread_catalog_metadata (id, catalog_revision) VALUES (1, ?1)",
                [changed_rows as i64],
            )
            .map_err(db_error)?;
    }
    Ok(())
}

pub(in crate::local_pool::profiles::repair) fn rewrite_rollout(
    path: &Path,
    target: &str,
    expected: usize,
) -> Result<(), String> {
    let metadata = super::scan::read_session_metadata(path)?;
    let mut replacements = super::scan::session_meta_replacements(&metadata, target);
    if replacements.is_empty() || replacements.len() != expected {
        return Err("ChatGPT rollout changed during repair".to_string());
    }
    let modified_at = fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    replace_file_with(path, false, move |temp_output| {
        let mut source_file = File::open(path).map_err(io_error)?;
        let mut position = 0_u64;
        for replacement in &mut replacements {
            if replacement.start < position || replacement.end < replacement.start {
                return Err("ChatGPT rollout changed during repair".to_string());
            }
            let prefix_len = replacement.start - position;
            let copied = {
                let mut prefix = Read::by_ref(&mut source_file).take(prefix_len);
                std::io::copy(&mut prefix, temp_output).map_err(io_error)?
            };
            if copied != prefix_len {
                return Err("ChatGPT rollout changed during repair".to_string());
            }
            let session_metadata = replacement
                .session_record
                .get_mut("payload")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| "ChatGPT session metadata is invalid".to_string())?;
            session_metadata.insert(
                "model_provider".to_string(),
                Value::String(target.to_string()),
            );
            let updated = serde_json::to_vec(&replacement.session_record)
                .map_err(|_| "ChatGPT session serialization failed".to_string())?;
            temp_output.write_all(&updated).map_err(io_error)?;
            temp_output
                .write_all(&replacement.separator)
                .map_err(io_error)?;
            if source_file
                .seek(SeekFrom::Start(replacement.end))
                .map_err(io_error)?
                != replacement.end
            {
                return Err("ChatGPT rollout changed during repair".to_string());
            }
            position = replacement.end;
        }
        std::io::copy(&mut source_file, temp_output).map_err(io_error)?;
        Ok(())
    })?;
    restore_modified_time(path, modified_at);
    Ok(())
}

fn restore_modified_time(path: &Path, modified_at: Option<SystemTime>) {
    let Some(modified_at) = modified_at else {
        return;
    };
    let _ = OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.set_times(fs::FileTimes::new().set_modified(modified_at)));
}

pub(in crate::local_pool::profiles::repair) fn restore_manifest(
    manifest: &RepairManifest,
    directory: &Path,
) -> Result<usize, String> {
    validate_manifest_paths(manifest, directory)?;
    let directory = fs::canonicalize(directory).map_err(io_error)?;
    let mut restored = 0;
    for backup_entry in &manifest.entries {
        let relative = Path::new(&backup_entry.backup_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err("repair backup path is invalid".to_string());
        }
        let backup = canonical_child(&directory, &directory.join(relative))?;
        let bytes = fs::read(backup).map_err(io_error)?;
        replace_file(
            Path::new(&backup_entry.original_path),
            &bytes,
            backup_entry.sqlite,
        )?;
        restored += 1;
    }
    Ok(restored)
}

pub(in crate::local_pool::profiles::repair) fn validate_manifest_paths(
    manifest: &RepairManifest,
    directory: &Path,
) -> Result<(), String> {
    let directory = portable_canonicalize(directory)?;
    let roots = manifest
        .profile_roots
        .iter()
        .map(|path| PathBuf::from(portable_path_value(path)))
        .collect::<Vec<_>>();
    for backup_entry in &manifest.entries {
        let original = portable_canonicalize(Path::new(&backup_entry.original_path))?;
        if !roots.iter().any(|root| original.starts_with(root)) {
            return Err("repair manifest path escaped its profile".to_string());
        }
        let relative = Path::new(&backup_entry.backup_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err("repair backup path is invalid".to_string());
        }
        let backup = canonical_child(&directory, &directory.join(relative))?;
        if !backup.is_file() {
            return Err("repair backup file is missing".to_string());
        }
    }
    Ok(())
}

fn replace_file(path: &Path, bytes: &[u8], sqlite: bool) -> Result<(), String> {
    replace_file_with(path, sqlite, |file| file.write_all(bytes).map_err(io_error))
}

fn replace_file_with(
    path: &Path,
    sqlite: bool,
    write: impl FnOnce(&mut File) -> Result<(), String>,
) -> Result<(), String> {
    let temporary = sibling_path(
        path,
        &format!(".repair-{}.tmp", uuid::Uuid::new_v4().simple()),
    );
    let previous_repair_path = sibling_path(path, ".repair-previous");
    if previous_repair_path.exists() {
        fs::remove_file(&previous_repair_path).map_err(io_error)?;
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(io_error)?;
    if let Err(error) = write(&mut file).and_then(|_| file.sync_all().map_err(io_error)) {
        drop(file);
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    drop(file);
    fs::rename(path, &previous_repair_path).map_err(io_error)?;
    if sqlite {
        for suffix in ["-wal", "-shm"] {
            let sidecar = sibling_path(path, suffix);
            if sidecar.exists() {
                if let Err(error) = fs::remove_file(sidecar) {
                    let _ = fs::rename(&previous_repair_path, path);
                    let _ = fs::remove_file(&temporary);
                    return Err(io_error(error));
                }
            }
        }
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::rename(&previous_repair_path, path);
        let _ = fs::remove_file(&temporary);
        return Err(io_error(error));
    }
    fs::remove_file(previous_repair_path).map_err(io_error)
}
