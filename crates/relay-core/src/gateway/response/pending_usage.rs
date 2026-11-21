use super::apply_usage;
use crate::gateway::streaming::UpstreamUsage;
use crate::{GatewayRuntime, UsageEvent};

pub(in crate::gateway) struct PendingUsageObservation<'a> {
    runtime: &'a GatewayRuntime,
    request_id: String,
    attempt: u16,
    capture: Box<UpstreamUsage>,
    revision: u64,
}

#[cfg(test)]
mod tests;

impl<'a> PendingUsageObservation<'a> {
    pub(in crate::gateway) fn new(runtime: &'a GatewayRuntime, event: UsageEvent) -> Self {
        Self {
            runtime,
            request_id: event.request_id.clone(),
            attempt: event.attempt,
            // Buffered collectors nest inside request and WebSocket fallback futures.
            // Keep the full native usage snapshot out of every parent's inline state.
            capture: Box::new(UpstreamUsage::new(event, None)),
            revision: 0,
        }
    }

    pub(in crate::gateway) fn observe(&mut self, bytes: &[u8]) {
        let _ = self.capture.observe(bytes);
        if let Some((revision, usage)) = self.capture.reported_usage()
            && revision != self.revision
        {
            self.runtime
                .observe_pending_dispatch(&self.request_id, self.attempt, |event| {
                    apply_usage(event, usage);
                });
            self.revision = revision;
        }
    }
}
