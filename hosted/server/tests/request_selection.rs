#[path = "support/customer_reader.rs"]
mod customer_reader;
#[path = "support/diagnostic_capture_cases.rs"]
mod diagnostic_capture_cases;
#[path = "support/model_status_cases.rs"]
mod model_status_cases;
#[path = "support/owner_overview_cases.rs"]
mod owner_overview_cases;
#[path = "support/owner_usage_cases.rs"]
mod owner_usage_cases;
#[path = "support/partial_text.rs"]
mod partial_text;
#[allow(dead_code)]
#[path = "support/payment_fixture.rs"]
mod payment;
#[path = "support/purchase_limit_race.rs"]
mod purchase_limit_race;
#[path = "support/refund_race.rs"]
mod refund_race;
#[path = "support/request_diagnostic_cases.rs"]
mod request_diagnostic_cases;
#[path = "support/restore_cases.rs"]
mod restore_cases;
#[allow(dead_code)]
mod support;
#[path = "support/text_late_cases.rs"]
mod text_late_cases;
#[path = "support/text_stream_cases.rs"]
mod text_stream_cases;
#[path = "support/text_stream_fixture.rs"]
mod text_stream_fixture;
#[path = "support/text_terminal_cases.rs"]
mod text_terminal_cases;
#[path = "support/usage_report_cases.rs"]
mod usage_report_cases;

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::post,
};
use ring::digest::{SHA256, digest};
use serde_json::{Value, json};
use sqlx::{PgExecutor, PgPool};
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst},
    },
    time::Duration,
};
use support::Cluster;
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;
use zenith_relay_hosted::{
    config::Config,
    db, inference,
    outbound::OutboundPolicy,
    payments::reconcile_paid,
    vault::{Purpose, Vault},
};

const ORIGIN: &str = "http://localhost:1443";
const MODEL: &str = "fixture-text";
const CREDIT: i64 = 1_000_000;
const SEAL: &[u8] = &[61; 32];

