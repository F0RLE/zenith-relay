use super::*;

fn backdate_manifest(path: &Path) -> Vec<u8> {
    let mut record: RepairManifest = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    record.created_at_ms = now_ms().saturating_sub(5_000);
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(path, &bytes).unwrap();
    bytes
}

#[test]
fn oversized_repair_preview_is_not_published_or_allowed_to_replace_existing_work() {
    let (root, state, _, profile, _, _) = fixture("snapshot-size");
    let preview = preview(&state, &[profile], TargetProvider::ZenithRelayLocal, false).unwrap();
    let original_path = snapshot_path(&state, &preview.session_id).unwrap();
    let original = fs::read(&original_path).unwrap();
    let mut snapshot = load_snapshot(&state, &preview.session_id).unwrap();
    snapshot.session_id = format!("repair_{}", uuid::Uuid::new_v4().simple());
    let mut history = snapshot.rollout_files[0].clone();
    history.records = 0;
    history.session_ids = (0..65_536)
        .map(|index| format!("synthetic-repair-thread-{index:032x}"))
        .collect();
    history.session_meta_count = history.session_ids.len();
    snapshot.history_rollouts.push(history);
    assert!(serde_json::to_vec(&snapshot).unwrap().len() as u64 > MAX_REPAIR_MANIFEST_BYTES);

    let path = snapshot_path(&state, &snapshot.session_id).unwrap();
    let result = save_snapshot(&state, &snapshot);
    let published = path.exists();
    let preserved = fs::read(original_path).unwrap() == original;
    fs::remove_dir_all(root).unwrap();

    assert!(
        result.is_err(),
        "a preview beyond the read limit must never be published"
    );
    assert!(!published);
    assert!(preserved);
}

#[test]
fn oversized_repair_preview_is_rejected_before_parsing() {
    let (root, state, _, _, _, _) = fixture("snapshot-read-size");
    let session_id = format!("repair_{}", uuid::Uuid::new_v4().simple());
    let path = snapshot_path(&state, &session_id).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    File::create(&path)
        .unwrap()
        .set_len(MAX_REPAIR_MANIFEST_BYTES + 1)
        .unwrap();
    let error = load_snapshot(&state, &session_id).err().unwrap();
    fs::remove_dir_all(root).unwrap();
    assert!(error.contains("too large"));
}

#[test]
fn oversized_rollback_manifest_is_rejected_without_changing_destinations() {
    let (root, state, backups, profile, rollout, database) = fixture("rollback-manifest-size");
    let original_rollout = fs::read(&rollout).unwrap();
    let preview = preview(&state, &[profile], TargetProvider::ZenithRelayLocal, false).unwrap();
    let applied = apply(&state, &backups, &preview.session_id).unwrap();
    let repaired_rollout = fs::read(&rollout).unwrap();
    let repaired_database = fs::read(&database).unwrap();
    let manifest_path = backups.join(&applied.backup_id).join("manifest.json");
    let mut bytes = fs::read(&manifest_path).unwrap();
    assert!(bytes.len() as u64 <= MAX_REPAIR_MANIFEST_BYTES);
    bytes.resize(MAX_REPAIR_MANIFEST_BYTES as usize + 1, b' ');
    let _: RepairManifest = serde_json::from_slice(&bytes).unwrap();
    fs::write(&manifest_path, &bytes).unwrap();

    let result = rollback(&backups, &applied.backup_id);
    let destinations_unchanged = fs::read(&rollout).unwrap() == repaired_rollout
        && fs::read(&database).unwrap() == repaired_database;
    let manifest_unchanged = fs::read(&manifest_path).unwrap() == bytes;

    bytes.truncate(MAX_REPAIR_MANIFEST_BYTES as usize);
    fs::write(&manifest_path, &bytes).unwrap();
    let restored = rollback(&backups, &applied.backup_id).unwrap();
    let original_restored =
        fs::read(&rollout).unwrap() == original_rollout && database_provider(&database) == "openai";
    fs::remove_dir_all(root).unwrap();

    assert!(
        result.is_err(),
        "valid JSON beyond the manifest limit must be rejected"
    );
    assert!(
        destinations_unchanged,
        "rejected rollback must not restore any file"
    );
    assert!(
        manifest_unchanged,
        "rejection must retain the recovery manifest"
    );
    assert_eq!(restored.files_restored, 2);
    assert!(
        original_restored,
        "a manifest exactly at the limit must still restore"
    );
}

