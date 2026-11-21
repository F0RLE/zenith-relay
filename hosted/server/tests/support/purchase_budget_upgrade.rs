use super::partial_text::{Attempt, PartialText};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub async fn seed_legacy(db: &PgPool, offer: Uuid) {
    // A zero selling tariff makes the fixture independent of payment setup.
    sqlx::raw_sql(
        "WITH customer AS (INSERT INTO relay_hosted.users(id) VALUES(gen_random_uuid()) RETURNING id),
         workspace AS (INSERT INTO relay_hosted.workspaces(owner_user_id) SELECT id FROM customer RETURNING *),
         identity AS (INSERT INTO relay_hosted.external_identities(provider_id,subject_digest,user_id)
             SELECT provider_id,decode(repeat('10',32),'hex'),owner_user_id
             FROM relay_hosted.configuration_revisions CROSS JOIN workspace RETURNING *)
         INSERT INTO relay_hosted.qualification_subjects(scope_id,subject_digest,workspace_id)
             SELECT q.id,i.subject_digest,w.id FROM relay_hosted.configuration_revisions q
             JOIN identity i USING(provider_id) JOIN workspace w ON w.owner_user_id=i.user_id;
         INSERT INTO relay_hosted.customer_keys(id,workspace_id,scope_id,issue_operation,label,verifier,user_epoch,
             installation_epoch,created_at,expires_at)
         SELECT gen_random_uuid(),q.workspace_id,q.scope_id,gen_random_uuid(),'Test key',
             decode(repeat(lpad(n::text,2,'0'),32),'hex'),u.authorization_epoch,h.authorization_epoch,now(),now()+interval '1 hour'
         FROM relay_hosted.qualification_subjects q JOIN relay_hosted.workspaces w ON w.id=q.workspace_id
         JOIN relay_hosted.users u ON u.id=w.owner_user_id CROSS JOIN relay_hosted.installations h CROSS JOIN generate_series(1,2) n;",
    )
    .execute(db)
    .await
    .unwrap();
    let (rid, aid) = historic_permit(db, offer, false, false).await;
    assert_eq!(finish(db, offer, (rid, aid), 3, 1).await, "settled");
}

async fn historic_permit(
    db: &PgPool,
    offer: Uuid,
    prior_month: bool,
    priced: bool,
) -> (Uuid, Uuid) {
    let mut tx = db.begin().await.unwrap();
    let row: (Uuid, Uuid) = sqlx::query_as(
        "WITH moment AS (SELECT CASE WHEN $2 THEN
           (date_trunc('month',clock_timestamp() AT TIME ZONE 'UTC')-interval '1 month'+interval '1 hour') AT TIME ZONE 'UTC'
           ELSE clock_timestamp() END AS stamp)
         INSERT INTO relay_hosted.text_requests(workspace_id,key_id,offer_id,authority_epoch,input_tag,send_tag,input_bound,output_bound,
             held,started_at,claim_by,close_by,claimed_at,execution)
         SELECT k.workspace_id,k.id,$1,s.admission_epoch,k.verifier,k.verifier,45,55,0,stamp-interval '1 second',
             stamp+interval '30 seconds',stamp+interval '65 seconds',stamp,'dispatched'
         FROM relay_hosted.customer_keys k CROSS JOIN relay_hosted.service_control s CROSS JOIN moment
         ORDER BY k.verifier LIMIT 1 RETURNING id,attempt_id",
    )
    .bind(offer)
    .bind(prior_month)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query("SELECT relay_hosted.record_request_journal(r,'request_hold') FROM relay_hosted.text_requests r WHERE id=$1")
        .bind(row.0).execute(&mut *tx).await.unwrap();
    if priced {
        sqlx::query(
            "INSERT INTO relay_hosted.text_purchase_attempts(attempt_id,item,rate_revision,month_utc,bound_micro_usd)
             SELECT r.attempt_id,o.inventory_id,1,date_trunc('month',r.claimed_at AT TIME ZONE 'UTC')::date,100
             FROM relay_hosted.text_requests r JOIN relay_hosted.text_offers o ON o.id=r.offer_id WHERE r.id=$1",
        )
        .bind(row.0)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    row
}

