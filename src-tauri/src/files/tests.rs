use super::*;
use std::path::PathBuf;

thread_local! {
    static FAIL_NEXT_WRITE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn fail_next_atomic_write(path: PathBuf) {
    FAIL_NEXT_WRITE.with_borrow_mut(|pending| *pending = Some(path));
}

pub(super) fn check_injected_write_failure(path: &Path) -> std::io::Result<()> {
    FAIL_NEXT_WRITE.with_borrow_mut(|pending| {
        if pending.as_deref() == Some(path) {
            *pending = None;
            return Err(std::io::Error::other("injected atomic write failure"));
        }
        Ok(())
    })
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("relay-file-write-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn atomic_write_preserves_unrelated_legacy_temporary_file() {
    let fixture = Fixture::new();
    let path = fixture.0.join("profile.json");
    let unrelated = path.with_extension("tmp");
    fs::write(&unrelated, "unrelated user data").unwrap();
    atomic_write(&path, "new profile").unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "new profile");
    assert_eq!(
        fs::read_to_string(unrelated).unwrap(),
        "unrelated user data"
    );
}

#[test]
fn atomic_write_replaces_existing_files_and_supports_tmp_destination() {
    let fixture = Fixture::new();
    for name in ["profile.json", "profile.tmp", "профиль с пробелом"] {
        let path = fixture.0.join("nested").join(name);
        atomic_write(&path, "first").unwrap();
        atomic_write(&path, "replacement").unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "replacement");
    }
    assert_eq!(fs::read_dir(fixture.0.join("nested")).unwrap().count(), 3);
}

#[test]
fn atomic_write_failure_preserves_destination_and_cleans_owned_staging_file() {
    let fixture = Fixture::new();
    let path = fixture.0.join("destination.json");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("keep"), "unchanged").unwrap();
    assert!(atomic_write(&path, "replacement").is_err());
    assert_eq!(fs::read_to_string(path.join("keep")).unwrap(), "unchanged");
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
}

#[test]
fn atomic_write_write_error_keeps_original_and_removes_owned_staging_file() {
    let fixture = Fixture::new();
    let path = fixture.0.join("profile.json");
    fs::write(&path, "original").unwrap();
    fail_next_atomic_write(path.clone());
    let error = atomic_write(&path, "replacement").unwrap_err();
    assert!(error.contains("injected atomic write failure"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
    atomic_write(&path, "retry").unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "retry");
}

#[test]
fn atomic_write_does_not_follow_legacy_temporary_hard_link() {
    let fixture = Fixture::new();
    let path = fixture.0.join("export.json");
    let unrelated = fixture.0.join("unrelated.txt");
    fs::write(&unrelated, "unchanged").unwrap();
    fs::hard_link(&unrelated, path.with_extension("tmp")).unwrap();
    atomic_write(&path, "synthetic export").unwrap();
    assert_eq!(fs::read_to_string(unrelated).unwrap(), "unchanged");
    assert_eq!(fs::read_to_string(path).unwrap(), "synthetic export");
}

#[test]
fn atomic_write_concurrent_writers_do_not_share_staging_files() {
    let fixture = Fixture::new();
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        for index in 0..8 {
            let root = &fixture.0;
            let barrier = &barrier;
            scope.spawn(move || {
                let path = root.join(format!("same stem.{index}"));
                let content = format!("writer {index}\n").repeat(4096);
                barrier.wait();
                for _ in 0..4 {
                    atomic_write(&path, &content).unwrap();
                    assert_eq!(fs::read_to_string(&path).unwrap(), content);
                }
            });
        }
    });
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 8);
}

#[test]
fn atomic_write_same_destination_contains_one_complete_concurrent_write() {
    let fixture = Fixture::new();
    let path = fixture.0.join("shared.json");
    let barrier = std::sync::Barrier::new(8);
    let contents = (0..8)
        .map(|index| format!("writer {index}\n").repeat(4096))
        .collect::<Vec<_>>();
    std::thread::scope(|scope| {
        for content in &contents {
            let path = &path;
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                atomic_write(path, content).unwrap();
            });
        }
    });
    assert!(contents.contains(&fs::read_to_string(path).unwrap()));
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn atomic_write_does_not_follow_legacy_temporary_symlink() {
    let fixture = Fixture::new();
    let path = fixture.0.join("export.json");
    let unrelated = fixture.0.join("unrelated.txt");
    fs::write(&unrelated, "unchanged").unwrap();
    std::os::unix::fs::symlink(&unrelated, path.with_extension("tmp")).unwrap();
    atomic_write(&path, "synthetic export").unwrap();
    assert_eq!(fs::read_to_string(unrelated).unwrap(), "unchanged");
    assert!(
        fs::symlink_metadata(path.with_extension("tmp"))
            .unwrap()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn atomic_write_keeps_credentials_private_from_creation() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let path = fixture.0.join("export.json");
    atomic_write(&path, "synthetic export").unwrap();
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
