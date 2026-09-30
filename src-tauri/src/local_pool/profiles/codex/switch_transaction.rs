//! In-process undo journal spanning detach, attach and secret cleanup.
//! Only successful Relay writes advance the expected value used by rollback.
use super::*;
use std::{cell::RefCell, collections::BTreeMap};

struct Change<T> {
    before: Option<T>,
    after: Option<T>,
}

type FileChanges = BTreeMap<PathBuf, Change<Vec<u8>>>;

thread_local! {
    static FILE_CHANGES: RefCell<Option<FileChanges>> = const { RefCell::new(None) };
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
    }
}

pub(super) struct JournalSecrets<'a, S> {
    backend: &'a S,
    changes: RefCell<BTreeMap<String, Change<String>>>,
}

impl<S: SecretBackend> JournalSecrets<'_, S> {
    fn update(&self, secret_ref: &str, value: Option<&str>) -> Result<()> {
        let before = self.backend.load(secret_ref)?;
        if self
            .changes
            .borrow()
            .get(secret_ref)
            .is_some_and(|change| change.after != before)
        {
            return Err(profile_restore_blocked());
        }
        match value {
            Some(value) => self.backend.save(secret_ref, value)?,
            None => self.backend.delete(secret_ref)?,
        }
        let after = value.map(str::to_owned);
        self.changes
            .borrow_mut()
            .entry(secret_ref.to_owned())
            .and_modify(|change| change.after = after.clone())
            .or_insert(Change { before, after });
        Ok(())
    }

    fn rollback(&self, files: &FileChanges) -> Result<()> {
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
        // Secrets must be recoverable before restoring backup references.
        for (secret_ref, change) in changes.iter() {
            if change.before == change.after {
                continue;
            }
            match &change.before {
                Some(value) => self.backend.save(secret_ref, value)?,
                None => self.backend.delete(secret_ref)?,
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
    fn save(&self, secret_ref: &str, value: &str) -> Result<()> {
        self.update(secret_ref, Some(value))
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
    for name in [CONFIG_FILE, AUTH_FILE] {
        let path = journal_path(&codex_home.join(name));
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
    let _guard = JournalGuard;
    let secrets = JournalSecrets {
        backend: secrets,
        changes: RefCell::new(BTreeMap::new()),
    };
    let result = operation(&secrets);
    let files = FILE_CHANGES
        .with_borrow_mut(Option::take)
        .unwrap_or_default();
    result.map_err(|error| with_rollback(error, secrets.rollback(&files)))
}
