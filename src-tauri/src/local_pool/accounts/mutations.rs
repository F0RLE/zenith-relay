use serde::Deserialize;

pub(crate) mod delete;
pub(crate) mod edit;

#[cfg(test)]
pub(in crate::local_pool::accounts) use delete::{
    ensure_accounts_exist, prune_account_task_selectors, restore_bound_account_profiles,
    rollback_deleted_account_side_effects,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateAccountInput {
    pub(super) account_id: String,
    #[serde(default)]
    pub(super) label: Option<String>,
    #[serde(default)]
    pub(super) priority: Option<i32>,
    #[serde(default)]
    pub(super) weight: Option<u32>,
    #[serde(default)]
    pub(super) allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub(super) excluded_models: Option<Vec<String>>,
    #[serde(default)]
    pub(super) in_pool: Option<bool>,
    #[serde(default)]
    pub(super) draining: Option<bool>,
    #[serde(default)]
    pub(super) purchase_cost_micro_usd: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetAccountProxyInput {
    pub(super) account_id: String,
    pub(super) proxy_url: Option<String>,
    #[serde(default)]
    pub(super) bypass_common_proxy: bool,
}
