use super::*;

/// Give short overlapping turns a chance to reuse their completed session,
/// without making a long-running turn indefinitely block another request.
pub(super) const SESSION_WAIT_TIMEOUT: Duration = Duration::from_secs(2);

impl SocketStore {
    pub(super) async fn acquire_or_isolate(
        &self,
        key: Option<planner::Fingerprint>,
    ) -> (OwnedMutexGuard<Lane>, u64) {
        let Some(key) = key else {
            return (Self::isolated(ConnectionResetReason::MissingContext), 0);
        };
        let Some(lane) = self.lane(key) else {
            return (
                Self::isolated(ConnectionResetReason::SessionCapacityExceeded),
                0,
            );
        };
        if let Ok(owner) = Arc::clone(&lane).try_lock_owned() {
            return (owner, 0);
        }
        let started = tokio::time::Instant::now();
        let owner = tokio::time::timeout(SESSION_WAIT_TIMEOUT, lane.lock_owned()).await;
        let waited_ms = started.elapsed().as_millis() as u64;
        (
            owner.unwrap_or_else(|_| Self::isolated(ConnectionResetReason::SessionWaitTimeout)),
            waited_ms,
        )
    }

    fn isolated(reason: ConnectionResetReason) -> OwnedMutexGuard<Lane> {
        let mut lane = Lane::new();
        lane.reset_reason = Some(reason);
        Arc::new(AsyncMutex::new(lane))
            .try_lock_owned()
            .expect("new lane")
    }

    fn lane(&self, key: planner::Fingerprint) -> Option<Arc<AsyncMutex<Lane>>> {
        let mut lanes = crate::poison::mutex(&self.lanes);
        let expired = lanes.get(&key).is_some_and(|lane| {
            Arc::strong_count(lane) == 1
                && lane
                    .try_lock()
                    .is_ok_and(|lane| lane.touched.elapsed() >= IDLE_TTL)
        });
        lanes.retain(|_, lane| {
            Arc::strong_count(lane) > 1
                || lane
                    .try_lock()
                    .map_or(true, |lane| lane.touched.elapsed() < IDLE_TTL)
        });
        if !lanes.contains_key(&key) && lanes.len() >= MAX_SESSIONS {
            // Owners and queued borrowers pin their lane, even between turns.
            let oldest = lanes
                .iter()
                .filter(|(_, lane)| Arc::strong_count(lane) == 1)
                .filter_map(|(key, lane)| lane.try_lock().ok().map(|lane| (*key, lane.touched)))
                .min_by_key(|(_, touched)| *touched)
                .map(|(key, _)| key)?;
            lanes.remove(&oldest);
        }
        Some(Arc::clone(lanes.entry(key).or_insert_with(|| {
            let mut lane = Lane::new();
            if expired {
                lane.reset_reason = Some(ConnectionResetReason::SessionExpired);
            }
            Arc::new(AsyncMutex::new(lane))
        })))
    }

    #[cfg(test)]
    pub(super) fn acquire(&self, key: planner::Fingerprint) -> Option<OwnedMutexGuard<Lane>> {
        self.lane(key)?.try_lock_owned().ok()
    }
}