struct Listener {
    url: String,
    task: JoinHandle<()>,
}
impl Listener {
    async fn start(app: Router) -> Self {
        let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", socket.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
        Self { url, task }
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
struct Provider {
    sends: AtomicUsize,
    retry_after: AtomicU64,
    exceed_bound: AtomicBool,
    reply_gate: std::sync::Mutex<Option<Arc<ReplyGate>>>,
    stream: std::sync::Mutex<Option<text_stream_fixture::StreamReply>>,
}

#[derive(Default)]
struct ReplyGate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

async fn upstream(State(s): State<Arc<Provider>>, h: HeaderMap, Json(v): Json<Value>) -> Response {
    assert_eq!(h["authorization"], "Bearer fixture-supplier");
    assert!(!h.contains_key("idempotency-key"));
    let model = v["model"].as_str().unwrap();
    assert!(matches!(
        model,
        MODEL | purchase_limit_race::OTHER_MODEL | purchase_limit_race::PRIOR_MODEL
    ));
    assert_eq!(v["store"], false);
    s.sends.fetch_add(1, SeqCst);
    let gate = s.reply_gate.lock().unwrap().take();
    if let Some(gate) = gate {
        gate.entered.notify_one();
        gate.release.notified().await;
    }
    let retry_after = s.retry_after.load(SeqCst);
    if retry_after != 0 {
        return (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", retry_after.to_string())],
        )
            .into_response();
    }
    if v["stream"] == true {
        return s
            .stream
            .lock()
            .unwrap()
            .take()
            .expect("fixture stream script")
            .respond();
    }
    let input = v["input"].as_str().unwrap().len();
    let output = v["max_output_tokens"].as_u64().unwrap() + u64::from(s.exceed_bound.load(SeqCst));
    Json(json!({"object":"response", "id":"resp_fixture", "status":"completed","model":model,
        "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"OK"}]}],
        "usage":{"input_tokens":input,"input_tokens_details":{"cached_tokens":0},"output_tokens":output}
    })).into_response()
}

struct Source {
    id: Uuid,
    item: Uuid,
    provider: Arc<Provider>,
    listener: Listener,
}
impl Source {
    async fn new() -> Self {
        let provider = Arc::new(Provider::default());
        let listener = Listener::start(
            Router::new()
                .route("/v1/responses", post(upstream))
                .with_state(provider.clone()),
        )
        .await;
        Self {
            id: Uuid::new_v4(),
            item: Uuid::new_v4(),
            provider,
            listener,
        }
    }
}

struct Fixture {
    server: Listener,
    vault: Vault,
    http: reqwest::Client,
    admin: PgPool,
    worker: PgPool,
    pay: PgPool,
    merchant: payment::Adapter,
    a: Source,
    b: Source,
    scope: Uuid,
    space: Uuid,
    key: Uuid,
    bearer: String,
    offer: Uuid,
    _cluster: Cluster,
}

fn fixture_config() -> Config {
    Config::parse(
        &include_str!("../config.example.toml")
            .replace("lock_timeout_ms = 1000", "lock_timeout_ms = 3000"),
    )
    .unwrap()
}

impl Fixture {
    async fn new() -> Self {
        Self::setup(false).await
    }

    async fn setup(checkout: bool) -> Self {
        let c = Cluster::new();
        let admin = PgPool::connect(&c.dsn("fixture_boot")).await.unwrap();
        db::migrate(&admin, ORIGIN).await.unwrap();
        c.create_logins(
            &admin,
            &[
                "route_worker",
                "route_vault",
                "route_payment",
                "route_qualifier",
            ],
        )
        .await;
        for (sql, variable, role) in [
            (
                include_str!("../inference-grants.sql"),
                ":\"inference_role\"",
                "route_worker",
            ),
            (
                include_str!("../vault-grants.sql"),
                ":\"vault_role\"",
                "route_vault",
            ),
            (
                include_str!("../payment-grants.sql"),
                ":\"payment_role\"",
                "route_payment",
            ),
            (
                include_str!("../qualification-grants.sql"),
                ":\"qualification_role\"",
                "route_qualifier",
            ),
        ] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(sql.replace(variable, role)))
                .execute(&admin)
                .await
                .unwrap();
        }
        let scope = Uuid::new_v4();
        let active = sqlx::query(
            "WITH op AS (INSERT INTO relay_hosted.users(id) VALUES(gen_random_uuid()) RETURNING id),
             owner AS (INSERT INTO relay_hosted.admin_assignments(user_id,role) SELECT id,'owner' FROM op RETURNING user_id),
             issuer AS (INSERT INTO relay_hosted.identity_providers VALUES
                (gen_random_uuid(),'https://identity.example','fixture',decode(repeat('11',32),'hex')) RETURNING id),
             scope AS (INSERT INTO relay_hosted.configuration_revisions SELECT $1,2,1,'qualification',
                'verify_customer_access',owner.user_id,issuer.id,ARRAY[gen_random_uuid()],now(),now()+interval '1 hour'
                FROM owner,issuer RETURNING id)
             UPDATE relay_hosted.service_control SET mode='qualification',revision=2,qualification_scope_ref=(SELECT id FROM scope)",
        ).bind(scope).execute(&admin).await.unwrap();
        assert_eq!(active.rows_affected(), 1);
        let space: Uuid = sqlx::query_scalar(
            "WITH customer AS (INSERT INTO relay_hosted.users(id) VALUES(gen_random_uuid()) RETURNING id),
             workspace AS (INSERT INTO relay_hosted.workspaces(owner_user_id) SELECT id FROM customer RETURNING *),
             identity AS (INSERT INTO relay_hosted.external_identities(provider_id,subject_digest,user_id)
                 SELECT q.provider_id,decode(repeat('22',32),'hex'),w.owner_user_id
                 FROM relay_hosted.configuration_revisions q CROSS JOIN workspace w WHERE q.id=$1 RETURNING *)
             INSERT INTO relay_hosted.qualification_subjects(scope_id,subject_digest,workspace_id)
                SELECT $1,i.subject_digest,w.id FROM identity i JOIN workspace w ON w.owner_user_id=i.user_id RETURNING workspace_id",
        ).bind(scope).fetch_one(&admin).await.unwrap();
        let (key, bearer) = issue(&admin, scope, space).await;
        let config = fixture_config();
        let pay = db::connect(&config, &c.dsn("route_payment")).await.unwrap();
        let merchant =
            payment::Adapter::new(&admin, "sandbox", payment::Adapter::signing_key()).await;
        let invoice = "fund-route-test";
        let delivery = if checkout {
            refund_race::checkout_invoice(&admin, space, merchant.binding, invoice, CREDIT).await;
            merchant.sign("paid", "settled", invoice, "USD", CREDIT / 10_000)
        } else {
            payment::invoice(&admin, space, merchant.binding, invoice, CREDIT).await;
            merchant.paid("settled", invoice)
        };
        reconcile_paid(&pay, &merchant, &delivery).await.unwrap();
        payment::assert_wallet(&admin, space, CREDIT, 1).await;
        let root = c.temp.path().join("vault");
        let file = c.temp.path().join("vault-key");
        fs::create_dir(&root).unwrap();
        fs::write(&file, [71; 32]).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let vp = db::connect(&config, &c.dsn("route_vault")).await.unwrap();
        let vault = Vault::open(&vp, ORIGIN, &root, &file).await.unwrap();
        let a = Source::new().await;
        let b = Source::new().await;
        for (source, rates) in [(&a, ["2", ".50", "3"]), (&b, ["1", ".25", "6"])] {
            vault
                .put(source.id, Purpose::Provider, 0, b"fixture-supplier")
                .await
                .unwrap();
            stock(&admin, scope, source, source.item, MODEL, 1_000, rates).await;
        }
        let offer = publish(&admin, a.item, 1, None).await;
        let worker = db::connect(&config, &c.dsn("route_worker")).await.unwrap();
        let server = Listener::start(
            inference::router(
                worker.clone(),
                ORIGIN,
                vault.clone(),
                OutboundPolicy::default(),
                None,
            )
            .await
            .unwrap(),
        )
        .await;
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(8))
            .build()
            .unwrap();
        Self {
            server,
            http,
            vault,
            admin,
            worker,
            pay,
            merchant,
            a,
            b,
            scope,
            space,
            key,
            bearer,
            offer,
            _cluster: c,
        }
    }

    fn submit(
        &self,
        operation: &'static str,
        input: usize,
        output: u64,
    ) -> reqwest::RequestBuilder {
        self.submit_model(&self.bearer, MODEL, operation, input, output)
    }

    fn submit_model(
        &self,
        bearer: &str,
        model: &str,
        operation: &str,
        input: usize,
        output: u64,
    ) -> reqwest::RequestBuilder {
        self.http
            .post(format!("{}/v1/responses", self.server.url))
            .bearer_auth(bearer)
            .header("idempotency-key", operation)
            .json(&json!({"model":model, "input":"x".repeat(input), "max_output_tokens":output}))
    }

    fn sends(&self) -> (usize, usize) {
        (
            self.a.provider.sends.load(SeqCst),
            self.b.provider.sends.load(SeqCst),
        )
    }

    async fn verify_purchase(
        &self,
        response: reqwest::Response,
        item: Uuid,
        charge: i64,
        bound: i64,
        cost: &str,
    ) {
        let id = request_id(&response);
        let status = response.status();
        assert_eq!(status, 200, "{:?}", response.json::<Value>().await.unwrap());
        verify_receipt(&self.admin, id, item, charge, bound, cost).await;
    }

    async fn listed(&self) -> bool {
        let r = self
            .http
            .get(format!("{}/v1/models", self.server.url))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        r.json::<Value>().await.unwrap()["data"] == json!([{"id":MODEL,"object":"model"}])
    }

    async fn restart(&mut self) {
        self.server.task.abort();
        self.worker.close().await;
        self.admin.close().await;
        self._cluster.stop();
        self._cluster.start();
        self.admin = PgPool::connect(&self._cluster.dsn("fixture_boot"))
            .await
            .unwrap();
        self.worker = db::connect(&fixture_config(), &self._cluster.dsn("route_worker"))
            .await
            .unwrap();
        self.server = Listener::start(
            inference::router(
                self.worker.clone(),
                ORIGIN,
                self.vault.clone(),
                OutboundPolicy::default(),
                None,
            )
            .await
            .unwrap(),
        )
        .await;
    }
}

