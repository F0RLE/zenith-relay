use super::*;

struct Probe<'a> {
    f: &'a Fixture,
    source: &'a Source,
    id: Uuid,
}

impl<'a> Probe<'a> {
    fn new(f: &'a Fixture, source: &'a Source) -> Self {
        Self {
            f,
            source,
            id: Uuid::new_v4(),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/v1", self.source.listener.url)
    }

    async fn step(&self, qualifier: &PgPool, step: &str) -> String {
        sqlx::query_scalar("SELECT relay_hosted.text_check_step($1,$2,$3,$4,1,0,false,$5,$6)")
            .bind(ORIGIN)
            .bind(self.f.scope)
            .bind(self.id)
            .bind(self.source.id)
            .bind(json!({
                "id":self.id,"scope":self.f.scope,"source":self.source.id,"provider_revision":1,
                "proxy_revision":0,"uses_proxy":false,"source_label":"Fresh check","endpoint":self.endpoint(),
                "model":MODEL,"max_input_bytes":1000,"max_output_tokens":1000,
                "tokens_per_byte":1,"overhead_tokens":0,"close_after_s":65,"valid_for_s":3600
            }))
            .bind(step)
            .fetch_one(qualifier)
            .await
            .unwrap()
    }

    async fn record(&self, qualifier: &PgPool) -> bool {
        sqlx::query_scalar(
            "SELECT relay_hosted.record_text_check($1,$2,$3,$4,1,0,false,$5,$6,'Fresh check',1000,1000,1,0,65,3600,NULL)",
        )
        .bind(ORIGIN).bind(self.f.scope).bind(self.id).bind(self.source.id).bind(self.endpoint()).bind(MODEL)
        .fetch_one(qualifier).await.unwrap()
    }
}

async fn qualifier(f: &Fixture) -> PgPool {
    db::connect(&fixture_config(), &f._cluster.dsn("route_qualifier"))
        .await
        .unwrap()
}

async fn rule(
    f: &Fixture,
    item: Uuid,
    revision: i64,
    action: &str,
    data: Value,
    proof: Option<i64>,
) -> String {
    sqlx::query_scalar(
        "SELECT relay_hosted.change_text_rule($1,$2,$3,gen_random_uuid(),$4,$5,$6,$7,$8,$8,$9,$9,
            floor(extract(epoch FROM clock_timestamp())*1000)::bigint+30000)",
    )
    .bind(ORIGIN)
    .bind(OWNER)
    .bind(item)
    .bind(revision)
    .bind(action)
    .bind(data)
    .bind(proof.map(|_| &[0x42_u8][..]))
    .bind(proof)
    .bind(proof.map(|_| false))
    .fetch_one(&f.admin)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn member_models_keep_a_block_when_a_source_is_parked_and_requalified() {
    let f = Fixture::new().await;
    owner_session(&f.admin).await;
    let b = &f.b;
    let disable = member_models::Edit::new(&f, b.id, 0, json!({(MODEL):false}));
    assert_eq!(disable.apply(&f.admin, Some(1)).await, "applied");
    assert_eq!(
        serving_change(&f.admin, b.id, Uuid::new_v4(), 0, "park", Some(2)).await,
        "applied"
    );
    assert_eq!(
        member_models::view(&f.admin, OWNER, b.id).await,
        Some(json!({"error":"missing"}))
    );
    let mut enable = member_models::Edit::new(&f, b.id, 1, json!({(MODEL):true}));
    assert_eq!(enable.apply(&f.admin, Some(3)).await, "conflict");
    assert_eq!(
        serving_change(&f.admin, b.id, Uuid::new_v4(), 1, "resume", Some(3)).await,
        "applied"
    );
    let q = qualifier(&f).await;
    let new = Probe::new(&f, b);
    assert_eq!(new.step(&q, "prepare").await, "prepared");
    assert_eq!(new.step(&q, "claim").await, "claimed");
    assert!(new.record(&q).await);
    assert_eq!(
        rule(&f, new.id, 0, "admit", json!({}), Some(4)).await,
        "applied"
    );
    let rates =
        json!({"input":"1.000000000000","cached":"0.000000000000","output":"1.000000000000"});
    assert_eq!(
        rule(&f, new.id, 1, "save", json!({"purchase":rates}), None).await,
        "applied"
    );

    let saved = member_models::view(&f.admin, OWNER, b.id).await.unwrap();
    assert_eq!(saved["source_revision"], "2");
    assert_eq!(saved["items"], json!([{"model":MODEL,"allowed":false}]));
    let held = Held::reserve(&f, f.key, &f.bearer).await;
    assert_eq!(held.quotes(&f.worker).await, [(f.a.item, 1, 2300)]);
    assert_eq!(enable.apply(&f.admin, Some(5)).await, "conflict");
    enable.source_revision = 2;
    assert_eq!(enable.apply(&f.admin, Some(5)).await, "applied");
    assert!(
        held.quotes(&f.worker)
            .await
            .iter()
            .all(|route| route.0 != new.id)
    );
    assert_eq!(held.finish(&f.worker, "not_sent").await, "released");
    f.verify_purchase(
        f.submit("new-evidence-after-allow", 1000, 100)
            .send()
            .await
            .unwrap(),
        new.id,
        3500,
        1100,
        "1100",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn parking_an_offer_anchor_fences_waiting_permits_without_withdrawing_the_offer() {
    let f = Fixture::new().await;
    owner_session(&f.admin).await;
    let q = qualifier(&f).await;
    let a = &f.a;
    let old_check = Probe::new(&f, a);
    let stale_admission = Probe::new(&f, a);
    assert_eq!(old_check.step(&q, "prepare").await, "prepared");
    assert_eq!(stale_admission.step(&q, "prepare").await, "prepared");
    assert_eq!(stale_admission.step(&q, "claim").await, "claimed");
    assert!(stale_admission.record(&q).await);
    assert_eq!(
        rule(&f, stale_admission.id, 0, "admit", json!({}), None).await,
        "fresh_required"
    );
    let hold = Held::reserve(&f, f.key, &f.bearer).await;
    assert_eq!(hold.quotes(&f.worker).await.len(), 2);
    let mut park = f.admin.begin().await.unwrap();
    let op = Uuid::new_v4();
    assert_eq!(
        serving_change(&mut *park, a.id, op, 0, "park", Some(1)).await,
        "applied"
    );
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *park)
        .await
        .unwrap();
    let (permit, ()) = tokio::join!(hold.claim_on(&f.worker, f.space, a.id, a.item, 1), async {
        wait_for_block(&f.admin, pid, "relay_hosted.claim_text_candidate(").await;
        park.commit().await.unwrap();
    });
    assert_eq!(permit, "denied");
    let saved = connection(&f, a.id).await;
    assert_eq!(saved["serving"], serving("1", "paused", false, false));
    assert_eq!(saved["verified_models"], json!([]));
    assert_eq!(saved["credential_state"], "stored");
    let (offer, variant): (Uuid, i64) =
        sqlx::query_as("SELECT id,revision FROM relay_hosted.active_text_listings WHERE item=$1")
            .bind(a.item)
            .fetch_one(&f.admin)
            .await
            .unwrap();
    assert_eq!((offer, variant), (f.offer, 1));
    let projected: Value = sqlx::query_scalar(
        "SELECT row FROM jsonb_array_elements(relay_hosted.owner_text_rules($1,$2)) row WHERE row->>'id'=$3",
    )
    .bind(ORIGIN).bind(OWNER).bind(a.item.to_string()).fetch_one(&f.admin).await.unwrap();
    assert_eq!(
        (
            projected["admitted"].as_bool(),
            projected["enabled"].as_bool()
        ),
        (Some(false), Some(true))
    );
    assert!(!projected["effective"].is_null());
    assert_eq!(hold.quotes(&f.worker).await, [(f.b.item, 1, 1600)]);

    let resume = Uuid::new_v4();
    assert_eq!(
        serving_change(&f.admin, a.id, resume, 1, "resume", Some(2)).await,
        "applied"
    );
    assert_eq!(old_check.step(&q, "claim").await, "denied");
    assert_eq!(
        rule(&f, stale_admission.id, 0, "admit", json!({}), Some(3)).await,
        "inactive"
    );
    let current = Probe::new(&f, a);
    assert_eq!(current.step(&q, "prepare").await, "prepared");
    assert_eq!(current.step(&q, "claim").await, "claimed");
    assert!(current.record(&q).await);
    assert_eq!(
        connection(&f, a.id).await["verified_models"],
        json!([MODEL])
    );
    assert_eq!(
        rule(&f, current.id, 0, "admit", json!({}), Some(3)).await,
        "applied"
    );
    let rates =
        json!({"input":"1.000000000000","cached":"0.000000000000","output":"1.000000000000"});
    assert_eq!(
        rule(&f, a.item, 0, "save", json!({"purchase":rates}), None).await,
        "inactive"
    );
    assert_eq!(
        rule(&f, current.id, 1, "save", json!({"purchase":rates}), None).await,
        "applied"
    );
    assert_eq!(
        connection(&f, a.id).await["serving"],
        serving("2", "enabled", true, false)
    );
    assert!(!hold.claim(&f, a.id, a.item, 1).await);
    assert!(!hold.claim(&f, a.id, current.id, 1).await);
    assert!(hold.claim(&f, f.b.id, f.b.item, 1).await);
    assert_eq!(hold.finish(&f.worker, "completed").await, "settled");
    verify_receipt(&f.admin, hold.id, f.b.item, 3500, 1600, "1600").await;
    f.verify_purchase(
        f.submit("newly-admitted-binding", 1000, 100)
            .send()
            .await
            .unwrap(),
        current.id,
        3500,
        1100,
        "1100",
    )
    .await;
    assert_eq!(f.sends(), (1, 0));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn trash_and_restore_drain_a_committed_permit_without_reenabling_the_only_supplier() {
    let f = Fixture::new().await;
    owner_session(&f.admin).await;
    // The remaining supplier uses a protected proxy binding.
    f.vault.revoke(f.b.id, Purpose::Provider, 1).await.unwrap();
    let a = &f.a;
    let proxy = b"http://fixture:fixture@127.0.0.1:1";
    f.vault.put(a.id, Purpose::Proxy, 0, proxy).await.unwrap();
    let proxied: Uuid = sqlx::query_scalar(
        "WITH binding AS (
            INSERT INTO relay_hosted.text_inventory(id,scope_id,source_id,provider_revision,proxy_revision,uses_proxy,base_url,
                model,source_label,max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,close_after_s,valid_until)
            SELECT gen_random_uuid(),scope_id,source_id,provider_revision,1,true,base_url,model,source_label,
                max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,close_after_s,valid_until
            FROM relay_hosted.text_inventory WHERE id=$1 RETURNING id,scope_id,model
         ), rate AS (
            INSERT INTO relay_hosted.text_purchase_rates SELECT b.id,1,p.input_rate,p.cached_rate,p.output_rate
            FROM binding b CROSS JOIN relay_hosted.text_purchase_rates p WHERE p.item=$1 RETURNING item
         ) INSERT INTO relay_hosted.text_model_rules(id,scope_id,model,admitted,purchase_revision)
            SELECT id,scope_id,model,true,1 FROM binding JOIN rate ON item=id RETURNING id",
    ).bind(a.item).fetch_one(&f.admin).await.unwrap();
    let hold = Held::reserve(&f, f.key, &f.bearer).await;
    let mut send = f.admin.begin().await.unwrap();
    assert_eq!(
        hold.claim_on(&mut *send, f.space, a.id, proxied, 1).await,
        "claimed"
    );
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *send)
        .await
        .unwrap();
    let op = Uuid::new_v4();
    let (trashed, ()) = tokio::join!(
        serving_change(&f.admin, a.id, op, 0, "trash", Some(1)),
        async {
            wait_for_block(&f.admin, pid, "relay_hosted.change_source_serving(").await;
            send.commit().await.unwrap();
        }
    );
    assert_eq!(trashed, "applied");
    assert!(!f.listed().await);
    assert_eq!(
        connection(&f, a.id).await["serving"],
        serving("1", "trashed", false, true)
    );
    let remove = Uuid::new_v4();
    assert_eq!(
        serving_change(&f.admin, a.id, remove, 1, "remove", Some(2)).await,
        "draining"
    );
    assert_eq!(
        serving_change(&f.admin, a.id, op, 0, "park", None).await,
        "conflict"
    );
    assert_eq!(
        serving_change(&f.admin, a.id, Uuid::new_v4(), 1, "resume", None).await,
        "conflict"
    );
    let restored = Uuid::new_v4();
    assert_eq!(
        serving_change(&f.admin, a.id, restored, 1, "restore", Some(2)).await,
        "applied"
    );
    assert_eq!(
        serving_change(&f.admin, a.id, op, 0, "trash", None).await,
        "applied"
    );
    assert_eq!(
        connection(&f, a.id).await["serving"],
        serving("2", "paused", false, true)
    );
    let resume = Uuid::new_v4();
    assert_eq!(
        serving_change(&f.admin, a.id, resume, 2, "resume", Some(3)).await,
        "draining"
    );
    assert_eq!(hold.finish(&f.worker, "completed").await, "settled");
    verify_receipt(&f.admin, hold.id, proxied, 3500, 2300, "2300").await;
    assert_eq!(
        serving_change(&f.admin, a.id, resume, 2, "resume", Some(3)).await,
        "applied"
    );
    let saved = connection(&f, a.id).await;
    assert_eq!(saved["proxy_bound"], true);
    assert_eq!(saved["verified_models"], json!([]));
    assert_eq!(saved["serving"], serving("3", "enabled", false, false));
    assert!(
        f.vault
            .read(a.id, Purpose::Provider, 1)
            .await
            .unwrap()
            .as_slice()
            == b"fixture-supplier"
    );
    assert!(
        f.vault
            .read(a.id, Purpose::Proxy, 1)
            .await
            .unwrap()
            .as_slice()
            == proxy
    );
    assert!(!f.listed().await);
    assert_eq!(f.sends(), (0, 0));

    assert_eq!(
        serving_change(&f.admin, a.id, remove, 3, "remove", None).await,
        "conflict"
    );
    assert_eq!(
        serving_change(&f.admin, a.id, Uuid::new_v4(), 3, "trash", Some(4)).await,
        "applied"
    );
    assert_eq!(
        serving_change(&f.admin, a.id, remove, 1, "remove", Some(5)).await,
        "conflict"
    );
    assert_eq!(
        serving_change(&f.admin, a.id, remove, 4, "remove", None).await,
        "fresh_required"
    );
    assert_eq!(
        serving_change(&f.admin, a.id, remove, 4, "remove", Some(5)).await,
        "applied"
    );
    assert_eq!(
        serving_change(&f.admin, a.id, remove, 4, "remove", None).await,
        "applied"
    );
    assert_eq!(
        serving_change(&f.admin, a.id, remove, 4, "restore", None).await,
        "conflict"
    );
    assert_eq!(
        serving_change(&f.admin, a.id, Uuid::new_v4(), 5, "restore", Some(6)).await,
        "missing"
    );
    for purpose in [Purpose::Provider, Purpose::Proxy] {
        assert!(matches!(
            f.vault.read(a.id, purpose, 1).await,
            Err(zenith_relay_hosted::vault::Error::Stale)
        ));
        assert!(matches!(
            f.vault.put(a.id, purpose, 2, b"late fixture write").await,
            Err(zenith_relay_hosted::vault::Error::Stale)
        ));
    }
    let removed = connection(&f, a.id).await;
    assert_eq!(removed["credential_state"], "revoked");
    assert_eq!(removed["proxy_bound"], false);
    assert!(removed["serving"].is_null());
    verify_receipt(&f.admin, hold.id, proxied, 3500, 2300, "2300").await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn trash_rejects_late_json_and_stream_evidence() {
    let f = Fixture::new().await;
    owner_session(&f.admin).await;
    let q = qualifier(&f).await;
    let stream_id = Uuid::new_v4();
    let stream = |step: &str| {
        sqlx::query_scalar::<_, Option<Value>>(
            "SELECT relay_hosted.text_stream_check($1,$2,$3,$4,$5)",
        )
        .bind(ORIGIN)
        .bind(f.scope)
        .bind(stream_id)
        .bind(f.a.item)
        .bind(step.to_owned())
        .fetch_one(&q)
    };
    assert_eq!(
        stream("prepare").await.unwrap().unwrap()["state"],
        "prepared"
    );
    let probe = Probe::new(&f, &f.a);
    assert_eq!(probe.step(&q, "prepare").await, "prepared");
    assert_eq!(probe.step(&q, "claim").await, "claimed");
    assert_eq!(
        serving_change(&f.admin, f.a.id, Uuid::new_v4(), 0, "trash", Some(1)).await,
        "applied"
    );
    assert_eq!(Probe::new(&f, &f.a).step(&q, "prepare").await, "denied");
    assert!(!probe.record(&q).await);
    assert!(stream("claim").await.unwrap().is_none());
    assert!(stream("record").await.unwrap().is_none());
    assert_eq!(
        serving_change(&f.admin, f.a.id, Uuid::new_v4(), 1, "restore", Some(2)).await,
        "applied"
    );
    assert!(!probe.record(&q).await);
    assert!(
        connection(&f, f.a.id).await["verified_models"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
