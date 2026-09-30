use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountIdentity {
    pub stable_index: String,
    pub identity_hash: String,
    pub organization_hash: Option<String>,
}

impl AccountIdentity {
    pub fn from_hashed_parts(
        source_kind: &str,
        base_url_scope: &str,
        identity_hash: &str,
        secret_fingerprint_hash: &str,
        namespace: &str,
        organization_hash: Option<&str>,
    ) -> Result<Self, &'static str> {
        let source_kind = required(source_kind)?;
        let base_url_scope = required(base_url_scope)?;
        let identity_hash = required(identity_hash)?;
        let secret_fingerprint_hash = required(secret_fingerprint_hash)?;
        let namespace = required(namespace)?;
        let organization_hash = organization_hash
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let stable_index = hex::encode(Sha256::digest(
            format!(
                "{}\0{}\0{}\0{}\0{}",
                source_kind.to_ascii_lowercase(),
                base_url_scope.to_ascii_lowercase(),
                identity_hash.to_ascii_lowercase(),
                secret_fingerprint_hash.to_ascii_lowercase(),
                namespace.to_ascii_lowercase(),
            )
            .as_bytes(),
        ));
        Ok(Self {
            stable_index,
            identity_hash: identity_hash.to_ascii_lowercase(),
            organization_hash,
        })
    }
}

fn required(value: &str) -> Result<&str, &'static str> {
    let value = value.trim();
    if value.is_empty() {
        Err("stable identity parts must not be empty")
    } else {
        Ok(value)
    }
}
