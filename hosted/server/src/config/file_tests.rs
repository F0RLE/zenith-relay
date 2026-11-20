use super::read_limited;
use std::fs;

#[test]
fn parser_limit_applies_to_direct_callers() {
    let text = " ".repeat(65_536);
    assert!(super::checked_text(&text).is_ok());
    assert_eq!(
        super::checked_text(&(text + " ")),
        Err("configuration exceeds 64 KiB")
    );
    assert!(super::Config::parse(&" ".repeat(65_537)).is_err());
}

#[test]
fn explicit_regular_files_are_bounded() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.toml");
    fs::write(&path, "test").unwrap();
    assert_eq!(
        read_limited(&path, 4, false).as_deref().map(|s| s.as_str()),
        Some("test")
    );
    assert!(read_limited(&path, 3, false).is_none());
    assert!(read_limited(root.path(), 100, false).is_none());
    assert!(read_limited(std::path::Path::new("relative.toml"), 100, false).is_none());
    assert!(read_limited(&root.path().join("nested/../config.toml"), 100, false).is_none());
}

#[cfg(unix)]
#[test]
fn credentials_reject_links_and_public_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("credential");
    fs::write(&path, "synthetic").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(read_limited(&path, 100, true).is_some());
    let link = root.path().join("alias");
    symlink(&path, &link).unwrap();
    assert!(read_limited(&link, 100, false).is_none());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_limited(&path, 100, true).is_none());
}
