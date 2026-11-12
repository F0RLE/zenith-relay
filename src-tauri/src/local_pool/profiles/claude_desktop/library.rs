use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

// Desktop's bootstrap ignores appliedId unless it is a lowercase UUID-shaped ID.
const PROFILE_ID: &str = "d8e70c16-32ea-47c8-923a-6c1a7cf195d4";
const MAX_CONFIG_BYTES: u64 = 2 * 1024 * 1024;

mod payloads;
mod snapshot;
use payloads::{
    attached_metadata, attached_profile, profile_is_current, restored_metadata, serialize,
};
pub(super) use snapshot::create as create_snapshot;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Recovery {
    library: PathBuf,
    profile_id: String,
    previous_applied_id: Option<String>,
    profile_hashes: Vec<String>,
    #[serde(default)]
    snapshot_name: Option<String>,
    #[serde(default)]
    created_at_ms: Option<u64>,
    #[serde(default)]
    pending: bool,
    #[serde(default)]
    preserve_profile_on_restore: bool,
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
    LocalPoolError::new(
        ErrorCode::ProfileRestoreBlocked,
        "Claude Desktop recovery data or profile path cannot be used safely",
    )
}

fn invalid_config() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::InvalidState,
        "Claude Desktop configuration is not a valid profile document",
    )
}

fn changed() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::Conflict,
        "Claude Desktop configuration changed during the operation; retry with its current state",
    )
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
    match crate::files::read_regular_file_bounded(path, MAX_CONFIG_BYTES) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => Err(invalid()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

fn parse(bytes: &[u8]) -> Result<Value> {
    serde_json::from_slice(bytes).map_err(|_| invalid_config())
}

pub(super) fn uses_model_identity(library: &Path) -> Result<bool> {
    Ok(read(&library.join(format!("{PROFILE_ID}.json")))?
        .as_deref()
        .map(parse)
        .transpose()?
        .is_some_and(|profile| {
            profile["inferenceModels"]
                .as_array()
                .is_some_and(|models| models.iter().any(|model| model.get("behavesAs").is_some()))
        }))
}

pub(super) fn model_identity_support(library: &Path, observed: Option<bool>) -> Result<bool> {
    if let Some(supported) = observed {
        return Ok(supported);
    }
    if uses_model_identity(library)? {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "Claude Desktop version could not be verified; the existing model identity configuration was kept",
        ));
    }
    Ok(false)
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

fn metadata(bytes: Option<&[u8]>) -> Result<Value> {
    let value = bytes
        .map(parse)
        .transpose()?
        .unwrap_or_else(|| json!({"entries": []}));
    let entries = value
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(invalid_config)?;
    let mut ids = std::collections::HashSet::new();
    for entry in entries {
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(invalid_config)?;
        if !safe_id(id) || !ids.insert(id) {
            return Err(invalid());
        }
        if entry.get("name").and_then(Value::as_str).is_none() {
            return Err(invalid_config());
        }
    }
    if let Some(id) = value.get("appliedId")
        && !id.is_null()
        && id.as_str().is_none_or(|id| !safe_id(id))
    {
        return Err(invalid());
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
    if value.as_ref().is_some_and(|record| {
        record.profile_id != PROFILE_ID
            || record.library != library
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

fn has_entry(meta: &Value, profile_id: &str) -> bool {
    meta["entries"]
        .as_array()
        .is_some_and(|entries| entries.iter().any(|entry| entry["id"] == profile_id))
}

// A public ID/display name is not proof of ownership after losing recovery.
// Reclaim only the current gateway binding with validated model identities.
// Client preferences are preserved separately from that connection proof.
fn matches_model_identity(model: &Value, config: &Value) -> bool {
    let expected = config["inferenceModels"]
        .as_array()
        .and_then(|models| {
            models
                .iter()
                .find(|expected| expected["name"] == model["name"])
        })
        .and_then(|model| model.get("behavesAs"));
    match (model.get("behavesAs"), expected) {
        (None, None) => true,
        (Some(Value::String(identity)), Some(Value::String(expected))) => {
            !identity.is_empty() && identity == expected
        }
        _ => false,
    }
}

fn matches_gateway(profile: &[u8], config: &Value) -> bool {
    let Ok(profile) = parse(profile) else {
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
                    fields.keys().all(|key| {
                        matches!(
                            key.as_str(),
                            "name" | "labelOverride" | "maxEffort" | "behavesAs"
                        )
                    }) && model["name"].as_str().is_some_and(|name| !name.is_empty())
                        && fields.get("labelOverride").is_none_or(Value::is_string)
                        && fields.get("maxEffort").is_none_or(|effort| {
                            matches!(
                                effort.as_str(),
                                Some("low" | "medium" | "high" | "xhigh" | "max")
                            )
                        })
                        && matches_model_identity(model, config)
                })
            })
        })
    {
        return false;
    }
    [
        "inferenceProvider",
        "inferenceCredentialKind",
        "inferenceGatewayAuthScheme",
        "inferenceGatewayBaseUrl",
        "inferenceGatewayApiKey",
    ]
    .into_iter()
    .all(|field| profile.get(field) == config.get(field))
}

