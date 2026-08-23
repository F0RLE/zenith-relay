pub(crate) mod authority;
pub(crate) mod credentials;
pub(crate) mod export_ops;
pub(crate) mod exports;
pub(crate) mod import_orchestrator;
pub(crate) mod import_session;
pub(crate) mod mutations;
pub(crate) mod oauth;
pub(crate) mod oauth_flow;
pub(crate) mod proxy;
pub(crate) mod quota_refresh;
pub(crate) mod quota_service;
pub(crate) mod records;
pub(crate) mod reset_credits;
pub(crate) mod wake;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LimitedBodyError {
    Transport,
    TooLarge,
}

pub(crate) async fn collect_limited(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, LimitedBodyError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(LimitedBodyError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| LimitedBodyError::Transport)?
    {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(LimitedBodyError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod operation_tests;

#[derive(Clone, Copy, Default)]
pub(crate) struct NativeSecretBackend;

impl import_session::SecretBackend for NativeSecretBackend {
    fn save(
        &self,
        secret_ref: &str,
        value: &str,
    ) -> Result<(), import_session::SecretBackendError> {
        crate::local_pool::store::secret_store::save(secret_ref, value)
            .map_err(|_| import_session::SecretBackendError)
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>, import_session::SecretBackendError> {
        crate::local_pool::store::secret_store::load(secret_ref)
            .map_err(|_| import_session::SecretBackendError)
    }

    fn delete(&self, secret_ref: &str) -> Result<(), import_session::SecretBackendError> {
        crate::local_pool::store::secret_store::delete(secret_ref)
            .map_err(|_| import_session::SecretBackendError)
    }
}
