use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

const INVITE: &str = "SELECT relay_hosted.change_device($1,
    (SELECT verifier FROM relay_hosted.admin_sessions ORDER BY created_at DESC LIMIT 1),
    $2,$2,0,'invite','Fixture device',NULL)";
const CHANGE: &str = "SELECT relay_hosted.change_device($1,
    (SELECT verifier FROM relay_hosted.admin_sessions ORDER BY created_at DESC LIMIT 1),
    $2,$3,$4,$5,NULL,$6)";

async fn change(
    tx: &mut Transaction<'_, Postgres>,
    origin: &str,
    device: Uuid,
    revision: i64,
    action: &str,
    proof: Option<&[u8]>,
) -> String {
    sqlx::query_scalar(CHANGE)
        .bind(origin)
        .bind(Uuid::new_v4())
        .bind(device)
        .bind(revision)
        .bind(action)
        .bind(proof)
        .fetch_one(&mut **tx)
        .await
        .unwrap()
}

// Leave the approved browser/device pair unchanged for the subsequent restart.
pub(super) async fn check(db: &PgPool, origin: &str, device: Uuid) {
    let mut tx = db.begin().await.unwrap();
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
        let result: String = sqlx::query_scalar(INVITE)
            .bind(origin)
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(result, if index < 16 { "applied" } else { "limit" });
        pending.push(id);
    }
    let proof = [31u8; 32];
    assert_eq!(
        change(&mut tx, origin, pending[0], 1, "revoke", None).await,
        "applied"
    );
    assert_eq!(
        change(&mut tx, origin, pending[0], 1, "approve", Some(&proof)).await,
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
        change(&mut tx, origin, pending[1], 1, "approve", Some(&proof)).await,
        "conflict"
    );
    sqlx::query(
        "UPDATE relay_hosted.admin_sessions SET created_at=created_at-interval '3 minutes'",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        change(&mut tx, origin, pending[2], 1, "approve", Some(&proof)).await,
        "fresh_required"
    );
    tx.rollback().await.unwrap();
}
