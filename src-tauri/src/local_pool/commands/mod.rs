pub(crate) mod automations;
pub(crate) mod connections;
pub(crate) mod gateway;
pub(crate) mod oauth;
pub(crate) mod opencode;
pub(crate) mod pool;
pub(crate) mod profiles;
pub(crate) mod proxies;
pub(crate) mod recovery;
pub(crate) mod remote_server;
pub(crate) mod state;
pub(crate) mod usage;

mod runtime;

use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result as LocalResult},
    models::{GatewaySettings, LocalAccountRecord, ProviderSourceRecord},
    store::secret_store,
};
use std::collections::BTreeMap;
use zenith_relay_core::{PriceEvidence, PricingContext, SourcePricingMetadata, TokenPrice};

pub(super) fn cleanup_created_secret(secret_ref: &str, cause: &LocalPoolError) -> LocalResult<()> {
    secret_store::delete(secret_ref).map_err(|cleanup| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "{}; secret cleanup failed: {}",
                cause.message, cleanup.message
            ),
        )
    })
}

/// Builds the immutable pricing identity used by all desktop projections.
/// Storage records contain only redacted/local identifiers and explicit
/// provider metadata; no pricing provider is inferred from a label or URL.
pub(super) fn pricing_context(
    gateway: &GatewaySettings,
    sources: &[ProviderSourceRecord],
    accounts: &[LocalAccountRecord],
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
            },
        );
        let mut evidence = BTreeMap::<String, PriceEvidence>::new();
        for (model, price) in &source.detected_model_prices {
            evidence
                .entry(model.to_ascii_lowercase())
                .or_default()
                .provider = Some(TokenPrice::from(*price));
        }
        for (model, price) in &source.model_price_overrides {
            evidence
                .entry(model.to_ascii_lowercase())
                .or_default()
                .manual = Some(TokenPrice::from(*price));
        }
        source_evidence.insert(source.id.clone(), evidence);
    }
    let global_manual_prices = gateway
        .model_price_overrides
        .iter()
        .map(|(model, price)| (model.to_ascii_lowercase(), TokenPrice::from(*price)))
        .collect();
    PricingContext {
        account_provider_families,
        source_metadata,
        source_evidence,
        global_manual_prices,
    }
}

pub(in crate::local_pool) use runtime::{
    apply_account_policy_if_running, apply_local_gateway_key_scope,
    apply_source_policies_if_running, apply_source_policy_if_running, core_error, current_time_ms,
    fail_closed, record_catalog_refresh_result, refresh_active_codex_catalog_in_background,
    refresh_local_gateway_key_scope_if_running, restart_after_secret_change, restart_or_rollback,
    runtime_account_policy, runtime_from_store, sync_accounts_or_rollback,
    sync_gateway_or_rollback, sync_records_or_rollback, sync_refreshed_account_or_rollback,
};
