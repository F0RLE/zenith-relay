mod response;
mod route;

use super::super::{GatewayRuntime, ResponseAffinityBinding};
use sha2::{Digest, Sha256};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard};

// Order each key's in-memory decision and durable write, including across
// replacement runtimes. Stripes bound lock storage without serializing every
// chat. Never hold the scheduler lock during store I/O.
static PROMPT_AFFINITY_LOCKS: [Mutex<()>; 64] = [const { Mutex::new(()) }; 64];

impl GatewayRuntime {
    pub(in crate::runtime) fn lock_prompt_affinity(
        &self,
        key: &str,
    ) -> Option<MutexGuard<'static, ()>> {
        self.response_affinity_store.as_ref().map(|_| {
            let mut hash = DefaultHasher::new();
            key.hash(&mut hash);
            crate::poison::mutex(
                &PROMPT_AFFINITY_LOCKS[(hash.finish() as usize) % PROMPT_AFFINITY_LOCKS.len()],
            )
        })
    }

    pub(crate) fn prompt_affinity_key(
        &self,
        local_key_id: &str,
        model: &str,
        prompt_cache_key: Option<&str>,
        client_context_id: Option<&str>,
    ) -> Option<String> {
        // Codex normally supplies prompt_cache_key. Older clients do not, but
        // still send a privacy-safe session/thread fingerprint. Keep that
        // session on the successful candidate so provider-side prefix caches
        // survive normal key rotation; explicit cache keys remain stronger.
        let (material_kind, material) = prompt_cache_key
            // Provider cache keys are opaque: check blanks without merging
            // distinct supplied values into the same local affinity binding.
            .filter(|cache_key| !cache_key.trim().is_empty())
            .map(|cache_key| ("cache", cache_key))
            .or_else(|| {
                client_context_id
                    .map(str::trim)
                    .filter(|context_id| !context_id.is_empty())
                    .map(|context_id| ("session", context_id))
            })?;
        let digest = hex::encode(Sha256::digest(
            format!(
                "prompt\0{}\0{}\0{}\0{}",
                local_key_id,
                crate::model_id_key(model),
                material_kind,
                material,
            )
            .as_bytes(),
        ));
        // Keep the material class in the opaque scheduler key so selection
        // can give explicit provider cache keys a stronger owner preference
        // without changing normal session-based rotation.
        Some(format!(
            "{}{}",
            if material_kind == "cache" {
                "cache:"
            } else {
                "session:"
            },
            digest
        ))
    }

    #[cfg(test)]
    pub(crate) fn bind_prompt_affinity(&self, key: Option<&str>, candidate_id: &str, now_ms: u64) {
        self.update_prompt_affinity(key, candidate_id, None, now_ms);
    }

    pub(crate) fn bind_prompt_affinity_for_lease(
        &self,
        key: Option<&str>,
        lease: &super::super::CandidateLease,
        now_ms: u64,
    ) {
        if !Arc::ptr_eq(&self.scheduler, &lease.scheduler) {
            return;
        }
        self.update_prompt_affinity(key, &lease.candidate_id, Some(lease), now_ms);
    }

    fn update_prompt_affinity(
        &self,
        key: Option<&str>,
        candidate_id: &str,
        lease: Option<&super::super::CandidateLease>,
        now_ms: u64,
    ) {
        if let Some(key) = key {
            let _persistence = self.lock_prompt_affinity(key);
            let binding = {
                let mut scheduler = self.lock_scheduler();
                if let Some(lease) = lease {
                    let Some((observed_key, observed_revision)) = lease.prompt_owner.as_ref()
                    else {
                        return;
                    };
                    if observed_key != key {
                        return;
                    }
                    let current = scheduler.prompt_affinity_binding(key, now_ms);
                    let same_owner = current
                        .as_ref()
                        .is_some_and(|(owner, _)| owner == candidate_id);
                    if !same_owner
                        && current.as_ref().map(|(_, revision)| *revision) != *observed_revision
                    {
                        scheduler.remember_prompt_affinity_spillover(key, candidate_id, now_ms);
                        return;
                    }
                }
                if !scheduler.bind_prompt_affinity_sticky(key, candidate_id, now_ms) {
                    return;
                }
                self.response_affinity_store
                    .as_ref()
                    .and_then(|_| scheduler.prompt_affinity_snapshot(key, now_ms))
            };
            if let (Some(store), Some((candidate_id, expires_at_ms))) =
                (self.response_affinity_store.as_ref(), binding)
            {
                let _ = store.upsert(&ResponseAffinityBinding {
                    key: key.to_owned(),
                    candidate_id,
                    expires_at_ms,
                });
            }
        }
    }

    pub(in crate::runtime) fn restore_prompt_affinity_from_store(&self, key: &str, now_ms: u64) {
        let Some(store) = self.response_affinity_store.as_ref() else {
            return;
        };
        // Warm memory needs neither disk I/O nor a wait for a pending write.
        if self.lock_scheduler().has_prompt_affinity(key, now_ms) {
            return;
        }
        let _persistence = self.lock_prompt_affinity(key);
        if self.lock_scheduler().has_prompt_affinity(key, now_ms) {
            return;
        }
        let Ok(Some(binding)) = store.find(key, now_ms) else {
            return;
        };
        let mut scheduler = self.lock_scheduler();
        let owner = scheduler
            .candidate(&binding.candidate_id)
            .map(|candidate| candidate.id.clone());
        if let Some(owner) = owner {
            scheduler.restore_prompt_affinity(key, &owner, binding.expires_at_ms, now_ms);
        }
    }

    pub(crate) fn invalidate_prompt_affinity(&self, key: Option<&str>, candidate_id: &str) -> bool {
        key.is_some_and(|key| {
            let _persistence = self.lock_prompt_affinity(key);
            // A spillover rejection or an old owner's late failure says
            // nothing about the currently warmed owner's provider cache.
            let invalidated = self
                .lock_scheduler()
                .invalidate_prompt_affinity_for_candidate(key, candidate_id);
            if invalidated && let Some(store) = self.response_affinity_store.as_ref() {
                let _ = store.delete(key);
            }
            invalidated
        })
    }
}
