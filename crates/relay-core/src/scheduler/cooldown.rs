use std::collections::BTreeMap;

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
