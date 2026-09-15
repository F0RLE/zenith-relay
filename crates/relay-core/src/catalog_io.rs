use reqwest::Response;
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CatalogIoError {
    TooLarge,
    Io,
    Network,
    InvalidJson,
}

pub(crate) fn read_json<T: DeserializeOwned>(
    path: &Path,
    max_bytes: usize,
) -> Result<Option<T>, CatalogIoError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(CatalogIoError::Io),
    };
    if metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
        return Err(CatalogIoError::TooLarge);
    }
    let bytes = read_bounded(path, max_bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| CatalogIoError::InvalidJson)
}

pub(crate) fn write_json_if_changed<T: Serialize>(
    path: &Path,
    value: &T,
    max_bytes: usize,
) -> Result<bool, CatalogIoError> {
    let bytes = serde_json::to_vec(value).map_err(|_| CatalogIoError::InvalidJson)?;
    if bytes.len() > max_bytes {
        return Err(CatalogIoError::TooLarge);
    }
    if existing_bytes_equal(path, &bytes, max_bytes) {
        return Ok(false);
    }
    let parent = path.parent().ok_or(CatalogIoError::Io)?;
    fs::create_dir_all(parent).map_err(|_| CatalogIoError::Io)?;
    let temporary = temporary_path(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|_| CatalogIoError::Io)?;
        file.write_all(&bytes).map_err(|_| CatalogIoError::Io)?;
        file.sync_all().map_err(|_| CatalogIoError::Io)?;
        replace_file(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map(|()| true)
}

fn read_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, CatalogIoError> {
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    let limit = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    File::open(path)
        .map_err(|_| CatalogIoError::Io)?
        .take(limit)
        .read_to_end(&mut bytes)
        .map_err(|_| CatalogIoError::Io)?;
    if bytes.len() > max_bytes {
        return Err(CatalogIoError::TooLarge);
    }
    Ok(bytes)
}

fn existing_bytes_equal(path: &Path, expected: &[u8], max_bytes: usize) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
        return false;
    }
    read_bounded(path, max_bytes)
        .ok()
        .is_some_and(|actual| actual == expected)
}

pub(crate) async fn response_json(
    mut response: Response,
    max_bytes: usize,
) -> Result<serde_json::Value, CatalogIoError> {
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(max_bytes).unwrap_or(u64::MAX))
    {
        return Err(CatalogIoError::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CatalogIoError::Network)?
    {
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(CatalogIoError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| CatalogIoError::InvalidJson)
}

pub(crate) fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

fn temporary_path(path: &Path) -> PathBuf {
    path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ))
}

fn replace_file(temporary: &Path, target: &Path) -> Result<(), CatalogIoError> {
    if fs::rename(temporary, target).is_ok() {
        return Ok(());
    }
    if !target.exists() {
        return Err(CatalogIoError::Io);
    }
    let backup = temporary_path(&target.with_extension("bak"));
    fs::rename(target, &backup).map_err(|_| CatalogIoError::Io)?;
    match fs::rename(temporary, target) {
        Ok(()) => {
            let _ = fs::remove_file(backup);
            Ok(())
        }
        Err(_) => {
            let _ = fs::rename(backup, target);
            Err(CatalogIoError::Io)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn test_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "zenith-relay-catalog-io-{}-{}",
            std::process::id(),
            NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn bounded_cache_reads_reject_growth_and_writes_can_repair_oversized_files() {
        let path = test_path();
        std::fs::write(&path, b"0123456789").unwrap();
        assert_eq!(read_json::<Value>(&path, 4), Err(CatalogIoError::TooLarge));
        assert!(write_json_if_changed(&path, &serde_json::json!({"ok": true}), 64).unwrap());
        assert_eq!(
            read_json::<Value>(&path, 64).unwrap(),
            Some(serde_json::json!({"ok": true}))
        );
        let _ = std::fs::remove_file(path);
    }
}
