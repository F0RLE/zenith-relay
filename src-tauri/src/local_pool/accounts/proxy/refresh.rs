use super::super::{
    authority::CodexRefreshClient,
    credentials::CredentialRefresh,
    oauth::{CodexOAuthClient, OAuthClientKind},
};
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
};
use zenith_relay_core::{
    ProxyConfig,
    accounts::{TokenRefreshFailure, TokenRefreshFailureKind},
};

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

    fn client_for_account(
        &self,
        account_id: &str,
    ) -> std::result::Result<&CodexOAuthClient, TokenRefreshFailure> {
        self.clients
            .get(account_id)
            .or_else(|| {
                self.direct_accounts
                    .contains(account_id)
                    .then_some(&self.direct)
            })
            .ok_or_else(|| {
                TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "proxy_client_missing")
            })
    }
}

impl CodexRefreshClient for ProxyRefreshClient {
    fn refresh_siwc<'a>(
        &'a self,
        local_account_id: &'a str,
        registration: &'a super::super::oauth::siwc::SiwcRegistration,
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
            let client = self.client_for_account(local_account_id)?;
            let tokens = client
                .exchange_siwc_refresh(registration, refresh_token, now_ms)
                .await?;
            CredentialRefresh::from_oauth(tokens).map_err(|_| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "invalid_refresh_response",
                )
            })
        })
    }
    fn refresh<'a>(
        &'a self,
        local_account_id: &'a str,
        _provider_account_id: Option<&'a str>,
        refresh_token: &'a str,
        now_ms: u64,
        kind: OAuthClientKind,
    ) -> Pin<
        Box<
            dyn Future<Output = std::result::Result<CredentialRefresh, TokenRefreshFailure>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let client = self.client_for_account(local_account_id)?;
            let tokens = client
                .for_kind(kind)
                .exchange_refresh_token(refresh_token, now_ms)
                .await?;
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
    fn refresh_only_uses_explicitly_configured_account_routes() {
        let client = ProxyRefreshClient::new([("synthetic-direct".to_string(), None)]).unwrap();
        assert!(std::ptr::eq(
            client.client_for_account("synthetic-direct").unwrap(),
            &client.direct
        ));
        assert!(client.client_for_account("synthetic-missing").is_err());
    }
}
