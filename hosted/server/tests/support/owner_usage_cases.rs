use super::{
    customer_reader::{Reader, add_workspace, setup_reader},
    purchase_limit_race::OTHER_MODEL,
    *,
};

pub(super) const TAG: &[u8] = &[0x7e; 32];
const FROM: i64 = 1_709_164_800_123;
const TO: i64 = FROM + 3600000;

pub(super) async fn owner(f: &Fixture) -> PgPool {
    setup_reader(f).await;
    sqlx::raw_sql(
        "UPDATE relay_hosted.installations SET owner_user_id=(SELECT user_id FROM relay_hosted.admin_assignments),setup_state='claimed';
         INSERT INTO relay_hosted.admin_credentials(credential_id,user_id,passkey)
             SELECT '\\x01'::bytea,user_id,'{}' FROM relay_hosted.admin_assignments;",
    )
    .execute(&f.admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO relay_hosted.admin_sessions SELECT $1,user_id,credential_id,
            installation_epoch,user_epoch,now(),now(),now()+interval '8 hours' FROM relay_hosted.active_owner_credentials",
    )
    .bind(TAG)
    .execute(&f.admin)
    .await
    .unwrap();
    db::connect(&fixture_config(), &f._cluster.dsn("report_reader"))
        .await
        .unwrap()
}

async fn list(pool: &PgPool, model: Option<&str>, before: Option<Uuid>, errors: bool) -> Value {
    sqlx::query_scalar(
        "SELECT relay_hosted.owner_request_page($1,$2,NULL,$3,$4,
        ROW($5,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL)::relay_hosted.owner_request_filter,$6,$7)",
    )
    .bind(ORIGIN)
    .bind(TAG)
    .bind(FROM)
    .bind(TO)
    .bind(model)
    .bind(before)
    .bind(errors)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn detail(pool: &PgPool, tag: &[u8], target: Uuid) -> Option<Value> {
    sqlx::query_scalar("SELECT relay_hosted.owner_request_detail($1,$2,$3)")
        .bind(ORIGIN)
        .bind(tag)
        .bind(target)
        .fetch_one(pool)
        .await
        .unwrap()
}

pub(super) async fn request_detail(pool: &PgPool, target: Uuid) -> Value {
    detail(pool, TAG, target).await.unwrap()
}