async fn issue(db: &PgPool, scope: Uuid, space: Uuid) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let bearer = format!("rlk_{}.{}", id.simple(), "ac".repeat(32));
    sqlx::query(
        "INSERT INTO relay_hosted.customer_keys(id,workspace_id,scope_id,issue_operation,label,verifier,user_epoch,installation_epoch,created_at,expires_at)
         SELECT $1,w.id,$2,gen_random_uuid(),'Fixture',$3,u.authorization_epoch,h.authorization_epoch,now(),now()+interval '30 minutes'
         FROM relay_hosted.workspaces w JOIN relay_hosted.users u ON u.id=w.owner_user_id CROSS JOIN relay_hosted.installations h WHERE w.id=$4",
    ).bind(id).bind(scope).bind(digest(&SHA256, bearer.as_bytes()).as_ref()).bind(space).execute(db).await.unwrap();
    (id, bearer)
}

async fn stock(
    db: &PgPool,
    scope: Uuid,
    source: &Source,
    item: Uuid,
    model: &str,
    output: i32,
    rates: [&str; 3],
) {
    sqlx::query(
        "WITH stock AS (
            INSERT INTO relay_hosted.text_inventory(id,scope_id,source_id,provider_revision,proxy_revision,uses_proxy,base_url,model,source_label,
                max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,close_after_s,valid_until)
            VALUES ($1,$2,$3,1,0,false,$4,$5,'Fixture source',1000,$6,1,0,65,now()+interval '30 minutes') RETURNING id,scope_id,model
         ), price AS (
            INSERT INTO relay_hosted.text_purchase_rates(item,revision,input_rate,cached_rate,output_rate)
            SELECT id,1,$7::text::numeric,$8::text::numeric,$9::text::numeric FROM stock RETURNING item
         ) INSERT INTO relay_hosted.text_model_rules(id,scope_id,model,admitted,purchase_revision)
         SELECT id,scope_id,model,true,1 FROM stock JOIN price ON id=item",
    ).bind(item).bind(scope).bind(source.id).bind(format!("{}/v1", source.listener.url)).bind(model)
        .bind(output).bind(rates[0]).bind(rates[1]).bind(rates[2]).execute(db).await.unwrap();
}

