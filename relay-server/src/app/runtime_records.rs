use crate::state::{now_ms, GatewayKeyRecord, ServerAccountRecord, SourceRecord};
use zenith_relay_core::{
    protocol::{account_candidate_enabled, account_operational_state, AccountOperationalInput},
    LocalGatewayKey, ProviderSource, ProxyConfig, RuntimeChatGptAccount, RuntimeMixedLocalKey,
    RuntimeSource,
};

pub(super) fn runtime_source(record: SourceRecord, api_key: String) -> RuntimeSource {
    RuntimeSource {
        source: ProviderSource {
            id: record.id,
            name: record.name,
            base_url: record.base_url,
            api_key,
            wire_api: record.wire_api,
            models: record.models,
        },
        protocol_bindings: record.protocol_bindings,
        enabled: record.enabled,
        draining: record.draining,
        priority: record.priority,
        weight: record.weight,
        recovery_delay_seconds: record.recovery_delay_seconds,
        allowed_models: record.allowed_models,
        excluded_models: record.excluded_models,
        last_used_at_ms: None,
    }
}

pub(super) fn runtime_account(
    record: ServerAccountRecord,
    credential: &crate::state::AccountCredential,
    proxy: Option<ProxyConfig>,
    quota_stale_after_ms: u64,
) -> RuntimeChatGptAccount {
    let operational = account_operational_state(AccountOperationalInput {
        enabled: record.enabled,
        in_pool: record.in_pool,
        draining: record.draining,
        secret_available: true,
        proxy_available: true,
        auth_state: record.auth_state,
        health: record.health,
        subscription: &record.subscription,
        quota: &record.quota,
        last_error_code: record.last_error_code.as_deref(),
        now_ms: now_ms(),
        quota_stale_after_ms,
    });
    let models = record.effective_models().to_vec();
    RuntimeChatGptAccount {
        id: record.id,
        source_id: record.source_id,
        chatgpt_account_id: credential.chatgpt_account_id.clone(),
        responses_url: credential.responses_url.clone(),
        models,
        enabled: account_candidate_enabled(record.enabled, operational.routing_block_reason),
        draining: record.draining,
        priority: record.priority,
        weight: record.weight,
        allowed_models: record.allowed_models,
        excluded_models: record.excluded_models,
        health: operational.health,
        quota: operational.quota,
        quota_updated_at_ms: record.quota.updated_at_ms,
        quota_snapshot: record.quota.clone(),
        subscription_plan_type: record.subscription.plan_type.clone(),
        subscription_expires_at_ms: record.subscription.active_until_ms,
        last_used_at_ms: record.last_used_at_ms,
        cooldowns: record.cooldowns,
        consecutive_failures: record.consecutive_failures,
        proxy,
    }
}

pub(super) fn runtime_key(
    record: GatewayKeyRecord,
    secret: String,
    pool_source_ids: &[String],
    pool_account_ids: &[String],
) -> RuntimeMixedLocalKey {
    RuntimeMixedLocalKey {
        key: LocalGatewayKey {
            id: record.id,
            secret,
        },
        enabled: record.enabled,
        source_ids: Some(pool_source_ids.to_vec()),
        account_ids: Some(pool_account_ids.to_vec()),
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        model_prefix: None,
        wire_apis: Some(vec![
            zenith_relay_core::protocol::ClientWireApi::Responses,
            zenith_relay_core::protocol::ClientWireApi::Messages,
            zenith_relay_core::protocol::ClientWireApi::ChatCompletions,
            zenith_relay_core::protocol::ClientWireApi::Gemini,
        ]),
    }
}
