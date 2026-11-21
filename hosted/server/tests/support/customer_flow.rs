use crate::{payment, support};
#[path = "checkout_flow.rs"]
mod checkout;
#[path = "device.rs"]
mod device;
#[path = "funded_fixture.rs"]
mod funded;
#[path = "service_notice.rs"]
mod notices;
#[path = "refund_flow.rs"]
mod refunds;

use sqlx::PgPool;
use std::{
    fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use support::{Cluster, Process};
use uuid::Uuid;
use zenith_relay_hosted::{
    access::qualified_workspace,
    config::Config,
    db,
    payments::{CreditError, reconcile_paid},
};

pub(super) async fn checkpoint(browser: &mut Process, path: &Path) {
    tokio::time::timeout(Duration::from_secs(75), async {
        while !path.is_file() {
            assert!(
                browser.0.try_wait().unwrap().is_none(),
                "customer browser exited before checkpoint"
            );
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("customer journey timed out");
}

pub(super) async fn age_current_passkey(db: &PgPool) {
    let aged = sqlx::query(
        "UPDATE relay_hosted.admin_sessions SET created_at=created_at-interval '3 minutes'
         WHERE verifier=(SELECT verifier FROM relay_hosted.admin_sessions ORDER BY created_at DESC LIMIT 1)",
    )
    .execute(db)
    .await
    .unwrap();
    assert_eq!(aged.rows_affected(), 1);
}

pub async fn journey(with_inference: bool) {
    let c = Cluster::new();
    let admin = PgPool::connect(&c.dsn("fixture_boot")).await.unwrap();
    c.create_logins(
        &admin,
        &[
            "fixture_migrator",
            "fixture_setup",
            "fixture_runtime",
            "fixture_payments",
            "fixture_vault",
            "fixture_inference",
            "fixture_qualifier",
        ],
    )
    .await;
    sqlx::query("GRANT CREATE ON DATABASE postgres TO fixture_migrator")
        .execute(&admin)
        .await
        .unwrap();
    let port = support::port();
    let issuer_port = (0..).map(|_| support::port()).find(|p| *p != port).unwrap();
    let origin = format!("http://localhost:{port}");
    let issuer = format!("http://127.0.0.1:{issuer_port}");
    let provider = Uuid::new_v4();
    let cfg = c.temp.path().join("identity.toml");
    let mut toml = include_str!("../../config.example.toml")
        .replace("1443", &port.to_string())
        .replace("# support = ", "support = ")
        + &format!(
            "\n[customer_oidc]\nid=\"{provider}\"\nissuer=\"{issuer}\"\nclient_id=\"relay-fixture\"\n\
         authorization_endpoint=\"{issuer}/authorize\"\ntoken_endpoint=\"{issuer}/token\"\njwks_uri=\"{issuer}/keys\"\n"
        );
    if with_inference {
        toml.push_str(
            "\n[diagnostics.capture]\nmax_events=16\nmax_bytes=4096\nmax_events_per_second=16\n",
        );
    }
    fs::write(&cfg, &toml).unwrap();
    let config = Config::parse(&toml).unwrap();
    let migrator = db::connect(&config, &c.dsn("fixture_migrator"))
        .await
        .unwrap();
    db::migrate(&migrator, &origin).await.unwrap();
    sqlx::raw_sql(
        "GRANT USAGE ON SCHEMA relay_hosted TO fixture_runtime,fixture_setup;
         GRANT SELECT ON relay_hosted.closed_installation,relay_hosted._sqlx_migrations TO fixture_runtime,fixture_setup;
         GRANT SELECT ON relay_hosted.pending_owner TO fixture_setup;
         GRANT EXECUTE ON FUNCTION relay_hosted.bind_first_owner(uuid,bigint,text,uuid,bytea,jsonb,bigint) TO fixture_setup;",
    ).execute(&migrator).await.unwrap();
    for sql in [
        include_str!("../../runtime-grants.sql").replace(":\"runtime_role\"", "fixture_runtime"),
        include_str!("../../payment-grants.sql").replace(":\"payment_role\"", "fixture_payments"),
        include_str!("../../vault-grants.sql").replace(":\"vault_role\"", "fixture_vault"),
        include_str!("../../inference-grants.sql")
            .replace(":\"inference_role\"", "fixture_inference"),
        include_str!("../../qualification-grants.sql")
            .replace(":\"qualification_role\"", "fixture_qualifier"),
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&migrator)
            .await
            .unwrap();
    }
    let code_dir = c.temp.path().join("enrollment");
    let exchange = c.temp.path().join("checkpoints");
    let key_file = c.temp.path().join("digest-key");
    fs::create_dir(&code_dir).unwrap();
    fs::create_dir(&exchange).unwrap();
    fs::write(&key_file, [31; 32]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&code_dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&key_file, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web");
    let dist = web.join("dist");
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let mut setup = Process(
        c.cli("fixture_setup", &cfg)
            .arg("setup-owner")
            .arg("--web-dist")
            .arg(&dist)
            .arg("--code-dir")
            .arg(&code_dir)
            .spawn()
            .unwrap(),
    );
    support::ready(&mut setup, &http, &format!("{origin}/setup/owner")).await;
    let code = support::fixture_files::setup_code(&code_dir);
    let mut browser = Process(
        Command::new("bun")
            .current_dir(&web)
            .arg("tests/owner-passkey.mjs")
            .arg(&origin)
            .arg(code)
            .arg("customer-identity")
            .arg(&issuer)
            .arg(&exchange)
            .arg(if with_inference { "funded" } else { "identity" })
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            if let Some(exit) = setup.0.try_wait().unwrap() {
                assert!(exit.success());
                break;
            }
            assert!(browser.0.try_wait().unwrap().is_none());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let serve = || {
        Process(
            c.cli("fixture_runtime", &cfg)
                .env("ZENITH_RELAY_HOSTED_DIGEST_KEY_FILE", &key_file)
                .arg("serve")
                .arg("--web-dist")
                .arg(&dist)
                .spawn()
                .unwrap(),
        )
    };
    let mut server = serve();
    let config_url = format!("{origin}/api/hosted/v1/public/config");
    support::ready(&mut server, &http, &config_url).await;
    let paired = device::pair(&mut browser, &exchange, &origin, &admin).await;
    checkpoint(&mut browser, &exchange.join("review")).await;
    let counts = || {
        sqlx::query_as::<_,(i64,i64,i64,i64)>(
        "SELECT (SELECT count(*) FROM relay_hosted.users), (SELECT count(*) FROM relay_hosted.workspaces),
         (SELECT count(*) FROM relay_hosted.wallets), (SELECT count(*) FROM relay_hosted.identity_candidates)",
    ).fetch_one(&admin)
    };
    assert_eq!(counts().await.unwrap(), (1, 0, 0, 2));
    age_current_passkey(&admin).await;
    fs::write(exchange.join("approve"), []).unwrap();
    let runtime = db::connect(&config, &c.dsn("fixture_runtime"))
        .await
        .unwrap();
    let notice = if with_inference {
        None
    } else {
        Some(
            notices::interrupt_publication(&admin, &runtime, &origin, &mut browser, &exchange)
                .await,
        )
    };
    checkpoint(&mut browser, &exchange.join("customers")).await;
    let [a, b]: [Uuid; 2] =
        zenith_relay_core::bounded_json::from_slice(&fs::read(exchange.join("customers")).unwrap())
            .unwrap();
    assert_ne!(a, b);
    assert_eq!(counts().await.unwrap(), (3, 2, 2, 0));
    let confirmations = || {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT count(*),count(confirmed_by_passkey_id) FROM relay_hosted.configuration_revisions",
        )
        .fetch_one(&admin)
    };
    assert_eq!(confirmations().await.unwrap(), (1, 1));
    payment::assert_wallet(&admin, a, 0, 0).await;
    payment::assert_wallet(&admin, b, 0, 0).await;
    let worker = db::connect(&config, &c.dsn("fixture_payments"))
        .await
        .unwrap();
    let mut merchant =
        payment::Adapter::new(&admin, "sandbox", payment::Adapter::signing_key()).await;
    if with_inference {
        payment::invoice(&admin, a, merchant.binding, "customer-top-up", 10_000_000).await;
        let rejected = merchant.paid("paid", "customer-top-up").tamper();
        assert_eq!(
            reconcile_paid(&worker, &merchant, &rejected).await,
            Err(CreditError::Unverified)
        );
        let receipt = reconcile_paid(
            &worker,
            &merchant,
            &merchant.paid("paid", "customer-top-up"),
        )
        .await
        .unwrap();
        merchant.rotate_key();
        assert_eq!(
            reconcile_paid(
                &worker,
                &merchant,
                &merchant.paid("paid", "customer-top-up")
            )
            .await,
            Ok(receipt)
        );
    } else {
        checkout::run(
            &admin,
            (worker, async || {
                db::connect(&config, &c.dsn("fixture_payments"))
                    .await
                    .unwrap()
            }),
            &origin,
            &merchant,
            &mut browser,
            &exchange,
            (a, b),
        )
        .await;
    }
    fs::write(exchange.join("funded"), []).unwrap();
    if !with_inference {
        refunds::run(
            &admin,
            async || {
                db::connect(&config, &c.dsn("fixture_payments"))
                    .await
                    .unwrap()
            },
            merchant.binding,
            &mut browser,
            &exchange,
            a,
        )
        .await;
    }
    checkpoint(&mut browser, &exchange.join("keys-issued")).await;
    let mut pipe = browser.0.stdout.take().unwrap();
    let mut read_key = || {
        let mut raw = zeroize::Zeroizing::new([0_u8; 101]);
        pipe.read_exact(raw.as_mut()).unwrap();
        zeroize::Zeroizing::new(std::str::from_utf8(raw.as_ref()).unwrap().to_owned())
    };
    let original = read_key();
    let replacement = read_key();
    assert_eq!(
        qualified_workspace(&admin, &origin, &original)
            .await
            .unwrap(),
        Some(a)
    );
    assert_eq!(
        qualified_workspace(&admin, &origin, &replacement)
            .await
            .unwrap(),
        Some(a)
    );
    for query in [
        "SELECT verifier FROM relay_hosted.management_devices",
        "SELECT id FROM relay_hosted.customer_sessions",
        "SELECT operation_id FROM relay_hosted.customer_access_changes",
        "SELECT relay_hosted.replay_customer_access('http://localhost:1443',gen_random_uuid(),gen_random_uuid(),'[]')",
        "SELECT relay_hosted.connection_page(gen_random_uuid(),NULL::uuid)",
        "SELECT operation FROM relay_hosted.device_changes",
        "SELECT confirmed_by_passkey_id FROM relay_hosted.text_rule_changes",
        "SELECT limit_micro_usd FROM relay_hosted.text_purchase_limit_changes",
        "SELECT relay_hosted.lock_supply_signer(gen_random_uuid(),decode('00','hex'))",
        "SELECT accounted_micro_usd FROM relay_hosted.text_purchase_periods",
        "SELECT valued_micro_usd FROM relay_hosted.text_late_usage",
        "SELECT percent FROM relay_hosted.text_source_markups",
        "SELECT inputs FROM relay_hosted.text_sale_reviews",
        "SELECT message FROM relay_hosted.service_notices",
        "SELECT user_id FROM relay_hosted.service_notice_inbox",
        "SELECT actor_id FROM relay_hosted.service_notice_operations",
        "SELECT merchant_reference FROM relay_hosted.payment_intents",
        "SELECT * FROM relay_hosted.claim_checkout_invoice(gen_random_uuid())",
        "SELECT handoff FROM relay_hosted.checkout_resume_checks",
        "SELECT * FROM relay_hosted.claim_checkout_resume(gen_random_uuid())",
        "SELECT relay_hosted.finish_checkout_resume(gen_random_uuid(),gen_random_uuid(),gen_random_uuid(),NULL)",
        "SELECT provider_reference FROM relay_hosted.payment_refunds",
        "SELECT actor_id FROM relay_hosted.payment_refund_reviews",
        "SELECT relay_hosted.lock_owner_payment_session('http://127.0.0.1', decode(repeat('00',32),'hex'))",
        "SELECT * FROM relay_hosted.claim_payment_refund(gen_random_uuid())",
        "SELECT * FROM relay_hosted.pending_refund_lookups(gen_random_uuid(),NULL::uuid)",
        "SELECT * FROM relay_hosted.refund_for_lookup(gen_random_uuid(),gen_random_uuid())",
        // Constant NULL arguments can short-circuit a STRICT scalar function
        // before permission checks. Exercise a call with actual arguments.
        "SELECT relay_hosted.observe_payment_refund(gen_random_uuid(),gen_random_uuid(),'invoice','USD',1,'refund',true)",
    ] {
        let error = sqlx::query(query).execute(&runtime).await.expect_err(query);
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("42501"),
            "{query}"
        );
    }
    assert!(
        qualified_workspace(&runtime, &origin, &replacement)
            .await
            .is_err()
    );
    fs::write(exchange.join("keys-verified"), []).unwrap();
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT has_function_privilege('relay_hosted.own_active_customer_sessions(relay_hosted.customer_sessions,timestamptz)', 'EXECUTE')",
        )
        .fetch_one(&runtime)
        .await
        .unwrap()
    );
    if with_inference {
        checkpoint(&mut browser, &exchange.join("funding-keys")).await;
        let b_key = read_key();
        funded::run(
            &c,
            &cfg,
            &config,
            &admin,
            a,
            b,
            &replacement,
            &b_key,
            &paired,
            &mut browser,
            async || {
                server.0.kill().unwrap();
                server.0.wait().unwrap();
                server = serve();
                support::ready(&mut server, &http, &config_url).await;
            },
        )
        .await;
        fs::write(exchange.join("charge-complete"), []).unwrap();
    } else {
        let request = exchange.join("age-customer-session");
        checkpoint(&mut browser, &request).await;
        let id: Uuid = fs::read_to_string(request).unwrap().parse().unwrap();
        let aged = sqlx::query(
            "UPDATE relay_hosted.customer_sessions SET last_seen_at=clock_timestamp()-interval '31 minutes'
             WHERE id=$1 AND workspace_id=$2",
        )
        .bind(id)
        .bind(a)
        .execute(&admin)
        .await
        .unwrap();
        assert_eq!(aged.rows_affected(), 1);
        fs::write(exchange.join("customer-session-aged"), []).unwrap();
    }
    checkpoint(&mut browser, &exchange.join("restart")).await;
    assert_eq!(
        qualified_workspace(&admin, &origin, &original)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        qualified_workspace(&admin, &origin, &replacement)
            .await
            .unwrap(),
        Some(a)
    );
    // A changed key must fail the startup binding check, not merely the occupied port.
    fs::write(&key_file, [57; 32]).unwrap();
    let wrong_key = c
        .cli("fixture_runtime", &cfg)
        .env("ZENITH_RELAY_HOSTED_DIGEST_KEY_FILE", &key_file)
        .arg("serve")
        .output()
        .unwrap();
    assert!(!wrong_key.status.success());
    assert!(
        String::from_utf8_lossy(&wrong_key.stderr)
            .contains("customer identity registration or digest key does not match")
    );
    fs::write(&key_file, [31; 32]).unwrap();
    drop(server);
    let mut server = serve();
    support::ready(&mut server, &http, &config_url).await;
    let inventory = paired.connections(None).await.unwrap();
    device::assert_observations(&paired).await;
    assert_eq!(inventory.items.len(), if with_inference { 32 } else { 0 });
    if with_inference {
        let after = inventory
            .next_after
            .expect("cursor for the remaining sources");
        assert_eq!(
            paired.connections(Some(after)).await.unwrap().items.len(),
            2
        );
    } else {
        assert!(inventory.next_after.is_none());
    }
    device::reject_customer_case(&http, &origin, &paired, a).await;
    fs::write(exchange.join("restarted"), []).unwrap();
    checkpoint(&mut browser, &exchange.join("after-restart")).await;
    if let Some(id) = notice {
        notices::expire_private_notice(&admin, &runtime, &mut browser, &exchange, id).await;
    }
    assert_eq!(
        qualified_workspace(&admin, &origin, &replacement)
            .await
            .unwrap(),
        Some(a)
    );
    checkpoint(&mut browser, &exchange.join("suspension-checked")).await;
    assert_eq!(
        qualified_workspace(&admin, &origin, &replacement)
            .await
            .unwrap(),
        None
    );
    let changes: Vec<(String, i64)> = sqlx::query_as(
        "SELECT action,expected_revision FROM relay_hosted.customer_access_changes
         WHERE workspace_id=$1 ORDER BY expected_revision",
    )
    .bind(a)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(
        changes,
        [
            ("suspend".into(), 1),
            ("reinstate".into(), 2),
            ("close".into(), 3)
        ]
    );
    let session_count = || {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM relay_hosted.customer_sessions")
            .fetch_one(&admin)
    };
    let sessions_before_expiry = session_count().await.unwrap();
    sqlx::query("UPDATE relay_hosted.configuration_revisions SET expires_at=created_at WHERE mode='qualification'")
        .execute(&admin).await.unwrap();
    fs::write(exchange.join("scope-expired"), []).unwrap();
    checkpoint(&mut browser, &exchange.join("paused")).await;
    assert_eq!(session_count().await.unwrap(), sessions_before_expiry);
    assert_eq!(confirmations().await.unwrap(), (2, 2));
    assert_eq!(
        paired.connections(None).await.err(),
        Some("device_not_authorized")
    );
    device::assert_revoked(&paired).await;
    assert!(browser.0.wait().unwrap().success());
    assert_eq!(counts().await.unwrap(), (3, 2, 2, 0));
    if with_inference {
        payment::assert_wallet(&admin, a, 8_799_400, 14).await;
    } else {
        payment::assert_wallet(&admin, a, 8_000_000, 5).await;
    }
    payment::assert_wallet(&admin, b, 0, 0).await;
    assert_eq!(
        sqlx::query_as::<_, (String, i64, i64)>(
            "SELECT mode,revision,admission_epoch FROM relay_hosted.service_control"
        )
        .fetch_one(&admin)
        .await
        .unwrap(),
        ("maintenance".into(), 3, 3)
    );
    drop(server);
    c.stop();
}
