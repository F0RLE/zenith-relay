use super::DesktopState;
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError},
    models::{CatalogRefreshClient, ClientCatalogRefreshFailure, GatewaySettings},
    profiles::{ClientCatalogRefreshResults, CodexCatalogRefreshStatus},
};
use std::{collections::BTreeMap, sync::Arc};
use zenith_relay_core::error_codes;

#[derive(Clone, Default)]
pub(in crate::local_pool) struct ClientCatalogRefreshFence {
    codex: Arc<()>,
    claude_desktop: Arc<()>,
    opencode: Arc<()>,
}

impl ClientCatalogRefreshFence {
    fn client_mut(&mut self, client: CatalogRefreshClient) -> &mut Arc<()> {
        match client {
            CatalogRefreshClient::ClaudeDesktop => &mut self.claude_desktop,
            CatalogRefreshClient::OpenCode => &mut self.opencode,
        }
    }

    fn client(&self, client: CatalogRefreshClient) -> &Arc<()> {
        match client {
            CatalogRefreshClient::ClaudeDesktop => &self.claude_desktop,
            CatalogRefreshClient::OpenCode => &self.opencode,
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct CatalogRefreshWarnings {
    codex: Option<String>,
    codex_at_ms: Option<u64>,
    clients: BTreeMap<CatalogRefreshClient, ClientCatalogRefreshFailure>,
    fence: ClientCatalogRefreshFence,
}

impl CatalogRefreshWarnings {
    pub(super) fn from_gateway(gateway: &GatewaySettings) -> Self {
        Self {
            codex: gateway.catalog_refresh_error.clone(),
            codex_at_ms: gateway.catalog_refresh_error_at_ms,
            clients: gateway.client_catalog_refresh_failures.clone(),
            ..Self::default()
        }
    }
}

impl DesktopState {
    pub(crate) fn record_catalog_refresh_result(&self, error: Option<&LocalPoolError>) {
        let error_code = error.map(|error| catalog_refresh_error_code(error.code));
        let warning = error_code.map(|code| format!("model_catalog_refresh_failed:{code}"));
        self.update_catalog_refresh_warnings(|warnings| {
            warnings.fence.codex = Arc::new(());
            warnings.codex = warning;
            warnings.codex_at_ms = error_code.map(|_| super::now_ms());
            true
        });
    }

    pub(crate) fn record_catalog_refresh_deferred(&self) {
        self.update_catalog_refresh_warnings(|warnings| {
            warnings.fence.codex = Arc::new(());
            warnings.codex = Some("model_catalog_refresh_deferred:codex_running".to_string());
            warnings.codex_at_ms = None;
            true
        });
    }

    pub(in crate::local_pool) fn begin_client_catalog_refresh(
        &self,
    ) -> Option<ClientCatalogRefreshFence> {
        // Use the publication order so a pending warning write cannot replace
        // this new fence with the cloned state from an older operation.
        let _store = self.store().ok()?;
        let mut warnings = self.catalog_refresh_state.lock().ok()?;
        warnings.fence = ClientCatalogRefreshFence::default();
        Some(warnings.fence.clone())
    }

    pub(in crate::local_pool) fn record_client_catalog_refresh_results(
        &self,
        fence: &ClientCatalogRefreshFence,
        results: &ClientCatalogRefreshResults,
    ) {
        let recorded_at_ms = super::now_ms();
        self.update_catalog_refresh_warnings(|warnings| {
            let mut changed = false;
            if Arc::ptr_eq(&warnings.fence.codex, &fence.codex) {
                warnings.fence.codex = Arc::new(());
                changed = true;
                match &results.codex {
                    Ok(CodexCatalogRefreshStatus::Deferred) => {
                        warnings.codex =
                            Some("model_catalog_refresh_deferred:codex_running".into());
                        warnings.codex_at_ms = None;
                    }
                    Ok(_) => {
                        warnings.codex = None;
                        warnings.codex_at_ms = None;
                    }
                    Err(error) => {
                        warnings.codex = Some(format!(
                            "model_catalog_refresh_failed:{}",
                            catalog_refresh_error_code(error.code)
                        ));
                        warnings.codex_at_ms = Some(recorded_at_ms);
                    }
                }
            }
            for (client, result) in [
                (CatalogRefreshClient::ClaudeDesktop, &results.claude_desktop),
                (CatalogRefreshClient::OpenCode, &results.opencode),
            ] {
                if !Arc::ptr_eq(warnings.fence.client(client), fence.client(client)) {
                    continue;
                }
                *warnings.fence.client_mut(client) = Arc::new(());
                changed = true;
                warnings.clients.remove(&client);
                if let Err(error) = result {
                    warnings.clients.insert(
                        client,
                        ClientCatalogRefreshFailure {
                            code: error.code,
                            recorded_at_ms,
                        },
                    );
                }
            }
            changed
        });
    }

    pub(in crate::local_pool) fn clear_client_catalog_refresh_failure(
        &self,
        client: CatalogRefreshClient,
    ) {
        self.update_catalog_refresh_warnings(|warnings| {
            *warnings.fence.client_mut(client) = Arc::new(());
            warnings.clients.remove(&client);
            true
        });
    }

    fn update_catalog_refresh_warnings(
        &self,
        update: impl FnOnce(&mut CatalogRefreshWarnings) -> bool,
    ) {
        // The store guard orders persistence and in-memory publication together.
        // In-memory warnings remain best effort if storage is unavailable.
        let Ok(mut store) = self.store() else {
            if let Ok(mut warnings) = self.catalog_refresh_state.lock() {
                update(&mut warnings);
            }
            return;
        };
        let Ok(mut warnings) = self.catalog_refresh_state.lock().map(|slot| slot.clone()) else {
            return;
        };
        if !update(&mut warnings) {
            return;
        }
        let mut gateway = store.gateway().clone();
        gateway.catalog_refresh_error = warnings.codex.clone();
        gateway.catalog_refresh_error_at_ms = warnings.codex_at_ms;
        gateway.client_catalog_refresh_failures = warnings.clients.clone();
        let _ = store.replace_gateway(gateway);
        if let Ok(mut slot) = self.catalog_refresh_state.lock() {
            *slot = warnings;
        }
    }

    pub(crate) fn catalog_refresh_warning(&self) -> Option<String> {
        self.catalog_refresh_state
            .lock()
            .ok()
            .and_then(|slot| slot.codex.clone())
    }

    pub(crate) fn catalog_refresh_warnings(&self) -> Vec<String> {
        let Ok(state) = self.catalog_refresh_state.lock() else {
            return Vec::new();
        };
        let mut warnings: Vec<_> = state.codex.iter().cloned().collect();
        warnings.extend(state.clients.iter().map(|(client, failure)| {
            format!(
                "client_catalog_refresh_failed:{}:{}",
                client.as_str(),
                catalog_refresh_error_code(failure.code)
            )
        }));
        warnings
    }
}

fn catalog_refresh_error_code(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Io => error_codes::IO,
        ErrorCode::Conflict => error_codes::CONFLICT,
        ErrorCode::GatewayUnavailable => error_codes::GATEWAY_UNAVAILABLE,
        ErrorCode::PoolModelsUnavailable => error_codes::POOL_MODELS_UNAVAILABLE,
        ErrorCode::SourceTestFailed => error_codes::SOURCE_TEST_FAILED,
        ErrorCode::InvalidState => error_codes::INVALID_STATE,
        ErrorCode::NotFound => error_codes::NOT_FOUND,
        ErrorCode::ProfileRestoreBlocked => error_codes::PROFILE_RESTORE_BLOCKED,
        ErrorCode::RecoveryRequired => error_codes::RECOVERY_REQUIRED,
        ErrorCode::SecretStoreUnavailable => error_codes::SECRET_STORE_UNAVAILABLE,
        ErrorCode::UnsupportedSchema => error_codes::UNSUPPORTED_SCHEMA,
    }
}
