pub(crate) mod connections;
pub(crate) mod gateway;
pub(crate) mod profiles;
pub(crate) mod state;
pub(crate) mod usage;

use super::{
    error::{ErrorCode, LocalPoolError, Result},
    state::DesktopState,
    store::secret_store,
};
use std::sync::Arc;
use zenith_relay_core::{GatewayRuntime, LocalGatewayKey, ProviderSource};

fn runtime_from_store(state: &DesktopState) -> Result<Arc<GatewayRuntime>> {
    let (source, key) = {
        let store = state.store()?;
        let source = store
            .sources()
            .iter()
            .find(|source| source.enabled)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "no enabled source"))?;
        let key = store
            .keys()
            .iter()
            .find(|key| key.enabled)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "no enabled local key"))?;
        (source, key)
    };
    let api_key = secret_store::load(&source.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    let local_key = secret_store::load(&key.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local key secret is missing"))?;
    GatewayRuntime::new(
        ProviderSource {
            id: source.id,
            name: source.name,
            base_url: source.base_url,
            api_key,
            wire_api: source.wire_api,
            models: source.models,
        },
        LocalGatewayKey {
            id: key.id,
            secret: local_key,
        },
        state.usage_callback(),
    )
    .map(Arc::new)
    .map_err(core_error)
}

fn core_error(error: zenith_relay_core::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::InvalidState, error.to_string())
}
