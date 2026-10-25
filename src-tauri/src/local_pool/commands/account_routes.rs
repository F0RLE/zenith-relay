use crate::local_pool::{
    accounts::{
        credentials::{credential_local_error, CredentialStore, StoredCodexCredentials},
        NativeSecretBackend,
    },
    error::{CommandError, ErrorCode, LocalPoolError, Result},
    models::{GatewaySettings, LocalAccountRecord},
    state::DesktopState,
};
use tauri::State;
use zenith_relay_core::{
    accounts::{
        select_account_route_group, validate_account_route_connection, AccountRouteIdentity,
    },
    automations::WakeTask,
    providers::chatgpt::OAuthClientKind,
    CandidateRuntimeSnapshot, ModelRules,
};

fn conflict(message: &'static str) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Conflict, message)
}

fn copy_route_settings(destination: &mut LocalAccountRecord, source: &LocalAccountRecord) {
    destination.account.label = source.account.label.clone();
    destination.account.tags = source.account.tags.clone();
    destination.account.enabled = source.account.enabled;
    destination.account.draining = source.account.draining;
    destination.account.in_pool = source.account.in_pool;
    destination.priority = source.priority;
    destination.weight = source.weight;
    destination.purchase_cost_micro_usd = source.purchase_cost_micro_usd;
}

fn restore_route_records(
    current: &mut [LocalAccountRecord],
    previous: &[LocalAccountRecord],
) -> Result<()> {
    for previous in previous {
        let destination = current
            .iter_mut()
            .find(|record| {
                record.account.id == previous.account.id
                    && record.account.identity.identity_hash
                        == previous.account.identity.identity_hash
            })
            .ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "Account changed during route rollback",
                )
            })?;
        copy_route_settings(destination, previous);
        destination.image_pool_enabled = previous.image_pool_enabled;
    }
    Ok(())
}

fn original_login_email(
    record: &LocalAccountRecord,
    credentials: &StoredCodexCredentials,
) -> Option<String> {
    let claims = zenith_relay_core::accounts::decode_unverified_jwt_payload::<serde_json::Value>(
        credentials.id_token()?,
    )?;
    let email = claims.get("email")?.as_str()?;
    // Pin the original sign-in email to its saved identity, not editable login notes.
    let identity = crate::local_pool::accounts::records::credential_identity_hash(
        credentials.oauth_client_kind(),
        credentials.provider_account_id()?,
        credentials.provider_user_id(),
        Some(email),
    );
    (identity == record.account.identity.identity_hash && !email.trim().is_empty())
        .then(|| email.trim().to_ascii_lowercase())
}

fn route_identity(
    record: &LocalAccountRecord,
    credentials: &StoredCodexCredentials,
) -> AccountRouteIdentity {
    AccountRouteIdentity {
        client_kind: credentials.oauth_client_kind(),
        provider_account_id: credentials.provider_account_id().map(str::to_owned),
        provider_user_id: credentials.provider_user_id().map(str::to_owned),
        original_login_email: original_login_email(record, credentials),
    }
}

fn route_group_connections<'a>(
    accounts: &'a [LocalAccountRecord],
    gateway: &GatewaySettings,
    account_id: &str,
    credentials: &CredentialStore<NativeSecretBackend>,
) -> Result<Vec<(&'a LocalAccountRecord, StoredCodexCredentials)>> {
    let ids = gateway
        .account_route_group(account_id)
        .map(|group| group.account_ids.clone())
        .unwrap_or_else(|| vec![account_id.to_owned()]);
    ids.iter()
        .map(|id| {
            let record = accounts
                .iter()
                .find(|account| account.account.id == *id && account.recovery.is_none())
                .ok_or_else(|| conflict("Account is unavailable"))?;
            Ok((
                record,
                credentials.require(id).map_err(credential_local_error)?,
            ))
        })
        .collect()
}

fn validate_route_connection(
    connections: &[(&LocalAccountRecord, StoredCodexCredentials)],
    candidate: &LocalAccountRecord,
    credentials: &StoredCodexCredentials,
) -> Result<()> {
    validate_account_route_connection(
        connections
            .iter()
            .map(|(record, credentials)| route_identity(record, credentials)),
        &route_identity(candidate, credentials),
    )
    .map_err(conflict)
}

