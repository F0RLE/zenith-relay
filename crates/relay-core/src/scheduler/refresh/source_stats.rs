//! Runtime-only source observations are scoped to the exact checked endpoint.
//! Catalog discovery can normalize an address without changing configuration
//! revision, so the revision alone is not sufficient for cache delivery.
use super::RefreshFreshness;
use crate::SourceProviderStats;

#[derive(Clone, Debug)]
pub struct SourceStatsObservation {
    base_url: String,
    pub stats: SourceProviderStats,
}

impl SourceStatsObservation {
    pub fn new(base_url: String, stats: SourceProviderStats) -> Self {
        Self { base_url, stats }
    }

    pub fn current(&self, base_url: &str) -> Option<&SourceProviderStats> {
        (self.base_url == base_url).then_some(&self.stats)
    }

    pub fn snapshot(
        &self,
        base_url: &str,
        freshness: RefreshFreshness,
    ) -> Option<SourceProviderStats> {
        let mut value = self.current(base_url)?.clone();
        value.stale |= matches!(freshness, RefreshFreshness::Stale { .. });
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SourceStatsProvider, SourceStatsStatus};

    #[test]
    fn normalized_endpoint_cannot_receive_a_previous_endpoint_observation() {
        let stats =
            SourceProviderStats::empty(SourceStatsProvider::Zenith, SourceStatsStatus::Available)
                .observed(None, 42);
        let observation =
            SourceStatsObservation::new("https://provider.example.test".into(), stats);
        assert!(observation
            .snapshot(
                "https://provider.example.test/v1",
                RefreshFreshness::Fresh { as_of_ms: 1 }
            )
            .is_none());
        let stale = observation
            .snapshot(
                "https://provider.example.test",
                RefreshFreshness::Stale { as_of_ms: 1 },
            )
            .unwrap();
        assert!(stale.stale);
        assert_eq!(stale.as_of_ms, Some(42));
        assert!(!observation.stats.stale);
    }
}