async fn publish(db: &PgPool, item: Uuid, revision: i64, input: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "WITH sale AS (
            INSERT INTO relay_hosted.text_offers(id,inventory_id,scope_id,source_id,provider_revision,base_url,model,revision,
                max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,input_rate,cached_rate,output_rate,close_after_s)
            SELECT gen_random_uuid(),id,scope_id,source_id,provider_revision,base_url,model,$2,max_input_bytes,max_output_tokens,
                tokens_per_byte,overhead_tokens,coalesce($3::text::numeric,3),.40,5,close_after_s
            FROM relay_hosted.text_inventory WHERE id=$1 RETURNING id
         ) UPDATE relay_hosted.text_model_rules SET enabled=true,last_offer=(SELECT id FROM sale),first_live_revision=1
         WHERE id=$1 RETURNING last_offer",
    ).bind(item).bind(revision).bind(input).fetch_one(db).await.unwrap()
}

async fn limit<'e>(db: impl PgExecutor<'e>, source: Uuid, limit: Option<i64>) {
    sqlx::query(
        "INSERT INTO relay_hosted.text_purchase_limit_changes(source_id,revision,operation,actor,limit_micro_usd)
         SELECT $1,coalesce(max(revision),0)+1,gen_random_uuid(),
            (SELECT user_id FROM relay_hosted.admin_assignments WHERE role='owner'),$2
         FROM relay_hosted.text_purchase_limit_changes WHERE source_id=$1",
    ).bind(source).bind(limit).execute(db).await.unwrap();
}

