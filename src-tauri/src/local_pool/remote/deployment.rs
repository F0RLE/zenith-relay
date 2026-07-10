use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::RngCore;
use serde::Serialize;
use std::{fs, path::Path};
use url::Url;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentPlan {
    pub directory: String,
    pub public_base_url: String,
    pub management_token: String,
    pub compose_command: String,
}

pub fn prepare(root: &Path, public_base_url: &str) -> Result<DeploymentPlan> {
    let public_base_url = validate_public_base_url(public_base_url)?;
    let deployment_id = uuid::Uuid::new_v4().simple().to_string();
    let directory = root.join("deployments").join(&deployment_id);
    fs::create_dir_all(&directory).map_err(io_error)?;
    let management_token = random_urlsafe(32);
    let mut vault_key = [0_u8; 32];
    rand::rng().fill_bytes(&mut vault_key);
    let environment = format!(
        "ZENITH_RELAY_BIND=0.0.0.0:14999\nZENITH_RELAY_PUBLIC_BASE_URL={}\nZENITH_RELAY_DATA_DIR=/var/lib/zenith-relay\nZENITH_RELAY_MANAGEMENT_TOKEN={}\nZENITH_RELAY_VAULT_KEY={}\n",
        public_base_url,
        management_token,
        STANDARD.encode(vault_key),
    );
    let compose = "services:\n  relay:\n    image: ghcr.io/f0rle/zenith-relay-server:latest\n    restart: unless-stopped\n    env_file: .env\n    ports:\n      - \"14999:14999\"\n    volumes:\n      - relay-data:/var/lib/zenith-relay\nvolumes:\n  relay-data:\n";
    let readme = "Upload this directory to your server, configure HTTPS in front of port 14999, then run: docker compose up -d\nDo not commit or share .env. Connect Zenith Relay using the public URL and the one-time management token shown by the app.\n";
    fs::write(directory.join(".env"), environment).map_err(io_error)?;
    fs::write(directory.join("compose.yaml"), compose).map_err(io_error)?;
    fs::write(directory.join("README.txt"), readme).map_err(io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory.join(".env"), fs::Permissions::from_mode(0o600))
            .map_err(io_error)?;
    }
    Ok(DeploymentPlan {
        directory: directory.display().to_string(),
        public_base_url,
        management_token,
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
    fn deployment_bundle_keeps_vault_key_out_of_returned_state() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-deploy-{}", uuid::Uuid::new_v4()));
        let plan = prepare(&root, "https://relay.example.test").unwrap();
        let environment = fs::read_to_string(Path::new(&plan.directory).join(".env")).unwrap();
        assert!(environment.contains("ZENITH_RELAY_VAULT_KEY="));
        assert!(!format!("{plan:?}").contains("ZENITH_RELAY_VAULT_KEY"));
        fs::remove_dir_all(root).unwrap();
    }
}
