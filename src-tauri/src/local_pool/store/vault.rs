use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    ChaCha20Poly1305, Nonce, Tag,
    aead::{Aead, KeyInit},
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_agent_message_key"
    )]
    agent_message_key: Option<AgentMessageKeyRecord>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AgentMessageKeyRecord {
    version: u8,
    key: String,
}

fn deserialize_agent_message_key<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<AgentMessageKeyRecord>, D::Error> {
    // Only an absent field is a legacy vault. A present null/incomplete record
    // is corruption, never permission to silently replace an existing key.
    AgentMessageKeyRecord::deserialize(deserializer).map(Some)
}

impl AgentMessageKeyRecord {
    fn decode(&self) -> Result<[u8; 32], String> {
        if self.version != 1 {
            return Err("agent-message key record version is unsupported".into());
        }
        URL_SAFE_NO_PAD
            .decode(&self.key)
            .map_err(|_| "agent-message key record is invalid".to_string())?
            .try_into()
            .map_err(|_| "agent-message key record has an invalid length".to_string())
    }
}

pub struct Vault {
    path: PathBuf,
    backup_path: PathBuf,
    key: [u8; 32],
    vault_data: Mutex<VaultData>,
}

impl Vault {
    /// Atomic within this vault's process-local writer. Desktop startup owns a
    /// single configured vault behind its single-instance application boundary.
    pub(in crate::local_pool) fn agent_message_key(&self) -> Result<[u8; 32], String> {
        let mut data = self.lock()?;
        if let Some(record) = data.agent_message_key.as_ref() {
            return record.decode();
        }
        let mut key = [0; 32];
        rand::rng().fill_bytes(&mut key);
        data.agent_message_key = Some(AgentMessageKeyRecord {
            version: 1,
            key: URL_SAFE_NO_PAD.encode(key),
        });
        if let Err(error) = self.persist(&data) {
            data.agent_message_key = None;
            return Err(error);
        }
        Ok(key)
    }

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
        let max_plaintext_bytes =
            MAX_VAULT_BYTES - (MAGIC.len() + NONCE_BYTES + Tag::default().len()) as u64;
        if plaintext_json.len() as u64 > max_plaintext_bytes {
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
    let bytes =
        crate::files::read_regular_file_bounded(path, MAX_VAULT_BYTES).map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidData {
                "secret vault file is unsafe".to_string()
            } else {
                io_error(error)
            }
        })?;
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
    if let Some(record) = vault_data.agent_message_key.as_ref() {
        record.decode()?;
    }
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
    fn oversized_vault_is_rejected_without_replacing_the_file() {
        let root = crate::files::test_temp_root("vault-size");
        let path = root.join("secrets.enc");
        fs::File::create(&path)
            .unwrap()
            .set_len(MAX_VAULT_BYTES + 1)
            .unwrap();
        let error = Vault::open(&root, [3; 32]).err().unwrap();
        assert_eq!(error, "secret vault file is unsafe");
        assert_eq!(fs::metadata(&path).unwrap().len(), MAX_VAULT_BYTES + 1);
        assert!(!root.join("secrets.enc.bak").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vault_rejects_envelope_overflow_without_overwriting_existing_secrets() {
        let root = crate::files::test_temp_root("vault-envelope");
        let vault = Vault::open(&root, [3; 32]).unwrap();
        vault
            .save("source:existing", "synthetic-existing-secret")
            .unwrap();
        let original = fs::read(root.join("secrets.enc")).unwrap();
        let mut candidate = VaultData::default();
        for index in 0..MAX_VAULT_BYTES as usize / MAX_SECRET_BYTES {
            candidate
                .secrets_by_ref
                .insert(format!("source:size-{index}"), "x".repeat(MAX_SECRET_BYTES));
        }
        let encoded_len = serde_json::to_vec(&candidate).unwrap().len();
        let trim = encoded_len - (MAX_VAULT_BYTES as usize - 1);
        candidate
            .secrets_by_ref
            .get_mut("source:size-0")
            .unwrap()
            .truncate(MAX_SECRET_BYTES - trim);
        validate_data(&candidate).unwrap();
        assert_eq!(
            serde_json::to_vec(&candidate).unwrap().len() as u64,
            MAX_VAULT_BYTES - 1
        );

        let result = vault.persist(&candidate);
        let preserved = fs::read(root.join("secrets.enc")).unwrap() == original;
        drop(vault);
        let can_reopen = Vault::open(&root, [3; 32]).is_ok();
        fs::remove_dir_all(root).unwrap();
        assert!(
            result.is_err(),
            "an encrypted file beyond the read limit must never be published"
        );
        assert!(
            preserved,
            "failed writes must preserve the existing encrypted vault"
        );
        assert!(
            can_reopen,
            "a failed save must not make the existing vault unreadable"
        );
    }

    #[test]
    fn agent_message_key_survives_reopen_backup_and_generic_secret_changes() {
        let root = crate::files::test_temp_root("agent-key");
        let vault = Vault::open(&root, [3; 32]).unwrap();
        vault
            .save("source:synthetic", "synthetic-provider-secret")
            .unwrap();
        assert!(vault.lock().unwrap().agent_message_key.is_none());
        let key = vault.agent_message_key().unwrap();
        let bytes = fs::read(root.join("secrets.enc")).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(&URL_SAFE_NO_PAD.encode(key)));
        vault
            .save("account:synthetic", "synthetic-account-secret")
            .unwrap();
        vault.delete("source:synthetic").unwrap();
        assert_eq!(vault.secret_refs().unwrap(), ["account:synthetic"]);
        drop(vault);
        let reopened = Vault::open(&root, [3; 32]).unwrap();
        assert_eq!(reopened.agent_message_key().unwrap(), key);
        reopened.delete("account:synthetic").unwrap();
        drop(reopened);
        fs::rename(root.join("secrets.enc"), root.join("secrets.enc.bak")).unwrap();
        assert_eq!(
            Vault::open(&root, [3; 32])
                .unwrap()
                .agent_message_key()
                .unwrap(),
            key
        );
        let other_root = crate::files::test_temp_root("agent-key-other");
        assert_ne!(
            Vault::open(&other_root, [3; 32])
                .unwrap()
                .agent_message_key()
                .unwrap(),
            key
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(other_root).unwrap();
    }

    #[test]
    fn competing_agent_message_key_initializers_return_one_durable_key() {
        let root = crate::files::test_temp_root("agent-key-race");
        let vault = std::sync::Arc::new(Vault::open(&root, [3; 32]).unwrap());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads = (0..8)
            .map(|_| {
                let vault = vault.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    vault.agent_message_key().unwrap()
                })
            })
            .collect::<Vec<_>>();
        let keys = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        assert!(keys.iter().all(|key| key == &keys[0]));
        drop(vault);
        assert_eq!(
            Vault::open(&root, [3; 32])
                .unwrap()
                .agent_message_key()
                .unwrap(),
            keys[0]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn initialized_agent_message_key_corruption_never_generates_a_replacement() {
        for record in [
            serde_json::Value::Null,
            serde_json::json!({}),
            serde_json::json!({"version":1}),
            serde_json::json!({"version":1,"key":"invalid"}),
            serde_json::json!({"version":2,"key":URL_SAFE_NO_PAD.encode([7;32])}),
        ] {
            let root = crate::files::test_temp_root("relay-agent-key-invalid");
            let payload =
                serde_json::to_vec(&serde_json::json!({"values":{},"agent_message_key":record}))
                    .unwrap();
            let cipher = ChaCha20Poly1305::new((&[3; 32]).into());
            let nonce = Nonce::from([4; NONCE_BYTES]);
            let ciphertext = cipher.encrypt(&nonce, payload.as_ref()).unwrap();
            let mut bytes = MAGIC.to_vec();
            bytes.extend_from_slice(nonce.as_slice());
            bytes.extend_from_slice(&ciphertext);
            fs::write(root.join("secrets.enc"), &bytes).unwrap();
            assert!(Vault::open(&root, [3; 32]).is_err());
            assert_eq!(fs::read(root.join("secrets.enc")).unwrap(), bytes);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn unavailable_agent_message_key_persistence_returns_no_volatile_key() {
        let fixture = crate::files::test_temp_root("agent-key-io");
        let root = fixture.join("vault");
        let vault = Vault::open(&root, [3; 32]).unwrap();
        fs::remove_dir(&root).unwrap();
        fs::write(&root, "synthetic-unavailable-directory").unwrap();
        assert!(vault.agent_message_key().is_err());
        assert!(vault.lock().unwrap().agent_message_key.is_none());
        fs::remove_file(&root).unwrap();
        fs::create_dir(&root).unwrap();
        let key = vault.agent_message_key().unwrap();
        drop(vault);
        assert_eq!(
            Vault::open(&root, [3; 32])
                .unwrap()
                .agent_message_key()
                .unwrap(),
            key
        );
        fs::remove_dir_all(fixture).unwrap();
    }

    #[test]
    fn vault_encrypts_large_values_and_recovers_backup() {
        let root = crate::files::test_temp_root("vault");
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
