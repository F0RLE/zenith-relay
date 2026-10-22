use super::error::{OAuthError, OAuthErrorCode};
use super::{
    OAuthClientKind, BASIS_POINTS_OAUTH_REDIRECT_URI, CALLBACK_PATH, CODEX_OAUTH_CALLBACK_PORTS,
    CODEX_OAUTH_ISSUER, CODEX_OAUTH_ORIGINATOR,
};
use std::collections::BTreeMap;
use url::Url;
use zenith_relay_core::url_has_userinfo;

pub fn validate_authorization_url(
    kind: OAuthClientKind,
    authorization_url: &str,
    redirect_uri: &str,
) -> Result<Url, OAuthError> {
    let invalid = || OAuthError::new(OAuthErrorCode::InvalidConfiguration, false);
    let authorization = Url::parse(authorization_url).map_err(|_| invalid())?;
    let issuer = Url::parse(CODEX_OAUTH_ISSUER).map_err(|_| invalid())?;
    if authorization.origin() != issuer.origin()
        || authorization.path() != kind.authorize_path()
        || url_has_userinfo(&authorization)
        || authorization.fragment().is_some()
    {
        return Err(invalid());
    }
    let redirect = Url::parse(redirect_uri).map_err(|_| invalid())?;
    let valid_redirect = match kind {
        OAuthClientKind::Codex => {
            redirect.scheme() == "http"
                && redirect.host_str() == Some("localhost")
                && redirect
                    .port()
                    .is_some_and(|port| CODEX_OAUTH_CALLBACK_PORTS.contains(&port))
                && redirect.path() == CALLBACK_PATH
                && redirect.query().is_none()
                && redirect.fragment().is_none()
                && !url_has_userinfo(&redirect)
        }
        OAuthClientKind::ExcelBps => redirect_uri == BASIS_POINTS_OAUTH_REDIRECT_URI,
        OAuthClientKind::Siwc => {
            redirect.scheme() == "http"
                && redirect.host_str() == Some("127.0.0.1")
                && redirect.port().is_some_and(|port| port != 0)
                && redirect.path() == CALLBACK_PATH
                && redirect.query().is_none()
                && redirect.fragment().is_none()
                && !url_has_userinfo(&redirect)
        }
    };
    if !valid_redirect {
        return Err(invalid());
    }
    let mut pairs = BTreeMap::new();
    for (key, value) in authorization.query_pairs() {
        if pairs.insert(key.into_owned(), value.into_owned()).is_some() {
            return Err(invalid());
        }
    }
    let mut expected = BTreeMap::from([
        ("response_type", "code"),
        ("client_id", kind.client_id()),
        ("redirect_uri", redirect_uri),
        ("scope", kind.scope()),
        ("code_challenge_method", "S256"),
    ]);
    if kind == OAuthClientKind::Siwc {
        expected.remove("client_id");
        let client_id = pairs.remove("client_id").ok_or_else(invalid)?;
        if client_id == zenith_relay_core::providers::chatgpt::SIWC_BOOTSTRAP_CLIENT_ID {
            if pairs.remove("agent_name_hint").as_deref() != Some("Zenith Relay") {
                return Err(invalid());
            }
        } else if !super::siwc::valid_client_id(&client_id) {
            return Err(invalid());
        }
        let host = pairs.remove("ext_agent_host_id").ok_or_else(invalid)?;
        if host.is_empty() || host.len() > 256 || host.chars().any(char::is_control) {
            return Err(invalid());
        }
        let nonce = pairs.remove("nonce").ok_or_else(invalid)?;
        if !is_urlsafe_nonce(&nonce) {
            return Err(invalid());
        }
        if pairs
            .remove("prompt")
            .is_some_and(|prompt| prompt != "consent")
        {
            return Err(invalid());
        }
        expected.insert("resource", super::siwc::RESOURCE);
    }
    match kind {
        OAuthClientKind::Siwc => {}
        OAuthClientKind::Codex => {
            expected.insert("id_token_add_organizations", "true");
            expected.insert("codex_cli_simplified_flow", "true");
            expected.insert("originator", CODEX_OAUTH_ORIGINATOR);
        }
        OAuthClientKind::ExcelBps => {
            expected.insert("audience", "https://api.openai.com/v1");
            expected.insert("platform", "PC");
        }
    }
    for (key, value) in expected {
        if pairs.remove(key).as_deref() != Some(value) {
            return Err(invalid());
        }
    }
    let challenge = pairs.remove("code_challenge").ok_or_else(invalid)?;
    let state = pairs.remove("state").ok_or_else(invalid)?;
    let nonce = match kind {
        OAuthClientKind::Codex | OAuthClientKind::Siwc => state.as_str(),
        OAuthClientKind::ExcelBps => state
            .strip_prefix("bps.")
            .and_then(|state| state.strip_suffix(".PC"))
            .ok_or_else(invalid)?,
    };
    if !pairs.is_empty()
        || challenge.len() != 43
        || !is_urlsafe_nonce(&challenge)
        || !is_urlsafe_nonce(nonce)
    {
        return Err(invalid());
    }
    Ok(authorization)
}

fn is_urlsafe_nonce(value: &str) -> bool {
    (32..=256).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
