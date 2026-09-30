use super::*;

pub(crate) struct CodexRefreshClient {
    http: reqwest::Client,
}

impl CodexRefreshClient {
    pub(crate) fn new_with_proxy(proxy: Option<&ProxyConfig>) -> Result<Self, String> {
        let builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(20))
            .user_agent("Zenith Relay Server");
        let http = match proxy {
            Some(proxy) => proxy.apply(builder),
            None => builder,
        }
        .build()
        .map_err(|error| error.to_string())?;
        Ok(Self { http })
    }
}

pub(crate) struct ServerRefreshClients {
    pub(crate) direct: CodexRefreshClient,
    pub(crate) direct_accounts: HashSet<String>,
    pub(crate) clients: HashMap<String, CodexRefreshClient>,
}

impl TokenRefreshAdapter for ServerRefreshClients {
    fn refresh<'a>(
        &'a self,
        account_id: &'a str,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async move {
            let client = match self.clients.get(account_id) {
                Some(client) => client,
                None if self.direct_accounts.contains(account_id) => &self.direct,
                None => {
                    return Err(TokenRefreshFailure::new(
                        TokenRefreshFailureKind::Transient,
                        "proxy_client_missing",
                    ))
                }
            };
            client.refresh(account_id, refresh_token, now_ms).await
        })
    }
}

impl TokenRefreshAdapter for CodexRefreshClient {
    fn refresh<'a>(
        &'a self,
        _account_id: &'a str,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async move {
            if refresh_token.is_empty()
                || refresh_token.len() > 64 * 1024
                || refresh_token.bytes().any(|byte| byte.is_ascii_control())
            {
                return Err(TokenRefreshFailure::new(
                    TokenRefreshFailureKind::InvalidatedRefreshToken,
                    error_codes::INVALID_REFRESH_TOKEN,
                ));
            }
            let (response, permit) = management_http_gate()
                .send(
                    &self.http,
                    self.http
                        .post(CODEX_TOKEN_ENDPOINT)
                        .json(&serde_json::json!({
                            "client_id": CODEX_CLIENT_ID,
                            "grant_type": "refresh_token",
                            "refresh_token": refresh_token,
                        })),
                    HttpClass::Auth,
                )
                .await
                .map_err(|_| {
                    TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "transport")
                })?;
            let status = response.status();
            let body = collect_token_response(response).await?;
            drop(permit);
            if !status.is_success() {
                let code = token_refresh_provider_error_code(&body)
                    .unwrap_or_else(|| "token_refresh_failed".to_string());
                let kind = token_refresh_failure_kind(&code);
                return Err(TokenRefreshFailure::new(kind, &code));
            }
            let payload: TokenResponse = serde_json::from_slice(&body).map_err(|_| {
                TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "invalid_response")
            })?;
            let expires_at_ms = payload.expires_in.and_then(|seconds| {
                u64::try_from(seconds)
                    .ok()
                    .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1_000)))
            });
            TokenRefresh::new(
                payload.access_token,
                payload.refresh_token,
                payload.id_token,
                expires_at_ms,
            )
            .map_err(|_| {
                TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "invalid_response")
            })
        })
    }
}

pub(super) async fn collect_token_response(
    response: reqwest::Response,
) -> Result<Vec<u8>, TokenRefreshFailure> {
    let oversized =
        || TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "response_too_large");
    if response
        .content_length()
        .is_some_and(|length| length > MAX_TOKEN_RESPONSE_BYTES as u64)
    {
        return Err(oversized());
    }
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| {
            TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "transport")
        })?;
        if body.len().saturating_add(chunk.len()) > MAX_TOKEN_RESPONSE_BYTES {
            return Err(oversized());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
}
