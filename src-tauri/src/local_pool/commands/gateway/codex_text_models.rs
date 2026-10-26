use super::super::{fence_runtime_candidates, restart_or_rollback};
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    state::DesktopState,
};
use serde::Deserialize;
use tauri::State;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetCodexTextModelsInput {
    enabled: bool,
    expected_enabled: bool,
    risk_accepted: bool,
}

#[tauri::command]
pub async fn set_local_codex_text_models(
    input: SetCodexTextModelsInput,
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    apply_codex_text_models(input, state.inner()).await
}

pub(in crate::local_pool::commands) async fn apply_codex_text_models(
    input: SetCodexTextModelsInput,
    state: &DesktopState,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    let old_gateway = state.store()?.gateway().clone();
    validate_change(&input, old_gateway.codex_text_models_enabled)?;
    if old_gateway.codex_text_models_enabled == input.enabled {
        return Ok(());
    }
    // Keep pending old-runtime dispatches fenced until the replacement or
    // rollback is published. No credentials, membership or model rules change.
    let account_ids = state
        .store()?
        .accounts()
        .iter()
        .map(|account| account.account.id.clone())
        .collect::<Vec<_>>();
    let runtime = state.gateway.runtime().await;
    let _fences = fence_runtime_candidates(runtime.as_deref(), &account_ids, &[]);
    let mut gateway = old_gateway.clone();
    gateway.codex_text_models_enabled = input.enabled;
    state.store()?.replace_gateway(gateway)?;
    restart_or_rollback(state, || state.store()?.replace_gateway(old_gateway)).await?;
    Ok(())
}

fn validate_change(input: &SetCodexTextModelsInput, current: bool) -> Result<(), LocalPoolError> {
    if input.expected_enabled != current {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "Codex text-model access changed. Refresh settings and try again.",
        ));
    }
    if input.enabled && !input.risk_accepted {
        return Err(LocalPoolError::invalid_state(
            "Confirm the risks before enabling Codex text models in the pool.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_text_opt_in_requires_consent_and_current_state() {
        for current in [false, true] {
            for enabled in [false, true] {
                for risk_accepted in [false, true] {
                    let mut input = SetCodexTextModelsInput {
                        enabled,
                        expected_enabled: current,
                        risk_accepted,
                    };
                    assert_eq!(
                        validate_change(&input, current).is_ok(),
                        !enabled || risk_accepted
                    );
                    input.expected_enabled = !current;
                    assert!(validate_change(&input, current).is_err());
                }
            }
        }
    }

    #[test]
    fn codex_text_opt_in_ipc_requires_explicit_fields() {
        let valid = serde_json::json!({
            "enabled": true, "expectedEnabled": false, "riskAccepted": true
        });
        assert!(serde_json::from_value::<SetCodexTextModelsInput>(valid.clone()).is_ok());
        for field in ["enabled", "expectedEnabled", "riskAccepted"] {
            let mut incomplete = valid.clone();
            incomplete.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<SetCodexTextModelsInput>(incomplete).is_err());
        }
        let mut unknown = valid;
        unknown["addAllAccounts"] = serde_json::json!(true);
        assert!(serde_json::from_value::<SetCodexTextModelsInput>(unknown).is_err());
    }

    #[test]
    fn codex_text_opt_in_defaults_off_and_round_trips_without_relaxing_other_policy() {
        use crate::local_pool::models::GatewaySettings;
        let settings = GatewaySettings::default();
        assert!(!settings.codex_text_models_enabled);
        let mut legacy = serde_json::to_value(&settings).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("codexTextModelsEnabled");
        let mut restored: GatewaySettings = serde_json::from_value(legacy).unwrap();
        assert!(!restored.codex_text_models_enabled);
        restored.codex_text_models_enabled = true;
        let restored: GatewaySettings =
            serde_json::from_value(serde_json::to_value(restored).unwrap()).unwrap();
        assert!(restored.codex_text_models_enabled);
        assert!(restored.block_degraded_routes_enabled);
        assert!(restored.account_routes.is_empty());
    }
}
