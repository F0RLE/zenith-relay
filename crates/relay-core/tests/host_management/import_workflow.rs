use super::*;
use zenith_relay_core::{
    accounts::{ImportAuthMode, ParsedImportItem, parse_import},
    providers::chatgpt::connector::import::*,
};

impl ImportRecord for Record {
    fn configured_models(&self) -> &[String] {
        &self.models
    }
    fn discovered_models(&self) -> Option<&[String]> {
        self.discovered.as_deref()
    }
    fn set_discovered_models(&mut self, models: Option<Vec<String>>) {
        self.discovered = models;
    }
    fn preserve_host_settings(&mut self, previous: &Self) {
        self.settings = previous.settings;
        self.recovery = previous.recovery;
    }
    fn normalize(&mut self) {}
}

struct Host {
    previous: Option<Record>,
    trace: Mutex<Vec<&'static str>>,
    fail_commit: bool,
}
impl ImportHost for Host {
    type Account = Record;
    type Error = &'static str;
    fn error(_: &'static str, message: &'static str) -> Self::Error {
        message
    }
    fn now_ms(&self) -> u64 {
        10_000
    }
    async fn resolve(
        &self,
        _: ParsedImportItem,
        _: u64,
    ) -> Result<ResolvedImport<Record>, Self::Error> {
        self.trace.lock().unwrap().push("resolve");
        Ok(ResolvedImport {
            credentials: credentials(),
            old_credential: None,
            existing_account: self.previous.clone(),
            preserved_refresh_token: false,
            subscription_active_until_ms: None,
            proxy: None,
            provider_account_id: "synthetic-provider".into(),
            identity_is_registered: true,
        })
    }
    async fn discover(
        &self,
        _: &ResolvedImport<Record>,
        _: u64,
    ) -> Result<Vec<String>, Self::Error> {
        self.trace.lock().unwrap().push("discover");
        Ok(vec!["synthetic-model".into()])
    }
    fn new_record(
        &self,
        _: &StoredCodexCredentials,
        auth: AccountAuthMode,
        models: Vec<String>,
        _: i32,
        _: u64,
    ) -> Result<Record, Self::Error> {
        let mut value = record("headless");
        value.record.auth_mode = auth;
        value.record.in_pool = false;
        value.models = models;
        Ok(value)
    }
    async fn probe_quota(
        &self,
        _: &mut Record,
        _: &ResolvedImport<Record>,
    ) -> observations::QuotaReadOutcome {
        self.trace.lock().unwrap().push("quota");
        observations::QuotaReadOutcome::Skipped
    }
    async fn commit(&self, _: &Record, _: &ResolvedImport<Record>) -> Result<(), Self::Error> {
        self.trace.lock().unwrap().push("commit");
        if self.fail_commit {
            Err("commit failed")
        } else {
            Ok(())
        }
    }
}

fn item() -> ParsedImportItem {
    parse_import(r#"{"access_token":"synthetic-access","account_id":"synthetic-provider","name":"Imported"}"#, None, &[]).unwrap().items.remove(0)
}
fn options() -> AccountImportOptions<'static> {
    AccountImportOptions {
        add_to_pool: true,
        discover_models: true,
        probe_quota: true,
        configured_models: &[],
    }
}

#[tokio::test]
async fn import_preserves_existing_metadata_and_commits_after_both_probes() {
    let mut previous = record("headless");
    previous.record.label = "Operator label".into();
    previous.record.enabled = false;
    previous.record.tags.insert("operator-tag".into());
    previous.models.push("configured-model".into());
    let host = Host {
        previous: Some(previous.clone()),
        trace: Mutex::default(),
        fail_commit: false,
    };
    let (account, _) = import_account(&host, item(), ImportAuthMode::OAuth, options())
        .await
        .unwrap();
    assert_eq!(account.record.label, previous.record.label);
    assert_eq!(account.record.tags, previous.record.tags);
    assert!(!account.record.enabled);
    assert_eq!(account.models, previous.models);
    assert_eq!(account.discovered, Some(vec!["synthetic-model".into()]));
    assert_eq!(
        *host.trace.lock().unwrap(),
        ["resolve", "discover", "quota", "commit"]
    );
}

#[tokio::test]
async fn login_import_does_not_grant_native_images_consent() {
    let host = Host {
        previous: None,
        trace: Mutex::default(),
        fail_commit: false,
    };
    let (account, _) = import_account(&host, item(), ImportAuthMode::OAuth, options())
        .await
        .unwrap();
    assert!(!account.record.in_pool && !account.images);
}

#[tokio::test]
async fn durable_commit_failure_does_not_return_success() {
    let host = Host {
        previous: None,
        trace: Mutex::default(),
        fail_commit: true,
    };
    assert_eq!(
        import_account(&host, item(), ImportAuthMode::OAuth, options())
            .await
            .err(),
        Some("commit failed")
    );
}
