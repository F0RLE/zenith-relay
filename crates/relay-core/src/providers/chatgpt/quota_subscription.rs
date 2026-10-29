mod client;
mod metadata;
mod parse;

pub use client::{
    CODEX_ACCOUNTS_CHECK_ENDPOINT, CODEX_SUBSCRIPTIONS_ENDPOINT, CodexSubscriptionClient,
};
pub use metadata::{
    CodexSubscriptionMetadata, SUBSCRIPTION_REFRESH_INTERVAL_MS, merge_subscription_metadata,
    merge_subscription_metadata_at, subscription_refresh_due,
};
pub use parse::{
    AccountCheckIdentityError, account_ids_from_check_response,
    parse_accounts_check as parse_account_check_subscription, parse_subscription_timestamp_ms,
    parse_subscription_timestamp_text, resolve_account_check_account_id,
    unverified_chatgpt_account_id_hints,
};

#[cfg(test)]
use client::CHATGPT_WEB_USER_AGENT;
#[cfg(test)]
use parse::parse_accounts_check;

use crate::quota::QuotaRefreshFailure;

const MAX_ACCOUNT_ID_BYTES: usize = 512;

fn failure(code: &str, retryable: bool) -> QuotaRefreshFailure {
    QuotaRefreshFailure::new(code, retryable)
}

#[cfg(test)]
mod tests;
