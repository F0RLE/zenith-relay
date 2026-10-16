use super::*;

pub(in crate::local_pool::profiles::repair) fn collect_history_databases(
    root: &Path,
    seen: &mut HashSet<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let sqlite_directory = root.join("sqlite");
    let mut candidates = vec![
        root.join("state_5.sqlite"),
        sqlite_directory.join("state_5.sqlite"),
    ];
    if sqlite_directory.is_dir() {
        let mut discovered = Vec::new();
        for directory_entry in fs::read_dir(&sqlite_directory).map_err(io_error)? {
            let directory_entry = directory_entry.map_err(io_error)?;
            let path = directory_entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(io_error)?;
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && is_sqlite_database_path(&path)
            {
                discovered.push(path);
            }
        }
        discovered.sort();
        candidates.extend(discovered);
    }

    let mut databases = Vec::new();
    for path in candidates {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_error(error)),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        let path = canonical_child(root, &path)?;
        if !seen.insert(path.clone()) {
            continue;
        }
        if databases.len() >= MAX_DATABASE_FILES {
            return Err("repair history database limit exceeded".to_string());
        }
        databases.push(path);
    }
    Ok(databases)
}

pub(in crate::local_pool::profiles::repair) fn is_sqlite_database_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some(extension)
            if extension.eq_ignore_ascii_case("db")
                || extension.eq_ignore_ascii_case("sqlite")
                || extension.eq_ignore_ascii_case("sqlite3")
    )
}

pub(in crate::local_pool::profiles::repair) fn table_columns(
    connection: &Connection,
    table: &str,
) -> Result<HashSet<String>, String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(db_error)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(db_error)?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(db_error)?;
    Ok(columns)
}

pub(in crate::local_pool::profiles::repair) fn profile_root_for_path(
    profile_roots: &[String],
    path: &Path,
) -> Result<PathBuf, String> {
    let path = portable_canonicalize(path)?;
    profile_roots
        .iter()
        .map(|root| PathBuf::from(portable_path_value(root)))
        .find(|root| path.starts_with(root))
        .ok_or_else(|| "repair preview path escaped its profile".to_string())
}

pub(in crate::local_pool::profiles::repair) fn canonical_rollout_path(
    profile_root: &Path,
    rollout_path: &str,
) -> Option<String> {
    let path = PathBuf::from(portable_path_value(rollout_path));
    let path = if path.is_absolute() {
        path
    } else {
        profile_root.join(path)
    };
    fs::canonicalize(path).ok().map(|path| path_string(&path))
}

pub(in crate::local_pool::profiles::repair) fn scan_database(
    profile_root: &Path,
    path: &Path,
    target: &str,
    eligible_rollout_paths: &HashSet<String>,
    eligible_thread_ids: &HashSet<String>,
) -> Result<DatabaseSnapshot, String> {
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(db_error)?;
    let thread_columns = table_columns(&connection, "threads")?;
    let mut rows = Vec::new();
    if ["id", "model_provider", "rollout_path"]
        .iter()
        .all(|column| thread_columns.contains(*column))
    {
        let mut statement = connection
            .prepare(
                "SELECT id, COALESCE(model_provider, ''), rollout_path \
                 FROM threads \
                 WHERE COALESCE(model_provider, '') <> ?1 \
                 ORDER BY id",
            )
            .map_err(db_error)?;
        let candidates = statement
            .query_map([target], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        rows = candidates
            .into_iter()
            .filter_map(|(id, provider, rollout_path)| {
                canonical_rollout_path(profile_root, &rollout_path)
                    .filter(|canonical| eligible_rollout_paths.contains(canonical))
                    .map(|canonical| (id, provider, rollout_path, canonical))
            })
            .collect();
    }

    let catalog_columns = table_columns(&connection, "local_thread_catalog")?;
    let mut catalog_rows = Vec::new();
    if ["host_id", "thread_id", "model_provider"]
        .iter()
        .all(|column| catalog_columns.contains(*column))
    {
        let missing_candidate = if catalog_columns.contains("missing_candidate") {
            "COALESCE(missing_candidate, 0)"
        } else {
            "0"
        };
        let query = format!(
            "SELECT COALESCE(host_id, ''), thread_id, COALESCE(model_provider, ''), {missing_candidate} \
             FROM local_thread_catalog \
             WHERE COALESCE(thread_id, '') <> '' \
             ORDER BY host_id, thread_id"
        );
        let mut statement = connection.prepare(&query).map_err(db_error)?;
        let candidates = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        catalog_rows = candidates
            .into_iter()
            .filter(|(_, thread_id, provider, missing_candidate)| {
                eligible_thread_ids.contains(thread_id)
                    && (provider != target || *missing_candidate != 0)
            })
            .collect();
    }

    let mut hasher = Sha256::new();
    for (id, provider, rollout_path, _) in &rows {
        hasher.update(b"threads");
        for field_text in [id, provider, rollout_path] {
            hasher.update((field_text.len() as u64).to_le_bytes());
            hasher.update(field_text.as_bytes());
        }
    }
    for (host_id, thread_id, provider, missing_candidate) in &catalog_rows {
        hasher.update(b"local_thread_catalog");
        for field_text in [host_id, thread_id, provider] {
            hasher.update((field_text.len() as u64).to_le_bytes());
            hasher.update(field_text.as_bytes());
        }
        hasher.update(missing_candidate.to_le_bytes());
    }
    Ok(DatabaseSnapshot {
        path: path_string(path),
        hash: hex::encode(hasher.finalize()),
        rows: rows.len().saturating_add(catalog_rows.len()),
        threads: rows
            .iter()
            .map(|(id, _, rollout_path, _)| DatabaseThreadSnapshot {
                id: id.clone(),
                rollout_path: rollout_path.clone(),
            })
            .collect(),
        catalog_threads: catalog_rows
            .iter()
            .map(|(host_id, thread_id, _, _)| DatabaseCatalogThreadSnapshot {
                host_id: host_id.clone(),
                thread_id: thread_id.clone(),
            })
            .collect(),
    })
}
