use super::*;
use crate::accounts::{
    AccountAuthState, TokenPersistenceAdapter, TokenPersistenceFailure, TokenRefresh,
    TokenRefreshAdapter, TokenRefreshFailure, TokenRefreshFailureKind, TokenSet,
};
use crate::{
    CandidateHealth, CandidateQuota, CapabilityOrigin, CapabilityStatus, ModelEndpointCapability,
    ToolUseDiagnostics, QUOTA_STALE_AFTER_MS,
};
use futures_util::future::BoxFuture;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
mod affinity;
mod basis_points;
mod dispatch;
mod listing;
mod member_quota;
mod policy;

struct NeverRefresh;

impl TokenRefreshAdapter for NeverRefresh {
    fn refresh<'a>(
        &'a self,
        _account_id: &'a str,
        _refresh_token: &'a str,
        _now_ms: u64,
    ) -> BoxFuture<'a, std::result::Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async {
            Err(TokenRefreshFailure::new(
                TokenRefreshFailureKind::Transient,
                "not_called",
            ))
        })
    }
}

struct NoopPersistence;

impl TokenPersistenceAdapter for NoopPersistence {
    fn persist<'a>(
        &'a self,
        _account_id: &'a str,
        _tokens: &'a TokenSet,
    ) -> BoxFuture<'a, std::result::Result<(), TokenPersistenceFailure>> {
        Box::pin(async { Ok(()) })
    }

    fn persist_auth_state<'a>(
        &'a self,
        _account_id: &'a str,
        _auth_state: AccountAuthState,
    ) -> BoxFuture<'a, std::result::Result<(), TokenPersistenceFailure>> {
        Box::pin(async { Ok(()) })
    }

    fn persist_agent_task_id<'a>(
        &'a self,
        _account_id: &'a str,
        _expected_task_id: Option<&'a str>,
        task_id: &'a str,
    ) -> BoxFuture<'a, std::result::Result<String, TokenPersistenceFailure>> {
        Box::pin(async move { Ok(task_id.to_string()) })
    }
}

fn source(id: &str, key: &str, models: &[&str]) -> ProviderSource {
    ProviderSource {
        id: id.to_string(),
        name: id.to_string(),
        base_url: "https://example.test/v1".to_string(),
        api_key: key.to_string(),
        wire_api: WireApi::Responses,
        models: models.iter().map(|model| (*model).to_string()).collect(),
    }
}

fn key(id: &str, secret: &str) -> LocalGatewayKey {
    LocalGatewayKey {
        id: id.to_string(),
        secret: secret.to_string(),
    }
}

fn quota_account(snapshot: QuotaSnapshot) -> RuntimeChatGptAccount {
    RuntimeChatGptAccount {
        oauth_client_kind: Default::default(),
        id: "account-1".to_string(),
        source_id: "openai-codex".to_string(),
        chatgpt_account_id: "account-1".to_string(),
        chatgpt_user_id: None,
        responses_url: "https://example.test/v1/responses".to_string(),
        basis_points_enabled: false,
        basis_points_headers: None,
        models: vec!["gpt-test".to_string()],
        enabled: true,
        draining: false,
        priority: 0,
        weight: 1,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        health: CandidateHealth::Healthy,
        quota: CandidateQuota::from_snapshot(&snapshot, 1_000, QUOTA_STALE_AFTER_MS),
        quota_updated_at_ms: snapshot.updated_at_ms,
        quota_snapshot: snapshot,
        subscription_plan_type: None,
        subscription_expires_at_ms: None,
        last_used_at_ms: None,
        cooldowns: BTreeMap::new(),
        consecutive_failures: 0,
        proxy: None,
    }
}

fn quota_runtime(snapshot: QuotaSnapshot) -> GatewayRuntime {
    quota_runtime_with_agent(snapshot, None)
}

fn quota_runtime_with_agent(
    snapshot: QuotaSnapshot,
    agent: Option<AgentIdentityCredential>,
) -> GatewayRuntime {
    GatewayRuntime::from_mixed_pool(
        Vec::new(),
        vec![quota_account(snapshot)],
        vec![RuntimeMixedLocalKey {
            key: key("key-1", "local-secret"),
            enabled: true,
            source_ids: None,
            account_ids: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
            wire_apis: None,
        }],
        RuntimeChatGptAuth {
            token_authority: Arc::new(TokenAuthority::new(1).unwrap()),
            refresh_adapter: Arc::new(NeverRefresh),
            persistence_adapter: Arc::new(NoopPersistence),
            refresh_skew_ms: 60_000,
            agent_identities: agent
                .into_iter()
                .map(|agent| ("account-1".to_string(), agent))
                .collect(),
        },
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap()
}

#[derive(Default)]
struct RecordedResponseAffinityStore {
    found: Mutex<Vec<String>>,
    restored_binding: Mutex<Option<ResponseAffinityBinding>>,
    upserts: Mutex<Vec<ResponseAffinityBinding>>,
    deletes: Mutex<Vec<String>>,
}

impl ResponseAffinityStore for RecordedResponseAffinityStore {
    fn load(&self, _now_ms: u64) -> std::result::Result<Vec<ResponseAffinityBinding>, String> {
        Ok(Vec::new())
    }

    fn find(
        &self,
        key: &str,
        _now_ms: u64,
    ) -> std::result::Result<Option<ResponseAffinityBinding>, String> {
        crate::poison::mutex(&self.found).push(key.to_string());
        Ok(crate::poison::mutex(&self.restored_binding).clone())
    }

    fn upsert(&self, binding: &ResponseAffinityBinding) -> std::result::Result<(), String> {
        crate::poison::mutex(&self.upserts).push(binding.clone());
        Ok(())
    }

    fn delete(&self, key: &str) -> std::result::Result<(), String> {
        crate::poison::mutex(&self.deletes).push(key.to_string());
        Ok(())
    }

    fn delete_candidate(&self, _candidate_id: &str) -> std::result::Result<(), String> {
        Ok(())
    }
}
