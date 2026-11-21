#[path = "capture_poll.rs"]
mod capture_poll;

use super::{
    customer_reader::Reader,
    owner_usage_cases::{TAG, owner, request_detail},
    text_stream_fixture::{
        StreamWriter, qualifier, qualify, read_prefix, response_event, submit, usage,
    },
    *,
};
use tokio::time::{sleep, timeout};
use zenith_relay_hosted::config::CaptureLimits;

const CAPS: CaptureLimits = CaptureLimits {
    default_seconds: 60,
    max_seconds: 120,
    max_events: 16,
    max_bytes: 4096,
    max_events_per_second: 16,
};

async fn start(
    db: &PgPool,
    tag: &[u8],
    operation: Uuid,
    request: Uuid,
    seconds: i32,
    caps: Option<CaptureLimits>,
) -> Option<Value> {
    start_with_proof(db, tag, operation, request, seconds, caps, None).await
}

async fn arm(
    admin: &PgPool,
    db: &PgPool,
    operation: Uuid,
    request: Uuid,
    seconds: i32,
    caps: CaptureLimits,
) -> Option<Value> {
    let revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM relay_hosted.admin_credentials WHERE credential_id=decode('01','hex')",
    )
    .fetch_one(admin)
    .await
    .unwrap();
    start_with_proof(
        db,
        TAG,
        operation,
        request,
        seconds,
        Some(caps),
        Some(revision),
    )
    .await
}

async fn start_with_proof(
    db: &PgPool,
    tag: &[u8],
    operation: Uuid,
    request: Uuid,
    seconds: i32,
    caps: Option<CaptureLimits>,
    revision: Option<i64>,
) -> Option<Value> {
    sqlx::query_scalar(
        "SELECT relay_hosted.start_diagnostic_capture($1,$2,$3,$4,'request_timing',$5,$6,$7,$8,$9,
            CASE WHEN $10::bigint IS NOT NULL THEN decode('01','hex') END,$10,0,false,false,
            floor(extract(epoch FROM clock_timestamp())*1000)::bigint+30000)",
    )
    .bind(ORIGIN)
    .bind(tag)
    .bind(operation)
    .bind(request)
    .bind(seconds)
    .bind(caps.map(|c| c.max_seconds))
    .bind(caps.map(|c| c.max_events))
    .bind(caps.map(|c| c.max_bytes))
    .bind(caps.map(|c| c.max_events_per_second))
    .bind(revision)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn detail(db: &PgPool, tag: &[u8], id: Uuid) -> Option<Value> {
    sqlx::query_scalar("SELECT relay_hosted.owner_diagnostic_captures($1,$2,$3)")
        .bind(ORIGIN)
        .bind(tag)
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap()
}

async fn stop(db: &PgPool, tag: &[u8], id: Uuid) -> Option<Value> {
    sqlx::query_scalar("SELECT relay_hosted.stop_diagnostic_capture($1,$2,$3)")
        .bind(ORIGIN)
        .bind(tag)
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap()
}

async fn append(db: &PgPool, id: Uuid, r: &Held, code: &str) -> bool {
    append_observation(db, id, (r.id, r.attempt), code, None).await
}

async fn append_observation(
    db: &PgPool,
    id: Uuid,
    (request, attempt): (Uuid, Uuid),
    code: &str,
    at: Option<i64>,
) -> bool {
    sqlx::query_scalar(
        "SELECT relay_hosted.append_capture_milestone($1,$2,$3,$4::text::relay_hosted.capture_milestone,
            coalesce($5,floor(extract(epoch FROM clock_timestamp())*1000)::bigint))",
    )
    .bind(id)
    .bind(request)
    .bind(attempt)
    .bind(code)
    .bind(at)
    .fetch_one(db)
    .await
    .unwrap()
}

fn session(value: &Value) -> Uuid {
    value["session"]["id"]
        .as_str()
        .expect("capture acknowledgement")
        .parse()
        .unwrap()
}