fn validate_attach(
    meta: &Value,
    profile: Option<&[u8]>,
    record: Option<&Recovery>,
    config: &Value,
    reconnect: bool,
) -> Result<()> {
    if let Some(record) = record {
        // A valid recovery record identifies this integration. Users can edit
        // its client settings; only an explicit reconnect may adopt those edits.
        if !reconnect && !owns_profile(record, profile) {
            return Err(invalid());
        }
    } else if (profile.is_some() || meta["appliedId"] == PROFILE_ID || has_entry(meta, PROFILE_ID))
        && (!has_entry(meta, PROFILE_ID)
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
    validate_attach(&meta, profile.as_deref(), record.as_ref(), config, true)?;
    let (bytes, _) = attached_profile(profile.as_deref(), record.as_ref(), config)?;
    if record.is_some() && profile_is_current(&meta, profile.as_deref(), &bytes) {
        return Ok(());
    }
    attached_metadata(meta, true).map(|_| ())
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
            && has_entry(&meta, PROFILE_ID)
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
    let meta = metadata(original_meta.as_deref())?;
    let existing = recovery(recovery_path, library)?;
    let created = existing.is_none();
    if !select
        && (meta["appliedId"] != PROFILE_ID
            || !has_entry(&meta, PROFILE_ID)
            || !existing
                .as_ref()
                .is_some_and(|record| owns_profile(record, original_profile.as_deref())))
    {
        return Ok(false);
    }
    validate_attach(
        &meta,
        original_profile.as_deref(),
        existing.as_ref(),
        config,
        select,
    )?;
    let (bytes, preserve_profile) =
        attached_profile(original_profile.as_deref(), existing.as_ref(), config)?;
    let preserve_profile =
        preserve_profile || (has_entry(&meta, PROFILE_ID) && !owns_entry(&meta, PROFILE_ID));
    if existing.as_ref().is_some_and(|record| {
        owns_profile(record, original_profile.as_deref())
            && record.preserve_profile_on_restore == preserve_profile
    }) && profile_is_current(&meta, original_profile.as_deref(), &bytes)
    {
        return Ok(false);
    }
    let mut record = existing.unwrap_or_else(|| Recovery {
        library: library.to_path_buf(),
        profile_id: PROFILE_ID.to_owned(),
        previous_applied_id: meta["appliedId"]
            .as_str()
            .filter(|id| *id != PROFILE_ID)
            .map(str::to_owned),
        profile_hashes: Vec::new(),
        snapshot_name: None,
        created_at_ms: Some(zenith_relay_core::unix_time_ms()),
        pending: false,
        preserve_profile_on_restore: false,
    });
    record.preserve_profile_on_restore = preserve_profile;
    if select && meta["appliedId"] != PROFILE_ID && record.snapshot_name.is_none() {
        record.previous_applied_id = meta["appliedId"].as_str().map(str::to_owned);
    }
    // Both fingerprints permit recovery after interruption between the two atomic writes.
    record.profile_hashes = original_profile.iter().map(|bytes| hash(bytes)).collect();
    record.profile_hashes.push(hash(&bytes));
    record.pending = false;
    let metadata_bytes = attached_metadata(meta, select)?;
    write(recovery_path, &serialize(&record)?)?;
    if read(&meta_path)? != original_meta || read(&profile_path)? != original_profile {
        return Err(changed());
    }
    write(&profile_path, &bytes)?;
    if read(&meta_path)? != original_meta
        || read(&profile_path)?.as_deref() != Some(bytes.as_slice())
    {
        return Err(changed());
    }
    write(&meta_path, &metadata_bytes)?;
    let status = inspect(library, recovery_path)?;
    if !status.configured {
        return Err(invalid());
    }
    Ok(created)
}

fn preserve_restored_profile(record: &Recovery, meta: &Value, profile: Option<&[u8]>) -> bool {
    record.preserve_profile_on_restore
        || !owns_profile(record, profile)
        || (has_entry(meta, PROFILE_ID) && !owns_entry(meta, PROFILE_ID))
}

// Validate before shutting down. Pending snapshots never changed the client,
// so consuming them needs no process lifecycle operation.
pub(super) fn check_restore(library: &Path, recovery_path: &Path) -> Result<bool> {
    let Some(record) = recovery(recovery_path, library)? else {
        return Ok(false);
    };
    if record.pending {
        return Ok(false);
    }
    let meta = metadata(read(&library.join("_meta.json"))?.as_deref())?;
    let profile = read(&library.join(format!("{PROFILE_ID}.json")))?;
    let preserve = preserve_restored_profile(&record, &meta, profile.as_deref());
    restored_metadata(meta, &record, preserve)?;
    Ok(true)
}

pub(super) fn restore(library: &Path, recovery_path: &Path) -> Result<bool> {
    let Some(record) = recovery(recovery_path, library)? else {
        return Ok(false);
    };
    if record.pending {
        // No client state was written by this snapshot, even if the user has
        // since edited or selected a profile outside Relay.
        crate::files::remove_regular_file(recovery_path).map_err(|()| io_error("remove"))?;
        return Ok(true);
    }
    let meta_path = library.join("_meta.json");
    let profile_path = library.join(format!("{PROFILE_ID}.json"));
    let original_meta = read(&meta_path)?;
    let profile = read(&profile_path)?;
    let meta = metadata(original_meta.as_deref())?;
    let preserve = preserve_restored_profile(&record, &meta, profile.as_deref());
    let metadata_bytes = restored_metadata(meta, &record, preserve)?;
    if read(&meta_path)? != original_meta || read(&profile_path)? != profile {
        return Err(changed());
    }
    write(&meta_path, &metadata_bytes)?;
    if !preserve {
        if read(&profile_path)? != profile {
            return Err(changed());
        }
        crate::files::remove_regular_file(&profile_path).map_err(|()| io_error("remove"))?;
    }
    crate::files::remove_regular_file(recovery_path).map_err(|()| io_error("remove"))?;
    Ok(true)
}

#[cfg(test)]
mod tests;
