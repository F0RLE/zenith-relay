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
        RefreshIdentity::new(format!("source:{}", self.source_id), self.revision, 0)
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
    fn next(&mut self) -> Result<u64> {
        self.clock = self.clock.checked_add(1).ok_or_else(invalid)?;
        Ok(self.clock)
    }
    pub(super) fn with_sources(
        &self,
        previous: &[ProviderSourceRecord],
        sources: &[ProviderSourceRecord],
    ) -> Result<Self> {
        let previous = previous
            .iter()
            .map(|source| (source.id.as_str(), source))
            .collect::<BTreeMap<_, _>>();
        let mut next = self.clone();
        next.sources.clear();
        for source in sources {
            let revision = if previous
                .get(source.id.as_str())
                .is_some_and(|old| same_scope(old, source))
            {
                self.sources.get(&source.id).copied().ok_or_else(invalid)?
            } else {
                next.next()?
            };
            if next.sources.insert(source.id.clone(), revision).is_some() {
                return Err(invalid());
            }
        }
        Ok(next)
    }
}
impl LocalPoolStore {
    pub(crate) fn source_refresh_scope(
        &self,
        id: &str,
    ) -> Result<(ProviderSourceRecord, SourceRefreshFence)> {
        let source = self
            .source(id)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
        let revision = self
            .source_refresh_revisions
            .sources
            .get(id)
            .copied()
            .ok_or_else(invalid)?;
        Ok((
            source,
            SourceRefreshFence {
                source_id: id.into(),
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
    pub(crate) fn invalidate_source_refresh(&mut self, id: &str) -> Result<()> {
        let mut next = self.source_refresh_revisions.clone();
        if next.sources.contains_key(id) {
            let revision = next.next()?;
            next.sources.insert(id.into(), revision);
        }
        self.database
            .replace_state_json(&[(STATE_SOURCE_REVISIONS, serialize_state(&next)?)])?;
        self.source_refresh_revisions = next;
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
        let record = sources
            .iter_mut()
            .find(|source| source.id == fence.source_id)
            .ok_or_else(invalid)?;
        apply(record)?;
        if record.id != fence.source_id {
            return Err(invalid());
        }
        let result = record.clone();
        // Only this observation owner bypasses configuration revision updates.
        // Normal/manual catalog edits still retire both source resource kinds.
        self.database
            .replace_state_json(&[(STATE_SOURCES, serialize_state(&sources)?)])?;
        self.sources = sources;
        self.notify_refresh_changed();
        Ok(result)
    }
}
fn same_scope(old: &ProviderSourceRecord, new: &ProviderSourceRecord) -> bool {
    old.enabled == new.enabled
        && old.base_url == new.base_url
        && old.secret_ref == new.secret_ref
        && old.wire_api == new.wire_api
        && old.protocol_bindings == new.protocol_bindings
        && old.protocol_config == new.protocol_config
        && old.models == new.models
        && (old.last_test_status.as_deref() == Some("manual"))
            == (new.last_test_status.as_deref() == Some("manual"))
}
fn invalid() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        "source refresh revisions are invalid",
    )
}
