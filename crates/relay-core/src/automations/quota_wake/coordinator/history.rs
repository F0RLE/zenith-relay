use super::super::{
    is_safe_id, WakeCycle, WakeCycleStatus, WakeHistory, WakeOutcome, MAX_WAKE_ATTEMPTS,
    MAX_WAKE_JITTER_SECONDS,
};
use crate::accounts::AccountRecord;
use crate::error::safe_error_code;
use crate::quota::{QuotaTransition, QuotaWindowKind};
use sha2::{Digest, Sha256};

pub(super) fn valid_cycle(cycle: &WakeCycle) -> bool {
    !cycle.key.is_empty()
        && is_safe_id(&cycle.task_id)
        && !cycle.account_id.trim().is_empty()
        && !cycle.transition_fingerprint.trim().is_empty()
        && (1..=MAX_WAKE_ATTEMPTS).contains(&cycle.max_attempts)
        && cycle.attempts_started <= cycle.max_attempts
        && cycle.jitter_seconds <= MAX_WAKE_JITTER_SECONDS
        && (cycle.status == WakeCycleStatus::Completed
            || (cycle.request.is_some() && cycle.verification.is_some()))
}

pub(super) fn natural_use_history(cycle: &WakeCycle, used_at_ms: u64) -> WakeHistory {
    history(
        cycle,
        WakeOutcome::SkippedAlreadyStarted,
        used_at_ms,
        used_at_ms,
        None,
    )
}

pub(super) fn canceled_history(
    cycle: &WakeCycle,
    completed_at_ms: u64,
    error_code: &str,
) -> WakeHistory {
    history(
        cycle,
        WakeOutcome::SkippedIneligible,
        cycle.recorded_at_ms,
        completed_at_ms.max(cycle.recorded_at_ms),
        Some(safe_error_code(error_code)),
    )
}

fn history(
    cycle: &WakeCycle,
    outcome: WakeOutcome,
    started_at_ms: u64,
    completed_at_ms: u64,
    error_code: Option<String>,
) -> WakeHistory {
    WakeHistory {
        task_id: cycle.task_id.clone(),
        account_id: cycle.account_id.clone(),
        window_kind: cycle.window_kind,
        transition_fingerprint: cycle.transition_fingerprint.clone(),
        model_id: None,
        trigger: cycle.trigger,
        attempt: cycle.attempts_started,
        outcome,
        started_at_ms,
        completed_at_ms,
        latency_ms: None,
        input_tokens: None,
        output_tokens: None,
        error_code,
    }
}

pub(super) fn cycle_key(account: &AccountRecord, transition: &QuotaTransition) -> String {
    hex::encode(Sha256::digest(
        format!(
            "{}\0{:?}\0{}",
            account.id, transition.window_kind, transition.fingerprint,
        )
        .as_bytes(),
    ))
}

pub(super) fn same_cycle(
    cycle: &WakeCycle,
    account_id: &str,
    window_kind: QuotaWindowKind,
    transition_fingerprint: &str,
) -> bool {
    cycle.account_id == account_id
        && cycle.window_kind == window_kind
        && cycle.transition_fingerprint == transition_fingerprint
}

pub(super) fn deterministic_jitter_ms(cycle_key: &str, attempt: u8, jitter_seconds: u32) -> u64 {
    if jitter_seconds == 0 {
        return 0;
    }
    let digest = Sha256::digest(format!("{cycle_key}\0{attempt}").as_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    let seconds = u64::from_le_bytes(bytes) % (u64::from(jitter_seconds) + 1);
    seconds.saturating_mul(1_000)
}
