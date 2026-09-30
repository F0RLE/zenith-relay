use super::*;

#[test]
fn repair_reconciles_the_current_desktop_catalog_for_the_transferred_chat_only() {
    let (root, state, backups, profile, _rollout, database) = fixture("current-catalog");
    let catalog = profile.join("sqlite").join("codex-dev.db");
    fs::create_dir_all(catalog.parent().unwrap()).unwrap();
    let connection = Connection::open(&catalog).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE local_thread_catalog(\
                 host_id TEXT NOT NULL,\
                 thread_id TEXT NOT NULL,\
                 model_provider TEXT,\
                 missing_candidate INTEGER NOT NULL DEFAULT 0,\
                 PRIMARY KEY(host_id, thread_id)\
             );\
             CREATE TABLE local_thread_catalog_metadata(\
                 id INTEGER PRIMARY KEY,\
                 catalog_revision INTEGER\
             );\
             INSERT INTO local_thread_catalog_metadata(id, catalog_revision) VALUES (1, 7);\
             INSERT INTO local_thread_catalog(host_id, thread_id, model_provider, missing_candidate)\
                 VALUES ('local', 'thread-test', '', 1);\
             INSERT INTO local_thread_catalog(host_id, thread_id, model_provider, missing_candidate)\
                 VALUES ('local', 'thread-unrelated', 'openai', 1);",
        )
        .unwrap();
    drop(connection);

    let preview = preview(
        &state,
        std::slice::from_ref(&profile),
        TargetProvider::ZenithRelayLocal,
        false,
    )
    .unwrap();
    assert_eq!(preview.sqlite_row_count, 2);

    let applied = apply(&state, &backups, &preview.session_id).unwrap();
    assert_eq!(applied.sqlite_rows_changed, 2);
    assert_eq!(database_provider(&database), "zenith_relay_local");
    assert_eq!(
        catalog_thread_provider(&catalog, "thread-test"),
        "zenith_relay_local"
    );
    assert_eq!(catalog_missing_candidate(&catalog, "thread-test"), 0);
    assert_eq!(catalog_revision(&catalog), 8);
    assert_eq!(
        catalog_thread_provider(&catalog, "thread-unrelated"),
        "openai"
    );
    assert_eq!(catalog_missing_candidate(&catalog, "thread-unrelated"), 1);

    rollback(&backups, &applied.backup_id).unwrap();
    assert_eq!(catalog_thread_provider(&catalog, "thread-test"), "");
    assert_eq!(catalog_missing_candidate(&catalog, "thread-test"), 1);
    assert_eq!(catalog_revision(&catalog), 7);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repair_reconciles_catalog_rows_with_a_legacy_null_host_id() {
    let (root, state, backups, profile, _rollout, _database) = fixture("null-catalog-host");
    let catalog = profile.join("sqlite").join("codex-dev.db");
    fs::create_dir_all(catalog.parent().unwrap()).unwrap();
    let connection = Connection::open(&catalog).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE local_thread_catalog(\
                 host_id TEXT,\
                 thread_id TEXT NOT NULL,\
                 model_provider TEXT,\
                 missing_candidate INTEGER NOT NULL DEFAULT 0,\
                 PRIMARY KEY(host_id, thread_id)\
             );\
             CREATE TABLE local_thread_catalog_metadata(\
                 id INTEGER PRIMARY KEY,\
                 catalog_revision INTEGER\
             );\
             INSERT INTO local_thread_catalog_metadata(id, catalog_revision) VALUES (1, 4);\
             INSERT INTO local_thread_catalog(host_id, thread_id, model_provider, missing_candidate)\
                 VALUES (NULL, 'thread-test', 'openai', 1);",
        )
        .unwrap();
    drop(connection);

    let preview = preview(
        &state,
        std::slice::from_ref(&profile),
        TargetProvider::ZenithRelayLocal,
        false,
    )
    .unwrap();
    assert_eq!(preview.sqlite_row_count, 2);

    let applied = apply(&state, &backups, &preview.session_id).unwrap();
    assert_eq!(applied.sqlite_rows_changed, 2);

    let connection = Connection::open(&catalog).unwrap();
    let provider: String = connection
        .query_row(
            "SELECT model_provider FROM local_thread_catalog \
             WHERE host_id IS NULL AND thread_id = 'thread-test'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let missing_candidate: i64 = connection
        .query_row(
            "SELECT missing_candidate FROM local_thread_catalog \
             WHERE host_id IS NULL AND thread_id = 'thread-test'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);
    assert_eq!(provider, "zenith_relay_local");
    assert_eq!(missing_candidate, 0);
    assert_eq!(catalog_revision(&catalog), 5);

    rollback(&backups, &applied.backup_id).unwrap();
    let connection = Connection::open(&catalog).unwrap();
    let restored_provider: String = connection
        .query_row(
            "SELECT model_provider FROM local_thread_catalog \
             WHERE host_id IS NULL AND thread_id = 'thread-test'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);
    assert_eq!(restored_provider, "openai");
    assert_eq!(catalog_revision(&catalog), 4);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repair_completes_a_partial_transfer_when_rollouts_already_match_the_target() {
    let (root, state, backups, profile, rollout, database) = fixture("partial-catalog");
    let content = fs::read_to_string(&rollout).unwrap();
    fs::write(
        &rollout,
        content.replace(
            "\"model_provider\":\"openai\"",
            "\"model_provider\":\"zenith_relay_local\"",
        ),
    )
    .unwrap();
    let catalog = profile.join("sqlite").join("codex-dev.db");
    fs::create_dir_all(catalog.parent().unwrap()).unwrap();
    let connection = Connection::open(&catalog).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE local_thread_catalog(\
                 host_id TEXT NOT NULL,\
                 thread_id TEXT NOT NULL,\
                 model_provider TEXT,\
                 missing_candidate INTEGER NOT NULL DEFAULT 0,\
                 PRIMARY KEY(host_id, thread_id)\
             );\
             INSERT INTO local_thread_catalog(host_id, thread_id, model_provider, missing_candidate)\
                 VALUES ('local', 'thread-test', 'openai', 1);",
        )
        .unwrap();
    drop(connection);

    let preview = preview(
        &state,
        std::slice::from_ref(&profile),
        TargetProvider::ZenithRelayLocal,
        false,
    )
    .unwrap();
    assert_eq!(preview.rollout_record_count, 0);
    assert_eq!(preview.sqlite_row_count, 2);

    let applied = apply(&state, &backups, &preview.session_id).unwrap();
    assert_eq!(applied.rollout_records_changed, 0);
    assert_eq!(applied.sqlite_rows_changed, 2);
    assert_eq!(database_provider(&database), "zenith_relay_local");
    assert_eq!(
        catalog_thread_provider(&catalog, "thread-test"),
        "zenith_relay_local"
    );
    assert_eq!(catalog_missing_candidate(&catalog, "thread-test"), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn changed_rollout_is_rejected_before_backup_or_write() {
    let (root, state, backups, profile, rollout, database) = fixture("changed");
    let preview = preview(&state, &[profile], TargetProvider::ZenithRelayLocal, false).unwrap();
    let mut file = OpenOptions::new().append(true).open(&rollout).unwrap();
    writeln!(file, "{{\"type\":\"event_msg\",\"payload\":{{}}}}").unwrap();

    let error = apply(&state, &backups, &preview.session_id).unwrap_err();
    assert!(error.contains("changed after repair preview"));
    assert_eq!(rollout_provider_from_file(&rollout), "openai");
    assert_eq!(database_provider(&database), "openai");
    assert!(!backups.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn large_rollout_provider_repair_streams_the_payload() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-large-rollout-{}",
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&root).unwrap();
    let rollout = root.join("rollout-large.jsonl");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&rollout)
        .unwrap();
    writeln!(
        file,
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"thread-large\",\"model_provider\":\"openai\"}}}}"
    )
    .unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    drop(file);

    let snapshot = scan_rollout(&rollout, "zenith_relay_local").unwrap();
    assert_eq!(snapshot.records, 1);
    rewrite_rollout(&rollout, "zenith_relay_local", 1).unwrap();
    assert_eq!(rollout_provider_from_file(&rollout), "zenith_relay_local");
    assert!(fs::metadata(&rollout).unwrap().len() > 64 * 1024 * 1024);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_rollout_provider_still_requires_repair() {
    assert_eq!(
        rollout_provider(b"{\"type\":\"session_meta\",\"payload\":{}}\n"),
        Some(None)
    );
}

#[test]
fn cleanup_keeps_latest_internal_backup_and_preserves_user_directories() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-repair-retention-{}",
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&root).unwrap();
    let ids = (1_u64..=4)
        .map(|index| format!("history_repair_{index:032x}"))
        .collect::<Vec<_>>();
    let now = now_ms();
    let created_at = [
        now.saturating_sub(HISTORY_REPAIR_BACKUP_TTL_MS + 1),
        now.saturating_sub(HISTORY_REPAIR_BACKUP_TTL_MS / 2),
        now.saturating_sub(HISTORY_REPAIR_BACKUP_TTL_MS / 4),
        now.saturating_sub(1),
    ];
    for (index, id) in ids.iter().enumerate() {
        let directory = root.join(id);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("manifest.json"),
            format!("{{\"createdAtMs\":{}}}", created_at[index]),
        )
        .unwrap();
    }
    let user_snapshot = root.join("snapshot_user_named");
    fs::create_dir_all(&user_snapshot).unwrap();

    assert_eq!(cleanup_history_repair_backups(&root).unwrap(), 3);
    assert!(!root.join(&ids[0]).exists());
    assert!(!root.join(&ids[1]).exists());
    assert!(!root.join(&ids[2]).exists());
    assert!(root.join(&ids[3]).exists());
    assert!(user_snapshot.exists());

    let preserved = root.join(&ids[0]);
    fs::create_dir_all(&preserved).unwrap();
    fs::write(preserved.join("manifest.json"), "{\"createdAtMs\":1}").unwrap();
    assert_eq!(
        cleanup_history_repair_backups_preserving(&root, Some(&ids[0])).unwrap(),
        1
    );
    assert!(root.join(&ids[0]).exists());
    assert!(!root.join(&ids[2]).exists());
    assert!(!root.join(&ids[3]).exists());
    assert!(user_snapshot.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn discard_removes_only_a_valid_internal_backup() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-repair-discard-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let backup_id = format!("history_repair_{}", uuid::Uuid::new_v4().simple());
    let backup = root.join(&backup_id);
    let unrelated = root.join("keep");
    fs::create_dir_all(&backup).unwrap();
    fs::create_dir_all(&unrelated).unwrap();

    discard(&root, &backup_id).unwrap();
    assert!(!backup.exists());
    assert!(discard(&root, "../keep").is_err());
    assert!(unrelated.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cleanup_removes_only_expired_valid_repair_previews() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-repair-preview-retention-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let directory = root.join("repair_previews");
    fs::create_dir_all(&directory).unwrap();
    let expired_id = format!("repair_{}", uuid::Uuid::new_v4().simple());
    let active_id = format!("repair_{}", uuid::Uuid::new_v4().simple());
    for (session_id, expires_at_ms) in [(&expired_id, 1), (&active_id, u64::MAX)] {
        let snapshot = RepairSnapshot {
            version: SNAPSHOT_VERSION,
            session_id: session_id.clone(),
            target_provider: "openai".into(),
            profile_roots: Vec::new(),
            rollout_files: Vec::new(),
            history_rollouts: Vec::new(),
            databases: Vec::new(),
            created_at_ms: 1,
            expires_at_ms,
        };
        fs::write(
            directory.join(format!("{session_id}.json")),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
    }
    fs::write(directory.join("repair_invalid.json"), "invalid").unwrap();

    assert_eq!(cleanup_expired_previews(&root).unwrap(), 1);
    assert!(!directory.join(format!("{expired_id}.json")).exists());
    assert!(directory.join(format!("{active_id}.json")).exists());
    assert!(directory.join("repair_invalid.json").exists());
    fs::remove_dir_all(root).unwrap();
}
