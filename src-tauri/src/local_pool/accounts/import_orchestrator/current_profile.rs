use super::{existing_identity_index, imported_identity, read_import_documents};
use crate::local_pool::accounts::credentials::CredentialStore;
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::commands::current_time_ms;
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::profiles::codex;
use crate::local_pool::state::DesktopState;
use crate::platform::default_codex_home;
use std::path::Path;
use zenith_relay_core::accounts::{
    parse_import, ImportAuthMode, ImportPreviewStatus, ParsedImport,
};

pub(super) fn current_profile_documents(state: &DesktopState) -> LocalResult<Vec<String>> {
    let codex_home = default_codex_home();
    let bindings = codex::profile_bindings(&codex_home, &state.profile_backup_root())?;
    current_codex_import_documents(&codex_home, &bindings)
}

pub(super) fn current_profile_available(state: &DesktopState) -> LocalResult<bool> {
    let codex_home = default_codex_home();
    let bindings = codex::profile_bindings(&codex_home, &state.profile_backup_root())?;
    if current_codex_profile_is_managed(&bindings) {
        return Ok(false);
    }
    let auth_path = codex_home.join("auth.json");
    if !auth_path.is_file() {
        return Ok(false);
    }
    let documents = read_import_documents(vec![auth_path])?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let existing = existing_identity_index(state, &credentials)?;
    let Ok(parsed) = parse_import(
        &documents[0],
        Some("auth.json"),
        &existing.keys().cloned().collect::<Vec<_>>(),
    ) else {
        return Ok(false);
    };
    Ok(is_usable_current_chatgpt_profile(
        &parsed,
        current_time_ms(),
    ))
}

pub(in crate::local_pool::accounts) fn current_codex_import_documents(
    codex_home: &Path,
    bindings: &[codex::ProfileBinding],
) -> LocalResult<Vec<String>> {
    if current_codex_profile_is_managed(bindings) {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "the current ChatGPT profile is already managed by the local gateway",
        ));
    }
    let auth_path = codex_home.join("auth.json");
    if !auth_path.is_file() {
        return Err(LocalPoolError::new(
            ErrorCode::NotFound,
            "the current ChatGPT profile was not found",
        ));
    }
    read_import_documents(vec![auth_path])
}

fn current_codex_profile_is_managed(bindings: &[codex::ProfileBinding]) -> bool {
    bindings.iter().any(|binding| {
        binding.active && binding.credential_kind == codex::ProfileCredentialKind::LocalGateway
    })
}

pub(in crate::local_pool::accounts) fn is_usable_current_chatgpt_profile(
    parsed: &ParsedImport,
    _now_ms: u64,
) -> bool {
    let ([row], [item]) = (parsed.preview.rows.as_slice(), parsed.items.as_slice()) else {
        return false;
    };
    let identity = imported_identity(item.secrets().id_token(), item.secrets().access_token());
    // Match the official client's behavior: local JWT expiry is only a hint.
    // A profile with an access token can still be usable, and a refresh token
    // must be allowed to prove itself through the explicit refresh action.
    let refreshable =
        item.secrets().access_token().is_some() || item.secrets().refresh_token().is_some();
    row.auth_mode == ImportAuthMode::OAuth
        && row.status == ImportPreviewStatus::Ready
        && row.selectable
        && !row.existing
        && refreshable
        && (item.account_id.is_some() || identity.provider_account_id.is_some())
}
