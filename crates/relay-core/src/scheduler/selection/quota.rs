use super::PoolScheduler;
use crate::scheduler::{CandidateQuota, RuntimeCandidate};

impl PoolScheduler {
    pub(super) fn routing_quota_factor(&self, candidate: &RuntimeCandidate) -> u64 {
        let reserve = self
            .protected_candidate
            .as_ref()
            .filter(|(candidate_id, _)| candidate_id == &candidate.id)
            .map_or(0, |(_, reserve)| *reserve);
        match candidate.quota {
            CandidateQuota::Available(remaining) => remaining.saturating_sub(reserve),
            CandidateQuota::Unknown if reserve == 0 => 1,
            CandidateQuota::Unknown | CandidateQuota::Exhausted | CandidateQuota::Stale => 0,
        }
    }

    pub(super) fn routing_quota(&self, candidate: &RuntimeCandidate) -> CandidateQuota {
        match candidate.quota {
            CandidateQuota::Available(_) => {
                CandidateQuota::Available(self.routing_quota_factor(candidate))
            }
            quota => quota,
        }
    }
}
