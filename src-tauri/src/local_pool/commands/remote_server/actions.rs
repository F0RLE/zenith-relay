use super::session::{active_client, remote_error};
use super::{ExecuteRemoteServerActionInput, RemoteServerAction};
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    remote::client::RemoteClientError,
    state::DesktopState,
};
use reqwest::Method;
use tauri::State;

#[tauri::command]
pub async fn execute_remote_server_action(
    input: ExecuteRemoteServerActionInput,
    state: State<'_, DesktopState>,
) -> Result<serde_json::Value, CommandError> {
    let _mutation = state.setup_guard().await;
    if let RemoteServerAction::DeleteAccount { id } = &input.action {
        if state
            .store()?
            .ownership_operation()
            .is_some_and(|operation| {
                operation
                    .remote_account_ids
                    .iter()
                    .any(|account_id| account_id == id)
            })
        {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "account ownership recovery must finish before deleting this server record",
            )
            .into());
        }
    }
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    let (method, path, requires_payload) = action_request(&input.action)?;
    let needs_rotation = matches!(input.action, RemoteServerAction::SetRoutingPolicy)
        && input
            .payload
            .as_ref()
            .and_then(|payload| payload.pointer("/poolRouting/version"))
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|version| version >= 2);
    if needs_rotation
        && !client
            .capabilities()
            .await
            .map_err(remote_error)?
            .supports(zenith_relay_core::protocol::Feature::Rotation)
    {
        return Err(LocalPoolError::new(
            ErrorCode::UnsupportedSchema,
            "server does not support pool rotation",
        )
        .into());
    }
    if matches!(input.action, RemoteServerAction::SetToolPolicy)
        && !client
            .capabilities()
            .await
            .map_err(remote_error)?
            .supports(zenith_relay_core::protocol::Feature::ToolPolicy)
    {
        return Err(LocalPoolError::new(
            ErrorCode::UnsupportedSchema,
            "server does not support tool catalog policy",
        )
        .into());
    }
    let uses_protocol_contract = matches!(&input.action, RemoteServerAction::ProbeSource { .. })
        || (matches!(
            &input.action,
            RemoteServerAction::CreateSource | RemoteServerAction::UpdateSource { .. }
        ) && input.payload.as_ref().is_some_and(|payload| {
            payload
                .get("protocolBindings")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|bindings| {
                    bindings.iter().any(|binding| {
                        binding
                            .get("adapter")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|adapter| {
                                !matches!(
                                    adapter,
                                    "native" | "responses_to_messages" | "responses_to_gemini"
                                )
                            })
                    })
                })
        }));
    if uses_protocol_contract
        && !client
            .capabilities()
            .await
            .map_err(remote_error)?
            .supports(zenith_relay_core::protocol::Feature::SourceProtocols)
    {
        return Err(LocalPoolError::new(
            ErrorCode::UnsupportedSchema,
            "server does not support source protocol discovery",
        )
        .into());
    }
    if requires_payload && input.payload.is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "remote action payload is required",
        )
        .into());
    }
    client
        .mutate(method, &path, input.payload.as_ref())
        .await
        .map_err(remote_action_error)
}

