use super::*;
use customer_reader::{Reader, setup_reader};
use refund_race::fixture::Merchant;
use zenith_relay_hosted::payments::{
    CreditError,
    refunds::{RefundError, submit_next},
};

const LATE_REQUEST: &str = "after-backup-dispatch";
const LATE_INVOICE: &str = "after-backup-credit";
const REFUND: i64 = 100_000;

type Fence = (String, i64, i64, i64, Option<Uuid>);

async fn fence_state(db: &PgPool) -> Fence {
    sqlx::query_as(
        "SELECT s.mode,s.revision,s.admission_epoch,i.authorization_epoch,s.restore_operation
         FROM relay_hosted.service_control s JOIN relay_hosted.installations i ON s.installation_id=i.id",
    )
    .fetch_one(db)
    .await
    .unwrap()
}

async fn key_workspace(worker: &PgPool, key: Uuid, bearer: &str) -> Option<Uuid> {
    sqlx::query_scalar("SELECT relay_hosted.qualified_key_workspace($1,$2,$3)")
        .bind(ORIGIN)
        .bind(key)
        .bind(digest(&SHA256, bearer.as_bytes()).as_ref())
        .fetch_one(worker)
        .await
        .unwrap()
}

async fn owner_mode(runtime: &PgPool) -> Option<String> {
    sqlx::query_scalar("SELECT mode FROM relay_hosted.owner_service_view($1,$2)")
        .bind(ORIGIN)
        .bind(refund_race::OWNER)
        .fetch_optional(runtime)
        .await
        .unwrap()
}

