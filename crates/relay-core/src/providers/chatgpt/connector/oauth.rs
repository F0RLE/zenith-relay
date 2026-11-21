mod authorization;
mod client;
mod error;
mod exchange;
mod identity;
mod parse;
mod session;
pub mod siwc;

#[cfg(test)]
pub use crate::providers::chatgpt::CODEX_OAUTH_CLIENT_ID;
pub use crate::providers::chatgpt::{BASIS_POINTS_OAUTH_REDIRECT_URI, OAuthClientKind};
pub use authorization::validate_authorization_url;
pub use client::CodexOAuthClient;
pub use error::OAuthError;
pub use error::OAuthErrorCode;
pub use exchange::{OAuthCallback, OAuthTokenSet};
pub use session::OAuthPendingSession;

pub const CODEX_OAUTH_ISSUER: &str = "https://auth.openai.com";
#[cfg(test)]
pub const CODEX_OAUTH_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const CODEX_OAUTH_ORIGINATOR: &str = "codex_cli_rs";
pub const CODEX_OAUTH_CALLBACK_PORTS: [u16; 2] = [1455, 1457];

const CALLBACK_PATH: &str = "/auth/callback";
const MAX_CALLBACK_URL_BYTES: usize = 8 * 1024;
const MAX_TOKEN_BYTES: usize = 64 * 1024;
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;
const PENDING_TTL_MS: u64 = 15 * 60 * 1_000;

#[cfg(test)]
mod tests;
