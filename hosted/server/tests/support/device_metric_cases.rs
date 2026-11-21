use super::*;
use crate::text_stream_fixture::{
    StreamWriter, delta, measured_terminal, qualifier, qualify, read_prefix, response_event, submit,
};

impl Device {
    async fn metrics(&self, pool: &PgPool, hours: i32) -> Option<Value> {
        sqlx::query_scalar("SELECT relay_hosted.device_overview_metrics($1,$2,$3,$4,$5)")
            .bind(ORIGIN)
            .bind(self.installation)
            .bind(self.id)
            .bind(PROOF)
            .bind(hours)
            .fetch_one(pool)
            .await
            .unwrap()
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn desktop_pool_and_generation_use_actual_route_state_and_paced_terminal_receipts() {
    let f = Fixture::new().await;
    let pool = owner_usage_cases::owner(&f).await;
    let overview = Device::save(&f, &["overview.read"]).await;
    let usage = Device::save(&f, &["usage.read"]).await;
    assert!(usage.metrics(&pool, 24).await.is_none());
    assert_eq!(
        overview.metrics(&pool, 12).await.unwrap()["error"],
        "invalid_filter"
    );
    assert!(
        overview.overview(&pool, ORIGIN).await.unwrap()["result"]
            .get("metrics")
            .is_none()
    );
    assert!(
        sqlx::query("SELECT relay_hosted.desktop_pool_capacity($1)")
            .bind(overview.installation)
            .execute(&pool)
            .await
            .is_err()
    );
    let stats =
        async |hours| overview.metrics(&pool, hours).await.unwrap()["result"]["metrics"].clone();
    assert_eq!(
        stats(24).await,
        json!({"pool":{"members":"2","ready":"2","busy":"0"},"generation":null})
    );
    assert_eq!(f.sends(), (0, 0));

    qualify(&f, &qualifier(&f).await, &f.a).await;
    sqlx::query("INSERT INTO relay_hosted.text_route_waits VALUES ($1,$2,now()+interval '1 hour')")
        .bind(f.b.id)
        .bind(MODEL)
        .execute(&f.admin)
        .await
        .unwrap();
    assert_eq!(
        stats(24).await["pool"],
        json!({"members":"2","ready":"1","busy":"0"})
    );

    let writer = StreamWriter::arm(&f.a, "text/event-stream");
    let created = response_event("in_progress", 0, Value::Null);
    writer.write(created.clone());
    let mut response = submit(&f, "desktop-generation").send().await.unwrap();
    assert_eq!(response.status(), 200);
    let id = request_id(&response);
    read_prefix(&mut response, &created).await;
    assert_eq!(
        stats(24).await,
        json!({"pool":{"members":"2","ready":"0","busy":"1"},"generation":null})
    );
    for n in [1, 2] {
        let bytes = delta(n);
        writer.write(bytes.clone());
        read_prefix(&mut response, &bytes).await;
        if n == 1 {
            tokio::time::sleep(Duration::from_millis(160)).await;
        }
    }
    let end = measured_terminal();
    writer.write(end.clone());
    assert_eq!(response.bytes().await.unwrap(), end);
    writer.cancelled().await;
    let (ms, tokens): (i32, i64) = sqlx::query_as(
        "SELECT generation_ms,generation_tokens FROM relay_hosted.text_completion_timings WHERE request_id=$1",
    ).bind(id).fetch_one(&f.admin).await.unwrap();
    assert!(ms >= 150);
    assert_eq!(tokens, 14);
    let sample = stats(24).await;
    assert_eq!(
        sample["pool"],
        json!({"members":"2","ready":"1","busy":"0"})
    );
    assert_eq!(sample["generation"]["samples"], "1");
    assert!(
        (sample["generation"]["tokens_per_second"].as_f64().unwrap() - 14_000.0 / f64::from(ms))
            .abs()
            < 0.0051
    );
    let receipt = text_late_cases::at_status(&f, id).await;
    assert_eq!(receipt["execution"], "completed");
    assert_eq!(receipt["total_tokens"], "1020");
    assert_eq!(f.sends(), (2, 0));

    for (age, has_month) in [(200, true), (721, false)] {
        sqlx::query(
            "UPDATE relay_hosted.text_completion_timings
            SET observed_at=statement_timestamp()-$2*interval '1 hour' WHERE request_id=$1",
        )
        .bind(id)
        .bind(age)
        .execute(&f.admin)
        .await
        .unwrap();
        sqlx::query("SELECT relay_hosted.prune_model_status_completions()")
            .execute(&pool)
            .await
            .unwrap();
        assert!(stats(168).await["generation"].is_null());
        assert_eq!(!stats(720).await["generation"].is_null(), has_month);
    }
    assert_eq!(text_late_cases::at_status(&f, id).await, receipt);
    sqlx::query(
        "UPDATE relay_hosted.service_control SET mode='maintenance',qualification_scope_ref=NULL",
    )
    .execute(&f.admin)
    .await
    .unwrap();
    assert_eq!(stats(24).await, json!({"pool":null,"generation":null}));
}
