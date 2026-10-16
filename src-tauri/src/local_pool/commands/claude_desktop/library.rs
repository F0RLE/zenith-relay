use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

// Desktop's bootstrap ignores appliedId unless it is a lowercase UUID-shaped ID.
const PROFILE_ID: &str = "d8e70c16-32ea-47c8-923a-6c1a7cf195d4";
const MAX_CONFIG_BYTES: u64 = 2 * 1024 * 1024;

mod snapshot;
pub(super) use snapshot::create as create_snapshot;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Recovery {
    library: PathBuf,
    #[serde(default)]
    profile_id: String,
    previous_applied_id: Option<String>,
    profile_hashes: Vec<String>,
    #[serde(default)]
    snapshot_name: Option<String>,
    #[serde(default)]
    created_at_ms: Option<u64>,
    #[serde(default)]
    pending: bool,
}

#[derive(Debug)]
pub(super) struct LibraryStatus {
    pub configured: bool,
    pub model_count: usize,
    pub has_recovery: bool,
    pub snapshot_name: Option<String>,
    pub created_at_ms: Option<u64>,
}

fn invalid() -> LocalPoolError {
    LocalPoolError::new(ErrorCode::ProfileRestoreBlocked,
        "Claude Desktop configuration changed outside Relay or is invalid; keep the recovery record and inspect the configuration before reconnecting")
}

fn io_error(_: impl std::fmt::Display) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::Io,
        "Claude Desktop configuration could not be read or saved",
    )
}

