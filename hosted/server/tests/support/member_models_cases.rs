use super::*;

use crate::purchase_limit_race::OTHER_MODEL as OTHER;

pub(super) struct Edit {
    pub op: Uuid,
    pub source: Uuid,
    pub scope: Uuid,
    pub source_revision: i64,
    pub revision: i64,
    pub flags: Value,
}

impl Edit {
    pub(super) fn new(f: &Fixture, source: Uuid, revision: i64, flags: Value) -> Self {
        Self {
            op: Uuid::new_v4(),
            source,
            scope: f.scope,
            source_revision: 0,
            revision,
            flags,
        }
    }

    pub(super) async fn apply<'e>(&self, db: impl PgExecutor<'e>, proof: Option<i64>) -> String {
        self.call(db, OWNER, proof).await.unwrap()
    }

    async fn call<'e>(
        &self,
        db: impl PgExecutor<'e>,
        tag: &[u8],
        proof: Option<i64>,
    ) -> sqlx::Result<String> {
        sqlx::query_scalar(
            "SELECT relay_hosted.change_member_models($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10,$11,$11,
                floor(extract(epoch FROM clock_timestamp())*1000)::bigint+30000)",
        )
        .bind(ORIGIN)
        .bind(tag)
        .bind(self.source)
        .bind(self.op)
        .bind(self.scope)
        .bind(self.source_revision)
        .bind(self.revision)
        .bind(&self.flags)
        .bind(proof.map(|_| &[0x42_u8][..]))
        .bind(proof)
        .bind(proof.map(|_| false))
        .fetch_one(db)
        .await
    }
}

