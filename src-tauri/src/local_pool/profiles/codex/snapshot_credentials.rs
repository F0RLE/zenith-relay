//! Codex's direct keyring contract: `Codex Auth`, `cli|<home SHA-256[0..16]>`.
//! Keep native credentials separate from Relay's encrypted snapshot storage.
//! Verified against openai/codex 4b4a5cf, login/src/auth/storage.rs.
use super::*;

mod encrypted;

pub(super) const NATIVE_AUTH_REF: &str = "profile:codex:snapshot:native-auth";

pub(in crate::local_pool::profiles) fn validate_legacy_snapshot_auth(
    snapshot: &UserProfileSnapshot,
) -> Result<()> {
    if store(snapshot.config.as_deref())? != AuthStore::File {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "This older snapshot did not capture Codex's keyring authorization; create a new snapshot of the intended login",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthStore {
    File,
    Keyring,
    Auto,
}

pub(super) fn store(config: Option<&str>) -> Result<AuthStore> {
    let config = parse_config(config.unwrap_or_default())?;
    let mode = match config.get("cli_auth_credentials_store").map(Item::as_str) {
        None | Some(Some("file")) => AuthStore::File,
        Some(Some("keyring")) => AuthStore::Keyring,
        Some(Some("auto")) => AuthStore::Auto,
        Some(Some("ephemeral")) => {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "Codex keeps this authorization in process memory; sign in with persistent storage before creating a snapshot",
            ));
        }
        Some(_) => {
            return Err(LocalPoolError::invalid_state(
                "Unknown Codex credential store",
            ));
        }
    };
    Ok(mode)
}

pub(super) fn encrypted_backend(config: Option<&str>) -> Result<bool> {
    let config = parse_config(config.unwrap_or_default())?;
    match config
        .get("features")
        .and_then(|features| features.get("secret_auth_storage"))
    {
        Some(value) => value.as_bool().ok_or_else(|| {
            LocalPoolError::invalid_state("Codex secret_auth_storage must be a boolean")
        }),
        None => Ok(cfg!(windows)),
    }
}

pub(crate) fn validate_capture_mode(home: &Path) -> Result<()> {
    let path = home.join(CONFIG_FILE);
    let bytes = read_optional_bytes(&path)?;
    let config = snapshot_text(&bytes, &path)?;
    store(config)?;
    encrypted_backend(config)?;
    Ok(())
}

pub(super) fn pin_backend(config: Option<String>) -> Result<Option<String>> {
    if store(config.as_deref())? == AuthStore::File {
        return Ok(config);
    }
    let encrypted = encrypted_backend(config.as_deref())?;
    let mut document = parse_config(config.as_deref().unwrap_or_default())?;
    if document.get("features").is_none() {
        document["features"] = Item::Table(Table::new());
    }
    let features = document
        .get_mut("features")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| LocalPoolError::invalid_state("Codex features must be a table"))?;
    features.insert("secret_auth_storage", value(encrypted));
    Ok(Some(document.to_string()))
}

pub(super) trait AuthKeyring {
    fn load(&self, home: &Path, encrypted: bool) -> Result<Option<String>>;
    fn replace(&self, home: &Path, encrypted: bool, auth: Option<&str>) -> Result<()>;
}

pub(super) struct NativeAuthKeyring;

pub(super) fn lock(home: &Path) -> Result<fs::File> {
    let path = home.join("auth.lock");
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| io_error_at(&path, error))?;
    file.try_lock().map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Codex authorization is in use; retry after the other operation finishes",
        )
    })?;
    Ok(file)
}

#[cfg(not(test))]
fn entry(home: &Path) -> Result<keyring::Entry> {
    let home = canonical_profile_dir(home)?;
    let hash = key_hash(home.to_string_lossy().as_ref());
    keyring::Entry::new("Codex Auth", &format!("cli|{}", &hash[..16])).map_err(|_| keyring_error())
}

#[cfg(not(test))]
fn keyring_error() -> LocalPoolError {
    // Some keyring errors contain secret bytes. Never forward their Display.
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        "Codex authorization store is unavailable",
    )
}

