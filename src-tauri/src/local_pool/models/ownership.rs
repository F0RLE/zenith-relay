use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipOperationKind {
    MoveToRemote,
    ReturnToLocal,
    ForceActivateLocal,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipOperationPhase {
    MovePrepared,
    MoveRemoteApplying,
    MoveRemoteCommitted,
    MoveLocalCommitted,
    ReturnPrepared,
    ReturnLocalStaged,
    ReturnRemoteRemoved,
    ReturnLocalCommitted,
    ForcePrepared,
    ForceLocalCommitted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipOperationRecord {
    pub id: String,
    pub kind: OwnershipOperationKind,
    pub phase: OwnershipOperationPhase,
    pub server_id: String,
    pub local_account_ids: Vec<String>,
    #[serde(default)]
    pub remote_account_ids: Vec<String>,
    #[serde(default)]
    pub created_remote_account_ids: Vec<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl OwnershipOperationRecord {
    pub fn validate(&self) -> Result<(), &'static str> {
        let has_prefixed_id = |candidate_id: &str, prefix: &str| {
            candidate_id.strip_prefix(prefix).is_some_and(|suffix| {
                suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        };
        let valid_object_id = |object_id: &str| zenith_relay_core::is_ascii_token(object_id, 128);
        if !has_prefixed_id(&self.id, "ownership_")
            || self.server_id.is_empty()
            || self.server_id.len() > 128
            || self.local_account_ids.is_empty()
            || self.local_account_ids.len() > 256
            || self
                .local_account_ids
                .iter()
                .any(|account_id| !valid_object_id(account_id))
            || self
                .remote_account_ids
                .iter()
                .any(|account_id| !valid_object_id(account_id))
            || self
                .created_remote_account_ids
                .iter()
                .any(|account_id| !valid_object_id(account_id))
            || self.updated_at_ms < self.created_at_ms
        {
            return Err("remote ownership operation is invalid");
        }
        let mut local_ids = HashSet::new();
        let mut remote_ids = HashSet::new();
        let mut created_ids = HashSet::new();
        if self
            .local_account_ids
            .iter()
            .any(|account_id| !local_ids.insert(account_id))
            || self
                .remote_account_ids
                .iter()
                .any(|account_id| !remote_ids.insert(account_id))
            || self
                .created_remote_account_ids
                .iter()
                .any(|account_id| !created_ids.insert(account_id))
            || self.remote_account_ids.len() > self.local_account_ids.len()
            || self.created_remote_account_ids.len() > self.local_account_ids.len()
        {
            return Err("remote ownership operation contains inconsistent account ids");
        }
        let valid_phase = matches!(
            (self.kind, self.phase),
            (
                OwnershipOperationKind::MoveToRemote,
                OwnershipOperationPhase::MovePrepared
                    | OwnershipOperationPhase::MoveRemoteApplying
                    | OwnershipOperationPhase::MoveRemoteCommitted
                    | OwnershipOperationPhase::MoveLocalCommitted
            ) | (
                OwnershipOperationKind::ReturnToLocal,
                OwnershipOperationPhase::ReturnPrepared
                    | OwnershipOperationPhase::ReturnLocalStaged
                    | OwnershipOperationPhase::ReturnRemoteRemoved
                    | OwnershipOperationPhase::ReturnLocalCommitted
            ) | (
                OwnershipOperationKind::ForceActivateLocal,
                OwnershipOperationPhase::ForcePrepared
                    | OwnershipOperationPhase::ForceLocalCommitted
            )
        );
        if !valid_phase {
            return Err("remote ownership operation phase is invalid");
        }
        Ok(())
    }
}