async fn finish(
    db: &PgPool,
    offer: Uuid,
    request: (Uuid, Uuid),
    input: i64,
    output: i64,
) -> String {
    sqlx::query_scalar("SELECT relay_hosted.finish_text($1,$2,$3,'completed',$4,0,$5)")
        .bind(offer)
        .bind(request.0)
        .bind(request.1)
        .bind(input)
        .bind(output)
        .fetch_one(db)
        .await
        .unwrap()
}

pub async fn seed_priced(db: &PgPool, item: Uuid, offer: Uuid) -> (Uuid, Uuid) {
    sqlx::query("INSERT INTO relay_hosted.text_purchase_rates(item,revision,input_rate,cached_rate,output_rate) VALUES($1,1,1,0,1)")
        .bind(item).execute(db).await.unwrap();
    let settled = historic_permit(db, offer, false, true).await;
    assert_eq!(finish(db, offer, settled, 10, 20).await, "settled");
    historic_permit(db, offer, true, true).await
}

pub async fn seed_late_usage(db: &PgPool, template: Uuid) -> (Uuid, Attempt) {
    let (key, space): (Uuid, Uuid) = sqlx::query_as(
        "SELECT id,workspace_id FROM relay_hosted.customer_keys ORDER BY verifier LIMIT 1",
    )
    .fetch_one(db)
    .await
    .unwrap();
    let binding: Uuid = sqlx::query_scalar(
        "INSERT INTO relay_hosted.merchant_bindings(provider_id,environment,merchant_identity)
         VALUES('fixture','sandbox','upgrade') RETURNING id",
    )
    .fetch_one(db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO relay_hosted.payment_intents(workspace_id,operation_id,quote_id,merchant_binding_id,
             merchant_reference,quoted_currency,quoted_amount_minor,credit_micro_usd)
         VALUES($1,gen_random_uuid(),gen_random_uuid(),$2,'upgrade-funds','USD',1,10000)",
    )
    .bind(space)
    .bind(binding)
    .execute(db)
    .await
    .unwrap();
    let receipt: Uuid = sqlx::query_scalar(
        "SELECT relay_hosted.post_verified_payment($1,'paid','upgrade-funds','USD',1)",
    )
    .bind(binding)
    .fetch_one(db)
    .await
    .unwrap();
    assert!(!receipt.is_nil());
    let (item, offer): (Uuid, Uuid) = sqlx::query_as(
        "WITH stock AS (
            INSERT INTO relay_hosted.text_inventory(id,scope_id,source_id,provider_revision,proxy_revision,uses_proxy,
                base_url,model,source_label,max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,close_after_s,valid_until)
            SELECT gen_random_uuid(),scope_id,gen_random_uuid(),1,0,false,base_url,'fixture-adjustment','Adjustment fixture',
                1000,100,1,0,65,now()+interval '10 minutes' FROM relay_hosted.text_offers WHERE id=$1 RETURNING *
         ), rated AS (
            INSERT INTO relay_hosted.text_purchase_rates(item,revision,input_rate,cached_rate,output_rate)
            SELECT id,1,1,0,1 FROM stock RETURNING item
         ) INSERT INTO relay_hosted.text_offers(id,inventory_id,scope_id,source_id,provider_revision,base_url,model,revision,
             max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,input_rate,cached_rate,output_rate,close_after_s)
         SELECT gen_random_uuid(),i.id,i.scope_id,i.source_id,1,i.base_url,i.model,1,i.max_input_bytes,i.max_output_tokens,1,0,3,1,5,65
         FROM stock i JOIN rated p ON p.item=i.id RETURNING inventory_id,id",
    )
    .bind(template)
    .fetch_one(db)
    .await
    .unwrap();
    let attempt = PartialText {
        key,
        offer,
        item,
        rate_revision: 1,
        claimed_at: "2024-02-29 23:59:30+00".into(),
        input_bound: 1000,
        output_bound: 100,
        observed_output: 100,
    }
    .seed(db)
    .await;
    assert_eq!(attempt.finish(db, "deadline", [None; 3]).await, "settled");
    assert_eq!(
        attempt
            .finish(db, "incomplete", [Some(100), Some(0), Some(20)])
            .await,
        "settled"
    );
    (space, attempt)
}

