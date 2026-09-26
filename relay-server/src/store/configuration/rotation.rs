//! Idempotent startup upgrade before any listener/runtime is created.

use super::*;

impl Store {
    pub(crate) fn upgrade_rotation_policy(&self) -> Result<(), String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let settings = configuration_settings_from_connection(&transaction)?;
        if settings
            .routing
            .pool_routing
            .as_ref()
            .is_some_and(zenith_relay_core::PoolRoutingPolicy::is_current_rotation)
        {
            return Ok(());
        }
        let policy = settings.resolved_pool_routing();
        policy.validate_activation().map_err(str::to_string)?;
        transaction.execute("INSERT INTO metadata(key, value) VALUES ('pool_routing', ?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [to_json(&policy)?]).map_err(db_error)?;
        // Never toggle gateway_enabled, permissions or user retry controls.
        transaction.commit().map_err(db_error)
    }
}
