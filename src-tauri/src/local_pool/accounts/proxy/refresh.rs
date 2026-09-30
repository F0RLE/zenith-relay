use super::super::{
    authority::CodexRefreshClient, credentials::CredentialRefresh, oauth::CodexOAuthClient,
};
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
};
use zenith_relay_core::{
    accounts::{TokenRefreshFailure, TokenRefreshFailureKind},
    ProxyConfig,
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
