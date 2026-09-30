use super::*;
use crate::accounts::{
    AccountAuthState, TokenAuthority, TokenPersistenceAdapter, TokenPersistenceFailure,
    TokenRefresh, TokenRefreshAdapter, TokenRefreshFailure, TokenRefreshFailureKind, TokenSet,
};
use crate::providers::chatgpt::{RuntimeChatGptAccount, RuntimeChatGptAuth};
use crate::{
    CandidateHealth, CandidateQuota, DefaultServiceTier, GatewayRuntimeOptions, LocalGatewayKey,
    RuntimeMixedLocalKey, WireApi,
};
use futures_util::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Arc;

mod native_cards;
mod speed_reasoning;

struct NoopRefresh;

impl TokenRefreshAdapter for NoopRefresh {
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

fn native_catalog_test_runtime(
    model_prefix: Option<&str>,
    model_metadata_catalog: Option<crate::model_metadata::ModelMetadataCatalogHandle>,
) -> GatewayRuntime {
    native_catalog_test_runtime_with_accounts(
        model_prefix,
        model_metadata_catalog,
        &["native-account"],
        &["gpt-native"],
    )
}

fn native_catalog_test_runtime_with_accounts(
    model_prefix: Option<&str>,
    model_metadata_catalog: Option<crate::model_metadata::ModelMetadataCatalogHandle>,
    account_ids: &[&str],
    models: &[&str],
) -> GatewayRuntime {
    GatewayRuntime::from_mixed_pool_allow_unroutable(
        Vec::new(),
        account_ids
            .iter()
            .map(|account_id| RuntimeChatGptAccount {
                id: (*account_id).into(),
                source_id: "chatgpt".into(),
                chatgpt_account_id: "chatgpt-account".into(),
                responses_url: "https://example.test/v1/responses".into(),
                basis_points_enabled: false,
                models: models.iter().map(|model| (*model).into()).collect(),
                enabled: true,
                draining: false,
                priority: 0,
                weight: 1,
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                health: CandidateHealth::Healthy,
                quota: CandidateQuota::Unknown,
                quota_updated_at_ms: None,
                quota_snapshot: Default::default(),
                subscription_plan_type: None,
                subscription_expires_at_ms: None,
                last_used_at_ms: None,
                cooldowns: Default::default(),
                consecutive_failures: 0,
                proxy: None,
            })
            .collect(),
        vec![RuntimeMixedLocalKey {
            key: LocalGatewayKey {
                id: "key".into(),
                secret: "secret".into(),
            },
            enabled: true,
            source_ids: None,
            account_ids: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: model_prefix.map(str::to_owned),
            wire_apis: None,
        }],
        RuntimeChatGptAuth {
            token_authority: Arc::new(TokenAuthority::new(1).unwrap()),
            refresh_adapter: Arc::new(NoopRefresh),
            persistence_adapter: Arc::new(NoopPersistence),
            refresh_skew_ms: 60_000,
            agent_identities: HashMap::new(),
        },
        GatewayRuntimeOptions {
            model_metadata_catalog,
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap()
}
