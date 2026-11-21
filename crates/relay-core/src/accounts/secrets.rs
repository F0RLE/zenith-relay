//! Host-owned durable secret storage. Implementations must not log secret values.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SecretBackendError;

pub trait SecretBackend {
    fn save(&self, secret_ref: &str, secret_value: &str) -> Result<(), SecretBackendError>;
    fn load(&self, secret_ref: &str) -> Result<Option<String>, SecretBackendError>;
    fn delete(&self, secret_ref: &str) -> Result<(), SecretBackendError>;
}
