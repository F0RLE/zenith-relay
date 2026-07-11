pub(crate) mod authority;
pub(crate) mod credentials;
pub(crate) mod exports;
pub(crate) mod import_session;
pub(crate) mod imports;
pub(crate) mod models;
pub(crate) mod oauth;
pub(crate) mod oauth_flow;
pub(crate) mod proxy;
pub(crate) mod quota;
pub(crate) mod quota_service;
pub(crate) mod records;
pub(crate) mod wake;

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
