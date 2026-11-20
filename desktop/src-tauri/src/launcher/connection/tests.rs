use super::*;
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

const ID: &str = "926fb7ef-9742-4bf3-ad3a-a58dd082c68a";

struct Fixture(PathBuf);
impl Fixture {
    fn new(value: Value) -> Self {
        let root = crate::files::test_temp_root("native-connection");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("_meta.json"),
            serde_json::to_vec(&json!({
                "appliedId": ID, "entries": [{ "id": ID, "name": "Synthetic" }]
            }))
            .unwrap(),
        )
        .unwrap();
        let fixture = Self(root);
        fixture.write(value);
        fixture
    }
    fn path(&self) -> PathBuf {
        self.0.join(format!("{ID}.json"))
    }
    fn recovery(&self) -> PathBuf {
        self.0.join("recovery.json")
    }
    fn write(&self, value: Value) {
        fs::write(self.path(), serde_json::to_vec(&value).unwrap()).unwrap();
    }
    fn config(&self) -> native::Config {
        native::Config::load(&self.0).unwrap().unwrap()
    }
    fn value(&self) -> Value {
        serde_json::from_slice(&fs::read(self.path()).unwrap()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn connect(config: &native::Config, previous: Option<&Record>) -> Record {
    config
        .prepare(
            previous,
            Some("proxy_synthetic".into()),
            Some(native::endpoint("http://proxy.example.test:8080").unwrap()),
        )
        .unwrap()
}

#[test]
fn native_setting_persists_across_reader_recreation_and_preserves_sign_in() {
    let original =
        json!({"deploymentMode":"1p","preferences":{"theme":"dark"},"inferenceProvider":"custom"});
    let fixture = Fixture::new(original.clone());
    let meta = fs::read(fixture.0.join("_meta.json")).unwrap();
    let mut config = fixture.config();
    let mut record = connect(&config, None);
    config.apply(&record, &fixture.recovery()).unwrap();
    record.pending = false;
    drop(config);
    let reopened = fixture.config();
    let setting = inspect(&(None, Some(record)), Some(&reopened));
    assert_eq!(setting.status, ConnectionStatus::Saved);
    let mut value = fixture.value();
    assert_eq!(
        value.as_object_mut().unwrap().remove("egressProxyUrl"),
        Some(json!("http://proxy.example.test:8080/"))
    );
    assert_eq!(value, original);
    assert_eq!(fs::read(fixture.0.join("_meta.json")).unwrap(), meta);
}

#[test]
fn reset_restores_only_owned_fields_and_preserves_later_client_edits() {
    let fixture = Fixture::new(
        json!({"egressProxyUrl":null,"egressProxyPacUrl":"https://proxy.example.test/config.pac","theme":"dark"}),
    );
    let mut config = fixture.config();
    let mut record = connect(&config, None);
    config.apply(&record, &fixture.recovery()).unwrap();
    record.pending = false;
    let mut value = fixture.value();
    value["theme"] = json!("light");
    fixture.write(value);
    let mut reopened = fixture.config();
    let reset = reopened.prepare(Some(&record), None, None).unwrap();
    reopened.apply(&reset, &fixture.recovery()).unwrap();
    assert_eq!(
        fixture.value(),
        json!({"egressProxyUrl":null,"egressProxyPacUrl":"https://proxy.example.test/config.pac","theme":"light"})
    );
}

#[test]
fn external_proxy_edits_including_reverting_to_original_are_preserved() {
    let fixture = Fixture::new(json!({}));
    let mut config = fixture.config();
    let mut record = connect(&config, None);
    config.apply(&record, &fixture.recovery()).unwrap();
    record.pending = false;
    for value in [
        json!({}),
        json!({"egressProxyUrl":"http://external.example.test:8080/"}),
    ] {
        fixture.write(value.clone());
        let config = fixture.config();
        assert!(config.externally_changed(&record));
        assert_eq!(
            inspect(&(None, Some(record.clone())), Some(&config)).status,
            ConnectionStatus::Changed
        );
        assert!(matches!(
            config.prepare(Some(&record), None, None),
            Err(ConnectionError::Changed)
        ));
        assert_eq!(fixture.value(), value);
    }
}

#[test]
fn native_write_checks_both_profile_and_selection_after_journaling() {
    let fixture = Fixture::new(json!({}));
    let mut config = fixture.config();
    let record = connect(&config, None);
    fixture.write(json!({"theme":"new"}));
    assert_eq!(
        config.apply(&record, &fixture.recovery()),
        Err(ConnectionError::Changed)
    );
    let mut config = fixture.config();
    let record = connect(&config, None);
    fs::write(fixture.0.join("_meta.json"), b"{}").unwrap();
    assert_eq!(
        config.apply(&record, &fixture.recovery()),
        Err(ConnectionError::Changed)
    );
    assert_eq!(fixture.value(), json!({"theme":"new"}));
}

#[test]
fn interrupted_native_write_is_visible_and_can_be_retried() {
    let fixture = Fixture::new(json!({}));
    let config = fixture.config();
    let record = connect(&config, None);
    assert_eq!(
        inspect(&(None, Some(record.clone())), Some(&config)).status,
        ConnectionStatus::NotSaved
    );
    let next = connect(&config, Some(&record));
    let mut reopened = fixture.config();
    reopened.apply(&next, &fixture.recovery()).unwrap();
    assert_eq!(
        inspect(&(None, Some(next)), Some(&reopened)).status,
        ConnectionStatus::Saved
    );
}

#[test]
fn failed_native_write_preserves_existing_configuration() {
    let fixture = Fixture::new(json!({"theme":"dark"}));
    let mut config = fixture.config();
    let record = connect(&config, None);
    crate::files::fail_next_atomic_write(fixture.path());
    assert_eq!(
        config.apply(&record, &fixture.recovery()),
        Err(ConnectionError::Unavailable)
    );
    assert_eq!(fixture.value(), json!({"theme":"dark"}));
}

#[test]
fn proxy_credentials_and_ambiguous_endpoints_are_rejected_before_persistence() {
    assert_eq!(
        native::endpoint("http://proxy.example.test:80").unwrap(),
        "http://proxy.example.test/"
    );
    assert_eq!(
        native::endpoint("http://synthetic-user:synthetic-password@proxy.example.test:8080"),
        Err(ConnectionError::AuthUnsupported)
    );
    for url in [
        "socks5://proxy.example.test:1080",
        "http://proxy.example.test:8080/?token=synthetic",
        "http://proxy.example.test;DIRECT:8080",
    ] {
        assert!(native::endpoint(url).is_err());
    }
    let fixture = Fixture::new(
        json!({"egressProxyPacUrl":"https://proxy.example.test/config?token=synthetic"}),
    );
    assert!(matches!(
        fixture.config().prepare(None, None, None),
        Err(ConnectionError::AuthUnsupported)
    ));
}

#[test]
fn other_applications_are_rejected_before_reading_connection_preferences() {
    for application in [Application::Chatgpt, Application::Opencode] {
        assert!(matches!(
            read(application),
            Err(ConnectionError::Unsupported)
        ));
    }
}

#[test]
fn legacy_selection_is_never_reported_as_a_native_connection() {
    let legacy = inspect(&(Some("proxy_synthetic".into()), None), None);
    assert_eq!(legacy.status, ConnectionStatus::Legacy);
    assert!(!legacy.native_supported);
    assert_eq!(
        inspect(&(None, None), None).status,
        ConnectionStatus::Unsupported
    );
}

#[test]
fn full_revision_tracks_native_edits_but_ipc_never_contains_native_data() {
    let fixture = Fixture::new(json!({"inferenceApiKey":"synthetic-secret"}));
    let first = inspect(&(None, None), Some(&fixture.config()));
    fixture.write(json!({"inferenceApiKey":"synthetic-new-secret"}));
    let second = inspect(&(None, None), Some(&fixture.config()));
    assert_ne!(first.revision, second.revision);
    let serialized = serde_json::to_string(&second).unwrap();
    assert!(!serialized.contains("secret"));
    assert!(!serialized.contains("library"));
    assert!(!serialized.contains("egressProxy"));
}

#[test]
fn bootstrap_and_missing_profiles_are_not_treated_as_native_support() {
    let fixture = Fixture::new(json!({"bootstrapUrl":"https://config.example.test/"}));
    assert!(native::Config::load(&fixture.0).unwrap().is_none());
    fs::write(
        fixture.0.join("_meta.json"),
        b"{\"hybridPointer\":\"https://config.example.test/\"}",
    )
    .unwrap();
    assert!(native::Config::load(&fixture.0).unwrap().is_none());
    fs::write(
        fixture.0.join("_meta.json"),
        b"{\"appliedId\":\"../outside\"}",
    )
    .unwrap();
    assert!(matches!(
        native::Config::load(&fixture.0),
        Err(ConnectionError::Unavailable)
    ));
}

#[cfg(windows)]
#[test]
fn native_connection_rejects_directory_junctions() {
    let fixture = Fixture::new(json!({}));
    let link = crate::files::test_temp_root("native-connection-junction");
    // The fixture helper creates the directory; the junction itself needs an absent path.
    fs::remove_dir(&link).unwrap();
    junction::create(&fixture.0, &link).unwrap();
    let result = native::Config::load(&link);
    junction::delete(&link).unwrap();
    assert!(matches!(result, Err(ConnectionError::Unavailable)));
}