#[tauri::command]
pub async fn local_account_route_candidates(
    account_id: String,
    state: State<'_, DesktopState>,
) -> std::result::Result<Vec<String>, CommandError> {
    let (accounts, gateway) = {
        let store = state.store()?;
        (store.accounts().to_vec(), store.gateway().clone())
    };
    let candidates = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<String>> {
        let credentials = CredentialStore::from_backend(NativeSecretBackend);
        let connections = route_group_connections(&accounts, &gateway, &account_id, &credentials)?;
        Ok(accounts
            .iter()
            .filter(|account| {
                account.recovery.is_none()
                    && gateway.account_route_group(&account.account.id).is_none()
                    && account.account.id != account_id
            })
            .filter_map(|account| {
                let candidate = credentials.load(&account.account.id).ok().flatten()?;
                validate_route_connection(&connections, account, &candidate)
                    .is_ok()
                    .then(|| account.account.id.clone())
            })
            .collect())
    })
    .await
    .map_err(|_| conflict("Could not inspect account connections"))??;
    Ok(candidates)
}

fn select_records(
    accounts: &mut [LocalAccountRecord],
    gateway: &mut GatewaySettings,
    current: &str,
    next: &str,
    attach: bool,
    next_kind: OAuthClientKind,
) -> Result<Vec<String>> {
    if !gateway.account_route_is_selected(current) {
        return Err(conflict(
            "The selected account route changed; refresh and try again",
        ));
    }
    let source = accounts
        .iter()
        .find(|a| a.account.id == current)
        .cloned()
        .ok_or_else(|| conflict("Account route was not found"))?;
    let group = select_account_route_group(&gateway.account_routes, current, next, attach)
        .map_err(conflict)?;
    let destination = accounts
        .iter_mut()
        .find(|a| a.account.id == next)
        .ok_or_else(|| conflict("Account route was not found"))?;
    if source.recovery.is_some() || destination.recovery.is_some() {
        return Err(conflict("Recover the account before changing its route"));
    }
    copy_route_settings(destination, &source);
    // Route switching does not transfer or revive native Images consent.
    destination.image_pool_enabled = false;
    destination.account.in_pool &= next_kind != OAuthClientKind::Codex;
    for account in accounts.iter_mut() {
        if group.account_ids.contains(&account.account.id) && account.account.id != next {
            account.account.in_pool = false;
            account.image_pool_enabled = false;
        }
    }
    if let Some(policy) = &mut gateway.pool_routing {
        group.retarget_pool_routing(policy, current, next_kind);
    }
    gateway
        .account_routes
        .retain(|existing| !existing.account_ids.iter().any(|id| id == current));
    let ids = group.account_ids.clone();
    gateway.account_routes.push(group);
    gateway.validate().map_err(LocalPoolError::invalid_state)?;
    Ok(ids)
}

fn select_automation_route(
    tasks: &mut [WakeTask],
    route_ids: &[String],
    destination: &LocalAccountRecord,
    next_kind: OAuthClientKind,
) -> Result<()> {
    zenith_relay_core::automations::select_automation_route(
        tasks,
        route_ids,
        &destination.account.id,
        destination.effective_models(),
        &ModelRules::from_allow_deny(&destination.allowed_models, &destination.excluded_models),
        next_kind,
        super::current_time_ms(),
    )
    .map_err(conflict)
}

