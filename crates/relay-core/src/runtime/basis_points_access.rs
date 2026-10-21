//! Fresh BPS access is bound to the executor's proxy and exact credential incarnation.
use super::{AuthenticatedKey, GatewayRuntime, PreparedAuthorization};
use crate::accounts::TokenDispatchRevision;
use crate::providers::chatgpt::{
    basis_points_access_url, basis_points_headers, parse_basis_points_model_access,
    BasisPointsModelAccess, ModelDiscoveryFailure, ModelDiscoveryFailureCode, OAuthClientKind,
    BASIS_POINTS_RATE_LIMIT_COOLDOWN_MS, MAX_BASIS_POINTS_ACCESS_BYTES,
};
use futures_util::{stream, StreamExt};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const ACCESS_TTL: Duration = Duration::from_secs(60);
const ACCESS_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(super) struct CachedBasisPointsAccess {
    result: Result<BasisPointsModelAccess, ModelDiscoveryFailure>,
    revision: TokenDispatchRevision,
    expires_at: Instant,
}

#[cfg(test)]
impl CachedBasisPointsAccess {
    pub(super) fn expire(&mut self) {
        self.expires_at = Instant::now();
    }
}

/// Catalog filtering uses a snapshot taken before the scheduler lock. Credential
/// guards precede that lock during dispatch and access publication.
pub(super) struct BasisPointsModelSnapshot {
    models_by_account: HashMap<String, HashSet<String>>,
}

impl BasisPointsModelSnapshot {
    pub(super) fn model_available(&self, candidate_id: &str, source_model: &str) -> bool {
        self.models_by_account
            .get(candidate_id)
            .is_none_or(|models| models.contains(&crate::model_id_key(source_model)))
    }
}

fn failure(code: ModelDiscoveryFailureCode, retryable: bool) -> ModelDiscoveryFailure {
    ModelDiscoveryFailure {
        code,
        retryable,
        http_status: None,
        retry_after_ms: None,
    }
}

impl GatewayRuntime {
    pub(crate) fn is_siwc_account(&self, candidate_id: &str) -> bool {
        self.chatgpt_accounts
            .get(candidate_id)
            .is_some_and(|account| account.oauth_client_kind == OAuthClientKind::Siwc)
    }
    pub fn is_basis_points_account(&self, candidate_id: &str) -> bool {
        self.chatgpt_accounts
            .get(candidate_id)
            .is_some_and(|account| account.oauth_client_kind == OAuthClientKind::ExcelBps)
    }

    pub(super) fn basis_points_model_snapshot(&self) -> BasisPointsModelSnapshot {
        let models_by_account = self
            .chatgpt_accounts
            .values()
            .filter(|account| account.oauth_client_kind == OAuthClientKind::ExcelBps)
            .map(|account| {
                let models = self
                    .fresh_basis_points_access(&account.id)
                    .into_iter()
                    .flat_map(|access| access.models)
                    .map(|model| crate::model_id_key(&model.id))
                    .collect();
                (account.id.clone(), models)
            })
            .collect();
        BasisPointsModelSnapshot { models_by_account }
    }

    pub(crate) fn fresh_basis_points_access(
        &self,
        candidate_id: &str,
    ) -> Option<BasisPointsModelAccess> {
        self.cached_basis_points_access(candidate_id)?.result.ok()
    }

    fn cached_basis_points_access(&self, candidate_id: &str) -> Option<CachedBasisPointsAccess> {
        let account = self.chatgpt_accounts.get(candidate_id)?;
        // Release the cache lock before acquiring the credential guard.
        // Publication holds the credential guard before editing this cache.
        let mut cached = crate::poison::read(&account.basis_points_access).clone()?;
        let remaining = cached.expires_at.checked_duration_since(Instant::now())?;
        if remaining.is_zero() {
            return None;
        }
        drop(cached.revision.guard()?);
        if let Err(error) = &mut cached.result {
            if error.code == ModelDiscoveryFailureCode::RateLimited
                || error.retry_after_ms.is_some()
            {
                // Reusing a failed read must not restart its original Retry-After.
                error.retry_after_ms = Some(remaining.as_millis().min(u128::from(u64::MAX)) as u64);
            }
        }
        Some(cached)
    }

