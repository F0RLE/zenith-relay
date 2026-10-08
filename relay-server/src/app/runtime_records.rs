use crate::state::{now_ms, GatewayKeyRecord, ServerAccountRecord, SourceRecord};
use zenith_relay_core::{
    protocol::{account_candidate_enabled, account_operational_state, AccountOperationalInput},
    LocalGatewayKey, ProviderSource, ProxyConfig, RuntimeChatGptAccount, RuntimeMixedLocalKey,
    RuntimeSource,
};

pub(super) fn runtime_source(source_record: SourceRecord, api_key: String) -> RuntimeSource {
    RuntimeSource {
        source: ProviderSource {
            id: source_record.id,
            name: source_record.name,
            base_url: source_record.base_url,
            api_key,
            wire_api: source_record.wire_api,
            models: source_record.models,
        },
        protocol_bindings: source_record.protocol_bindings,
        protocol_config: source_record.protocol_config,
        enabled: source_record.enabled,
        draining: source_record.draining,
        priority: source_record.priority,
        weight: source_record.weight,
        recovery_delay_seconds: source_record.recovery_delay_seconds,
        allowed_models: source_record.allowed_models,
        excluded_models: source_record.excluded_models,
        last_used_at_ms: None,
    }
}

pub(super) fn runtime_account(
    account_record: ServerAccountRecord,
    credential: &crate::state::AccountCredential,
    proxy: Option<ProxyConfig>,
    basis_points_enabled: bool,
    quota_stale_after_ms: u64,
) -> RuntimeChatGptAccount {
    let operational = account_operational_state(AccountOperationalInput::from_source(
        &account_record,
        true,
        true,
        now_ms(),
        quota_stale_after_ms,
    ));
    let models = account_record.effective_models().to_vec();
    RuntimeChatGptAccount {
        id: account_record.id,
        source_id: account_record.source_id,
        chatgpt_account_id: credential.chatgpt_account_id.clone(),
        responses_url: credential.responses_url.clone(),
        basis_points_enabled: basis_points_enabled
            && credential.has_oauth()
            && !credential.is_agent_identity(),
        models,
        enabled: account_candidate_enabled(
            account_record.enabled,
            operational.routing_block_reason,
        ),
        draining: account_record.draining,
        priority: account_record.priority,
        weight: account_record.weight,
        allowed_models: account_record.allowed_models,
        excluded_models: account_record.excluded_models,
        health: operational.health,
        quota: operational.quota,
        quota_updated_at_ms: account_record.quota.updated_at_ms,
        quota_snapshot: account_record.quota.clone(),
        subscription_plan_type: account_record.subscription.plan_type.clone(),
        subscription_expires_at_ms: account_record.subscription.active_until_ms,
        last_used_at_ms: account_record.last_used_at_ms,
        cooldowns: account_record.cooldowns,
        consecutive_failures: account_record.consecutive_failures,
        proxy,
    }
}

pub(super) fn runtime_key(
    gateway_key_record: GatewayKeyRecord,
    secret: String,
    pool_source_ids: &[String],
    pool_account_ids: &[String],
) -> RuntimeMixedLocalKey {
    RuntimeMixedLocalKey {
        key: LocalGatewayKey {
            id: gateway_key_record.id,
            secret,
        },
        enabled: gateway_key_record.enabled,
        source_ids: Some(pool_source_ids.to_vec()),
        account_ids: Some(pool_account_ids.to_vec()),
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        model_prefix: None,
        wire_apis: Some(zenith_relay_core::protocol::local_gateway_client_wire_apis()),
    }
}
