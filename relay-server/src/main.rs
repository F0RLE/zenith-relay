use axum::extract::connect_info::IntoMakeServiceWithConnectInfo;
use std::{fs, future::IntoFuture, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
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
    let command = MaintenanceCommand::parse(std::env::args().skip(1).collect())?;
    fs::create_dir_all(&config.data_dir).map_err(|error| format!("server I/O failed: {error}"))?;
    let _data_lock = acquire_data_lock(&config.data_dir)?;
    if let Some(command) = command {
        return command.execute(&config);
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
        () = shutdown_signal() => {
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

#[derive(Debug)]
enum MaintenanceCommand {
    Backup(PathBuf),
    Restore(PathBuf),
}

impl MaintenanceCommand {
    fn parse(args: Vec<String>) -> Result<Option<Self>, String> {
        if args.is_empty() {
            return Ok(None);
        }
        if args.len() != 2 {
            return Err(Self::usage());
        }
        let command = match args[0].as_str() {
            "--backup" => Self::Backup(PathBuf::from(&args[1])),
            "--restore" => Self::Restore(PathBuf::from(&args[1])),
            _ => return Err(Self::usage()),
        };
        Ok(Some(command))
    }

    fn execute(self, config: &Config) -> Result<(), String> {
        match self {
            Self::Backup(path) => backup::backup(config, &path),
            Self::Restore(path) => backup::restore(config, &path),
        }
    }

    fn usage() -> String {
        "supported commands are --backup <dir> and --restore <dir>".to_string()
    }
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

#[cfg(test)]
mod tests {
    use super::MaintenanceCommand;

    #[test]
    fn maintenance_command_parser_keeps_server_start_as_the_default() {
        assert!(MaintenanceCommand::parse(Vec::new()).unwrap().is_none());
    }

    #[test]
    fn maintenance_command_parser_accepts_backup_and_restore() {
        assert!(MaintenanceCommand::parse(vec!["--backup".into(), "backup".into()]).is_ok());
        assert!(MaintenanceCommand::parse(vec!["--restore".into(), "backup".into()]).is_ok());
    }

    #[test]
    fn maintenance_command_parser_rejects_unknown_or_incomplete_commands() {
        for args in [
            vec!["--unknown".into(), "path".into()],
            vec!["--backup".into()],
            vec!["--restore".into(), "one".into(), "two".into()],
        ] {
            assert_eq!(
                MaintenanceCommand::parse(args).unwrap_err(),
                "supported commands are --backup <dir> and --restore <dir>"
            );
        }
    }
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