#[test]
fn oversized_backup_manifest_is_not_published_and_preserves_existing_recovery() {
    let (root, state, backups, profile, rollout, database) = fixture("backup-manifest-size");
    let original_rollout = fs::read(&rollout).unwrap();
    let preview = preview(&state, &[profile], TargetProvider::ZenithRelayLocal, false).unwrap();
    let mut snapshot = load_snapshot(&state, &preview.session_id).unwrap();
    let applied = apply(&state, &backups, &preview.session_id).unwrap();
    let repaired_rollout = fs::read(&rollout).unwrap();
    let repaired_database = fs::read(&database).unwrap();
    let existing_manifest_path = backups.join(&applied.backup_id).join("manifest.json");
    let existing_manifest = backdate_manifest(&existing_manifest_path);
    let profile_root = snapshot.profile_roots[0].clone();
    snapshot.profile_roots =
        vec![profile_root.clone(); MAX_REPAIR_MANIFEST_BYTES as usize / profile_root.len() + 1];
    assert!(
        serde_json::to_vec(&snapshot.profile_roots).unwrap().len() as u64
            > MAX_REPAIR_MANIFEST_BYTES
    );
    let backup_id = format!("history_repair_{}", uuid::Uuid::new_v4().simple());
    let directory = backups.join(&backup_id);

    let result = create_backup(&directory, &backup_id, &snapshot);
    let published = directory.join("manifest.json").exists();
    let directory_created = directory.exists();
    let destinations_unchanged = fs::read(&rollout).unwrap() == repaired_rollout
        && fs::read(&database).unwrap() == repaired_database;
    cleanup_history_repair_backups(&backups).unwrap();
    let existing_manifest_unchanged =
        fs::read(&existing_manifest_path).ok().as_deref() == Some(existing_manifest.as_slice());
    let restored = rollback(&backups, &applied.backup_id);
    let original_restored = restored.is_ok()
        && fs::read(&rollout).unwrap() == original_rollout
        && database_provider(&database) == "openai";
    fs::remove_dir_all(root).unwrap();

    assert!(
        result.is_err(),
        "a backup beyond the rollback read limit must not be published"
    );
    assert!(!published);
    assert!(
        !directory_created,
        "a size rejection must not leave a partial backup that displaces recovery"
    );
    assert!(destinations_unchanged);
    assert!(existing_manifest_unchanged);
    assert_eq!(restored.unwrap().files_restored, 2);
    assert!(
        original_restored,
        "existing recovery must remain usable after rejection"
    );
}

#[test]
fn failed_backup_copy_cannot_displace_existing_recovery() {
    let (root, state, backups, profile, _, _) = fixture("backup-copy-failure");
    let preview = preview(&state, &[profile], TargetProvider::ZenithRelayLocal, false).unwrap();
    let mut snapshot = load_snapshot(&state, &preview.session_id).unwrap();
    let applied = apply(&state, &backups, &preview.session_id).unwrap();
    let previous_manifest = backups.join(&applied.backup_id).join("manifest.json");
    let previous_bytes = backdate_manifest(&previous_manifest);
    snapshot.rollout_files[0].path = path_string(&root.join("missing-source.jsonl"));
    let failed_id = format!("history_repair_{}", uuid::Uuid::new_v4().simple());
    let failed_directory = backups.join(&failed_id);

    let result = create_backup(&failed_directory, &failed_id, &snapshot);
    cleanup_history_repair_backups(&backups).unwrap();
    let previous_preserved =
        fs::read(&previous_manifest).ok().as_deref() == Some(previous_bytes.as_slice());
    let restored = rollback(&backups, &applied.backup_id);
    let restored_count = restored
        .map(|result| result.files_restored)
        .unwrap_or_default();
    fs::remove_dir_all(root).unwrap();

    assert!(
        result.is_err(),
        "the missing source must fail backup creation"
    );
    assert!(
        previous_preserved,
        "retention must not replace a working backup with an incomplete attempt"
    );
    assert_eq!(restored_count, 2);
}
