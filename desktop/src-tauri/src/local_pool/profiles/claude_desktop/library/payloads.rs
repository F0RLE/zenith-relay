use super::*;

pub(super) fn serialize(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec_pretty(value).map_err(io_error)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(invalid());
    }
    Ok(bytes)
}

pub(super) fn profile_is_current(meta: &Value, profile: Option<&[u8]>, bytes: &[u8]) -> bool {
    profile == Some(bytes) && has_entry(meta, PROFILE_ID) && meta["appliedId"] == PROFILE_ID
}

pub(super) fn attached_profile(
    profile: Option<&[u8]>,
    record: Option<&Recovery>,
    config: &Value,
) -> Result<(Vec<u8>, bool)> {
    let preserve = record.is_some_and(|record| record.preserve_profile_on_restore);
    let edited = profile.is_some() && !record.is_some_and(|record| owns_profile(record, profile));
    if !preserve && !edited {
        return serialize(config).map(|bytes| (bytes, false));
    }
    let mut merged = profile
        .map(parse)
        .transpose()?
        .unwrap_or_else(|| json!({}))
        .as_object()
        .cloned()
        .ok_or_else(invalid_config)?;
    for (field, value) in config.as_object().ok_or_else(invalid_config)? {
        // Reconnection owns routing and the generated model catalog. Existing
        // client preferences and extension settings are the user's choices.
        if field.starts_with("inference") || !merged.contains_key(field) {
            merged.insert(field.clone(), value.clone());
        }
    }
    let merged = Value::Object(merged);
    let preserve = preserve || merged != *config;
    serialize(&merged).map(|bytes| (bytes, preserve))
}

pub(super) fn attached_metadata(mut meta: Value, select: bool) -> Result<Vec<u8>> {
    if !has_entry(&meta, PROFILE_ID) {
        meta["entries"]
            .as_array_mut()
            .ok_or_else(invalid)?
            .push(json!({"id": PROFILE_ID, "name": "Zenith Relay"}));
    }
    if select {
        meta["appliedId"] = json!(PROFILE_ID);
    }
    serialize(&meta)
}

pub(super) fn restored_metadata(
    mut meta: Value,
    record: &Recovery,
    preserve_profile: bool,
) -> Result<Vec<u8>> {
    if meta["appliedId"] == PROFILE_ID {
        if let Some(previous) = record
            .previous_applied_id
            .as_ref()
            .filter(|previous| has_entry(&meta, previous))
        {
            meta["appliedId"] = json!(previous);
        } else {
            meta.as_object_mut()
                .ok_or_else(invalid)?
                .remove("appliedId");
        }
    }
    if !preserve_profile {
        meta["entries"]
            .as_array_mut()
            .ok_or_else(invalid)?
            .retain(|entry| entry["id"] != PROFILE_ID);
    }
    serialize(&meta)
}
