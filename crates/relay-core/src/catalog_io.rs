use reqwest::Response;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CatalogIoError {
    TooLarge,
    Io,
    Network,
    InvalidJson,
}

/// Hash and count the canonical JSON bytes without retaining a serialized copy.
pub(crate) fn json_fingerprint<T: Serialize + ?Sized>(
    value: &T,
) -> Result<(String, usize), CatalogIoError> {
    struct HashWriter {
        hash: Sha256,
        bytes: usize,
    }
    impl Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.hash.update(bytes);
            self.bytes += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = HashWriter {
        hash: Sha256::new(),
        bytes: 0,
    };
    {
        let mut writer = BufWriter::new(&mut output);
        serde_json::to_writer(&mut writer, value).map_err(|_| CatalogIoError::InvalidJson)?;
        writer.flush().map_err(|_| CatalogIoError::Io)?;
    }
    Ok((
        format!("sha256:{}", hex::encode(output.hash.finalize())),
        output.bytes,
    ))
}

pub(crate) fn read_json<T: DeserializeOwned>(
    path: &Path,
    max_bytes: usize,
) -> Result<Option<T>, CatalogIoError> {
    read_bytes(path, max_bytes)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| CatalogIoError::InvalidJson))
        .transpose()
}

pub(crate) fn read_bytes(path: &Path, max_bytes: usize) -> Result<Option<Vec<u8>>, CatalogIoError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(CatalogIoError::Io),
    };
    if metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
        return Err(CatalogIoError::TooLarge);
    }
    read_bounded(path, max_bytes).map(Some)
}

pub(crate) fn write_json_if_changed<T: Serialize>(
    path: &Path,
    json_value: &T,
    max_bytes: usize,
) -> Result<bool, CatalogIoError> {
    let serialized_json =
        serde_json::to_vec(json_value).map_err(|_| CatalogIoError::InvalidJson)?;
    if serialized_json.len() > max_bytes {
        return Err(CatalogIoError::TooLarge);
    }
    if existing_bytes_equal(path, &serialized_json, max_bytes) {
        return Ok(false);
    }
    let parent = path.parent().ok_or(CatalogIoError::Io)?;
    fs::create_dir_all(parent).map_err(|_| CatalogIoError::Io)?;
    let temporary = temporary_path(path);
    write_staged_catalog(path, &temporary, &serialized_json).map(|()| true)
}

fn write_staged_catalog(
    path: &Path,
    temporary: &Path,
    serialized_json: &[u8],
) -> Result<(), CatalogIoError> {
    // Cleanup owns the staging path only after exclusive creation succeeds.
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(temporary)
        .map_err(|_| CatalogIoError::Io)?;
    let write_result = (|| {
        file.write_all(serialized_json)
            .map_err(|_| CatalogIoError::Io)?;
        file.sync_all().map_err(|_| CatalogIoError::Io)?;
        drop(file);
        replace_file(temporary, path)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    write_result
}

fn read_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, CatalogIoError> {
    let mut file_bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    let limit = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    File::open(path)
        .map_err(|_| CatalogIoError::Io)?
        .take(limit)
        .read_to_end(&mut file_bytes)
        .map_err(|_| CatalogIoError::Io)?;
    if file_bytes.len() > max_bytes {
        return Err(CatalogIoError::TooLarge);
    }
    Ok(file_bytes)
}

fn existing_bytes_equal(path: &Path, expected: &[u8], max_bytes: usize) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if metadata.len() != expected.len() as u64
        || metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX)
    {
        return false;
    }
    // Comparing a catalog must not retain a second complete file in memory.
    // Stop at the first difference, including growth after the metadata read.
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut remaining = expected;
    loop {
        let Ok(chunk) = reader.fill_buf() else {
            return false;
        };
        if chunk.is_empty() {
            return remaining.is_empty();
        }
        let Some(tail) = remaining.strip_prefix(chunk) else {
            return false;
        };
        remaining = tail;
        let consumed = chunk.len();
        reader.consume(consumed);
    }
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
    let mut response_bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CatalogIoError::Network)?
    {
        if response_bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(CatalogIoError::TooLarge);
        }
        response_bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&response_bytes).map_err(|_| CatalogIoError::InvalidJson)
}

