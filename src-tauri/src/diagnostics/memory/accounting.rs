use std::sync::{Arc, Mutex, Weak};
use zenith_relay_core::gateway::CacheMemoryDiagnostics;

/// Only numeric accounting survives a snapshot. A Weak key cannot keep an old
/// model/price catalog alive, and distinguishes new allocations even if their
/// advertised revision is unchanged.
pub(super) struct CatalogAccounting<T> {
    current: Mutex<(Weak<T>, Vec<CacheMemoryDiagnostics>)>,
}

impl<T> CatalogAccounting<T> {
    pub(super) const fn new() -> Self {
        Self {
            current: Mutex::new((Weak::new(), Vec::new())),
        }
    }

    pub(super) fn append(
        &self,
        snapshot: &Arc<T>,
        output: &mut Vec<CacheMemoryDiagnostics>,
        measure: impl FnOnce(&T) -> Vec<CacheMemoryDiagnostics>,
    ) {
        let mut current = zenith_relay_core::poison::mutex(&self.current);
        let key = Arc::downgrade(snapshot);
        if !current.0.ptr_eq(&key) {
            *current = (key, measure(snapshot));
        }
        output.extend_from_slice(&current.1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn measure(value: &usize) -> Vec<CacheMemoryDiagnostics> {
        vec![CacheMemoryDiagnostics {
            key: "synthetic",
            entries: Some(1),
            bytes: Some(*value),
            limit_bytes: None,
            basis: "retained_payload",
        }]
    }

    #[test]
    fn unchanged_catalog_is_measured_once_without_retaining_its_payload() {
        let cache = CatalogAccounting::new();
        let snapshot = Arc::new(123);
        let lifetime = Arc::downgrade(&snapshot);
        let mut rows = Vec::new();
        let calls = AtomicUsize::new(0);
        for shared in [&snapshot, &snapshot.clone()] {
            cache.append(shared, &mut rows, |value| {
                calls.fetch_add(1, Ordering::Relaxed);
                measure(value)
            });
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].bytes, rows[1].bytes);
        assert_eq!(Arc::strong_count(&snapshot), 1);
        drop(snapshot);
        assert!(lifetime.upgrade().is_none());

        let replacement = Arc::new(456);
        cache.append(&replacement, &mut rows, measure);
        assert_eq!(rows[2].bytes, Some(456));
        assert_eq!(Arc::strong_count(&replacement), 1);
    }

    #[test]
    fn copy_on_write_invalidates_accounting_without_a_revision_key() {
        let cache = CatalogAccounting::new();
        let mut snapshot = Arc::new(123);
        let mut rows = Vec::new();
        cache.append(&snapshot, &mut rows, measure);
        *Arc::make_mut(&mut snapshot) = 456;
        cache.append(&snapshot, &mut rows, measure);
        assert_eq!(rows[0].bytes, Some(123));
        assert_eq!(rows[1].bytes, Some(456));
    }

    #[test]
    fn overlapping_measurements_share_one_traversal_but_keep_every_result() {
        let cache = CatalogAccounting::new();
        let snapshot = Arc::new(123);
        let calls = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    let mut rows = Vec::new();
                    cache.append(&snapshot, &mut rows, |value| {
                        calls.fetch_add(1, Ordering::Relaxed);
                        measure(value)
                    });
                    assert_eq!(rows.len(), 1);
                    assert_eq!(rows[0].bytes, Some(123));
                });
            }
        });
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}
