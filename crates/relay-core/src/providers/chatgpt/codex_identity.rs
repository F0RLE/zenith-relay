use reqwest::header::{HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use std::{
    env,
    sync::{LazyLock, RwLock},
};

// Relay-owned OAuth calls start on the last verified stable Rust Codex release
// and may advance only to a newer stable release fetched from the official
// release feed. API-key routes preserve the downstream client's identity
// headers instead.
pub const CODEX_STABLE_FALLBACK_VERSION: &str = "0.154.0";
pub const CODEX_CLIENT_VERSION: &str = CODEX_STABLE_FALLBACK_VERSION;
pub const CODEX_ORIGINATOR: &str = "codex_cli_rs";

static CONFIGURED_CODEX_CLIENT_VERSION: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new(CODEX_CLIENT_VERSION.to_string()));

/// Returns the newest verified stable Codex Rust release configured for this
/// process. A built-in fallback remains available before the first network
/// refresh or when GitHub cannot be reached.
pub fn configured_codex_client_version() -> String {
    CONFIGURED_CODEX_CLIENT_VERSION
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Replaces the process-wide Codex release after the host verifies it from the
/// official release feed. Pre-release tags, delayed results, and equal versions
/// are ignored, so the process can never downgrade or move onto an alpha/beta.
pub fn configure_codex_client_version(value: &str) -> bool {
    let mut configured = CONFIGURED_CODEX_CLIENT_VERSION
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !is_newer_official_release(value, &configured) {
        return false;
    }
    *configured = value.to_string();
    true
}

fn is_newer_official_release(candidate: &str, configured: &str) -> bool {
    let (Ok(candidate), Ok(configured)) = (
        semver::Version::parse(candidate),
        semver::Version::parse(configured),
    ) else {
        return false;
    };
    candidate.pre.is_empty() && configured.pre.is_empty() && candidate > configured
}

#[derive(Clone, Debug)]
pub struct CodexIdentityEnvelope {
    account_id: HeaderValue,
    originator: HeaderValue,
    user_agent: HeaderValue,
    version: HeaderValue,
    client_version: String,
}

impl CodexIdentityEnvelope {
    pub fn standard(account_id: &str) -> Result<Self, &'static str> {
        let client_version = configured_codex_client_version();
        Self::new(account_id, &client_version)
    }

    pub fn new(account_id: &str, client_version: &str) -> Result<Self, &'static str> {
        if account_id.is_empty() || account_id.len() > 512 {
            return Err("ChatGPT account id is invalid");
        }
        if !valid_codex_client_version(client_version) {
            return Err("Codex client version is invalid");
        }
        let mut account_id = HeaderValue::from_str(account_id)
            .map_err(|_| "ChatGPT account id contains invalid header characters")?;
        account_id.set_sensitive(true);
        let user_agent = HeaderValue::from_str(&codex_user_agent_for_current_host(client_version))
            .map_err(|_| "Codex user agent is invalid")?;
        let version =
            HeaderValue::from_str(client_version).map_err(|_| "Codex client version is invalid")?;
        Ok(Self {
            account_id,
            originator: HeaderValue::from_static(CODEX_ORIGINATOR),
            user_agent,
            version,
            client_version: client_version.to_string(),
        })
    }

    pub fn client_version(&self) -> &str {
        &self.client_version
    }

    pub fn with_client_version(&self, client_version: &str) -> Result<Self, &'static str> {
        Self::new(
            self.account_id
                .to_str()
                .map_err(|_| "ChatGPT account id is invalid")?,
            client_version,
        )
    }

    /// Rebuilds the envelope with the current process release while retaining
    /// only the already-validated account header. This avoids stale identity
    /// headers in a gateway that was started before a background update.
    pub fn with_configured_client_version(&self) -> Result<Self, &'static str> {
        let client_version = configured_codex_client_version();
        self.with_client_version(&client_version)
    }

    /// Applies Relay's owned account binding and fallback identity to a
    /// request created outside the gateway forwarding path.
    ///
    /// This keeps discovery and quota probes on the same header contract as
    /// forwarded account traffic without overwriting an identity which the
    /// request builder has already supplied.
    pub fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut headers = HeaderMap::new();
        self.insert(&mut headers);
        request.headers(headers)
    }

    /// Adds Relay's fallback identity only where the downstream request did
    /// not already provide one. The selected account binding is the one
    /// exception: it is always owned by Relay and cannot be supplied by the
    /// client.
    pub fn insert(&self, headers: &mut HeaderMap) {
        if !headers.contains_key(USER_AGENT) {
            headers.insert(USER_AGENT, self.user_agent.clone());
        }
        let version = HeaderName::from_static("version");
        if !headers.contains_key(&version) {
            headers.insert(version, self.version.clone());
        }
        let originator = HeaderName::from_static("originator");
        if !headers.contains_key(&originator) {
            headers.insert(originator, self.originator.clone());
        }
        // This is an account-binding header, not a client identity header.
        // Never allow a forwarded value to bind a request to another account.
        headers.insert(
            HeaderName::from_static("chatgpt-account-id"),
            self.account_id.clone(),
        );
    }
}

pub fn valid_codex_client_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+' | b'_'))
}

/// Matches the official Rust Codex User-Agent shape while using only local,
/// non-identifying platform and terminal information.
fn codex_user_agent_for_current_host(client_version: &str) -> String {
    let os = os_info::get();
    codex_user_agent(
        client_version,
        &os.os_type().to_string(),
        &os.version().to_string(),
        os.architecture().unwrap_or("unknown"),
        &codex_terminal_token(),
    )
}

