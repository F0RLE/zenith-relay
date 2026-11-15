mod error;
mod store;
mod stored;
mod totp;
mod wire;

pub use error::{CredentialError, CredentialErrorCode, bearer_authorization};
pub use store::{CredentialStore, credential_secret_ref};
pub use stored::{CredentialRefresh, StoredCodexCredentials};
pub use totp::totp_code;

pub fn require_plan_permission(credentials: &StoredCodexCredentials) -> Result<(), &'static str> {
    if credentials.oauth_client_kind() == super::oauth::OAuthClientKind::Siwc
        && !credentials
            .siwc_registration()
            .is_some_and(|registration| registration.plan_use_enabled())
    {
        return Err("ChatGPT plan use is disabled. Sign in again to authorize plan usage.");
    }
    Ok(())
}

pub fn validate_pool_account(
    credentials: &StoredCodexCredentials,
    image_pool_enabled: bool,
) -> Result<(), &'static str> {
    if credentials.oauth_client_kind() == super::oauth::OAuthClientKind::Codex
        && !image_pool_enabled
    {
        return Err(
            "Enable this Codex connection explicitly for image generation before adding it to the pool.",
        );
    }
    require_plan_permission(credentials)
}

#[cfg(test)]
mod tests;
