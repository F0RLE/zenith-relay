use super::*;

pub(super) fn serialize(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec_pretty(value).map_err(io_error)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(invalid());
    }
    Ok(bytes)
}

pub(super) fn profile_is_current(meta: &Value, profile: Option<&[u8]>, bytes: &[u8]) -> bool {
    profile == Some(bytes) && owns_entry(meta, PROFILE_ID) && meta["appliedId"] == PROFILE_ID
}

pub(super) fn attached_metadata(
    mut meta: Value,
    select: bool,
    has_legacy: bool,
) -> Result<Vec<u8>> {
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
    serialize(&meta)
}

pub(super) fn restored_metadata(mut meta: Value, record: &Recovery) -> Result<Vec<u8>> {
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
    serialize(&meta)
}
