use super::{ISSUER, OAuthError, OAuthErrorCode};
use jsonwebtoken::{Algorithm, decode_header, jwk::JwkSet};
use std::{
    future::Future,
    sync::OnceLock,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const TTL: Duration = Duration::from_secs(300);
const RETRY_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Default)]
struct Cache {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    keys: Option<(Instant, JwkSet)>,
    attempted_at: Option<Instant>,
}

fn unavailable() -> OAuthError {
    let mut error = OAuthError::new(OAuthErrorCode::Transport, true);
    error.provider_code = Some("siwc_identity_verification_unavailable".into());
    error
}

impl Cache {
    async fn get<F, Fut>(&self, kid: &str, now: Instant, fetch: F) -> Result<JwkSet, OAuthError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<JwkSet, OAuthError>>,
    {
        // Serialize metadata retrieval, not signature validation or token refresh.
        let mut state = self.state.lock().await;
        if let Some((loaded_at, keys)) = &state.keys
            && now.saturating_duration_since(*loaded_at) < TTL
            && keys.find(kid).is_some()
        {
            return Ok(keys.clone());
        }
        if state
            .attempted_at
            .is_some_and(|at| now.saturating_duration_since(at) < RETRY_INTERVAL)
        {
            // A suppressed lookup is not evidence of an invalid identity.
            return Err(unavailable());
        }
        state.attempted_at = Some(now);
        let keys = match fetch().await {
            Ok(keys) if !keys.keys.is_empty() => keys,
            _ => return Err(unavailable()),
        };
        state.keys = Some((now, keys.clone()));
        if keys.find(kid).is_none() {
            return Err(OAuthError::new(OAuthErrorCode::InvalidJwt, false));
        }
        Ok(keys)
    }
}

pub(super) async fn for_token(http: &reqwest::Client, token: &str) -> Result<JwkSet, OAuthError> {
    let invalid = || OAuthError::new(OAuthErrorCode::InvalidJwt, false);
    let header = decode_header(token).map_err(|_| invalid())?;
    let kid = header
        .kid
        .filter(|kid| !kid.is_empty() && kid.len() <= 256)
        .ok_or_else(invalid)?;
    if header.alg != Algorithm::RS256 || kid.chars().any(char::is_control) {
        return Err(invalid());
    }
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE
        .get_or_init(Cache::default)
        .get(&kid, Instant::now(), || async {
            let (response, permit) =
                zenith_relay_core::scheduler::refresh::http::management_http_gate()
                    .send(
                        http,
                        http.get(format!("{ISSUER}/.well-known/jwks.json")),
                        zenith_relay_core::scheduler::refresh::http::HttpClass::Auth,
                    )
                    .await
                    .map_err(|_| unavailable())?;
            if !response.status().is_success() {
                return Err(unavailable());
            }
            let bytes = super::super::super::collect_limited(
                response,
                super::super::MAX_TOKEN_RESPONSE_BYTES,
            )
            .await
            .map_err(|_| unavailable())?;
            drop(permit);
            serde_json::from_slice(&bytes).map_err(|_| unavailable())
        })
        .await
}

#[cfg(test)]
#[path = "jwks_tests.rs"]
mod tests;