fn request_id(response: &reqwest::Response) -> Uuid {
    response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
}

async fn verify_receipt(db: &PgPool, rid: Uuid, item: Uuid, charge: i64, bound: i64, cost: &str) {
    let row: (Uuid, i64, i64, String, i64) = sqlx::query_as(
        "SELECT a.item,r.charged,a.bound_micro_usd,v.valued_micro_usd::text,
            (SELECT count(*) FROM relay_hosted.journal_transactions j WHERE j.request_id=r.id)
         FROM relay_hosted.text_requests r JOIN relay_hosted.text_purchase_attempts a USING(attempt_id)
         JOIN relay_hosted.text_purchase_results v USING(attempt_id) WHERE r.id=$1",
    ).bind(rid).fetch_one(db).await.unwrap();
    assert_eq!(row, (item, charge, bound, cost.into(), 2));
}

async fn lock_source<'e>(db: impl PgExecutor<'e>, source: Uuid) {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('text-source:'||$1::text,0))")
        .bind(source.to_string())
        .execute(db)
        .await
        .unwrap();
}

async fn wait_for_worker(db: &PgPool, function: &str, event: Option<&str>, since: f64) {
    wait_for_workers(db, function, event, since, 1).await;
}

async fn wait_for_workers(
    db: &PgPool,
    function: &str,
    event: Option<&str>,
    since: f64,
    count: i64,
) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let seen: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE usename='route_worker'
                    AND position($1::text IN query)>0 AND ($2::text IS NULL OR wait_event=$2)
                    AND query_start>to_timestamp($3)",
            )
            .bind(function)
            .bind(event)
            .bind(since)
            .fetch_one(db)
            .await
            .unwrap();
            if seen >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("selection worker did not reach {function}"));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn automatic_selects_by_whole_request_then_respects_capacity_caps_and_cooldowns() {
    let f = Fixture::new().await;
    assert!(f.listed().await);
    f.verify_purchase(
        f.submit("input-heavy", 1000, 100).send().await.unwrap(),
        f.b.item,
        3500,
        1600,
        "1600",
    )
    .await;
    f.verify_purchase(
        f.submit("output-heavy", 100, 1000).send().await.unwrap(),
        f.a.item,
        5300,
        3200,
        "3200",
    )
    .await;
    assert_eq!(f.sends(), (1, 1));

    // Fill B's cap after its quote was selected, but before the physical permit.
    let mut cap = f.admin.begin().await.unwrap();
    lock_source(&mut *cap, f.b.id).await;
    let request = tokio::spawn(f.submit("cap-race", 1000, 100).send());
    wait_for_worker(&f.admin, "claim_text_candidate", Some("advisory"), 0.0).await;
    limit(&mut *cap, f.b.id, Some(0)).await;
    cap.commit().await.unwrap();
    f.verify_purchase(
        request.await.unwrap().unwrap(),
        f.a.item,
        3500,
        2300,
        "2300",
    )
    .await;
    f.verify_purchase(
        f.submit("skip-capped", 1000, 100).send().await.unwrap(),
        f.a.item,
        3500,
        2300,
        "2300",
    )
    .await;
    limit(&f.admin, f.a.id, Some(0)).await;
    let unavailable = f.submit("no-headroom", 1000, 100).send().await.unwrap();
    let released = request_id(&unavailable);
    assert_eq!(unavailable.status(), 503);
    assert_eq!(
        unavailable.json::<Value>().await.unwrap()["error"]["code"],
        "relay_not_dispatched"
    );
    assert_eq!(f.sends(), (3, 1));
    let state: (String, i64, i64) = sqlx::query_as(
        "SELECT r.basis,r.charged,w.reserved_micro_usd FROM relay_hosted.text_requests r
         JOIN relay_hosted.wallets w USING(workspace_id) WHERE r.id=$1",
    )
    .bind(released)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert_eq!(state, ("not_sent".into(), 0, 0));
    assert!(f.listed().await);

    limit(&f.admin, f.a.id, None).await;
    limit(&f.admin, f.b.id, None).await;
    f.b.provider.retry_after.store(8 * 24 * 3600, SeqCst);
    let limited = f
        .submit("limited-after-send", 1000, 100)
        .send()
        .await
        .unwrap();
    let limited_id = request_id(&limited);
    assert_eq!(limited.status(), 503);
    assert_eq!(
        limited.json::<Value>().await.unwrap()["error"]["code"],
        "relay_outcome_pending"
    );
    let wait: (Uuid, String, bool) = sqlx::query_as(
        "SELECT source_id,model,not_before>clock_timestamp()+interval '7 days 23 hours' FROM relay_hosted.text_route_waits",
    ).fetch_one(&f.admin).await.unwrap();
    assert_eq!(wait, (f.b.id, MODEL.into(), true));
    assert_eq!(f.sends(), (3, 2), "a 429 is not a retry grant");
    let duplicate = f
        .submit("limited-after-send", 1000, 100)
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), 409);
    assert_eq!(request_id(&duplicate), limited_id);
    f.verify_purchase(
        f.submit("next-operation", 1000, 100).send().await.unwrap(),
        f.a.item,
        3500,
        2300,
        "2300",
    )
    .await;
    assert_eq!(f.sends(), (4, 2));

    f.a.provider.retry_after.store(u64::MAX, SeqCst);
    let too_far = f
        .submit("unrepresentable-wait", 1000, 100)
        .send()
        .await
        .unwrap();
    assert_eq!(too_far.status(), 503);
    let indefinite: bool = sqlx::query_scalar(
        "SELECT not_before='infinity'::timestamptz FROM relay_hosted.text_route_waits WHERE source_id=$1",
    ).bind(f.a.id).fetch_one(&f.admin).await.unwrap();
    assert!(indefinite, "an unrepresentable wait must remain blocked");
    let all_waiting = f.submit("all-cooling", 1000, 100).send().await.unwrap();
    assert_eq!(all_waiting.status(), 503);
    assert_eq!(
        all_waiting.json::<Value>().await.unwrap()["error"]["code"],
        "relay_not_dispatched"
    );
    assert_eq!(f.sends(), (5, 2));
}

