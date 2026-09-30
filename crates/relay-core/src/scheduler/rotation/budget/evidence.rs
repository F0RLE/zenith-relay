/// The four independent proofs required before a non-trivial operation may be
/// sent to another route.  A repeatable body alone is deliberately not enough.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryEvidence {
    pub input_repeatable: bool,
    pub execution: ExecutionEvidence,
    pub target_portable: bool,
    pub idempotency: IdempotencyContract,
}

impl RetryEvidence {
    pub const fn pre_execution() -> Self {
        Self {
            input_repeatable: true,
            execution: ExecutionEvidence::RejectedBeforeExecution,
            target_portable: true,
            idempotency: IdempotencyContract::None,
        }
    }

    pub const fn unknown() -> Self {
        Self {
            input_repeatable: false,
            execution: ExecutionEvidence::Unknown,
            target_portable: false,
            idempotency: IdempotencyContract::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionEvidence {
    NotSent,
    RejectedBeforeExecution,
    Accepted,
    Unknown,
    Terminal,
}

impl ExecutionEvidence {
    pub(super) const fn replay_allowed(self, idempotency: IdempotencyContract) -> bool {
        matches!(self, Self::NotSent | Self::RejectedBeforeExecution)
            || (matches!(self, Self::Accepted)
                && matches!(idempotency, IdempotencyContract::Proven))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdempotencyContract {
    None,
    /// The adapter has verified deduplication for this exact operation,
    /// identity, endpoint and retention window.
    Proven,
}
