use super::scan::*;
use super::snapshot::*;
use super::*;

mod catalog_transfer;
mod history_sync;
mod snapshots;

fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
    let root = crate::files::test_temp_root(&format!("repair-{name}"));
    let state = root.join("state");
    let backups = root.join("backups");
    let profile = root.join("profile");
    let session = profile.join("sessions/2026/07/11");
    fs::create_dir_all(&session).unwrap();
    fs::create_dir_all(&state).unwrap();
    let rollout = session.join("rollout-test.jsonl");
    fs::write(
        &rollout,
        concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-test\",\"model_provider\":\"openai\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"content\":\"synthetic-private-prompt\"}}\n"
        ),
    )
    .unwrap();
    let database = profile.join("state_5.sqlite");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE threads(id TEXT PRIMARY KEY, model_provider TEXT NOT NULL, rollout_path TEXT NOT NULL);",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO threads(id, model_provider, rollout_path) VALUES ('thread-test', 'openai', ?1)",
            [path_string(&rollout)],
        )
        .unwrap();
    drop(connection);
    (root, state, backups, profile, rollout, database)
}

#[cfg(any(unix, windows))]
#[test]
fn rollout_enumeration_rejects_an_external_session_directory() {
    let root = crate::files::test_temp_root("repair-external-session");
    let profile = root.join("profile");
    let outside = root.join("outside");
    fs::create_dir(&profile).unwrap();
    fs::create_dir(&outside).unwrap();
    let sessions = profile.join("sessions");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &sessions).unwrap();
    #[cfg(windows)]
    junction::create(&outside, &sessions).unwrap();

    let profile = fs::canonicalize(profile).unwrap();
    let mut seen = HashSet::new();
    let mut collection = RolloutCollection::default();
    let mut remaining = MAX_TOTAL_REWRITE_BYTES;
    let result = collect_rollouts(
        &sessions,
        &profile,
        "openai",
        0,
        &mut seen,
        &mut collection,
        &mut remaining,
    );
    assert!(result.is_err());
    assert!(seen.is_empty());
    assert!(collection.history.is_empty());
    assert!(collection.rewrites.is_empty());
    #[cfg(windows)]
    junction::delete(&sessions).unwrap();
    fs::remove_dir_all(root).unwrap();
}

fn rollout_provider_from_file(path: &Path) -> String {
    let mut line = String::new();
    BufReader::new(File::open(path).unwrap())
        .read_line(&mut line)
        .unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    value["payload"]["model_provider"]
        .as_str()
        .unwrap()
        .to_string()
}

fn rollout_providers_from_file(path: &Path) -> Vec<String> {
    BufReader::new(File::open(path).unwrap())
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| rollout_provider(line.as_bytes()).flatten())
        .collect()
}

fn latest_rollout_provider_from_file(path: &Path) -> String {
    rollout_providers_from_file(path)
        .into_iter()
        .last()
        .expect("session metadata provider")
}

fn database_provider(path: &Path) -> String {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT model_provider FROM threads WHERE id='thread-test'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn catalog_thread_provider(path: &Path, thread_id: &str) -> String {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT COALESCE(model_provider, '') \
             FROM local_thread_catalog \
             WHERE host_id = 'local' AND thread_id = ?1",
            [thread_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn catalog_missing_candidate(path: &Path, thread_id: &str) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT missing_candidate \
             FROM local_thread_catalog \
             WHERE host_id = 'local' AND thread_id = ?1",
            [thread_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn catalog_revision(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT catalog_revision FROM local_thread_catalog_metadata WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}
