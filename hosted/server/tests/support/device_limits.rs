use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

const CHANGE: &str = "SELECT relay_hosted.change_device($1,s.verifier,$2,$3,$4,$5,$6,$7,'{}',false,
    CASE WHEN $8 THEN s.credential_id END,k.revision,(k.passkey #>> '{cred,counter}')::bigint+1,
    false,false,floor(extract(epoch FROM clock_timestamp())*1000)::bigint+30000)
    FROM (SELECT * FROM relay_hosted.admin_sessions ORDER BY last_seen_at DESC LIMIT 1) s
    JOIN relay_hosted.admin_credentials k ON k.credential_id=s.credential_id";

async fn change(
    tx: &mut Transaction<'_, Postgres>,
    origin: &str,
    device: Uuid,
    revision: i64,
    action: &str,
    proof: Option<&[u8]>,
    confirm: bool,
) -> String {
    sqlx::query_scalar(CHANGE)
        .bind(origin)
        .bind(if action == "invite" {
            device
        } else {
            Uuid::new_v4()
        })
        .bind(device)
        .bind(revision)
        .bind(action)
        .bind((action == "invite").then_some("Fixture device"))
        .bind(proof)
        .bind(confirm)
        .fetch_one(&mut **tx)
        .await
        .unwrap()
}

// Leave the approved browser/device pair unchanged for the subsequent restart.
pub(super) async fn check(db: &PgPool, origin: &str, device: Uuid) {
    let mut tx = db.begin().await.unwrap();
    let audit: (i64, i64) = sqlx::query_as(
        "SELECT count(*),count(confirmed_by_passkey_id) FROM relay_hosted.device_changes WHERE device_id=$1",
    )
    .bind(device)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(audit, (2, 2));
    let deadline = sqlx::query("UPDATE relay_hosted.management_devices SET
        invited_at=invited_at-interval '25 hours', approve_by=approve_by-interval '25 hours',
        approved_at=approved_at-interval '25 hours', expires_at=expires_at-interval '25 hours' WHERE id=$1")
        .bind(device).execute(&mut *tx).await.unwrap();
    assert_eq!(deadline.rows_affected(), 1);
    let expired: bool = sqlx::query_scalar(
        "SELECT relay_hosted.device_connections($1,
        installation_id,id,verifier,NULL) IS NULL FROM relay_hosted.management_devices WHERE id=$2",
    )
    .bind(origin)
    .bind(device)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(expired);

    let mut pending = Vec::new();
    for index in 0..17 {
        let id = Uuid::new_v4();
        let result = change(&mut tx, origin, id, 0, "invite", None, true).await;
        assert_eq!(result, if index < 16 { "applied" } else { "limit" });
        pending.push(id);
    }
    let proof = [31u8; 32];
    assert_eq!(
        change(&mut tx, origin, pending[0], 1, "revoke", None, false).await,
        "fresh_required"
    );
    assert_eq!(
        change(&mut tx, origin, pending[0], 1, "revoke", None, true).await,
        "applied"
    );
    assert_eq!(
        change(
            &mut tx,
            origin,
            pending[0],
            1,
            "approve",
            Some(&proof),
            true
        )
        .await,
        "conflict"
    );
    sqlx::query(
        "UPDATE relay_hosted.management_devices SET invited_at=invited_at-interval '6 minutes',
        approve_by=approve_by-interval '6 minutes' WHERE id=$1",
    )
    .bind(pending[1])
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        change(
            &mut tx,
            origin,
            pending[1],
            1,
            "approve",
            Some(&proof),
            true
        )
        .await,
        "conflict"
    );
    sqlx::query("UPDATE relay_hosted.admin_sessions SET created_at=now()")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        change(
            &mut tx,
            origin,
            pending[2],
            1,
            "approve",
            Some(&proof),
            false
        )
        .await,
        "fresh_required"
    );
    tx.rollback().await.unwrap();
}