fn temporary_path(path: &Path) -> PathBuf {
    path.with_extension(format!(
        "tmp-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default(),
        NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

fn replace_file(temporary: &Path, target: &Path) -> Result<(), CatalogIoError> {
    // rename replaces an existing file on supported platforms. Never move an
    // existing destination aside after failure: it may be a directory, or a
    // concurrent writer may prevent both publication and restoration.
    fs::rename(temporary, target).map_err(|_| CatalogIoError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            Self(crate::test_support::temp_root("catalog-io"))
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn failed_catalog_publish_never_moves_an_existing_directory() {
        let directory = TestDirectory::new();
        let path = directory.path("catalog.json");
        fs::create_dir(&path).unwrap();
        let sentinel = path.join("sentinel.txt");
        fs::write(&sentinel, b"synthetic-existing-data").unwrap();
        let temporary = directory.path("owned.tmp");

        assert_eq!(
            write_staged_catalog(&path, &temporary, b"new cache"),
            Err(CatalogIoError::Io)
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"synthetic-existing-data");
        assert!(!temporary.exists());
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn failed_exclusive_catalog_open_preserves_foreign_staging_and_existing_cache() {
        let directory = TestDirectory::new();
        let path = directory.path("catalog.json");
        let temporary = directory.path("existing.tmp");
        fs::write(&path, b"original cache").unwrap();
        fs::write(&temporary, b"unrelated staging content").unwrap();
        let result = write_staged_catalog(&path, &temporary, b"new cache");
        assert_eq!(result, Err(CatalogIoError::Io));
        assert_eq!(fs::read(&path).unwrap(), b"original cache");
        assert_eq!(
            fs::read(&temporary).ok().as_deref(),
            Some(b"unrelated staging content".as_slice())
        );
    }

    #[test]
    fn concurrent_cache_and_backup_staging_names_remain_distinct() {
        let directory = TestDirectory::new();
        let path = directory.path("catalog.json");
        let backup = path.with_extension("bak");
        let names = std::thread::scope(|scope| {
            let handles = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        (0..128)
                            .flat_map(|_| [temporary_path(&path), temporary_path(&backup)])
                            .collect::<Vec<_>>()
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .flat_map(|handle| handle.join().unwrap())
                .collect::<std::collections::HashSet<_>>()
        });
        assert_eq!(names.len(), 8 * 128 * 2);
    }

    #[test]
    fn bounded_cache_reads_reject_growth_and_writes_can_repair_oversized_files() {
        let directory = TestDirectory::new();
        let path = directory.path("catalog.json");
        std::fs::write(&path, b"0123456789").unwrap();
        assert_eq!(read_json::<Value>(&path, 4), Err(CatalogIoError::TooLarge));
        assert!(write_json_if_changed(&path, &serde_json::json!({"ok": true}), 64).unwrap());
        assert_eq!(
            read_json::<Value>(&path, 64).unwrap(),
            Some(serde_json::json!({"ok": true}))
        );
    }

    #[test]
    fn streamed_cache_comparison_checks_boundaries_lengths_and_limits() {
        let directory = TestDirectory::new();
        let path = directory.path("catalog.json");
        let expected: Vec<u8> = (0..3 * 64 * 1024 + 17).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &expected).unwrap();
        assert!(existing_bytes_equal(&path, &expected, expected.len()));
        for index in [
            0,
            64 * 1024 - 1,
            64 * 1024,
            2 * 64 * 1024,
            expected.len() - 1,
        ] {
            let mut different = expected.clone();
            different[index] ^= 1;
            assert!(!existing_bytes_equal(&path, &different, expected.len()));
        }
        assert!(!existing_bytes_equal(&path, &expected, expected.len() - 1));
        assert!(!existing_bytes_equal(
            &path,
            &expected[..expected.len() - 1],
            expected.len()
        ));
        let mut longer = expected.clone();
        longer.push(1);
        assert!(!existing_bytes_equal(&path, &longer, longer.len()));
        std::fs::write(&path, []).unwrap();
        assert!(existing_bytes_equal(&path, &[], 0));
    }
}
