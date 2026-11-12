use super::{
    CachedModelManifest, ChatGptAccountExecutor, PassiveQuotaState, RuntimeTeamBreakerCallback,
    SourceCandidateBinding, catalog_hints, clients,
};
use crate::SourceConnector;
use crate::model_metadata::ModelMetadataCatalogHandle;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// Configured source graph and its existing per-source clients. Policy commits
/// acquire the scheduler before recovery_delays_ms; readers copy a delay before
/// entering the scheduler. Connection changes still replace the routing graph.
pub(super) struct RuntimeSourceState {
    pub(super) discovery_client: reqwest::Client,
    pub(super) connectors: BTreeMap<String, SourceConnector>,
    /// Proxy-specific entries; other candidates use the shared runtime clients.
    pub(super) http_clients: BTreeMap<String, reqwest::Client>,
    pub(super) websocket_clients: BTreeMap<String, reqwest::Client>,
    pub(super) candidate_bindings: BTreeMap<String, SourceCandidateBinding>,
    pub(super) recovery_delays_ms: Mutex<BTreeMap<String, u64>>,
}

/// Account executors, principal groups and passive observations share the graph
/// lifetime. Quota observations, breaker suppression and callback registration
/// retain their independent locks instead of an aggregate account-state lock.
pub(super) struct RuntimeAccountState {
    pub(super) executors: BTreeMap<String, ChatGptAccountExecutor>,
    pub(super) team_members: BTreeMap<String, BTreeSet<String>>,
    pub(super) team_breaker_recent: Mutex<BTreeMap<String, u64>>,
    pub(super) team_breaker_callback: Arc<Mutex<RuntimeTeamBreakerCallback>>,
    pub(super) passive_quotas: Mutex<BTreeMap<String, PassiveQuotaState>>,
}

/// Provider evidence is separate from editable model policy and conversation
/// ownership. Each existing cache retains its own lock, limits and invalidation;
/// catalog-hint callbacks are copied out before they run.
pub(super) struct RuntimeProviderMetadata {
    pub(super) reference_catalog: Option<ModelMetadataCatalogHandle>,
    pub(super) codex_manifests: Mutex<BTreeMap<String, CachedModelManifest>>,
    pub(super) official_codex_ultra: Mutex<BTreeMap<String, Value>>,
    pub(super) websocket_transport_observations:
        Mutex<BTreeMap<(String, String), clients::WebsocketTransportObservation>>,
    pub(super) catalog_hints: Mutex<BTreeMap<String, catalog_hints::CatalogHint>>,
    pub(super) catalog_hint_callback: Mutex<catalog_hints::CatalogHintCallback>,
}

impl RuntimeProviderMetadata {
    pub(super) fn new(reference_catalog: Option<ModelMetadataCatalogHandle>) -> Self {
        Self {
            reference_catalog,
            codex_manifests: Mutex::new(BTreeMap::new()),
            official_codex_ultra: Mutex::new(BTreeMap::new()),
            websocket_transport_observations: Mutex::new(BTreeMap::new()),
            catalog_hints: Mutex::new(BTreeMap::new()),
            catalog_hint_callback: Mutex::new(Arc::new(|_, _| false)),
        }
    }
}
