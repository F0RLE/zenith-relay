use super::super::backup_original_config;
use super::*;
use crate::local_pool::{
    error::ErrorCode,
    profiles::opencode::config::{
        backup_name_path, backup_path, missing_marker_path, read_config,
        write_config_with_model_order,
    },
};
use serde_json::{Value, json};
use std::fs;

fn with_config(name: &str, check: impl FnOnce(&DesktopState, &Path)) {
    let root = crate::files::test_temp_root(name);
    let state = DesktopState::open(root.clone()).unwrap();
    let path = root.join("client/opencode.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    check(&state, &path);
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

fn write_value(path: &Path, value: Value) {
    write_config_with_model_order(path, value.as_object().unwrap(), &[]).unwrap();
}

#[test]
fn external_connection_changes_do_not_block_restore_or_replace_newer_settings() {
    with_config("opencode-restore-external-edit", |state, path| {
        write_value(
            path,
            json!({
                "provider": {"foreign": {"options": {"apiKey": "synthetic-original"}}},
                "model": "foreign/original", "theme": "before"
            }),
        );
        assert!(backup_original_config(state, path, Some("Before connection")).unwrap());
        let current = concat!(
            "{\n  // A newer user connection is preserved.\n",
            "  \"provider\": {\"foreign\": {\"options\": {\"apiKey\": \"synthetic-newer\"}}},\n",
            "  \"model\": \"foreign/newer\", \"theme\": \"after\", \"plugin\": [\"user-plugin\"]\n}\n"
        );
        fs::write(path, current).unwrap();
        assert!(restore_saved_config(state, path).unwrap());
        assert_eq!(fs::read_to_string(path).unwrap(), current);
        assert!(!backup_path(state).exists());
        assert!(!backup_name_path(state).exists());
        assert!(!restore_saved_config(state, path).unwrap());
    });
}

#[test]
fn restore_replaces_only_managed_connection_and_keeps_foreign_providers_and_preferences() {
    with_config("opencode-restore-managed", |state, path| {
        write_value(
            path,
            json!({
                "provider": {"original": {"options": {"apiKey": "synthetic-original"}}},
                "model": "original/model", "theme": "before"
            }),
        );
        backup_original_config(state, path, None).unwrap();
        write_value(
            path,
            json!({
                "provider": {
                    "zenith-relay": {"options": {"apiKey": "synthetic-relay"}},
                    "newer": {"options": {"apiKey": "synthetic-newer"}}
                },
                "model": "zenith-relay/test", "theme": "after", "plugin": ["user-plugin"]
            }),
        );
        assert!(restore_saved_config(state, path).unwrap());
        let restored = read_config(path).unwrap();
        assert!(restored["provider"].get("zenith-relay").is_none());
        assert_eq!(
            restored["provider"]["original"]["options"]["apiKey"],
            "synthetic-original"
        );
        assert_eq!(
            restored["provider"]["newer"]["options"]["apiKey"],
            "synthetic-newer"
        );
        assert_eq!(restored["model"], "original/model");
        assert_eq!(restored["theme"], "after");
        assert_eq!(restored["plugin"], json!(["user-plugin"]));
    });
}

#[test]
fn restore_keeps_unrelated_preferences_that_the_user_removed_absent() {
    with_config("opencode-restore-removed-preferences", |state, path| {
        write_value(
            path,
            json!({
                "theme": "original", "plugin": ["original-plugin"], "model": "original/model"
            }),
        );
        backup_original_config(state, path, None).unwrap();
        write_value(path, json!({"model": "zenith-relay/test"}));
        assert!(restore_saved_config(state, path).unwrap());
        assert_eq!(
            Value::Object(read_config(path).unwrap()),
            json!({"model": "original/model"})
        );
    });
}

#[test]
fn a_missing_current_file_restores_the_original_connection_and_selection() {
    with_config("opencode-restore-missing-current", |state, path| {
        let original = json!({
            "provider": {"original": {"options": {"apiKey": "synthetic-original"}}},
            "model": "original/model", "theme": "before"
        });
        write_value(path, original.clone());
        backup_original_config(state, path, None).unwrap();
        fs::remove_file(path).unwrap();
        assert!(restore_saved_config(state, path).unwrap());
        assert_eq!(Value::Object(read_config(path).unwrap()), original);
    });
}

#[test]
fn an_originally_missing_file_keeps_newer_settings_after_the_managed_connection_is_removed() {
    with_config("opencode-restore-missing-original", |state, path| {
        backup_original_config(state, path, Some("Before connection")).unwrap();
        let current = "{\n  // The user already disconnected Relay.\n  \"theme\": \"current\"\n}\n";
        fs::write(path, current).unwrap();
        assert!(restore_saved_config(state, path).unwrap());
        assert_eq!(fs::read_to_string(path).unwrap(), current);
        assert!(!missing_marker_path(state).exists());
        assert!(!backup_name_path(state).exists());
    });
}

#[test]
fn restoring_an_originally_missing_file_removes_only_managed_configuration() {
    with_config("opencode-restore-strip-managed", |state, path| {
        backup_original_config(state, path, None).unwrap();
        write_value(
            path,
            json!({
                "provider": {
                    "zenith-relay": {"options": {"apiKey": "synthetic-relay"}},
                    "foreign": {"options": {"apiKey": "synthetic-foreign"}}
                },
                "model": "zenith-relay/test", "theme": "current"
            }),
        );
        assert!(restore_saved_config(state, path).unwrap());
        assert_eq!(
            Value::Object(read_config(path).unwrap()),
            json!({
                "provider": {"foreign": {"options": {"apiKey": "synthetic-foreign"}}},
                "theme": "current"
            })
        );
    });
}

#[test]
fn invalid_current_or_snapshot_files_keep_the_recovery_record_and_current_bytes() {
    for (name, current, snapshot, expected_code) in [
        (
            "opencode-invalid-current",
            "{",
            None,
            ErrorCode::InvalidState,
        ),
        (
            "opencode-invalid-snapshot",
            "{}",
            Some("{"),
            ErrorCode::RecoveryRequired,
        ),
        (
            "opencode-invalid-snapshot-root",
            "{}",
            Some("[]"),
            ErrorCode::RecoveryRequired,
        ),
        (
            "opencode-unclosed-current-comment",
            "{} /* unfinished comment",
            None,
            ErrorCode::InvalidState,
        ),
        (
            "opencode-unclosed-snapshot-comment",
            "{}",
            Some("{} /* unfinished comment"),
            ErrorCode::RecoveryRequired,
        ),
    ] {
        with_config(name, |state, path| {
            write_value(path, json!({"theme": "original"}));
            backup_original_config(state, path, Some("Before connection")).unwrap();
            if let Some(snapshot) = snapshot {
                fs::write(backup_path(state), snapshot).unwrap();
            }
            fs::write(path, current).unwrap();
            let recovery = fs::read(backup_path(state)).unwrap();
            let error = restore_saved_config(state, path).unwrap_err();
            assert_eq!(error.code, expected_code);
            assert_eq!(fs::read_to_string(path).unwrap(), current);
            assert_eq!(fs::read(backup_path(state)).unwrap(), recovery);
            assert!(backup_name_path(state).exists());
        });
    }
}

#[test]
fn non_object_provider_settings_are_preserved_instead_of_silently_replaced() {
    for provider in [
        Value::Null,
        json!(false),
        json!(7),
        json!([]),
        json!("external-config"),
    ] {
        for original_is_missing in [false, true] {
            with_config("opencode-unmergeable-current-provider", |state, path| {
                if !original_is_missing {
                    write_value(path, json!({"provider": {"original": {}}}));
                }
                backup_original_config(state, path, Some("Before connection")).unwrap();
                write_value(path, json!({"provider": provider, "theme": "current"}));
                let current = fs::read(path).unwrap();
                let error = restore_saved_config(state, path).unwrap_err();
                assert_eq!(error.code, ErrorCode::InvalidState);
                assert_eq!(fs::read(path).unwrap(), current);
                assert!(backup_path(state).exists() || missing_marker_path(state).exists());
                assert!(backup_name_path(state).exists());
            });
        }
        with_config("opencode-unmergeable-snapshot-provider", |state, path| {
            write_value(path, json!({"theme": "original"}));
            backup_original_config(state, path, None).unwrap();
            let invalid = json!({"provider": provider});
            fs::write(backup_path(state), serde_json::to_vec(&invalid).unwrap()).unwrap();
            let current = fs::read(path).unwrap();
            let recovery = fs::read(backup_path(state)).unwrap();
            let error = restore_saved_config(state, path).unwrap_err();
            assert_eq!(error.code, ErrorCode::RecoveryRequired);
            assert_eq!(fs::read(path).unwrap(), current);
            assert_eq!(fs::read(backup_path(state)).unwrap(), recovery);
        });
    }
}

#[test]
fn changed_files_are_not_overwritten_or_consumed_after_preparation() {
    for original_is_missing in [false, true] {
        for change in [
            "config",
            "remove-config",
            "record",
            "remove-record",
            "name",
            "other-record",
        ] {
            with_config("opencode-restore-concurrent-edit", |state, path| {
                if !original_is_missing {
                    write_value(path, json!({"model": "original/model"}));
                }
                backup_original_config(state, path, Some("Before connection")).unwrap();
                write_value(path, json!({"model": "zenith-relay/test"}));
                let prepared = plan::prepare(state, path).unwrap().unwrap();
                let (record, other) = if original_is_missing {
                    (missing_marker_path(state), backup_path(state))
                } else {
                    (backup_path(state), missing_marker_path(state))
                };
                match change {
                    "config" => fs::write(path, "{\"theme\":\"external\"}\n").unwrap(),
                    "remove-config" => fs::remove_file(path).unwrap(),
                    "record" => fs::write(record, b"changed recovery record").unwrap(),
                    "remove-record" => fs::remove_file(record).unwrap(),
                    "name" => fs::write(backup_name_path(state), b"New snapshot name").unwrap(),
                    "other-record" => fs::write(other, b"new recovery record").unwrap(),
                    _ => unreachable!(),
                }
                let paths = [
                    path.to_path_buf(),
                    backup_path(state),
                    missing_marker_path(state),
                    backup_name_path(state),
                ];
                let expected: Vec<_> = paths.iter().map(|path| fs::read(path).ok()).collect();
                let error = prepared.apply().unwrap_err();
                assert_eq!(error.code, ErrorCode::Conflict);
                let actual: Vec<_> = paths.iter().map(|path| fs::read(path).ok()).collect();
                assert_eq!(actual, expected, "files changed after conflict: {change}");
            });
        }
    }
}

#[test]
fn a_new_config_created_after_preparation_keeps_its_contents_and_recovery() {
    with_config("opencode-restore-created-current", |state, path| {
        write_value(path, json!({"model": "original/model"}));
        backup_original_config(state, path, None).unwrap();
        fs::remove_file(path).unwrap();
        let prepared = plan::prepare(state, path).unwrap().unwrap();
        let external = "{\"model\":\"foreign/newer\"}\n";
        fs::write(path, external).unwrap();
        let error = prepared.apply().unwrap_err();
        assert_eq!(error.code, ErrorCode::Conflict);
        assert_eq!(fs::read_to_string(path).unwrap(), external);
        assert!(backup_path(state).exists());
    });
}

#[test]
fn inconsistent_recovery_records_are_preserved_without_touching_the_client() {
    for additional_backup in [false, true] {
        with_config("opencode-inconsistent-recovery", |state, path| {
            backup_original_config(state, path, Some("Before connection")).unwrap();
            write_value(path, json!({"model": "zenith-relay/test"}));
            if additional_backup {
                fs::write(backup_path(state), b"{}").unwrap();
            } else {
                fs::write(missing_marker_path(state), b"corrupt marker").unwrap();
            }
            let current = fs::read(path).unwrap();
            let recovery = fs::read(missing_marker_path(state)).unwrap();
            let error = restore_saved_config(state, path).unwrap_err();
            assert_eq!(error.code, ErrorCode::RecoveryRequired);
            assert_eq!(fs::read(path).unwrap(), current);
            assert_eq!(fs::read(missing_marker_path(state)).unwrap(), recovery);
            assert_eq!(backup_path(state).exists(), additional_backup);
            assert!(backup_name_path(state).exists());
        });
    }
}

#[test]
fn write_failure_keeps_the_config_and_recovery_available_for_retry() {
    with_config("opencode-restore-write-failure", |state, path| {
        write_value(path, json!({"model": "original/model"}));
        backup_original_config(state, path, Some("Before connection")).unwrap();
        write_value(path, json!({"model": "zenith-relay/test"}));
        let current = fs::read(path).unwrap();
        let recovery = fs::read(backup_path(state)).unwrap();
        crate::files::fail_next_atomic_write(path.to_path_buf());
        let error = restore_saved_config(state, path).unwrap_err();
        assert_eq!(error.code, ErrorCode::Io);
        assert_eq!(fs::read(path).unwrap(), current);
        assert_eq!(fs::read(backup_path(state)).unwrap(), recovery);
        assert!(backup_name_path(state).exists());
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        assert!(restore_saved_config(state, path).unwrap());
        assert_eq!(read_config(path).unwrap()["model"], "original/model");
    });
}
