use super::super::sqlite::Store;
use super::pricing::{test_pricing_catalog, test_pricing_context};
use super::retention::DAY_MS;
use crate::state::identity_hint;
use crate::state::ServerAccountRecord;
use crate::state::SourceRecord;
use crate::store::test_support::test_root;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use zenith_relay_core::accounts::AccountAuthState;
use zenith_relay_core::accounts::AccountHealthState;
use zenith_relay_core::protocol::UsageQuery;
use zenith_relay_core::quota::QuotaSnapshot;
use zenith_relay_core::quota::Subscription;
use zenith_relay_core::ResponseAffinityBinding;
use zenith_relay_core::ResponseAffinityStore;
use zenith_relay_core::RoutingDiagnostics;
use zenith_relay_core::SelectionReason;
use zenith_relay_core::{
    ApiEquivalentSummary, ApiModelPriceOverride, DefaultServiceTier, ToolUseDiagnostics,
    UsageEvent, WireApi,
};
use zenith_relay_core::{ErrorOrigin, TerminalOutputKind, ToolChoiceMode};

mod archive_prices;
mod terminal_rows;
