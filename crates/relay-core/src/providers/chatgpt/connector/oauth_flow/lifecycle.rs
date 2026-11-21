use super::*;

/// The mutation guard serializes callback, exchange claim, checkpoint and
/// cancellation for this host. Implementations must not leak callback material.
pub trait AttemptHost {
    type Guard<'a>
    where
        Self: 'a;
    fn lock(&self) -> Self::Guard<'_>;
    fn read(&self, login_id: &str) -> Result<PendingSnapshot, OAuthFlowError>;
    fn write(&self, snapshot: &PendingSnapshot) -> Result<(), OAuthFlowError>;
    fn remove(&self, login_id: &str) -> Result<(), OAuthFlowError>;
    fn load_secret(&self, key: &str) -> Result<Option<String>, OAuthFlowError>;
    fn save_secret(&self, key: &str, value: &str) -> Result<(), OAuthFlowError>;
    fn delete_secret(&self, key: &str) -> Result<(), OAuthFlowError>;
    fn emit(&self, event: OAuthFlowEvent);
}

pub fn callback_secret_ref(login_id: &str) -> String {
    format!("oauth-callback:{login_id}")
}

fn emit(host: &impl AttemptHost, login_id: &str, status: OAuthFlowStatus) {
    host.emit(OAuthFlowEvent {
        login_id: login_id.into(),
        status,
        diagnostic: None,
    });
}

pub fn accept_callback(
    host: &impl AttemptHost,
    login_id: &str,
    url: &str,
    now_ms: u64,
) -> Result<(), OAuthFlowError> {
    let _guard = host.lock();
    let mut snapshot = host.read(login_id)?;
    if snapshot.status != OAuthFlowStatus::Pending {
        return Err(OAuthFlowError::new(
            OAuthFlowErrorCode::CallbackAlreadyReceived,
            "OAuth callback was already received",
        )
        .for_login(login_id));
    }
    if snapshot.pending.expires_at_ms() <= now_ms {
        return Err(
            OAuthFlowError::new(OAuthFlowErrorCode::Expired, "OAuth login expired")
                .for_login(login_id),
        );
    }
    if let Err(error) = snapshot.pending.parse_callback(url, now_ms) {
        if snapshot.pending.client_kind() == OAuthClientKind::Siwc
            && error.is_authorization_denied()
        {
            snapshot.status = OAuthFlowStatus::Failed;
            host.write(&snapshot)?;
            emit(host, login_id, OAuthFlowStatus::Failed);
            return Err(OAuthFlowError::new(
                OAuthFlowErrorCode::AuthorizationDenied,
                "ChatGPT authorization was declined",
            )
            .for_login(login_id));
        }
        return Err(OAuthFlowError::new(
            OAuthFlowErrorCode::CallbackInvalid,
            "OAuth callback is invalid",
        )
        .for_login(login_id));
    }
    host.save_secret(&snapshot.callback_secret_ref, url)?;
    snapshot.status = OAuthFlowStatus::CallbackReceived;
    if let Err(error) = host.write(&snapshot) {
        host.delete_secret(&snapshot.callback_secret_ref)
            .map_err(|_| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::RecoveryRequired,
                    "OAuth callback cleanup requires recovery",
                )
                .for_login(login_id)
            })?;
        return Err(error.for_login(login_id));
    }
    emit(host, login_id, OAuthFlowStatus::CallbackReceived);
    Ok(())
}

pub fn exchange_material(
    host: &impl AttemptHost,
    login_id: &str,
    now_ms: u64,
) -> Result<OAuthExchangeMaterial, OAuthFlowError> {
    let _guard = host.lock();
    let mut snapshot = host.read(login_id)?;
    if snapshot.status != OAuthFlowStatus::CallbackReceived {
        return Err(OAuthFlowError::new(
            OAuthFlowErrorCode::SecretMissing,
            "OAuth callback has not been received",
        )
        .for_login(login_id));
    }
    if snapshot.exchange_started {
        return Err(OAuthFlowError::new(
            OAuthFlowErrorCode::RecoveryRequired,
            "OAuth code exchange was already started; use its saved completion or restart sign-in",
        )
        .for_login(login_id));
    }
    if snapshot.pending.expires_at_ms() <= now_ms {
        return Err(
            OAuthFlowError::new(OAuthFlowErrorCode::Expired, "OAuth login expired")
                .for_login(login_id),
        );
    }
    let url = host
        .load_secret(&snapshot.callback_secret_ref)?
        .ok_or_else(|| {
            OAuthFlowError::new(
                OAuthFlowErrorCode::SecretMissing,
                "OAuth callback secret is missing",
            )
            .for_login(login_id)
        })?;
    let callback = snapshot.pending.parse_callback(&url, now_ms).map_err(|_| {
        OAuthFlowError::new(
            OAuthFlowErrorCode::CallbackInvalid,
            "OAuth callback is invalid",
        )
        .for_login(login_id)
    })?;
    snapshot.exchange_started = true;
    host.write(&snapshot)?;
    Ok(OAuthExchangeMaterial {
        pending: snapshot.pending,
        callback,
    })
}

