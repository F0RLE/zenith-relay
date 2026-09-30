use super::*;

impl PreparedAuthorization {
    pub(crate) fn incarnation(&self) -> AuthorizationIncarnation {
        if let Some(revision) = &self.token_revision {
            AuthorizationIncarnation::OAuth(revision.clone())
        } else if let Some(revision) = self.agent_identity_revision {
            AuthorizationIncarnation::Agent(revision)
        } else {
            AuthorizationIncarnation::Source
        }
    }

    /// Hold the credential's incarnation through the same budget/scheduler
    /// transaction that starts a generation. No async lock or provider I/O is
    /// performed under this guard.
    pub(in crate::runtime) fn dispatch_guard<'a>(
        &'a self,
        runtime: &'a GatewayRuntime,
        candidate_id: &str,
    ) -> Option<PreparedAuthorizationDispatchGuard<'a>> {
        if let Some(revision) = &self.token_revision {
            let account = runtime.chatgpt_accounts.get(candidate_id)?;
            if !account.active.load(Ordering::Acquire) {
                return None;
            }
            return Some(PreparedAuthorizationDispatchGuard {
                _token: Some(revision.guard()?),
                _agent: None,
            });
        }
        if let Some(expected) = self.agent_identity_revision {
            let account = runtime.chatgpt_accounts.get(candidate_id)?;
            let identity = account.agent_identity.read().ok()?;
            if !account.active.load(Ordering::Acquire)
                || account.agent_identity_revision.load(Ordering::Acquire) != expected
                || identity.as_ref().map(agent_credential_fingerprint)
                    != self.agent_credential_fingerprint
            {
                return None;
            }
            return Some(PreparedAuthorizationDispatchGuard {
                _token: None,
                _agent: Some(identity),
            });
        }
        runtime.source_candidate_bindings.get(candidate_id)?;
        Some(PreparedAuthorizationDispatchGuard {
            _token: None,
            _agent: None,
        })
    }

    pub(crate) fn credential_fingerprint(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        Sha256::digest(self.turn_state_credential()).into()
    }

    pub(in crate::runtime::authorization) fn turn_state_credential(&self) -> &[u8] {
        self.agent_credential_fingerprint.as_ref().map_or_else(
            || self.authorization.as_bytes(),
            |fingerprint| fingerprint.as_slice(),
        )
    }
}

/// Build the request before adding account authorization so the existing
/// downstream headers can be inspected and preserved. `RequestBuilder::headers`
/// cannot express that distinction: applying a second header map replaces the
/// client's identity even when it was already valid.
pub(super) fn apply_prepared_authorization(
    request: reqwest::RequestBuilder,
    prepared: &PreparedAuthorization,
    client_version: Option<&str>,
) -> std::result::Result<(reqwest::Client, reqwest::Request), AuthorizedRequestError> {
    let (client, request) = request.build_split();
    let mut request = request.map_err(AuthorizedRequestError::Transport)?;
    request
        .headers_mut()
        .insert(prepared.header_name.clone(), prepared.authorization.clone());
    if let Some(identity) = prepared.identity.as_ref() {
        // A model-catalog request has no forwarded client headers, so its
        // requested version is a useful fallback. For normal routed requests
        // the explicit downstream identity remains authoritative.
        let identity = match client_version {
            Some(version) => identity
                .with_client_version(version)
                .map_err(|_| AuthorizedRequestError::NotReplayable)?,
            None => identity.clone(),
        };
        identity.insert(request.headers_mut());
    }
    Ok((client, request))
}