async fn sql(db: &PgPool, text: &'static str) {
    sqlx::raw_sql(text).execute(db).await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn capture_fences_scope_retries_expiry_and_audit_recovery() {
    let f = Fixture::new().await;
    let management = owner(&f).await;
    let reader = Reader::new(&f, f.space, 8).await;
    let r = Held::reserve(&f, f.key, &f.bearer).await;
    let another = Held::reserve(&f, f.key, &f.bearer).await;
    let receipt = reader.current_receipt(r.id).await;
    let op = Uuid::new_v4();
    assert_eq!(
        start(&management, &reader.tag, op, r.id, 60, Some(CAPS)).await,
        None
    );
    assert_eq!(
        start(&management, TAG, op, r.id, 60, None).await.unwrap()["error"],
        "capture_disabled"
    );

    assert_eq!(
        start(&management, TAG, op, r.id, 60, Some(CAPS))
            .await
            .unwrap()["error"],
        "fresh_required"
    );
    assert_eq!(
        start(&management, TAG, op, Uuid::new_v4(), 60, Some(CAPS))
            .await
            .unwrap()["error"],
        "capture_request_unavailable",
    );
    let (a, b) = tokio::join!(
        arm(&f.admin, &management, op, r.id, 60, CAPS),
        arm(&f.admin, &management, op, r.id, 60, CAPS),
    );
    let started = a.unwrap()["session"].clone();
    assert_eq!(started, b.unwrap()["session"]);
    let id = started["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        started["expires_at_ms"].as_i64().unwrap() - started["started_at_ms"].as_i64().unwrap(),
        60000
    );
    assert_eq!(
        start(&management, TAG, op, another.id, 60, Some(CAPS))
            .await
            .unwrap()["error"],
        "capture_conflict"
    );
    assert_eq!(
        start(&management, TAG, Uuid::new_v4(), another.id, 60, Some(CAPS))
            .await
            .unwrap()["error"],
        "capture_busy",
    );
    assert_eq!(detail(&management, &reader.tag, id).await, None);
    assert_eq!(stop(&management, &reader.tag, id).await, None);
    for (db, query) in [
        (
            &management,
            "SELECT * FROM relay_hosted.diagnostic_sessions",
        ),
        (
            &management,
            "SELECT * FROM relay_hosted.diagnostic_capture_audit",
        ),
        (
            &management,
            "SELECT relay_hosted.append_capture_milestone(gen_random_uuid(),gen_random_uuid(),gen_random_uuid(),'first_stream_frame',1)",
        ),
        (
            &f.worker,
            "SELECT * FROM relay_hosted.diagnostic_capture_events",
        ),
        (
            &f.worker,
            "SELECT relay_hosted.owner_diagnostic_captures('http://localhost:1443','\\x01',NULL)",
        ),
        (&f.worker, "SELECT relay_hosted.sweep_diagnostic_capture()"),
        (
            &f.worker,
            "SELECT relay_hosted.reset_owner_browser_sessions('http://localhost:1443')",
        ),
    ] {
        let e = sqlx::query(query).execute(db).await.unwrap_err();
        assert_eq!(
            e.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("42501")
        );
    }
    assert!(
        append(&f.worker, id, &another, "first_stream_frame").await,
        "foreign target is ignored",
    );
    assert!(
        !append_observation(
            &f.worker,
            id,
            (r.id, another.attempt),
            "first_stream_frame",
            None
        )
        .await
    );
    assert!(!append(&f.worker, id, &r, "dispatch_committed").await);
    assert!(
        !append_observation(
            &f.worker,
            id,
            (r.id, r.attempt),
            "first_stream_frame",
            Some(1)
        )
        .await
    );
    assert!(append(&f.worker, id, &r, "first_stream_frame").await);
    assert!(append(&f.worker, id, &r, "first_stream_frame").await);
    let read = detail(&management, TAG, id).await.unwrap();
    assert_eq!(read["session"]["consumed"]["events"], 1);
    assert_eq!(
        read["events"][0],
        json!({
            "sequence":1, "code":"first_stream_frame", "occurred_at_ms":read["events"][0]["occurred_at_ms"],
        })
    );
    let frozen_expiry: bool = sqlx::query_scalar(
        "SELECT e.expires_at=r.started_at+interval '720 hours' FROM relay_hosted.diagnostic_capture_events e
         JOIN relay_hosted.text_requests r ON r.id=e.request_id",
    ).fetch_one(&f.admin).await.unwrap();
    assert!(frozen_expiry);
    let own = reader.current_receipt(r.id).await;
    assert_eq!(own["items"][0]["request_id"], r.id.to_string());
    assert_eq!(own.get("diagnostic"), Some(&Value::Null));
    assert_eq!(own, receipt);
    assert_eq!(
        request_detail(&management, r.id).await["diagnostics"],
        json!([])
    );
    // A new connection can discover the same target. A lower configured ceiling cannot.
    let reconnected = db::connect(&fixture_config(), &f._cluster.dsn("route_worker"))
        .await
        .unwrap();
    let target: (Uuid, Uuid) =
        sqlx::query_as("SELECT * FROM relay_hosted.capture_target($1,120,16,4096,16)")
            .bind(ORIGIN)
            .fetch_one(&reconnected)
            .await
            .unwrap();
    assert_eq!(target, (id, r.id));
    let denied: Option<Uuid> =
        sqlx::query_scalar("SELECT session_id FROM relay_hosted.capture_target($1,120,16,128,16)")
            .bind(ORIGIN)
            .fetch_optional(&reconnected)
            .await
            .unwrap();
    assert!(denied.is_none());
    assert_eq!(
        detail(&management, TAG, id).await.unwrap()["session"]["consumed"]["events"],
        1
    );

    // Disarm independently of audit availability; both start and close records are recoverable.
    sql(
        &f.admin,
        "CREATE FUNCTION relay_hosted.fixture_audit_failure() RETURNS trigger LANGUAGE plpgsql AS
             $$ BEGIN RAISE EXCEPTION 'fixture audit failure'; END $$;
         CREATE TRIGGER fixture_audit_failure BEFORE INSERT ON relay_hosted.diagnostic_capture_audit
             FOR EACH ROW WHEN (NEW.action='close') EXECUTE FUNCTION relay_hosted.fixture_audit_failure();",
    ).await;
    sql(
        &f.admin,
        "UPDATE relay_hosted.admin_sessions SET created_at=now()-interval '3 minutes'",
    )
    .await;
    let mut unavailable = f.admin.begin().await.unwrap();
    sqlx::query("LOCK TABLE relay_hosted.diagnostic_capture_audit IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *unavailable)
        .await
        .unwrap();
    let stopped = timeout(Duration::from_secs(3), stop(&management, TAG, id))
        .await
        .expect("stop must not depend on audit reads or writes")
        .unwrap()["session"]
        .clone();
    unavailable.rollback().await.unwrap();
    assert_eq!(stopped["state"], "stopped");
    assert_eq!(stopped["audit_pending"], true);
    assert!(
        sqlx::query("SELECT relay_hosted.prune_diagnostic_captures()")
            .execute(&management)
            .await
            .is_err()
    );
    assert_eq!(
        stop(&management, TAG, id).await.unwrap()["session"],
        stopped
    );
    assert!(append(&reconnected, id, &r, "terminal_observed").await);
    assert_eq!(
        detail(&management, TAG, id).await.unwrap()["events"],
        read["events"]
    );
    assert_eq!(
        start(&management, TAG, op, r.id, 60, None).await.unwrap()["session"],
        stopped
    );

    let expired = session(
        &arm(&f.admin, &management, Uuid::new_v4(), another.id, 1, CAPS)
            .await
            .unwrap(),
    );
    let queued_at = detail(&management, TAG, expired).await.unwrap()["session"]["started_at_ms"]
        .as_i64()
        .unwrap();
    sleep(Duration::from_millis(1050)).await;
    assert_eq!(
        detail(&management, TAG, expired).await.unwrap()["session"]["state"],
        "expired"
    );
    let delayed = |id| {
        append_observation(
            &reconnected,
            id,
            (another.id, another.attempt),
            "terminal_observed",
            Some(queued_at),
        )
    };
    assert!(delayed(expired).await);
    assert_eq!(
        detail(&management, TAG, expired).await.unwrap()["events"],
        json!([])
    );
    let revoked = session(
        &arm(&f.admin, &management, Uuid::new_v4(), another.id, 60, CAPS)
            .await
            .unwrap(),
    );
    assert!(delayed(expired).await);
    assert!(!delayed(revoked).await);
    assert_eq!(
        detail(&management, TAG, revoked).await.unwrap()["events"],
        json!([])
    );
    let signed_out: bool = sqlx::query_scalar("SELECT relay_hosted.end_admin_session($1)")
        .bind(TAG)
        .fetch_one(&management)
        .await
        .unwrap();
    assert!(signed_out);
    sql(
        &management,
        "SELECT relay_hosted.sweep_diagnostic_capture()",
    )
    .await;
    assert!(append(&reconnected, revoked, &another, "terminal_observed").await);
    let reason: String = sqlx::query_scalar(
        "SELECT stop_reason::text FROM relay_hosted.diagnostic_sessions WHERE id=$1",
    )
    .bind(revoked)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert_eq!(reason, "revoked");
    assert_eq!(detail(&management, TAG, revoked).await, None);

    sql(
        &f.admin,
        "DROP TRIGGER fixture_audit_failure ON relay_hosted.diagnostic_capture_audit",
    )
    .await;
    const PRUNE: &str = "SELECT relay_hosted.prune_diagnostic_captures()";
    sql(&management, PRUNE).await;
    let audit: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT session_id,stop_reason::text FROM relay_hosted.diagnostic_capture_audit
         WHERE action='close' ORDER BY stop_reason::text",
    )
    .fetch_all(&f.admin)
    .await
    .unwrap();
    assert_eq!(
        audit,
        [
            (expired, "expired".into()),
            (revoked, "revoked".into()),
            (id, "stopped".into())
        ]
    );
    sql(
        &f.admin,
        "UPDATE relay_hosted.diagnostic_sessions SET started_at=started_at-interval '15 days',
            ended_at=ended_at-interval '15 days',expires_at=expires_at-interval '15 days'",
    )
    .await;
    sql(&management, PRUNE).await;
    let (unlinked, audited): (bool, i64) = sqlx::query_as(
        "SELECT (SELECT session_id IS NULL FROM relay_hosted.diagnostic_capture_events),
         (SELECT count(*) FROM relay_hosted.diagnostic_capture_audit)",
    )
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert!(unlinked);
    assert_eq!(audited, 6);
    let starts_with_passkey: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM relay_hosted.diagnostic_capture_audit
         WHERE action='start' AND confirmed_by_passkey_id IS NOT NULL",
    )
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert_eq!(starts_with_passkey, 3);
    sql(&f.admin, "UPDATE relay_hosted.diagnostic_capture_events SET recorded_at=now()-interval '2 days',expires_at=now()-interval '1 day'").await;
    sql(&management, "SELECT relay_hosted.prune_capture_events()").await;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM relay_hosted.diagnostic_capture_events")
            .fetch_one(&f.admin)
            .await
            .unwrap();
    assert_eq!(count, 0);
    assert_eq!(f.sends(), (0, 0));
    let held = r.amount + another.amount;
    payment::assert_wallet_with_holds(&f.admin, f.space, CREDIT - held, held, 3).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn preparing_a_capture_cannot_close_one_or_arm_a_finished_request() {
    let f = Fixture::new().await;
    let management = owner(&f).await;
    let old = Held::reserve(&f, f.key, &f.bearer).await;
    let next = Held::reserve(&f, f.key, &f.bearer).await;
    let id = session(
        &arm(&f.admin, &management, Uuid::new_v4(), old.id, 60, CAPS)
            .await
            .unwrap(),
    );
    sqlx::query(
        "UPDATE relay_hosted.diagnostic_sessions SET started_at=now()-interval '2 minutes',
           expires_at=now()-interval '1 minute' WHERE id=$1",
    )
    .bind(id)
    .execute(&f.admin)
    .await
    .unwrap();
    let unchanged = || {
        sqlx::query_as::<_, (bool, i64, i64)>(
            "SELECT ended_at IS NULL AND proof_tag IS NOT NULL,
               (SELECT count(*) FROM relay_hosted.diagnostic_sessions),
               (SELECT count(*) FROM relay_hosted.diagnostic_capture_audit)
             FROM relay_hosted.diagnostic_sessions WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&f.admin)
    };
    let op = Uuid::new_v4();
    assert_eq!(
        start(&management, TAG, op, next.id, 60, Some(CAPS))
            .await
            .unwrap()["error"],
        "fresh_required",
    );
    assert_eq!(unchanged().await.unwrap(), (true, 1, 1));
    assert_eq!(next.finish(&f.worker, "not_sent").await, "released");
    assert_eq!(
        arm(&f.admin, &management, op, next.id, 60, CAPS)
            .await
            .unwrap()["error"],
        "capture_request_unavailable",
    );
    assert_eq!(unchanged().await.unwrap(), (true, 1, 1));
    assert_eq!(f.sends(), (0, 0));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn capture_is_fenced_by_the_web_process_session_reset() {
    let f = Fixture::new().await;
    let management = owner(&f).await;
    let reader = Reader::new(&f, f.space, 8).await;
    let r = Held::reserve(&f, f.key, &f.bearer).await;
    let receipt = reader.current_receipt(r.id).await;
    let id = session(
        &arm(&f.admin, &management, Uuid::new_v4(), r.id, 60, CAPS)
            .await
            .unwrap(),
    );
    let target = || {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT session_id FROM relay_hosted.capture_target($1,120,16,4096,16)",
        )
        .bind(ORIGIN)
        .fetch_optional(&f.worker)
    };
    sql(
        &management,
        "SELECT relay_hosted.reset_owner_browser_sessions('https://other.example')",
    )
    .await;
    assert_eq!(target().await.unwrap(), Some(id));
    assert!(append(&f.worker, id, &r, "first_stream_frame").await);
    sqlx::query("SELECT relay_hosted.reset_owner_browser_sessions($1)")
        .bind(ORIGIN)
        .execute(&management)
        .await
        .unwrap();
    assert_eq!(target().await.unwrap(), None);
    assert_eq!(detail(&management, TAG, id).await, None);
    assert!(append(&f.worker, id, &r, "terminal_observed").await);
    let closed: (String, i32, bool) = sqlx::query_as(
        "SELECT stop_reason::text,consumed_events,proof_tag IS NULL
         FROM relay_hosted.diagnostic_sessions WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert_eq!(closed, ("revoked".into(), 1, true));
    assert_eq!(reader.current_receipt(r.id).await, receipt);
    assert_eq!(f.sends(), (0, 0));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn capture_budgets_are_atomic_under_concurrent_writers() {
    let f = Fixture::new().await;
    let management = owner(&f).await;
    let r = Held::reserve(&f, f.key, &f.bearer).await;
    for (max_events, max_bytes, max_events_per_second, expected) in
        [(2, 4096, 16, 2), (16, 128, 16, 1), (16, 4096, 1, 1)]
    {
        let caps = CaptureLimits {
            max_events,
            max_bytes,
            max_events_per_second,
            ..CAPS
        };
        let op = Uuid::new_v4();
        let id = session(
            &arm(&f.admin, &management, op, r.id, 60, caps)
                .await
                .unwrap(),
        );
        let (a, b, c) = tokio::join!(
            append(&f.worker, id, &r, "upstream_response"),
            append(&f.worker, id, &r, "first_stream_frame"),
            append(&f.worker, id, &r, "terminal_observed"),
        );
        assert!(a && b && c);
        let v = detail(&management, TAG, id).await.unwrap();
        let snapshot = &v["session"];
        assert_eq!(snapshot["state"], "exhausted");
        assert_eq!(snapshot["consumed"]["events"], expected);
        assert!(snapshot["consumed"]["bytes"].as_i64().unwrap() <= i64::from(caps.max_bytes));
        assert_eq!(v["events"].as_array().unwrap().len(), expected);
        assert_eq!(
            start(&management, TAG, op, r.id, 60, Some(CAPS))
                .await
                .unwrap()["session"],
            *snapshot
        );
    }
    assert_eq!(f.sends(), (0, 0));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn capture_sink_failure_does_not_change_a_stream_or_its_charge() {
    let mut f = Fixture::new().await;
    let management = owner(&f).await;
    let reader = Reader::new(&f, f.space, 8).await;
    f.server = Listener::start(
        inference::router(
            f.worker.clone(),
            ORIGIN,
            f.vault.clone(),
            OutboundPolicy::default(),
            Some(CAPS),
        )
        .await
        .unwrap(),
    )
    .await;
    qualify(&f, &qualifier(&f).await, &f.a).await;
    let wire = StreamWriter::arm(&f.a, "text/event-stream");
    let running = tokio::spawn(submit(&f, "capture-sink-failure").send());
    let rid = timeout(Duration::from_secs(6), async {
        loop {
            let found: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM relay_hosted.text_requests WHERE execution='dispatched'",
            )
            .fetch_optional(&f.admin)
            .await
            .unwrap();
            if let Some(id) = found {
                break id;
            }
            sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("committed stream dispatch");
    let id = session(
        &arm(&f.admin, &management, Uuid::new_v4(), rid, 60, CAPS)
            .await
            .unwrap(),
    );
    capture_poll::subscribed(&f.admin, "route_worker").await;
    let created = response_event("in_progress", 0, Value::Null);
    wire.write(created.clone());
    let mut reply = running.await.unwrap().unwrap();
    assert_eq!(reply.status(), 200);
    read_prefix(&mut reply, &created).await;
    let before = timeout(Duration::from_secs(6), async {
        loop {
            let events = detail(&management, TAG, id).await.unwrap()["events"].clone();
            if events
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["code"] == "first_stream_frame")
            {
                break events;
            }
            sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("the real stream milestone reached the collector");
    sql(
        &f.admin,
        "REVOKE EXECUTE ON FUNCTION relay_hosted.append_capture_milestone(
        uuid,uuid,uuid,relay_hosted.capture_milestone,bigint) FROM route_worker",
    )
    .await;
    inference::check_worker(&f.worker, ORIGIN).await.unwrap();
    let terminal = response_event("completed", 2, usage(1000, 100, 20));
    wire.write(terminal.clone());
    assert_eq!(reply.bytes().await.unwrap(), terminal);
    wire.cancelled().await;
    verify_receipt(&f.admin, rid, f.a.item, 2840, 2300, "1910").await;
    payment::assert_wallet(&f.admin, f.space, CREDIT - 2840, 3).await;
    assert_eq!(f.sends(), (2, 0), "capture must not cause a replay");
    let after = detail(&management, TAG, id).await.unwrap();
    assert_eq!(after["events"], before);
    assert_eq!(after["coverage"], "partial");
    assert!(reader.current_receipt(rid).await.get("events").is_none());
    assert!(!after.to_string().contains("resp_fixture"));
    assert!(!after.to_string().contains("fixture-supplier"));
}
