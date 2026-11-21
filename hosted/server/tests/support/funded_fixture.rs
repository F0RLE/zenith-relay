use crate::support::{Cluster, Process, port};
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
    },
    time::Duration,
};
use tokio::sync::Semaphore;
use uuid::Uuid;
use zenith_relay_hosted::{
    config::Config,
    db,
    vault::{Purpose, Vault},
};
#[path = "capture_poll.rs"]
mod capture_poll;
#[path = "settlement_edges.rs"]
mod edges;
#[allow(dead_code)]
#[path = "partial_text.rs"]
mod partial_text;

struct Up {
    sends: AtomicUsize,
    reads: AtomicUsize,
    probes: AtomicUsize,
    omit_usage: AtomicBool,
    entered: Semaphore,
    release: Semaphore,
}
async fn catalog(State(up): State<Arc<Up>>, h: HeaderMap) -> Json<Value> {
    assert!(h["authorization"] == "Bearer fixture-loopback-only" && h["x-fixture-path"] == "proxy");
    up.reads.fetch_add(1, SeqCst);
    Json(json!({"object":"list","data":[
        {"object":"model","id":"fixture-text"}, {"object":"model","id":"review-text"}
    ]}))
}
async fn upstream(State(up): State<Arc<Up>>, h: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
    assert!(h["authorization"] == "Bearer fixture-loopback-only" && h["x-fixture-path"] == "proxy");
    assert!(!h.contains_key("idempotency-key") && !h.contains_key("x-workspace-id"));
    assert_eq!(body["store"], false);
    if body["input"] == "Reply with OK." {
        up.probes.fetch_add(1, SeqCst);
        let mut reply = json!({"object":"response","id":"resp_probe","model":body["model"],"status":"completed",
            "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"OK"}]}],
            "usage":{"input_tokens":4,"input_tokens_details":{"cached_tokens":0},"output_tokens":1,"total_tokens":5}});
        if up.omit_usage.load(SeqCst) {
            reply.as_object_mut().unwrap().remove("usage");
        }
        return Json(reply);
    }
    up.sends.fetch_add(1, SeqCst);
    up.entered.add_permits(1);
    up.release.acquire().await.unwrap().forget();
    Json(
        json!({"object":"response","id":"resp_fixture","model":"fixture-text","status":"completed",
        "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"fixture"}]}],
        "usage":{"input_tokens":40000,"input_tokens_details":{"cached_tokens":0},"output_tokens":10000,"total_tokens":50000}}),
    )
}
// Forward just two fixed paths to this test's own listener. Never use an incoming destination.
async fn via(
    State((addr, client)): State<(std::net::SocketAddr, reqwest::Client)>,
    req: Request,
) -> Response {
    let path = match (req.method().as_str(), req.uri().path()) {
        ("GET", "/v1/models") => "/v1/models",
        ("POST", "/v1/responses") => "/v1/responses",
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    assert_eq!(req.uri().authority().unwrap().as_str(), addr.to_string());
    let outgoing = client
        .request(req.method().clone(), format!("http://{addr}{path}"))
        .header("authorization", req.headers()["authorization"].clone())
        .header("content-type", "application/json")
        .header("x-fixture-path", "proxy");
    let bytes = axum::body::to_bytes(req.into_body(), 65536).await.unwrap();
    let res = outgoing.body(bytes).send().await.unwrap();
    (
        res.status(),
        [("content-type", "application/json")],
        res.bytes().await.unwrap(),
    )
        .into_response()
}
async fn seen(up: &Up) {
    tokio::time::timeout(Duration::from_secs(8), up.entered.acquire())
        .await
        .expect("provider send")
        .unwrap()
        .forget()
}

async fn assert_not_dispatched(db: &PgPool, response: reqwest::Response) {
    assert_eq!(response.status(), 503);
    let id = Uuid::parse_str(response.headers()["x-request-id"].to_str().unwrap()).unwrap();
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["code"],
        "relay_not_dispatched"
    );
    let state: (String, i64, bool) = sqlx::query_as(
        "SELECT r.basis,r.charged,a.attempt_id IS NOT NULL FROM relay_hosted.text_requests r
         LEFT JOIN relay_hosted.text_purchase_attempts a USING(attempt_id) WHERE r.id=$1",
    )
    .bind(id)
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(state, ("not_sent".into(), 0, false));
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    c: &Cluster,
    cfg: &Path,
    config: &Config,
    admin: &PgPool,
    a: Uuid,
    b: Uuid,
    ak: &str,
    bk: &str,
    device: &zenith_relay_hosted_device::DeviceKey,
    browser: &mut Process,
    mut restart_web: impl AsyncFnMut(),
) {
    let dir = c.temp.path().join("vault");
    let key = c.temp.path().join("vault-secret");
    fs::create_dir(&dir).unwrap();
    fs::write(&key, [73; 32]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let source = Uuid::new_v4();
    let item = Uuid::new_v4();
    let vp = db::connect(config, &c.dsn("fixture_vault")).await.unwrap();
    let vault = Vault::open(&vp, &config.server.public_origin, &dir, &key)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let upstream_url = format!("http://{address}/v1");
    let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_url = format!("http://{}/", proxy.local_addr().unwrap());
    let proxy_app = Router::new().fallback(via).with_state((
        address,
        reqwest::Client::builder().no_proxy().build().unwrap(),
    ));
    let proxy_task = tokio::spawn(async move { axum::serve(proxy, proxy_app).await.unwrap() });
    for (purpose, value) in [("provider", "fixture-loopback-only"), ("proxy", &proxy_url)] {
        let file = c.temp.path().join(format!("private-{purpose}"));
        fs::write(&file, value).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    let mut add = c.cli("fixture_vault", cfg);
    add.args([
        "vault-provision",
        "--operation",
        &Uuid::new_v4().to_string(),
        "--source",
        &source.to_string(),
        "--provider-file",
    ])
    .arg(c.temp.path().join("private-provider"))
    .arg("--proxy-file")
    .arg(c.temp.path().join("private-proxy"))
    .arg("--vault-dir")
    .arg(&dir)
    .env("ZENITH_RELAY_HOSTED_VAULT_KEY_FILE", &key);
    for message in [
        "Source and route stored.",
        "Import previously committed; no changes.",
    ] {
        let out = add.output().unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), message);
    }
    let up = Arc::new(Up {
        sends: 0.into(),
        reads: 0.into(),
        probes: 0.into(),
        omit_usage: false.into(),
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let app = Router::new()
        .route("/v1/models", get(catalog))
        .route("/v1/responses", post(upstream))
        .with_state(up.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let scope = c
        .cli("fixture_qualifier", cfg)
        .args(["qualify-text", "--print-scope"])
        .output()
        .unwrap();
    assert!(scope.status.success());
    let scope: Uuid = std::str::from_utf8(&scope.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let bind = format!("127.0.0.1:{}", port());
    let serve = || {
        Process(
            c.cli("fixture_inference", cfg)
                .args(["serve-text", "--bind", &bind, "--vault-dir"])
                .arg(&dir)
                .env("ZENITH_RELAY_HOSTED_VAULT_KEY_FILE", &key)
                .env(
                    "ZENITH_RELAY_HOSTED_VAULT_DATABASE_URL_FILE",
                    c.role_file("fixture_vault"),
                )
                .spawn()
                .unwrap(),
        )
    };
    let http = reqwest::Client::builder()
        .no_proxy()
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(75))
        .build()
        .unwrap();
    let url = format!("http://{bind}/v1/responses");
    let ready = |mut child: Process| {
        let http = http.clone();
        let url = url.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    assert!(
                        child.0.try_wait().unwrap().is_none(),
                        "text worker exited before readiness"
                    );
                    if http
                        .post(&url)
                        .send()
                        .await
                        .is_ok_and(|r| r.status() == 401)
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(40)).await;
                }
            })
            .await
            .unwrap();
            child
        }
    };
    let mut worker = ready(serve()).await;
    let list = || {
        http.get(format!("http://{bind}/v1/models"))
            .bearer_auth(ak)
            .send()
    };
    let models = |v: &Value| v["data"].as_array().unwrap().len();
    assert_eq!(models(&list().await.unwrap().json().await.unwrap()), 0);
    let file = c.temp.path().join("review.json");
    let profile = json!({"id":item,"scope":scope,"source":source,"source_label":"Test connection","endpoint":upstream_url,"model":"fixture-text",
        "provider_revision":1,"proxy_revision":1,"uses_proxy":true,"max_input_bytes":40000,"max_output_tokens":30000,
        "tokens_per_byte":1,"overhead_tokens":0,"close_after_s":65,"valid_for_s":3600});
    let saved = device.connections(None).await.unwrap().items;
    assert_eq!(saved.len(), 1);
    assert!(saved[0].id == source && saved[0].proxy_bound && saved[0].verified_models.is_empty());
    let qualify = |value: &Value, role: &str| {
        fs::write(&file, serde_json::to_vec(value).unwrap()).unwrap();
        let mut cmd = c.cli(role, cfg);
        cmd.args(["qualify-text", "--profile"])
            .arg(&file)
            .arg("--vault-dir")
            .arg(&dir)
            .env("ZENITH_RELAY_HOSTED_VAULT_KEY_FILE", &key)
            .env(
                "ZENITH_RELAY_HOSTED_VAULT_DATABASE_URL_FILE",
                c.role_file("fixture_vault"),
            );
        tokio::task::spawn_blocking(move || cmd.output().unwrap())
    };
    assert!(
        !qualify(&profile, "fixture_runtime")
            .await
            .unwrap()
            .status
            .success()
    );
    let mut absent = profile.clone();
    absent["id"] = json!(Uuid::new_v4());
    absent["model"] = json!("not-in-source");
    assert!(
        !qualify(&absent, "fixture_qualifier")
            .await
            .unwrap()
            .status
            .success()
    );
    assert_eq!(up.probes.load(SeqCst), 0);
    for _ in 0..2 {
        let r = qualify(&profile, "fixture_qualifier").await.unwrap();
        assert!(
            r.status.success(),
            "qualification: {}",
            String::from_utf8_lossy(&r.stderr)
        );
    }
    let verified = device.connections(None).await.unwrap().items;
    assert_eq!(verified[0].label.as_deref(), Some("Test connection"));
    assert_eq!(verified[0].verified_models, ["fixture-text"]);
    assert_eq!((up.reads.load(SeqCst), up.probes.load(SeqCst)), (2, 1));
    assert_eq!(models(&list().await.unwrap().json().await.unwrap()), 0);
    let exchange = c.temp.path().join("checkpoints");
    fs::write(exchange.join("inventory-ready"), []).unwrap();
    super::checkpoint(browser, &exchange.join("rule-published")).await;
    let serving: Vec<(i64, bool)> = sqlx::query_as(
        "SELECT c.revision,c.paused FROM relay_hosted.source_serving_changes c
         JOIN relay_hosted.admin_credentials k ON (k.passkey_id,k.user_id)=(c.confirmed_by_passkey_id,c.actor)
         WHERE c.source_id=$1 ORDER BY c.revision",
    )
    .bind(source)
    .fetch_all(admin)
    .await
    .unwrap();
    assert_eq!(serving, [(1, true), (2, false)]);
    let offer: Uuid =
        sqlx::query_scalar("SELECT id FROM relay_hosted.active_text_listings WHERE item=$1")
            .bind(item)
            .fetch_one(admin)
            .await
            .unwrap();
    let published = list().await.unwrap().json::<Value>().await.unwrap();
    assert_eq!(
        published["data"],
        json!([{"id":"fixture-text","object":"model"}])
    );
    assert_eq!((up.reads.load(SeqCst), up.probes.load(SeqCst)), (2, 1));
    let purchases = || {
        sqlx::query_as::<_, (Option<i64>, Option<i64>, Option<String>, Option<bool>)>(
            "SELECT a.rate_revision,a.bound_micro_usd,p.valued_micro_usd::text,
                    a.month_utc=date_trunc('month',r.claimed_at AT TIME ZONE 'UTC')::date
             FROM relay_hosted.text_requests r
             LEFT JOIN relay_hosted.text_purchase_attempts a USING(attempt_id)
             LEFT JOIN relay_hosted.text_purchase_results p USING(attempt_id)
             ORDER BY r.started_at",
        )
        .fetch_all(admin)
    };
    let body = json!({"model":"fixture-text","input":"x".repeat(40000),"max_output_tokens":30000});
    let submit = |key: &str, op: &str, body: &Value| {
        http.post(&url)
            .bearer_auth(key)
            .header("Idempotency-Key", op)
            .header("X-Workspace-Id", b.to_string())
            .json(body)
            .send()
    };
    // No funds and no grant are different refusals; neither sends nor reserves.
    assert_eq!(submit(bk, "one", &body).await.unwrap().status(), 402);
    let mut other = body.clone();
    other["model"] = json!("unoffered");
    assert_eq!(submit(ak, "other", &other).await.unwrap().status(), 403);
    assert_eq!(up.sends.load(SeqCst), 0);
    assert!(purchases().await.unwrap().is_empty());
    let mut first = tokio::spawn(submit(ak, "one", &body));
    tokio::select! {
        ()=seen(&up)=>(),
        r=&mut first=>{
            let res=r.unwrap().unwrap();let status=res.status();
            let code=res.json::<Value>().await.unwrap();
            let states:Vec<(String,String)>=sqlx::query_as("SELECT execution,accounting FROM relay_hosted.text_requests ORDER BY started_at").fetch_all(admin).await.unwrap();
            panic!("first request returned {status} {:?} before a send; states {states:?}",code["error"]["code"]);
        }
    }
    let wallet = || {
        sqlx::query_as::<_,(i64,i64)>("SELECT available_micro_usd,reserved_micro_usd FROM relay_hosted.wallets WHERE workspace_id=$1").bind(a).fetch_one(admin)
    };
    assert_eq!(wallet().await.unwrap(), (8_000_000, 2_000_000));
    let first_bound = (Some(1), Some(530_000), None, Some(true));
    assert_eq!(
        purchases().await.unwrap().as_slice(),
        std::slice::from_ref(&first_bound)
    );
    let mut occupied = profile.clone();
    occupied["id"] = json!(Uuid::new_v4());
    assert!(
        !qualify(&occupied, "fixture_qualifier")
            .await
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        (up.reads.load(SeqCst), up.probes.load(SeqCst)),
        (2, 1),
        "a blocked check cannot reach either upstream endpoint"
    );
    fs::write(exchange.join("first-hold"), []).unwrap();
    super::checkpoint(browser, &exchange.join("lower-price")).await;
    assert_eq!(purchases().await.unwrap(), [first_bound]);
    let mut equiv = body.clone();
    equiv["store"] = json!(false);
    equiv["stream"] = json!(false);
    let dupe = submit(ak, "one", &equiv).await.unwrap();
    assert_eq!(dupe.status(), 409);
    let logical = dupe.headers()["x-request-id"].to_str().unwrap().to_owned();
    assert_eq!(
        dupe.json::<Value>().await.unwrap()["error"]["code"],
        "relay_request_in_progress"
    );
    let conflict = submit(ak, "one", &other).await.unwrap();
    assert_eq!(conflict.status(), 409);
    assert!(!conflict.headers().contains_key("x-request-id"));
    assert_eq!(up.sends.load(SeqCst), 1);
    fs::write(exchange.join("capture-request"), &logical).unwrap();
    super::checkpoint(browser, &exchange.join("capture-started")).await;
    let captures: (i64, i64) = sqlx::query_as(
        "SELECT count(*),count(confirmed_by_passkey_id) FROM relay_hosted.diagnostic_capture_audit
         WHERE action='start'",
    )
    .fetch_one(admin)
    .await
    .unwrap();
    assert_eq!(captures, (1, 1));
    assert_eq!(wallet().await.unwrap(), (8_000_000, 2_000_000));
    capture_poll::subscribed(admin, "fixture_inference").await;
    up.release.add_permits(1);
    let reply = first.await.unwrap().unwrap();
    assert_eq!(reply.status(), 200);
    assert_eq!(reply.headers()["x-request-id"], logical);
    fs::write(exchange.join("capture-finished"), []).unwrap();
    super::checkpoint(browser, &exchange.join("capture-checked")).await;
    assert_eq!(wallet().await.unwrap(), (8_800_000, 0));
    assert_eq!(up.sends.load(SeqCst), 1);
    // Lose the successful response body, restart, and repeat the operation.
    drop(reply);
    drop(worker);
    worker = ready(serve()).await;
    let again = submit(ak, "one", &body).await.unwrap();
    assert_eq!(again.status(), 409);
    assert_eq!(again.headers()["x-request-id"], logical);
    let duplicate = again.json::<Value>().await.unwrap();
    assert_eq!(
        duplicate["error"]["code"],
        "relay_request_result_unavailable"
    );
    assert!(duplicate.get("output").is_none());
    let status = format!("http://{bind}/api/hosted/v1/requests/{logical}/status");
    assert_eq!(
        http.get(&status)
            .bearer_auth(bk)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let rec = &http
        .get(&status)
        .bearer_auth(ak)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["data"];
    assert_eq!(rec["charge_micro_usd"], "1200000");
    assert_eq!(rec["hold_micro_usd"], "2000000");
    assert!(rec["wallet_receipt_id"].is_string());
    assert_eq!(rec["input_tokens"], "40000");
    assert_eq!(rec["cached_tokens"], "0");
    assert_eq!(rec["output_tokens"], "10000");
    assert_eq!(rec["tariff_revision"], "1");
    assert_eq!(wallet().await.unwrap(), (8_800_000, 0));
    assert_eq!(up.sends.load(SeqCst), 1);
    let (rid, aid): (Uuid, Uuid) =
        sqlx::query_as("SELECT id,attempt_id FROM relay_hosted.text_requests WHERE id=$1")
            .bind(Uuid::parse_str(&logical).unwrap())
            .fetch_one(admin)
            .await
            .unwrap();
    let wp = db::connect(config, &c.dsn("fixture_inference"))
        .await
        .unwrap();
    let observed = partial_text::Attempt {
        offer,
        request: rid,
        id: aid,
    };
    let verified = [Some(40000), Some(0), Some(10000)];
    assert!(observed.capture(&wp, "completed", verified).await);
    assert_eq!(observed.reconcile(&wp, "deadline").await, "settled");
    assert_eq!(wallet().await.unwrap(), (8_800_000, 0));
    let first_cost = (Some(1), Some(530_000), Some("310000".into()), Some(true));
    assert_eq!(
        purchases().await.unwrap().as_slice(),
        std::slice::from_ref(&first_cost)
    );
    let period = || {
        sqlx::query_as::<_, (String, String, bool)>(
            "SELECT accounted_micro_usd::text,outstanding_micro_usd::text,unpriced
             FROM relay_hosted.text_purchase_periods WHERE source_id=$1
             AND month_utc=date_trunc('month',clock_timestamp() AT TIME ZONE 'UTC')::date",
        )
        .bind(source)
        .fetch_one(admin)
    };
    let spend_before_cap = ("310000".into(), "0".into(), false);
    assert_eq!(period().await.unwrap(), spend_before_cap);
    super::age_current_passkey(admin).await;
    fs::write(exchange.join("purchase-recorded"), []).unwrap();
    super::checkpoint(browser, &exchange.join("limit-too-low")).await;
    // The source is idle. At one microUSD below the bound, there is no permit,
    // no supplier attempt and no surviving customer hold.
    let denied = tokio::time::timeout(Duration::from_secs(8), submit(ak, "over-cap", &body))
        .await
        .unwrap()
        .unwrap();
    assert_not_dispatched(admin, denied).await;
    assert_eq!(up.sends.load(SeqCst), 1);
    assert_eq!(wallet().await.unwrap(), (8_800_000, 0));
    assert_eq!(period().await.unwrap(), spend_before_cap);
    fs::write(exchange.join("cap-rejected"), []).unwrap();
    super::checkpoint(browser, &exchange.join("limit-set")).await;
    // Kill one worker with one acknowledged send and a second reserved behind the same physical slot.
    let sent = tokio::spawn(submit(ak, "crash-send", &body));
    seen(&up).await;
    let queued = tokio::spawn(submit(ak, "crash-reserve", &body));
    tokio::time::timeout(Duration::from_secs(6), async {
        while wallet().await.unwrap() != (4_800_000, 4_000_000) {
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("both holds committed");
    drop(worker);
    let _ = tokio::join!(sent, queued);
    up.release.add_permits(1);
    let mut worker = ready(serve()).await;
    for op in ["crash-send", "crash-reserve"] {
        assert_eq!(submit(ak, op, &body).await.unwrap().status(), 409);
    }
    // Observe real committed deadlines elapsed, not a test-only mode change or a rewritten hold.
    tokio::time::timeout(Duration::from_secs(78), async {
        while wallet().await.unwrap() != (8_800_000, 0) {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("restarted worker closed both holds");
    let states: Vec<(String, String)> = sqlx::query_as(
        "SELECT execution,basis FROM relay_hosted.text_requests ORDER BY started_at",
    )
    .fetch_all(admin)
    .await
    .unwrap();
    assert_eq!(
        states,
        [
            ("completed".into(), "provider_final".into()),
            ("failed".into(), "not_sent".into()),
            ("uncertain".into(), "policy_waiver".into()),
            ("failed".into(), "not_sent".into())
        ]
    );
    assert_eq!(up.sends.load(SeqCst), 2);
    assert_eq!(
        purchases().await.unwrap(),
        [
            first_cost,
            (None, None, None, None),
            (Some(2), Some(620_000), None, Some(true)),
            (None, None, None, None)
        ]
    );
    assert_eq!(
        period().await.unwrap(),
        ("310000".into(), "620000".into(), false)
    );
    // The old send is now closed for customer accounting, so the busy guard
    // cannot mask whether its supplier exposure still consumes the cap.
    let denied = tokio::time::timeout(Duration::from_secs(8), submit(ak, "after-waiver", &body))
        .await
        .unwrap()
        .unwrap();
    assert_not_dispatched(admin, denied).await;
    assert_eq!(up.sends.load(SeqCst), 2);
    assert_eq!(wallet().await.unwrap(), (8_800_000, 0));
    // Late usage can price the sent attempt without reopening a waived customer receipt.
    let (late_offer, late_request, late_attempt): (Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT offer_id,id,attempt_id FROM relay_hosted.text_requests WHERE basis='policy_waiver'",
    )
    .fetch_one(admin)
    .await
    .unwrap();
    let observed = partial_text::Attempt {
        offer: late_offer,
        request: late_request,
        id: late_attempt,
    };
    for _ in 0..2 {
        assert!(observed.capture(&wp, "completed", verified).await);
        assert_eq!(observed.reconcile(&wp, "deadline").await, "released");
    }
    let late: (String, i64, String) = sqlx::query_as(
        "SELECT r.basis,r.charged,p.valued_micro_usd::text FROM relay_hosted.text_requests r
         JOIN relay_hosted.text_purchase_results p USING(attempt_id) WHERE r.id=$1",
    )
    .bind(late_request)
    .fetch_one(admin)
    .await
    .unwrap();
    assert_eq!(late, ("policy_waiver".into(), 0, "420000".into()));
    assert_eq!(wallet().await.unwrap(), (8_800_000, 0));
    assert_eq!(
        period().await.unwrap(),
        ("730000".into(), "0".into(), false)
    );
    let key_id = Uuid::parse_str(rec["key_id"].as_str().unwrap()).unwrap();
    let current: Uuid =
        sqlx::query_scalar("SELECT id FROM relay_hosted.active_text_listings WHERE item=$1")
            .bind(item)
            .fetch_one(admin)
            .await
            .unwrap();
    assert_ne!(current, offer);
    edges::check(admin, &config.server.public_origin, current, a, key_id, ak).await;
    sqlx::query(
        "UPDATE relay_hosted.admin_sessions SET created_at=created_at-interval '3 minutes'",
    )
    .execute(admin)
    .await
    .unwrap();
    fs::write(exchange.join("receipts-closed"), []).unwrap();
    super::checkpoint(browser, &exchange.join("withdrawn")).await;
    assert_eq!(models(&list().await.unwrap().json().await.unwrap()), 0);
    assert_eq!(submit(ak, "withdrawn", &body).await.unwrap().status(), 403);
    fs::write(exchange.join("withdrawal-seen"), []).unwrap();
    super::checkpoint(browser, &exchange.join("republished")).await;
    let old_authority:Option<Uuid>=sqlx::query_scalar("SELECT relay_hosted.lock_text_authority($1,$2,$3,verifier) FROM relay_hosted.customer_keys WHERE id=$3")
        .bind(&config.server.public_origin).bind(current).bind(key_id).fetch_one(admin).await.unwrap();
    assert!(
        old_authority.is_none(),
        "resuming must not revive a withdrawn permit"
    );
    assert_eq!(models(&list().await.unwrap().json().await.unwrap()), 1);

    let second_source = Uuid::from_u128(2);
    vault
        .put(
            second_source,
            Purpose::Provider,
            0,
            b"fixture-loopback-only",
        )
        .await
        .unwrap();
    vault
        .put(second_source, Purpose::Proxy, 0, proxy_url.as_bytes())
        .await
        .unwrap();
    for (id, label) in [
        (source, "Test connection"),
        (second_source, "Second connection"),
    ] {
        let mut review = profile.clone();
        review["id"] = json!(Uuid::new_v4());
        review["source"] = json!(id);
        review["source_label"] = json!(label);
        review["model"] = json!("review-text");
        let result = qualify(&review, "fixture_qualifier").await.unwrap();
        assert!(
            result.status.success(),
            "review qualification: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let reference_file = c.temp.path().join("catalog-publishers.json");
    fs::write(
        &reference_file,
        r#"{"openai/review-text":{"name":"Fixture review"}}"#,
    )
    .unwrap();
    let imported = c
        .cli("fixture_boot", cfg)
        .args(["import-catalog-publishers", "--file"])
        .arg(reference_file)
        .output()
        .unwrap();
    assert!(
        imported.status.success(),
        "publisher import: {}",
        String::from_utf8_lossy(&imported.stderr)
    );
    // Start the two-source policy phase from durable state with its own ceremony window.
    restart_web().await;
    fs::write(exchange.join("selling-review-ready"), []).unwrap();
    super::checkpoint(browser, &exchange.join("review-awaits-supplier-removal")).await;
    vault
        .revoke(second_source, Purpose::Proxy, 1)
        .await
        .unwrap();
    fs::write(exchange.join("review-supplier-removed"), []).unwrap();
    super::checkpoint(browser, &exchange.join("selling-review-checked")).await;
    let old_write_paths_removed: bool = sqlx::query_scalar(
        "SELECT to_regprocedure('relay_hosted.change_text_rule(text,bytea,uuid,uuid,bigint,text,jsonb)') IS NULL
             AND to_regprocedure('relay_hosted.change_purchase_limit(text,bytea,uuid,uuid,bigint,bigint)') IS NULL",
    )
    .fetch_one(admin)
    .await
    .unwrap();
    assert!(old_write_paths_removed);
    let confirmations: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT c.action,count(*),count(k.passkey_id)
         FROM relay_hosted.text_rule_changes c LEFT JOIN relay_hosted.admin_credentials k
         ON (k.passkey_id,k.user_id)=(c.confirmed_by_passkey_id,c.actor)
         WHERE c.action IN ('admit','publish','suspend') GROUP BY c.action ORDER BY c.action",
    )
    .fetch_all(admin)
    .await
    .unwrap();
    assert_eq!(
        confirmations,
        [
            ("admit".into(), 3, 3),
            ("publish".into(), 5, 5),
            ("suspend".into(), 2, 2)
        ]
    );
    let caps: Vec<(Option<i64>, bool)> = sqlx::query_as(
        "SELECT c.limit_micro_usd,k.passkey_id IS NOT NULL FROM relay_hosted.text_purchase_limit_changes c
         LEFT JOIN relay_hosted.admin_credentials k ON (k.passkey_id,k.user_id)=(c.confirmed_by_passkey_id,c.actor)
         WHERE c.source_id=$1 ORDER BY c.revision",
    )
    .bind(source)
    .fetch_all(admin)
    .await
    .unwrap();
    assert_eq!(
        caps,
        [(Some(929_999), true), (None, true), (Some(930_000), true)]
    );
    let tariffs: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT r.mode,o.input_rate::text,o.output_rate::text,o.cached_rate::text
         FROM relay_hosted.text_offers o JOIN relay_hosted.text_sale_reviews r
         ON (o.scope_id,o.model,o.sale_review_revision)=(r.scope_id,r.model,r.revision)
         WHERE o.model='review-text' ORDER BY o.revision",
    )
    .fetch_all(admin)
    .await
    .unwrap();
    assert_eq!(
        tariffs,
        [
            (
                "model_markup".into(),
                "3.000000000000".into(),
                "9.000000000000".into(),
                "0.750000000000".into()
            ),
            (
                "fixed".into(),
                "3.000000000000".into(),
                "5.000000000000".into(),
                "0.400000000000".into()
            ),
        ]
    );
    assert_eq!(models(&list().await.unwrap().json().await.unwrap()), 1);
    assert_eq!(up.sends.load(SeqCst), 2, "price edits do not send requests");
    // Start the next phase from durable state, with a fresh confirmation window.
    restart_web().await;
    fs::write(exchange.join("source-lifecycle-ready"), []).unwrap();
    super::checkpoint(browser, &exchange.join("source-lifecycle-checked")).await;
    let lifecycle: Vec<(String, bool, i64)> = sqlx::query_as(
        "SELECT c.action,c.paused,c.inventory_epoch FROM relay_hosted.source_serving_changes c
         JOIN relay_hosted.admin_credentials k ON (k.passkey_id,k.user_id)=(c.confirmed_by_passkey_id,c.actor)
         WHERE c.source_id=$1 AND c.revision>2 ORDER BY c.revision",
    ).bind(source).fetch_all(admin).await.unwrap();
    assert_eq!(
        lifecycle,
        [
            ("park".into(), true, 3),
            ("trash".into(), true, 4),
            ("restore".into(), true, 5),
            ("resume".into(), false, 5),
        ]
    );
    assert_eq!(models(&list().await.unwrap().json().await.unwrap()), 0);
    up.omit_usage.store(true, SeqCst);
    let mut uncertain = profile.clone();
    uncertain["id"] = json!(Uuid::new_v4());
    for _ in 0..2 {
        assert!(
            !qualify(&uncertain, "fixture_qualifier")
                .await
                .unwrap()
                .status
                .success()
        );
    }
    assert_eq!(
        (up.reads.load(SeqCst), up.probes.load(SeqCst)),
        (5, 4),
        "an uncertain check must not be repeated"
    );
    let mismatched: bool = sqlx::query_scalar(
        "SELECT relay_hosted.record_text_check($1,$2,$3,$4,1,1,true,$5,'different-model',
        'Test connection',40000,30000,1,0,65,3600,NULL)",
    )
    .bind(&config.server.public_origin)
    .bind(scope)
    .bind(Uuid::parse_str(uncertain["id"].as_str().unwrap()).unwrap())
    .bind(source)
    .bind(&upstream_url)
    .fetch_one(admin)
    .await
    .unwrap();
    assert!(!mismatched, "a probe cannot qualify a different profile");
    proxy_task.abort();
    let _ = proxy_task.await;
    let mut offline = profile.clone();
    offline["id"] = json!(Uuid::new_v4());
    assert!(
        !qualify(&offline, "fixture_qualifier")
            .await
            .unwrap()
            .status
            .success()
    );
    vault.revoke(source, Purpose::Proxy, 1).await.unwrap();
    let mut direct = profile.clone();
    direct["id"] = json!(Uuid::new_v4());
    direct["uses_proxy"] = json!(false);
    direct["proxy_revision"] = json!(2);
    let rejected = qualify(&direct, "fixture_qualifier").await.unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("check unavailable"));
    assert_eq!(models(&list().await.unwrap().json().await.unwrap()), 0);
    assert_eq!(
        submit(ak, "proxy-revoked", &body).await.unwrap().status(),
        403
    );
    let replay = submit(ak, "one", &body).await.unwrap();
    assert_eq!(replay.status(), 409);
    assert_eq!(replay.headers()["x-request-id"], logical);
    assert_eq!(up.sends.load(SeqCst), 2);
    for id in 1..=33 {
        if Uuid::from_u128(id) == second_source {
            continue;
        }
        vault
            .put(Uuid::from_u128(id), Purpose::Provider, 0, b"fixture-unused")
            .await
            .unwrap();
    }
    vault
        .revoke(Uuid::from_u128(1), Purpose::Provider, 1)
        .await
        .unwrap();
    // A proxy slot without a provider is not an API source.
    vault
        .put(Uuid::from_u128(96), Purpose::Proxy, 0, proxy_url.as_bytes())
        .await
        .unwrap();
    let owner_sources = format!(
        "{}/api/hosted/v1/admin/connections",
        config.server.public_origin
    );
    assert_eq!(
        http.get(owner_sources)
            .bearer_auth(ak)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    fs::write(exchange.join("connections-changed"), []).unwrap();
    super::checkpoint(browser, &exchange.join("connections-checked")).await;
    assert!(
        vault
            .read(second_source, Purpose::Provider, 1)
            .await
            .is_err()
    );
    assert_eq!((up.reads.load(SeqCst), up.probes.load(SeqCst)), (5, 4));

    let claimed_at: String =
        sqlx::query_scalar("SELECT (clock_timestamp()-interval '2 minutes')::text")
            .fetch_one(admin)
            .await
            .unwrap();
    let partial = partial_text::PartialText {
        key: key_id,
        offer,
        item,
        rate_revision: 1,
        claimed_at,
        input_bound: 100,
        output_bound: 100,
        observed_output: 100,
    }
    .seed(admin)
    .await;
    assert_eq!(partial.reconcile(&wp, "deadline").await, "settled");
    assert_eq!(wallet().await.unwrap(), (8_796_000, 0));
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT relay_hosted.record_request_diagnostic($1,$2,$3,'upstream_transport_body',NULL,NULL)",
        )
        .bind(partial.offer)
        .bind(partial.request)
        .bind(partial.id)
        .fetch_one(&wp)
        .await
        .unwrap()
    );
    fs::write(
        exchange.join("usage-before-credit"),
        partial.request.to_string(),
    )
    .unwrap();
    super::checkpoint(browser, &exchange.join("usage-pinned")).await;
    assert!(
        partial
            .capture(&wp, "incomplete", [Some(10), Some(0), Some(10)])
            .await
    );
    assert_eq!(partial.reconcile(&wp, "deadline").await, "settled");
    assert_eq!(wallet().await.unwrap(), (8_799_400, 0));
    fs::write(exchange.join("usage-credit-posted"), []).unwrap();
    super::checkpoint(browser, &exchange.join("usage-checked")).await;
    // A reconciliation failure must stop the text worker even when it is recorded.
    sqlx::query("REVOKE EXECUTE ON FUNCTION relay_hosted.due_text() FROM fixture_inference")
        .execute(admin)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(exit) = worker.0.try_wait().unwrap() {
                assert!(!exit.success());
                break;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("worker kept running after reconciliation failed");
    fs::write(exchange.join("operations-failed"), []).unwrap();
    super::checkpoint(browser, &exchange.join("operations-failure-seen")).await;
    sqlx::query("GRANT EXECUTE ON FUNCTION relay_hosted.due_text() TO fixture_inference")
        .execute(admin)
        .await
        .unwrap();
    let _restarted = ready(serve()).await;
    fs::write(exchange.join("operations-recovered"), []).unwrap();
    super::checkpoint(browser, &exchange.join("operations-checked")).await;
    assert_eq!(wallet().await.unwrap(), (8_799_400, 0));
    assert_eq!(up.sends.load(SeqCst), 2);
    task.abort();
}
