use ring::digest::{SHA256, digest};
use sqlx::{Acquire, PgPool};
use uuid::Uuid;

/// Exercise the journal at an alternate tariff without publishing it, sending a
/// request, or changing the two customers' end-to-end receipts. All writes roll back.
pub async fn check(
    db: &PgPool,
    origin: &str,
    previous: Uuid,
    workspace: Uuid,
    key: Uuid,
    raw: &str,
) {
    let mut tx = db.begin().await.unwrap();
    let offer = Uuid::new_v4();
    sqlx::query("INSERT INTO relay_hosted.text_offers(id,inventory_id,scope_id,source_id,provider_revision,base_url,model,revision,
        max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,input_rate,cached_rate,output_rate,close_after_s)
        SELECT $1,inventory_id,scope_id,source_id,provider_revision,base_url,model,revision+1,max_input_bytes,max_output_tokens,
        1,0,1.25,0.5,2.125,65 FROM relay_hosted.text_offers WHERE id=$2")
        .bind(offer).bind(previous).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE relay_hosted.text_model_rules SET last_offer=$1 WHERE last_offer=$2")
        .bind(offer)
        .bind(previous)
        .execute(&mut *tx)
        .await
        .unwrap();
    let proof = digest(&SHA256, raw.as_bytes());
    let seal = &[17_u8; 32][..];
    let reserve = || {
        sqlx::query_as::<_,(String,Option<Uuid>,Option<Uuid>,Option<i64>)>(
        "SELECT result,rid,attempt,held FROM relay_hosted.reserve_text($1,$2,$3,$4,NULL,$5,$5,'fixture-text',4,3)")
        .bind(origin).bind(offer).bind(key).bind(proof.as_ref()).bind(seal)
    };
    for case in ["partial", "final", "excess"] {
        let mut step = tx.begin().await.unwrap();
        let (state, rid, aid, hold) = reserve().fetch_one(&mut *step).await.unwrap();
        let (rid, aid) = (rid.unwrap(), aid.unwrap());
        assert_eq!((state.as_str(), hold), ("new", Some(12)));
        let claim = |offset: i64| {
            sqlx::query_scalar::<_,bool>("SELECT relay_hosted.claim_text($1,r.offer_id,r.id,r.attempt_id,r.workspace_id,
            r.key_id,o.source_id,o.model,r.authority_epoch+$3,o.revision,r.held,r.send_tag,o.max_input_bytes,r.output_bound)
            FROM relay_hosted.text_requests r JOIN relay_hosted.text_offers o ON o.id=r.offer_id WHERE r.id=$2")
            .bind(origin).bind(rid).bind(offset)
        };
        assert!(!claim(1).fetch_one(&mut *step).await.unwrap());
        let purchase = || {
            sqlx::query_as::<_, (i64, i64, Option<String>)>(
                "SELECT a.rate_revision,a.bound_micro_usd,p.valued_micro_usd::text
                 FROM relay_hosted.text_purchase_attempts a LEFT JOIN relay_hosted.text_purchase_results p USING(attempt_id)
                 WHERE a.attempt_id=$1",
            )
            .bind(aid)
        };
        assert!(
            purchase()
                .fetch_optional(&mut *step)
                .await
                .unwrap()
                .is_none()
        );
        if case == "partial" {
            sqlx::query(
                "WITH price AS (
                    INSERT INTO relay_hosted.text_purchase_rates(item,revision,input_rate,cached_rate,output_rate)
                    SELECT id,purchase_revision+1,16,3,13 FROM relay_hosted.text_model_rules WHERE last_offer=$1
                    RETURNING item,revision
                 ) UPDATE relay_hosted.text_model_rules r SET purchase_revision=p.revision FROM price p WHERE r.id=p.item",
            )
            .bind(offer)
            .execute(&mut *step)
            .await
            .unwrap();
        }
        assert!(claim(0).fetch_one(&mut *step).await.unwrap());
        assert!(!claim(0).fetch_one(&mut *step).await.unwrap());
        let (revision, bound) = if case == "partial" { (3, 103) } else { (2, 62) };
        assert_eq!(
            purchase().fetch_one(&mut *step).await.unwrap(),
            (revision, bound, None)
        );
        let (inputs, cached) = match case {
            "partial" => (4, None),
            "final" => (4, Some(2)),
            _ => (80, Some(0)),
        };
        let state: String =
            sqlx::query_scalar("SELECT relay_hosted.finish_text($1,$2,$3,'completed',$4,$5,2)")
                .bind(offer)
                .bind(rid)
                .bind(aid)
                .bind(i64::from(inputs))
                .bind(cached.map(i64::from))
                .fetch_one(&mut *step)
                .await
                .unwrap();
        if case == "partial" {
            assert_eq!(state, "pending");
            let result: (Option<i64>, Option<i64>, Option<i64>) = sqlx::query_as(
                "SELECT cached_tokens,output_tokens,charged
                FROM relay_hosted.text_requests WHERE id=$1",
            )
            .bind(rid)
            .fetch_one(&mut *step)
            .await
            .unwrap();
            assert_eq!(result, (None, Some(2), None));
            assert_eq!(
                purchase().fetch_one(&mut *step).await.unwrap(),
                (revision, bound, None)
            );
            step.rollback().await.unwrap();
            continue;
        }
        assert_eq!(state, "settled");
        let amounts: (i64, i64) = sqlx::query_as(
            "SELECT valued::bigint,charged FROM relay_hosted.text_requests WHERE id=$1",
        )
        .bind(rid)
        .fetch_one(&mut *step)
        .await
        .unwrap();
        // Fractional disjoint lines are rounded once. Excess is never customer debt.
        assert_eq!(amounts, if case == "final" { (8, 8) } else { (105, 12) });
        let expense = if case == "final" { "40" } else { "660" };
        assert_eq!(
            purchase().fetch_one(&mut *step).await.unwrap(),
            (revision, bound, Some(expense.into()))
        );
        step.commit().await.unwrap();
    }
    assert_eq!(reserve().fetch_one(&mut *tx).await.unwrap().0, "unoffered");
    let authority: Option<Uuid> =
        sqlx::query_scalar("SELECT relay_hosted.lock_text_authority($1,$2,$3,$4)")
            .bind(origin)
            .bind(previous)
            .bind(key)
            .bind(proof.as_ref())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(
        authority.is_none(),
        "bound breach fences the inventory across tariffs"
    );
    let wallet:(i64,i64)=sqlx::query_as("SELECT available_micro_usd,reserved_micro_usd FROM relay_hosted.wallets WHERE workspace_id=$1")
        .bind(workspace).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(wallet, (8_800_000 - 8 - 12, 0));
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
        .execute(&mut *tx)
        .await
        .unwrap();
    for tamper in [
        "UPDATE relay_hosted.text_requests SET held=held+1 WHERE offer_id=$1",
        "UPDATE relay_hosted.text_requests SET output_tokens=0 WHERE offer_id=$1",
        "UPDATE relay_hosted.text_purchase_results SET valued_micro_usd=0
         WHERE attempt_id IN (SELECT attempt_id FROM relay_hosted.text_requests WHERE offer_id=$1)",
        "INSERT INTO relay_hosted.journal_postings SELECT j.id,3,a.id,1 FROM relay_hosted.journal_transactions j
         JOIN relay_hosted.text_requests r ON r.id=j.request_id CROSS JOIN relay_hosted.ledger_accounts a
         WHERE r.offer_id=$1 AND j.operation_kind='request_hold' AND a.account_kind='inference_revenue' LIMIT 1",
    ] {
        let mut savepoint=tx.begin().await.unwrap();
        let error=sqlx::query(tamper).bind(offer).execute(&mut *savepoint).await.unwrap_err();
        assert_eq!(error.as_database_error().unwrap().code().as_deref(),Some("23514"));
        savepoint.rollback().await.unwrap();
    }
    tx.rollback().await.unwrap();
}
