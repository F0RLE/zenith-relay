use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError, Result},
    models::BindScope,
    state::DesktopState,
};
use serde::Serialize;
use std::net::IpAddr;
use sysinfo::{InterfaceOperationalState, IpNetwork, Networks};
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkAccess {
    enabled: bool,
    addresses: Vec<NetworkAddress>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct NetworkAddress {
    interface: String,
    base_url: String,
}

fn network_addresses<'a>(
    interfaces: impl IntoIterator<Item = (&'a str, InterfaceOperationalState, &'a [IpNetwork])>,
    port: u16,
) -> Vec<NetworkAddress> {
    let mut addresses = Vec::new();
    for (name, state, networks) in interfaces {
        if !matches!(
            state,
            InterfaceOperationalState::Up | InterfaceOperationalState::Unknown
        ) {
            continue;
        }
        for network in networks {
            if let IpAddr::V4(ip) = network.addr
                && ip.is_private()
            {
                addresses.push(NetworkAddress {
                    interface: name.to_owned(),
                    base_url: format!("http://{ip}:{port}/v1"),
                });
            }
        }
    }
    // Keep every usable interface: a default route may belong to a VPN, not the LAN.
    addresses.sort_by(|a, b| (&a.interface, &a.base_url).cmp(&(&b.interface, &b.base_url)));
    addresses.dedup();
    addresses
}

fn read_snapshot(state: &DesktopState) -> Result<NetworkAccess> {
    let settings = state.store()?.gateway().clone();
    let networks = Networks::new_with_refreshed_list();
    Ok(NetworkAccess {
        enabled: settings.bind_scope == BindScope::Lan,
        addresses: network_addresses(
            networks.iter().map(|(name, network)| {
                (
                    name.as_str(),
                    network.operational_state(),
                    network.ip_networks(),
                )
            }),
            settings.port,
        ),
    })
}

async fn snapshot(state: &DesktopState) -> Result<NetworkAccess> {
    let state = state.clone();
    super::super::run_blocking(move || read_snapshot(&state)).await
}

#[tauri::command]
pub async fn get_local_network_access(
    state: State<'_, DesktopState>,
) -> std::result::Result<NetworkAccess, CommandError> {
    snapshot(&state).await.map_err(Into::into)
}

pub(super) async fn update(
    state: &DesktopState,
    enabled: bool,
    expected_enabled: bool,
) -> Result<NetworkAccess> {
    let _mutation = state.setup_guard().await;
    let old = state.store()?.gateway().clone();
    if (old.bind_scope == BindScope::Lan) != expected_enabled {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "network access changed; refresh before saving",
        ));
    }
    if enabled != expected_enabled {
        if enabled {
            super::super::pool::ensure_lan_gateway_key(state)?;
        }
        let mut settings = old.clone();
        settings.bind_scope = if enabled {
            BindScope::Lan
        } else {
            BindScope::Localhost
        };
        state.store()?.replace_gateway(settings)?;
        super::super::sync_gateway_or_rollback(state, old).await?;
    }
    snapshot(state).await
}

#[tauri::command]
pub async fn update_local_network_access(
    enabled: bool,
    expected_enabled: bool,
    state: State<'_, DesktopState>,
) -> std::result::Result<NetworkAccess, CommandError> {
    update(&state, enabled, expected_enabled)
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests;
