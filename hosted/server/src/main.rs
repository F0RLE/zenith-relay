use std::{
    env, future::IntoFuture, net::SocketAddr, path::Path, process::ExitCode, time::Duration,
};
use tokio::net::TcpListener;
use zenith_relay_hosted::{
    config, db, http, inference, operations, qualification, setup, transfers,
    vault::{ImportRoute, ProvisionResult, Purpose, Vault},
};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("relay-hosted: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), &'static str> {
    const USAGE: &str = "usage: migrate | upgrade-check | restore-fence --installation UUID --operation UUID | serve [--web-dist PATH] | setup-owner --web-dist PATH --code-dir PRIVATE_DIR | vault-check --vault-dir PRIVATE_DIR | vault-add --source UUID --purpose provider|proxy --secret-file PRIVATE_FILE --vault-dir PRIVATE_DIR | vault-provision --operation UUID --source UUID --provider-file PRIVATE_FILE (--direct | --proxy-file PRIVATE_FILE) --vault-dir PRIVATE_DIR | serve-transfers --bind LOOPBACK:PORT --vault-dir PRIVATE_DIR | qualify-text --print-scope | qualify-text --profile PATH --vault-dir PRIVATE_DIR | serve-text --bind LOOPBACK:PORT --vault-dir PRIVATE_DIR | reconcile-text";
    enum Action<'a> {
        Migrate,
        UpgradeCheck,
        RestoreFence(uuid::Uuid, uuid::Uuid),
        Serve(Option<&'a Path>),
        Setup(&'a Path, &'a Path),
        Vault(&'a Path),
        VaultAdd(uuid::Uuid, Purpose, &'a Path, &'a Path),
        Provision(
            uuid::Uuid,
            uuid::Uuid,
            &'a Path,
            ImportRoute<&'a Path>,
            &'a Path,
        ),
        Qualify(&'a Path, &'a Path),
        Scope,
        Text(SocketAddr, &'a Path),
        Transfers(SocketAddr, &'a Path),
        Reconcile,
    }
    let args: Vec<_> = env::args_os().skip(1).collect();
    let action = match args.as_slice() {
        [flag] if flag == "--help" || flag == "-h" => {
            println!("{USAGE}");
            return Ok(());
        }
        [op] if op == "migrate" => Action::Migrate,
        [op] if op == "upgrade-check" => Action::UpgradeCheck,
        [op, id_flag, installation, op_flag, operation]
            if op == "restore-fence" && id_flag == "--installation" && op_flag == "--operation" =>
        {
            Action::RestoreFence(
                installation
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or(USAGE)?,
                operation
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or(USAGE)?,
            )
        }
        [
            op,
            op_flag,
            operation,
            src_flag,
            source,
            key_flag,
            key,
            tail @ ..,
        ] if op == "vault-provision"
            && op_flag == "--operation"
            && src_flag == "--source"
            && key_flag == "--provider-file" =>
        {
            let (route, dir) = match tail {
                [direct, v, dir] if direct == "--direct" && v == "--vault-dir" => {
                    (ImportRoute::Direct, dir)
                }
                [proxy, path, v, dir] if proxy == "--proxy-file" && v == "--vault-dir" => {
                    (ImportRoute::Proxy(Path::new(path)), dir)
                }
                _ => return Err(USAGE),
            };
            Action::Provision(
                operation
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or(USAGE)?,
                source.to_str().and_then(|s| s.parse().ok()).ok_or(USAGE)?,
                Path::new(key),
                route,
                Path::new(dir),
            )
        }
        [op] if op == "reconcile-text" => Action::Reconcile,
        [op, flag] if op == "qualify-text" && flag == "--print-scope" => Action::Scope,
        [op, bind_flag, bind, vault_flag, dir]
            if (op == "serve-text" || op == "serve-transfers")
                && bind_flag == "--bind"
                && vault_flag == "--vault-dir" =>
        {
            let addr: SocketAddr = bind.to_str().and_then(|v| v.parse().ok()).ok_or(USAGE)?;
            if !addr.ip().is_loopback() || addr.port() == 0 {
                return Err("worker requires loopback and an explicit port");
            }
            if op == "serve-text" {
                Action::Text(addr, Path::new(dir))
            } else {
                Action::Transfers(addr, Path::new(dir))
            }
        }
        [op, profile_flag, profile, flag, dir]
            if op == "qualify-text" && profile_flag == "--profile" && flag == "--vault-dir" =>
        {
            Action::Qualify(Path::new(profile), Path::new(dir))
        }
        [
            op,
            source_flag,
            source,
            purpose_flag,
            purpose,
            secret_flag,
            secret,
            flag,
            dir,
        ] if op == "vault-add"
            && source_flag == "--source"
            && purpose_flag == "--purpose"
            && secret_flag == "--secret-file"
            && flag == "--vault-dir" =>
        {
            let purpose = match purpose.to_str() {
                Some("provider") => Purpose::Provider,
                Some("proxy") => Purpose::Proxy,
                _ => return Err(USAGE),
            };
            Action::VaultAdd(
                source.to_str().and_then(|s| s.parse().ok()).ok_or(USAGE)?,
                purpose,
                Path::new(secret),
                Path::new(dir),
            )
        }
        [op, flag, dir] if op == "vault-check" && flag == "--vault-dir" => {
            Action::Vault(Path::new(dir))
        }
        [op, web, dist, code, dir]
            if op == "setup-owner" && web == "--web-dist" && code == "--code-dir" =>
        {
            Action::Setup(Path::new(dist), Path::new(dir))
        }
        [op] if op == "serve" => Action::Serve(None),
        [op, flag, path] if op == "serve" && flag == "--web-dist" => {
            Action::Serve(Some(Path::new(path)))
        }
        _ => return Err(USAGE),
    };
    let c = config::Config::load()?;
    let pool = db::connect(&c, &config::database_url()?).await?;
    match action {
        Action::Migrate => db::migrate(&pool, &c.server.public_origin).await?,
        Action::UpgradeCheck => {
            let plan = db::upgrade_plan(&pool, &c.server.public_origin).await?;
            println!(
                "Installed schema: {}. Target schema: {}.",
                plan.installed, plan.target
            );
            if plan.pending.is_empty() {
                println!("No pending migrations.");
            } else {
                let pending: Vec<_> = plan.pending.iter().map(ToString::to_string).collect();
                println!("Pending migrations: {}.", pending.join(", "));
            }
        }
        Action::RestoreFence(installation, operation) => {
            db::fence_restore(&pool, &c.server.public_origin, installation, operation).await?;
            println!("Restore fenced. Keep ingress closed for reconciliation.");
        }
        Action::Serve(web) => serve(c, &pool, web).await?,
        Action::Reconcile => {
            inference::check_worker(&pool, &c.server.public_origin).await?;
            let n = inference::reconcile_due(&pool)
                .await
                .map_err(|_| "request reconciliation unavailable")?;
            println!("Reconciled {n} due requests.");
        }
        Action::Text(bind, dir) => {
            inference::check_worker(&pool, &c.server.public_origin).await?;
            let mut reconciliation = operations::Reconciliation::default();
            reconciliation
                .run(&pool)
                .await
                .map_err(|_| "request reconciliation unavailable")?;
            let vault = separate_vault(&c, dir).await?;
            let app = inference::router(
                pool.clone(),
                &c.server.public_origin,
                vault,
                c.network.outbound_policy,
                c.diagnostics.limits(),
            )
            .await?;
            tokio::select! {
                r=listen(bind,app)=>r?,
                r=recover(&pool,&c.server.public_origin,reconciliation)=>r?,
            }
        }
        Action::Transfers(bind, dir) => {
            let vault = open_vault(&pool, &c, dir).await?;
            let app = transfers::router(&c.server.public_origin, vault).await?;
            listen(bind, app).await?;
        }
        Action::Qualify(profile, dir) => {
            let scope = qualification::scope(&pool, &c.server.public_origin).await?;
            let vault = separate_vault(&c, dir).await?;
            qualification::check(scope, &vault, profile, &c.network.outbound_policy).await?;
            println!("Verified model recorded.");
        }
        Action::Scope => println!(
            "{}",
            qualification::scope(&pool, &c.server.public_origin)
                .await?
                .id()
        ),
        Action::Provision(operation, source, key, route, dir) => {
            let vault = open_vault(&pool, &c, dir).await?;
            match vault
                .provision_files(operation, source, key, route)
                .await
                .map_err(|e| e.message())?
            {
                ProvisionResult::Created => println!("Source and route stored."),
                ProvisionResult::PreviouslyCommitted => {
                    println!("Import previously committed; no changes.")
                }
            }
        }
        Action::VaultAdd(id, purpose, file, dir) => {
            let vault = open_vault(&pool, &c, dir).await?;
            vault
                .add_from_file(id, purpose, file)
                .await
                .map_err(|e| e.message())?;
            println!("New secret slot stored at revision 1.");
        }
        Action::Setup(dist, dir) => setup::run(&c, &pool, dist, dir).await?,
        Action::Vault(dir) => {
            open_vault(&pool, &c, dir).await?;
            println!("Vault binding verified.");
        }
    }
    pool.close().await;
    Ok(())
}

async fn open_vault(
    pool: &sqlx::PgPool,
    c: &config::Config,
    dir: &Path,
) -> Result<Vault, &'static str> {
    let key = env::var_os("ZENITH_RELAY_HOSTED_VAULT_KEY_FILE")
        .ok_or("private vault key file required")?;
    Vault::open(pool, &c.server.public_origin, dir, Path::new(&key))
        .await
        .map_err(|e| e.message())
}
async fn separate_vault(c: &config::Config, dir: &Path) -> Result<Vault, &'static str> {
    let pool = db::connect(
        c,
        &config::role_url("ZENITH_RELAY_HOSTED_VAULT_DATABASE_URL_FILE")?,
    )
    .await?;
    open_vault(&pool, c, dir).await
}

async fn serve(
    c: config::Config,
    pool: &sqlx::PgPool,
    web: Option<&Path>,
) -> Result<(), &'static str> {
    let origin = c.server.public_origin.clone();
    db::check_runtime(pool, &origin).await?;
    let app = http::router(
        http::StateData {
            auth: zenith_relay_hosted::auth::Admin::configured(&c)?,
            customer: zenith_relay_hosted::identity::Customer::configured(&c, pool, web).await?,
            pool: pool.clone(),
            origin: c.server.public_origin,
            display_name: c.branding.name,
            policies: c.policies,
            capture: c.diagnostics.limits(),
        },
        web,
    )?;
    let listener = bind_listener(c.server.bind).await?;
    sqlx::query("SELECT relay_hosted.reset_owner_browser_sessions($1)")
        .bind(&origin)
        .execute(pool)
        .await
        .map_err(|_| "could not invalidate previous owner browser sessions")?;
    tokio::select! {
        result = serve_bound(listener, app) => result,
        never = operations::prune_projections(pool) => match never {},
    }
}