pub(super) async fn view(pool: &PgPool, tag: &[u8], source: Uuid) -> Option<Value> {
    sqlx::query_scalar("SELECT relay_hosted.owner_member_models($1,$2,$3)")
        .bind(ORIGIN)
        .bind(tag)
        .bind(source)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn member_models_deny_a_waiting_permit_without_interrupting_dispatched_work() {
    let f = Fixture::new().await;
    owner_session(&f.admin).await;
    sqlx::query("ALTER ROLE route_worker SET default_transaction_isolation TO 'repeatable read'")
        .execute(&f.admin)
        .await
        .unwrap();
    let worker = db::connect(&fixture_config(), &f._cluster.dsn("route_worker"))
        .await
        .unwrap();
    let isolation: String = sqlx::query_scalar("SHOW transaction_isolation")
        .fetch_one(&worker)
        .await
        .unwrap();
    assert_eq!(isolation, "read committed");
    let other_item = Uuid::new_v4();
    stock(
        &f.admin,
        f.scope,
        &f.b,
        other_item,
        OTHER,
        1_000,
        ["1", "1", "1"],
    )
    .await;
    publish(&f.admin, other_item, 1, None).await;
    let active = Held::reserve(&f, f.key, &f.bearer).await;
    assert!(active.claim(&f, f.b.id, f.b.item, 1).await);
    let waiting = Held::reserve(&f, f.key, &f.bearer).await;
    assert_eq!(waiting.quotes(&f.worker).await.len(), 2);
    let price_inputs = || {
        sqlx::query_scalar::<_, Value>(
            "SELECT relay_hosted.text_price_inputs(i,false) FROM relay_hosted.text_inventory i WHERE i.id=$1",
        )
        .bind(f.a.item)
        .fetch_one(&f.admin)
    };
    let price_before = price_inputs().await.unwrap();
    let mut tx = f.admin.begin().await.unwrap();
    let disable_b = Edit::new(&f, f.b.id, 0, json!({(MODEL):false}));
    assert_eq!(disable_b.apply(&mut *tx, Some(1)).await, "applied");
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let (permit, ()) = tokio::join!(
        waiting.claim_on(&worker, f.space, f.b.id, f.b.item, 1),
        async {
            wait_for_block(&f.admin, pid, "relay_hosted.claim_text_candidate(").await;
            tx.commit().await.unwrap();
        }
    );
    assert_eq!(permit, "denied");
    assert_eq!(price_inputs().await.unwrap(), price_before);
    assert_eq!(waiting.quotes(&f.worker).await, [(f.a.item, 1, 2300)]);
    assert_eq!(active.finish(&f.worker, "completed").await, "settled");
    verify_receipt(&f.admin, active.id, f.b.item, 3500, 1600, "1600").await;
    assert!(waiting.claim(&f, f.a.id, f.a.item, 1).await);
    assert_eq!(waiting.finish(&f.worker, "completed").await, "settled");
    verify_receipt(&f.admin, waiting.id, f.a.item, 3500, 2300, "2300").await;

    let old_hold = Held::reserve(&f, f.key, &f.bearer).await;
    let disable_a = Edit::new(&f, f.a.id, 0, json!({(MODEL):false}));
    assert_eq!(disable_a.apply(&f.admin, Some(2)).await, "applied");
    let listed = || {
        sqlx::query_as::<_, (Uuid, String, i64)>(
            "SELECT id,model,revision FROM relay_hosted.active_text_listings ORDER BY model",
        )
        .fetch_all(&f.admin)
    };
    let only_other = listed().await.unwrap();
    assert_eq!(only_other.len(), 1);
    assert_eq!(only_other[0].1, OTHER);
    assert!(
        !old_hold.claim(&f, f.a.id, f.a.item, 1).await,
        "the last blocked supplier cannot use an existing hold"
    );
    let allow_b = Edit::new(&f, f.b.id, 1, json!({(MODEL):true}));
    assert_eq!(allow_b.apply(&f.admin, Some(3)).await, "applied");
    assert!(
        !old_hold.claim(&f, f.b.id, f.b.item, 1).await,
        "a route omitted from an earlier reservation cannot be added later"
    );
    assert_eq!(old_hold.finish(&f.worker, "not_sent").await, "released");
    assert!(
        listed()
            .await
            .unwrap()
            .contains(&(f.offer, MODEL.into(), 1))
    );
    assert_eq!(price_inputs().await.unwrap(), price_before);
    f.verify_purchase(
        f.submit_model(&f.bearer, OTHER, "other-model-can-serve", 1000, 100)
            .send()
            .await
            .unwrap(),
        other_item,
        3500,
        1100,
        "1100",
    )
    .await;
    assert_eq!(f.sends(), (0, 1));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn member_models_batch_is_owner_only_exact_revisioned_and_receipt_safe() {
    let f = Fixture::new().await;
    owner_session(&f.admin).await;
    customer_reader::setup_reader(&f).await;
    let customer = customer_reader::Reader::new(&f, f.space, 32).await;
    let pool = &customer.pool;
    for model in [MODEL, OTHER, "Fixture-text"] {
        stock(
            &f.admin,
            f.scope,
            &f.b,
            Uuid::new_v4(),
            model,
            1_000,
            ["1", "1", "1"],
        )
        .await;
    }
    let initial = view(pool, OWNER, f.b.id).await.unwrap();
    assert_eq!(
        initial,
        json!({"scope_id":f.scope,"source_revision":"0","revision":"0","items":[
            {"model":"Fixture-text","allowed":true},{"model":OTHER,"allowed":true},{"model":MODEL,"allowed":true}
        ]})
    );
    assert!(view(pool, &customer.tag, f.b.id).await.is_none());
    assert_eq!(
        view(pool, OWNER, Uuid::new_v4()).await,
        Some(json!({"error":"missing"}))
    );
    let mut edit = Edit::new(&f, f.b.id, 0, json!({(MODEL):false,(OTHER):false}));
    assert_eq!(
        edit.call(pool, &customer.tag, None).await.unwrap(),
        "denied"
    );
    assert!(edit.call(&f.worker, OWNER, None).await.is_err());
    assert!(
        sqlx::query("SELECT * FROM relay_hosted.source_model_blocks")
            .fetch_all(pool)
            .await
            .is_err()
    );

    edit.flags["unknown-model"] = json!(false);
    assert_eq!(edit.apply(pool, Some(1)).await, "conflict");
    edit.flags.as_object_mut().unwrap().remove("unknown-model");
    let scope = edit.scope;
    edit.scope = Uuid::new_v4();
    assert_eq!(edit.apply(pool, Some(1)).await, "conflict");
    edit.scope = scope;
    edit.source_revision = 1;
    assert_eq!(edit.apply(pool, Some(1)).await, "conflict");
    edit.source_revision = 0;
    assert_eq!(edit.apply(pool, None).await, "fresh_required");
    assert_eq!(view(pool, OWNER, f.b.id).await, Some(initial.clone()));
    assert_eq!(edit.apply(pool, Some(1)).await, "applied");
    assert_eq!(edit.apply(pool, None).await, "applied");
    edit.flags[MODEL] = json!(true);
    assert_eq!(edit.apply(pool, None).await, "conflict");
    edit.op = Uuid::new_v4();
    edit.flags = json!({(MODEL):true});
    assert_eq!(edit.apply(pool, Some(2)).await, "conflict");
    let saved = view(pool, OWNER, f.b.id).await.unwrap();
    assert_eq!(saved["revision"], "1");
    assert_eq!(saved["source_revision"], "0");
    assert_eq!(
        saved["items"],
        json!([
            {"model":"Fixture-text","allowed":true},{"model":OTHER,"allowed":false},{"model":MODEL,"allowed":false}
        ])
    );
    assert!(f.listed().await);
}
