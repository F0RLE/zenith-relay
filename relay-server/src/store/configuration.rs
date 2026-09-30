use super::sqlite::{db_error, to_json, Store};
use document::configuration_settings_from_connection;
use rusqlite::TransactionBehavior;

mod document;
mod settings;
mod validation;

#[cfg(test)]
mod tests;

pub use document::configuration_revision;

use zenith_relay_core::protocol::ConfigurationPresetSettings;

pub use zenith_relay_core::protocol::{
    DEFAULT_MAX_RETRY_CANDIDATES, DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS,
};

#[cfg(test)]
pub(super) type SourcePriceOverrides = zenith_relay_core::SourceModelPriceOverrides;

#[derive(Debug)]
pub enum ConfigurationReplaceError {
    Stale { current_revision: String },
    Invalid(String),
    Store(String),
}

pub struct ConfigurationReplacement {
    pub previous: ConfigurationPresetSettings,
    pub previous_revision: String,
    pub revision: String,
}

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
