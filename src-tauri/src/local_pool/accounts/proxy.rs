use super::{
    authority::CodexRefreshClient,
    credentials::{CredentialRefresh, StoredCodexCredentials},
    oauth::CodexOAuthClient,
};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::GatewaySettings,
    store::secret_store,
};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
};
use zenith_relay_core::{
    accounts::{TokenRefreshFailure, TokenRefreshFailureKind},
    protocol::ProxyMode,
    ProxyConfig,
};

pub const COMMON_PROXY_SECRET_REF: &str = "proxy:common";

pub fn effective_proxy_url(
    settings: &GatewaySettings,
    credentials: &StoredCodexCredentials,
) -> Result<Option<String>> {
    choose_proxy_url(
        credentials.proxy_url(),
        settings.common_proxy_configured,
        || secret_store::load(COMMON_PROXY_SECRET_REF),
    )
}

pub fn effective_proxy_config(
    settings: &GatewaySettings,
    credentials: &StoredCodexCredentials,
) -> Result<Option<ProxyConfig>> {
    effective_proxy_url(settings, credentials)?
        .map(|value| {
            ProxyConfig::parse(&value).map_err(|_| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "stored account proxy URL is invalid",
                )
            })
        })
        .transpose()
}

pub fn common_proxy_config(settings: &GatewaySettings) -> Result<Option<ProxyConfig>> {
    if !settings.common_proxy_configured {
        return Ok(None);
    }
    let value = secret_store::load(COMMON_PROXY_SECRET_REF)?.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::SecretStoreUnavailable,
            "common account proxy is configured but its secret is unavailable",
        )
    })?;
    ProxyConfig::parse(&value).map(Some).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "stored common proxy URL is invalid",
        )
    })
}

pub fn proxy_status(
    settings: &GatewaySettings,
    credentials: &StoredCodexCredentials,
    common_available: bool,
) -> (ProxyMode, bool) {
    if credentials.proxy_url().is_some() {
        return (
            ProxyMode::Account,
            credentials
                .proxy_url()
                .is_some_and(|value| ProxyConfig::parse(value).is_ok()),
        );
    }
    if settings.common_proxy_configured {
        return (ProxyMode::Common, common_available);
    }
    (ProxyMode::Direct, true)
}

pub fn common_proxy_available(settings: &GatewaySettings) -> bool {
    settings.common_proxy_configured
        && secret_store::load(COMMON_PROXY_SECRET_REF)
            .ok()
            .flatten()
            .is_some_and(|value| ProxyConfig::parse(&value).is_ok())
}

fn choose_proxy_url(
    account_proxy: Option<&str>,
    common_configured: bool,
    load_common: impl FnOnce() -> Result<Option<String>>,
) -> Result<Option<String>> {
    if let Some(value) = account_proxy {
        return Ok(Some(value.to_string()));
    }
    if !common_configured {
        return Ok(None);
    }
    load_common()?.map(Some).ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::SecretStoreUnavailable,
            "common account proxy is configured but its secret is unavailable",
        )
    })
}

pub struct ProxyRefreshClient {
    direct: CodexOAuthClient,
    direct_accounts: HashSet<String>,
    clients: HashMap<String, CodexOAuthClient>,
}

impl ProxyRefreshClient {
    pub fn new(proxies: impl IntoIterator<Item = (String, Option<ProxyConfig>)>) -> Result<Self> {
        let direct = CodexOAuthClient::new_with_proxy(None).map_err(|_| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "failed to initialize account refresh client",
            )
        })?;
        let mut direct_accounts = HashSet::new();
        let mut clients = HashMap::new();
        for (account_id, proxy) in proxies {
            match proxy {
                Some(proxy) => {
                    let client = CodexOAuthClient::new_with_proxy(Some(&proxy)).map_err(|_| {
                        LocalPoolError::new(
                            ErrorCode::InvalidState,
                            "failed to initialize account proxy client",
                        )
                    })?;
                    clients.insert(account_id, client);
                }
                None => {
                    direct_accounts.insert(account_id);
                }
            }
        }
        Ok(Self {
            direct,
            direct_accounts,
            clients,
        })
    }
}

impl CodexRefreshClient for ProxyRefreshClient {
    fn refresh<'a>(
        &'a self,
        local_account_id: &'a str,
        _provider_account_id: Option<&'a str>,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> Pin<
        Box<
            dyn Future<Output = std::result::Result<CredentialRefresh, TokenRefreshFailure>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let client = match self.clients.get(local_account_id) {
                Some(client) => client,
                None if self.direct_accounts.contains(local_account_id) => &self.direct,
                None => {
                    return Err(TokenRefreshFailure::new(
                        TokenRefreshFailureKind::Transient,
                        "proxy_client_missing",
                    ))
                }
            };
            let tokens = client.exchange_refresh_token(refresh_token, now_ms).await?;
            CredentialRefresh::from_oauth(tokens).map_err(|_| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "invalid_refresh_response",
                )
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_proxy_overrides_common_and_missing_common_fails_closed() {
        let account = choose_proxy_url(Some("http://account.example:8080/"), true, || {
            panic!("common proxy must not be loaded for an account override")
        })
        .unwrap();
        assert_eq!(account.as_deref(), Some("http://account.example:8080/"));

        let error = choose_proxy_url(None, true, || Ok(None)).unwrap_err();
        assert!(matches!(error.code, ErrorCode::SecretStoreUnavailable));
        assert_eq!(choose_proxy_url(None, false, || Ok(None)).unwrap(), None);
    }
}