    /// A failed access check hides executable permissions, not the reason the
    /// configured route is unavailable. Let execution report that failure even
    /// when automatic route recovery is disabled.
    pub(crate) fn basis_points_access_failed_for_model(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        protocols: &[crate::WireApi],
    ) -> bool {
        let scope = key.scope_snapshot();
        let candidate_ids = self
            .lock_scheduler()
            .candidates()
            .filter(|candidate| {
                self.is_basis_points_account(&candidate.id)
                    && candidate.is_configured(model, protocols, &scope)
            })
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        candidate_ids.iter().any(|candidate_id| {
            self.cached_basis_points_access(candidate_id)
                .is_some_and(|cached| cached.result.is_err())
        })
    }

    pub(crate) fn basis_points_reasoning_available(
        &self,
        candidate_id: &str,
        model: &str,
        effort: &str,
    ) -> bool {
        !self.is_basis_points_account(candidate_id)
            || self
                .fresh_basis_points_access(candidate_id)
                .and_then(|access| {
                    access
                        .models
                        .into_iter()
                        .find(|entry| entry.id.eq_ignore_ascii_case(model))
                })
                .is_some_and(|entry| entry.supports_reasoning_effort(effort))
    }

    pub(super) async fn prepare_basis_points_authorization(
        &self,
        candidate_id: &str,
    ) -> Result<PreparedAuthorization, super::AuthorizedRequestError> {
        self.validate_basis_points_plan(candidate_id, None)
            .map_err(super::AuthorizedRequestError::ModelAccess)?;
        let mut prepared = self
            .prepare_authorization(candidate_id, super::runtime_now_ms())
            .await
            .map_err(super::AuthorizedRequestError::Prepare)?;
        let mut access = self.read_basis_points_access(candidate_id, &prepared).await;
        if access
            .as_ref()
            .err()
            .is_some_and(|error| error.code == ModelDiscoveryFailureCode::Unauthorized)
        {
            // Discovery is management I/O, so a proven 401 can be repaired
            // without spending a generation or replaying an accepted request.
            prepared = self
                .refresh_authorization_after_unauthorized(
                    candidate_id,
                    &prepared,
                    super::runtime_now_ms(),
                )
                .await
                .map_err(super::AuthorizedRequestError::Prepare)?;
            access = self.read_basis_points_access(candidate_id, &prepared).await;
        }
        access.map_err(super::AuthorizedRequestError::ModelAccess)?;
        Ok(prepared)
    }

    /// Refresh scoped accounts concurrently without creating a second background
    /// owner. Cancellation drops the HTTP response and the per-account lock.
    pub(crate) async fn refresh_basis_points_access(&self, key: &AuthenticatedKey) {
        let scope = key.scope_snapshot();
        let candidate_ids = {
            let scheduler = self.lock_scheduler();
            self.chatgpt_accounts
                .values()
                .filter(|account| {
                    account.oauth_client_kind == OAuthClientKind::ExcelBps
                        && scheduler
                            .candidate(&account.id)
                            .is_some_and(|candidate| candidate.is_discovery_visible(&scope))
                })
                .map(|account| account.id.clone())
                .collect::<Vec<_>>()
        };
        let refreshes = stream::iter(candidate_ids.into_iter().map(|candidate_id| async move {
            if self.cached_basis_points_access(&candidate_id).is_none() {
                let _ = self.prepare_basis_points_authorization(&candidate_id).await;
            }
        }))
        .buffer_unordered(4)
        .collect::<Vec<_>>();
        let _ = tokio::time::timeout(Duration::from_secs(12), refreshes).await;
    }

