use super::{
    refund_race::{OWNER, owner_session},
    *,
};

#[path = "source_lifecycle_cases.rs"]
mod lifecycle;
#[path = "member_models_cases.rs"]
mod member_models;
#[path = "pool_rotation_cases.rs"]
mod rotation;

pub(super) async fn wait_for_block(db: &PgPool, blocker: i32, query: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT FROM pg_stat_activity WHERE wait_event_type='Lock'
                    AND position($1::text IN query)>0 AND $2=ANY(pg_blocking_pids(pid)))",
            )
            .bind(query)
            .bind(blocker)
            .fetch_one(db)
            .await
            .unwrap();
            if waiting {
                return;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("expected {query} to wait for the transaction"));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn selling_edit_on_a_supplier_waits_for_the_live_rule_before_locking_that_supplier() {
    let f = Fixture::new().await;
    owner_session(&f.admin).await;
    let hold = Held::reserve(&f, f.key, &f.bearer).await;
    let mut dispatch = f.admin.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar(
        "SELECT pg_backend_pid() FROM relay_hosted.text_model_rules WHERE id=$1 FOR SHARE",
    )
    .bind(f.a.item)
    .fetch_one(&mut *dispatch)
    .await
    .unwrap();
    let save = sqlx::query_scalar::<_, String>(
        "SELECT relay_hosted.change_text_rule($1,$2,$3,gen_random_uuid(),0,'save',$4,
            NULL,NULL,NULL,NULL,NULL,NULL)",
    )
    .bind(ORIGIN)
    .bind(OWNER)
    .bind(f.b.item)
    .bind(json!({"input":"2.000000000000","cached":"0.400000000000","output":"5.000000000000"}))
    .fetch_one(&f.admin);
    let (edit, ()) = tokio::join!(save, async {
        wait_for_block(&f.admin, pid, "relay_hosted.change_text_rule(").await;
        assert_eq!(
            hold.claim_on(&mut *dispatch, f.space, f.b.id, f.b.item, 1)
                .await,
            "claimed"
        );
        dispatch.commit().await.unwrap();
    });
    assert_eq!(edit.unwrap(), "applied");
    assert_eq!(hold.finish(&f.worker, "completed").await, "settled");
    verify_receipt(&f.admin, hold.id, f.b.item, 3500, 1600, "1600").await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn confirmed_cap_and_physical_admission_serialize_without_repricing_an_existing_permit() {
    let f = Fixture::new().await;
    owner_session(&f.admin).await;
    let hold = Held::reserve(&f, f.key, &f.bearer).await;
    let mut dispatch = f.admin.begin().await.unwrap();
    lock_source(&mut *dispatch, f.b.id).await;
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *dispatch)
        .await
        .unwrap();
    // The browser journey proves WebAuthn; here the verifier metadata is
    // synthetic to exercise the transaction ordering and expense projection.
    let save = sqlx::query_scalar::<_, String>(
        "SELECT relay_hosted.change_purchase_limit($1,$2,$3,gen_random_uuid(),0,0,
            decode('42','hex'),1,1,false,false,floor(extract(epoch FROM clock_timestamp())*1000)::bigint+30000)",
    )
    .bind(ORIGIN)
    .bind(OWNER)
    .bind(f.b.id)
    .fetch_one(&f.admin);
    let (cap, ()) = tokio::join!(save, async {
        wait_for_block(&f.admin, pid, "relay_hosted.change_purchase_limit(").await;
        assert_eq!(
            hold.claim_on(&mut *dispatch, f.space, f.b.id, f.b.item, 1)
                .await,
            "claimed"
        );
        dispatch.commit().await.unwrap();
    });
    assert_eq!(cap.unwrap(), "applied");
    assert_eq!(hold.finish(&f.worker, "completed").await, "settled");
    verify_receipt(&f.admin, hold.id, f.b.item, 3500, 1600, "1600").await;
    let blocked = Held::reserve(&f, f.key, &f.bearer).await;
    assert!(!blocked.claim(&f, f.b.id, f.b.item, 1).await);
    assert_eq!(blocked.finish(&f.worker, "not_sent").await, "released");
    assert_eq!(f.sends(), (0, 0));
}