fn action_request(action: &RemoteServerAction) -> Result<(Method, String, bool), CommandError> {
    let request = match action {
        RemoteServerAction::CreateSource => (Method::POST, "/sources".to_string(), true),
        RemoteServerAction::UpdateSource { id } => {
            (Method::PATCH, object_path("sources", id)?, true)
        }
        RemoteServerAction::DeleteSource { id } => {
            (Method::DELETE, object_path("sources", id)?, false)
        }
        RemoteServerAction::TestSource { id } => (
            Method::POST,
            format!("{}/test", object_path("sources", id)?),
            false,
        ),
        RemoteServerAction::ProbeSource { id } => (
            Method::POST,
            format!("{}/probe", object_path("sources", id)?),
            true,
        ),
        RemoteServerAction::PreviewAccountImport => {
            (Method::POST, "/accounts/import/preview".to_string(), true)
        }
        RemoteServerAction::ConfirmAccountImport => {
            (Method::POST, "/accounts/import/confirm".to_string(), true)
        }
        RemoteServerAction::PreviewAccountBatchImport => (
            Method::POST,
            "/accounts/import/batch/preview".to_string(),
            true,
        ),
        RemoteServerAction::ConfirmAccountBatchImport => (
            Method::POST,
            "/accounts/import/batch/confirm".to_string(),
            true,
        ),
        RemoteServerAction::UpdateAccount { id } => {
            (Method::PATCH, object_path("accounts", id)?, true)
        }
        RemoteServerAction::RefreshAccount { id } => (
            Method::POST,
            format!("{}/refresh", object_path("accounts", id)?),
            false,
        ),
        RemoteServerAction::DeleteAccount { id } => {
            (Method::DELETE, object_path("accounts", id)?, false)
        }
        RemoteServerAction::SetCommonProxy => (Method::POST, "/proxies/common".to_string(), true),
        RemoteServerAction::SetAccountProxyRequired => {
            (Method::POST, "/proxies/policy".to_string(), true)
        }
        RemoteServerAction::SetAccountProxy { id } => (
            Method::POST,
            format!("{}/proxy", object_path("accounts", id)?),
            true,
        ),
        RemoteServerAction::AssignAccountProxies => {
            (Method::POST, "/accounts/proxies/assign".to_string(), true)
        }
        RemoteServerAction::SetPoolMembership => (Method::POST, "/pool/members".to_string(), true),
        RemoteServerAction::SetQuotaPolicy => (Method::POST, "/quota/settings".to_string(), true),
        RemoteServerAction::SetRoutingPolicy => {
            (Method::POST, "/routing/settings".to_string(), true)
        }
        RemoteServerAction::RefreshAllQuotas => {
            (Method::POST, "/pool/quota/refresh".to_string(), false)
        }
        RemoteServerAction::RefreshPricingCatalog => {
            (Method::POST, "/pricing/refresh".to_string(), false)
        }
        RemoteServerAction::SetModelEnabled => (Method::POST, "/models/rules".to_string(), true),
        RemoteServerAction::SetModelPrice => (Method::POST, "/models/prices".to_string(), true),
        RemoteServerAction::SetModelReasoning => {
            (Method::POST, "/models/reasoning".to_string(), true)
        }
        RemoteServerAction::SetModelServiceTier => {
            (Method::POST, "/models/service-tier".to_string(), true)
        }
        RemoteServerAction::SetModelOrder => (Method::POST, "/models/order".to_string(), true),
        RemoteServerAction::StartGateway => (Method::POST, "/gateway/start".to_string(), false),
        RemoteServerAction::StopGateway => (Method::POST, "/gateway/stop".to_string(), false),
        RemoteServerAction::SetCodexBackgroundTasks => (
            Method::POST,
            "/gateway/codex-background-tasks".to_string(),
            true,
        ),
        RemoteServerAction::SetToolPolicy => {
            (Method::POST, "/gateway/tool-policy".to_string(), true)
        }
        RemoteServerAction::SetChatgptRetryUntilAvailable => (
            Method::POST,
            "/gateway/chatgpt-retry-until-available".to_string(),
            true,
        ),
        RemoteServerAction::SetBlockDegradedRoutes => (
            Method::POST,
            "/gateway/block-degraded-routes".to_string(),
            true,
        ),
        RemoteServerAction::SetCodexWebsockets => {
            (Method::POST, "/gateway/codex-websockets".to_string(), true)
        }
        RemoteServerAction::CreateWakeTask => (Method::POST, "/wake-tasks".to_string(), true),
        RemoteServerAction::UpdateWakeTask { id } => {
            (Method::PATCH, object_path("wake-tasks", id)?, true)
        }
        RemoteServerAction::DeleteWakeTask { id } => {
            (Method::DELETE, object_path("wake-tasks", id)?, false)
        }
        RemoteServerAction::TestWakeTask { id } => (
            Method::POST,
            format!("{}/test", object_path("wake-tasks", id)?),
            false,
        ),
        RemoteServerAction::ClearUsage => (Method::DELETE, "/usage".to_string(), false),
    };
    Ok(request)
}

pub(super) fn object_path(collection: &str, id: &str) -> Result<String, CommandError> {
    if !zenith_relay_core::is_ascii_token(id, 128) {
        return Err(
            LocalPoolError::new(ErrorCode::InvalidState, "remote object id is invalid").into(),
        );
    }
    Ok(format!("/{collection}/{id}"))
}

fn remote_action_error(error: RemoteClientError) -> CommandError {
    if matches!(error, RemoteClientError::PoolRoutingConflict) {
        LocalPoolError::new(ErrorCode::Conflict, error.to_string()).into()
    } else {
        remote_error(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pricing_catalog_refresh_uses_its_dedicated_empty_post_request() {
        let (method, path, requires_payload) =
            action_request(&RemoteServerAction::RefreshPricingCatalog)
                .expect("pricing catalog refresh request should be supported");

        assert_eq!(method, Method::POST);
        assert_eq!(path, "/pricing/refresh");
        assert!(!requires_payload);
    }
}
