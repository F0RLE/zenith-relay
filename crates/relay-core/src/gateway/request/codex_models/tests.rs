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

#[test]
fn api_context_limits_follow_key_scope_without_changing_reference_metadata() {
    let sources =
        [("small", 1000, 900, 100), ("large", 2000, 1800, 200)]
            .into_iter()
            .map(|(id, context, input, output)| {
                let mut source = crate::RuntimeSource::unrestricted(crate::ProviderSource {
                    id: id.into(),
                    name: id.into(),
                    base_url: "https://example.invalid/v1/responses".into(),
                    api_key: "synthetic-key".into(),
                    proxy_url: None,
                    wire_api: WireApi::Responses,
                    models: vec!["gpt-test".into()],
                });
                source.protocol_config.capabilities =
                    vec![serde_json::from_value(json!({
                "modelId":"gpt-test", "upstreamWireApi":"responses", "status":"declared",
                "origin":"catalog", "checkedAtMs":1,
                "tokenLimits":{"context":context, "input":input, "output":output}
            })).unwrap()];
                source
            })
            .collect();
    let keys = [None, Some(vec!["large".to_owned()])]
        .into_iter()
        .enumerate()
        .map(|(index, scope)| {
            let mut key = crate::RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: format!("key-{index}"),
                secret: format!("synthetic-secret-{index}"),
            });
            key.source_ids = scope;
            key.model_prefix = Some("tenant".into());
            key
        })
        .collect();
    let catalog = crate::model_metadata::ModelMetadataCatalog::from_models_dev_json(
        r#"{"openai/gpt-test":{"name":"Synthetic","limit":{"context":4096,"input":3600,"output":512}}}"#
    ).unwrap();
    let runtime = GatewayRuntime::from_pool(
        sources,
        keys,
        GatewayRuntimeOptions {
            model_metadata_catalog: Some(crate::model_metadata::ModelMetadataCatalogHandle::new(
                catalog,
            )),
            ..Default::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    for (index, maximum, input_budget) in [(0, 1000, 900), (1, 2000, 1800)] {
        let key = runtime
            .authenticate_secret(&format!("synthetic-secret-{index}"))
            .unwrap();
        let response =
            build_codex_models_response(&runtime, &key, &["tenant/gpt-test".into()], None).unwrap();
        let entry = &response["models"][0];
        assert_eq!(entry["max_context_window"], maximum);
        assert!(entry["context_window"].as_u64().unwrap() <= maximum);
        assert!(entry["auto_compact_token_limit"].as_u64().unwrap() <= input_budget);
    }
    assert_eq!(
        runtime.model_capabilities("gpt-test").context_limit,
        Some(4096)
    );
}

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
                oauth_client_kind: Default::default(),
                id: (*account_id).into(),
                source_id: "chatgpt".into(),
                chatgpt_account_id: "chatgpt-account".into(),
                chatgpt_user_id: None,
                responses_url: "https://example.test/v1/responses".into(),
                basis_points_headers: None,
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
                proxy_timezone: None,
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