async fn key_revocation(admin: &PgPool, key: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT revoked_at::text FROM relay_hosted.customer_keys WHERE id=$1")
        .bind(key)
        .fetch_one(admin)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL with pg_basebackup and pg_verifybackup"]
async fn physical_restore_fences_missing_claims_and_keeps_obligations() {
    let f = Fixture::setup(true).await;
    setup_reader(&f).await;
    refund_race::owner_session(&f.admin).await;
    let customer = Reader::new(&f, f.space, 27).await;
    let (other_key, other_bearer) = issue(&f.admin, f.scope, f.space).await;
    let (installation, early_payment): (Uuid, Uuid) = sqlx::query_as(
        "SELECT i.id,p.id FROM relay_hosted.installations i CROSS JOIN relay_hosted.payment_intents p",
    )
    .fetch_one(&f.admin)
    .await
    .unwrap();
    let refund = refund_race::request(&customer, early_payment, REFUND).await;
    assert_eq!(
        refund_race::review(&customer.pool, refund, Uuid::new_v4(), "approve").await,
        "applied"
    );
    let queued_invoice: Uuid = sqlx::query_scalar(
        "INSERT INTO relay_hosted.payment_intents(workspace_id,operation_id,quote_id,merchant_binding_id,
            quoted_currency,quoted_amount_minor,credit_micro_usd,checkout_method_id)
         SELECT $1,gen_random_uuid(),gen_random_uuid(),merchant_binding_id,'USD',100,$2,id
         FROM relay_hosted.checkout_methods RETURNING id",
    )
    .bind(f.space)
    .bind(CREDIT)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    let late_payment =
        payment::invoice(&f.admin, f.space, f.merchant.binding, LATE_INVOICE, CREDIT).await;
    let held = Held::reserve(&f, f.key, &f.bearer).await;
    let before = fence_state(&f.admin).await;
    let reserved = held.amount + REFUND;
    let snapshot = f._cluster.base_backup();
    let copied_vault = snapshot.temp.path().join("encrypted-vault");
    fs::create_dir(&copied_vault).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&copied_vault, fs::Permissions::from_mode(0o700)).unwrap();
    }
    for entry in fs::read_dir(f._cluster.temp.path().join("vault")).unwrap() {
        let entry = entry.unwrap();
        assert!(entry.file_type().unwrap().is_file());
        fs::copy(entry.path(), copied_vault.join(entry.file_name())).unwrap();
    }

    // All three effects commit AFTER the backup. The merchant's effect
    // registry and the supplier's count are independent of the restored DB.
    let merchant = Merchant::new(f.merchant.binding, "fund-route-test".into());
    assert_eq!(submit_next(&f.pay, &merchant).await, Ok(Some(refund)));
    assert_eq!(merchant.submissions(), 1);
    let delivery = f.merchant.paid("after-backup-event", LATE_INVOICE);
    reconcile_paid(&f.pay, &f.merchant, &delivery)
        .await
        .unwrap();
    let response = f.submit(LATE_REQUEST, 1000, 100).send().await.unwrap();
    let late_request = request_id(&response);
    f.verify_purchase(response, f.b.item, 3500, 1600, "1600")
        .await;
    assert_eq!(f.sends(), (0, 1));
    payment::assert_wallet_with_holds(
        &f.admin,
        f.space,
        2 * CREDIT - reserved - 3500,
        held.amount,
        7,
    )
    .await;
    let revoked: String =
        sqlx::query_scalar("SELECT relay_hosted.revoke_customer_key($1,$2,$3,$4,$5)")
            .bind(ORIGIN)
            .bind(customer.provider)
            .bind([17_u8; 32].as_slice())
            .bind(customer.tag.as_slice())
            .bind(f.key)
            .fetch_one(&customer.pool)
            .await
            .unwrap();
    assert_eq!(revoked, "revoked");
    let revoked_at = key_revocation(&f.admin, f.key).await.unwrap();
    assert_eq!(key_workspace(&f.worker, f.key, &f.bearer).await, None);

    // Never run a second active authority. The replacement is a direct
    // PostgreSQL restore on a new loopback port, with no Relay listeners.
    f.server.task.abort();
    customer.pool.close().await;
    f.worker.close().await;
    f.pay.close().await;
    f.admin.close().await;
    f._cluster.stop();
    snapshot.start();
    let config = fixture_config();
    let boot_dsn = snapshot.dsn("fixture_boot");
    let boot = || db::connect(&config, &boot_dsn);
    let restore = boot().await.unwrap();
    let runtime = db::connect(&config, &snapshot.dsn("report_reader"))
        .await
        .unwrap();
    let worker = db::connect(&config, &snapshot.dsn("route_worker"))
        .await
        .unwrap();
    let pay = db::connect(&config, &snapshot.dsn("route_payment"))
        .await
        .unwrap();
    let vault_pool = db::connect(&config, &snapshot.dsn("route_vault"))
        .await
        .unwrap();
    let vault = Vault::open(
        &vault_pool,
        ORIGIN,
        &copied_vault,
        &f._cluster.temp.path().join("vault-key"),
    )
    .await
    .unwrap();
    assert!(vault.read(f.b.id, Purpose::Provider, 1).await.is_ok());
    let stale_router = inference::router(
        worker.clone(),
        ORIGIN,
        vault,
        OutboundPolicy::default(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(fence_state(&restore).await, before);
    assert_eq!(owner_mode(&runtime).await.as_deref(), Some("qualification"));
    assert_eq!(key_revocation(&restore, f.key).await, None);
    assert_eq!(
        key_workspace(&worker, f.key, &f.bearer).await,
        Some(f.space)
    );
    let lost_claims: (bool, bool) = sqlx::query_as(
        "SELECT NOT EXISTS(SELECT FROM relay_hosted.text_requests WHERE id=$1),
                NOT EXISTS(SELECT FROM relay_hosted.journal_transactions WHERE payment_id=$2)",
    )
    .bind(late_request)
    .bind(late_payment)
    .fetch_one(&restore)
    .await
    .unwrap();
    assert_eq!(lost_claims, (true, true));

    let cfg = snapshot.temp.path().join("restore.toml");
    let bind = format!("127.0.0.1:{}", support::port());
    fs::write(
        &cfg,
        include_str!("../../config.example.toml").replace("127.0.0.1:1443", &bind),
    )
    .unwrap();
    let operation = Uuid::new_v4();
    let fence = |role, installation: Uuid, operation: Uuid| {
        snapshot
            .cli(role, &cfg)
            .args([
                "restore-fence",
                "--installation",
                &installation.to_string(),
                "--operation",
                &operation.to_string(),
            ])
            .output()
            .unwrap()
    };
    let wrong = fence("fixture_boot", Uuid::new_v4(), operation);
    assert!(!wrong.status.success());
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("installation ID does not match"));
    assert!(
        !fence("report_reader", installation, operation)
            .status
            .success()
    );
    assert_eq!(fence_state(&restore).await, before);
    for _ in 0..2 {
        let out = fence("fixture_boot", installation, operation);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert!(
        !fence("fixture_boot", installation, Uuid::new_v4())
            .status
            .success()
    );
    let fenced = (
        "maintenance".into(),
        before.1 + 1,
        before.2 + 1,
        before.3 + 1,
        Some(operation),
    );
    assert_eq!(fence_state(&restore).await, fenced);
    for ready in [
        db::check_runtime(&runtime, ORIGIN).await,
        inference::check_worker(&worker, ORIGIN).await,
    ] {
        assert_eq!(ready, Err("installation restore is fenced"));
    }
    let mut web = support::Process(
        snapshot
            .cli("report_reader", &cfg)
            .arg("serve")
            .spawn()
            .unwrap(),
    );
    let status = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Some(status) = web.0.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("serve must exit before binding");
    assert!(!status.success());
    assert_eq!(owner_mode(&runtime).await, None);
    assert_eq!(key_workspace(&worker, f.key, &f.bearer).await, None);
    assert_eq!(key_workspace(&worker, other_key, &other_bearer).await, None);
    assert_eq!(
        held.claim_on(&worker, f.space, f.b.id, f.b.item, 1).await,
        "denied"
    );

    // Replay a known revocation from outside the backup; absence of an entry
    // is not permission to restore any other pre-backup credential.
    for _ in 0..2 {
        let applied: String = sqlx::query_scalar(
            "UPDATE relay_hosted.customer_keys SET revoked_at=coalesce(revoked_at,$2::text::timestamptz)
             WHERE id=$1 RETURNING revoked_at::text",
        )
        .bind(f.key)
        .bind(&revoked_at)
        .fetch_one(&restore)
        .await
        .unwrap();
        assert_eq!(applied, revoked_at);
    }
    let stale = Listener::start(stale_router).await;
    let replay = f
        .http
        .post(format!("{}/v1/responses", stale.url))
        .bearer_auth(&f.bearer)
        .header("idempotency-key", LATE_REQUEST)
        .json(&json!({"model": MODEL, "input": "x".repeat(1000), "max_output_tokens": 100}))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 503);
    assert_eq!(
        replay.json::<Value>().await.unwrap()["error"]["code"],
        "relay_unavailable"
    );
    assert_eq!(f.sends(), (0, 1));
    drop(stale);

    // No new merchant claims, and do not acknowledge automatic credit replay
    // while the newer obligations have not been reconciled.
    let err = sqlx::query("SELECT * FROM relay_hosted.claim_checkout_invoice($1)")
        .bind(f.merchant.binding)
        .execute(&pay)
        .await
        .unwrap_err();
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("RH003")
    );
    assert_eq!(
        submit_next(&pay, &merchant).await,
        Err(RefundError::Storage)
    );
    assert_eq!(merchant.submissions(), 1);
    let early = f
        .merchant
        .sign("paid", "settled", "fund-route-test", "USD", CREDIT / 10_000);
    for callback in [&early, &delivery, &delivery] {
        assert_eq!(
            reconcile_paid(&pay, &f.merchant, callback).await,
            Err(CreditError::Unavailable)
        );
    }
    payment::assert_wallet_with_holds(&restore, f.space, CREDIT - reserved, reserved, 3).await;
    let obligations: (bool, bool, bool, bool) = sqlx::query_as(
        "SELECT EXISTS(SELECT FROM relay_hosted.text_requests WHERE id=$1 AND execution='reserved' AND held=$2),
                EXISTS(SELECT FROM relay_hosted.payment_refunds WHERE id=$3 AND state='queued' AND claimed_at IS NULL),
                EXISTS(SELECT FROM relay_hosted.payment_intents WHERE id=$4 AND checkout_claimed_at IS NULL),
                EXISTS(SELECT FROM relay_hosted.payment_intents WHERE id=$5)",
    )
    .bind(held.id)
    .bind(held.amount)
    .bind(refund)
    .bind(queued_invoice)
    .bind(late_payment)
    .fetch_one(&restore)
    .await
    .unwrap();
    assert_eq!(obligations, (true, true, true, true));
    let reopening = sqlx::query(
        "UPDATE relay_hosted.service_control SET mode='qualification',qualification_scope_ref=$1",
    )
    .bind(f.scope)
    .execute(&restore)
    .await
    .unwrap_err();
    assert_eq!(
        reopening.as_database_error().unwrap().constraint(),
        Some("restore_stays_in_maintenance")
    );

    for pool in [&restore, &runtime, &worker, &vault_pool, &pay] {
        pool.close().await;
    }
    snapshot.stop();
    snapshot.start();
    let restore = boot().await.unwrap();
    let upgrade = db::upgrade_plan(&restore, ORIGIN).await.unwrap();
    assert!(upgrade.pending.is_empty());
    db::migrate(&restore, ORIGIN).await.unwrap();
    assert_eq!(fence_state(&restore).await, fenced);
    assert_eq!(
        db::closed_setup(&restore, ORIGIN).await,
        Err("installation restore is fenced")
    );
    assert_eq!(key_revocation(&restore, f.key).await, Some(revoked_at));
    payment::assert_wallet_with_holds(&restore, f.space, CREDIT - reserved, reserved, 3).await;
}
