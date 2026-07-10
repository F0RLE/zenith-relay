use axum::extract::connect_info::IntoMakeServiceWithConnectInfo;
use serde::Serialize;
use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use zenith_relay_server::{
    config::Config,
    http, jobs, state,
    state::AppState,
    store::{Store, Vault},
};

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

    fs::create_dir_all(&config.data_dir).map_err(io_error)?;
    let store = Arc::new(Store::open(config.data_dir.join("relay.sqlite"))?);
    let vault = Arc::new(Vault::open(
        &config.data_dir.join("vault"),
        config.vault_key,
    )?);
    let state = AppState::new(config.clone(), store, vault)?;
    state.rebuild_runtime().await?;
    jobs::start(state.clone());
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|error| format!("failed to bind {}: {error}", config.bind))?;
    let service: IntoMakeServiceWithConnectInfo<_, SocketAddr> =
        http::router(state).into_make_service_with_connect_info();
    axum::serve(listener, service)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| format!("server failed: {error}"))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupManifest {
    format: &'static str,
    version: u32,
    created_at_ms: u64,
}

fn backup(config: &Config, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err("backup destination already exists".to_string());
    }
    fs::create_dir_all(destination).map_err(io_error)?;
    let store = Store::open(config.data_dir.join("relay.sqlite"))?;
    store.backup_to(&destination.join("relay.sqlite"))?;
    let vault = config.data_dir.join("vault").join("secrets.enc");
    if vault.exists() {
        fs::create_dir_all(destination.join("vault")).map_err(io_error)?;
        fs::copy(vault, destination.join("vault").join("secrets.enc")).map_err(io_error)?;
    }
    let manifest = serde_json::to_vec_pretty(&BackupManifest {
        format: "zenith-relay-server-backup",
        version: 1,
        created_at_ms: state::now_ms(),
    })
    .map_err(|_| "backup manifest serialization failed".to_string())?;
    fs::write(destination.join("manifest.json"), manifest).map_err(io_error)?;
    Ok(())
}

fn restore(config: &Config, source: &Path) -> Result<(), String> {
    let manifest = fs::read(source.join("manifest.json")).map_err(io_error)?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&manifest).map_err(|_| "backup manifest is invalid".to_string())?;
    if manifest.get("format").and_then(|value| value.as_str()) != Some("zenith-relay-server-backup")
    {
        return Err("backup format is not supported".to_string());
    }
    let database = source.join("relay.sqlite");
    if !database.is_file() {
        return Err("backup database is missing".to_string());
    }
    fs::create_dir_all(&config.data_dir).map_err(io_error)?;
    restore_file(&database, &config.data_dir.join("relay.sqlite"))?;
    let vault = source.join("vault").join("secrets.enc");
    if vault.is_file() {
        let target = config.data_dir.join("vault").join("secrets.enc");
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        restore_file(&vault, &target)?;
    }
    Store::open(config.data_dir.join("relay.sqlite"))?;
    Vault::open(&config.data_dir.join("vault"), config.vault_key)?;
    Ok(())
}

fn restore_file(source: &Path, target: &Path) -> Result<(), String> {
    let backup = PathBuf::from(format!("{}.pre-restore", target.display()));
    if backup.exists() {
        fs::remove_file(&backup).map_err(io_error)?;
    }
    if target.exists() {
        fs::rename(target, &backup).map_err(io_error)?;
    }
    if let Err(error) = fs::copy(source, target) {
        let _ = fs::remove_file(target);
        if backup.exists() {
            let _ = fs::rename(&backup, target);
        }
        return Err(io_error(error));
    }
    if backup.exists() {
        fs::remove_file(backup).map_err(io_error)?;
    }
    Ok(())
}

fn io_error(error: std::io::Error) -> String {
    format!("server I/O failed: {error}")
}
