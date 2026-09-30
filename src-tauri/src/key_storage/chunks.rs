use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(super) const CHUNK_UTF16_UNITS: usize = 1024;
pub(super) const MAX_CHUNKS: usize = 4096;
pub(super) const MANIFEST_PREFIX: &str = "__zenith_relay_secret_manifest__:";
pub(super) const MANIFEST_VERSION: u8 = 1;

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct SecretManifest {
    pub(super) version: u8,
    pub(super) generation: String,
    pub(super) count: usize,
    pub(super) sha256: String,
}

pub(super) fn split_secret(value: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    let mut units = 0;
    for character in value.chars() {
        let character_units = character.len_utf16();
        if units + character_units > CHUNK_UTF16_UNITS && !chunk.is_empty() {
            chunks.push(std::mem::take(&mut chunk));
            units = 0;
        }
        chunk.push(character);
        units += character_units;
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    chunks
}

pub(super) fn encode_manifest(manifest: &SecretManifest) -> Result<String, String> {
    let json = serde_json::to_string(manifest)
        .map_err(|_| "Не удалось подготовить манифест защищённого секрета".to_string())?;
    Ok(format!("{MANIFEST_PREFIX}{json}"))
}

pub(super) fn decode_manifest(value: &str) -> Result<SecretManifest, String> {
    let json = value
        .strip_prefix(MANIFEST_PREFIX)
        .ok_or_else(|| "Некорректный манифест защищённого секрета".to_string())?;
    let manifest: SecretManifest = serde_json::from_str(json)
        .map_err(|_| "Некорректный манифест защищённого секрета".to_string())?;
    let valid_generation = manifest.generation.len() == 32
        && manifest
            .generation
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit());
    let valid_hash =
        manifest.sha256.len() == 64 && manifest.sha256.bytes().all(|byte| byte.is_ascii_hexdigit());
    if manifest.version != MANIFEST_VERSION
        || manifest.count == 0
        || manifest.count > MAX_CHUNKS
        || !valid_generation
        || !valid_hash
    {
        return Err("Некорректный манифест защищённого секрета".to_string());
    }
    Ok(manifest)
}

pub(super) fn chunk_user(user: &str, generation: &str, index: usize) -> String {
    format!("{user}:chunk:{generation}:{index}")
}

pub(super) fn delete_manifest_chunks(service: &str, user: &str, manifest: &SecretManifest) {
    let _ = delete_manifest_chunks_result(service, user, manifest);
}

pub(super) fn delete_manifest_chunks_result(
    service: &str,
    user: &str,
    manifest: &SecretManifest,
) -> Result<(), String> {
    let mut first_error = None;
    for index in 0..manifest.count {
        if let Err(error) =
            super::delete_from_service(service, &chunk_user(user, &manifest.generation, index))
        {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

pub(super) fn sha256_hex(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
