use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

pub fn child(root: &Path, name: &std::ffi::OsStr) -> PathBuf {
    assert!(root.is_absolute());
    let mut components = Path::new(name).components();
    assert!(matches!(components.next(), Some(Component::Normal(_))));
    assert!(components.next().is_none());
    let path = root.join(name);
    assert!(path.starts_with(root));
    let metadata = path.symlink_metadata().unwrap();
    assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        assert_eq!(metadata.file_attributes() & 0x400, 0);
    }
    path
}

pub fn setup_code(root: &Path) -> PathBuf {
    let mut entries = fs::read_dir(root).unwrap();
    let name = entries.next().unwrap().unwrap().file_name();
    assert!(entries.next().is_none(), "expected one setup code");
    let suffix = name.to_str().unwrap().strip_prefix("relay-setup-").unwrap();
    assert!(suffix.len() >= 6 && suffix.bytes().all(|b| b.is_ascii_alphanumeric()));
    child(root, &name)
}

#[allow(dead_code)] // Shared by the TLS fixtures, not every integration binary.
pub fn openssl() -> Command {
    Command::new("openssl")
}

#[test]
fn fixture_children_reject_traversal_and_setup_extras() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("relay-setup-AbCd12"), "synthetic").unwrap();
    assert_eq!(
        setup_code(root.path()),
        root.path().join("relay-setup-AbCd12")
    );
    for name in ["../outside", "nested/file", ".", ".."] {
        assert!(
            std::panic::catch_unwind(|| child(root.path(), std::ffi::OsStr::new(name))).is_err()
        );
    }
    fs::write(root.path().join("unexpected"), "").unwrap();
    assert!(std::panic::catch_unwind(|| setup_code(root.path())).is_err());
}
