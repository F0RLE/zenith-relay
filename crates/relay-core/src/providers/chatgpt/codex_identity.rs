use reqwest::header::{HeaderMap, HeaderName, HeaderValue, USER_AGENT};

pub const CODEX_CLIENT_VERSION: &str = "0.144.1";
pub const CODEX_ORIGINATOR: &str = "codex_cli_rs";

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
        Self::new(account_id, CODEX_CLIENT_VERSION)
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
        let user_agent = HeaderValue::from_str(&format!(
            "{CODEX_ORIGINATOR}/{client_version} ({}; {})",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
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

    pub fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut headers = HeaderMap::new();
        self.insert(&mut headers);
        request.headers(headers)
    }

    pub fn insert(&self, headers: &mut HeaderMap) {
        headers.insert(USER_AGENT, self.user_agent.clone());
        headers.insert(HeaderName::from_static("version"), self.version.clone());
        headers.insert(
            HeaderName::from_static("originator"),
            self.originator.clone(),
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_envelope_keeps_originator_user_agent_and_version_consistent() {
        let identity = CodexIdentityEnvelope::standard("account-1").unwrap();
        let mut headers = HeaderMap::new();
        identity.insert(&mut headers);

        assert_eq!(identity.client_version(), CODEX_CLIENT_VERSION);
        assert_eq!(headers["originator"], CODEX_ORIGINATOR);
        assert_eq!(headers["version"], CODEX_CLIENT_VERSION);
        assert!(headers[USER_AGENT]
            .to_str()
            .unwrap()
            .starts_with(&format!("{CODEX_ORIGINATOR}/{CODEX_CLIENT_VERSION} ")));
        assert_eq!(headers["chatgpt-account-id"], "account-1");
    }
}
