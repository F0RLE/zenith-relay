use super::{
    text_late_cases::at_status,
    text_stream_fixture::{
        PRIVATE_ERROR, StreamWriter, check, delta, duplicate, qualifier, qualify, read_prefix,
        response_event, submit, usage,
    },
    *,
};
use axum::body::Bytes;
use tokio::time::{sleep, timeout};

const SSE: &str = "text/event-stream";

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn sse_requires_its_own_check_streams_original_bytes_and_charges_once() {
    let f = Fixture::new().await;
    assert!(f.listed().await);
    let no_stream = submit(&f, "native-stream").send().await.unwrap();
    assert_eq!(no_stream.status(), 403);
    assert!(!no_stream.headers().contains_key("x-request-id"));
    assert_eq!(f.sends(), (0, 0));
    payment::assert_wallet(&f.admin, f.space, CREDIT, 1).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM relay_hosted.text_requests")
            .fetch_one(&f.admin)
            .await
            .unwrap(),
        0
    );

    let q = qualifier(&f).await;
    let op = qualify(&f, &q, &f.a).await;
    check(&f, &q, op, f.a.item).await.unwrap();
    assert!(check(&f, &q, Uuid::new_v4(), f.a.item).await.is_err());
    assert_eq!(f.sends(), (1, 0), "the qualifier did not repeat the check");

    let upstream = StreamWriter::arm(&f.a, SSE);
    let preamble = b": fixture\r\nid: sample\r\nretry: 500\r\n\r\n";
    let created = response_event("in_progress", 0, Value::Null);
    upstream.write(preamble.as_slice());
    upstream.write(created.clone());
    let mut response = submit(&f, "native-stream").send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], SSE);
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    let request = request_id(&response);
    read_prefix(&mut response, &[preamble.as_slice(), &created].concat()).await;

    let bytes = delta(10);
    let middle = bytes.windows(3).position(|s| s == "✓".as_bytes()).unwrap() + 1;
    upstream.write(bytes.slice(..middle));
    sleep(Duration::from_millis(15)).await;
    upstream.write(bytes.slice(middle..));
    read_prefix(&mut response, &bytes).await;
    let running = at_status(&f, request).await;
    assert_eq!(running["execution"], "dispatched");
    assert_eq!(running["hold_micro_usd"], "3500");
    assert!(running["charge_micro_usd"].is_null());
    assert!(source_busy(&f, f.a.id).await);
    duplicate(&f, "native-stream", request, "relay_request_in_progress").await;
    let conflict = f.submit("native-stream", 1000, 100).send().await.unwrap();
    assert_eq!(conflict.status(), 409);
    assert!(!conflict.headers().contains_key("x-request-id"));
    assert_eq!(
        conflict.json::<Value>().await.unwrap()["error"]["code"],
        "relay_idempotency_conflict"
    );

    let end = response_event("completed", 11, usage(1000, 100, 20));
    upstream.write([end.as_ref(), end.as_ref(), b"data: [DONE]\r\n\r\n"].concat());
    assert_eq!(response.bytes().await.unwrap(), end);
    upstream.cancelled().await;
    assert!(!source_busy(&f, f.a.id).await);
    verify_receipt(&f.admin, request, f.a.item, 2840, 2300, "1910").await;
    payment::assert_wallet(&f.admin, f.space, CREDIT - 2840, 3).await;
    let receipt = at_status(&f, request).await;
    assert_eq!(receipt["execution"], "completed");
    assert_eq!(receipt["basis"], "provider_final");
    assert_eq!(receipt["total_tokens"], "1020");
    let elapsed: i32 = sqlx::query_scalar(
        "SELECT elapsed_ms FROM relay_hosted.text_completion_timings WHERE request_id=$1",
    )
    .bind(request)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert!((15..=90_000).contains(&elapsed));
    duplicate(
        &f,
        "native-stream",
        request,
        "relay_request_result_unavailable",
    )
    .await;
    assert_eq!(f.sends(), (2, 0), "cheaper JSON-only supply did not send");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn sse_distinguishes_incomplete_failed_and_invalid_upstream_responses() {
    let f = Fixture::new().await;
    let q = qualifier(&f).await;
    let invalid = StreamWriter::arm(&f.b, SSE);
    invalid.write(response_event("in_progress", 0, Value::Null));
    invalid.write(delta(1));
    invalid.write(response_event(
        "completed",
        2,
        json!({"input_tokens":9,"output_tokens":2}),
    ));
    let op = Uuid::new_v4();
    assert!(check(&f, &q, op, f.b.item).await.is_err());
    assert!(check(&f, &q, op, f.b.item).await.is_err());
    assert_eq!(
        submit(&f, "failed-probe").send().await.unwrap().status(),
        403
    );
    assert_eq!(f.sends(), (0, 1));
    payment::assert_wallet(&f.admin, f.space, CREDIT, 1).await;
    qualify(&f, &q, &f.a).await;

    for (state, charge, purchase) in [("incomplete", 3240, "2150"), ("failed", 2765, "1865")] {
        let up = StreamWriter::arm(&f.a, SSE);
        let mut response = submit(&f, state).send().await.unwrap();
        let id = request_id(&response);
        let start = response_event("in_progress", 0, Value::Null);
        up.write(start.clone());
        up.write(delta(10));
        read_prefix(&mut response, &[start.as_ref(), &delta(10)].concat()).await;
        let end = response_event(
            state,
            11,
            usage(1000, 100, if state == "failed" { 5 } else { 100 }),
        );
        up.write(end.clone());
        let rest = response.bytes().await.unwrap();
        if state == "incomplete" {
            assert_eq!(rest, end);
        } else {
            assert_error(&rest, "relay_upstream_failed", 12);
            assert!(!String::from_utf8_lossy(&rest).contains(PRIVATE_ERROR));
            wait_for_diagnostic(&f, id, "upstream_terminal").await;
        }
        verify_receipt(&f.admin, id, f.a.item, charge, 2300, purchase).await;
        assert_eq!(at_status(&f, id).await["execution"], state);
        assert!(
            !sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT FROM relay_hosted.text_completion_timings WHERE request_id=$1)",
            )
            .bind(id)
            .fetch_one(&f.admin)
            .await
            .unwrap()
        );
    }

    let _wrong_mime = StreamWriter::arm(&f.a, "application/json");
    let refused = submit(&f, "wrong-mime").send().await.unwrap();
    assert_eq!(refused.status(), 503);
    let rid = request_id(&refused);
    assert_eq!(
        refused.json::<Value>().await.unwrap()["error"]["code"],
        "relay_outcome_pending"
    );
    assert!(at_status(&f, rid).await["charge_micro_usd"].is_null());
    wait_for_diagnostic(&f, rid, "adapter_upstream_stream_invalid").await;
    duplicate(&f, "wrong-mime", rid, "relay_request_in_progress").await;
    assert_eq!(f.sends(), (4, 1));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; exercises the upstream idle limit"]