async fn serving_change<'e>(
    db: impl PgExecutor<'e>,
    source: Uuid,
    operation: Uuid,
    revision: i64,
    action: &str,
    proof_revision: Option<i64>,
) -> String {
    sqlx::query_scalar(
        "SELECT relay_hosted.change_source_serving($1,$2,$3,$4,$5,$6,$7,$8,$8,$9,$9,
            floor(extract(epoch FROM clock_timestamp())*1000)::bigint+30000)",
    )
    .bind(ORIGIN)
    .bind(OWNER)
    .bind(source)
    .bind(operation)
    .bind(revision)
    .bind(action)
    .bind(proof_revision.map(|_| &[0x42_u8][..]))
    .bind(proof_revision)
    .bind(proof_revision.map(|_| false))
    .fetch_one(db)
    .await
    .unwrap()
}

async fn connection(f: &Fixture, source: Uuid) -> Value {
    sqlx::query_scalar(
        "SELECT item FROM jsonb_array_elements(relay_hosted.owner_connections($1,$2,NULL)->'items') item
         WHERE (item->>'id')::uuid=$3",
    )
    .bind(ORIGIN)
    .bind(OWNER)
    .bind(source)
    .fetch_one(&f.admin)
    .await
    .unwrap()
}

async fn check_step(f: &Fixture, pool: &PgPool, op: Uuid, step: &str) -> String {
    sqlx::query_scalar("SELECT relay_hosted.text_check_step($1,$2,$3,$4,1,0,false,$5,$6)")
        .bind(ORIGIN)
        .bind(f.scope)
        .bind(op)
        .bind(f.b.id)
        .bind(
            json!({"id":op,"scope":f.scope,"source":f.b.id,"provider_revision":1,
            "proxy_revision":0,"uses_proxy":false}),
        )
        .bind(step)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn source_pause_before_physical_permit_uses_backup_without_repricing() {
    let f = Fixture::new().await;
    owner_session(&f.admin).await;
    let qualifier = db::connect(&fixture_config(), &f._cluster.dsn("route_qualifier"))
        .await
        .unwrap();
    let check = Uuid::new_v4();
    let price_inputs = || {
        sqlx::query_scalar::<_, Value>(
            "SELECT relay_hosted.text_price_inputs(i,false) FROM relay_hosted.text_inventory i WHERE i.id=$1",
        ).bind(f.a.item).fetch_one(&f.admin)
    };
    let inputs = price_inputs().await.unwrap();
    let mut saved = connection(&f, f.b.id).await;
    assert_eq!(saved["serving"], serving("0", "enabled", true, false));
    assert_eq!(
        check_step(&f, &qualifier, check, "prepare").await,
        "prepared"
    );
    let op = Uuid::new_v4();
    assert_eq!(
        serving_change(&f.admin, f.b.id, op, 0, "pause", None).await,
        "fresh_required"
    );
    let hold = Held::reserve(&f, f.key, &f.bearer).await;
    assert_eq!(hold.quotes(&f.worker).await.len(), 2);
    let mut pause = f.admin.begin().await.unwrap();
    lock_source(&mut *pause, f.b.id).await;
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *pause)
        .await
        .unwrap();
    let (permit, ()) = tokio::join!(
        hold.claim_on(&f.worker, f.space, f.b.id, f.b.item, 1),
        async {
            wait_for_block(&f.admin, pid, "relay_hosted.claim_text_candidate(").await;
            assert_eq!(
                serving_change(&mut *pause, f.b.id, op, 0, "pause", Some(1)).await,
                "applied"
            );
            pause.commit().await.unwrap();
        }
    );
    assert_eq!(permit, "denied");
    saved["serving"] = serving("1", "paused", true, false);
    assert_eq!(connection(&f, f.b.id).await, saved);
    assert_eq!(price_inputs().await.unwrap(), inputs);
    assert_eq!(check_step(&f, &qualifier, check, "claim").await, "denied");
    assert_eq!(
        check_step(&f, &qualifier, Uuid::new_v4(), "prepare").await,
        "denied"
    );
    assert!(f.listed().await);
    assert_eq!(hold.quotes(&f.worker).await, [(f.a.item, 1, 2300)]);
    assert!(hold.claim(&f, f.a.id, f.a.item, 1).await);
    assert_eq!(hold.finish(&f.worker, "completed").await, "settled");
    verify_receipt(&f.admin, hold.id, f.a.item, 3500, 2300, "2300").await;
    assert_eq!(f.sends(), (0, 0));

    let resume = Uuid::new_v4();
    assert_eq!(
        serving_change(&f.admin, f.b.id, resume, 1, "resume", Some(2)).await,
        "applied"
    );
    assert_eq!(
        serving_change(&f.admin, f.b.id, op, 0, "pause", None).await,
        "applied"
    );
    assert_eq!(
        serving_change(&f.admin, f.a.id, op, 0, "pause", None).await,
        "conflict"
    );
    assert_eq!(
        connection(&f, f.b.id).await["serving"],
        serving("2", "enabled", true, false)
    );
    f.verify_purchase(
        f.submit("resumed-supplier", 1000, 100)
            .send()
            .await
            .unwrap(),
        f.b.item,
        3500,
        1600,
        "1600",
    )
    .await;
    assert_eq!(f.sends(), (0, 1));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn physical_permit_before_source_pause_drains_without_cancelling_accepted_work() {
    let f = Fixture::new().await;
    owner_session(&f.admin).await;
    let hold = Held::reserve(&f, f.key, &f.bearer).await;
    let mut dispatch = f.admin.begin().await.unwrap();
    assert_eq!(
        hold.claim_on(&mut *dispatch, f.space, f.b.id, f.b.item, 1)
            .await,
        "claimed"
    );
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *dispatch)
        .await
        .unwrap();
    let pause = serving_change(&f.admin, f.b.id, Uuid::new_v4(), 0, "pause", Some(1));
    let (changed, ()) = tokio::join!(pause, async {
        wait_for_block(&f.admin, pid, "relay_hosted.change_source_serving(").await;
        dispatch.commit().await.unwrap();
    });
    assert_eq!(changed, "applied");
    assert_eq!(
        connection(&f, f.b.id).await["serving"],
        serving("1", "paused", true, true)
    );
    let resume = Uuid::new_v4();
    assert_eq!(
        serving_change(&f.admin, f.b.id, resume, 1, "resume", Some(2)).await,
        "draining"
    );
    let new = Held::reserve(&f, f.key, &f.bearer).await;
    assert_eq!(new.quotes(&f.worker).await, [(f.a.item, 1, 2300)]);
    assert_eq!(new.finish(&f.worker, "not_sent").await, "released");
    assert_eq!(hold.finish(&f.worker, "completed").await, "settled");
    verify_receipt(&f.admin, hold.id, f.b.item, 3500, 1600, "1600").await;
    assert_eq!(
        connection(&f, f.b.id).await["serving"],
        serving("1", "paused", true, false)
    );
    assert_eq!(
        serving_change(&f.admin, f.b.id, resume, 1, "resume", Some(2)).await,
        "applied"
    );
    assert_eq!(
        serving_change(&f.admin, f.b.id, resume, 1, "resume", None).await,
        "applied"
    );
    let receipts: Vec<(i64, bool)> = sqlx::query_as(
        "SELECT c.revision,c.paused FROM relay_hosted.source_serving_changes c
         JOIN relay_hosted.admin_credentials k ON (k.passkey_id,k.user_id)=(c.confirmed_by_passkey_id,c.actor)
         WHERE c.source_id=$1 ORDER BY c.revision",
    ).bind(f.b.id).fetch_all(&f.admin).await.unwrap();
    assert_eq!(receipts, [(1, true), (2, false)]);
    assert_eq!(f.sends(), (0, 0));
}

fn serving(revision: &str, state: &str, in_pool: bool, draining: bool) -> Value {
    json!({"revision":revision,"state":state,"in_pool":in_pool,"draining":draining})
}
