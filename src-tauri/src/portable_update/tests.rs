use super::helper::{replace_executable, rollback_executable, write_acknowledgement};
use super::*;
use std::{
    env,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = env::temp_dir().join(format!("zenith-relay-portable-{label}-{stamp}"));
    fs::create_dir_all(&path).unwrap();
    path
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
