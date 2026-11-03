//! In-process undo journal spanning detach, attach and secret cleanup.
//! Only successful Relay writes advance the expected value used by rollback.
use super::*;
use snapshot_credentials::{AuthKeyring, NativeAuthKeyring};
use std::{cell::RefCell, collections::BTreeMap};

struct Change<T> {
    before: Option<T>,
    after: Option<T>,
}

type FileChanges = BTreeMap<PathBuf, Change<Vec<u8>>>;
type NativeAuthChanges = BTreeMap<(PathBuf, bool), Change<String>>;

thread_local! {
    static FILE_CHANGES: RefCell<Option<FileChanges>> = const { RefCell::new(None) };
    static NATIVE_AUTH_CHANGES: RefCell<Option<NativeAuthChanges>> = const { RefCell::new(None) };
}

/// The caller holds Codex's auth lock. Journal native credentials alongside
/// files so a later attach/cleanup failure can restore the previous login.
pub(super) fn replace_native_auth(
    home: &Path,
    encrypted: bool,
    expected: &Option<String>,
    updated: Option<&str>,
) -> Result<()> {
    let home = canonical_profile_dir(home)?;
    let key = (home.clone(), encrypted);
    if NativeAuthKeyring.load(&home, encrypted)? != *expected {
        return Err(profile_restore_blocked());
    }
    NATIVE_AUTH_CHANGES.with_borrow(|journal| {
        if journal
            .as_ref()
            .and_then(|journal| journal.get(&key))
            .is_some_and(|change| &change.after != expected)
        {
            return Err(profile_restore_blocked());
        }
        Ok(())
    })?;
    NativeAuthKeyring.replace(&home, encrypted, updated)?;
    NATIVE_AUTH_CHANGES.with_borrow_mut(|journal| {
        if let Some(journal) = journal {
            let after = updated.map(str::to_owned);
            journal
                .entry(key)
                .and_modify(|change| change.after = after.clone())
                .or_insert_with(|| Change {
                    before: expected.clone(),
                    after,
                });
        }
    });
    Ok(())
}

pub(super) fn check_file(path: &Path, expected: &Option<Vec<u8>>) -> Result<()> {
    let key = journal_path(path);
    FILE_CHANGES.with_borrow(|journal| {
        if journal
            .as_ref()
            .and_then(|journal| journal.get(&key))
            .is_some_and(|change| &change.after != expected)
        {
            return Err(profile_changed_at(path));
        }
        Ok(())
    })
}

pub(super) fn record_file(path: &Path, before: &Option<Vec<u8>>, after: Option<Vec<u8>>) {
    let key = journal_path(path);
    FILE_CHANGES.with_borrow_mut(|journal| {
        if let Some(journal) = journal {
            journal
                .entry(key)
                .and_modify(|change| change.after = after.clone())
                .or_insert_with(|| Change {
                    before: before.clone(),
                    after,
                });
        }
    });
}

fn journal_path(path: &Path) -> PathBuf {
    path.parent()
        .and_then(|parent| fs::canonicalize(parent).ok())
        .zip(path.file_name())
        .map_or_else(|| path.to_owned(), |(parent, name)| parent.join(name))
}

struct JournalGuard;

impl Drop for JournalGuard {
    fn drop(&mut self) {
        FILE_CHANGES.with_borrow_mut(|journal| *journal = None);
        NATIVE_AUTH_CHANGES.with_borrow_mut(|journal| *journal = None);
    }
}

pub(super) struct JournalSecrets<'a, S> {
    backend: &'a S,
    changes: RefCell<BTreeMap<String, Change<String>>>,
}

