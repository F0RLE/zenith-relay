use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

const MAGIC: &[u8; 4] = b"ZDV1";
const NONCE_BYTES: usize = 12;
const MAX_SECRET_BYTES: usize = 4 * 1024 * 1024;
const MAX_VAULT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SECRET_REFS: usize = 8_192;

#[derive(Default, Deserialize, Serialize)]
struct VaultData {
    #[serde(rename = "values")]
    secrets_by_ref: BTreeMap<String, String>,
}

pub struct Vault {
    path: PathBuf,
    backup_path: PathBuf,
    key: [u8; 32],
    vault_data: Mutex<VaultData>,
}

impl Vault {
    pub fn has_persisted_data(root: &Path) -> bool {
        root.join("secrets.enc").exists() || root.join("secrets.enc.bak").exists()
    }

    pub fn open(root: &Path, key: [u8; 32]) -> Result<Self, String> {
        ensure_directory(root)?;
        let path = root.join("secrets.enc");
        let backup_path = root.join("secrets.enc.bak");
        if !path.exists() && backup_path.exists() {
            fs::rename(&backup_path, &path).map_err(io_error)?;
        }
        let vault_data = if path.exists() {
            decrypt_file(&path, &key)?
        } else {
            VaultData::default()
        };
        validate_data(&vault_data)?;
        Ok(Self {
            path,
            backup_path,
            key,
            vault_data: Mutex::new(vault_data),
        })
    }

    pub fn save(&self, secret_ref: &str, secret_value: &str) -> Result<(), String> {
        validate_ref(secret_ref)?;
        if secret_value.is_empty() || secret_value.len() > MAX_SECRET_BYTES {
            return Err("secret value is empty or too large".to_string());
        }
        let mut vault_data = self.lock()?;
        if !vault_data.secrets_by_ref.contains_key(secret_ref)
            && vault_data.secrets_by_ref.len() >= MAX_SECRET_REFS
        {
            return Err("secret vault entry limit is reached".to_string());
        }
        let previous_secret = vault_data
            .secrets_by_ref
            .insert(secret_ref.to_string(), secret_value.to_string());
        if let Err(error) = self.persist(&vault_data) {
            match previous_secret {
                Some(previous_secret) => {
                    vault_data
                        .secrets_by_ref
                        .insert(secret_ref.to_string(), previous_secret);
                }
                None => {
                    vault_data.secrets_by_ref.remove(secret_ref);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn load(&self, secret_ref: &str) -> Result<Option<String>, String> {
        validate_ref(secret_ref)?;
        Ok(self.lock()?.secrets_by_ref.get(secret_ref).cloned())
    }

    pub fn contains(&self, secret_ref: &str) -> Result<bool, String> {
        validate_ref(secret_ref)?;
        Ok(self.lock()?.secrets_by_ref.contains_key(secret_ref))
    }

    pub fn secret_refs(&self) -> Result<Vec<String>, String> {
        Ok(self.lock()?.secrets_by_ref.keys().cloned().collect())
    }

    pub fn delete(&self, secret_ref: &str) -> Result<bool, String> {
        validate_ref(secret_ref)?;
        let mut vault_data = self.lock()?;
        let Some(previous_secret) = vault_data.secrets_by_ref.remove(secret_ref) else {
            return Ok(false);
        };
        if let Err(error) = self.persist(&vault_data) {
            vault_data
                .secrets_by_ref
                .insert(secret_ref.to_string(), previous_secret);
            return Err(error);
        }
        Ok(true)
    }

    fn persist(&self, vault_data: &VaultData) -> Result<(), String> {
        let plaintext_json =
            serde_json::to_vec(vault_data).map_err(|_| "vault serialization failed")?;
        if plaintext_json.len() as u64 > MAX_VAULT_BYTES {
            return Err("secret vault size limit is reached".to_string());
        }
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let mut nonce_bytes = [0_u8; NONCE_BYTES];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from(nonce_bytes);
        let ciphertext = cipher
            .encrypt(&nonce, plaintext_json.as_ref())
            .map_err(|_| "vault encryption failed")?;
        let mut bytes = Vec::with_capacity(MAGIC.len() + NONCE_BYTES + ciphertext.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&nonce_bytes);
        bytes.extend_from_slice(&ciphertext);
        atomic_replace(&self.path, &self.backup_path, &bytes)
    }

    fn lock(&self) -> Result<MutexGuard<'_, VaultData>, String> {
        self.vault_data
            .lock()
            .map_err(|_| "secret vault lock is unavailable".to_string())
    }
}

fn decrypt_file(path: &Path, key: &[u8; 32]) -> Result<VaultData, String> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_VAULT_BYTES
    {
        return Err("secret vault file is unsafe".to_string());
    }
    let bytes = fs::read(path).map_err(io_error)?;
    if bytes.len() <= MAGIC.len() + NONCE_BYTES || &bytes[..MAGIC.len()] != MAGIC {
        return Err("secret vault file header is invalid".to_string());
    }
    let nonce = Nonce::try_from(&bytes[MAGIC.len()..MAGIC.len() + NONCE_BYTES])
        .map_err(|_| "secret vault nonce is invalid".to_string())?;
    let cipher = ChaCha20Poly1305::new(key.into());
    let plaintext = cipher
        .decrypt(&nonce, &bytes[MAGIC.len() + NONCE_BYTES..])
        .map_err(|_| "secret vault decryption failed")?;
    serde_json::from_slice(&plaintext).map_err(|_| "secret vault payload is invalid".to_string())
}

fn validate_data(vault_data: &VaultData) -> Result<(), String> {
    if vault_data.secrets_by_ref.len() > MAX_SECRET_REFS {
        return Err("secret vault entry limit is exceeded".to_string());
    }
    for (secret_ref, secret_value) in &vault_data.secrets_by_ref {
        validate_ref(secret_ref)?;
        if secret_value.is_empty() || secret_value.len() > MAX_SECRET_BYTES {
            return Err("secret vault contains an invalid value".to_string());
        }
    }
    Ok(())
}

fn atomic_replace(path: &Path, backup: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "secret vault path has no parent".to_string())?;
    let temporary = parent.join(format!(".secrets-{}.tmp", uuid::Uuid::new_v4().simple()));
    let replace_result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(io_error)?;
        file.write_all(bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        drop(file);
        if backup.exists() {
            fs::remove_file(backup).map_err(io_error)?;
        }
        if path.exists() {
            fs::rename(path, backup).map_err(io_error)?;
        }
        if let Err(error) = fs::rename(&temporary, path) {
            if backup.exists() {
                let _ = fs::rename(backup, path);
            }
            return Err(io_error(error));
        }
        let _ = fs::remove_file(backup);
        Ok(())
    })();
    if replace_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    replace_result
}

