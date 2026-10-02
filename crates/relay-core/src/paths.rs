use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Appends a suffix to the full path, including its extension.
/// `file.sqlite` plus `-wal` is `file.sqlite-wal`, not a replaced extension.
pub fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(suffix);
    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_keeps_the_existing_extension() {
        assert_eq!(
            path_with_suffix(Path::new("relay.sqlite"), "-wal"),
            PathBuf::from("relay.sqlite-wal")
        );
    }
}
