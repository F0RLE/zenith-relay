use super::*;
use std::{collections::HashMap, path::PathBuf, sync::Mutex};

mod bindings;
mod model_catalog;
mod reasoning;
mod recovery;
mod websocket;

#[derive(Default)]
struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SecretBackend for MemorySecrets {
    fn save(&self, secret_ref: &str, secret_value: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert(secret_ref.into(), secret_value.into());
        Ok(())
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(secret_ref).cloned())
    }

    fn delete(&self, secret_ref: &str) -> Result<()> {
        self.0.lock().unwrap().remove(secret_ref);
        Ok(())
    }
}

#[derive(Default)]
struct FailingDeleteSecrets(MemorySecrets);

#[derive(Default)]
struct SwitchFaultSecrets {
    memory: MemorySecrets,
    fail_projection_save: Mutex<bool>,
    fail_delete_at: Mutex<Option<usize>>,
    external_config: Mutex<Option<PathBuf>>,
}

impl SecretBackend for SwitchFaultSecrets {
    fn save(&self, secret_ref: &str, secret_value: &str) -> Result<()> {
        if secret_ref.starts_with("profile:codex:projection:")
            && std::mem::take(&mut *self.fail_projection_save.lock().unwrap())
        {
            if let Some(path) = self.external_config.lock().unwrap().take() {
                fs::write(path, "model_provider = 'external'\n").map_err(io_error)?;
            }
            return Err(LocalPoolError::new(
                ErrorCode::SecretStoreUnavailable,
                "injected save failure",
            ));
        }
        self.memory.save(secret_ref, secret_value)
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        self.memory.load(secret_ref)
    }

    fn delete(&self, secret_ref: &str) -> Result<()> {
        let mut countdown = self.fail_delete_at.lock().unwrap();
        if let Some(remaining) = countdown.as_mut() {
            *remaining -= 1;
            if *remaining == 0 {
                *countdown = None;
                return Err(LocalPoolError::new(
                    ErrorCode::SecretStoreUnavailable,
                    "injected delete failure",
                ));
            }
        }
        self.memory.delete(secret_ref)
    }
}

impl SecretBackend for FailingDeleteSecrets {
    fn save(&self, secret_ref: &str, secret_value: &str) -> Result<()> {
        self.0.save(secret_ref, secret_value)
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        self.0.load(secret_ref)
    }

    fn delete(&self, _secret_ref: &str) -> Result<()> {
        Err(LocalPoolError::new(
            ErrorCode::SecretStoreUnavailable,
            "injected delete failure",
        ))
    }
}

struct MutatingSecrets {
    values: Mutex<HashMap<String, String>>,
    path: PathBuf,
    content: Vec<u8>,
}

impl MutatingSecrets {
    fn new(path: PathBuf, content: impl Into<Vec<u8>>) -> Self {
        Self {
            values: Mutex::new(HashMap::new()),
            path,
            content: content.into(),
        }
    }
}

impl SecretBackend for MutatingSecrets {
    fn save(&self, secret_ref: &str, secret_value: &str) -> Result<()> {
        self.values
            .lock()
            .unwrap()
            .insert(secret_ref.into(), secret_value.into());
        fs::write(&self.path, &self.content).map_err(io_error)
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        Ok(self.values.lock().unwrap().get(secret_ref).cloned())
    }

    fn delete(&self, secret_ref: &str) -> Result<()> {
        self.values.lock().unwrap().remove(secret_ref);
        Ok(())
    }
}

struct MutatingLoadSecrets {
    values: Mutex<HashMap<String, String>>,
    path: PathBuf,
    content: Vec<u8>,
}

impl MutatingLoadSecrets {
    fn new(path: PathBuf, content: impl Into<Vec<u8>>) -> Self {
        Self {
            values: Mutex::new(HashMap::new()),
            path,
            content: content.into(),
        }
    }
}

impl SecretBackend for MutatingLoadSecrets {
    fn save(&self, secret_ref: &str, secret_value: &str) -> Result<()> {
        self.values
            .lock()
            .unwrap()
            .insert(secret_ref.into(), secret_value.into());
        Ok(())
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        // Attach loads the undo record before the backup file exists. Once that
        // file is on disk, a later secret read is the restore path: rewrite it
        // so the restore sees an external change and leaves the profile alone.
        if self.path.is_file() {
            fs::write(&self.path, &self.content).map_err(io_error)?;
        }
        Ok(self.values.lock().unwrap().get(secret_ref).cloned())
    }

    fn delete(&self, secret_ref: &str) -> Result<()> {
        self.values.lock().unwrap().remove(secret_ref);
        Ok(())
    }
}

fn profile_dirs(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-profile-{name}-{}",
        uuid::Uuid::new_v4()
    ));
    let home = root.join("profile");
    let backups = root.join("backups");
    fs::create_dir_all(&home).unwrap();
    (root, home, backups)
}

fn profile_backup_count(backups: &Path) -> usize {
    fs::read_dir(backups)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|directory_entry| {
            directory_entry
                .path()
                .extension()
                .and_then(|value| value.to_str())
                == Some("json")
        })
        .count()
}

fn write_test_catalog_file(path: &Path, slug: &str) {
    let mut catalog_entry = routed_codex_catalog_entry(None, slug, 2, None);
    catalog_entry["slug"] = Value::String(slug.into());
    catalog_entry["display_name"] = Value::String(slug.into());
    catalog_entry["description"] = Value::String("Native user model".into());
    catalog_entry["comp_hash"] = Value::String("official".into());
    catalog_entry["default_reasoning_level"] = Value::String("medium".into());
    catalog_entry["supported_reasoning_levels"] = json!([
        {"effort": "medium", "description": "Medium"}
    ]);
    fs::write(
        path,
        serde_json::to_string_pretty(&json!({"models": [catalog_entry]})).unwrap(),
    )
    .unwrap();
}
