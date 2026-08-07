use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CooldownReason {
    Transient,
    RateLimit,
    Mandatory,
}

pub(super) fn active_retry_at(
    cooldowns: &BTreeMap<String, u64>,
    model: &str,
    now_ms: u64,
) -> Option<u64> {
    cooldowns
        .iter()
        .filter(|(candidate_model, retry_at)| {
            (**retry_at > now_ms)
                && (candidate_model.as_str() == "*" || candidate_model.eq_ignore_ascii_case(model))
        })
        .map(|(_, retry_at)| *retry_at)
        .max()
}

pub(super) fn has_expired_cooldown(
    cooldowns: &BTreeMap<String, u64>,
    model: &str,
    now_ms: u64,
) -> bool {
    cooldowns.iter().any(|(candidate_model, retry_at)| {
        *retry_at <= now_ms
            && (candidate_model == "*" || candidate_model.eq_ignore_ascii_case(model))
    })
}
