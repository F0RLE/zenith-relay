use super::*;
use crate::local_pool::{
    models::CatalogRefreshClient,
    profiles::{ClientCatalogRefreshResults, CodexCatalogRefreshStatus},
};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
fn catalog_warning_is_published_with_its_store_transaction() {
    let root = temp_root("catalog-warning-publication");
    let state = DesktopState::open(root.clone()).unwrap();
    state.record_catalog_refresh_deferred();
    let previous = state.catalog_refresh_warning();
    let mut store = state.store().unwrap();
    let worker_state = state.clone();
    let (started, ready) = mpsc::sync_channel(0);
    let writer = std::thread::spawn(move || {
        started.send(()).unwrap();
        worker_state.record_catalog_refresh_result(Some(&LocalPoolError::new(
            ErrorCode::Io,
            "synthetic catalog error",
        )));
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let deadline = Instant::now() + Duration::from_millis(100);
    let mut published_before_commit = false;
    while Instant::now() < deadline {
        if state.catalog_refresh_warning() != previous {
            published_before_commit = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut gateway = store.gateway().clone();
    gateway.hidden_models = vec!["synthetic-visible-model".into()];
    store.replace_gateway(gateway).unwrap();
    drop(store);
    writer.join().unwrap();
    assert!(
        !published_before_commit,
        "memory must not publish an uncommitted warning"
    );
    let warning = state.catalog_refresh_warning();
    assert_eq!(warning.as_deref(), Some("model_catalog_refresh_failed:io"));
    assert_eq!(
        state.store().unwrap().gateway().catalog_refresh_error,
        warning
    );
    assert_eq!(
        state.store().unwrap().gateway().hidden_models,
        ["synthetic-visible-model"]
    );
    drop(state);
    let restored = DesktopState::open(root.clone()).unwrap();
    assert_eq!(restored.catalog_refresh_warning(), warning);
    assert_eq!(
        restored.store().unwrap().gateway().hidden_models,
        ["synthetic-visible-model"]
    );
    drop(restored);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn independent_client_failures_and_codex_defer_survive_restart_and_scoped_success() {
    let root = temp_root("client-catalog-outcomes");
    let state = DesktopState::open(root.clone()).unwrap();
    let results = ClientCatalogRefreshResults {
        claude_desktop: Err(LocalPoolError::new(
            ErrorCode::Io,
            "synthetic-private-claude-message",
        )),
        opencode: Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "synthetic-private-opencode-message",
        )),
        codex: Ok(CodexCatalogRefreshStatus::Deferred),
    };
    state.record_client_catalog_refresh_results(
        &state.begin_client_catalog_refresh().unwrap(),
        &results,
    );
    let expected = vec![
        "model_catalog_refresh_deferred:codex_running".to_string(),
        "client_catalog_refresh_failed:claude_desktop:io".to_string(),
        "client_catalog_refresh_failed:opencode:invalid_state".to_string(),
    ];
    assert_eq!(state.catalog_refresh_warnings(), expected);
    {
        let store = state.store().unwrap();
        let gateway = store.gateway();
        assert!(gateway.catalog_refresh_error_at_ms.is_none());
        let failures = &gateway.client_catalog_refresh_failures;
        assert_eq!(failures.len(), 2);
        assert_eq!(
            failures[&CatalogRefreshClient::ClaudeDesktop].code,
            ErrorCode::Io
        );
        assert_eq!(
            failures[&CatalogRefreshClient::OpenCode].code,
            ErrorCode::InvalidState
        );
        assert!(failures.values().all(|failure| failure.recorded_at_ms > 0));
        assert!(
            !serde_json::to_string(failures)
                .unwrap()
                .contains("synthetic-private")
        );
    }
    drop(state);
    let state = DesktopState::open(root.clone()).unwrap();
    assert_eq!(state.catalog_refresh_warnings(), expected);
    state.clear_client_catalog_refresh_failure(CatalogRefreshClient::ClaudeDesktop);
    assert_eq!(
        state.catalog_refresh_warnings(),
        [expected[0].clone(), expected[2].clone()]
    );
    assert!(state.catalog_refresh_warning().is_some());
    state.record_catalog_refresh_result(None);
    assert!(state.catalog_refresh_warning().is_none());
    assert_eq!(state.catalog_refresh_warnings(), expected[2..]);
    drop(state);
    let state = DesktopState::open(root.clone()).unwrap();
    assert_eq!(state.catalog_refresh_warnings(), expected[2..]);
    state.record_client_catalog_refresh_results(
        &state.begin_client_catalog_refresh().unwrap(),
        &ClientCatalogRefreshResults {
            claude_desktop: Ok(()),
            opencode: Ok(()),
            codex: Ok(CodexCatalogRefreshStatus::Updated),
        },
    );
    assert!(state.catalog_refresh_warnings().is_empty());
    assert!(
        state
            .store()
            .unwrap()
            .gateway()
            .client_catalog_refresh_failures
            .is_empty()
    );
    drop(state);
    let state = DesktopState::open(root.clone()).unwrap();
    assert!(state.catalog_refresh_warnings().is_empty());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn late_background_failures_do_not_undo_successful_reconnects() {
    let root = temp_root("client-catalog-late-failure");
    let state = DesktopState::open(root.clone()).unwrap();
    let fence = state.begin_client_catalog_refresh().unwrap();
    let pending = ClientCatalogRefreshResults {
        claude_desktop: Err(LocalPoolError::new(
            ErrorCode::Io,
            "synthetic Claude failure",
        )),
        opencode: Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "synthetic OpenCode failure",
        )),
        codex: Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "synthetic Codex failure",
        )),
    };

    state.clear_client_catalog_refresh_failure(CatalogRefreshClient::OpenCode);
    state.record_catalog_refresh_result(None);
    state.record_client_catalog_refresh_results(&fence, &pending);

    let expected = ["client_catalog_refresh_failed:claude_desktop:io"];
    assert_eq!(state.catalog_refresh_warnings(), expected);
    drop(state);
    let restored = DesktopState::open(root.clone()).unwrap();
    assert_eq!(restored.catalog_refresh_warnings(), expected);
    drop(restored);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reconnecting_claude_keeps_pending_peer_results_independent() {
    let root = temp_root("client-catalog-claude-reconnect");
    let state = DesktopState::open(root.clone()).unwrap();
    let fence = state.begin_client_catalog_refresh().unwrap();
    state.clear_client_catalog_refresh_failure(CatalogRefreshClient::ClaudeDesktop);
    state.record_client_catalog_refresh_results(
        &fence,
        &ClientCatalogRefreshResults {
            claude_desktop: Err(LocalPoolError::new(ErrorCode::Io, "synthetic old error")),
            opencode: Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "synthetic current error",
            )),
            codex: Ok(CodexCatalogRefreshStatus::Deferred),
        },
    );
    let expected = [
        "model_catalog_refresh_deferred:codex_running",
        "client_catalog_refresh_failed:opencode:conflict",
    ];
    assert_eq!(state.catalog_refresh_warnings(), expected);
    drop(state);
    let restored = DesktopState::open(root.clone()).unwrap();
    assert_eq!(restored.catalog_refresh_warnings(), expected);
    drop(restored);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_success_and_replayed_batches_preserve_newer_failures_and_restart_requirement() {
    let root = temp_root("client-catalog-stale-success");
    let state = DesktopState::open(root.clone()).unwrap();
    let older = state.begin_client_catalog_refresh().unwrap();
    let newer = state.begin_client_catalog_refresh().unwrap();
    state.record_client_catalog_refresh_results(
        &newer,
        &ClientCatalogRefreshResults {
            claude_desktop: Err(LocalPoolError::new(
                ErrorCode::Io,
                "synthetic current error",
            )),
            opencode: Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "synthetic current error",
            )),
            codex: Ok(CodexCatalogRefreshStatus::Deferred),
        },
    );
    let expected = [
        "model_catalog_refresh_deferred:codex_running",
        "client_catalog_refresh_failed:claude_desktop:io",
        "client_catalog_refresh_failed:opencode:conflict",
    ];
    let persisted = serde_json::to_value(state.store().unwrap().gateway()).unwrap();
    let observer = rusqlite::Connection::open(root.join("data/database/relay.sqlite")).unwrap();
    observer
        .execute_batch(concat!(
            "CREATE TABLE synthetic_catalog_writes (operation TEXT NOT NULL); ",
            "CREATE TRIGGER observe_catalog_insert AFTER INSERT ON app_state ",
            "BEGIN INSERT INTO synthetic_catalog_writes VALUES ('insert'); END; ",
            "CREATE TRIGGER observe_catalog_update AFTER UPDATE ON app_state ",
            "BEGIN INSERT INTO synthetic_catalog_writes VALUES ('update'); END;",
        ))
        .unwrap();
    let success = ClientCatalogRefreshResults {
        claude_desktop: Ok(()),
        opencode: Ok(()),
        codex: Ok(CodexCatalogRefreshStatus::Updated),
    };
    for fence in [&older, &newer] {
        state.record_client_catalog_refresh_results(fence, &success);
        assert_eq!(state.catalog_refresh_warnings(), expected);
        assert_eq!(
            serde_json::to_value(state.store().unwrap().gateway()).unwrap(),
            persisted
        );
    }
    let writes: i64 = observer
        .query_row("SELECT COUNT(*) FROM synthetic_catalog_writes", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(writes, 0, "superseded batches must not write warning state");
    drop(observer);
    drop(state);
    let restored = DesktopState::open(root.clone()).unwrap();
    assert_eq!(restored.catalog_refresh_warnings(), expected);
    restored.record_client_catalog_refresh_results(
        &restored.begin_client_catalog_refresh().unwrap(),
        &success,
    );
    assert!(restored.catalog_refresh_warnings().is_empty());
    drop(restored);
    let reopened = DesktopState::open(root.clone()).unwrap();
    assert!(reopened.catalog_refresh_warnings().is_empty());
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_later_codex_save_preserves_client_failures_after_a_warning_write_failure() {
    let root = temp_root("client-catalog-write-failure");
    let state = DesktopState::open(root.clone()).unwrap();
    state.record_catalog_refresh_deferred();
    let path = root.join("data/database/relay.sqlite");
    assert!(path.is_file());
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch(concat!(
            "CREATE TRIGGER fail_catalog_insert BEFORE INSERT ON app_state ",
            "BEGIN SELECT RAISE(ABORT, 'synthetic warning failure'); END; ",
            "CREATE TRIGGER fail_catalog_update BEFORE UPDATE ON app_state ",
            "BEGIN SELECT RAISE(ABORT, 'synthetic warning failure'); END;",
        ))
        .unwrap();
    state.record_client_catalog_refresh_results(
        &state.begin_client_catalog_refresh().unwrap(),
        &ClientCatalogRefreshResults {
            claude_desktop: Err(LocalPoolError::new(
                ErrorCode::Io,
                "synthetic private message",
            )),
            opencode: Ok(()),
            codex: Ok(CodexCatalogRefreshStatus::Deferred),
        },
    );
    assert_eq!(state.catalog_refresh_warnings().len(), 2);
    assert!(
        state
            .store()
            .unwrap()
            .gateway()
            .client_catalog_refresh_failures
            .is_empty()
    );
    connection
        .execute_batch("DROP TRIGGER fail_catalog_insert; DROP TRIGGER fail_catalog_update;")
        .unwrap();
    state.record_catalog_refresh_result(None);
    assert!(state.catalog_refresh_warning().is_none());
    assert_eq!(
        state
            .store()
            .unwrap()
            .gateway()
            .client_catalog_refresh_failures
            .len(),
        1
    );
    drop(connection);
    drop(state);
    let restored = DesktopState::open(root.clone()).unwrap();
    assert_eq!(
        restored.catalog_refresh_warnings(),
        ["client_catalog_refresh_failed:claude_desktop:io"]
    );
    drop(restored);
    std::fs::remove_dir_all(root).unwrap();
}
