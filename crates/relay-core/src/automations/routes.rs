use super::{AccountSelector, WakeModelPolicy, WakeTask};
use crate::ModelRules;
use crate::providers::chatgpt::OAuthClientKind;

pub fn select_automation_route(
    tasks: &mut [WakeTask],
    route_ids: &[String],
    destination_id: &str,
    destination_models: &[String],
    destination_rules: &ModelRules,
    next_kind: OAuthClientKind,
    now_ms: u64,
) -> Result<(), &'static str> {
    let affected = |task: &WakeTask| match &task.account_selector {
        AccountSelector::AccountIds(ids) => ids
            .iter()
            .any(|id| id != destination_id && route_ids.contains(id)),
        AccountSelector::AllEligible | AccountSelector::Tags(_) => false,
    };
    // Validate the full batch before mutating any task.
    for task in tasks.iter().filter(|task| task.enabled && affected(task)) {
        if next_kind == OAuthClientKind::Codex {
            return Err(
                "Codex is not a pool connection; edit or disable this account's automations before switching",
            );
        }
        if let WakeModelPolicy::Explicit(model) = &task.model_policy
            && (!destination_models
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(model))
                || !destination_rules.allows(model))
        {
            return Err(
                "The selected route does not support an enabled automation model; edit or disable the automation first",
            );
        }
    }
    for task in tasks.iter_mut().filter(|task| affected(task)) {
        if let AccountSelector::AccountIds(ids) = &mut task.account_selector {
            ids.retain(|id| !route_ids.contains(id));
            ids.insert(destination_id.to_owned());
            task.updated_at_ms = now_ms;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