    pub(super) async fn read_basis_points_access(
        &self,
        candidate_id: &str,
        prepared: &PreparedAuthorization,
    ) -> Result<BasisPointsModelAccess, ModelDiscoveryFailure> {
        self.validate_basis_points_plan(candidate_id, Some(prepared))?;
        let account = self
            .chatgpt_accounts
            .get(candidate_id)
            .ok_or_else(|| failure(ModelDiscoveryFailureCode::InvalidAccountId, false))?;
        let revision = prepared
            .token_revision
            .as_ref()
            .ok_or_else(|| failure(ModelDiscoveryFailureCode::InvalidAccessToken, false))?;
        let _refresh = account.basis_points_access_refresh.lock().await;
        if let Some(cached) = self
            .cached_basis_points_access(candidate_id)
            .filter(|cache| cache.revision == *revision)
        {
            return cached.result;
        }
        let result = self.fetch_basis_points_access(candidate_id, prepared).await;
        let _guard = prepared
            .dispatch_guard(self, candidate_id)
            .ok_or_else(|| failure(ModelDiscoveryFailureCode::Transport, true))?;
        let cache_ttl = match &result {
            Ok(access) => {
                if !self.update_account_models(candidate_id, &access.model_ids()) {
                    return Err(failure(ModelDiscoveryFailureCode::Transport, true));
                }
                ACCESS_TTL
            }
            // Coalesce concurrent failed reads too, respecting Retry-After.
            // A failure never publishes stale permissions or erases inventory.
            Err(error) => Duration::from_millis(
                error
                    .retry_after_ms
                    .unwrap_or(if error.code == ModelDiscoveryFailureCode::RateLimited {
                        BASIS_POINTS_RATE_LIMIT_COOLDOWN_MS
                    } else {
                        1_000
                    })
                    .max(1_000),
            ),
        };
        let expires_at = Instant::now()
            .checked_add(cache_ttl)
            .unwrap_or_else(|| Instant::now() + ACCESS_TTL);
        *crate::poison::write(&account.basis_points_access) = Some(CachedBasisPointsAccess {
            result: result.clone(),
            revision: revision.clone(),
            expires_at,
        });
        if result
            .as_ref()
            .err()
            .is_some_and(|error| error.code == ModelDiscoveryFailureCode::RateLimited)
        {
            // Access throttling blocks this BPS connection's entire endpoint,
            // without changing quota or the separately stored native login.
            self.lock_scheduler().set_cooldown_with_reason(
                candidate_id,
                "*",
                crate::unix_time_ms().saturating_add(cache_ttl.as_millis() as u64),
                crate::scheduler::CooldownReason::RateLimit,
            );
            self.candidate_availability.notify_waiters();
        } else if result.is_ok() {
            self.clear_candidate_capability_blocks(candidate_id);
        }
        result
    }

    /// Update entitlement metadata without replacing the scheduler or active leases.
    pub fn update_account_subscription_plan(
        &self,
        account_id: &str,
        plan_type: Option<&str>,
    ) -> bool {
        let Some(account) = self.chatgpt_accounts.get(account_id) else {
            return false;
        };
        let mut current = crate::poison::write(&account.subscription_plan_type);
        if current.as_deref() == plan_type {
            return true;
        }
        let _fence = self.fence_candidate_dispatch(account_id);
        *current = plan_type.map(str::to_owned);
        *crate::poison::write(&account.basis_points_access) = None;
        if !account.oauth_client_kind.supports_pool(plan_type) {
            let mut scheduler = self.lock_scheduler();
            if let Some(candidate) = scheduler.candidate(account_id).cloned() {
                scheduler.update_candidate_availability(
                    account_id,
                    false,
                    candidate.health,
                    candidate.quota,
                );
            }
        }
        self.candidate_availability.notify_waiters();
        true
    }

    pub(in crate::runtime) fn validate_basis_points_plan(
        &self,
        candidate_id: &str,
        prepared: Option<&PreparedAuthorization>,
    ) -> Result<(), ModelDiscoveryFailure> {
        let account = self
            .chatgpt_accounts
            .get(candidate_id)
            .ok_or_else(|| failure(ModelDiscoveryFailureCode::InvalidAccountId, false))?;
        if prepared.is_some_and(|prepared| !prepared.account_plan_supported) {
            return Err(failure(
                ModelDiscoveryFailureCode::BasisPointsFreeAccountUnsupported,
                false,
            ));
        }
        let plan = crate::poison::read(&account.subscription_plan_type);
        let access_token = prepared
            .and_then(|prepared| prepared.authorization.to_str().ok())
            .and_then(|header| header.strip_prefix("Bearer "));
        account
            .oauth_client_kind
            .validate_plan(plan.as_deref(), None, access_token)
            .map_err(|_| {
                failure(
                    ModelDiscoveryFailureCode::BasisPointsFreeAccountUnsupported,
                    false,
                )
            })
    }

