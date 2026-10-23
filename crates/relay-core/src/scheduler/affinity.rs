use std::collections::HashMap;

const MAX_WARM_ALTERNATIVES: usize = 4;

#[derive(Clone, Debug)]
struct Binding {
    candidate_id: String,
    revision: u64,
    ttl_ms: u64,
    expires_at: u64,
    last_touched_at: u64,
    // Successful spillovers are only local hints, never additional owners.
    alternatives: Vec<(String, u64)>,
}

#[derive(Clone, Debug)]
pub struct AffinityCache {
    bindings: HashMap<String, Binding>,
    next_revision: u64,
    max_entries: usize,
    ttl_ms: u64,
}

impl AffinityCache {
    pub fn new(max_entries: usize, ttl_ms: u64) -> Self {
        Self {
            bindings: HashMap::new(),
            next_revision: 0,
            max_entries,
            ttl_ms,
        }
    }

    pub fn get(&mut self, key: &str, now_ms: u64) -> Option<&str> {
        self.get_with_revision(key, now_ms).map(|(owner, _)| owner)
    }

    pub fn get_with_revision(&mut self, key: &str, now_ms: u64) -> Option<(&str, u64)> {
        self.prune(now_ms);
        self.bindings
            .get(key)
            .map(|binding| (binding.candidate_id.as_str(), binding.revision))
    }

    pub(crate) fn get_with_expiry(&self, key: &str, now_ms: u64) -> Option<(&str, u64)> {
        self.bindings
            .get(key)
            .filter(|binding| binding.expires_at > now_ms)
            .map(|binding| (binding.candidate_id.as_str(), binding.expires_at))
    }

    pub fn refresh(&mut self, key: &str, now_ms: u64) -> bool {
        self.prune(now_ms);
        let Some(binding) = self.bindings.get_mut(key) else {
            return false;
        };
        binding.expires_at = binding
            .expires_at
            .max(now_ms.saturating_add(binding.ttl_ms));
        binding.last_touched_at = binding.last_touched_at.max(now_ms);
        true
    }

    pub fn bind(&mut self, key: impl Into<String>, candidate_id: impl Into<String>, now_ms: u64) {
        self.bind_for(key, candidate_id, now_ms, self.ttl_ms);
    }

    /// Refresh an existing owner, or create a binding when none exists.
    ///
    /// A temporary scheduler spillover must not silently replace a durable
    /// prompt-cache owner.  The owner is replaced only after the normal
    /// invalidation path removes it (for example after an auth/quota failure).
    pub fn bind_if_unbound_or_same(
        &mut self,
        key: impl Into<String>,
        candidate_id: &str,
        now_ms: u64,
    ) -> bool {
        let key = key.into();
        match self.get(&key, now_ms) {
            Some(bound_candidate_id) if bound_candidate_id == candidate_id => {
                self.refresh(&key, now_ms)
            }
            Some(_) => false,
            None => {
                self.bind(key, candidate_id.to_string(), now_ms);
                true
            }
        }
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
        self.insert_binding(
            key.into(),
            candidate_id.into(),
            ttl_ms,
            now_ms.saturating_add(ttl_ms),
            now_ms,
        );
    }