async fn sse_foreign_terminal_and_idle_upstream_leave_the_attempt_uncertain() {
    let f = Fixture::new().await;
    let q = qualifier(&f).await;
    qualify(&f, &q, &f.a).await;
    qualify(&f, &q, &f.b).await;
    for (op, source, code) in [
        ("foreign-terminal", &f.b, "adapter_upstream_stream_invalid"),
        ("idle-upstream", &f.a, "upstream_transport_timeout"),
    ] {
        let writer = StreamWriter::arm(source, SSE);
        let mut reply = submit(&f, op)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .unwrap();
        let request = request_id(&reply);
        let start = response_event("in_progress", 0, Value::Null);
        writer.write(start.clone());
        writer.write(delta(1));
        read_prefix(&mut reply, &[start.as_ref(), &delta(1)].concat()).await;
        if op == "foreign-terminal" {
            let end = response_event("completed", 2, usage(1000, 100, 20));
            writer.write(
                std::str::from_utf8(&end)
                    .unwrap()
                    .replace(MODEL, "other-model"),
            );
        }
        let body = timeout(Duration::from_secs(15), reply.bytes())
            .await
            .expect("worker did not bound the upstream read")
            .unwrap();
        assert_error(&body, "relay_outcome_pending", 2);
        writer.cancelled().await;
        let receipt = at_status(&f, request).await;
        assert_eq!(receipt["execution"], "uncertain");
        assert!(receipt["charge_micro_usd"].is_null());
        wait_for_diagnostic(&f, request, code).await;
        duplicate(&f, op, request, "relay_request_in_progress").await;
    }
    assert_eq!(f.sends(), (2, 2));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn sse_transport_error_after_output_is_not_a_graceful_eof() {
    let f = Fixture::new().await;
    qualify(&f, &qualifier(&f).await, &f.a).await;
    let upstream = StreamWriter::arm(&f.a, SSE);
    let mut reply = submit(&f, "interrupted-body").send().await.unwrap();
    let rid = request_id(&reply);
    let start = response_event("in_progress", 0, Value::Null);
    upstream.write(start.clone());
    upstream.write(delta(1));
    read_prefix(&mut reply, &[start.as_ref(), &delta(1)].concat()).await;
    upstream.interrupt_read();
    assert_error(&reply.bytes().await.unwrap(), "relay_outcome_pending", 2);
    wait_for_diagnostic(&f, rid, "upstream_transport_body").await;
    let receipt = at_status(&f, rid).await;
    assert_eq!(receipt["execution"], "uncertain");
    assert!(receipt["charge_micro_usd"].is_null());
    duplicate(&f, "interrupted-body", rid, "relay_request_in_progress").await;
    assert_eq!(f.sends(), (2, 0));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; waits for the saved request deadlines"]
async fn sse_eof_and_disconnect_close_only_at_the_saved_deadline() {
    let f = Fixture::new().await;
    let q = qualifier(&f).await;
    qualify(&f, &q, &f.a).await;
    qualify(&f, &q, &f.b).await;

    let eof = StreamWriter::arm(&f.b, SSE);
    let mut reply = submit(&f, "partial-eof").send().await.unwrap();
    let partial = request_id(&reply);
    let snapshots = [
        response_event(
            "in_progress",
            0,
            json!({"input_tokens":1000,"input_tokens_details":{"cached_tokens":100}}),
        ),
        response_event("in_progress", 1, json!({"output_tokens":9})),
        response_event("in_progress", 2, usage(1, 0, 10)),
        response_event("in_progress", 3, usage(1000, 120, 7)),
        response_event(
            "in_progress",
            4,
            json!({"input_tokens":1000,"output_tokens":8}),
        ),
        delta(5),
    ];
    for bytes in &snapshots {
        eof.write(bytes.clone());
    }
    read_prefix(&mut reply, &snapshots.as_slice().concat()).await;
    drop(eof);
    assert_error(&reply.bytes().await.unwrap(), "relay_outcome_pending", 6);
    let receipt = at_status(&f, partial).await;
    assert_eq!(receipt["execution"], "uncertain");
    assert_eq!(receipt["input_tokens"], "1000");
    assert_eq!(receipt["cached_tokens"], "120");
    assert_eq!(receipt["output_tokens"], "10");
    assert!(receipt["charge_micro_usd"].is_null());
    assert!(source_busy(&f, f.b.id).await);
    wait_for_diagnostic(&f, partial, "hosted_terminal_missing").await;

    let writer = StreamWriter::arm(&f.a, SSE);
    let mut reply = submit(&f, "disconnect").send().await.unwrap();
    let waived = request_id(&reply);
    let start = response_event("in_progress", 0, Value::Null);
    writer.write(start.clone());
    writer.write(delta(1));
    read_prefix(&mut reply, &[start.as_ref(), &delta(1)].concat()).await;
    drop(reply);
    writer.cancelled().await;
    assert!(source_busy(&f, f.a.id).await);
    wait_for_diagnostic(&f, waived, "hosted_delivery_closed").await;
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 0);
    duplicate(&f, "disconnect", waived, "relay_request_in_progress").await;
    let remaining: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM max(close_by)-clock_timestamp())::double precision
         FROM relay_hosted.text_requests WHERE id=ANY($1)",
    )
    .bind([partial, waived])
    .fetch_one(&f.admin)
    .await
    .unwrap();
    sleep(Duration::from_secs_f64(remaining.max(0.0) + 0.1)).await;
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 2);
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 0);

    let receipt = at_status(&f, partial).await;
    assert_eq!(receipt["execution"], "uncertain");
    assert_eq!(receipt["basis"], "provider_partial");
    assert_eq!(receipt["charge_micro_usd"], "2738");
    let receipt = at_status(&f, waived).await;
    assert_eq!(receipt["execution"], "uncertain");
    assert_eq!(receipt["basis"], "policy_waiver");
    assert!(receipt["input_tokens"].is_null() && receipt["output_tokens"].is_null());
    assert_eq!(receipt["charge_micro_usd"], "0");
    payment::assert_wallet(&f.admin, f.space, CREDIT - 2738, 5).await;
    for (source, bound) in [(&f.a, "2300"), (&f.b, "1600")] {
        assert!(!source_busy(&f, source.id).await);
        let period: (String, String) = sqlx::query_as(
            "SELECT accounted_micro_usd::text,outstanding_micro_usd::text
             FROM relay_hosted.text_purchase_periods WHERE source_id=$1",
        )
        .bind(source.id)
        .fetch_one(&f.admin)
        .await
        .unwrap();
        assert_eq!(period, ("0".into(), bound.into()));
    }
    let terminals: i64 =
        sqlx::query_scalar("SELECT count(*) FROM relay_hosted.text_terminal_evidence")
            .fetch_one(&f.admin)
            .await
            .unwrap();
    assert_eq!(
        terminals, 0,
        "neither EOF nor cancellation is a provider terminal"
    );
    assert_eq!(f.sends(), (2, 2));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn sse_terminal_survives_accounting_timeout_and_restart_without_replay() {
    let mut f = Fixture::new().await;
    qualify(&f, &qualifier(&f).await, &f.a).await;
    let up = StreamWriter::arm(&f.a, SSE);
    let mut reply = submit(&f, "stream-wallet-lock").send().await.unwrap();
    let rid = request_id(&reply);
    let created = response_event("in_progress", 0, Value::Null);
    up.write(created.clone());
    up.write(delta(1));
    read_prefix(&mut reply, &[created.as_ref(), &delta(1)].concat()).await;
    let mut wallet = f.admin.begin().await.unwrap();
    sqlx::query("SELECT FROM relay_hosted.wallets WHERE workspace_id=$1 FOR NO KEY UPDATE")
        .bind(f.space)
        .execute(&mut *wallet)
        .await
        .unwrap();
    up.write(response_event("completed", 2, usage(1000, 100, 20)));
    assert_error(&reply.bytes().await.unwrap(), "relay_outcome_pending", 3);
    let row: (String, i64, bool, bool, Option<i64>) = sqlx::query_as(
        "SELECT e.outcome,e.output_tokens,e.observed_at<r.close_by,e.applied_at IS NULL,r.charged
         FROM relay_hosted.text_requests r JOIN relay_hosted.text_terminal_evidence e ON e.request_id=r.id
         WHERE r.id=$1",
    )
    .bind(rid).fetch_one(&mut *wallet).await.unwrap();
    assert_eq!(row, ("completed".into(), 20, true, true, None));
    wallet.rollback().await.unwrap();
    up.cancelled().await;
    wait_for_diagnostic(&f, rid, "hosted_settlement_deferred").await;

    f.restart().await;
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 1);
    assert_eq!(inference::reconcile_due(&f.worker).await.unwrap(), 0);
    verify_receipt(&f.admin, rid, f.a.item, 2840, 2300, "1910").await;
    payment::assert_wallet(&f.admin, f.space, CREDIT - 2840, 3).await;
    duplicate(
        &f,
        "stream-wallet-lock",
        rid,
        "relay_request_result_unavailable",
    )
    .await;
    assert_eq!(f.sends(), (2, 0));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn sse_diagnostic_write_failure_does_not_change_terminal_or_journal() {
    let f = Fixture::new().await;
    qualify(&f, &qualifier(&f).await, &f.a).await;
    // Break only the optional write after the worker has started.
    sqlx::query(
        "REVOKE EXECUTE ON FUNCTION relay_hosted.record_request_diagnostic(uuid,uuid,uuid,text,smallint,bigint)
         FROM route_worker",
    )
    .execute(&f.admin)
    .await
    .unwrap();
    let denied = sqlx::query(
        "SELECT relay_hosted.record_request_diagnostic(NULL,NULL,NULL,'upstream_terminal',NULL,NULL)",
    )
    .execute(&f.worker)
    .await
    .unwrap_err();
    assert_eq!(
        denied.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );

    let up = StreamWriter::arm(&f.a, SSE);
    let mut reply = submit(&f, "diagnostic-unavailable").send().await.unwrap();
    assert_eq!(reply.status(), 200);
    let rid = request_id(&reply);
    let start = response_event("in_progress", 0, Value::Null);
    up.write(start.clone());
    up.write(delta(1));
    read_prefix(&mut reply, &[start.as_ref(), &delta(1)].concat()).await;
    up.write(response_event("failed", 2, usage(1000, 100, 5)));
    assert_error(&reply.bytes().await.unwrap(), "relay_upstream_failed", 3);
    up.cancelled().await;
    let recorded: (String, bool, i64) = sqlx::query_as(
        "SELECT t.outcome,t.applied_at IS NOT NULL,
           (SELECT count(*) FROM relay_hosted.request_diagnostics d WHERE d.request_id=t.request_id)
         FROM relay_hosted.text_terminal_evidence t WHERE t.request_id=$1",
    )
    .bind(rid)
    .fetch_one(&f.admin)
    .await
    .unwrap();
    assert_eq!(recorded, ("failed".into(), true, 0));
    verify_receipt(&f.admin, rid, f.a.item, 2765, 2300, "1865").await;
    payment::assert_wallet(&f.admin, f.space, CREDIT - 2765, 3).await;
    duplicate(
        &f,
        "diagnostic-unavailable",
        rid,
        "relay_request_result_unavailable",
    )
    .await;
    assert_eq!(f.sends(), (2, 0));
}

async fn wait_for_diagnostic(f: &Fixture, rid: Uuid, code: &str) {
    timeout(Duration::from_secs(3), async {
        loop {
            let matches: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM relay_hosted.request_diagnostics d
                 JOIN relay_hosted.text_requests r ON (r.id,r.attempt_id)=(d.request_id,d.attempt_id)
                 WHERE r.id=$1 AND d.code=$2",
            )
            .bind(rid)
            .bind(code)
            .fetch_one(&f.admin)
            .await
            .unwrap();
            if matches > 0 {
                assert_eq!(matches, 1);
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("did not retain {code}"));
}

async fn source_busy(f: &Fixture, source: Uuid) -> bool {
    sqlx::query_scalar("SELECT relay_hosted.text_source_in_flight($1)")
        .bind(source)
        .fetch_one(&f.admin)
        .await
        .unwrap()
}

fn assert_error(bytes: &Bytes, code: &str, sequence: u64) {
    let data = std::str::from_utf8(bytes)
        .unwrap()
        .strip_prefix("event: error\ndata: ")
        .unwrap()
        .strip_suffix("\n\n")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(data).unwrap(),
        json!({"type":"error","code":code,"message":code,"param":null,"sequence_number":sequence})
    );
}
