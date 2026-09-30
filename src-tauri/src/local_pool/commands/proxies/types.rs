use crate::local_pool::accounts::proxy::ProxyPoolSummary;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportProxyPoolInput {
    pub(super) proxy_urls: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignStoredProxyInput {
    pub(super) account_id: String,
    pub(super) proxy_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetStoredProxyAccountsInput {
    pub(super) proxy_id: String,
    pub(super) account_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteStoredProxiesInput {
    pub(super) proxy_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignFreeProxiesInput {
    pub(super) account_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPoolImportResult {
    pub added: usize,
    pub duplicates: usize,
    pub added_proxy_ids: Vec<String>,
    pub pool: ProxyPoolSummary,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProxyAssignmentResult {
    pub assigned: usize,
    pub unchanged: usize,
    pub unavailable: usize,
    pub pool: ProxyPoolSummary,
}

pub(super) enum ProxyChoice {
    Inherited,
    Direct,
    Automatic,
    Stored(String),
    Custom(String),
}