/// Persist the validated token result only while the claimed attempt still
/// exists. Cancellation fences a late provider reply before it can recreate a secret.
pub fn store_exchange_checkpoint(
    host: &impl AttemptHost,
    login_id: &str,
    encoded: &str,
    now_ms: u64,
) -> Result<(), OAuthFlowError> {
    let _guard = host.lock();
    let snapshot = host.read(login_id)?;
    if snapshot.status != OAuthFlowStatus::CallbackReceived
        || !snapshot.exchange_started
        || snapshot.pending.expires_at_ms() <= now_ms
    {
        return Err(OAuthFlowError::new(
            OAuthFlowErrorCode::RecoveryRequired,
            "OAuth exchange attempt is no longer current",
        )
        .for_login(login_id));
    }
    host.save_secret(&snapshot.callback_secret_ref, encoded)?;
    if host.load_secret(&snapshot.callback_secret_ref)?.as_deref() != Some(encoded) {
        return Err(OAuthFlowError::new(
            OAuthFlowErrorCode::RecoveryRequired,
            "OAuth completion checkpoint could not be verified",
        )
        .for_login(login_id));
    }
    Ok(())
}

pub fn cleanup(host: &impl AttemptHost, login_id: &str) -> Result<(), OAuthFlowError> {
    let _guard = host.lock();
    cleanup_locked(host, login_id)
}

fn cleanup_locked(host: &impl AttemptHost, login_id: &str) -> Result<(), OAuthFlowError> {
    host.delete_secret(&callback_secret_ref(login_id))?;
    host.remove(login_id)
}

pub fn finish(
    host: &impl AttemptHost,
    login_id: &str,
    status: OAuthFlowStatus,
) -> Result<(), OAuthFlowError> {
    let _guard = host.lock();
    cleanup_locked(host, login_id)?;
    emit(host, login_id, status);
    Ok(())
}

pub fn report_diagnostic(
    host: &impl AttemptHost,
    login_id: &str,
    diagnostic: OAuthFlowDiagnostic,
    now_ms: u64,
) {
    let _guard = host.lock();
    let Ok(snapshot) = host.read(login_id) else {
        return;
    };
    if snapshot.pending.expires_at_ms() <= now_ms
        || !matches!(
            snapshot.status,
            OAuthFlowStatus::Pending | OAuthFlowStatus::CallbackReceived
        )
    {
        return;
    }
    host.emit(OAuthFlowEvent {
        login_id: login_id.into(),
        status: snapshot.status,
        diagnostic: Some(diagnostic),
    });
}

pub fn reusable(
    snapshot: &PendingSnapshot,
    client: OAuthClientKind,
    target: Option<&str>,
    notes: Option<&str>,
    now_ms: u64,
) -> bool {
    snapshot.target_account_id.as_deref() == target
        && snapshot.notes_account_id.as_deref() == notes
        && snapshot.pending.client_kind() == client
        && snapshot.pending.expires_at_ms() > now_ms
        && matches!(
            snapshot.status,
            OAuthFlowStatus::Pending | OAuthFlowStatus::CallbackReceived
        )
}

pub fn new_attempt(
    oauth: &CodexOAuthClient,
    login_id: String,
    port: u16,
    now_ms: u64,
    target: Option<&str>,
    notes: Option<&str>,
    proxy: Option<&str>,
) -> Result<PendingSnapshot, OAuthFlowError> {
    let start = oauth.begin(port, now_ms).map_err(|_| {
        OAuthFlowError::new(
            OAuthFlowErrorCode::ListenerUnavailable,
            "OAuth login could not be initialized",
        )
    })?;
    Ok(PendingSnapshot {
        version: SNAPSHOT_VERSION,
        callback_secret_ref: callback_secret_ref(&login_id),
        login_id,
        authorization_url: start.authorization_url().to_string(),
        status: OAuthFlowStatus::Pending,
        exchange_started: false,
        target_account_id: target.map(str::to_owned),
        notes_account_id: notes.map(str::to_owned),
        sign_in_proxy_id: proxy.map(str::to_owned),
        pending: start.into_pending(),
    })
}

#[cfg(test)]
mod tests;
