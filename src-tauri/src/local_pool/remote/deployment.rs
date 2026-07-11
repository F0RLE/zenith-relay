use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::RngCore;
use serde::Serialize;
use std::{fmt, fs, path::Path};
use url::Url;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentPlan {
    pub directory: String,
    pub public_base_url: String,
    pub management_token: String,
    pub vault_key: String,
    pub compose_command: String,
}

impl fmt::Debug for DeploymentPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeploymentPlan")
            .field("directory", &self.directory)
            .field("public_base_url", &self.public_base_url)
            .field("management_token", &"[redacted]")
            .field("vault_key", &"[redacted]")
            .field("compose_command", &self.compose_command)
            .finish()
    }
}

pub fn prepare(root: &Path, public_base_url: &str) -> Result<DeploymentPlan> {
    let public_base_url = validate_public_base_url(public_base_url)?;
    let deployment_id = uuid::Uuid::new_v4().simple().to_string();
    let directory = root.join("deployments").join(&deployment_id);
    fs::create_dir_all(&directory).map_err(io_error)?;
    let management_token = random_urlsafe(32);
    let mut vault_key = [0_u8; 32];
    rand::rng().fill_bytes(&mut vault_key);
    let vault_key = STANDARD.encode(vault_key);
    let compose = format!("services:\n  relay:\n    image: ghcr.io/f0rle/zenith-relay-server:latest\n    restart: unless-stopped\n    environment:\n      ZENITH_RELAY_BIND: 0.0.0.0:14999\n      ZENITH_RELAY_PUBLIC_BASE_URL: {}\n      ZENITH_RELAY_DATA_DIR: /var/lib/zenith-relay\n      ZENITH_RELAY_MANAGEMENT_TOKEN: ${{ZENITH_RELAY_MANAGEMENT_TOKEN:?set in a protected shell or secret manager}}\n      ZENITH_RELAY_VAULT_KEY: ${{ZENITH_RELAY_VAULT_KEY:?set in a protected shell or secret manager}}\n    ports:\n      - \"14999:14999\"\n    volumes:\n      - relay-data:/var/lib/zenith-relay\nvolumes:\n  relay-data:\n", public_base_url);
    let readme = "Upload this directory to your server and configure HTTPS in front of port 14999. Set ZENITH_RELAY_MANAGEMENT_TOKEN and ZENITH_RELAY_VAULT_KEY in a protected shell or secret manager, then run docker compose up -d. The bundle intentionally contains no secrets.\n";
    fs::write(directory.join("compose.yaml"), compose).map_err(io_error)?;
    fs::write(directory.join("README.txt"), readme).map_err(io_error)?;
    Ok(DeploymentPlan {
        directory: directory.display().to_string(),
        public_base_url,
        management_token,
        vault_key,
        compose_command: "docker compose up -d".to_string(),
    })
}

fn validate_public_base_url(value: &str) -> Result<String> {
    let url = Url::parse(value.trim()).map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "remote public URL is invalid")
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "remote deployment URL must be an HTTPS origin",
        ));
    }
    Ok(url.origin().ascii_serialization())
}

fn random_urlsafe(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    rand::rng().fill_bytes(&mut value);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value)
}

fn io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::Io,
        format!("deployment bundle write failed: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_bundle_contains_no_plaintext_secrets() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-deploy-{}", uuid::Uuid::new_v4()));
        let plan = prepare(&root, "https://relay.example.test").unwrap();
        let directory = Path::new(&plan.directory);
        assert!(!directory.join(".env").exists());
        for file in ["compose.yaml", "README.txt"] {
            let content = fs::read_to_string(directory.join(file)).unwrap();
            assert!(!content.contains(&plan.management_token));
            assert!(!content.contains(&plan.vault_key));
        }
        let debug = format!("{plan:?}");
        assert!(!debug.contains(&plan.management_token));
        assert!(!debug.contains(&plan.vault_key));
        fs::remove_dir_all(root).unwrap();
    }
}
