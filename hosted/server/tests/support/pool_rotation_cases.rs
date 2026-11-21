use super::*;

struct Rotation {
    op: Uuid,
    scope: Uuid,
    revision: String,
    mode: &'static str,
    order: Vec<Uuid>,
}

impl Rotation {
    fn new(view: &Value, mode: &'static str, order: Vec<Uuid>) -> Self {
        Self {
            op: Uuid::new_v4(),
            scope: serde_json::from_value(view["scope_id"].clone()).unwrap(),
            revision: view["revision"].as_str().unwrap().into(),
            mode,
            order,
        }
    }

    async fn save<'e>(
        &self,
        db: impl PgExecutor<'e>,
        tag: &[u8],
        proof: Option<i64>,
    ) -> sqlx::Result<String> {
        sqlx::query_scalar(
            "SELECT relay_hosted.change_pool_rotation($1,$2,$3,$4,$5,$6,$7,$8,$9,$9,$10,$10,
                floor(extract(epoch FROM clock_timestamp())*1000)::bigint+30000)",
        )
        .bind(ORIGIN)
        .bind(tag)
        .bind(self.op)
        .bind(self.scope)
        .bind(&self.revision)
        .bind(self.mode)
        .bind(&self.order)
        .bind(proof.map(|_| &[0x42_u8][..]))
        .bind(proof)
        .bind(proof.map(|_| false))
        .fetch_one(db)
        .await
    }
}

async fn read(db: &PgPool, tag: &[u8]) -> Option<Value> {
    sqlx::query_scalar("SELECT relay_hosted.owner_pool_rotation($1,$2)")
        .bind(ORIGIN)
        .bind(tag)
        .fetch_one(db)
        .await
        .unwrap()
}

