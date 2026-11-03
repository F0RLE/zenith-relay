use std::sync::atomic::{AtomicU8, Ordering};

const RUNNING: u8 = 1;
const PENDING: u8 = 2;

/// One worker per desktop, with at most one follow-up pass for edits received
/// during a refresh. The atomic handoff cannot lose an edit at worker exit.
#[derive(Default)]
pub(in crate::local_pool) struct CatalogRefreshQueue(AtomicU8);

impl CatalogRefreshQueue {
    pub(in crate::local_pool) fn request(&self) -> bool {
        self.0.fetch_or(RUNNING | PENDING, Ordering::AcqRel) & RUNNING == 0
    }

    pub(in crate::local_pool) fn begin_pass(&self) {
        self.0.fetch_and(!PENDING, Ordering::AcqRel);
    }

    pub(in crate::local_pool) fn finish_pass(&self) -> bool {
        self.0
            .compare_exchange(RUNNING, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_before_refresh_uses_one_pass() {
        let queue = CatalogRefreshQueue::default();
        assert!(queue.request());
        for _ in 0..100 {
            assert!(!queue.request());
        }
        queue.begin_pass();
        assert!(!queue.finish_pass());
    }

    #[test]
    fn edits_during_refresh_schedule_only_one_follow_up() {
        let queue = CatalogRefreshQueue::default();
        assert!(queue.request());
        queue.begin_pass();
        for _ in 0..100 {
            assert!(!queue.request());
        }
        assert!(queue.finish_pass());
        queue.begin_pass();
        assert!(!queue.finish_pass());
    }

    #[test]
    fn edit_after_worker_exit_starts_a_new_worker() {
        let queue = CatalogRefreshQueue::default();
        assert!(queue.request());
        queue.begin_pass();
        assert!(!queue.finish_pass());
        assert!(queue.request());
        queue.begin_pass();
        assert!(!queue.finish_pass());
    }
}
