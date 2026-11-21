use super::{customer_reader::Reader, owner_usage_cases::TAG, *};

const SIBLING: &str = "text-sibling";
const OTHER: &str = "gpt-unattributed";
const LATE: &str = "text-discovered";

async fn view(pool: &PgPool, tag: &[u8]) -> Option<Value> {
    sqlx::query_scalar("SELECT relay_hosted.owner_model_display_order($1,$2)")
        .bind(ORIGIN)
        .bind(tag)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn update(pool: &PgPool, tag: &[u8], expected: &Value, order: &[&str]) -> Option<Value> {
    sqlx::query_scalar("SELECT relay_hosted.change_model_display_order($1,$2,$3,$4)")
        .bind(ORIGIN)
        .bind(tag)
        .bind(expected["revision"].as_str().unwrap())
        .bind(order)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn storefront(customer: &Reader) -> Option<Value> {
    sqlx::query_scalar("SELECT relay_hosted.customer_text_models($1,$2,$3,$4)")
        .bind(ORIGIN)
        .bind(customer.provider)
        .bind(&[0x11_u8; 32][..])
        .bind(customer.tag.as_slice())
        .fetch_one(&customer.pool)
        .await
        .unwrap()
}

fn order(view: &Value) -> Vec<&str> {
    view.as_array()
        .unwrap()
        .iter()
        .map(|row| row["model"].as_str().unwrap())
        .collect()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn catalog_order_keeps_complete_publisher_blocks_and_filters_only_after_ranking() {
    let f = Fixture::new().await;
    let owner = owner_usage_cases::owner(&f).await;
    let customer = Reader::new(&f, f.space, 1).await;
    let metadata = f._cluster.temp.path().join("catalog-publishers.json");
    fs::write(
        &metadata,
        r#"{"openai/fixture-text":{"name":"Fixture"},
             "openai/text-sibling":{"name":"Sibling"},
             "openai/text-discovered":{"name":"Discovered"}}"#,
    )
    .unwrap();
    assert!(
        db::import_catalog_publishers(&owner, ORIGIN, &metadata)
            .await
            .is_err()
    );
    let import = || db::import_catalog_publishers(&f.admin, ORIGIN, &metadata);
    assert_eq!(import().await.unwrap(), 6);
    let sibling = Uuid::new_v4();
    let other = Uuid::new_v4();
    stock(
        &f.admin,
        f.scope,
        &f.a,
        sibling,
        SIBLING,
        30,
        ["2", "1", "3"],
    )
    .await;
    stock(&f.admin, f.scope, &f.a, other, OTHER, 30, ["2", "1", "3"]).await;
    publish(&f.admin, sibling, 1, Some("3.000000000001")).await;
    publish(&f.admin, other, 1, None).await;
    let key_models = || async {
        f.http
            .get(format!("{}/v1/models", f.server.url))
            .bearer_auth(&f.bearer)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };
    let key_before = key_models().await;
    let initial = view(&owner, TAG).await.unwrap();
    assert_eq!(order(&initial["items"]), [MODEL, SIBLING, OTHER]);
    assert_eq!(initial["items"][0]["publisher"], "openai");
    assert!(initial["items"][2]["publisher"].is_null());
    assert_eq!(
        order(&storefront(&customer).await.unwrap()),
        [MODEL, SIBLING, OTHER]
    );
    assert_eq!(import().await.unwrap(), 6);
    assert_eq!(view(&owner, TAG).await, Some(initial.clone()));
    assert!(view(&owner, &customer.tag).await.is_none());
    assert!(
        update(&owner, &customer.tag, &initial, &[OTHER, SIBLING, MODEL])
            .await
            .is_none()
    );
    for bad in [
        vec![MODEL, OTHER, SIBLING],
        vec![MODEL, MODEL, OTHER],
        vec![MODEL, SIBLING],
        vec!["FIXTURE-TEXT", SIBLING, OTHER],
    ] {
        assert_eq!(
            update(&owner, TAG, &initial, &bad).await,
            Some(json!({"error":"invalid"}))
        );
    }
    assert_eq!(
        update(&owner, TAG, &initial, &[MODEL, SIBLING, OTHER]).await,
        Some(initial.clone())
    );
    let a = update(&owner, TAG, &initial, &[OTHER, SIBLING, MODEL]);
    let b = update(&owner, TAG, &initial, &[SIBLING, MODEL, OTHER]);
    let (a, b) = tokio::join!(a, b);
    let conflict = Some(json!({"error":"conflict"}));
    assert_eq!(u8::from(a == conflict) + u8::from(b == conflict), 1);
    let latest = view(&owner, TAG).await.unwrap();
    let saved = update(&owner, TAG, &latest, &[OTHER, SIBLING, MODEL])
        .await
        .unwrap();
    assert_ne!(saved["revision"], initial["revision"]);
    let listing = storefront(&customer).await.unwrap();
    assert_eq!(order(&listing), [OTHER, SIBLING, MODEL]);
    assert_eq!(listing[1]["rates"]["input"], "3.000000000001");
    assert_eq!(listing[1]["revision"], "1");
    assert!(listing[1].get("source").is_none());
    assert_eq!(key_models().await, key_before);
    for period in [24, 168] {
        assert_eq!(
            order(&customer.read_status(period).await.unwrap()["items"]),
            [OTHER, SIBLING, MODEL]
        );
    }

    sqlx::query("UPDATE relay_hosted.text_model_rules SET enabled=false WHERE id=$1")
        .bind(sibling)
        .execute(&f.admin)
        .await
        .unwrap();
    assert_eq!(view(&owner, TAG).await, Some(saved.clone()));
    assert_eq!(order(&storefront(&customer).await.unwrap()), [OTHER, MODEL]);
    assert_eq!(
        order(&customer.read_status(24).await.unwrap()["items"]),
        [OTHER, MODEL]
    );
    let late = Uuid::new_v4();
    stock(&f.admin, f.scope, &f.b, late, LATE, 30, ["2", "1", "3"]).await;
    let discovered = view(&owner, TAG).await.unwrap();
    assert_ne!(discovered["revision"], saved["revision"]);
    assert_eq!(order(&discovered["items"]), [OTHER, SIBLING, MODEL, LATE]);
    assert_eq!(
        update(&owner, TAG, &saved, &[MODEL, SIBLING, OTHER]).await,
        conflict
    );
    assert_eq!(order(&storefront(&customer).await.unwrap()), [OTHER, MODEL]);
    sqlx::query("UPDATE relay_hosted.text_model_rules SET enabled=true WHERE id=$1")
        .bind(sibling)
        .execute(&f.admin)
        .await
        .unwrap();
    assert_eq!(storefront(&customer).await, Some(listing));
    assert_eq!(
        f.a.provider.sends.load(SeqCst) + f.b.provider.sends.load(SeqCst),
        0
    );
    assert!(
        sqlx::query("SELECT * FROM relay_hosted.reference_publishers")
            .fetch_all(&owner)
            .await
            .is_err()
    );
}
