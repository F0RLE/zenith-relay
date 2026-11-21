use super::*;
use owner_usage_cases::TAG;

async fn inventory(pool: &PgPool, d: &Device) -> Option<Value> {
    sqlx::query_scalar("SELECT relay_hosted.device_pool_connections($1,$2,$3,$4,NULL)")
        .bind(ORIGIN)
        .bind(d.installation)
        .bind(d.id)
        .bind(PROOF)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn membership(page: &Value, source: Uuid) -> &Value {
    &page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == source.to_string())
        .unwrap()["pool"]
}

async fn prepare(
    pool: &PgPool,
    d: &Device,
    source: Uuid,
    action: &str,
    revision: &str,
) -> Option<Value> {
    sqlx::query_scalar("SELECT relay_hosted.prepare_desktop_pool_review($1,$2,$3,$4,$5,$6,$7)")
        .bind(ORIGIN)
        .bind(d.installation)
        .bind(d.id)
        .bind(PROOF)
        .bind(source)
        .bind(action)
        .bind(revision)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn preview(pool: &PgPool, op: Uuid, tag: &[u8]) -> Option<Value> {
    sqlx::query_scalar("SELECT relay_hosted.owner_desktop_pool_review($1,$2,$3)")
        .bind(ORIGIN)
        .bind(tag)
        .bind(op)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn apply(pool: &PgPool, op: Uuid, proof: Option<i64>) -> String {
    sqlx::query_scalar(
        "SELECT relay_hosted.apply_desktop_pool_review($1,$2,$3,$4,$5,$5,false,false,
        floor(extract(epoch FROM clock_timestamp())*1000)::bigint+30000)",
    )
    .bind(ORIGIN)
    .bind(TAG)
    .bind(op)
    .bind(proof.map(|_| &[1u8][..]))
    .bind(proof)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn grant(f: &Fixture) -> Device {
    let d = Device::save(f, &[]).await;
    sqlx::query("UPDATE relay_hosted.management_devices SET manage_pool=true WHERE id=$1")
        .bind(d.id)
        .execute(&f.admin)
        .await
        .unwrap();
    d
}

async fn proposal(pool: &PgPool, d: &Device, source: Uuid, action: &str) -> Uuid {
    let page = inventory(pool, d).await.unwrap();
    let state = membership(&page, source);
    assert_eq!(state["action"], action);
    let revision = state["revision"].as_str().unwrap();
    let reply = prepare(pool, d, source, action, revision).await.unwrap();
    let again = prepare(pool, d, source, action, revision).await.unwrap();
    assert_eq!(reply, again, "a lost reply must not issue a new review");
    let ready = inventory(pool, d).await.unwrap();
    assert_eq!(ready["pool_review"], reply["result"]);
    serde_json::from_value(reply["result"]["operation_id"].clone()).unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn desktop_pool_admits_only_reviewed_bindings_and_parks_without_deleting() {
    let f = Fixture::new().await;
    let pool = owner_usage_cases::owner(&f).await;
    let reader = Device::save(&f, &["usage.read"]).await;
    let writer = grant(&f).await;
    let source = f.b.id;
    let fresh = Uuid::new_v4();
    let extra = Uuid::new_v4();
    for (id, model) in [(fresh, MODEL), (extra, "fixture-text-b")] {
        stock(&f.admin, f.scope, &f.b, id, model, 1000, ["1", ".25", "6"]).await;
    }
    sqlx::query("UPDATE relay_hosted.text_model_rules SET admitted=false,purchase_revision=NULL WHERE id=ANY($1)")
        .bind([f.b.item, fresh, extra].as_slice()).execute(&f.admin).await.unwrap();
    // A more recent check can expire first. The earlier one must not become
    // admission authority again simply because its original TTL was longer.
    sqlx::query(
        "WITH observations AS (
            INSERT INTO relay_hosted.text_inventory(id,scope_id,source_id,provider_revision,proxy_revision,uses_proxy,base_url,
                model,source_label,max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,close_after_s,checked_at,valid_until)
            SELECT gen_random_uuid(),i.scope_id,i.source_id,i.provider_revision,i.proxy_revision,i.uses_proxy,i.base_url,
                'expired-latest',i.source_label,i.max_input_bytes,i.max_output_tokens,i.tokens_per_byte,i.overhead_tokens,i.close_after_s,
                now()+d.checked,now()+d.until_at FROM relay_hosted.text_inventory i
            CROSS JOIN (VALUES (interval '-3 minutes',interval '30 minutes'),(interval '-2 minutes',interval '-1 minute'))
                d(checked,until_at) WHERE i.id=$1 RETURNING id,scope_id,model
         ) INSERT INTO relay_hosted.text_model_rules(id,scope_id,model) SELECT * FROM observations",
    )
    .bind(f.b.item)
    .execute(&f.admin)
    .await
    .unwrap();
    assert_eq!(
        inventory(&pool, &reader).await.unwrap()["pool_access"],
        false
    );
    let page = inventory(&pool, &writer).await.unwrap();
    assert_eq!(page["pool_access"], true);
    let state = membership(&page, source);
    assert_eq!(state["in_pool"], false);
    assert_eq!(state["action"], "include");
    let revision = state["revision"].as_str().unwrap();
    assert!(
        prepare(&pool, &reader, source, "include", revision)
            .await
            .is_none()
    );
    assert_eq!(
        prepare(&pool, &writer, source, "exclude", revision)
            .await
            .unwrap(),
        json!({"error":"conflict"})
    );
    assert!(
        sqlx::query("SELECT relay_hosted.desktop_pool_plan($1,$2)")
            .bind(writer.installation)
            .bind(source)
            .execute(&pool)
            .await
            .is_err()
    );
    let op = proposal(&pool, &writer, source, "include").await;
    assert!(
        preview(&pool, op, PROOF).await.is_none(),
        "device proof is not owner authority"
    );
    assert_eq!(
        preview(&pool, op, TAG).await.unwrap()["models"],
        json!([MODEL, "fixture-text-b"])
    );
    assert_eq!(apply(&pool, op, None).await, "fresh_required");
    assert_eq!(apply(&pool, op, Some(0)).await, "passkey_changed");
    assert_eq!(
        membership(&inventory(&pool, &writer).await.unwrap(), source)["in_pool"],
        false
    );
    assert_eq!(apply(&pool, op, Some(1)).await, "applied");
    assert_eq!(apply(&pool, op, None).await, "applied");
    assert_eq!(preview(&pool, op, TAG).await.unwrap()["applied"], true);
    assert!(inventory(&pool, &writer).await.unwrap()["pool_review"].is_null());
    let admitted: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM relay_hosted.text_model_rules WHERE id=ANY($1) AND admitted
         AND purchase_revision IS NULL AND NOT enabled AND revision=1 ORDER BY model",
    )
    .bind([f.b.item, fresh, extra].as_slice())
    .fetch_all(&f.admin)
    .await
    .unwrap();
    assert_eq!(
        admitted,
        [fresh, extra],
        "only the latest checked binding for each exact model"
    );
    let exclude = proposal(&pool, &writer, source, "exclude").await;
    assert_eq!(apply(&pool, exclude, Some(2)).await, "applied");
    assert_eq!(apply(&pool, exclude, None).await, "applied");
    let page = inventory(&pool, &writer).await.unwrap();
    assert_eq!(
        membership(&page, source),
        &json!({
            "state":"paused","in_pool":false,"draining":false,"action":null,"revision":null
        })
    );
    assert!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == source.to_string() && r["credential_state"] == "stored")
    );
    assert_eq!(membership(&page, f.a.id)["in_pool"], true);
    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM relay_hosted.desktop_pool_reviews WHERE confirmed_by_passkey_id IS NOT NULL")
        .fetch_one(&f.admin).await.unwrap();
    assert_eq!(audits, 2);
    assert_eq!(f.sends(), (0, 0));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn desktop_pool_review_expires_and_cannot_survive_device_or_source_changes() {
    let f = Fixture::new().await;
    let pool = owner_usage_cases::owner(&f).await;
    let writer = grant(&f).await;
    let original = proposal(&pool, &writer, f.b.id, "exclude").await;
    let second = proposal(&pool, &writer, f.a.id, "exclude").await;
    assert_eq!(apply(&pool, original, Some(1)).await, "missing");
    sqlx::query("UPDATE relay_hosted.desktop_pool_reviews SET expires_at=clock_timestamp() WHERE operation_id=$1")
        .bind(second).execute(&f.admin).await.unwrap();
    assert_eq!(apply(&pool, second, Some(1)).await, "conflict");

    let changed = proposal(&pool, &writer, f.b.id, "exclude").await;
    sqlx::query("UPDATE relay_hosted.text_model_rules SET revision=revision+1 WHERE id=$1")
        .bind(f.b.item)
        .execute(&f.admin)
        .await
        .unwrap();
    assert_eq!(
        preview(&pool, changed, TAG).await.unwrap(),
        json!({"error":"conflict"})
    );
    assert_eq!(apply(&pool, changed, Some(1)).await, "conflict");
    let revoked = proposal(&pool, &writer, f.b.id, "exclude").await;
    sqlx::query("UPDATE relay_hosted.management_devices SET revoked_at=clock_timestamp(),revision=3 WHERE id=$1")
        .bind(writer.id).execute(&f.admin).await.unwrap();
    assert_eq!(apply(&pool, revoked, Some(1)).await, "conflict");
    assert!(inventory(&pool, &writer).await.is_none());
    let reader = Device::save(&f, &[]).await;
    assert_eq!(
        membership(&inventory(&pool, &reader).await.unwrap(), f.b.id)["in_pool"],
        true
    );
}