async fn ranks(db: &PgPool, hold: &Held, mode: &str) -> Vec<(Uuid, i32)> {
    let view: Value =
        sqlx::query_scalar("SELECT relay_hosted.request_text_candidates($1,$2,$3,$4,$5)")
            .bind(ORIGIN)
            .bind(hold.id)
            .bind(hold.attempt)
            .bind(hold.key)
            .bind(SEAL)
            .fetch_one(db)
            .await
            .unwrap();
    assert_eq!(view["mode"], mode);
    let mut list: Vec<(Uuid, i32)> = view["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                serde_json::from_value(r["source"].clone()).unwrap(),
                serde_json::from_value(r["priority"].clone()).unwrap(),
            )
        })
        .collect();
    list.sort_unstable_by_key(|(_, rank)| -rank);
    list
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn confirmed_rotation_pins_each_hold_and_ranks_only_permitted_supply() {
    let mut f = Fixture::new().await;
    owner_session(&f.admin).await;
    let before = Held::reserve(&f, f.key, &f.bearer).await;
    let initial = read(&f.admin, OWNER).await.unwrap();
    let failover = Rotation::new(&initial, "failover", vec![f.a.id, f.b.id]);
    let mut tx = f.admin.begin().await.unwrap();
    assert_eq!(
        failover.save(&mut *tx, OWNER, Some(1)).await.unwrap(),
        "applied"
    );
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let (racing, ()) = tokio::join!(Held::reserve(&f, f.key, &f.bearer), async {
        wait_for_block(&f.admin, pid, "relay_hosted.reserve_published_text(").await;
        tx.commit().await.unwrap();
    });
    assert_eq!(ranks(&f.worker, &before, "automatic").await.len(), 2);
    assert_eq!(
        ranks(&f.worker, &racing, "failover").await,
        [(f.a.id, -1), (f.b.id, -2)]
    );
    assert_eq!(before.finish(&f.worker, "not_sent").await, "released");
    assert_eq!(racing.finish(&f.worker, "not_sent").await, "released");
    assert!(f.listed().await);
    assert_eq!(f.sends(), (0, 0));

    f.restart().await;
    f.verify_purchase(
        f.submit("ranked-even-though-more-expensive", 1000, 100)
            .send()
            .await
            .unwrap(),
        f.a.item,
        3500,
        2300,
        "2300",
    )
    .await;
    let active = Held::reserve(&f, f.key, &f.bearer).await;
    assert!(active.claim(&f, f.a.id, f.a.item, 1).await);
    f.verify_purchase(
        f.submit("primary-is-busy", 1000, 100).send().await.unwrap(),
        f.b.item,
        3500,
        1600,
        "1600",
    )
    .await;
    let wait = Held::reserve(&f, f.key, &f.bearer).await;
    let saved = read(&f.admin, OWNER).await.unwrap();
    let auto = Rotation::new(&saved, "automatic", vec![f.a.id, f.b.id]);
    assert_eq!(
        auto.save(&f.admin, OWNER, Some(2)).await.unwrap(),
        "applied"
    );
    assert_eq!(ranks(&f.worker, &wait, "failover").await[0], (f.a.id, -1));
    assert_eq!(wait.finish(&f.worker, "not_sent").await, "released");
    assert_eq!(active.finish(&f.worker, "completed").await, "settled");
    verify_receipt(&f.admin, active.id, f.a.item, 3500, 2300, "2300").await;
    f.verify_purchase(
        f.submit("back-to-cheapest", 1000, 100)
            .send()
            .await
            .unwrap(),
        f.b.item,
        3500,
        1600,
        "1600",
    )
    .await;
    assert_eq!(f.sends(), (1, 2));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn policy_requires_exact_member_revision_and_owner_confirmation_and_replays_once() {
    let f = Fixture::new().await;
    owner_session(&f.admin).await;
    customer_reader::setup_reader(&f).await;
    let reader = customer_reader::Reader::new(&f, f.space, 32).await;
    let pool = &reader.pool;
    let current = read(pool, OWNER).await.unwrap();
    assert_eq!(current["scope_id"], json!(f.scope));
    assert_eq!(current["mode"], "automatic");
    assert_eq!(current["members"].as_array().unwrap().len(), 2);
    assert!(read(pool, &reader.tag).await.is_none());
    let mut change = Rotation::new(&current, "failover", vec![f.b.id, f.a.id]);
    assert_eq!(
        change.save(pool, &reader.tag, None).await.unwrap(),
        "denied"
    );
    assert!(change.save(&f.worker, OWNER, None).await.is_err());
    assert!(
        sqlx::query("SELECT * FROM relay_hosted.text_pool_rotation_changes")
            .execute(pool)
            .await
            .is_err()
    );
    change.order[1] = f.b.id;
    assert_eq!(change.save(pool, OWNER, None).await.unwrap(), "invalid");
    change.order[1] = Uuid::new_v4();
    assert_eq!(change.save(pool, OWNER, None).await.unwrap(), "invalid");
    change.order[1] = f.a.id;
    assert_eq!(
        change.save(pool, OWNER, None).await.unwrap(),
        "fresh_required"
    );
    assert_eq!(read(pool, OWNER).await, Some(current.clone()));
    assert_eq!(
        serving_change(pool, f.b.id, Uuid::new_v4(), 0, "park", Some(1)).await,
        "applied"
    );
    assert_eq!(change.save(pool, OWNER, Some(2)).await.unwrap(), "conflict");
    let smaller = read(pool, OWNER).await.unwrap();
    assert_eq!(smaller["members"][0]["source_id"], json!(f.a.id));
    assert_eq!(smaller["members"].as_array().unwrap().len(), 1);
    let mut saved = Rotation::new(&smaller, "failover", vec![f.a.id]);
    assert_eq!(saved.save(pool, OWNER, Some(2)).await.unwrap(), "applied");
    assert_eq!(saved.save(pool, OWNER, None).await.unwrap(), "applied");
    saved.mode = "automatic";
    assert_eq!(saved.save(pool, OWNER, None).await.unwrap(), "conflict");
    let recorded: i64 =
        sqlx::query_scalar("SELECT count(*) FROM relay_hosted.text_pool_rotation_changes")
            .fetch_one(&f.admin)
            .await
            .unwrap();
    assert_eq!(recorded, 1);
    assert_eq!(f.sends(), (0, 0));
}
