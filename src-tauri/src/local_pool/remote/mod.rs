pub mod client;
pub mod deployment;
pub mod origin;

use crate::local_pool::{
    error::Result,
    store::{secret_store, settings_store},
};
use serde::{Deserialize, Serialize};
use std::path::Path;

const TARGETS_FILE: &str = "remote_targets.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTargetRecord {
    pub origin: String,
    pub server_id: String,
    pub identity_fingerprint: String,
    pub server_version: String,
    pub protocol_version: u16,
    pub allow_insecure_http: bool,
    pub secret_ref: String,
    pub connected_at_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteTargets {
    active: Option<RemoteTargetRecord>,
}

pub fn load_active(root: &Path) -> Result<Option<RemoteTargetRecord>> {
    let path = root.join("settings").join(TARGETS_FILE);
    Ok(settings_store::load_json::<RemoteTargets>(&path)?.and_then(|value| value.active))
}

pub fn save_active(root: &Path, target: Option<RemoteTargetRecord>) -> Result<()> {
    settings_store::save_json(
        &root.join("settings").join(TARGETS_FILE),
        &RemoteTargets { active: target },
    )
}

pub fn load_token(target: &RemoteTargetRecord) -> Result<Option<String>> {
    secret_store::load(&target.secret_ref)
}

pub fn save_token(target: &RemoteTargetRecord, token: &str) -> Result<()> {
    secret_store::save(&target.secret_ref, token)
}

pub fn delete_token(target: &RemoteTargetRecord) -> Result<()> {
    secret_store::delete(&target.secret_ref)
}
