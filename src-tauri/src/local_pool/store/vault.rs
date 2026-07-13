use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use rand::RngCore;
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
const MAX_VALUES: usize = 8_192;

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
        let data = if path.exists() {
            decrypt_file(&path, &key)?
        } else {
            VaultData::default()
        };
        validate_data(&data)?;
        Ok(Self {
            path,
            backup_path,
            key,
            data: Mutex::new(data),
        })
    }

    pub fn save(&self, secret_ref: &str, value: &str) -> Result<(), String> {
        validate_ref(secret_ref)?;
        if value.is_empty() || value.len() > MAX_SECRET_BYTES {
            return Err("secret value is empty or too large".to_string());
        }
        let mut data = self.lock()?;
        if !data.values.contains_key(secret_ref) && data.values.len() >= MAX_VALUES {
            return Err("secret vault entry limit is reached".to_string());
        }
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
        if plaintext.len() as u64 > MAX_VAULT_BYTES {
            return Err("secret vault size limit is reached".to_string());
        }
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let mut nonce_bytes = [0_u8; NONCE_BYTES];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_ref())
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
    let nonce = Nonce::from_slice(&bytes[MAGIC.len()..MAGIC.len() + NONCE_BYTES]);
    let cipher = ChaCha20Poly1305::new(key.into());
    let plaintext = cipher
        .decrypt(nonce, &bytes[MAGIC.len() + NONCE_BYTES..])
        .map_err(|_| "secret vault decryption failed")?;
    serde_json::from_slice(&plaintext).map_err(|_| "secret vault payload is invalid".to_string())
}

fn validate_data(data: &VaultData) -> Result<(), String> {
    if data.values.len() > MAX_VALUES {
        return Err("secret vault entry limit is exceeded".to_string());
    }
    for (secret_ref, value) in &data.values {
        validate_ref(secret_ref)?;
        if value.is_empty() || value.len() > MAX_SECRET_BYTES {
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
    let result = (|| {
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
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
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

fn validate_ref(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
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
