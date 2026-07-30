use axum::extract::connect_info::IntoMakeServiceWithConnectInfo;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    future::IntoFuture,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::watch;
use zenith_relay_server::{
    config::Config,
    http, jobs, state,
    state::{AppState, COMMON_PROXY_SECRET_REF},
    store::{Store, Vault},
};

const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);
const BACKUP_FORMAT: &str = "zenith-relay-server-backup";
const BACKUP_VERSION: u32 = 1;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Zenith Relay Server failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let config = Config::from_env()?;
    let command = std::env::args().skip(1).collect::<Vec<_>>();
    if !command.is_empty() && command.len() != 2 {
        return Err("supported commands are --backup <dir> and --restore <dir>".to_string());
    }
    fs::create_dir_all(&config.data_dir).map_err(io_error)?;
    let _data_lock = acquire_data_lock(&config.data_dir)?;
    if let [operation, path] = command.as_slice() {
        return match operation.as_str() {
            "--backup" => backup(&config, Path::new(path)),
            "--restore" => restore(&config, Path::new(path)),
            _ => Err("supported commands are --backup <dir> and --restore <dir>".to_string()),
        };
    }
    if !command.is_empty() {
        return Err("supported commands are --backup <dir> and --restore <dir>".to_string());
    }
    let store = Arc::new(Store::open(config.data_dir.join("relay.sqlite"))?);
    let vault = Arc::new(Vault::open(
        &config.data_dir.join("vault"),
        config.vault_key,
    )?);
    let state = AppState::new(config.clone(), store, vault)?;
    state.rebuild_runtime().await?;
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|error| format!("failed to bind {}: {error}", config.bind))?;
    let (shutdown_sender, shutdown) = watch::channel(false);
    let background_jobs = jobs::start(state.clone(), shutdown.clone());
    let service: IntoMakeServiceWithConnectInfo<_, SocketAddr> =
        http::router(state.clone()).into_make_service_with_connect_info();
    let server = axum::serve(listener, service)
        .with_graceful_shutdown(wait_for_shutdown(shutdown))
        .into_future();
    tokio::pin!(server);
    let server_result = tokio::select! {
        result = server.as_mut() => result.map_err(|error| format!("server failed: {error}")),
        _ = shutdown_signal() => {
            let _ = shutdown_sender.send(true);
            tokio::time::timeout(SHUTDOWN_GRACE, server.as_mut())
                .await
                .map_err(|_| "server graceful shutdown timed out".to_string())?
                .map_err(|error| format!("server failed: {error}"))
        }
    };
    let _ = shutdown_sender.send(true);
    let jobs_result = tokio::time::timeout(SHUTDOWN_GRACE, background_jobs.join())
        .await
        .map_err(|_| "background jobs did not stop in time".to_string())?;
    let usage_result = tokio::time::timeout(SHUTDOWN_GRACE, state.shutdown_runtime())
        .await
        .map_err(|_| "usage writer did not flush in time".to_string())?;
    server_result?;
    jobs_result?;
    usage_result
}

