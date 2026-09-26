//! Forward-only gateway settings upgrade on ordinary desktop startup.
//! Old scalar hints are read for compatibility, never used by pool rotation.

use super::{serialize_state, TelemetryDb, STATE_GATEWAY};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::{GatewaySettings, LocalAccountRecord, ProviderSourceRecord},
};
use zenith_relay_core::PoolRoutingPolicy;

pub(super) fn upgrade_saved_gateway(
    database: &TelemetryDb,
    gateway: &mut GatewaySettings,
    sources: &[ProviderSourceRecord],
    accounts: &[LocalAccountRecord],
) -> Result<()> {
    // A saved current policy can still coexist with obsolete scalar fields from an
    // older build. Remove them on disk without changing any active controls.
    let remove_v1_scalars = database
        .state_json(STATE_GATEWAY)?
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .is_some_and(|value| {
            value.as_object().is_some_and(|fields| {
                [
                    "cooldownAfterFailures",
                    "keepLastCandidateAvailable",
                    "routingStrategy",
                    "subscriptionPlanOrder",
                ]
                .iter()
                .any(|key| fields.contains_key(*key))
            })
        });
    let upgrade_policy = !gateway
        .pool_routing
        .as_ref()
        .is_some_and(PoolRoutingPolicy::is_current_rotation);
    if upgrade_policy {
        let policy = gateway.pool_routing_for(sources, accounts);
        policy
            .validate_activation()
            .map_err(|message| LocalPoolError::new(ErrorCode::RecoveryRequired, message))?;
        gateway.pool_routing = Some(policy);
    }
    if upgrade_policy || remove_v1_scalars {
        database.replace_state_json(&[(STATE_GATEWAY, serialize_state(gateway)?)])?;
    }
    Ok(())
}
