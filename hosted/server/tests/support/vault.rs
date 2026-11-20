use super::support::Cluster;
use sqlx::PgPool;
use std::{fs, path::Path};
use uuid::Uuid;
use zenith_relay_hosted::{
    config::Config,
    db,
    vault::{Error, Purpose, Vault},
};
#[path = "provision.rs"]
mod provision;

fn private(path: &Path, _mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(_mode)).unwrap();
    }
    assert!(path.exists());
}

pub async fn verify(
    c: &Cluster,
    cfg: &Path,
    config: &Config,
    origin: &str,
    db: &PgPool,
    runtime: &PgPool,
) {
    let grants = include_str!("../../vault-grants.sql").replace(":\"vault_role\"", "fixture_vault");
    sqlx::raw_sql(sqlx::AssertSqlSafe(grants))
        .execute(db)
        .await
        .unwrap();
    let worker = db::connect(config, &c.dsn("fixture_vault")).await.unwrap();
    assert!(db::check_runtime(&worker, origin).await.is_err());
    let dir = c.temp.path().join("private-blobs");
    fs::create_dir(&dir).unwrap();
    private(&dir, 0o700);
    let key = c.temp.path().join("private-wrapping-key");
    let wrong = c.temp.path().join("wrong-wrapping-key");
    for (file, byte) in [(&key, 83), (&wrong, 91)] {
        fs::write(file, [byte; 32]).unwrap();
        private(file, 0o600);
    }
    let cli = || {
        let mut cmd = c.cli("fixture_vault", cfg);
        cmd.args(["vault-check", "--vault-dir"]).arg(&dir);
        cmd
    };
    assert!(
        !cli()
            .env_remove("ZENITH_RELAY_HOSTED_VAULT_KEY_FILE")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(Vault::open(runtime, origin, &dir, &key).await.is_err());
    assert!(matches!(
        Vault::open(db, origin, &dir, &key).await,
        Err(Error::Role)
    ));
    let mut command = cli();
    command.env("ZENITH_RELAY_HOSTED_VAULT_KEY_FILE", &key);
    // Do not block this test's Tokio runtime: rejected roles have pending SQLx rollbacks.
    let checked = tokio::task::spawn_blocking(move || command.output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let open = || Vault::open(&worker, origin, &dir, &key);
    let vault = open().await.unwrap();
    provision::verify(&vault, db, runtime).await;
    assert!(Vault::open(&worker, origin, &dir, &wrong).await.is_err());
    #[cfg(unix)]
    {
        private(&key, 0o644);
        assert!(open().await.is_err());
        private(&key, 0o600);
    }
    let marker = dir.join("installation.zhv");
    let saved_marker = fs::read(&marker).unwrap();
    fs::remove_file(&marker).unwrap();
    assert!(open().await.is_err());
    assert!(!marker.exists());
    fs::write(&marker, &saved_marker).unwrap();
    private(&marker, 0o600);

    let id = Uuid::new_v4();
    let provider = Purpose::Provider;
    let proxy = Purpose::Proxy;
    let secret = b"synthetic provider fixture, not an API credential";
    let route = b"synthetic proxy fixture";
    let next = b"replacement fixture";
    assert_eq!(vault.put(id, provider, 0, secret).await.unwrap(), 1);
    assert_eq!(vault.put(id, proxy, 0, route).await.unwrap(), 1);
    fs::write(&marker, b"damaged fixture binding").unwrap();
    assert!(matches!(
        vault.read(id, proxy, 1).await,
        Err(Error::Storage)
    ));
    assert_eq!(vault.put(id, proxy, 1, route).await, Err(Error::Storage));
    assert_eq!(vault.collect_unreferenced().await, Err(Error::Storage));
    fs::write(&marker, &saved_marker).unwrap();
    assert_eq!(vault.put(id, provider, 0, secret).await, Err(Error::Stale));
    let generation = || {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT generation FROM relay_hosted.connection_secret_slots
             WHERE connection_id=$1 AND purpose='provider'",
        )
        .bind(id)
        .fetch_one(db)
    };
    let old = dir.join(format!("{}.zhv", generation().await.unwrap()));
    let sealed = fs::read(&old).unwrap();
    assert!(!sealed.windows(secret.len()).any(|w| w == secret));
    let reference = sqlx::query_scalar::<_, String>(
        "SELECT row_to_json(s)::text FROM relay_hosted.connection_secret_slots s WHERE connection_id=$1 AND purpose='provider'",
    ).bind(id).fetch_one(db).await.unwrap();
    assert!(!reference.contains(std::str::from_utf8(secret).unwrap()));
    assert!(
        sqlx::query("SELECT * FROM relay_hosted.connection_secret_slots")
            .execute(runtime)
            .await
            .is_err()
    );
    fs::write(&old, b"corrupt").unwrap();
    assert_eq!(
        vault.read(id, provider, 1).await.unwrap_err(),
        Error::Storage
    );
    fs::write(&old, &sealed).unwrap();

    // Fail the deferred reference commit after the new ciphertext is already durable.
    sqlx::raw_sql(
        "CREATE FUNCTION relay_hosted.fixture_reject() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN RAISE EXCEPTION 'synthetic commit failure'; END $$;
        CREATE CONSTRAINT TRIGGER fixture_stop AFTER UPDATE ON relay_hosted.connection_secret_slots
        DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION relay_hosted.fixture_reject()",
    )
    .execute(db)
    .await
    .unwrap();
    assert_eq!(vault.put(id, provider, 1, next).await, Err(Error::Database));
    assert_eq!(&*vault.read(id, provider, 1).await.unwrap(), secret);
    assert_eq!(vault.collect_unreferenced().await.unwrap(), 1);
    sqlx::raw_sql("DROP TRIGGER fixture_stop ON relay_hosted.connection_secret_slots; DROP FUNCTION relay_hosted.fixture_reject()")
        .execute(db).await.unwrap();

    let (one, two) = tokio::join!(
        vault.put(id, provider, 1, next),
        vault.put(id, provider, 1, next)
    );
    assert!(matches!(
        (one, two),
        (Ok(2), Err(Error::Stale)) | (Err(Error::Stale), Ok(2))
    ));
    assert_eq!(vault.read(id, provider, 1).await.unwrap_err(), Error::Stale);
    let restarted = open().await.unwrap();
    assert_eq!(&*restarted.read(id, provider, 2).await.unwrap(), next);
    assert_eq!(restarted.collect_unreferenced().await.unwrap(), 1);
    assert!(!old.exists());
    let live_path = dir.join(format!("{}.zhv", generation().await.unwrap()));
    fs::remove_file(&live_path).unwrap();
    assert_eq!(
        restarted.read(id, provider, 2).await.unwrap_err(),
        Error::Storage
    );
    assert_eq!(restarted.revoke(id, provider, 2).await.unwrap(), 3);
    assert_eq!(vault.put(id, provider, 3, next).await, Err(Error::Stale));
    assert_eq!(vault.put(id, provider, 2, next).await, Err(Error::Stale));
    assert_eq!(vault.read(id, provider, 3).await.unwrap_err(), Error::Stale);
    assert_eq!(&*restarted.read(id, proxy, 1).await.unwrap(), route);
    assert_eq!(restarted.collect_unreferenced().await.unwrap(), 0);
}