#[cfg(not(test))]
impl AuthKeyring for NativeAuthKeyring {
    fn load(&self, home: &Path, encrypted: bool) -> Result<Option<String>> {
        if encrypted {
            return encrypted::load(home, &encrypted::NativeMasterKey);
        }
        match entry(home)?.get_password() {
            Ok(auth) => Ok(Some(auth)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(keyring_error()),
        }
    }

    fn replace(&self, home: &Path, encrypted: bool, auth: Option<&str>) -> Result<()> {
        if encrypted {
            return encrypted::replace(home, auth, &encrypted::NativeMasterKey);
        }
        let entry = entry(home)?;
        match auth {
            Some(auth) => entry.set_password(auth).map_err(|_| keyring_error()),
            None => match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err(keyring_error()),
            },
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_AUTH: std::cell::RefCell<std::collections::HashMap<(PathBuf, bool), String>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

#[cfg(test)]
impl AuthKeyring for NativeAuthKeyring {
    fn load(&self, home: &Path, encrypted: bool) -> Result<Option<String>> {
        let home = canonical_profile_dir(home)?;
        Ok(TEST_AUTH.with_borrow(|auth| auth.get(&(home, encrypted)).cloned()))
    }
    fn replace(&self, home: &Path, encrypted: bool, value: Option<&str>) -> Result<()> {
        let home = canonical_profile_dir(home)?;
        TEST_AUTH.with_borrow_mut(|auth| match value {
            Some(value) => {
                auth.insert((home, encrypted), value.to_owned());
            }
            None => {
                auth.remove(&(home, encrypted));
            }
        });
        Ok(())
    }
}

pub(super) fn read_profile_auth(home: &Path) -> Result<Option<Vec<u8>>> {
    let config_path = home.join(CONFIG_FILE);
    let config = read_optional_bytes(&config_path)?;
    let config = snapshot_text(&config, &config_path)?;
    let path = home.join(AUTH_FILE);
    let file = read_optional_bytes(&path)?;
    if store(config)? == AuthStore::File {
        return Ok(file);
    }
    read(
        home,
        config,
        || snapshot_text(&file, &path).map(|text| text.map(str::to_owned)),
        &NativeAuthKeyring,
    )
    .map(|captured| captured.auth.map(String::into_bytes))
}

pub(super) fn update_auth(
    home: &Path,
    expected: &Option<Vec<u8>>,
    updated: Option<&str>,
) -> Result<()> {
    let _auth_lock = lock(home)?;
    let config_path = home.join(CONFIG_FILE);
    let config = read_optional_bytes(&config_path)?;
    let mode = store(snapshot_text(&config, &config_path)?)?;
    let encrypted = encrypted_backend(snapshot_text(&config, &config_path)?)?;
    if read_profile_auth(home)? != *expected {
        return Err(profile_restore_blocked());
    }
    if mode == AuthStore::File {
        return replace_with_snapshot(&home.join(AUTH_FILE), expected, updated);
    }
    let file_path = home.join(AUTH_FILE);
    let fallback = read_optional_bytes(&file_path)?;
    let previous = NativeAuthKeyring.load(home, encrypted)?;
    switch_transaction::replace_native_auth(home, encrypted, &previous, updated)?;
    if let Err(error) = remove_if_unchanged(&file_path, &fallback) {
        let rollback = switch_transaction::replace_native_auth(
            home,
            encrypted,
            &updated.map(str::to_owned),
            previous.as_deref(),
        );
        return Err(with_rollback(error, rollback));
    }
    Ok(())
}

pub(super) struct ReadAuth {
    pub auth: Option<String>,
    pub uses_file: bool,
}

pub(super) fn read(
    home: &Path,
    config: Option<&str>,
    file: impl FnOnce() -> Result<Option<String>>,
    keyring: &impl AuthKeyring,
) -> Result<ReadAuth> {
    let (auth, uses_file) = match store(config)? {
        AuthStore::File => (file()?, true),
        AuthStore::Keyring => (keyring.load(home, encrypted_backend(config)?)?, false),
        // Fail on a locked store instead of capturing an unverified fallback
        // belonging to an older login. A missing entry can safely use the file.
        AuthStore::Auto => match keyring.load(home, encrypted_backend(config)?)? {
            Some(auth) => (Some(auth), false),
            None => (file()?, true),
        },
    };
    if auth.as_ref().is_some_and(|auth| auth.len() > 1024 * 1024) {
        return Err(LocalPoolError::invalid_state(
            "Codex authorization document is too large",
        ));
    }
    projection::merge_auth(None, auth.as_deref())?;
    Ok(ReadAuth { auth, uses_file })
}

/// Let the existing profile transaction journal native credentials together
/// with config, catalog, binding metadata and Relay's protected undo records.
pub(super) struct RestoreSecrets<'a, S, K> {
    pub home: &'a Path,
    pub encrypted: bool,
    pub secrets: &'a S,
    pub keyring: &'a K,
}

impl<S: SecretBackend, K: AuthKeyring> SecretBackend for RestoreSecrets<'_, S, K> {
    fn save(&self, reference: &str, value: &str) -> Result<()> {
        if reference == NATIVE_AUTH_REF {
            self.keyring.replace(self.home, self.encrypted, Some(value))
        } else {
            self.secrets.save(reference, value)
        }
    }
    fn load(&self, reference: &str) -> Result<Option<String>> {
        if reference == NATIVE_AUTH_REF {
            self.keyring.load(self.home, self.encrypted)
        } else {
            self.secrets.load(reference)
        }
    }
    fn delete(&self, reference: &str) -> Result<()> {
        if reference == NATIVE_AUTH_REF {
            self.keyring.replace(self.home, self.encrypted, None)
        } else {
            self.secrets.delete(reference)
        }
    }
}
