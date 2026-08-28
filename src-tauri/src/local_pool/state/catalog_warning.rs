use super::DesktopState;
use crate::local_pool::error::{ErrorCode, LocalPoolError};

impl DesktopState {
    pub(crate) fn record_catalog_refresh_result(&self, error: Option<&LocalPoolError>) {
        let error_code = error.map(|error| catalog_refresh_error_code(error.code));
        let warning = error_code.map(|code| format!("model_catalog_refresh_failed:{code}"));
        self.persist_catalog_refresh_warning(warning, error_code.map(|_| super::now_ms()));
    }

    pub(crate) fn record_catalog_refresh_deferred(&self) {
        self.persist_catalog_refresh_warning(
            Some("model_catalog_refresh_deferred:codex_running".to_string()),
            None,
        );
    }

    fn persist_catalog_refresh_warning(&self, warning: Option<String>, at_ms: Option<u64>) {
        let Ok(mut slot) = self.catalog_refresh_error.lock() else {
            return;
        };
        *slot = warning.clone();
        drop(slot);

        let Ok(mut store) = self.store() else {
            return;
        };
        let mut gateway = store.gateway().clone();
        gateway.catalog_refresh_error = warning;
        gateway.catalog_refresh_error_at_ms = at_ms;
        let _ = store.replace_gateway(gateway);
    }

    pub(crate) fn catalog_refresh_warning(&self) -> Option<String> {
        self.catalog_refresh_error
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }
}

fn catalog_refresh_error_code(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Io => "io",
        ErrorCode::Conflict => "conflict",
        ErrorCode::GatewayUnavailable => "gateway_unavailable",
        ErrorCode::SourceTestFailed => "source_test_failed",
        ErrorCode::InvalidState => "invalid_state",
        ErrorCode::NotFound => "not_found",
        ErrorCode::ProfileRestoreBlocked => "profile_restore_blocked",
        ErrorCode::RecoveryRequired => "recovery_required",
        ErrorCode::SecretStoreUnavailable => "secret_store_unavailable",
        ErrorCode::UnsupportedSchema => "unsupported_schema",
    }
}
