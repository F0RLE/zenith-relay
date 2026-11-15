use super::*;
use std::fs;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        Self(crate::files::test_temp_root("snapshot-io"))
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn bounded_metadata_accepts_empty_and_exact_limit_files() {
    let fixture = Fixture::new();
    let path = fixture.path("metadata.json");
    fs::write(&path, b"").unwrap();
    assert_eq!(read_bounded(&path, 0).unwrap(), b"");
    fs::write(&path, b"12345678").unwrap();
    assert_eq!(read_bounded(&path, 8).unwrap(), b"12345678");
    assert_eq!(
        read_bounded(&path, 7).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
}

#[test]
fn bounded_metadata_rejects_non_regular_and_missing_files() {
    let fixture = Fixture::new();
    assert_eq!(
        read_bounded(&fixture.0, 8).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        read_bounded(&fixture.path("missing"), 8).unwrap_err().code,
        ErrorCode::Io
    );
}

#[cfg(unix)]
#[test]
fn bounded_metadata_rejects_file_and_directory_symlinks() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let target = fixture.path("target.json");
    fs::write(&target, b"target").unwrap();
    for (destination, name) in [(&target, "file-link"), (&fixture.0, "directory-link")] {
        let link = fixture.path(name);
        symlink(destination, &link).unwrap();
        assert!(read_bounded(&link, 8).is_err());
        fs::remove_file(link).unwrap();
    }
    assert_eq!(fs::read(target).unwrap(), b"target");
}

#[cfg(windows)]
#[test]
fn bounded_metadata_rejects_junctions_without_reading_the_target() {
    let fixture = Fixture::new();
    let target = fixture.path("target.json");
    fs::write(&target, b"unchanged").unwrap();
    let link = fixture.path("junction");
    junction::create(&fixture.0, &link).unwrap();
    assert!(junction::exists(&link).unwrap());
    assert!(read_bounded(&link, 8).is_err());
    junction::delete(&link).unwrap();
    fs::remove_dir(&link).unwrap();
    assert_eq!(fs::read(target).unwrap(), b"unchanged");
}
