//! Source incarnations are independent from account/proxy revisions. Neither
//! credentials nor credential hashes are persisted as refresh identity.
use super::{serialize_state, LocalPoolStore, STATE_SOURCES};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::ProviderSourceRecord,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use zenith_relay_core::scheduler::refresh::RefreshIdentity;

pub(super) const STATE_SOURCE_REVISIONS: &str = "source_refresh_revisions";
#[derive(Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceRefreshRevisions {
    clock: u64,
    sources: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceRefreshFence {
    pub source_id: String,
    revision: u64,
}
impl SourceRefreshFence {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn identity(&self) -> RefreshIdentity {
        RefreshIdentity::new(
            zenith_relay_core::scheduler::source_member_key(&self.source_id),
            self.revision,
            0,
        )
    }
}
impl SourceRefreshRevisions {
    pub(super) fn validate(&self, sources: &[ProviderSourceRecord]) -> Result<()> {
        if self.sources.len() != sources.len()
            || sources.iter().any(|source| {
                self.sources
                    .get(&source.id)
                    .is_none_or(|rev| *rev == 0 || *rev > self.clock)
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
    fn allocate_revision(&mut self) -> Result<u64> {
        self.clock = self.clock.checked_add(1).ok_or_else(invalid)?;
        Ok(self.clock)
    }
    pub(super) fn with_sources(
        &self,
        previous_sources: &[ProviderSourceRecord],
        sources: &[ProviderSourceRecord],
    ) -> Result<Self> {
        let previous_sources_by_id = previous_sources
            .iter()
            .map(|source| (source.id.as_str(), source))
            .collect::<BTreeMap<_, _>>();
        let mut updated_revisions = self.clone();
        updated_revisions.sources.clear();
        for source in sources {
            let revision = if previous_sources_by_id
                .get(source.id.as_str())
                .is_some_and(|previous_source| same_scope(previous_source, source))
            {
                self.sources.get(&source.id).copied().ok_or_else(invalid)?
            } else {
                updated_revisions.allocate_revision()?
            };
            if updated_revisions
                .sources
                .insert(source.id.clone(), revision)
                .is_some()
            {
                return Err(invalid());
            }
        }
        Ok(updated_revisions)
    }
}
impl LocalPoolStore {
    pub(crate) fn source_refresh_scope(
        &self,
        source_id: &str,
    ) -> Result<(ProviderSourceRecord, SourceRefreshFence)> {
        let source_record = self
            .source(source_id)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
        let revision = self
            .source_refresh_revisions
            .sources
            .get(source_id)
            .copied()
            .ok_or_else(invalid)?;
        Ok((
            source_record,
            SourceRefreshFence {
                source_id: source_id.into(),
                revision,
            },
        ))
    }
    pub(crate) fn ensure_source_refresh_current(&self, fence: &SourceRefreshFence) -> Result<()> {
        if self.source_refresh_revisions.sources.get(&fence.source_id) != Some(&fence.revision) {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "source changed during refresh",
            ));
        }
        Ok(())
    }
    pub(crate) fn invalidate_source_refresh(&mut self, source_id: &str) -> Result<()> {
        let mut updated_revisions = self.source_refresh_revisions.clone();
        if updated_revisions.sources.contains_key(source_id) {
            let revision = updated_revisions.allocate_revision()?;
            updated_revisions.sources.insert(source_id.into(), revision);
        }
        self.database.replace_state_json(&[(
            STATE_SOURCE_REVISIONS,
            serialize_state(&updated_revisions)?,
        )])?;
        self.source_refresh_revisions = updated_revisions;
        self.notify_refresh_changed();
        Ok(())
    }
    pub(crate) fn apply_source_refresh(
        &mut self,
        fence: &SourceRefreshFence,
        apply: impl FnOnce(&mut ProviderSourceRecord) -> Result<()>,
    ) -> Result<ProviderSourceRecord> {
        self.ensure_source_refresh_current(fence)?;
        let mut sources = self.sources.clone();
        let source_record = sources
            .iter_mut()
            .find(|source| source.id == fence.source_id)
            .ok_or_else(invalid)?;
        apply(source_record)?;
        if source_record.id != fence.source_id {
            return Err(invalid());
        }
        let refreshed_source = source_record.clone();
        // Only this observation owner bypasses configuration revision updates.
        // Normal/manual catalog edits still retire both source resource kinds.
        self.database
            .replace_state_json(&[(STATE_SOURCES, serialize_state(&sources)?)])?;
        self.sources = sources;
        self.notify_refresh_changed();
        Ok(refreshed_source)
    }
}
fn same_scope(
    previous_source: &ProviderSourceRecord,
    updated_source: &ProviderSourceRecord,
) -> bool {
    previous_source.enabled == updated_source.enabled
        && previous_source.base_url == updated_source.base_url
        && previous_source.secret_ref == updated_source.secret_ref
        && previous_source.wire_api == updated_source.wire_api
        && previous_source.protocol_bindings == updated_source.protocol_bindings
        && previous_source.protocol_config == updated_source.protocol_config
        && previous_source.models == updated_source.models
        && (previous_source.last_test_status.as_deref() == Some("manual"))
            == (updated_source.last_test_status.as_deref() == Some("manual"))
}
fn invalid() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        "source refresh revisions are invalid",
    )
}