struct Held {
    id: Uuid,
    attempt: Uuid,
    key: Uuid,
    epoch: i64,
    amount: i64,
    offer: Uuid,
    tariff: i64,
    model: &'static str,
}
impl Held {
    async fn reserve(f: &Fixture, key: Uuid, bearer: &str) -> Self {
        Self::reserve_model(f, key, bearer, MODEL).await
    }

    async fn reserve_model(f: &Fixture, key: Uuid, bearer: &str, model: &'static str) -> Self {
        let (state, id, attempt, space, epoch, amount, offer, tariff): (
            String,
            Uuid,
            Uuid,
            Uuid,
            i64,
            i64,
            Uuid,
            i64,
        ) = sqlx::query_as(
            "SELECT * FROM relay_hosted.reserve_published_text($1,$2,$3,NULL,$4,$4,$5,1000,100,false)",
        )
        .bind(ORIGIN)
        .bind(key)
        .bind(digest(&SHA256, bearer.as_bytes()).as_ref())
        .bind(SEAL)
        .bind(model)
        .fetch_one(&f.worker)
        .await
        .unwrap();
        assert_eq!((state.as_str(), space), ("new", f.space));
        Self {
            id,
            attempt,
            key,
            epoch,
            amount,
            offer,
            tariff,
            model,
        }
    }

