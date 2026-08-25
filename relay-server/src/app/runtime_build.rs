use super::{
    account_runtime::{account_proxy_config, runtime_account, runtime_key, runtime_source},
    AccountCredential, AppState,
};
use crate::{
    state::{is_internal_gateway_key, GatewayKeyRecord, ServerAccountRecord, SourceRecord},
    token_refresh::{CodexRefreshClient, ServerRefreshClients, ServerTokenPersistence},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use zenith_relay_core::{
    providers::chatgpt::AgentIdentityCredential, GatewayRuntime, GatewayRuntimeOptions,
    RuntimeChatGptAccount, RuntimeChatGptAuth, RuntimeMixedLocalKey, RuntimeSource,
    QUOTA_STALE_AFTER_MS,
};

struct AccountRuntimeBuild {
    accounts: Vec<RuntimeChatGptAccount>,
    direct_refresh_accounts: HashSet<String>,
    refresh_clients: HashMap<String, CodexRefreshClient>,
    agent_identities: HashMap<String, AgentIdentityCredential>,
}

pub(super) async fn rebuild(state: &Arc<AppState>) -> Result<(), String> {
    let source_records = state.store.sources()?;
    let account_records = state.store.accounts()?;
    let key_records = state
        .store
        .keys()?
        .into_iter()
        .filter(|key| key.enabled && is_internal_gateway_key(key))
        .collect::<Vec<_>>();
    let hidden_models = state.store.hidden_models()?;
    let model_reasoning_allowed_levels = state.store.model_reasoning_allowed_levels()?;
    let model_service_tier_overrides = state.store.model_service_tier_overrides()?;
    let model_display_order = state.store.model_display_order()?;
    let routing_policy = state.store.routing_policy()?;
    let codex_background_tasks_enabled = state.store.codex_background_tasks_enabled()?;
    let codex_websockets_enabled = state.store.codex_websockets_enabled()?;
    let (mut pool_source_ids, mut pool_account_ids) =
        pool_member_ids(&source_records, &account_records);
    if key_records.is_empty() || (source_records.is_empty() && account_records.is_empty()) {
        return state.replace_runtime(None);
    }

    let sources = build_sources(state, source_records)?;
    let AccountRuntimeBuild {
        accounts,
        direct_refresh_accounts,
        refresh_clients,
        agent_identities,
    } = build_accounts(state, account_records).await?;
    if !has_active_candidate(&sources, &accounts) {
        return state.replace_runtime(None);
    }
    retain_active_pool_members(
        &sources,
        &accounts,
        &mut pool_source_ids,
        &mut pool_account_ids,
    );

    let keys = build_keys(state, key_records, &pool_source_ids, &pool_account_ids)?;
    if keys.is_empty() || (sources.is_empty() && accounts.is_empty()) {
        return state.replace_runtime(None);
    }

    let refresh = Arc::new(ServerRefreshClients {
        direct: CodexRefreshClient::new_with_proxy(None)?,
        direct_accounts: direct_refresh_accounts,
        clients: refresh_clients,
    });
    let persistence = Arc::new(ServerTokenPersistence {
        state: state.clone(),
    });
    let usage = state.usage_callback()?;
    let runtime = GatewayRuntime::from_mixed_pool(
        sources,
        accounts,
        keys,
        RuntimeChatGptAuth {
            token_authority: state.token_authority.clone(),
            refresh_adapter: refresh,
            persistence_adapter: persistence,
            refresh_skew_ms: 60_000,
            agent_identities,
        },
        GatewayRuntimeOptions {
            max_retry_candidates: usize::from(routing_policy.max_retry_candidates),
            cooldown_after_failures: routing_policy.cooldown_after_failures,
            keep_last_candidate_available: routing_policy.keep_last_candidate_available,
            routing_strategy: routing_policy.routing_strategy,
            subscription_plan_order: routing_policy.subscription_plan_order,
            hidden_models,
            default_service_tier: routing_policy.default_service_tier,
            quota_stale_after_ms: QUOTA_STALE_AFTER_MS,
            image_base_model: None,
            model_reasoning_allowed_levels,
            response_affinity_store: Some(state.store.clone()),
            provider_storm_breaker: true,
        },
        usage,
    )
    .map_err(|error| error.to_string())?;
    runtime
        .set_model_service_tier_overrides(model_service_tier_overrides)
        .map_err(|error| error.to_string())?;
    runtime.set_model_display_order(model_display_order);
    runtime.set_codex_background_tasks_enabled(codex_background_tasks_enabled);
    runtime.set_codex_websockets_enabled(codex_websockets_enabled);
    state.replace_runtime(Some(Arc::new(runtime)))
}

/// Candidate state remains available to management, while the internal profile
/// credential derives its request scope solely from pool membership.
fn pool_member_ids(
    sources: &[SourceRecord],
    accounts: &[ServerAccountRecord],
) -> (Vec<String>, Vec<String>) {
    let source_ids = sources
        .iter()
        .filter(|record| record.in_pool && record.supports_any_wire_api().unwrap_or(false))
        .map(|record| record.id.clone())
        .collect();
    let account_ids = accounts
        .iter()
        .filter(|record| record.in_pool)
        .map(|record| record.id.clone())
        .collect();
    (source_ids, account_ids)
}

fn build_sources(
    state: &AppState,
    records: Vec<SourceRecord>,
) -> Result<Vec<RuntimeSource>, String> {
    let mut sources = Vec::new();
    for record in records {
        let Some(api_key) = state.vault.load(&record.secret_ref)? else {
            continue;
        };
        sources.push(runtime_source(record, api_key));
    }
    Ok(sources)
}

async fn build_accounts(
    state: &Arc<AppState>,
    records: Vec<ServerAccountRecord>,
) -> Result<AccountRuntimeBuild, String> {
    let mut build = AccountRuntimeBuild {
        accounts: Vec::new(),
        direct_refresh_accounts: HashSet::new(),
        refresh_clients: HashMap::new(),
        agent_identities: HashMap::new(),
    };
    for record in records {
        let Some(secret) = state.vault.load(&record.secret_ref)? else {
            continue;
        };
        let credential: AccountCredential = serde_json::from_str(&secret)
            .map_err(|_| "stored account credential is invalid".to_string())?;
        let Ok(proxy) = account_proxy_config(state, &record, &credential) else {
            continue;
        };
        if let Some(agent) = credential.agent_identity()? {
            build.agent_identities.insert(record.id.clone(), agent);
        }
        if credential.has_oauth() {
            state
                .token_authority
                .register(&record.id, credential.tokens()?, record.auth_state)
                .await
                .map_err(|error| error.to_string())?;
            if proxy.is_some() {
                build.refresh_clients.insert(
                    record.id.clone(),
                    CodexRefreshClient::new_with_proxy(proxy.as_ref())?,
                );
            } else {
                build.direct_refresh_accounts.insert(record.id.clone());
            }
        }
        build.accounts.push(runtime_account(
            record,
            &credential,
            proxy,
            QUOTA_STALE_AFTER_MS,
        ));
    }
    Ok(build)
}

fn has_active_candidate(sources: &[RuntimeSource], accounts: &[RuntimeChatGptAccount]) -> bool {
    sources
        .iter()
        .any(|source| source.enabled && !source.draining)
        || accounts
            .iter()
            .any(|account| account.enabled && !account.draining)
}

fn retain_active_pool_members(
    sources: &[RuntimeSource],
    accounts: &[RuntimeChatGptAccount],
    source_ids: &mut Vec<String>,
    account_ids: &mut Vec<String>,
) {
    let active_source_ids = sources
        .iter()
        .filter(|source| source.enabled && !source.draining)
        .map(|source| source.source.id.as_str())
        .collect::<HashSet<_>>();
    source_ids.retain(|id| active_source_ids.contains(id.as_str()));
    let active_account_ids = accounts
        .iter()
        .filter(|account| account.enabled && !account.draining)
        .map(|account| account.id.as_str())
        .collect::<HashSet<_>>();
    account_ids.retain(|id| active_account_ids.contains(id.as_str()));
}

fn build_keys(
    state: &AppState,
    records: Vec<GatewayKeyRecord>,
    pool_source_ids: &[String],
    pool_account_ids: &[String],
) -> Result<Vec<RuntimeMixedLocalKey>, String> {
    let mut keys = Vec::new();
    for record in records {
        let Some(secret) = state.vault.load(&record.secret_ref)? else {
            continue;
        };
        if pool_source_ids.is_empty() && pool_account_ids.is_empty() {
            continue;
        }
        keys.push(runtime_key(
            record,
            secret,
            pool_source_ids,
            pool_account_ids,
        ));
    }
    Ok(keys)
}
