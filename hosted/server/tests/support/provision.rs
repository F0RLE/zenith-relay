use sqlx::PgPool;
use uuid::Uuid;
use zenith_relay_hosted::vault::{Error, ImportRoute, ProvisionResult, Purpose, Vault};

async fn qualifies(db: &PgPool, source: Uuid, proxy_revision: i64, proxied: bool) -> bool {
    sqlx::query_scalar(
        "SELECT relay_hosted.text_source_matches(id,$1,1,$2,$3) FROM relay_hosted.installations",
    )
    .bind(source)
    .bind(proxy_revision)
    .bind(proxied)
    .fetch_one(db)
    .await
    .unwrap()
}

pub(super) async fn verify(vault: &Vault, db: &PgPool, runtime: &PgPool) {
    let source = Uuid::new_v4();
    let op = Uuid::new_v4();
    let provider = b"fixture-provider";
    let proxy = b"http://operator:fixture@127.0.0.1:15311/";
    let submit = || vault.provision_api(op, source, provider, ImportRoute::Proxy(proxy));
    let (a, b) = tokio::join!(submit(), submit());
    use ProvisionResult::{Created, PreviouslyCommitted};
    assert!(matches!(
        (a, b),
        (Ok(Created), Ok(PreviouslyCommitted)) | (Ok(PreviouslyCommitted), Ok(Created))
    ));
    assert_eq!(
        vault
            .read(source, Purpose::Provider, 1)
            .await
            .unwrap()
            .as_slice(),
        provider
    );
    assert_eq!(
        vault
            .read(source, Purpose::Proxy, 1)
            .await
            .unwrap()
            .as_slice(),
        proxy
    );
    for mismatch in [
        vault
            .provision_api(op, source, provider, ImportRoute::Direct)
            .await,
        vault
            .provision_api(op, source, b"changed", ImportRoute::Proxy(proxy))
            .await,
        vault
            .provision_api(op, Uuid::new_v4(), provider, ImportRoute::Proxy(proxy))
            .await,
        vault
            .provision_api(Uuid::new_v4(), source, provider, ImportRoute::Proxy(proxy))
            .await,
    ] {
        assert_eq!(mismatch, Err(Error::Conflict));
    }
    assert!(qualifies(db, source, 1, true).await);
    vault.revoke(source, Purpose::Proxy, 1).await.unwrap();
    assert!(!qualifies(db, source, 2, false).await);
    assert!(!qualifies(db, source, 1, true).await);
    assert_eq!(submit().await, Ok(PreviouslyCommitted));
    assert_eq!(
        vault.read(source, Purpose::Proxy, 1).await.err(),
        Some(Error::Stale)
    );
    let direct = Uuid::new_v4();
    assert_eq!(
        vault
            .provision_api(Uuid::new_v4(), direct, provider, ImportRoute::Direct)
            .await,
        Ok(Created)
    );
    assert_eq!(
        vault.read(direct, Purpose::Proxy, 1).await.err(),
        Some(Error::Stale)
    );
    let denied = sqlx::query("SELECT payload_tag FROM relay_hosted.static_source_provisions")
        .execute(runtime)
        .await
        .unwrap_err();
    assert_eq!(
        denied.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    vault.collect_unreferenced().await.unwrap();

    let failed = Uuid::new_v4();
    let failed_op = Uuid::new_v4();
    let apply = || vault.provision_api(failed_op, failed, provider, ImportRoute::Proxy(proxy));
    assert_eq!(
        vault
            .provision_api(failed_op, failed, provider, ImportRoute::Proxy(b""))
            .await,
        Err(Error::Invalid)
    );
    sqlx::raw_sql("CREATE FUNCTION relay_hosted.fixture_import_abort() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN RAISE EXCEPTION 'synthetic failure at commit'; END $$;
        CREATE CONSTRAINT TRIGGER fixture_import_abort AFTER INSERT ON relay_hosted.static_source_provisions
        DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION relay_hosted.fixture_import_abort()")
        .execute(db).await.unwrap();
    assert_eq!(apply().await, Err(Error::Database));
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT
        (SELECT count(*) FROM relay_hosted.connection_secret_slots WHERE connection_id=$1),
        (SELECT count(*) FROM relay_hosted.static_source_provisions WHERE operation_id=$2)",
    )
    .bind(failed)
    .bind(failed_op)
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(counts, (0, 0));
    assert_eq!(vault.collect_unreferenced().await.unwrap(), 2);
    sqlx::raw_sql(
        "DROP TRIGGER fixture_import_abort ON relay_hosted.static_source_provisions;
        DROP FUNCTION relay_hosted.fixture_import_abort()",
    )
    .execute(db)
    .await
    .unwrap();
    assert_eq!(apply().await, Ok(Created));
}