async fn recover(
    pool: &sqlx::PgPool,
    origin: &str,
    mut reconciliation: operations::Reconciliation,
) -> Result<(), &'static str> {
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        db::closed_setup(pool, origin).await?;
        reconciliation
            .run(pool)
            .await
            .map_err(|_| "request reconciliation unavailable")?;
    }
}

async fn listen(bind: SocketAddr, app: axum::Router) -> Result<(), &'static str> {
    serve_bound(bind_listener(bind).await?, app).await
}

async fn bind_listener(bind: SocketAddr) -> Result<TcpListener, &'static str> {
    TcpListener::bind(bind)
        .await
        .map_err(|_| "cannot bind listener")
}

async fn serve_bound(listener: TcpListener, app: axum::Router) -> Result<(), &'static str> {
    let address = listener
        .local_addr()
        .map_err(|_| "cannot read listener address")?;
    eprintln!("relay-hosted: listening on {address}");
    let (stop, signal) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = signal.await;
        })
        .into_future();
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result,
        () = shutdown() => {
            let _ = stop.send(());
            tokio::time::timeout(Duration::from_secs(5), server).await.map_err(|_| "shutdown drain timed out")?
        }
    }.map_err(|_| "HTTP server stopped unexpectedly")?;
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! { _ = term.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
