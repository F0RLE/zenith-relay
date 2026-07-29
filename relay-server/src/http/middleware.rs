use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
};
use subtle::ConstantTimeEq;
use zenith_relay_core::protocol::{ApiError, ErrorEnvelope};

const MAX_FAILURES: u8 = 5;
const BLOCK_MS: u64 = 60_000;

#[derive(Clone)]
pub struct ManagementAuth {
    inner: Arc<ManagementAuthInner>,
}

struct ManagementAuthInner {
    expected_hash: [u8; 32],
    failures: Mutex<HashMap<IpAddr, FailureState>>,
}

#[derive(Clone, Copy)]
struct FailureState {
    failures: u8,
    blocked_until_ms: u64,
}

impl ManagementAuth {
    pub fn new(token: &str) -> Self {
        Self {
            inner: Arc::new(ManagementAuthInner {
                expected_hash: Sha256::digest(token.as_bytes()).into(),
                failures: Mutex::new(HashMap::new()),
            }),
        }
    }

    fn authorize(&self, ip: IpAddr, token: Option<&str>, now_ms: u64) -> AuthResult {
        let mut failures = match self.inner.failures.lock() {
            Ok(failures) => failures,
            Err(_) => return AuthResult::Blocked,
        };
        let supplied: [u8; 32] = Sha256::digest(token.unwrap_or_default().as_bytes()).into();
        if self.inner.expected_hash.ct_eq(&supplied).into() {
            failures.remove(&ip);
            return AuthResult::Allowed;
        }
        if failures
            .get(&ip)
            .is_some_and(|state| state.blocked_until_ms > now_ms)
        {
            return AuthResult::Blocked;
        }
        let state = failures.entry(ip).or_insert(FailureState {
            failures: 0,
            blocked_until_ms: 0,
        });
        state.failures = state.failures.saturating_add(1);
        if state.failures >= MAX_FAILURES {
            state.blocked_until_ms = now_ms.saturating_add(BLOCK_MS);
            state.failures = 0;
        }
        AuthResult::Denied
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AuthResult {
    Allowed,
    Denied,
    Blocked,
}

pub async fn require_management(
    State(auth): State<ManagementAuth>,
    request: Request,
    next: Next,
) -> Response {
    let ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|value| value.0.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let token = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match auth.authorize(ip, token, crate::state::now_ms()) {
        AuthResult::Allowed => next.run(request).await,
        AuthResult::Denied => auth_error(StatusCode::UNAUTHORIZED, "management_unauthorized"),
        AuthResult::Blocked => auth_error(StatusCode::TOO_MANY_REQUESTS, "management_blocked"),
    }
}

fn auth_error(status: StatusCode, code: &str) -> Response {
    (
        status,
        Json(ErrorEnvelope {
            error: ApiError {
                code: code.to_string(),
                message: "management authentication failed".to_string(),
                stage: "management_auth".to_string(),
                retryable: status == StatusCode::TOO_MANY_REQUESTS,
                request_id: uuid::Uuid::new_v4().to_string(),
            },
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn management_auth_accepts_only_its_token_and_blocks_repeated_failures() {
        let auth = ManagementAuth::new("synthetic-management-token-value");
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert_eq!(
            auth.authorize(ip, Some("synthetic-management-token-value"), 1),
            AuthResult::Allowed
        );
        for now in 2..=6 {
            assert_eq!(
                auth.authorize(ip, Some("local-pool-key"), now),
                AuthResult::Denied
            );
        }
        assert_eq!(
            auth.authorize(ip, Some("local-pool-key"), 7),
            AuthResult::Blocked
        );
        assert_eq!(
            auth.authorize(ip, Some("synthetic-management-token-value"), 8),
            AuthResult::Allowed
        );
        assert_eq!(
            auth.authorize(ip, Some("local-pool-key"), 9),
            AuthResult::Denied
        );
    }
}
