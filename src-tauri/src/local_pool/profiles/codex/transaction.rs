use super::{atomic_write, ErrorCode, LocalPoolError, Result};
use std::{fs, path::Path};

pub(super) fn read_optional_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error_at(path, error)),
    }
}

pub(super) fn snapshot_text<'a>(
    snapshot: &'a Option<Vec<u8>>,
    path: &Path,
) -> Result<Option<&'a str>> {
    snapshot
        .as_deref()
        .map(|content| {
            std::str::from_utf8(content).map_err(|error| {
                LocalPoolError::new(
                    ErrorCode::Io,
                    format!("{} is not valid UTF-8: {error}", path.display()),
                )
            })
        })
        .transpose()
}

pub(super) fn replace_if_unchanged(
    path: &Path,
    expected: &Option<Vec<u8>>,
    content: &str,
) -> Result<()> {
    if &read_optional_bytes(path)? != expected {
        return Err(profile_changed_at(path));
    }
    atomic_write(path, content).map_err(io_error_message)
}

pub(super) fn remove_if_unchanged(path: &Path, expected: &Option<Vec<u8>>) -> Result<()> {
    if &read_optional_bytes(path)? != expected {
        return Err(profile_changed_at(path));
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && expected.is_none() => Ok(()),
        Err(error) => Err(io_error_at(path, error)),
    }
}

pub(super) fn rollback_file(
    path: &Path,
    expected_content: &str,
    previous: &Option<Vec<u8>>,
) -> Result<()> {
    let expected = Some(expected_content.as_bytes().to_vec());
    restore_snapshot_if_unchanged(path, &expected, previous)
}

pub(super) fn restore_snapshot_if_unchanged(
    path: &Path,
    expected_current: &Option<Vec<u8>>,
    previous: &Option<Vec<u8>>,
) -> Result<()> {
    match snapshot_text(previous, path)? {
        Some(content) => replace_if_unchanged(path, expected_current, content),
        None => remove_if_unchanged(path, expected_current),
    }
}

pub(super) fn replace_with_snapshot(
    path: &Path,
    expected_current: &Option<Vec<u8>>,
    content: Option<&str>,
) -> Result<()> {
    match content {
        Some(content) => replace_if_unchanged(path, expected_current, content),
        None => remove_if_unchanged(path, expected_current),
    }
}

pub(super) fn merge_rollbacks(first: Result<()>, second: Result<()>) -> Result<()> {
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("{}; {}", first.message, second.message),
        )),
    }
}

pub(super) fn with_rollback(error: LocalPoolError, rollback: Result<()>) -> LocalPoolError {
    match rollback {
        Ok(()) => error,
        Err(rollback_error) => LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "{}; profile rollback failed: {}",
                error.message, rollback_error.message
            ),
        ),
    }
}

pub(super) fn profile_restore_blocked() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::ProfileRestoreBlocked,
        "ChatGPT profile changed after attach; restore was not applied",
    )
}

pub(super) fn profile_changed_at(path: &Path) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::ProfileRestoreBlocked,
        format!(
            "ChatGPT changed {} while Zenith Relay was updating the profile; no replacement was applied",
            path.display()
        ),
    )
}

pub(super) fn io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, error.to_string())
}

pub(super) fn io_error_at(path: &Path, error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::Io,
        format!("failed to access {}: {error}", path.display()),
    )
}

pub(super) fn io_error_message(error: String) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, error)
}
