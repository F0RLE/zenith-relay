//! Fence optional work to the Debug session that requested it.

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct Filter(AtomicU64);

impl Filter {
    fn set(&self, enabled: bool) {
        let _ = self
            .0
            .try_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                Some((epoch & !1).wrapping_add(2) | u64::from(enabled))
            });
    }

    fn epoch(&self, debug: bool) -> Option<u64> {
        let epoch = self.0.load(Ordering::Acquire);
        (debug && epoch & 1 == 1).then_some(epoch)
    }
}

static FILTER: Filter = Filter(AtomicU64::new(0));

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct DebugSession(u64);

impl DebugSession {
    pub(crate) fn capture() -> Option<Self> {
        FILTER.epoch(super::is_debug_enabled()).map(Self)
    }

    pub(crate) fn is_current(self) -> bool {
        Self::capture() == Some(self)
    }

    pub(super) fn epoch(self) -> u64 {
        self.0
    }
}

pub(super) fn set_debug_enabled(enabled: bool) {
    FILTER.set(enabled);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_is_opt_in_requires_debug_and_invalidates_queued_epochs() {
        let filter = Filter::default();
        assert_eq!(filter.epoch(true), None);
        filter.set(true);
        assert_eq!(filter.epoch(false), None);
        let first = filter.epoch(true).unwrap();
        filter.set(false);
        assert_eq!(filter.epoch(true), None);
        filter.set(true);
        assert_ne!(filter.epoch(true), Some(first));
    }
}
