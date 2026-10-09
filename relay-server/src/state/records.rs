use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use zenith_relay_core::{
    accounts::{AccountAuthState, AccountHealthState, TokenSet},
    model_metadata::ModelMetadataCatalog,
    providers::chatgpt::{AgentIdentityCredential, BasisPointsCapturedHeaders, OAuthClientKind},
    quota::{QuotaSnapshot, Subscription},
    ApiModelPriceOverride, SourceProtocolBinding, SourceProtocolConfig, SourceProtocolResolution,
    WireApi,
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProxyRecord {
    pub id: String,
    pub endpoint: String,
    pub secret_ref: String,
    pub created_at_ms: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRecord {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub in_pool: bool,
    pub draining: bool,
    pub base_url: String,
    pub secret_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_provider_family: Option<String>,
    pub wire_api: WireApi,
    #[serde(default)]
    pub protocol_bindings: Vec<SourceProtocolBinding>,
    #[serde(default)]
    pub protocol_config: SourceProtocolConfig,
    pub models: Vec<String>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    #[serde(default)]
    pub recovery_delay_seconds: u64,
    #[serde(default)]
    pub model_price_overrides: BTreeMap<String, ApiModelPriceOverride>,
    #[serde(default)]
    pub detected_model_prices: BTreeMap<String, ApiModelPriceOverride>,
    pub last_error_code: Option<String>,
}

impl SourceRecord {
    pub fn effective_protocol_bindings(&self) -> Result<Vec<SourceProtocolBinding>, String> {
        SourceProtocolResolution::resolved_protocol_bindings(self)
    }

    pub fn models_for_wire_api(&self, wire_api: WireApi) -> Result<Vec<String>, String> {
        SourceProtocolResolution::resolved_models(self, Some(wire_api))
    }

    pub fn supports_wire_api(&self, wire_api: WireApi) -> Result<bool, String> {
        SourceProtocolResolution::resolved_supports_wire_api(self, wire_api)
    }

    pub fn supports_any_wire_api(&self) -> Result<bool, String> {
        SourceProtocolResolution::resolved_supports_any(self)
    }

    pub fn models_with_cache_write_pricing(
        &self,
        reference_catalog: &ModelMetadataCatalog,
    ) -> std::collections::BTreeSet<String> {
        zenith_relay_core::cache_write_model_ids(
            SourceProtocolResolution::resolved_protocol_bindings_with_catalog(
                self,
                Some(reference_catalog),
            )
            .unwrap_or_default(),
        )
    }
}

zenith_relay_core::impl_stored_source_record!(SourceRecord);

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerAccountRecord {
    pub id: String,
    pub label: String,
    pub identity_hint: String,
    pub enabled: bool,
    #[serde(default)]
    pub in_pool: bool,
    pub draining: bool,
    pub source_id: String,
    pub secret_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_family: Option<String>,
    pub auth_state: AccountAuthState,
    pub health: AccountHealthState,
    pub models: Vec<String>,
    /// Last successful upstream discovery. The imported/configured `models`
    /// list is the stable baseline and is never replaced by a refresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered_models: Option<Vec<String>>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    pub subscription: Subscription,
    pub quota: QuotaSnapshot,
    #[serde(default)]
    pub purchase_cost_micro_usd: Option<u64>,
    pub cooldowns: BTreeMap<String, u64>,
    pub consecutive_failures: u32,
    #[serde(default)]
    pub created_at_ms: u64,
    pub last_used_at_ms: Option<u64>,
    pub last_error_code: Option<String>,
    #[serde(default)]
    pub proxy_id: Option<String>,
    #[serde(default)]
    pub bypass_common_proxy: bool,
}

zenith_relay_core::impl_effective_models!(ServerAccountRecord);

zenith_relay_core::impl_pool_participant!(ServerAccountRecord);
zenith_relay_core::impl_account_operational_source!(ServerAccountRecord);

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayKeyRecord {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    #[serde(default)]
    pub system: bool,
    pub secret_ref: String,
    pub created_at_ms: u64,
    pub last_used_at_ms: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountCredential {
    #[serde(default)]
    pub oauth_client_kind: OAuthClientKind,
    #[serde(default)]
    pub chatgpt_user_id: Option<String>,
    #[serde(default)]
    pub basis_points_headers: Option<BasisPointsCapturedHeaders>,
    #[serde(default)]
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub expires_at_ms: Option<u64>,
    pub issued_at_ms: u64,
    pub generation: u64,
    pub chatgpt_account_id: String,
    pub responses_url: String,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub agent_private_key: Option<String>,
    #[serde(default)]
    pub agent_runtime_id: Option<String>,
    #[serde(default)]
    pub agent_task_id: Option<String>,
}

impl AccountCredential {
    pub fn matches_connection(
        &self,
        kind: OAuthClientKind,
        account_id: &str,
        user_id: Option<&str>,
    ) -> bool {
        self.oauth_client_kind == kind
            && self.chatgpt_account_id == account_id
            && self.principal_user_id().as_deref() == user_id
    }

    pub fn principal_user_id(&self) -> Option<String> {
        self.chatgpt_user_id.clone().or_else(|| {
            [self.id_token.as_deref(), Some(self.access_token.as_str())]
                .into_iter()
                .flatten()
                .find_map(|token| {
                    let claims = zenith_relay_core::accounts::decode_unverified_jwt_payload::<
                        serde_json::Value,
                    >(token)?;
                    let auth = claims.get("https://api.openai.com/auth")?;
                    auth.get("chatgpt_user_id")
                        .or_else(|| auth.get("user_id"))?
                        .as_str()
                        .map(str::trim)
                        .filter(|user_id| !user_id.is_empty())
                        .map(str::to_string)
                })
        })
    }

    pub fn validate_oauth_client(&self) -> Result<(), String> {
        self.oauth_client_kind
            .validate_token_hints(self.id_token.as_deref(), Some(&self.access_token))
            .map_err(str::to_owned)?;
        if self.oauth_client_kind == OAuthClientKind::ExcelBps
            && (self.is_agent_identity() || !self.has_oauth())
        {
            return Err(
                "Excel OAuth requires its own access token and cannot use Agent Identity".into(),
            );
        }
        if let Some(headers) = self.basis_points_headers.as_ref() {
            headers
                .validate()
                .map_err(|_| "stored Basis Points headers are invalid".to_string())?;
        }
        Ok(())
    }

    pub fn agent_identity(&self) -> Result<Option<AgentIdentityCredential>, String> {
        self.validate_oauth_client()?;
        match (
            self.agent_private_key.as_ref(),
            self.agent_runtime_id.as_ref(),
            self.agent_task_id.as_ref(),
        ) {
            (None, None, None) => Ok(None),
            (Some(private_key), Some(runtime_id), task_id) => match task_id {
                Some(task_id) => AgentIdentityCredential::new(
                    private_key.clone(),
                    runtime_id.clone(),
                    task_id.clone(),
                ),
                None => {
                    AgentIdentityCredential::unregistered(private_key.clone(), runtime_id.clone())
                }
            }
            .map(Some)
            .map_err(|error| error.to_string()),
            _ => Err("stored Agent Identity credential is incomplete".to_string()),
        }
    }

    pub fn is_agent_identity(&self) -> bool {
        self.agent_private_key.is_some()
            || self.agent_runtime_id.is_some()
            || self.agent_task_id.is_some()
    }

    pub fn has_oauth(&self) -> bool {
        !self.access_token.trim().is_empty()
    }

    pub fn authorization(&self, now_ms: u64) -> Result<HeaderValue, String> {
        if let Some(agent) = self.agent_identity()? {
            return agent
                .authorization(now_ms)
                .map_err(|error| error.to_string());
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", self.access_token))
            .map_err(|_| "stored account access token is invalid".to_string())?;
        authorization.set_sensitive(true);
        Ok(authorization)
    }

    pub fn tokens(&self) -> Result<TokenSet, String> {
        self.validate_oauth_client()?;
        TokenSet::new(
            self.access_token.clone(),
            self.refresh_token.clone(),
            self.id_token.clone(),
            self.expires_at_ms,
            self.issued_at_ms,
            self.generation,
        )
        .map_err(str::to_string)
    }
}