pub(super) async fn diagnostic_filter(
    pool: &PgPool,
    stage: Option<&str>,
    category: Option<&str>,
    code: Option<&str>,
) -> Value {
    sqlx::query_scalar(
        "SELECT relay_hosted.owner_request_page($1,$2,168,NULL,NULL,
         ROW(NULL,NULL,NULL,NULL,NULL,NULL,$3::text,NULL,$4::text,NULL,$5::text)::relay_hosted.owner_request_filter,NULL,true)",
    )
    .bind(ORIGIN)
    .bind(TAG)
    .bind(stage)
    .bind(category)
    .bind(code)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn exact_interval_keyset_and_error_population_are_owner_only() {
    let f = Fixture::new().await;
    let pool = owner(&f).await;
    db::check_runtime(&pool, ORIGIN).await.unwrap();
    let customer = Reader::new(&f, f.space, 1).await;
    let (_, other_key) = add_workspace(&f).await;
    let other_item = Uuid::new_v4();
    stock(
        &f.admin,
        f.scope,
        &f.a,
        other_item,
        OTHER_MODEL,
        100,
        ["1"; 3],
    )
    .await;
    let other_offer = publish(&f.admin, other_item, 1, None).await;
    let stamp = || {
        sqlx::query_scalar::<_, String>(
            "SELECT last_seen_at::text FROM relay_hosted.admin_sessions WHERE verifier=$1",
        )
        .bind(TAG)
    };
    let last_seen = stamp().fetch_one(&f.admin).await.unwrap();

    // Include two workspaces, tied microsecond start times and both edges of the UTC range.
    sqlx::query(
        "WITH rows AS (
            SELECT $1::uuid AS key,$2::uuid AS offer,0::double precision AS us,'reserved' AS execution,'held' AS accounting
                FROM generate_series(1,35)
            UNION ALL VALUES ($1,$2,-1,'failed','pending'),($1,$2,3600000000,'incomplete','pending'),
                ($3,$4,3599999999,'completed','pending'),($1,$2,2000,'uncertain','pending')
         )
         INSERT INTO relay_hosted.text_requests(workspace_id,key_id,offer_id,authority_epoch,input_tag,send_tag,
            input_bound,output_bound,held,started_at,claim_by,close_by,execution,accounting)
         SELECT k.workspace_id,k.id,r.offer,1,k.verifier,k.verifier,1,1,0,s.at,s.at+interval '1 hour',s.at+interval '2 hours',
            r.execution,r.accounting
         FROM rows r JOIN relay_hosted.customer_keys k ON k.id=r.key
         CROSS JOIN LATERAL (SELECT to_timestamp($5::bigint/1000.0)+r.us*interval '1 microsecond' AS at) s",
    )
    .bind(f.key)
    .bind(f.offer)
    .bind(other_key)
    .bind(other_offer)
    .bind(FROM)
    .execute(&f.admin)
    .await
    .unwrap();

    let first = list(&pool, None, None, false).await;
    assert_eq!(first["from_ms"], FROM);
    assert_eq!(first["to_ms"], TO);
    let first_rows = first["items"].as_array().unwrap();
    assert_eq!(first_rows.len(), 32);
    assert_eq!(first_rows[0]["model"], OTHER_MODEL);
    assert_eq!(first_rows[0]["started_at_ms"], TO - 1);
    assert_eq!(first_rows[0]["issues"], json!(["settlement"]));
    assert!(first_rows[0]["posted_micro_usd"].is_null());
    let cursor = Uuid::parse_str(first["next_before"].as_str().unwrap()).unwrap();
    let next = list(&pool, None, Some(cursor), false).await;
    let tail = next["items"].as_array().unwrap();
    assert!(next["next_before"].is_null());
    assert_eq!(tail.len(), 5);
    let mut ids = std::collections::HashSet::new();
    assert!(
        first_rows
            .iter()
            .chain(tail)
            .all(|r| ids.insert(r["request_id"].as_str().unwrap()))
    );
    let tied = first_rows[2..]
        .iter()
        .chain(tail)
        .map(|r| r["request_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(tied.windows(2).all(|v| v[0] > v[1]));
    assert_eq!(
        list(&pool, None, Some(Uuid::new_v4()), false).await["error"],
        "cursor_unavailable"
    );
    assert_eq!(
        list(&pool, Some("Fixture-text"), None, false).await["items"],
        json!([])
    );
    assert_eq!(
        list(&pool, Some(OTHER_MODEL), None, false).await["items"],
        json!([first_rows[0]])
    );

    let errors = list(&pool, None, None, true).await;
    assert_eq!(errors["items"], json!([first_rows[0], first_rows[1]]));
    assert_eq!(first_rows[1]["issues"], json!(["execution"]));
    let case = detail(&pool, TAG, cursor).await.unwrap();
    assert_eq!(case["receipt"]["request_id"], cursor.to_string());
    assert_eq!(case["timeline"], json!([{"event":"reserved","at_ms":FROM}]));
    assert!(case["attempt"].is_null()); // A logical reservation is not a provider attempt.
    assert_eq!(
        detail(&pool, TAG, Uuid::new_v4()).await.unwrap()["error"],
        "not_found"
    );
    assert!(detail(&pool, &customer.tag, cursor).await.is_none());
    let denied: Option<Value> = sqlx::query_scalar(
        "SELECT relay_hosted.owner_request_page($1,$2,168,NULL,NULL,NULL,NULL,false)",
    )
    .bind(ORIGIN)
    .bind(customer.tag.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(denied.is_none());
    let forbidden = sqlx::query("SELECT relay_hosted.owner_usage_session($1,$2)")
        .bind(ORIGIN)
        .bind(TAG)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        forbidden.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    assert!(
        sqlx::query("SELECT input_tag FROM relay_hosted.text_requests LIMIT 1")
            .execute(&pool)
            .await
            .is_err()
    );
    assert_eq!(stamp().fetch_one(&f.admin).await.unwrap(), last_seen);
    sqlx::query("UPDATE relay_hosted.admin_sessions SET last_seen_at=now()-interval '21 minutes'")
        .execute(&f.admin)
        .await
        .unwrap();
    assert!(detail(&pool, TAG, cursor).await.is_none());
    assert_eq!(f.sends(), (0, 0));
}