#[tauri::command]
pub async fn select_local_account_route(
    account_id: String,
    connection_id: String,
    attach: bool,
    state: State<'_, DesktopState>,
) -> std::result::Result<(), CommandError> {
    let _setup = state.setup_guard().await;
    if account_id == connection_id {
        return Ok(());
    }
    let (old_accounts, old_gateway) = {
        let store = state.store()?;
        (store.accounts().to_vec(), store.gateway().clone())
    };
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let owner = credentials
        .require(&account_id)
        .map_err(credential_local_error)?;
    let next_credentials = credentials
        .require(&connection_id)
        .map_err(credential_local_error)?;
    let owner_record = old_accounts
        .iter()
        .find(|account| account.account.id == account_id)
        .ok_or_else(|| conflict("Account route was not found"))?;
    let destination = old_accounts
        .iter()
        .find(|a| a.account.id == connection_id)
        .ok_or_else(|| conflict("Account route was not found"))?;
    if attach {
        if owner.oauth_client_kind() != OAuthClientKind::Codex {
            crate::local_pool::accounts::records::require_pool_account(owner_record)?;
        }
        let connections =
            route_group_connections(&old_accounts, &old_gateway, &account_id, &credentials)?;
        validate_route_connection(&connections, destination, &next_credentials)?;
    }
    if next_credentials.oauth_client_kind() != OAuthClientKind::Codex {
        crate::local_pool::accounts::records::require_pool_account(destination)?;
    }
    let mut accounts = old_accounts.clone();
    let mut gateway = old_gateway.clone();
    let ids = select_records(
        &mut accounts,
        &mut gateway,
        &account_id,
        &connection_id,
        attach,
        next_credentials.oauth_client_kind(),
    )?;
    if attach {
        if let Some(group) = gateway
            .account_routes
            .iter_mut()
            .find(|group| group.selected_account_id == connection_id)
        {
            group.notes_account_id.get_or_insert_with(|| {
                if owner.password().is_some()
                    || owner.totp_secret().is_some()
                    || owner.phone().is_some()
                {
                    account_id.clone()
                } else if next_credentials.password().is_some()
                    || next_credentials.totp_secret().is_some()
                    || next_credentials.phone().is_some()
                {
                    connection_id.clone()
                } else {
                    account_id.clone()
                }
            });
        }
    }
    let runtime = state.gateway.runtime().await;
    let _fences = super::fence_runtime_candidates(runtime.as_deref(), &ids, &[]);
    if runtime.as_ref().is_some_and(|runtime| {
        routes_have_active_requests(&ids, &runtime.candidate_runtime_order())
    }) {
        return Err(conflict(
            "Wait for active requests to finish before changing the account route",
        )
        .into());
    }
    let (previous_accounts, previous_groups, previous_policy, previous_tasks) = {
        let mut store = state.store()?;
        let previous = store.accounts().to_vec();
        let previous_gateway = store.gateway().clone();
        let mut current = previous.clone();
        let mut current_gateway = previous_gateway.clone();
        select_records(
            &mut current,
            &mut current_gateway,
            &account_id,
            &connection_id,
            attach,
            next_credentials.oauth_client_kind(),
        )?;
        if let Some(group) = current_gateway
            .account_routes
            .iter_mut()
            .find(|group| group.selected_account_id == connection_id)
        {
            group.notes_account_id = gateway
                .account_route_group(&connection_id)
                .and_then(|group| group.notes_account_id.clone());
        }
        let mut automations = store.automations().clone();
        let previous_tasks = automations.tasks.clone();
        let destination = current
            .iter()
            .find(|account| account.account.id == connection_id)
            .ok_or_else(|| conflict("Account route was not found"))?;
        select_automation_route(
            &mut automations.tasks,
            &ids,
            destination,
            next_credentials.oauth_client_kind(),
        )?;
        store.replace_account_routes(current, current_gateway, automations)?;
        (
            previous
                .into_iter()
                .filter(|account| ids.contains(&account.account.id))
                .collect::<Vec<_>>(),
            previous_gateway.account_routes,
            previous_gateway.pool_routing,
            previous_tasks,
        )
    };
    super::restart_or_rollback(&state, || {
        let mut store = state.store()?;
        let mut current = store.accounts().to_vec();
        restore_route_records(&mut current, &previous_accounts)?;
        let mut gateway = store.gateway().clone();
        gateway.account_routes = previous_groups;
        gateway.pool_routing = previous_policy;
        let mut automations = store.automations().clone();
        // Roll back task targets, not newer wake history or consumed reset credits.
        automations.tasks = previous_tasks;
        store.replace_account_routes(current, gateway, automations)
    })
    .await?;
    Ok(())
}

#[tauri::command]
pub async fn detach_local_account_route(
    account_id: String,
    connection_id: String,
    state: State<'_, DesktopState>,
) -> std::result::Result<(), CommandError> {
    let _setup = state.setup_guard().await;
    let mut store = state.store()?;
    let accounts = store.accounts().to_vec();
    let mut gateway = store.gateway().clone();
    zenith_relay_core::accounts::detach_account_route(
        &mut gateway.account_routes,
        &account_id,
        &connection_id,
    )
    .map_err(conflict)?;
    let automations = store.automations().clone();
    store.replace_account_routes(accounts, gateway, automations)?;
    Ok(())
}

fn routes_have_active_requests(ids: &[String], candidates: &[CandidateRuntimeSnapshot]) -> bool {
    candidates.iter().any(|candidate| {
        ids.contains(&candidate.candidate_id)
            && (candidate.active_request_count > 0 || candidate.in_flight > 0)
    })
}

#[cfg(test)]
mod tests;
