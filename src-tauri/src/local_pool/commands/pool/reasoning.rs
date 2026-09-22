use crate::local_pool::{
    error::{CommandError, LocalPoolError},
    models::LocalPoolSnapshot,
    state::DesktopState,
};
use serde::Deserialize;
use tauri::State;
use zenith_relay_core::protocol::update_model_reasoning_policy;

type CommandResult<T> = std::result::Result<T, CommandError>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelReasoningInput {
    pub(super) model_id: String,
    #[serde(default)]
    pub(super) allowed_levels: Vec<String>,
}

pub(super) async fn set_local_model_reasoning(
    input: SetModelReasoningInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let canonical = super::canonical_pool_model(&state, &input.model_id)?;
    apply_local_model_reasoning(&state, canonical, input.allowed_levels).await
}

async fn apply_local_model_reasoning(
    state: &DesktopState,
    canonical: String,
    requested_levels: Vec<String>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let old_gateway = state.store()?.gateway().clone();
    let mut gateway = old_gateway.clone();
    update_model_reasoning_policy(
        &mut gateway.model_reasoning_allowed_levels,
        &canonical,
        requested_levels,
    )
    .map_err(LocalPoolError::invalid_state)?;
    if gateway == old_gateway {
        return state.snapshot().await.map_err(Into::into);
    }
    state.store()?.replace_gateway(gateway.clone())?;
    if let Some(runtime) = state.gateway.runtime().await {
        if let Err(error) =
            runtime.set_model_reasoning_allowed_levels(gateway.model_reasoning_allowed_levels)
        {
            state.store()?.replace_gateway(old_gateway)?;
            return Err(LocalPoolError::invalid_state(error).into());
        }
    }
    let snapshot = state.snapshot().await?;
    drop(_mutation);
    Ok(snapshot)
}
