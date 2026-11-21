// Shared by Core and Desktop test modules, without a production feature or API.
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub(super) fn create(label: &str, suffix: &str) -> PathBuf {
    assert!(
        !label.is_empty()
            && label.len() <= 64
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    );
    assert!(suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let root = std::env::temp_dir().join(format!("zenith-relay-{label}-{suffix}"));
    reserve(&root).expect("test fixture directory must be exclusively owned");
    root
}

fn reserve(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path)
    }
}

#[test]
fn fixture_roots_are_reserved_and_never_reuse_existing_data() {
    let root = super::temp_root("fixture-ownership");
    let sentinel = root.join("sentinel.txt");
    fs::write(&sentinel, b"synthetic-existing-data").unwrap();
    let error = reserve(&root).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&sentinel).unwrap(), b"synthetic-existing-data");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fixture_roots_reject_unsafe_components_before_touching_the_filesystem() {
    const SUFFIX: &str = "0123456789abcdef0123456789abcdef";
    for label in [
        "",
        ".",
        "..",
        "../escape",
        "nested/child",
        "nested\\child",
        "a b",
        "тест",
    ] {
        assert!(std::panic::catch_unwind(|| create(label, SUFFIX)).is_err());
    }
    let long_label = "x".repeat(65);
    assert!(std::panic::catch_unwind(|| create(&long_label, SUFFIX)).is_err());
    let non_hex = "g".repeat(32);
    for suffix in ["", "../escape", &SUFFIX[..16], &non_hex] {
        assert!(std::panic::catch_unwind(|| create("invalid-component", suffix)).is_err());
    }
}