impl<S: SecretBackend> JournalSecrets<'_, S> {
    fn update(&self, secret_ref: &str, secret_value: Option<&str>) -> Result<()> {
        let before = self.backend.load(secret_ref)?;
        if self
            .changes
            .borrow()
            .get(secret_ref)
            .is_some_and(|change| change.after != before)
        {
            return Err(profile_restore_blocked());
        }
        match secret_value {
            Some(secret_value) => self.backend.save(secret_ref, secret_value)?,
            None => self.backend.delete(secret_ref)?,
        }
        let after = secret_value.map(str::to_owned);
        self.changes
            .borrow_mut()
            .entry(secret_ref.to_owned())
            .and_modify(|change| change.after = after.clone())
            .or_insert(Change { before, after });
        Ok(())
    }

    fn rollback(&self, files: &FileChanges, native_auth: &NativeAuthChanges) -> Result<()> {
        let homes: std::collections::BTreeSet<_> =
            native_auth.keys().map(|(home, _)| home).collect();
        let _auth_locks = homes
            .into_iter()
            .map(|home| snapshot_credentials::lock(home))
            .collect::<Result<Vec<_>>>()?;
        // Refuse the complete rollback if a newer external writer owns a file.
        for (path, change) in files {
            if read_optional_bytes(path)? != change.after {
                return Err(profile_changed_at(path));
            }
        }
        let changes = self.changes.borrow();
        for (secret_ref, change) in changes.iter() {
            if self.backend.load(secret_ref)? != change.after {
                return Err(profile_restore_blocked());
            }
        }
        for ((home, encrypted), change) in native_auth {
            if NativeAuthKeyring.load(home, *encrypted)? != change.after {
                return Err(profile_restore_blocked());
            }
        }
        // Secrets must be recoverable before restoring backup references.
        for (secret_ref, change) in changes.iter() {
            if change.before == change.after {
                continue;
            }
            match &change.before {
                Some(previous_secret_value) => {
                    self.backend.save(secret_ref, previous_secret_value)?
                }
                None => self.backend.delete(secret_ref)?,
            }
        }
        for ((home, encrypted), change) in native_auth {
            if change.before != change.after {
                NativeAuthKeyring.replace(home, *encrypted, change.before.as_deref())?;
            }
        }
        for (path, change) in files {
            if change.before != change.after {
                restore_snapshot_if_unchanged(path, &change.after, &change.before)?;
            }
        }
        Ok(())
    }
}

impl<S: SecretBackend> SecretBackend for JournalSecrets<'_, S> {
    fn save(&self, secret_ref: &str, secret_value: &str) -> Result<()> {
        self.update(secret_ref, Some(secret_value))
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        self.backend.load(secret_ref)
    }

    fn delete(&self, secret_ref: &str) -> Result<()> {
        self.update(secret_ref, None)
    }
}

/// The caller holds the profile lock. Watch auth/config even when an early
/// failure means the operation never writes them.
pub(super) fn run<S: SecretBackend, T>(
    codex_home: &Path,
    secrets: &S,
    operation: impl FnOnce(&JournalSecrets<'_, S>) -> Result<T>,
) -> Result<T> {
    if FILE_CHANGES.with_borrow(Option::is_some) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "Nested profile transaction",
        ));
    }
    let mut files = BTreeMap::new();
    fs::create_dir_all(codex_home).map_err(io_error)?;
    for file_name in [CONFIG_FILE, AUTH_FILE] {
        let path = journal_path(&codex_home.join(file_name));
        let before = read_optional_bytes(&path)?;
        files.insert(
            path,
            Change {
                after: before.clone(),
                before,
            },
        );
    }
    FILE_CHANGES.with_borrow_mut(|journal| *journal = Some(files));
    NATIVE_AUTH_CHANGES.with_borrow_mut(|journal| *journal = Some(BTreeMap::new()));
    let _guard = JournalGuard;
    let secrets = JournalSecrets {
        backend: secrets,
        changes: RefCell::new(BTreeMap::new()),
    };
    let operation_result = operation(&secrets);
    let files = FILE_CHANGES
        .with_borrow_mut(Option::take)
        .unwrap_or_default();
    let native_auth = NATIVE_AUTH_CHANGES
        .with_borrow_mut(Option::take)
        .unwrap_or_default();
    operation_result.map_err(|error| with_rollback(error, secrets.rollback(&files, &native_auth)))
}
