use super::helper::{replace_executable, rollback_executable, write_acknowledgement};
use super::*;

fn temp_dir(label: &str) -> PathBuf {
    crate::files::test_temp_root(&format!("portable-{label}"))
}

#[test]
fn replacement_writes_new_bytes_and_keeps_backup_until_success() {
    let root = temp_dir("replace");
    let source = root.join("update.exe");
    let target = root.join("relay.exe");
    let backup = root.join("relay.exe.bak");
    fs::write(&source, b"new").unwrap();
    fs::write(&target, b"old").unwrap();

    replace_executable(&source, &target, &backup).unwrap();

    assert_eq!(fs::read(&target).unwrap(), b"new");
    assert_eq!(fs::read(&backup).unwrap(), b"old");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn acknowledgement_is_written_for_the_new_process() {
    let root = temp_dir("ack");
    let ack = root.join("ready.ack");

    write_acknowledgement(&ack).unwrap();

    assert_eq!(fs::read(&ack).unwrap(), b"ready");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn rollback_restores_the_previous_executable() {
    let root = temp_dir("rollback");
    let source = root.join("update.exe");
    let target = root.join("relay.exe");
    let backup = root.join("relay.exe.bak");
    fs::write(&source, b"new").unwrap();
    fs::write(&target, b"old").unwrap();
    replace_executable(&source, &target, &backup).unwrap();

    rollback_executable(&target, &backup).unwrap();

    assert_eq!(fs::read(&target).unwrap(), b"old");
    assert!(!backup.exists());
    let _ = fs::remove_dir_all(root);
}
