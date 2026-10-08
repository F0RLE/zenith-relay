mod response;

use super::super::{GatewayRuntime, ResponseAffinityBinding};
use crate::PROMPT_AFFINITY_TTL_MS;
use sha2::{Digest, Sha256};

impl GatewayRuntime {
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
            .map(str::trim)
            .filter(|cache_key| !cache_key.is_empty())
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

    pub(crate) fn bind_prompt_affinity(&self, key: Option<&str>, candidate_id: &str, now_ms: u64) {
        if let Some(key) = key {
            if self
                .lock_scheduler()
                .bind_prompt_affinity_sticky(key, candidate_id, now_ms)
            {
                self.persist_prompt_affinity(key, candidate_id, now_ms);
            }
        }
    }

    pub(crate) fn invalidate_prompt_affinity(&self, key: Option<&str>) -> bool {
        key.is_some_and(|key| {
            let invalidated = self.lock_scheduler().invalidate_prompt_affinity(key);
            if invalidated {
                if let Some(store) = self.response_affinity_store.as_ref() {
                    let _ = store.delete(key);
                }
            }
            invalidated
        })
    }

    pub(crate) fn persist_prompt_affinity(&self, key: &str, candidate_id: &str, now_ms: u64) {
        if let Some(store) = self.response_affinity_store.as_ref() {
            let _ = store.upsert(&ResponseAffinityBinding {
                key: key.to_string(),
                candidate_id: candidate_id.to_string(),
                expires_at_ms: now_ms.saturating_add(PROMPT_AFFINITY_TTL_MS),
            });
        }
    }
}
