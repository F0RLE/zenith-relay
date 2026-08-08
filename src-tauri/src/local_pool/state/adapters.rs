use super::DesktopState;
use crate::local_pool::{
    accounts::{
        authority::{AccountMetadataSink, MetadataSinkError},
        oauth_flow::{OAuthFlowEvent, OAuthFlowEventSink, OAuthFlowManager, OAuthFlowStatus},
        NativeSecretBackend,
    },
    response_affinity::DesktopResponseAffinityStore,
    store::LocalPoolStore,
    usage_writer::{DesktopUsageWriter, DesktopUsageWriterParts},
};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tauri::{Emitter, Manager, UserAttentionType};
use zenith_relay_core::{ResponseAffinityStore, UsageCallback};

impl DesktopState {
    pub(crate) fn account_metadata_sink(&self) -> Arc<StoreAccountMetadata> {
        Arc::new(StoreAccountMetadata {
            store: self.store.clone(),
        })
    }

    pub(crate) fn oauth_flow(&self) -> OAuthFlowManager<NativeSecretBackend, DesktopOAuthEvents> {
        self.oauth_flow.clone()
    }

    pub(crate) fn set_app_handle(&self, app: tauri::AppHandle) {
        self.oauth_events.set_app_handle(app);
    }

    pub fn usage_callback(&self) -> UsageCallback {
        DesktopUsageWriter::new(DesktopUsageWriterParts {
            telemetry: self.telemetry.clone(),
            store: self.store.clone(),
            quota_refresh: self.quota_refresh.clone(),
            quota_refresh_notify: self.quota_refresh_notify.clone(),
            wake: self.wake.clone(),
            failed: self.failed_usage_writes.clone(),
            wake_notify: self.wake_notify.clone(),
            state_events: self.oauth_events.clone(),
        })
        .callback()
    }

    pub(crate) fn response_affinity_store(&self) -> Arc<dyn ResponseAffinityStore> {
        Arc::new(DesktopResponseAffinityStore::new(
            self.telemetry.clone(),
            self.failed_affinity_writes.clone(),
        ))
    }
}

pub(crate) struct StoreAccountMetadata {
    store: Arc<Mutex<LocalPoolStore>>,
}

#[derive(Clone, Default)]
pub(crate) struct DesktopOAuthEvents {
    app: Arc<Mutex<Option<tauri::AppHandle>>>,
}

impl DesktopOAuthEvents {
    fn set_app_handle(&self, app: tauri::AppHandle) {
        if let Ok(mut current) = self.app.lock() {
            *current = Some(app);
        }
    }

    pub(in crate::local_pool) fn emit_state_changed(&self) {
        if let Some(app) = self.app.lock().ok().and_then(|app| app.clone()) {
            let _ = app.emit("zenith-state-changed", ());
        }
    }
}

impl OAuthFlowEventSink for DesktopOAuthEvents {
    fn emit(&self, event: OAuthFlowEvent) {
        let app = self.app.lock().ok().and_then(|app| app.clone());
        if let Some(app) = app {
            if event.status == OAuthFlowStatus::CallbackReceived {
                crate::tray::show_main_window(&app);
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.request_user_attention(Some(UserAttentionType::Informational));
                }
            }
            let _ = app.emit("relay-oauth-status", event);
        }
    }
}

impl AccountMetadataSink for StoreAccountMetadata {
    fn persist_generation<'a>(
        &'a self,
        local_account_id: &'a str,
        generation: u64,
        updated_at_ms: u64,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), MetadataSinkError>> + Send + 'a>> {
        Box::pin(async move {
            let mut store = self.store.lock().map_err(|_| MetadataSinkError)?;
            let mut account = store
                .account(local_account_id)
                .cloned()
                .ok_or(MetadataSinkError)?;
            account.account.token_generation = generation;
            account.account.token_updated_at_ms = Some(updated_at_ms);
            store.upsert_account(account).map_err(|_| MetadataSinkError)
        })
    }

    fn persist_auth_state<'a>(
        &'a self,
        local_account_id: &'a str,
        auth_state: zenith_relay_core::accounts::AccountAuthState,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), MetadataSinkError>> + Send + 'a>> {
        Box::pin(async move {
            let mut store = self.store.lock().map_err(|_| MetadataSinkError)?;
            let mut account = store
                .account(local_account_id)
                .cloned()
                .ok_or(MetadataSinkError)?;
            account.account.auth_state = auth_state;
            store.upsert_account(account).map_err(|_| MetadataSinkError)
        })
    }
}
