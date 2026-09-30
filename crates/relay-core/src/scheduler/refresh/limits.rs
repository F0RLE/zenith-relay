use super::{RefreshCoordinator, RefreshEntry, RefreshKind};

/// Management traffic is separate from inference leases. These are runtime
/// bounds, not a provider promise; adapters can impose a longer Retry-After.
#[derive(Clone, Copy, Debug)]
pub struct RefreshLimits {
    pub max_entries: usize,
    pub concurrent: usize,
    pub per_origin: usize,
    pub reserved_auth: usize,
    pub start_spacing_ms: u64,
    pub origin_spacing_ms: u64,
    pub minimum_interval_ms: u64,
}

impl Default for RefreshLimits {
    fn default() -> Self {
        Self {
            max_entries: 8_192,
            concurrent: 8,
            per_origin: 3,
            reserved_auth: 1,
            start_spacing_ms: 50,
            origin_spacing_ms: 250,
            minimum_interval_ms: 1_000,
        }
    }
}

// Weighted class round-robin. Auth has reserved capacity AND a larger share,
// but continuously due auth/quota cannot starve models or balance forever.
const CLASS_ORDER: [RefreshKind; 8] = [
    RefreshKind::Auth,
    RefreshKind::Quota,
    RefreshKind::Auth,
    RefreshKind::Models,
    RefreshKind::Quota,
    RefreshKind::Balance,
    RefreshKind::Metadata,
    RefreshKind::Prices,
];

impl RefreshCoordinator {
    pub(super) fn class_distance(&self, kind: RefreshKind) -> usize {
        (0..CLASS_ORDER.len())
            .find(|offset| CLASS_ORDER[(self.class_cursor + offset) % CLASS_ORDER.len()] == kind)
            .unwrap_or(CLASS_ORDER.len())
    }

    pub(super) fn advance_class(&mut self, kind: RefreshKind) {
        self.class_cursor = (self.class_cursor + self.class_distance(kind) + 1) % CLASS_ORDER.len();
    }

    pub(super) fn capacity_available(&self, entry: &RefreshEntry, kind: RefreshKind) -> bool {
        let total = self.jobs.len();
        let origin_total = self
            .jobs
            .values()
            .filter(|job| job.origin == entry.origin)
            .count();
        if total >= self.limits.concurrent || origin_total >= self.limits.per_origin {
            return false;
        }
        if kind == RefreshKind::Auth {
            return true;
        }
        let ordinary = self
            .jobs
            .values()
            .filter(|job| job.key.kind != RefreshKind::Auth)
            .count();
        let origin_ordinary = self
            .jobs
            .values()
            .filter(|job| job.origin == entry.origin && job.key.kind != RefreshKind::Auth)
            .count();
        ordinary < self.limits.concurrent - self.limits.reserved_auth
            && origin_ordinary < self.limits.per_origin - self.limits.reserved_auth
    }

    pub(super) fn eligible_at(&self, entry: &RefreshEntry) -> Option<u64> {
        entry.next_due_ms.map(|due| {
            due.max(entry.not_before_ms).max(self.next_start_ms).max(
                self.origin_next_start
                    .get(&entry.origin)
                    .copied()
                    .unwrap_or_default(),
            )
        })
    }
}
