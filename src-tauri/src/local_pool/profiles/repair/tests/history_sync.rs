use super::*;

#[test]
fn scan_reads_and_fingerprints_history_in_one_pass() {
    struct CountingReader<'a> {
        bytes: &'a [u8],
        consumed: &'a std::cell::Cell<usize>,
    }
    impl Read for CountingReader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = self.bytes.read(buffer)?;
            self.consumed.set(self.consumed.get() + count);
            Ok(count)
        }
    }
    let mut body = String::from("{\"type\":\"session_meta\",\"payload\":{\"id\":\"first\",\"model_provider\":\"openai\"}}\r\n");
    for _ in 0..5_000 {
        body.push_str("{\"type\":\"event_msg\",\"payload\":{\"type\":\"session_meta\",\"text\":\"synthetic\"}}\n");
    }
    // Also accept payload before the escaped type, with no final newline.
    body.push_str("{\"payload\":{\"id\":\"second\",\"model_provider\":\"openai\"},\"type\":\"session\\u005fmeta\"}");
    let consumed = std::cell::Cell::new(0);
    let mut digest = Sha256::new();
    let metadata = read_session_metadata_from(
        CountingReader {
            bytes: body.as_bytes(),
            consumed: &consumed,
        },
        Some(&mut digest),
    )
    .unwrap();
    assert_eq!(consumed.get(), body.len());
    assert_eq!(metadata.records.len(), 2);
    assert_eq!(metadata.records[0].separator, b"\r\n");
    assert_eq!(metadata.records[1].end, body.len() as u64);
    assert_eq!(
        hex::encode(digest.finalize()),
        hex::encode(Sha256::digest(body.as_bytes()))
    );
}