    async fn quotes(&self, worker: &PgPool) -> Vec<(Uuid, i64, i64)> {
        sqlx::query_as(
            "SELECT (item->>'item')::uuid,(item->>'rate_revision')::bigint,(item->>'cost')::bigint
             FROM jsonb_array_elements(relay_hosted.request_text_candidates($1,$2,$3,$4,$5)->'items') item",
        ).bind(ORIGIN).bind(self.id).bind(self.attempt).bind(self.key).bind(SEAL).fetch_all(worker).await.unwrap()
    }

    async fn claim(&self, f: &Fixture, source: Uuid, item: Uuid, revision: i64) -> bool {
        self.claim_on(&f.worker, f.space, source, item, revision)
            .await
            == "claimed"
    }

    async fn claim_on<'e>(
        &self,
        db: impl PgExecutor<'e>,
        space: Uuid,
        source: Uuid,
        item: Uuid,
        revision: i64,
    ) -> String {
        sqlx::query_scalar(
            "SELECT relay_hosted.claim_text_candidate($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,1000,100,$12,$13)",
        ).bind(ORIGIN).bind(self.id).bind(self.attempt).bind(space).bind(self.key).bind(source).bind(self.model)
            .bind(self.epoch).bind(self.tariff).bind(self.amount).bind(SEAL).bind(item).bind(revision).fetch_one(db).await.unwrap()
    }

    fn attempt(&self) -> partial_text::Attempt {
        partial_text::Attempt {
            offer: self.offer,
            request: self.id,
            id: self.attempt,
        }
    }

    async fn finish(&self, worker: &PgPool, state: &str) -> String {
        let attempt = self.attempt();
        if state == "completed" {
            assert!(
                attempt
                    .capture(worker, state, [Some(1000), Some(0), Some(100)])
                    .await
            );
            return attempt.reconcile(worker, "deadline").await;
        }
        attempt.reconcile(worker, state).await
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn superseded_quotes_new_supply_and_revocation_cannot_widen_a_hold() {
    let f = Fixture::new().await;
    let incompatible = Uuid::new_v4();
    stock(
        &f.admin,
        f.scope,
        &f.b,
        incompatible,
        MODEL,
        99,
        ["0", "0", "0"],
    )
    .await;
    let review: Value = sqlx::query_scalar(
        "SELECT relay_hosted.text_price_inputs(i,false) FROM relay_hosted.text_inventory i WHERE i.id=$1",
    ).bind(f.a.item).fetch_one(&f.admin).await.unwrap();
    let rows = review.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|p| p["item"] == json!(f.b.item)));

    let hold = Held::reserve(&f, f.key, &f.bearer).await;
    assert_eq!((hold.offer, hold.amount), (f.offer, 3500));
    let mut quotes = hold.quotes(&f.worker).await;
    quotes.sort_unstable_by_key(|row| row.2);
    assert_eq!(quotes, [(f.b.item, 1, 1600), (f.a.item, 1, 2300)]);
    assert!(!hold.claim(&f, f.b.id, incompatible, 1).await);
    sqlx::query(
        "WITH next AS (
            INSERT INTO relay_hosted.text_purchase_rates(item,revision,input_rate,cached_rate,output_rate)
            VALUES($1,2,2,.25,6) RETURNING item
         ) UPDATE relay_hosted.text_model_rules SET purchase_revision=2 FROM next WHERE id=item",
    ).bind(f.b.item).execute(&f.admin).await.unwrap();
    assert!(!hold.claim(&f, f.b.id, f.b.item, 1).await, "stale quote");
    let new = Uuid::new_v4();
    stock(&f.admin, f.scope, &f.b, new, MODEL, 1000, ["1", ".25", "6"]).await;
    assert_eq!(hold.quotes(&f.worker).await, [(f.a.item, 1, 2300)]);
    assert!(!hold.claim(&f, f.b.id, new, 1).await, "unpinned inventory");
    let latest_offer = publish(&f.admin, f.a.item, 2, Some("2")).await;
    assert!(hold.claim(&f, f.a.id, f.a.item, 1).await);
    assert!(
        !hold.claim(&f, f.a.id, f.a.item, 1).await,
        "claim acknowledgement cannot be recovered as a permit"
    );
    assert_eq!(hold.finish(&f.worker, "completed").await, "settled");
    verify_receipt(&f.admin, hold.id, f.a.item, 3500, 2300, "2300").await;

    let (key, raw) = issue(&f.admin, f.scope, f.space).await;
    let revoked = Held::reserve(&f, key, &raw).await;
    assert_eq!(revoked.offer, latest_offer);
    sqlx::query("UPDATE relay_hosted.customer_keys SET revoked_at=now() WHERE id=$1")
        .bind(key)
        .execute(&f.admin)
        .await
        .unwrap();
    assert!(revoked.quotes(&f.worker).await.is_empty());
    assert!(!revoked.claim(&f, f.b.id, new, 1).await);
    assert_eq!(revoked.finish(&f.worker, "not_sent").await, "released");
    assert_eq!(f.sends(), (0, 0));

    assert_eq!(
        f.vault.revoke(f.a.id, Purpose::Provider, 1).await.unwrap(),
        2
    );
    assert!(
        f.listed().await,
        "one lost credential does not withdraw the offer when a compatible supplier remains"
    );
    f.verify_purchase(
        f.submit("after-anchor-loss", 1000, 100)
            .send()
            .await
            .unwrap(),
        new,
        2500,
        1600,
        "1600",
    )
    .await;
    f.b.provider.exceed_bound.store(true, SeqCst);
    f.verify_purchase(
        f.submit("bound-breach", 1000, 100).send().await.unwrap(),
        new,
        2500,
        1600,
        "1606",
    )
    .await;
    assert_eq!(f.sends(), (0, 2));
    let fences: Vec<Uuid> = sqlx::query_scalar("SELECT item FROM relay_hosted.text_route_fences")
        .fetch_all(&f.admin)
        .await
        .unwrap();
    assert_eq!(fences, [new]);
    assert!(!f.listed().await);
    assert_eq!(
        f.submit("no-qualified-supplier", 1000, 100)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(f.sends(), (0, 2));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn capacity_lost_between_quote_and_claim_can_wait_for_the_same_source() {
    let f = Fixture::new().await;
    limit(&f.admin, f.a.id, Some(0)).await;
    let occupying = Held::reserve(&f, f.key, &f.bearer).await;
    let mut permit = f.admin.begin().await.unwrap();
    lock_source(&mut *permit, f.b.id).await;
    let request = tokio::spawn(f.submit("capacity-race", 1000, 100).send());
    wait_for_worker(&f.admin, "claim_text_candidate", Some("advisory"), 0.0).await;

    // A different database connection claims the slot after the HTTP worker selected it.
    assert_eq!(
        occupying
            .claim_on(&mut *permit, f.space, f.b.id, f.b.item, 1)
            .await,
        "claimed"
    );
    let since: f64 =
        sqlx::query_scalar("SELECT extract(epoch FROM clock_timestamp())::double precision")
            .fetch_one(&mut *permit)
            .await
            .unwrap();
    permit.commit().await.unwrap();
    wait_for_worker(&f.admin, "request_text_candidates", None, since).await;
    assert_eq!(f.sends(), (0, 0));

    assert_eq!(occupying.finish(&f.worker, "completed").await, "settled");
    f.verify_purchase(
        request.await.unwrap().unwrap(),
        f.b.item,
        3500,
        1600,
        "1600",
    )
    .await;
    assert_eq!(f.sends(), (0, 1));
}
