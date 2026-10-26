use super::*;

pub(super) fn normalize_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "Snapshot name must contain 1 to 80 characters without control characters",
        ));
    }
    Ok(name.to_owned())
}

// Like OpenCode recovery, Claude keeps one original recovery point. It only
// needs the prior selection: foreign profiles and their secrets remain in place.
pub(in super::super) fn create(library: &Path, recovery_path: &Path, name: &str) -> Result<bool> {
    let name = normalize_name(name)?;
    if recovery(recovery_path, library)?.is_some() {
        return Ok(false);
    }
    let meta_path = library.join("_meta.json");
    let original_meta = read(&meta_path)?;
    let meta = metadata(original_meta.as_deref())?;
    if read(&library.join(format!("{PROFILE_ID}.json")))?.is_some()
        || meta["appliedId"] == PROFILE_ID
        || meta["entries"]
            .as_array()
            .is_some_and(|entries| entries.iter().any(|entry| entry["id"] == PROFILE_ID))
    {
        return Err(invalid());
    }
    let record = Recovery {
        library: library.to_owned(),
        profile_id: PROFILE_ID.to_owned(),
        previous_applied_id: meta["appliedId"].as_str().map(str::to_owned),
        profile_hashes: Vec::new(),
        snapshot_name: Some(name),
        created_at_ms: Some(zenith_relay_core::unix_time_ms()),
        pending: true,
    };
    if read(&meta_path)? != original_meta {
        return Err(invalid());
    }
    write(recovery_path, &serialize(&record)?)?;
    Ok(true)
}