#[test]
fn preview_apply_and_rollback_repair_only_provider_metadata() {
    let (root, state, backups, profile, rollout, database) = fixture("round-trip");
    let preview = preview(
        &state,
        std::slice::from_ref(&profile),
        TargetProvider::ZenithRelayLocal,
        true,
    )
    .unwrap();
    assert_eq!(preview.profile_count, 1);
    assert_eq!(preview.rollout_file_count, 1);
    assert_eq!(preview.rollout_record_count, 1);
    assert_eq!(preview.sqlite_row_count, 1);
    assert!(preview.codex_running);
    assert!(!serde_json::to_string(&preview)
        .unwrap()
        .contains("synthetic-private-prompt"));

    let applied = apply(&state, &backups, &preview.session_id).unwrap();
    assert_eq!(applied.rollout_records_changed, 1);
    assert_eq!(applied.sqlite_rows_changed, 1);
    assert_eq!(rollout_provider_from_file(&rollout), "zenith_relay_local");
    assert_eq!(database_provider(&database), "zenith_relay_local");
    assert!(fs::read_to_string(&rollout)
        .unwrap()
        .contains("synthetic-private-prompt"));

    let rolled_back = rollback(&backups, &applied.backup_id).unwrap();
    assert_eq!(rolled_back.files_restored, 2);
    assert_eq!(rollout_provider_from_file(&rollout), "openai");
    assert_eq!(database_provider(&database), "openai");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn automatic_sync_supports_ready_api_and_reuses_matching_history() {
    let (root, state, backups, profile, rollout, database) = fixture("automatic-ready-api");
    let applied = synchronize(&state, &backups, &profile, TargetProvider::CodexLocalAccess)
        .unwrap()
        .unwrap();
    assert_eq!(rollout_provider_from_file(&rollout), "codex_local_access");
    assert_eq!(database_provider(&database), "codex_local_access");
    assert!(
        synchronize(&state, &backups, &profile, TargetProvider::CodexLocalAccess,)
            .unwrap()
            .is_none()
    );

    rollback(&backups, &applied.backup_id).unwrap();
    assert_eq!(rollout_provider_from_file(&rollout), "openai");
    assert_eq!(database_provider(&database), "openai");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn matching_history_does_not_consume_the_rewrite_budget() {
    let (root, state, backups, profile, rollout, database) = fixture("rewrite-budget");
    let matching = profile.join("sessions").join("already-matching.jsonl");
    let matching_content = format!(
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"matching\",\"model_provider\":\"zenith_relay_local\"}}}}\n{}",
        "{\"type\":\"event_msg\",\"payload\":{\"text\":\"synthetic\"}}\n".repeat(100)
    );
    fs::write(&matching, &matching_content).unwrap();
    let rewrite_bytes = fs::metadata(&rollout).unwrap().len();
    assert!(matching_content.len() as u64 > rewrite_bytes);

    let preview = preview_with_rewrite_budget(
        &state,
        std::slice::from_ref(&profile),
        TargetProvider::ZenithRelayLocal,
        false,
        rewrite_bytes,
    )
    .unwrap();
    assert_eq!(preview.rollout_record_count, 1);
    assert_eq!(preview.sqlite_row_count, 1);
    let applied = apply(&state, &backups, &preview.session_id).unwrap();
    assert_eq!(rollout_provider_from_file(&rollout), "zenith_relay_local");
    assert_eq!(database_provider(&database), "zenith_relay_local");
    assert_eq!(fs::read_to_string(&matching).unwrap(), matching_content);
    let manifest: RepairManifest = serde_json::from_slice(
        &fs::read(backups.join(&applied.backup_id).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest
            .entries
            .iter()
            .filter(|entry| !entry.sqlite)
            .count(),
        1
    );

    let unchanged = preview_with_rewrite_budget(
        &state,
        std::slice::from_ref(&profile),
        TargetProvider::ZenithRelayLocal,
        false,
        0,
    )
    .unwrap();
    assert_eq!(unchanged.rollout_record_count, 0);
    assert_eq!(unchanged.sqlite_row_count, 0);
    rollback(&backups, &applied.backup_id).unwrap();
    assert_eq!(rollout_provider_from_file(&rollout), "openai");
    assert_eq!(fs::read_to_string(&matching).unwrap(), matching_content);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn history_rewrite_budget_still_rejects_oversized_changes_before_writing() {
    let (root, state, backups, profile, rollout, database) = fixture("rewrite-limit");
    let original = fs::read(&rollout).unwrap();
    let error = preview_with_rewrite_budget(
        &state,
        &[profile],
        TargetProvider::ZenithRelayLocal,
        false,
        original.len() as u64 - 1,
    )
    .unwrap_err();
    assert_eq!(error, "repair rollout data limit exceeded");
    assert_eq!(fs::read(&rollout).unwrap(), original);
    assert_eq!(database_provider(&database), "openai");
    assert!(!backups.exists());
    assert!(!state.join("repair_previews").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn automatic_sync_from_ready_api_to_chatgpt_repairs_the_latest_session_metadata() {
    let (root, state, backups, profile, rollout, database) = fixture("ready-api-to-chatgpt");
    synchronize(&state, &backups, &profile, TargetProvider::CodexLocalAccess)
        .unwrap()
        .expect("initial API migration");
    let mut file = OpenOptions::new().append(true).open(&rollout).unwrap();
    writeln!(
        file,
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"thread-test\",\"model_provider\":\"codex_local_access\",\"cwd\":\"C:/latest\"}}}}"
    )
    .unwrap();
    drop(file);

    let applied = synchronize(&state, &backups, &profile, TargetProvider::Openai)
        .unwrap()
        .expect("account migration");

    assert_eq!(applied.rollout_records_changed, 2);
    assert_eq!(database_provider(&database), "openai");
    assert_eq!(latest_rollout_provider_from_file(&rollout), "openai");
    assert_eq!(
        rollout_providers_from_file(&rollout),
        vec!["openai".to_string(), "openai".to_string()]
    );
    assert!(fs::read_to_string(&rollout)
        .unwrap()
        .contains("\"cwd\":\"C:/latest\""));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repair_rewrites_every_session_metadata_record_in_a_rollout() {
    let (root, state, backups, profile, rollout, database) = fixture("all-session-meta");
    let mut file = OpenOptions::new().append(true).open(&rollout).unwrap();
    for id in ["thread-middle", "thread-last"] {
        writeln!(
            file,
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"model_provider\":\"openai\"}}}}"
        )
        .unwrap();
    }
    drop(file);

    let preview = preview(
        &state,
        std::slice::from_ref(&profile),
        TargetProvider::ZenithRelayLocal,
        false,
    )
    .unwrap();
    assert_eq!(preview.rollout_record_count, 3);
    apply(&state, &backups, &preview.session_id).unwrap();
    assert_eq!(
        rollout_providers_from_file(&rollout),
        vec![
            "zenith_relay_local".to_string(),
            "zenith_relay_local".to_string(),
            "zenith_relay_local".to_string(),
        ]
    );
    fs::remove_dir_all(root).unwrap();
    let _ = database;
}

#[test]
fn repair_keeps_rollout_chronology_when_rewriting_provider_metadata() {
    let (root, state, backups, profile, rollout, _database) = fixture("preserve-rollout-time");
    let expected_modified = UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    OpenOptions::new()
        .write(true)
        .open(&rollout)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(expected_modified))
        .unwrap();

    let preview = preview(
        &state,
        std::slice::from_ref(&profile),
        TargetProvider::ZenithRelayLocal,
        false,
    )
    .unwrap();
    apply(&state, &backups, &preview.session_id).unwrap();

    assert_eq!(
        fs::metadata(&rollout)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap(),
        expected_modified.duration_since(UNIX_EPOCH).unwrap()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repair_does_not_update_threads_without_a_processed_rollout() {
    let (root, state, backups, profile, rollout, database) = fixture("linked-threads");
    let missing = profile.join("sessions/missing-rollout.jsonl");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "INSERT INTO threads(id, model_provider, rollout_path) VALUES ('thread-missing', 'openai', ?1)",
            [path_string(&missing)],
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
    assert_eq!(preview.sqlite_row_count, 1);
    apply(&state, &backups, &preview.session_id).unwrap();

    let connection = Connection::open(&database).unwrap();
    let missing_provider: String = connection
        .query_row(
            "SELECT model_provider FROM threads WHERE id='thread-missing'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);
    assert_eq!(database_provider(&database), "zenith_relay_local");
    assert_eq!(missing_provider, "openai");
    assert_eq!(rollout_provider_from_file(&rollout), "zenith_relay_local");
    fs::remove_dir_all(root).unwrap();
}