fn check_path(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Err(invalid()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    check_path(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES => Err(invalid()),
        Ok(_) => fs::read(path).map(Some).map_err(io_error),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

fn parse(bytes: &[u8]) -> Result<Value> {
    serde_json::from_slice(bytes).map_err(|_| invalid())
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    check_path(path)?;
    let parent = path.parent().ok_or_else(invalid)?;
    fs::create_dir_all(parent).map_err(io_error)?;
    let temporary = parent.join(format!(".relay-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary).map_err(io_error)?;
        file.write_all(bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        fs::rename(&temporary, path).map_err(io_error)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn serialize(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(value).map_err(io_error)
}

fn metadata(bytes: Option<&[u8]>) -> Result<Value> {
    let value = bytes
        .map(parse)
        .transpose()?
        .unwrap_or_else(|| json!({"entries": []}));
    let entries = value
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    let mut ids = std::collections::HashSet::new();
    for entry in entries {
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        if !safe_id(id) || !ids.insert(id) || entry.get("name").and_then(Value::as_str).is_none() {
            return Err(invalid());
        }
    }
    if let Some(id) = value.get("appliedId") {
        if !id.is_null() && id.as_str().is_none_or(|id| !safe_id(id)) {
            return Err(invalid());
        }
    }
    Ok(value)
}

fn safe_id(id: &str) -> bool {
    id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn recovery(path: &Path, library: &Path) -> Result<Option<Recovery>> {
    let value: Option<Recovery> = read(path)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| invalid()))
        .transpose()?;
    // Ignore recovery records from older Relay versions that used a legacy ID.
    let value = value.filter(|record| record.profile_id == PROFILE_ID);
    if value.as_ref().is_some_and(|record| {
        record.library != library
            || record
                .previous_applied_id
                .as_deref()
                .is_some_and(|id| !safe_id(id) || id == PROFILE_ID)
            || (record.profile_hashes.is_empty() != record.pending)
            || (record.pending && record.snapshot_name.is_none())
            || record
                .snapshot_name
                .as_ref()
                .is_some_and(|name| snapshot::normalize_name(name).is_err())
            || record.profile_hashes.len() > 2
            || record
                .profile_hashes
                .iter()
                .any(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    }) {
        return Err(invalid());
    }
    Ok(value)
}

fn owns_profile(record: &Recovery, bytes: Option<&[u8]>) -> bool {
    bytes.is_none_or(|bytes| record.profile_hashes.contains(&hash(bytes)))
}

fn owns_entry(meta: &Value, profile_id: &str) -> bool {
    meta["entries"].as_array().is_some_and(|entries| {
        entries
            .iter()
            .any(|entry| entry["id"] == profile_id && entry["name"] == "Zenith Relay")
    })
}

// A public ID/display name is not proof of ownership after losing recovery.
// Reclaim only a static profile bound to this Relay's current endpoint and key,
// with no foreign settings. The catalog may have changed since the last attach.
fn matches_gateway(profile: &[u8], config: &Value) -> bool {
    let Ok(mut profile) = parse(profile) else {
        return false;
    };
    if config["inferenceProvider"] != "gateway"
        || config["inferenceCredentialKind"] != "static"
        || config["inferenceGatewayAuthScheme"] != "bearer"
        || config["inferenceGatewayBaseUrl"]
            .as_str()
            .is_none_or(str::is_empty)
        || config["inferenceGatewayApiKey"]
            .as_str()
            .is_none_or(str::is_empty)
        || !profile["inferenceModels"].as_array().is_some_and(|models| {
            models.iter().all(|model| {
                model.as_object().is_some_and(|fields| {
                    fields
                        .keys()
                        .all(|key| matches!(key.as_str(), "name" | "labelOverride" | "maxEffort"))
                        && model["name"].as_str().is_some_and(|name| !name.is_empty())
                        && fields.get("labelOverride").is_none_or(Value::is_string)
                        && fields.get("maxEffort").is_none_or(|effort| {
                            matches!(
                                effort.as_str(),
                                Some("low" | "medium" | "high" | "xhigh" | "max")
                            )
                        })
                })
            })
        })
    {
        return false;
    }
    profile["inferenceModels"] = config["inferenceModels"].clone();
    // Older Relay profiles did not opt into tool search. A missing field is
    // that old default, not permission to replace an explicit foreign setting.
    if config["toolSearchEnabled"] == true && profile.get("toolSearchEnabled").is_none() {
        profile["toolSearchEnabled"] = json!(true);
    }
    profile == *config
}

fn validate_attach(
    meta: &Value,
    profile: Option<&[u8]>,
    record: Option<&Recovery>,
    config: &Value,
) -> Result<()> {
    if meta["entries"].as_array().is_some_and(|entries| {
        entries
            .iter()
            .any(|entry| entry["id"] == PROFILE_ID && entry["name"] != "Zenith Relay")
    }) {
        return Err(invalid());
    }
    if let Some(record) = record {
        if !owns_profile(record, profile) {
            return Err(invalid());
        }
    } else if (profile.is_some() || meta["appliedId"] == PROFILE_ID || owns_entry(meta, PROFILE_ID))
        && (!owns_entry(meta, PROFILE_ID)
            || !profile.is_some_and(|bytes| matches_gateway(bytes, config)))
    {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn check_attach(library: &Path, recovery_path: &Path, config: &Value) -> Result<()> {
    let meta = metadata(read(&library.join("_meta.json"))?.as_deref())?;
    let profile = read(&library.join(format!("{PROFILE_ID}.json")))?;
    let record = recovery(recovery_path, library)?;
    validate_attach(&meta, profile.as_deref(), record.as_ref(), config)
}

pub(super) fn inspect(library: &Path, recovery_path: &Path) -> Result<LibraryStatus> {
    let meta = metadata(read(&library.join("_meta.json"))?.as_deref())?;
    let record = recovery(recovery_path, library)?;
    let profile = read(&library.join(format!("{PROFILE_ID}.json")))?;
    let owned = record
        .as_ref()
        .is_some_and(|record| owns_profile(record, profile.as_deref()));
    let model_count = if owned {
        profile
            .as_deref()
            .map(parse)
            .transpose()?
            .and_then(|profile| profile["inferenceModels"].as_array().map(Vec::len))
            .unwrap_or(0)
    } else {
        0
    };
    Ok(LibraryStatus {
        configured: owned
            && profile.is_some()
            && owns_entry(&meta, PROFILE_ID)
            && meta["appliedId"] == PROFILE_ID,
        model_count,
        has_recovery: record.is_some(),
        snapshot_name: record
            .as_ref()
            .and_then(|record| record.snapshot_name.clone()),
        created_at_ms: record
            .as_ref()
            .and_then(|record| record.created_at_ms)
            .or_else(|| {
                std::fs::metadata(recovery_path)
                    .ok()?
                    .modified()
                    .ok()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            }),
    })
}

pub(super) fn apply(
    library: &Path,
    recovery_path: &Path,
    config: &Value,
    select: bool,
) -> Result<bool> {
    let meta_path = library.join("_meta.json");
    let profile_path = library.join(format!("{PROFILE_ID}.json"));
    let original_meta = read(&meta_path)?;
    let original_profile = read(&profile_path)?;
    let mut meta = metadata(original_meta.as_deref())?;
    let existing = recovery(recovery_path, library)?;
    let created = existing.is_none();
    validate_attach(
        &meta,
        original_profile.as_deref(),
        existing.as_ref(),
        config,
    )?;
    if !select && (meta["appliedId"] != PROFILE_ID || !owns_entry(&meta, PROFILE_ID)) {
        return Ok(false);
    }
    // Cleanup is not legacy adoption: never delete a file based on its name alone.
    let legacy_path = library.join("zenith-relay.json");
    let legacy_profile = if owns_entry(&meta, "zenith-relay") {
        read(&legacy_path)?
    } else {
        None
    };
    let has_legacy = legacy_profile
        .as_deref()
        .is_some_and(|bytes| matches_gateway(bytes, config));
    let bytes = serialize(config)?;
    if !created
        && !has_legacy
        && original_profile.as_deref() == Some(bytes.as_slice())
        && owns_entry(&meta, PROFILE_ID)
        && meta["appliedId"] == PROFILE_ID
    {
        return Ok(false);
    }
    let mut record = existing.unwrap_or_else(|| Recovery {
        library: library.to_path_buf(),
        profile_id: PROFILE_ID.to_owned(),
        previous_applied_id: meta["appliedId"]
            .as_str()
            .filter(|id| *id != PROFILE_ID && !(has_legacy && *id == "zenith-relay"))
            .map(str::to_owned),
        profile_hashes: Vec::new(),
        snapshot_name: None,
        created_at_ms: Some(zenith_relay_core::unix_time_ms()),
        pending: false,
    });
    if select && meta["appliedId"] != PROFILE_ID {
        record.previous_applied_id = meta["appliedId"]
            .as_str()
            .filter(|id| !(has_legacy && *id == "zenith-relay"))
            .map(str::to_owned);
    }
    // Both fingerprints permit recovery after interruption between the two atomic writes.
    record.profile_hashes = original_profile.iter().map(|bytes| hash(bytes)).collect();
    record.profile_hashes.push(hash(&bytes));
    record.pending = false;
    write(recovery_path, &serialize(&record)?)?;
    if read(&meta_path)? != original_meta || read(&profile_path)? != original_profile {
        return Err(invalid());
    }
    write(&profile_path, &bytes)?;
    if !owns_entry(&meta, PROFILE_ID) {
        meta["entries"]
            .as_array_mut()
            .ok_or_else(invalid)?
            .push(json!({"id": PROFILE_ID, "name": "Zenith Relay"}));
    }
    // Remove an orphaned legacy entry left by an older Relay version.
    if has_legacy {
        meta["entries"]
            .as_array_mut()
            .ok_or_else(invalid)?
            .retain(|entry| !(entry["id"] == "zenith-relay" && entry["name"] == "Zenith Relay"));
    }
    if select {
        meta["appliedId"] = json!(PROFILE_ID);
    }
    if read(&meta_path)? != original_meta
        || read(&profile_path)?.as_deref() != Some(bytes.as_slice())
    {
        return Err(invalid());
    }
    write(&meta_path, &serialize(&meta)?)?;
    // Delete the orphaned legacy profile file after meta is safely updated.
    if has_legacy && read(&legacy_path)? == legacy_profile {
        crate::files::remove_regular_file(&legacy_path).map_err(|()| io_error("remove"))?;
    }
    let status = inspect(library, recovery_path)?;
    if !status.configured {
        return Err(invalid());
    }
    Ok(created)
}

fn validate_restore(record: &Recovery, meta: &Value, profile: Option<&[u8]>) -> Result<()> {
    if !owns_profile(record, profile)
        || meta["entries"].as_array().is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry["id"] == PROFILE_ID && entry["name"] != "Zenith Relay")
        })
        || (record.pending
            && (profile.is_some()
                || owns_entry(meta, PROFILE_ID)
                || meta["appliedId"] == PROFILE_ID))
        || (meta["appliedId"] == PROFILE_ID
            && record.previous_applied_id.as_ref().is_some_and(|previous| {
                !meta["entries"].as_array().is_some_and(|entries| {
                    entries
                        .iter()
                        .any(|entry| entry["id"].as_str() == Some(previous))
                })
            }))
    {
        return Err(invalid());
    }
    Ok(())
}

// Validate before shutting down. Pending snapshots never changed the client,
// so consuming them needs no process lifecycle operation.
pub(super) fn check_restore(library: &Path, recovery_path: &Path) -> Result<bool> {
    let Some(record) = recovery(recovery_path, library)? else {
        return Ok(false);
    };
    let meta = metadata(read(&library.join("_meta.json"))?.as_deref())?;
    let profile = read(&library.join(format!("{PROFILE_ID}.json")))?;
    validate_restore(&record, &meta, profile.as_deref())?;
    Ok(!record.pending)
}

pub(super) fn restore(library: &Path, recovery_path: &Path) -> Result<bool> {
    let Some(record) = recovery(recovery_path, library)? else {
        return Ok(false);
    };
    let meta_path = library.join("_meta.json");
    let profile_path = library.join(format!("{PROFILE_ID}.json"));
    let original_meta = read(&meta_path)?;
    let profile = read(&profile_path)?;
    let mut meta = metadata(original_meta.as_deref())?;
    validate_restore(&record, &meta, profile.as_deref())?;
    if record.pending {
        // An explicit snapshot before connection has not changed Claude at all.
        // Consume only the recovery point; preserve the user's current selection.
        crate::files::remove_regular_file(recovery_path).map_err(|()| io_error("remove"))?;
        return Ok(true);
    }
    if meta["appliedId"] == PROFILE_ID {
        if let Some(previous) = &record.previous_applied_id {
            meta["appliedId"] = json!(previous);
        } else {
            meta.as_object_mut()
                .ok_or_else(invalid)?
                .remove("appliedId");
        }
    }
    meta["entries"]
        .as_array_mut()
        .ok_or_else(invalid)?
        .retain(|entry| entry["id"] != PROFILE_ID);
    if read(&meta_path)? != original_meta || read(&profile_path)? != profile {
        return Err(invalid());
    }
    write(&meta_path, &serialize(&meta)?)?;
    crate::files::remove_regular_file(&profile_path).map_err(|()| io_error("remove"))?;
    crate::files::remove_regular_file(recovery_path).map_err(|()| io_error("remove"))?;
    Ok(true)
}

#[cfg(test)]
mod tests;
