use axum::extract::connect_info::IntoMakeServiceWithConnectInfo;
use std::{fs, future::IntoFuture, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::watch;
use zenith_relay_server::{
    backup,
    backup::acquire_data_lock,
    config::Config,
    http, jobs,
    state::AppState,
    store::{Store, Vault},
};

const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);
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
    fs::create_dir_all(&config.data_dir).map_err(|error| format!("server I/O failed: {error}"))?;
    let _data_lock = acquire_data_lock(&config.data_dir)?;
    if let [operation, path] = command.as_slice() {
        return match operation.as_str() {
            "--backup" => backup::backup(&config, std::path::Path::new(path)),
            "--restore" => backup::restore(&config, std::path::Path::new(path)),
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
