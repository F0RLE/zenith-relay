use crate::state::AppState;
use axum::routing::post;
use axum::Router;
use std::sync::Arc;

mod diagnose;
mod lifecycle;
mod settings;

use diagnose::diagnose_gateway;
use lifecycle::{start_gateway, stop_gateway};
use settings::{
    set_chatgpt_retry_until_available, set_codex_background_tasks, set_codex_websockets,
};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/diagnostics", post(diagnose_gateway))
        .route("/gateway/start", post(start_gateway))
        .route("/gateway/stop", post(stop_gateway))
        .route(
            "/gateway/codex-background-tasks",
            post(set_codex_background_tasks),
        )
        .route(
            "/gateway/chatgpt-retry-until-available",
            post(set_chatgpt_retry_until_available),
        )
        .route("/gateway/codex-websockets", post(set_codex_websockets))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use crate::{
        config::Config,
        store::{Store, Vault},
        test_fixtures::pooled_source,
    };
    use axum::extract::State;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[tokio::test]
    async fn stopping_gateway_retires_pending_dispatches_before_restart() {
        let root = TempDir::new().unwrap();
        let config = Config::for_test(root.path().into(), "127.0.0.1:0".parse().unwrap());
        let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
        let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
        let state = AppState::new(config, store, vault).unwrap();
        let source = pooled_source("stop-source", "test-model");
        state.store.save_source(&source).unwrap();
        state
            .vault
            .save(&source.secret_ref, "synthetic-key")
            .unwrap();
        state.rebuild_runtime().await.unwrap();
        let old = state.runtime().unwrap().unwrap();
        assert!(old
            .candidate_runtime_order()
            .iter()
            .any(|candidate| candidate.available));

        let _ = stop_gateway(State(state.clone())).await.unwrap();
        assert!(!state.store.gateway_enabled().unwrap());
        assert!(state.runtime().unwrap().is_none());
        assert!(old
            .candidate_runtime_order()
            .iter()
            .all(|candidate| !candidate.available));
        // Background catalog/availability jobs may request a rebuild while
        // stopped; they must not reopen the public gateway.
        state.rebuild_runtime().await.unwrap();
        assert!(state.runtime().unwrap().is_none());

        let _ = start_gateway(State(state.clone())).await.unwrap();
        let current = state.runtime().unwrap().unwrap();
        assert!(!Arc::ptr_eq(&old, &current));
        assert!(current
            .candidate_runtime_order()
            .iter()
            .any(|candidate| candidate.available));
        assert!(old
            .candidate_runtime_order()
            .iter()
            .all(|candidate| !candidate.available));
        state.shutdown_runtime().await.unwrap();
        state.refresh.shutdown().await;
    }
}
