use std::collections::HashMap;

#[derive(Clone, Debug)]
struct Binding {
    candidate_id: String,
    ttl_ms: u64,
    expires_at: u64,
    last_touched_at: u64,
}

#[derive(Clone, Debug)]
pub struct AffinityCache {
    bindings: HashMap<String, Binding>,
    max_entries: usize,
    ttl_ms: u64,
}

impl AffinityCache {
    pub fn new(max_entries: usize, ttl_ms: u64) -> Self {
        Self {
            bindings: HashMap::new(),
            max_entries,
            ttl_ms,
        }
    }

    pub fn get(&mut self, key: &str, now_ms: u64) -> Option<&str> {
        self.prune(now_ms);
        self.bindings
            .get(key)
            .map(|binding| binding.candidate_id.as_str())
    }

    pub fn refresh(&mut self, key: &str, now_ms: u64) -> bool {
        self.prune(now_ms);
        let Some(binding) = self.bindings.get_mut(key) else {
            return false;
        };
        binding.expires_at = now_ms.saturating_add(binding.ttl_ms);
        binding.last_touched_at = now_ms;
        true
    }

    pub fn bind(&mut self, key: impl Into<String>, candidate_id: impl Into<String>, now_ms: u64) {
        self.bind_for(key, candidate_id, now_ms, self.ttl_ms);
    }

    pub fn bind_for(
        &mut self,
        key: impl Into<String>,
        candidate_id: impl Into<String>,
        now_ms: u64,
        ttl_ms: u64,
    ) {
        if self.max_entries == 0 || ttl_ms == 0 {
            return;
        }
        self.prune(now_ms);
        let key = key.into();
        if !self.bindings.contains_key(&key) && self.bindings.len() >= self.max_entries {
            self.evict_oldest();
        }
        self.bindings.insert(
            key,
            Binding {
                candidate_id: candidate_id.into(),
                ttl_ms,
                expires_at: now_ms.saturating_add(ttl_ms),
                last_touched_at: now_ms,
            },
        );
    }

    pub fn invalidate(&mut self, key: &str) -> bool {
        self.bindings.remove(key).is_some()
    }

    pub fn invalidate_candidate(&mut self, candidate_id: &str) -> usize {
        let previous_len = self.bindings.len();
        self.bindings
            .retain(|_, binding| binding.candidate_id != candidate_id);
        previous_len - self.bindings.len()
    }

    pub fn clear(&mut self) {
        self.bindings.clear();
    }

    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    fn prune(&mut self, now_ms: u64) {
        self.bindings
            .retain(|_, binding| binding.expires_at > now_ms);
    }

    fn evict_oldest(&mut self) {
        let oldest = self
            .bindings
            .iter()
            .min_by(|(left_key, left), (right_key, right)| {
                left.last_touched_at
                    .cmp(&right.last_touched_at)
                    .then_with(|| left_key.cmp(right_key))
            })
            .map(|(key, _)| key.clone());
        if let Some(key) = oldest {
            self.bindings.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_is_bounded_expires_and_supports_invalidation() {
        let mut cache = AffinityCache::new(2, 10);
        cache.bind("old", "a", 0);
        cache.bind("kept", "b", 1);
        assert!(cache.refresh("kept", 2));
        cache.bind("new", "c", 3);
        assert_eq!(cache.get("old", 3), None);
        assert_eq!(cache.get("kept", 3), Some("b"));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.invalidate_candidate("b"), 1);
        assert_eq!(cache.get("kept", 3), None);
        assert!(!cache.refresh("new", 13));
        assert_eq!(cache.get("new", 13), None);
        assert!(cache.is_empty());

        let mut disabled_default = AffinityCache::new(1, 0);
        disabled_default.bind_for("response", "a", 20, 5);
        assert_eq!(disabled_default.get("response", 24), Some("a"));
        assert_eq!(disabled_default.get("response", 25), None);
    }
}
