use std::path::PathBuf;

#[path = "../../../crates/relay-core/src/test_support/fixture_root.rs"]
mod fixture_root;

pub(crate) fn temp_root(label: &str) -> PathBuf {
    fixture_root::create(label, &uuid::Uuid::new_v4().simple().to_string())
}
