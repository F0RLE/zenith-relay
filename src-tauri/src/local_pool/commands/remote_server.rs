use crate::local_pool::remote::RemoteTargetRecord;
use serde::{Deserialize, Serialize};
use zenith_relay_core::protocol::{Capabilities, HealthResponse};

pub(super) use zenith_relay_core::unix_time_ms as now_ms;

pub(crate) mod accounts;
pub(crate) mod actions;
pub(crate) mod gateway_key;
mod ownership;
pub(crate) mod preset;
pub(crate) mod session;

use actions::object_path;
pub(crate) use ownership::{reconcile_saved_remote_ownership, recover_pending_remote_ownership};
pub use ownership::{
    ForceActivateRemoteAccountLocallyInput, ForceActivateRemoteAccountLocallyResult,
    MoveLocalAccountsToRemoteInput, MoveLocalAccountsToRemoteResult,
    ReturnRemoteAccountToLocalInput, ReturnRemoteAccountToLocalResult,
};
pub(in crate::local_pool::commands) use session::{active_client, remote_error};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectRemoteServerInput {
    pub base_url: String,
    pub management_token: String,
    #[serde(default)]
    pub allow_insecure_http: bool,
    #[serde(default)]
    pub confirm_identity_change: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConnectionState {
    pub target: RemoteTargetRecord,
    pub health: HealthResponse,
    pub capabilities: Capabilities,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareRemoteDeploymentInput {
    pub public_base_url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RemoteServerAction {
    CreateSource,
    UpdateSource { id: String },
    DeleteSource { id: String },
    TestSource { id: String },
    ProbeSource { id: String },
    PreviewAccountImport,
    ConfirmAccountImport,
    PreviewAccountBatchImport,
    ConfirmAccountBatchImport,
    UpdateAccount { id: String },
    RefreshAccount { id: String },
    DeleteAccount { id: String },
    SetCommonProxy,
    SetAccountProxyRequired,
    SetAccountProxy { id: String },
    AssignAccountProxies,
    SetPoolMembership,
    SetQuotaPolicy,
    SetRoutingPolicy,
    RefreshAllQuotas,
    RefreshPricingCatalog,
    SetModelEnabled,
    SetModelPrice,
    SetModelReasoning,
    SetModelServiceTier,
    SetModelOrder,
    StartGateway,
    StopGateway,
    SetCodexBackgroundTasks,
    SetToolPolicy,
    SetChatgptRetryUntilAvailable,
    SetBlockDegradedRoutes,
    SetCodexWebsockets,
    CreateWakeTask,
    UpdateWakeTask { id: String },
    DeleteWakeTask { id: String },
    TestWakeTask { id: String },
    ClearUsage,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteRemoteServerActionInput {
    pub action: RemoteServerAction,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}