pub async fn check_late_usage(db: &PgPool, (space, attempt): (Uuid, Attempt)) {
    let wallet = || {
        sqlx::query_scalar::<_, i64>(
            "SELECT available_micro_usd FROM relay_hosted.wallets WHERE workspace_id=$1",
        )
        .bind(space)
        .fetch_one(db)
    };
    assert_eq!(wallet().await.unwrap(), 9500);
    assert_eq!(
        zenith_relay_hosted::inference::reconcile_due(db)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        zenith_relay_hosted::inference::reconcile_due(db)
            .await
            .unwrap(),
        0
    );
    assert_eq!(wallet().await.unwrap(), 9600);
    let state: (i64, String, i64) = sqlx::query_as(
        "SELECT r.charged,r.basis,c.amount_micro_usd FROM relay_hosted.text_requests r
         JOIN relay_hosted.text_customer_credits c ON c.request_id=r.id
         JOIN relay_hosted.journal_transactions j ON j.request_id=r.id AND j.operation_kind='request_credit'
         WHERE r.id=$1",
    )
    .bind(attempt.request)
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(state, (500, "provider_partial".into(), 100));
}

pub async fn check(db: &PgPool, origin: &str, offer: Uuid, late: (Uuid, Uuid)) {
    let source: Uuid =
        sqlx::query_scalar("SELECT source_id FROM relay_hosted.text_offers WHERE id=$1")
            .bind(offer)
            .fetch_one(db)
            .await
            .unwrap();
    let periods = || {
        sqlx::query_as::<_, (bool, String, String, bool)>(
            "SELECT month_utc=date_trunc('month',clock_timestamp() AT TIME ZONE 'UTC')::date,
                    accounted_micro_usd::text,outstanding_micro_usd::text,unpriced
             FROM relay_hosted.text_purchase_periods WHERE source_id=$1 ORDER BY month_utc",
        )
        .bind(source)
        .fetch_all(db)
    };
    let current = (true, "30".into(), "0".into(), true);
    assert_eq!(
        periods().await.unwrap(),
        [(false, "0".into(), "100".into(), false), current.clone()]
    );
    for _ in 0..2 {
        assert_eq!(finish(db, offer, late, 4, 3).await, "released");
    }
    assert_eq!(
        periods().await.unwrap(),
        [(false, "7".into(), "0".into(), false), current]
    );
    check_scope(db, origin, source, offer).await;
}

