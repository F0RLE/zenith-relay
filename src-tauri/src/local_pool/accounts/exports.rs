use crate::local_pool::{
    commands::recovery::write_account_export,
    error::{CommandError, ErrorCode, LocalPoolError},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tauri::AppHandle;
use zenith_relay_core::accounts::{
    AccountExportDocument, AccountExportFormat, MAX_ACCOUNT_EXPORT_ITEMS,
};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountExportDestination {
    Copy,
    Download,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountExportInput {
    pub account_ids: Vec<String>,
    pub format: AccountExportFormat,
    pub destination: AccountExportDestination,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountExportResult {
    pub format: AccountExportFormat,
    pub account_count: usize,
    pub file_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

pub fn finish_account_export(
    document: AccountExportDocument,
    destination: AccountExportDestination,
    app: &AppHandle,
) -> Result<AccountExportResult, CommandError> {
    document
        .validate()
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
    let (content, path) = match destination {
        AccountExportDestination::Copy => (Some(document.content.clone()), None),
        AccountExportDestination::Download => (None, write_account_export(&document, app)?),
    };
    Ok(AccountExportResult {
        format: document.format,
        account_count: document.account_count,
        file_name: document.file_name,
        content,
        path,
    })
}

pub fn normalize_account_ids(account_ids: Vec<String>) -> Result<Vec<String>, CommandError> {
    if account_ids.is_empty() || account_ids.len() > MAX_ACCOUNT_EXPORT_ITEMS {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "account export selection is invalid",
        )
        .into());
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(account_ids.len());
    for account_id in account_ids {
        let account_id = account_id.trim();
        if account_id.is_empty()
            || account_id.len() > 128
            || !account_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || !seen.insert(account_id.to_string())
        {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "account export selection is invalid",
            )
            .into());
        }
        normalized.push(account_id.to_string());
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::accounts::imports::parse_import;
    use zenith_relay_core::accounts::{build_account_export, AccountExportCredential};

    #[test]
    fn every_export_format_round_trips_through_the_account_import_parser() {
        for format in formats() {
            let document = build_account_export(format, &[fixture()], 1_788_000_000_000).unwrap();
            let parsed = parse_import(&document.content, None, &[]).unwrap();
            assert_eq!(parsed.items.len(), 1, "{format:?}");
            assert_eq!(
                parsed.items[0].secrets().access_token(),
                Some("synthetic-round-trip-access"),
                "{format:?}"
            );
        }
    }

    #[test]
    fn export_selection_rejects_duplicates_and_unsafe_ids() {
        assert!(normalize_account_ids(vec!["account_safe".into()]).is_ok());
        assert!(normalize_account_ids(vec!["account_safe".into(), "account_safe".into()]).is_err());
        assert!(normalize_account_ids(vec!["../account".into()]).is_err());
        assert!(normalize_account_ids(Vec::new()).is_err());
    }

    fn formats() -> [AccountExportFormat; 7] {
        [
            AccountExportFormat::Cpa,
            AccountExportFormat::Sub2api,
            AccountExportFormat::Cockpit,
            AccountExportFormat::NineRouter,
            AccountExportFormat::Codex,
            AccountExportFormat::AxonHub,
            AccountExportFormat::CodexManager,
        ]
    }

    fn fixture() -> AccountExportCredential {
        AccountExportCredential {
            label: "Synthetic account".into(),
            email: Some("synthetic@example.test".into()),
            access_token: "synthetic-round-trip-access".into(),
            refresh_token: Some("synthetic-round-trip-refresh".into()),
            id_token: Some("synthetic.round-trip.id".into()),
            account_id: Some("synthetic-account-id".into()),
            user_id: Some("synthetic-user-id".into()),
            organization_id: None,
            plan_type: Some("plus".into()),
            expires_at_ms: Some(1_788_003_600_000),
            issued_at_ms: 1_788_000_000_000,
            subscription_active_until_ms: None,
            created_at_ms: 1_787_000_000_000,
            priority: 10,
            enabled: true,
        }
    }
}
