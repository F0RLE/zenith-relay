pub(crate) mod account_routes;
pub(crate) mod automations;
pub(crate) mod connections;
pub(crate) mod gateway;
pub(crate) mod oauth;
pub(crate) mod pool;
pub(crate) mod profiles;
pub(crate) mod proxies;
pub(crate) mod recovery;
pub(crate) mod state;
pub(crate) mod usage;

#[cfg(test)]
mod rotation_regression_test;
use super::runtime;

use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result as LocalResult},
    models::{GatewaySettings, LocalAccountRecord, ProviderSourceRecord},
};
use std::collections::BTreeMap;
use zenith_relay_core::{
    PriceEvidence, PricingContext, SourcePricingMetadata, SourceProtocolResolution, TokenPrice,
    cache_write_model_ids, model_id_key, model_metadata::ModelMetadataCatalog,
};

/// Move owned local I/O off both the window thread and async executor. Await
/// completion so callers retain the operation's result and error semantics.
pub(in crate::local_pool) async fn run_blocking<T: Send + 'static>(
    work: impl FnOnce() -> LocalResult<T> + Send + 'static,
) -> LocalResult<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|_| LocalPoolError::new(ErrorCode::Io, "Local operation was interrupted"))?
}

pub(super) use super::store::secret_store::cleanup_created_secret;

/// Builds the immutable pricing identity used by all desktop projections.
/// Storage records contain only redacted/local identifiers and explicit
/// provider metadata; no pricing provider is inferred from a label or URL.
pub(super) fn pricing_context(
    gateway: &GatewaySettings,
    sources: &[ProviderSourceRecord],
    accounts: &[LocalAccountRecord],
    reference_catalog: &ModelMetadataCatalog,
) -> PricingContext {
    let account_provider_families = accounts
        .iter()
        .map(|account| {
            (
                account.account.id.clone(),
                account
                    .provider_family
                    .clone()
                    .unwrap_or_else(|| "openai".to_string()),
            )
        })
        .collect();
    let mut source_metadata = BTreeMap::new();
    let mut source_evidence = BTreeMap::new();
    for source in sources {
        source_metadata.insert(
            source.id.clone(),
            SourcePricingMetadata {
                pricing_provider: source.pricing_provider.clone(),
                official_provider_family: source.official_provider_family.clone(),
                cache_write_models: cache_write_model_ids(
                    SourceProtocolResolution::resolved_protocol_bindings_with_catalog(
                        source,
                        Some(reference_catalog),
                    )
                    .unwrap_or_default(),
                ),
            },
        );
        let mut evidence = BTreeMap::<String, PriceEvidence>::new();
        for (model, price) in &source.detected_model_prices {
            evidence.entry(model_id_key(model)).or_default().provider =
                Some(TokenPrice::from(*price));
        }
        for (model, price) in &source.model_price_overrides {
            evidence.entry(model_id_key(model)).or_default().manual =
                Some(TokenPrice::from(*price));
        }
        source_evidence.insert(source.id.clone(), evidence);
    }
    let global_manual_prices = gateway
        .model_price_overrides
        .iter()
        .map(|(model, price)| (model_id_key(model), TokenPrice::from(*price)))
        .collect();
    PricingContext {
        account_provider_families,
        source_metadata,
        source_evidence,
        global_manual_prices,
    }
}

pub(in crate::local_pool) use runtime::{
    apply_local_gateway_key_scope, apply_source_policies_if_running,
    apply_source_policy_if_running, core_error, current_time_ms, fail_closed,
    fence_runtime_candidates, queue_active_client_catalog_refresh, record_catalog_refresh_result,
    refresh_active_codex_catalog_in_background, refresh_local_gateway_key_scope_if_running,
    restart_after_secret_change, restart_or_rollback, runtime_account_policy, runtime_from_store,
    sync_gateway_or_rollback, sync_records_or_rollback, sync_running_account_states,
};

#[cfg(test)]
mod blocking_command_tests {
    use super::*;
    use std::{
        future::Future,
        sync::mpsc,
        task::{Context, Poll, Waker},
        time::Duration,
    };

    #[tokio::test(flavor = "current_thread")]
    async fn local_io_yields_without_waiting_on_the_calling_thread() {
        let caller = std::thread::current().id();
        let (release, wait) = mpsc::channel();
        let mut operation = Box::pin(run_blocking(move || {
            // The watchdog prevents a synchronous regression from hanging CI.
            let released = wait.recv_timeout(Duration::from_secs(2)).is_ok();
            Ok((std::thread::current().id(), released))
        }));
        let first_poll = operation
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()));
        let yielded = first_poll.is_pending();
        let _ = release.send(());
        let result = match first_poll {
            Poll::Ready(result) => result,
            Poll::Pending => operation.await,
        }
        .unwrap();
        assert!(
            yielded,
            "local I/O must yield while the operation is blocked"
        );
        assert_ne!(result.0, caller);
        assert!(
            result.1,
            "the caller must run before the operation finishes"
        );
    }

    #[tokio::test]
    async fn local_io_preserves_structured_errors() {
        let error = run_blocking(|| {
            Err::<(), _>(LocalPoolError::new(
                ErrorCode::Conflict,
                "synthetic conflict",
            ))
        })
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Conflict);
        assert_eq!(error.message, "synthetic conflict");
    }
}
