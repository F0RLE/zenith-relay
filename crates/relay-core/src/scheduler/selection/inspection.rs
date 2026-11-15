use super::super::rotation::{
    CandidateBlockReason, RotationConfigurationBlock, RotationDecisionTrace,
};
use super::*;

/// Hosts opt in before capture and enqueue without blocking admission.
pub trait RotationDebugSink: Send + Sync {
    fn epoch(&self) -> Option<u64>;
    fn submit(&self, epoch: u64, capture: RotationDebugCapture);
}

/// Exact pre-reservation evidence, containing no prompts or credentials.
pub struct RotationDebugCapture {
    engine: RotationEngine,
    request: RotationRequest,
    blocks: BTreeMap<String, RotationConfigurationBlock>,
    observed_at_ms: u64,
    pub preparation_blocked: bool,
    pub reserved: bool,
}

impl RotationDebugCapture {
    pub fn request_id(&self) -> u64 {
        self.request.request_id.0
    }
    pub fn model(&self) -> &str {
        &self.request.model
    }
    pub fn operation(&self) -> RotationOperation {
        self.request.operation
    }

    pub(crate) fn set_request_id(&mut self, id: RotationRequestId) {
        self.request.request_id = id;
    }

    /// Run the bounded fallback walk outside the live scheduler lock.
    pub fn into_trace(self) -> RotationDecisionTrace {
        let mut trace = self
            .engine
            .decision_trace(&self.request, self.observed_at_ms);
        for candidate in &mut trace.candidates {
            if candidate.block_reason == Some(CandidateBlockReason::NotAllowed) {
                candidate.configuration_block = self.blocks.get(&candidate.candidate_id).copied();
            }
        }
        trace
    }
}

impl PoolScheduler {
    pub(super) fn capture_rotation(
        &self,
        request: &SelectionRequest<'_>,
        prepared: Option<&RotationRequest>,
        lane: InFlightLane,
        operation: RotationOperation,
    ) -> RotationDebugCapture {
        let mut rotation = prepared.cloned().unwrap_or_else(|| {
            self.rotation_request(
                Some(RotationRequestId(0)),
                None,
                request.model,
                operation,
                BTreeSet::new(),
            )
        });
        rotation.tried.extend(request.tried.iter().cloned());
        if prepared.is_none() {
            rotation.owner = request.response_affinity_key.and_then(|key| {
                self.response_affinity
                    .get_with_expiry(key, request.now_ms)
                    .map(|(owner, _)| owner.to_owned())
            });
        }
        let blocks = self
            .candidates
            .values()
            .filter_map(|candidate| {
                let block = self
                    .rotation_configuration_block(
                        candidate,
                        request.model,
                        request.allowed_protocols,
                        request.scope,
                        request.now_ms,
                    )
                    .or_else(|| {
                        (!self.lane_allows(candidate, lane))
                            .then_some(RotationConfigurationBlock::LaneCapacity)
                    })
                    .or_else(|| {
                        prepared
                            .is_none()
                            .then_some(RotationConfigurationBlock::RequestPreparation)
                    })
                    .or_else(|| {
                        rotation
                            .allowed_candidates
                            .as_ref()
                            .is_some_and(|allowed| !allowed.contains(&candidate.id))
                            .then_some(RotationConfigurationBlock::PhysicalAlias)
                    });
                block.map(|block| (candidate.id.clone(), block))
            })
            .collect();
        RotationDebugCapture {
            engine: self.rotation.clone(),
            request: rotation,
            blocks,
            observed_at_ms: request.now_ms,
            preparation_blocked: prepared.is_none(),
            reserved: false,
        }
    }
}
