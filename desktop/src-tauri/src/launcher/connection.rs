use super::settings::{self, Application};
use crate::local_pool::{
    DesktopState,
    accounts::proxy::{ProxyPool, is_proxy_id},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fmt;

mod native;
pub(super) use native::Record;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "code")]
pub(crate) enum ConnectionError {
    #[serde(rename = "connection_unavailable")]
    Unavailable,
    #[serde(rename = "connection_not_found")]
    NotFound,
    #[serde(rename = "connection_unsupported")]
    Unsupported,
    #[serde(rename = "connection_changed")]
    Changed,
    #[serde(rename = "connection_auth_unsupported")]
    AuthUnsupported,
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "Application connection settings are unavailable.",
            Self::NotFound => "The selected connection is no longer available.",
            Self::Unsupported => {
                "This Claude Desktop configuration cannot save the selected connection."
            }
            Self::Changed => "Application settings changed. Reload and try again.",
            Self::AuthUnsupported => "This application cannot save proxy credentials securely.",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConnectionStatus {
    Default,
    Saved,
    Legacy,
    Changed,
    NotSaved,
    Unsupported,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionSetting {
    proxy_id: Option<String>,
    status: ConnectionStatus,
    native_supported: bool,
    revision: String,
}

type Preferences = (Option<String>, Option<Record>);

fn inspect(preferences: &Preferences, config: Option<&native::Config>) -> ConnectionSetting {
    let (legacy, record) = preferences;
    let proxy_id = record
        .as_ref()
        .map_or_else(|| legacy.clone(), |record| record.proxy_id.clone());
    let status = match record {
        Some(record) => match config {
            Some(config) if config.matches_target(record) => {
                if config.fields() == record.applied {
                    if record.proxy_id.is_some() {
                        ConnectionStatus::Saved
                    } else {
                        ConnectionStatus::Default
                    }
                } else if record.pending && config.fields() == record.previous {
                    ConnectionStatus::NotSaved
                } else {
                    ConnectionStatus::Changed
                }
            }
            _ => ConnectionStatus::Changed,
        },
        None if legacy.is_some() => ConnectionStatus::Legacy,
        None if config.is_some() => ConnectionStatus::Default,
        None => ConnectionStatus::Unsupported,
    };
    let mut hash = Sha256::new();
    // The revision exposes neither native configuration nor paths or endpoints.
    hash.update(serde_json::to_vec(preferences).expect("serializable connection preferences"));
    if let Some(config) = config {
        config.hash_into(&mut hash);
    }
    ConnectionSetting {
        proxy_id,
        status,
        native_supported: config.is_some(),
        revision: hex::encode(hash.finalize()),
    }
}

fn read(
    application: Application,
) -> Result<(Preferences, Option<native::Config>), ConnectionError> {
    if application != Application::ClaudeDesktop {
        return Err(ConnectionError::Unsupported);
    }
    let preferences =
        settings::connection_preferences(application).map_err(|_| ConnectionError::Unavailable)?;
    let config = native::resolve()?;
    Ok((preferences, config))
}

#[tauri::command]
pub(crate) async fn get_application_connection(
    application: Application,
) -> Result<ConnectionSetting, ConnectionError> {
    tauri::async_runtime::spawn_blocking(move || {
        let (preferences, config) = read(application)?;
        Ok(inspect(&preferences, config.as_ref()))
    })
    .await
    .map_err(|_| ConnectionError::Unavailable)?
}

#[tauri::command]
pub(crate) async fn set_application_connection(
    application: Application,
    proxy_id: Option<String>,
    expected_revision: String,
    state: tauri::State<'_, DesktopState>,
) -> Result<ConnectionSetting, ConnectionError> {
    let _mutation = state.setup_guard().await;
    let recovery_path = state.claude_desktop_recovery_file();
    tauri::async_runtime::spawn_blocking(move || {
        let (preferences, config) = read(application)?;
        let current = inspect(&preferences, config.as_ref());
        if current.revision != expected_revision {
            return Err(ConnectionError::Changed);
        }
        let persist = |record: Option<Record>, expected: &Preferences| {
            if settings::save_native_connection(application, record, expected)
                .map_err(|_| ConnectionError::Unavailable)?
            {
                Ok(())
            } else {
                Err(ConnectionError::Changed)
            }
        };
        // Forget a launch-only selection without changing the client's settings.
        if proxy_id.is_none() && preferences.1.is_none() {
            persist(None, &preferences)?;
            return Ok(inspect(&(None, None), config.as_ref()));
        }
        let mut config = config.ok_or(ConnectionError::Unsupported)?;
        // Releasing ownership must not undo a newer connection chosen in the client.
        if proxy_id.is_none()
            && preferences
                .1
                .as_ref()
                .is_some_and(|record| config.externally_changed(record))
        {
            persist(None, &preferences)?;
            return Ok(inspect(&(None, None), Some(&config)));
        }
        let endpoint = proxy_id
            .as_deref()
            .map(|id| {
                if !is_proxy_id(id) {
                    return Err(ConnectionError::NotFound);
                }
                let url = ProxyPool::load()
                    .map_err(|_| ConnectionError::Unavailable)?
                    .stored_url(id)
                    .map_err(|_| ConnectionError::NotFound)?;
                native::endpoint(&url)
            })
            .transpose()?;
        let mut record = config.prepare(preferences.1.as_ref(), proxy_id, endpoint)?;
        // Journal first: interruption remains distinguishable from a saved setting.
        persist(Some(record.clone()), &preferences)?;
        let pending = (None, Some(record.clone()));
        config.apply(&record, &recovery_path)?;
        let next = if record.proxy_id.is_none() {
            persist(None, &pending)?;
            None
        } else {
            record.pending = false;
            persist(Some(record.clone()), &pending)?;
            Some(record)
        };
        Ok(inspect(&(None, next), Some(&config)))
    })
    .await
    .map_err(|_| ConnectionError::Unavailable)?
}

#[cfg(test)]
mod tests;
