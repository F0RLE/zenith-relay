use base64::{engine::general_purpose::STANDARD, Engine};
use std::{env, fmt, net::SocketAddr, path::PathBuf};
use url::Url;

const MIN_MANAGEMENT_TOKEN_BYTES: usize = 24;

#[derive(Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub public_base_url: Url,
    pub management_token: String,
    pub vault_key: [u8; 32],
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let bind = env::var("ZENITH_RELAY_BIND")
            .unwrap_or_else(|_| "127.0.0.1:14999".to_string())
            .parse::<SocketAddr>()
            .map_err(|_| "ZENITH_RELAY_BIND must be a socket address".to_string())?;
        let data_dir = env::var_os("ZENITH_RELAY_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./data"));
        let public_base_url =
            env::var("ZENITH_RELAY_PUBLIC_BASE_URL").unwrap_or_else(|_| format!("http://{bind}"));
        let public_base_url = validate_public_base_url(&public_base_url)?;
        let management_token = env::var("ZENITH_RELAY_MANAGEMENT_TOKEN")
            .map_err(|_| "ZENITH_RELAY_MANAGEMENT_TOKEN is required".to_string())?;
        validate_management_token(&management_token)?;
        let vault_key = env::var("ZENITH_RELAY_VAULT_KEY")
            .map_err(|_| "ZENITH_RELAY_VAULT_KEY is required".to_string())?;
        let vault_key = decode_vault_key(&vault_key)?;
        Ok(Self {
            bind,
            data_dir,
            public_base_url,
            management_token,
            vault_key,
        })
    }

    #[cfg(test)]
    pub fn for_test(data_dir: PathBuf, bind: SocketAddr) -> Self {
        Self {
            bind,
            data_dir,
            public_base_url: Url::parse(&format!("http://{bind}")).unwrap(),
            management_token: "synthetic-management-token-value".to_string(),
            vault_key: [7; 32],
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Config")
            .field("bind", &self.bind)
            .field("data_dir", &self.data_dir)
            .field("public_base_url", &self.public_base_url)
            .field("management_token", &"[redacted]")
            .field("vault_key", &"[redacted]")
            .finish()
    }
}

fn validate_public_base_url(value: &str) -> Result<Url, String> {
    let mut url = Url::parse(value.trim())
        .map_err(|_| "ZENITH_RELAY_PUBLIC_BASE_URL is invalid".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("ZENITH_RELAY_PUBLIC_BASE_URL must use HTTP or HTTPS".to_string());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "ZENITH_RELAY_PUBLIC_BASE_URL must not contain credentials, query, or fragment"
                .to_string(),
        );
    }
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    Ok(url)
}

fn validate_management_token(value: &str) -> Result<(), String> {
    if value.len() < MIN_MANAGEMENT_TOKEN_BYTES || value.bytes().any(|byte| byte.is_ascii_control())
    {
        Err("ZENITH_RELAY_MANAGEMENT_TOKEN must be at least 24 printable bytes".to_string())
    } else {
        Ok(())
    }
}

fn decode_vault_key(value: &str) -> Result<[u8; 32], String> {
    let decoded = STANDARD
        .decode(value.trim())
        .map_err(|_| "ZENITH_RELAY_VAULT_KEY must be base64".to_string())?;
    decoded
        .try_into()
        .map_err(|_| "ZENITH_RELAY_VAULT_KEY must decode to exactly 32 bytes".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_debug_redacts_secrets_and_key_validation_is_strict() {
        let config = Config::for_test(PathBuf::from("data"), "127.0.0.1:14999".parse().unwrap());
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("synthetic-management-token-value"));
        assert!(decode_vault_key("short").is_err());
        assert!(validate_management_token("short").is_err());
    }
}
