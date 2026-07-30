use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

const MAGIC: &[u8; 4] = b"ZRV1";
const NONCE_BYTES: usize = 12;
const MAX_VAULT_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Default, Deserialize, Serialize)]
struct VaultData {
    values: BTreeMap<String, String>,
}

pub struct Vault {
    path: PathBuf,
    backup_path: PathBuf,
    key: [u8; 32],
    data: Mutex<VaultData>,
}

impl Vault {
    pub fn open(root: &Path, key: [u8; 32]) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(io_error)?;
        let path = root.join("secrets.enc");
        let backup_path = root.join("secrets.enc.bak");
        if !path.exists() && backup_path.exists() {
            fs::rename(&backup_path, &path).map_err(io_error)?;
        }
        let data = if path.exists() {
            decrypt_file(&path, &key)?
        } else {
            VaultData::default()
        };
        Ok(Self {
            path,
            backup_path,
            key,
            data: Mutex::new(data),
        })
    }

    pub fn save(&self, secret_ref: &str, value: &str) -> Result<(), String> {
        validate_ref(secret_ref)?;
        if value.is_empty() || value.len() > 1024 * 1024 {
            return Err("secret value is empty or too large".to_string());
        }
        let mut data = self.lock()?;
        let previous = data
            .values
            .insert(secret_ref.to_string(), value.to_string());
        if let Err(error) = self.persist(&data) {
            match previous {
                Some(value) => {
                    data.values.insert(secret_ref.to_string(), value);
                }
                None => {
                    data.values.remove(secret_ref);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn load(&self, secret_ref: &str) -> Result<Option<String>, String> {
        validate_ref(secret_ref)?;
        Ok(self.lock()?.values.get(secret_ref).cloned())
    }

    pub fn delete(&self, secret_ref: &str) -> Result<bool, String> {
        validate_ref(secret_ref)?;
        let mut data = self.lock()?;
        let Some(previous) = data.values.remove(secret_ref) else {
            return Ok(false);
        };
        if let Err(error) = self.persist(&data) {
            data.values.insert(secret_ref.to_string(), previous);
            return Err(error);
        }
        Ok(true)
    }

    fn persist(&self, data: &VaultData) -> Result<(), String> {
        let plaintext = serde_json::to_vec(data).map_err(|_| "vault serialization failed")?;
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let mut nonce_bytes = [0_u8; NONCE_BYTES];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from(nonce_bytes);
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_ref())
            .map_err(|_| "vault encryption failed")?;
        let mut bytes = Vec::with_capacity(MAGIC.len() + NONCE_BYTES + ciphertext.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&nonce_bytes);
        bytes.extend_from_slice(&ciphertext);
        atomic_replace(&self.path, &self.backup_path, &bytes)
    }

    fn lock(&self) -> Result<MutexGuard<'_, VaultData>, String> {
        self.data
            .lock()
            .map_err(|_| "vault lock poisoned".to_string())
    }
}

fn decrypt_file(path: &Path, key: &[u8; 32]) -> Result<VaultData, String> {
    let metadata = fs::metadata(path).map_err(io_error)?;
    if metadata.len() > MAX_VAULT_BYTES {
        return Err("vault file is too large".to_string());
    }
    let bytes = fs::read(path).map_err(io_error)?;
    if bytes.len() <= MAGIC.len() + NONCE_BYTES || &bytes[..MAGIC.len()] != MAGIC {
        return Err("vault file header is invalid".to_string());
    }
    let nonce = Nonce::try_from(&bytes[MAGIC.len()..MAGIC.len() + NONCE_BYTES])
        .map_err(|_| "vault nonce is invalid".to_string())?;
    let cipher = ChaCha20Poly1305::new(key.into());
    let plaintext = cipher
        .decrypt(&nonce, &bytes[MAGIC.len() + NONCE_BYTES..])
        .map_err(|_| "vault decryption failed")?;
    serde_json::from_slice(&plaintext).map_err(|_| "vault payload is invalid".to_string())
}

fn atomic_replace(path: &Path, backup: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "vault path has no parent".to_string())?;
    let temporary = parent.join(format!(".secrets-{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temporary, bytes).map_err(io_error)?;
    if backup.exists() {
        fs::remove_file(backup).map_err(io_error)?;
    }
    if path.exists() {
        fs::rename(path, backup).map_err(io_error)?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        if backup.exists() {
            let _ = fs::rename(backup, path);
        }
        return Err(io_error(error));
    }
    if backup.exists() {
        fs::remove_file(backup).map_err(io_error)?;
    }
    Ok(())
}

fn validate_ref(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
    {
        Err("secret reference is invalid".to_string())
    } else {
        Ok(())
    }
}

fn io_error(error: std::io::Error) -> String {
    format!("vault I/O failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_encrypts_round_trips_and_rejects_wrong_key() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-vault-{}", uuid::Uuid::new_v4()));
        let vault = Vault::open(&root, [3; 32]).unwrap();
        vault.save("source:test", "synthetic-secret-value").unwrap();
        let bytes = fs::read(root.join("secrets.enc")).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-secret-value"));
        drop(vault);

        let reopened = Vault::open(&root, [3; 32]).unwrap();
        assert_eq!(
            reopened.load("source:test").unwrap().as_deref(),
            Some("synthetic-secret-value")
        );
        assert!(Vault::open(&root, [4; 32]).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