    async fn fetch_basis_points_access(
        &self,
        candidate_id: &str,
        prepared: &PreparedAuthorization,
    ) -> Result<BasisPointsModelAccess, ModelDiscoveryFailure> {
        let account = self
            .chatgpt_accounts
            .get(candidate_id)
            .ok_or_else(|| failure(ModelDiscoveryFailureCode::InvalidAccountId, false))?;
        let url = basis_points_access_url(&account.basis_points_url)
            .ok_or_else(|| failure(ModelDiscoveryFailureCode::InvalidEndpoint, false))?;
        if prepared.dispatch_guard(self, candidate_id).is_none() {
            return Err(failure(ModelDiscoveryFailureCode::Transport, true));
        }
        let response = account
            .clients
            .http
            .get(url)
            .headers(basis_points_headers(
                &account.chatgpt_account_id,
                account.chatgpt_user_id.as_deref(),
                account.basis_points_headers.as_ref(),
            ))
            .header(prepared.header_name.clone(), prepared.authorization.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(ACCESS_TIMEOUT)
            .send()
            .await
            .map_err(|error| {
                failure(
                    if error.is_timeout() {
                        ModelDiscoveryFailureCode::Timeout
                    } else {
                        ModelDiscoveryFailureCode::Transport
                    },
                    true,
                )
            })?;
        let status = response.status();
        if !status.is_success() {
            let code = match status.as_u16() {
                401 => ModelDiscoveryFailureCode::Unauthorized,
                403 => ModelDiscoveryFailureCode::Forbidden,
                429 => ModelDiscoveryFailureCode::RateLimited,
                _ if status.is_server_error() => ModelDiscoveryFailureCode::Upstream,
                _ => ModelDiscoveryFailureCode::HttpStatus,
            };
            let header_delay =
                crate::transport::retry_after_ms(response.headers(), std::time::SystemTime::now());
            // Keep bounded error bodies only long enough to extract a retry
            // deadline; malformed bodies must not replace the proven status.
            return Err(ModelDiscoveryFailure {
                code,
                retryable: status.as_u16() == 429 || status.is_server_error(),
                http_status: Some(status.as_u16()),
                retry_after_ms: header_delay,
            });
        }
        let bytes = crate::transport::collect_limited(response, MAX_BASIS_POINTS_ACCESS_BYTES)
            .await
            .map_err(|error| {
                failure(
                    match error {
                        crate::Error::UpstreamBodyTooLarge => {
                            ModelDiscoveryFailureCode::ResponseTooLarge
                        }
                        crate::Error::Upstream(error) if error.is_timeout() => {
                            ModelDiscoveryFailureCode::Timeout
                        }
                        _ => ModelDiscoveryFailureCode::Transport,
                    },
                    true,
                )
            })?;
        parse_basis_points_model_access(&bytes)
    }

    pub(super) async fn verify_basis_points_request(
        &self,
        candidate_id: &str,
        prepared: &PreparedAuthorization,
        request: &reqwest::Request,
    ) -> Result<(), super::AuthorizedRequestError> {
        if !self.is_basis_points_account(candidate_id) {
            return Ok(());
        }
        let access = self
            .read_basis_points_access(candidate_id, prepared)
            .await
            .map_err(super::AuthorizedRequestError::ModelAccess)?;
        let body = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok());
        let Some(model) = body
            .as_ref()
            .and_then(|body| body.get("model"))
            .and_then(Value::as_str)
        else {
            return Ok(());
        };
        let Some(available) = access
            .models
            .iter()
            .find(|entry| entry.id.eq_ignore_ascii_case(model))
        else {
            return Err(super::AuthorizedRequestError::ModelUnavailable);
        };
        if let Some(effort) = body
            .as_ref()
            .and_then(|body| body.get("reasoning_effort"))
            .and_then(Value::as_str)
        {
            if !available.supports_reasoning_effort(effort) {
                return Err(super::AuthorizedRequestError::ReasoningUnavailable);
            }
        }
        Ok(())
    }
}