fn codex_user_agent(
    client_version: &str,
    os_name: &str,
    os_version: &str,
    architecture: &str,
    terminal: &str,
) -> String {
    format!(
        "{CODEX_ORIGINATOR}/{client_version} ({os_name} {os_version}; {architecture}) {terminal}"
    )
}

fn codex_terminal_token() -> String {
    if let Some(program) = environment_user_agent_token("TERM_PROGRAM") {
        return match environment_user_agent_token("TERM_PROGRAM_VERSION") {
            Some(version) => format!("{program}/{version}"),
            None => program,
        };
    }
    if env::var_os("WT_SESSION").is_some_and(|value| !value.is_empty()) {
        return "WindowsTerminal".to_string();
    }
    environment_user_agent_token("TERM").unwrap_or_else(|| "unknown".to_string())
}

fn environment_user_agent_token(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| sanitize_user_agent_token(&value))
        .filter(|value| !value.is_empty())
}

fn sanitize_user_agent_token(value: &str) -> String {
    value
        .trim()
        .chars()
        .take(128)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '/') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_envelope_keeps_originator_user_agent_and_version_consistent() {
        let identity = CodexIdentityEnvelope::new("account-1", CODEX_CLIENT_VERSION).unwrap();
        let mut headers = HeaderMap::new();
        identity.insert(&mut headers);

        assert_eq!(identity.client_version(), CODEX_CLIENT_VERSION);
        assert_eq!(headers["originator"], CODEX_ORIGINATOR);
        assert_eq!(headers["version"], CODEX_CLIENT_VERSION);
        assert!(headers[USER_AGENT]
            .to_str()
            .unwrap()
            .starts_with(&format!("{CODEX_ORIGINATOR}/{CODEX_CLIENT_VERSION} (")));
        assert_eq!(headers["chatgpt-account-id"], "account-1");
    }

    #[test]
    fn user_agent_uses_the_official_codex_shape() {
        assert_eq!(
            codex_user_agent(
                CODEX_CLIENT_VERSION,
                "Windows",
                "10.0",
                "x86_64",
                "WindowsTerminal"
            ),
            "codex_cli_rs/0.154.0 (Windows 10.0; x86_64) WindowsTerminal"
        );
    }

    #[test]
    fn explicit_client_versions_remain_available_for_downstream_clients() {
        let identity = CodexIdentityEnvelope::new("account-1", CODEX_CLIENT_VERSION).unwrap();
        let client_identity = identity.with_client_version("0.154.0").unwrap();
        assert_eq!(client_identity.client_version(), "0.154.0");
        assert_eq!(identity.client_version(), CODEX_CLIENT_VERSION);
    }

    #[test]
    fn client_version_validation_is_shared_by_gateway_and_identity_headers() {
        assert!(valid_codex_client_version(CODEX_CLIENT_VERSION));
        assert!(valid_codex_client_version("release_test+1"));
        assert!(!valid_codex_client_version(""));
        assert!(!valid_codex_client_version("0.155.0-alpha.3\ninvalid"));
        assert!(!valid_codex_client_version(&"a".repeat(65)));
    }

    #[test]
    fn newer_verified_release_never_downgrades_process_identity() {
        assert!(is_newer_official_release("0.155.0", CODEX_CLIENT_VERSION));
        assert!(!is_newer_official_release(CODEX_CLIENT_VERSION, "0.155.0"));
        assert!(!is_newer_official_release(
            CODEX_CLIENT_VERSION,
            CODEX_CLIENT_VERSION
        ));
        assert!(!is_newer_official_release(
            "0.155.0-alpha.3.7",
            CODEX_CLIENT_VERSION
        ));
        assert!(!is_newer_official_release(
            "not-semver",
            CODEX_CLIENT_VERSION
        ));
    }

    #[test]
    fn user_agent_tokens_are_header_safe_and_bounded() {
        assert_eq!(
            sanitize_user_agent_token(" Windows Terminal 1.2\n"),
            "Windows_Terminal_1.2"
        );
        assert_eq!(sanitize_user_agent_token(&"x".repeat(129)).len(), 128);
    }

    #[test]
    fn insert_preserves_downstream_identity_but_owns_account_binding() {
        let identity =
            CodexIdentityEnvelope::new("selected-account", CODEX_CLIENT_VERSION).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("client/9.9"));
        headers.insert(
            HeaderName::from_static("originator"),
            HeaderValue::from_static("ChatGPT Desktop"),
        );
        headers.insert(
            HeaderName::from_static("version"),
            HeaderValue::from_static("9.9.9"),
        );
        headers.insert(
            HeaderName::from_static("chatgpt-account-id"),
            HeaderValue::from_static("client-selected-account"),
        );

        identity.insert(&mut headers);

        assert_eq!(headers[USER_AGENT], "client/9.9");
        assert_eq!(headers["originator"], "ChatGPT Desktop");
        assert_eq!(headers["version"], "9.9.9");
        assert_eq!(headers["chatgpt-account-id"], "selected-account");
    }

    #[test]
    fn insert_fills_missing_identity_headers_from_relay_fallback() {
        let identity = CodexIdentityEnvelope::new("selected-account", "0.155.0").unwrap();
        let mut headers = HeaderMap::new();

        identity.insert(&mut headers);

        assert!(headers[USER_AGENT]
            .to_str()
            .unwrap()
            .starts_with("codex_cli_rs/0.155.0 ("));
        assert_eq!(headers["originator"], CODEX_ORIGINATOR);
        assert_eq!(headers["version"], "0.155.0");
        assert_eq!(headers["chatgpt-account-id"], "selected-account");
    }
}
