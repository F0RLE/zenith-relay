use super::*;
use std::collections::BTreeSet;

/// Caller holds setup_guard. Split from the Tauri event wrapper so the
/// durable membership and live routing transaction can be exercised locally.
pub(in crate::local_pool) async fn apply_local_pool_membership(
    input: PoolMembershipInput,
    state: &DesktopState,
) -> CommandResult<(LocalPoolSnapshot, bool, Vec<String>)> {
    let account_ids = input.account_ids.into_iter().collect::<BTreeSet<_>>();
    let source_ids = input.source_ids.into_iter().collect::<BTreeSet<_>>();
    let (old_sources, old_accounts, old_keys) = {
        let store = state.store()?;
        (
            store.sources().to_vec(),
            store.accounts().to_vec(),
            store.keys().to_vec(),
        )
    };
    let known_sources = old_sources
        .iter()
        .map(|source| source.id.as_str())
        .collect::<Vec<_>>();
    let verified_sources = old_sources
        .iter()
        .filter(|source| input.in_pool && source_ids.contains(&source.id))
        .filter(|source| source.supports_any_wire_api().unwrap_or(false))
        .map(|source| source.id.as_str())
        .collect::<Vec<_>>();
    let routes = state.store()?.gateway().account_routes.clone();
    use zenith_relay_core::accounts::management::{
        MembershipError, PoolMembershipEvidence, validate_pool_membership,
    };
    validate_pool_membership(
        &old_accounts,
        &PoolMembershipEvidence {
            requested_accounts: &account_ids,
            requested_sources: &source_ids,
            known_source_ids: &known_sources,
            verified_source_ids: &verified_sources,
            routes: &routes,
            in_pool: input.in_pool,
        },
    )
    .map_err(|error| {
        let (code, message) = match error {
            MembershipError::Empty => (
                ErrorCode::InvalidState,
                "at least one pool member is required",
            ),
            MembershipError::Missing => (ErrorCode::NotFound, "pool member not found"),
            MembershipError::RecoveryRequired => (
                ErrorCode::Conflict,
                "account requires local recovery before joining the pool",
            ),
            MembershipError::UnselectedRoute => (
                ErrorCode::Conflict,
                "Select this account route before adding it to the pool",
            ),
            MembershipError::UnverifiedSource => (
                ErrorCode::Conflict,
                "source must expose at least one verified API route before joining the local pool",
            ),
        };
        LocalPoolError::new(code, message)
    })?;

    if input.in_pool {
        let settings = state.store()?.gateway().clone();
        if let Some(account) = old_accounts.iter().find(|account| {
            account_ids.contains(&account.account.id) && !account.has_pool_models(&settings)
        }) {
            let message = if zenith_relay_core::account_image_restriction(
                account.oauth_client_kind.unwrap_or_default(),
                account.account.subscription.plan_type.as_deref(),
            ) == Some(
                zenith_relay_core::ImageGenerationRestriction::ProviderFreePlan,
            ) {
                "OpenAI provider restriction: Images are unavailable for this Free account and no text models are enabled for the pool"
            } else {
                "Account has no models available for the pool"
            };
            return Err(LocalPoolError::new(ErrorCode::Conflict, message).into());
        }
    }

    let mut sources = old_sources.clone();
    let mut accounts = old_accounts.clone();
    let model_refresh_account_ids = if input.in_pool {
        old_accounts
            .iter()
            .filter(|account| account_ids.contains(&account.account.id) && !account.account.in_pool)
            .map(|account| account.account.id.clone())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    for source in &mut sources {
        if source_ids.contains(&source.id) {
            source.in_pool = input.in_pool;
        }
    }
    for account in &mut accounts {
        if account_ids.contains(&account.account.id) {
            crate::local_pool::accounts::records::set_pool_membership(account, input.in_pool)?;
        }
    }
    if sources == old_sources && accounts == old_accounts {
        return Ok((state.snapshot().await?, false, Vec::new()));
    }

    let changed_accounts = accounts
        .iter()
        .filter(|account| account_ids.contains(&account.account.id))
        .cloned()
        .collect::<Vec<_>>();
    let fenced_accounts = old_accounts
        .iter()
        .filter(|account| {
            account_ids.contains(&account.account.id) && account.account.in_pool != input.in_pool
        })
        .map(|account| account.account.id.clone())
        .collect::<Vec<_>>();
    let fenced_sources = old_sources
        .iter()
        .filter(|source| source_ids.contains(&source.id) && source.in_pool != input.in_pool)
        .map(|source| source.id.clone())
        .collect::<Vec<_>>();
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences =
        fence_runtime_candidates(runtime.as_deref(), &fenced_accounts, &fenced_sources);
    state
        .store()?
        .replace_pool_records(sources, accounts, old_keys.clone())?;
    let policy_now_ms = super::super::current_time_ms();
    let updated_in_place = if let Some(runtime) = state.gateway.runtime().await {
        changed_accounts.iter().all(|account| {
            runtime.update_account_policy(
                &account.account.id,
                runtime_account_policy(account, policy_now_ms),
            )
        }) && super::super::apply_local_gateway_key_scope(state, &runtime).unwrap_or(false)
    } else {
        false
    };
    if !updated_in_place {
        restart_or_rollback(state, || {
            state
                .store()?
                .replace_pool_records(old_sources, old_accounts, old_keys)
        })
        .await?;
    }
    let now_ms = policy_now_ms;
    for account_id in account_ids {
        state.sync_account_quota_refresh(&account_id, now_ms)?;
    }
    let snapshot = state.snapshot().await?;
    Ok((snapshot, updated_in_place, model_refresh_account_ids))
}
