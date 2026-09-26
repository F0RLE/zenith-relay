//! Forward-only storage compatibility for the normal 1.1.3 application upgrade.
//! This does not expose a second scheduler, consent workflow or downgrade path.

use super::{PoolRoutingMode, PoolRoutingPolicy};

impl PoolRoutingPolicy {
    /// Only the known version-one mode/version pair changes. Inventory order,
    /// weights, concurrency and all unrelated settings remain untouched.
    /// Validation remains mandatory: unknown versions and corrupt values are
    /// never silently clamped or treated as a supported legacy policy.
    pub(crate) fn upgrade_legacy_format(&mut self) {
        if self.version == 1 && self.mode != PoolRoutingMode::Automatic {
            self.version = 2;
            if self.mode == PoolRoutingMode::Smart {
                self.mode = PoolRoutingMode::Automatic;
            }
        }
    }
}

#[cfg(test)]
mod tests;
