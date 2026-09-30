//! Local gateway key authentication for one running runtime.

use super::*;

impl GatewayRuntime {
    pub(crate) fn authenticate(
        &self,
        authorization: Option<&HeaderValue>,
    ) -> Option<AuthenticatedKey> {
        let secret = authorization
            .and_then(|value| value.to_str().ok())
            .and_then(parse_bearer)?;
        self.authenticate_secret(secret)
    }

    pub(crate) fn authenticate_secret(&self, secret: &str) -> Option<AuthenticatedKey> {
        if secret.is_empty() || secret.len() > 4_096 {
            return None;
        }
        let candidate: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
        self.keys
            .iter()
            .find(|key| key.enabled && bool::from(candidate.ct_eq(&key.secret_hash)))
            .map(|key| self.authenticated_key(key))
    }

    /// Creates an ephemeral key scope for scheduler-owned work on exactly one
    /// OAuth account.  Background probes must use the same scheduler,
    /// cooldowns, token authority, and usage callback as normal gateway
    /// requests, but they must never inherit the user's broad pool scope or
    /// fall back to a different account.
    pub(crate) fn internal_account_key(
        &self,
        local_key_id: &str,
        account_id: &str,
    ) -> Option<AuthenticatedKey> {
        let local_key_id = local_key_id.trim();
        let account_id = account_id.trim();
        if local_key_id.is_empty() || account_id.is_empty() {
            return None;
        }
        self.chatgpt_accounts.get(account_id)?;
        Some(AuthenticatedKey {
            id: local_key_id.to_string(),
            scope: Arc::new(RwLock::new(CandidateScope {
                // An explicit empty source set prevents a synthetic internal
                // key from selecting an API source while the account set below
                // pins selection to the requested OAuth candidate.
                source_ids: Some(BTreeSet::new()),
                account_ids: Some(BTreeSet::from([account_id.to_string()])),
                model_rules: ModelRules::default(),
            })),
            scope_revision: Arc::new(AtomicU64::new(0)),
            model_rules: ModelRules::default(),
            model_prefix: None,
            client_wire_apis: Some(vec![ClientWireApi::Responses]),
        })
    }

    pub(super) fn authenticated_key(&self, key: &RuntimeKey) -> AuthenticatedKey {
        AuthenticatedKey {
            id: key.id.clone(),
            scope: key.scope.clone(),
            scope_revision: key.scope_revision.clone(),
            model_rules: key.model_rules.clone(),
            model_prefix: key.model_prefix.clone(),
            client_wire_apis: key.client_wire_apis.clone(),
        }
    }

    pub(crate) fn allows_client_wire_api(
        &self,
        key: &AuthenticatedKey,
        wire_api: ClientWireApi,
    ) -> bool {
        key.client_wire_apis
            .as_ref()
            .is_none_or(|allowed| allowed.contains(&wire_api))
    }
}
