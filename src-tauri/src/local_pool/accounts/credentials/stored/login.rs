use super::super::error::{CredentialError, CredentialErrorCode};
use super::StoredCodexCredentials;
use zenith_relay_core::accounts::{
    normalize_login_email, normalize_login_password, normalize_login_phone,
    normalize_login_totp_secret,
};

impl StoredCodexCredentials {
    pub(in crate::local_pool::accounts) fn apply_stored_login_material(
        mut self,
        phone: Option<String>,
        password: Option<String>,
        totp_secret: Option<String>,
    ) -> Self {
        self.phone = phone.and_then(|phone_value| normalize_login_phone(&phone_value));
        self.password =
            password.and_then(|password_value| normalize_login_password(&password_value));
        self.totp_secret = totp_secret
            .and_then(|totp_secret_value| normalize_login_totp_secret(&totp_secret_value));
        self
    }

    pub(crate) fn fill_missing_login_from(mut self, previous_login: &Self) -> Self {
        if self.phone.is_none() {
            self.phone = previous_login.phone.clone();
        }
        if self.password.is_none() {
            self.password = previous_login.password.clone();
        }
        if self.totp_secret.is_none() {
            self.totp_secret = previous_login.totp_secret.clone();
        }
        self
    }

    pub(in crate::local_pool::accounts) fn replace_login_notes(
        mut self,
        email: String,
        phone: String,
        password: String,
        totp_secret: String,
    ) -> Result<Self, CredentialError> {
        self.email = required_note(&email, normalize_login_email)?;
        self.phone = required_note(&phone, normalize_login_phone)?;
        self.password = required_note(&password, normalize_login_password)?;
        self.totp_secret = required_note(&totp_secret, normalize_login_totp_secret)?;
        Ok(self)
    }
}

fn required_note(
    note_text: &str,
    normalize: fn(&str) -> Option<String>,
) -> Result<Option<String>, CredentialError> {
    let trimmed = note_text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    normalize(trimmed).map(Some).ok_or_else(|| {
        CredentialError::new(
            CredentialErrorCode::InvalidSecret,
            "account login note is invalid",
        )
    })
}