    pub(crate) fn remember_alternative(&mut self, key: &str, candidate_id: &str, now_ms: u64) {
        let Some(binding) = self
            .bindings
            .get_mut(key)
            .filter(|binding| binding.expires_at > now_ms)
        else {
            return;
        };
        if binding.candidate_id == candidate_id || binding.ttl_ms == 0 {
            return;
        }
        binding
            .alternatives
            .retain(|(_, expires_at)| *expires_at > now_ms);
        let expires_at = now_ms.saturating_add(binding.ttl_ms);
        if let Some((_, existing_expiry)) = binding
            .alternatives
            .iter_mut()
            .find(|(id, _)| id == candidate_id)
        {
            *existing_expiry = (*existing_expiry).max(expires_at);
        } else {
            binding
                .alternatives
                .push((candidate_id.to_owned(), expires_at));
        }
        binding.alternatives.sort_unstable_by(|left, right| {
            right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0))
        });
        binding.alternatives.truncate(MAX_WARM_ALTERNATIVES);
    }

    pub(crate) fn alternatives(&self, key: &str, now_ms: u64) -> Vec<String> {
        let Some(binding) = self
            .bindings
            .get(key)
            .filter(|binding| binding.expires_at > now_ms)
        else {
            return Vec::new();
        };
        binding
            .alternatives
            .iter()
            .filter(|(_, expires_at)| *expires_at > now_ms)
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn restore(
        &mut self,
        key: impl Into<String>,
        candidate_id: impl Into<String>,
        expires_at: u64,
        now_ms: u64,
    ) {
        if self.max_entries == 0 || expires_at <= now_ms {
            return;
        }
        self.insert_binding(
            key.into(),
            candidate_id.into(),
            self.ttl_ms,
            expires_at,
            now_ms,
        );
    }

    pub fn contains(&mut self, key: &str, now_ms: u64) -> bool {
        self.prune(now_ms);
        self.bindings.contains_key(key)
    }

    pub fn invalidate(&mut self, key: &str) -> bool {
        self.bindings.remove(key).is_some()
    }

    pub(crate) fn invalidate_if_owner(&mut self, key: &str, candidate_id: &str) -> bool {
        if self
            .bindings
            .get(key)
            .is_some_and(|binding| binding.candidate_id == candidate_id)
        {
            return self.invalidate(key);
        }
        if let Some(binding) = self.bindings.get_mut(key) {
            binding.alternatives.retain(|(id, _)| id != candidate_id);
        }
        false
    }

    pub fn invalidate_candidate(&mut self, candidate_id: &str) -> usize {
        let previous_len = self.bindings.len();
        self.bindings.retain(|_, binding| {
            binding.alternatives.retain(|(id, _)| id != candidate_id);
            binding.candidate_id != candidate_id
        });
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

    fn insert_binding(
        &mut self,
        key: String,
        candidate_id: String,
        ttl_ms: u64,
        expires_at: u64,
        now_ms: u64,
    ) {
        self.prune(now_ms);
        if !self.bindings.contains_key(&key) && self.bindings.len() >= self.max_entries {
            self.evict_oldest();
        }
        self.next_revision = self
            .next_revision
            .checked_add(1)
            .expect("affinity revision exhausted");
        self.bindings.insert(
            key,
            Binding {
                candidate_id,
                revision: self.next_revision,
                ttl_ms,
                expires_at,
                last_touched_at: now_ms,
                alternatives: Vec::new(),
            },
        );
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
    fn warm_alternatives_are_bounded_expire_and_do_not_refresh_the_owner() {
        let mut cache = AffinityCache::new(2, 100);
        cache.bind("thread", "owner", 0);
        let (_, revision) = cache.get_with_revision("thread", 0).unwrap();
        for index in 1..=6 {
            cache.remember_alternative("thread", &format!("warm-{index}"), index);
        }
        assert_eq!(
            cache.alternatives("thread", 6),
            ["warm-6", "warm-5", "warm-4", "warm-3"]
        );
        assert_eq!(
            cache.alternatives("another-thread", 6),
            Vec::<String>::new()
        );
        assert_eq!(
            cache.get_with_revision("thread", 6),
            Some(("owner", revision))
        );
        assert_eq!(cache.get_with_expiry("thread", 6), Some(("owner", 100)));
        // A late observation cannot make a warm hint older or reorder it.
        cache.remember_alternative("thread", "warm-6", 2);
        assert_eq!(cache.alternatives("thread", 6)[0], "warm-6");
        assert!(cache.refresh("thread", 50));
        assert!(cache.alternatives("thread", 106).is_empty());
        assert_eq!(cache.get("thread", 106), Some("owner"));

        cache.remember_alternative("thread", "warm-again", 140);
        assert!(cache.alternatives("thread", 150).is_empty());
        assert_eq!(cache.get("thread", 150), None);
    }

    #[test]
    fn alternative_invalidation_and_restore_do_not_adopt_or_discard_another_owner() {
        let mut cache = AffinityCache::new(3, 100);
        for key in ["one", "two"] {
            cache.bind(key, "owner", 0);
            cache.remember_alternative(key, "warm", 1);
        }
        assert!(!cache.invalidate_if_owner("one", "warm"));
        assert!(cache.alternatives("one", 2).is_empty());
        assert_eq!(cache.get("one", 2), Some("owner"));
        assert_eq!(cache.alternatives("two", 2), ["warm"]);
        assert_eq!(cache.invalidate_candidate("warm"), 0);
        assert!(cache.alternatives("two", 2).is_empty());

        cache.remember_alternative("one", "warm", 3);
        cache.restore("one", "owner", 100, 4);
        assert!(cache.alternatives("one", 4).is_empty());
        cache.remember_alternative("one", "warm", 5);
        assert!(cache.invalidate_if_owner("one", "owner"));
        assert!(cache.alternatives("one", 5).is_empty());
        assert_eq!(cache.get("two", 5), Some("owner"));
    }

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

        let mut restored = AffinityCache::new(1, 10);
        restored.restore("response", "a", 25, 20);
        assert_eq!(restored.get("response", 24), Some("a"));
        assert!(restored.refresh("response", 24));
        assert_eq!(restored.get("response", 33), Some("a"));
        assert_eq!(restored.get("response", 34), None);
    }

    #[test]
    fn rebinding_same_owner_changes_revision_but_refresh_does_not() {
        let mut cache = AffinityCache::new(1, 100);
        cache.bind("response", "a", 10);
        let (_, first) = cache.get_with_revision("response", 10).unwrap();
        assert!(cache.refresh("response", 11));
        assert_eq!(cache.get_with_revision("response", 11), Some(("a", first)));
        assert!(cache.invalidate("response"));
        cache.bind("response", "a", 12);
        let (_, second) = cache.get_with_revision("response", 12).unwrap();
        assert!(second > first);
    }

    #[test]
    fn late_affinity_refresh_preserves_newer_expiry_and_recency() {
        let mut cache = AffinityCache::new(2, 100);
        cache.bind("warm", "a", 0);
        cache.bind("other", "b", 40);
        let (_, revision) = cache.get_with_revision("warm", 40).unwrap();
        assert!(cache.refresh("warm", 50));
        assert!(cache.refresh("warm", 25));
        assert_eq!(cache.get_with_revision("warm", 50), Some(("a", revision)));
        assert_eq!(cache.get_with_expiry("warm", 50), Some(("a", 150)));

        cache.bind("new", "c", 60);
        assert_eq!(cache.get("other", 60), None);
        assert_eq!(cache.get("warm", 149), Some("a"));
        assert_eq!(cache.get("warm", 150), None);
    }
}