async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            return;
        }
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(unix)]
async fn shutdown_signal() {
    let Ok(mut terminate) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    else {
        let _ = tokio::signal::ctrl_c().await;
        return;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupManifest {
    format: String,
    version: u32,
    created_at_ms: u64,
}

fn backup(config: &Config, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err("backup destination already exists".to_string());
    }
    let staging = temporary_sibling(destination, "partial")?;
    fs::create_dir_all(&staging).map_err(io_error)?;
    let store = Store::open(config.data_dir.join("relay.sqlite"))?;
    let vault = Vault::open(&config.data_dir.join("vault"), config.vault_key)?;
    validate_store_secrets(&store, &vault)?;
    let result = (|| {
        store.backup_to(&staging.join("relay.sqlite"))?;
        let vault_path = config.data_dir.join("vault").join("secrets.enc");
        if vault_path.is_file() {
            fs::create_dir_all(staging.join("vault")).map_err(io_error)?;
            fs::copy(vault_path, staging.join("vault").join("secrets.enc")).map_err(io_error)?;
        }
        let manifest = serde_json::to_vec_pretty(&BackupManifest {
            format: BACKUP_FORMAT.to_string(),
            version: BACKUP_VERSION,
            created_at_ms: state::now_ms(),
        })
        .map_err(|_| "backup manifest serialization failed".to_string())?;
        fs::write(staging.join("manifest.json"), manifest).map_err(io_error)?;
        validate_backup(&staging, config.vault_key)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        fs::rename(&staging, destination).map_err(io_error)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(staging);
    }
    result
}

fn restore(config: &Config, source: &Path) -> Result<(), String> {
    validate_manifest(source)?;
    let database = source.join("relay.sqlite");
    if !database.is_file() {
        return Err("backup database is missing".to_string());
    }
    let staging = config
        .data_dir
        .join(format!(".restore-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&staging).map_err(io_error)?;
    let result = (|| {
        fs::copy(source.join("manifest.json"), staging.join("manifest.json")).map_err(io_error)?;
        fs::copy(&database, staging.join("relay.sqlite")).map_err(io_error)?;
        let vault = source.join("vault").join("secrets.enc");
        if vault.is_file() {
            fs::create_dir_all(staging.join("vault")).map_err(io_error)?;
            fs::copy(vault, staging.join("vault").join("secrets.enc")).map_err(io_error)?;
        }
        validate_backup(&staging, config.vault_key)?;
        activate_restore(&config.data_dir, &staging)
    })();
    let _ = fs::remove_dir_all(staging);
    result
}

fn validate_manifest(source: &Path) -> Result<BackupManifest, String> {
    let manifest = fs::read(source.join("manifest.json")).map_err(io_error)?;
    let manifest: BackupManifest =
        serde_json::from_slice(&manifest).map_err(|_| "backup manifest is invalid".to_string())?;
    if manifest.format != BACKUP_FORMAT || manifest.version != BACKUP_VERSION {
        return Err("backup format is not supported".to_string());
    }
    Ok(manifest)
}

fn validate_backup(root: &Path, vault_key: [u8; 32]) -> Result<(), String> {
    validate_manifest(root)?;
    let database = root.join("relay.sqlite");
    if !database.is_file() {
        return Err("backup database is missing".to_string());
    }
    let store = Store::open(database)?;
    let vault = Vault::open(&root.join("vault"), vault_key)?;
    let result = validate_store_secrets(&store, &vault);
    drop(vault);
    drop(store);
    for suffix in [".migration.lock", "-wal", "-shm"] {
        let _ = fs::remove_file(append_suffix(&root.join("relay.sqlite"), suffix));
    }
    result
}

fn validate_store_secrets(store: &Store, vault: &Vault) -> Result<(), String> {
    let _ = store.server_id()?;
    let proxies = store.proxies()?;
    let proxy_ids = proxies
        .iter()
        .map(|record| record.id.clone())
        .collect::<std::collections::HashSet<_>>();
    let accounts = store.accounts()?;
    if accounts
        .iter()
        .filter_map(|record| record.proxy_id.as_deref())
        .any(|proxy_id| !proxy_ids.contains(proxy_id))
        || store
            .common_proxy_id()?
            .is_some_and(|proxy_id| !proxy_ids.contains(proxy_id.as_str()))
    {
        return Err("backup references a missing proxy object".to_string());
    }
    let secret_refs = store
        .sources()?
        .into_iter()
        .map(|record| record.secret_ref)
        .chain(accounts.into_iter().map(|record| record.secret_ref))
        .chain(store.keys()?.into_iter().map(|record| record.secret_ref))
        .chain(proxies.into_iter().map(|record| record.secret_ref));
    for secret_ref in secret_refs {
        if vault.load(&secret_ref)?.is_none() {
            return Err("backup references a missing encrypted secret".to_string());
        }
    }
    if store.common_proxy_configured()?
        && store.common_proxy_id()?.is_none()
        && vault.load(COMMON_PROXY_SECRET_REF)?.is_none()
    {
        return Err("backup references a missing encrypted proxy".to_string());
    }
    Ok(())
}

fn activate_restore(data_dir: &Path, staging: &Path) -> Result<(), String> {
    let database = data_dir.join("relay.sqlite");
    let vault = data_dir.join("vault").join("secrets.enc");
    let targets = vec![
        (database.clone(), Some(staging.join("relay.sqlite"))),
        (append_suffix(&database, "-wal"), None),
        (append_suffix(&database, "-shm"), None),
        (
            vault.clone(),
            Some(staging.join("vault").join("secrets.enc")),
        ),
        (append_suffix(&vault, ".bak"), None),
    ];
    let mut moved = Vec::new();
    for (target, _) in &targets {
        if !target.exists() {
            continue;
        }
        let backup = temporary_sibling(target, "pre-restore")?;
        if let Err(error) = fs::rename(target, &backup) {
            rollback_restore(&moved, &[]);
            return Err(io_error(error));
        }
        moved.push((target.clone(), backup));
    }

    let mut installed = Vec::new();
    for (target, staged) in &targets {
        let Some(staged) = staged.as_ref().filter(|path| path.is_file()) else {
            continue;
        };
        if let Some(parent) = target.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                rollback_restore(&moved, &installed);
                return Err(io_error(error));
            }
        }
        if let Err(error) = fs::rename(staged, target) {
            rollback_restore(&moved, &installed);
            return Err(io_error(error));
        }
        installed.push(target.clone());
    }
    for (_, backup) in moved {
        let _ = fs::remove_file(backup);
    }
    Ok(())
}

fn rollback_restore(moved: &[(PathBuf, PathBuf)], installed: &[PathBuf]) {
    for target in installed.iter().rev() {
        let _ = fs::remove_file(target);
    }
    for (target, backup) in moved.iter().rev() {
        let _ = fs::rename(backup, target);
    }
}

fn acquire_data_lock(data_dir: &Path) -> Result<fs::File, String> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(data_dir.join("server.lock"))
        .map_err(io_error)?;
    lock.try_lock_exclusive()
        .map_err(|_| "server data directory is already in use".to_string())?;
    Ok(lock)
}

fn temporary_sibling(path: &Path, label: &str) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or_else(|| "backup path has no file name".to_string())?
        .to_string_lossy();
    Ok(path.with_file_name(format!(".{name}.{label}-{}", uuid::Uuid::new_v4())))
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn io_error(error: std::io::Error) -> String {
    format!("server I/O failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn data_directory_allows_only_one_server_or_maintenance_operation() {
        let root = TempDir::new().unwrap();
        let first = acquire_data_lock(root.path()).unwrap();
        assert!(acquire_data_lock(root.path()).is_err());
        drop(first);
        acquire_data_lock(root.path()).unwrap();
    }
}
