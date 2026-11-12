use super::*;
use crate::scheduler::rotation::RequestWaitKind;
use tokio::time::{Instant, sleep_until};

impl GatewayRuntime {
    /// Wait without a lease, using the existing bounded admission queue. The
    /// provider's cooldown and the request's own pacing must both have elapsed.
    pub(super) async fn retry_preferred_candidate(
        &self,
        mut request: AdmissionRequest,
    ) -> std::result::Result<Option<(Selection, CandidateLease)>, ()> {
        let Some(preferred) = request.budget.retry_candidate() else {
            return Ok(None);
        };
        loop {
            let notified = self.candidate_availability.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !request.budget.can_dispatch()
                || !self.model_enabled(&request.model)
                || self.lock_scheduler().is_retired()
            {
                return Err(());
            }
            let now = Instant::now();
            if !self.route_recovery_enabled() || now >= preferred.deadline {
                break;
            }
            let valid = {
                let scope = request.key.scope_read();
                let authorization = preferred
                    .token_revision
                    .as_ref()
                    .map(crate::accounts::TokenDispatchRevision::guard);
                let scheduler = self.lock_scheduler();
                let valid = request.model == preferred.model
                    && authorization.as_ref().is_none_or(Option::is_some)
                    && scheduler.candidate_permission_revision(&preferred.candidate_id)
                        == preferred.permission_revision
                    && scheduler
                        .candidate(&preferred.candidate_id)
                        .is_some_and(|candidate| {
                            candidate.is_visible(&request.model, &request.protocols, &scope)
                        });
                if valid {
                    request.tried.extend(
                        scheduler
                            .candidates()
                            .filter(|candidate| candidate.id != preferred.candidate_id)
                            .map(|candidate| candidate.id.clone()),
                    );
                    request.tried.remove(&preferred.candidate_id);
                }
                valid
            };
            if !valid {
                break;
            }
            let now_ms = crate::unix_time_ms();
            if preferred.not_before <= now {
                let selected = self.admit(request.clone(), now_ms).await;
                if selected.is_some() {
                    request.budget.finish_candidate_retry();
                    return Ok(selected);
                }
                if !request.budget.can_dispatch() {
                    return Err(());
                }
            }
            let now = Instant::now();
            let now_ms = crate::unix_time_ms();
            let retry_at = self.recovery_retry_at(
                &request.key,
                &request.model,
                &request.protocols,
                &request.tried,
                request.response_affinity.as_deref(),
                now_ms,
                request.operation,
            );
            if retry_at.is_none() && preferred.not_before <= now {
                break;
            }
            let due = preferred.not_before.max(
                now + std::time::Duration::from_millis(
                    retry_at.unwrap_or(now_ms).saturating_sub(now_ms),
                ),
            );
            if due <= now {
                break;
            }
            let queue_deadline = self.admission_deadline(&request.budget).map_err(|_| ())?;
            let deadline = queue_deadline
                .into_iter()
                .chain(request.budget.rate_limit_deadline())
                .fold(preferred.deadline, Instant::min);
            if now >= deadline {
                break;
            }
            let waiter = {
                let mut queue = crate::poison::mutex(&self.admission);
                self.register_waiter(
                    &mut queue,
                    &request,
                    RequestWaitKind::Recovery {
                        delay_ms: due.saturating_duration_since(now).as_millis() as u64,
                    },
                )
                .ok_or(())?
            };
            tokio::select! {
                _ = notified.as_mut() => {},
                _ = sleep_until(due.min(deadline)) => {},
            }
            drop(waiter);
        }
        request
            .budget
            .abandon_candidate_retry(preferred.allow_fallback);
        if preferred.allow_fallback {
            Ok(None)
        } else {
            Err(())
        }
    }
}