async fn check_scope(db: &PgPool, origin: &str, source: Uuid, template: Uuid) {
    let mut tx = db.begin().await.unwrap();
    let other = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO relay_hosted.connection_secret_slots
         SELECT installation_id,$1,'provider',1,gen_random_uuid(),decode(repeat('25',32),'hex') FROM relay_hosted.vault_bindings",
    )
    .bind(other).execute(&mut *tx).await.unwrap();
    sqlx::query(
        "WITH stock AS (
             INSERT INTO relay_hosted.text_inventory(id,scope_id,source_id,provider_revision,proxy_revision,uses_proxy,
                 base_url,model,source_label,max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,close_after_s,valid_until)
             SELECT gen_random_uuid(),o.scope_id,x.source,1,0,false,o.base_url,x.model,'Fixture',4000,64,1,0,65,now()+interval '30 minutes'
             FROM relay_hosted.text_offers o CROSS JOIN (VALUES($1::uuid,'old-source'),($2,'other-one'),($2,'other-two')) x(source,model)
             WHERE o.id=$3 RETURNING *
         ), rated AS (
             INSERT INTO relay_hosted.text_purchase_rates(item,revision,input_rate,cached_rate,output_rate)
             SELECT id,1,1,0,1 FROM stock RETURNING item,revision
         ), published AS (
             INSERT INTO relay_hosted.text_offers(id,inventory_id,scope_id,source_id,provider_revision,base_url,model,revision,
                 max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,input_rate,cached_rate,output_rate,close_after_s)
             SELECT gen_random_uuid(),id,scope_id,source_id,provider_revision,base_url,model,revision,
                 max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,0,0,0,close_after_s
             FROM stock JOIN rated ON id=item RETURNING *
         )
         INSERT INTO relay_hosted.text_model_rules(id,scope_id,model,admitted,draft,last_offer,enabled,first_live_revision,purchase_revision)
         SELECT inventory_id,scope_id,model,true,relay_hosted.text_rates(p),id,true,revision,revision FROM published p",
    )
    .bind(source).bind(other).bind(template).execute(&mut *tx).await.unwrap();
    sqlx::query(
        "INSERT INTO relay_hosted.text_purchase_limit_changes(source_id,revision,operation,actor,limit_micro_usd)
         SELECT x.source,1,gen_random_uuid(),a.user_id,x.cap FROM relay_hosted.admin_assignments a
         CROSS JOIN (VALUES($1::uuid,1000),($2,5)) x(source,cap) WHERE a.role='owner'",
    )
    .bind(source).bind(other).execute(&mut *tx).await.unwrap();
    // Old spend is unknown on A, but B can serve. After B's first attempt is
    // settled (no busy guard), its second model and key share the same cap.
    permit(&mut tx, origin, "old-source", 0, false).await;
    permit(&mut tx, origin, "other-one", 0, true).await;
    permit(&mut tx, origin, "other-two", 1, false).await;
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
}

async fn permit(
    tx: &mut Transaction<'_, Postgres>,
    origin: &str,
    model: &str,
    offset: i64,
    allowed: bool,
) {
    let (key, verifier): (Uuid, Vec<u8>) = sqlx::query_as(
        "SELECT id,verifier FROM relay_hosted.customer_keys ORDER BY verifier OFFSET $1 LIMIT 1",
    )
    .bind(offset)
    .fetch_one(&mut **tx)
    .await
    .unwrap();
    let (state, rid, aid, oid): (String, Option<Uuid>, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT result,rid,attempt,offer FROM relay_hosted.reserve_published_text($1,$2,$3,NULL,$3,$3,$4,3,1)",
    )
    .bind(origin).bind(key).bind(&verifier).bind(model).fetch_one(&mut **tx).await.unwrap();
    assert_eq!(state, "new");
    let rid = rid.unwrap();
    let result: bool = sqlx::query_scalar(
        "SELECT relay_hosted.claim_published_text($1,r.id,r.attempt_id,r.workspace_id,r.key_id,o.source_id,o.model,
            r.authority_epoch,o.revision,r.held,r.send_tag,o.max_input_bytes,r.output_bound)
         FROM relay_hosted.text_requests r JOIN relay_hosted.text_offers o ON o.id=r.offer_id WHERE r.id=$2",
    )
    .bind(origin).bind(rid).fetch_one(&mut **tx).await.unwrap();
    assert_eq!(result, allowed, "{model}");
    let result: String = sqlx::query_scalar("SELECT relay_hosted.finish_text($1,$2,$3,$4,3,0,1)")
        .bind(oid)
        .bind(rid)
        .bind(aid)
        .bind(if allowed { "completed" } else { "not_sent" })
        .fetch_one(&mut **tx)
        .await
        .unwrap();
    assert_eq!(result, if allowed { "settled" } else { "released" });
}
