use super::super::GatewayRuntime;
use crate::{CandidateHealth, CandidateQuota, CandidateQuotaState};

impl GatewayRuntime {
    pub fn update_candidate_availability(
        &self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        quota: CandidateQuota,
    ) -> bool {
        let updated = self.lock_scheduler().update_candidate_availability(
            candidate_id,
            enabled,
            health,
            quota,
        );
        if updated {
            self.candidate_availability.notify_waiters();
        }
        updated
    }

    pub fn update_candidate_availability_at(
        &self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        quota: CandidateQuota,
        quota_updated_at_ms: Option<u64>,
    ) -> bool {
        let updated = self.lock_scheduler().update_candidate_availability_at(
            candidate_id,
            enabled,
            health,
            quota,
            quota_updated_at_ms,
        );
        if updated {
            self.candidate_availability.notify_waiters();
        }
        updated
    }

    /// Applies the complete refreshed account state without rebuilding the
    /// gateway. Quota windows and provider credits originate from one provider
    /// response and must reach the scheduler together.
    pub fn update_candidate_availability_with_quota_at(
        &self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        quota_state: CandidateQuotaState,
    ) -> bool {
        let updated = self
            .lock_scheduler()
            .update_candidate_availability_with_quota_at(
                candidate_id,
                enabled,
                health,
                quota_state,
            );
        if updated {
            self.candidate_availability.notify_waiters();
        }
        updated
    }
}