fn ensure_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(io_error)?;
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("secret vault directory is unsafe".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    }
    Ok(())
}

fn validate_ref(secret_ref: &str) -> Result<(), String> {
    if !zenith_relay_core::is_ascii_ref(secret_ref, 128) {
        Err("secret reference is invalid".to_string())
    } else {
        Ok(())
    }
}

fn io_error(error: std::io::Error) -> String {
    format!("secret vault I/O failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_encrypts_large_values_and_recovers_backup() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-vault-{}", uuid::Uuid::new_v4()));
        let value = "synthetic-large-secret".repeat(8_192);
        let vault = Vault::open(&root, [3; 32]).unwrap();
        vault.save("import-session:test", &value).unwrap();
        let bytes = fs::read(root.join("secrets.enc")).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-large-secret"));
        drop(vault);

        let reopened = Vault::open(&root, [3; 32]).unwrap();
        assert_eq!(reopened.secret_refs().unwrap(), ["import-session:test"]);
        assert!(reopened.contains("import-session:test").unwrap());
        assert!(!reopened.contains("import-session:missing").unwrap());
        assert_eq!(
            reopened.load("import-session:test").unwrap().as_deref(),
            Some(value.as_str())
        );
        drop(reopened);
        fs::rename(root.join("secrets.enc"), root.join("secrets.enc.bak")).unwrap();
        assert_eq!(
            Vault::open(&root, [3; 32])
                .unwrap()
                .load("import-session:test")
                .unwrap()
                .as_deref(),
            Some(value.as_str())
        );
        assert!(Vault::open(&root, [4; 32]).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
