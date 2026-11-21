use super::checkpoint;
use crate::support::Process;
use sqlx::PgPool;
use std::{fs, path::Path};
use uuid::Uuid;
use zenith_relay_hosted::db;

pub(super) async fn interrupt_publication(
    admin: &PgPool,
    runtime: &PgPool,
    origin: &str,
    browser: &mut Process,
    exchange: &Path,
) -> Uuid {
    let file = exchange.join("notice-draft");
    checkpoint(browser, &file).await;
    let id: Uuid = fs::read_to_string(file).unwrap().parse().unwrap();
    let proof: Vec<u8> = sqlx::query_scalar(
        "SELECT verifier FROM relay_hosted.admin_sessions ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(admin)
    .await
    .unwrap();
    let current = || {
        sqlx::query_as::<_, (i32, Option<i16>, i64)>(
            "SELECT revision,recipient_count,
              (SELECT count(*) FROM relay_hosted.service_notice_inbox WHERE notice_id=$1)
             FROM relay_hosted.service_notices WHERE id=$1",
        )
        .bind(id)
        .fetch_one(admin)
    };
    assert_eq!(current().await.unwrap(), (2, None, 0));
    let aborted = Uuid::new_v4();
    let mut transaction = runtime.begin().await.unwrap();
    // Writing both the broadcast and its acknowledgement just before losing the
    // transaction must leave neither result visible on a different connection.
    let (key, revision, counter): (Vec<u8>, i64, i64) = sqlx::query_as(
        "SELECT k.credential_id,k.revision,(k.passkey #>> '{cred,counter}')::bigint
         FROM relay_hosted.admin_credentials k JOIN relay_hosted.admin_sessions s
         ON (s.credential_id,s.user_id)=(k.credential_id,k.user_id) WHERE s.verifier=$1",
    )
    .bind(&proof)
    .fetch_one(admin)
    .await
    .unwrap();
    let result: String = sqlx::query_scalar(
        "SELECT relay_hosted.change_service_notice($1,$2,$3,$4,2,'publish',NULL,NULL,$5,$6,$7,false,false,
            floor(extract(epoch FROM clock_timestamp())*1000)::bigint+30000)",
    )
    .bind(origin)
    .bind(proof)
    .bind(id)
    .bind(aborted)
    .bind(key)
    .bind(revision)
    .bind(counter + 1)
    .fetch_one(&mut *transaction)
    .await
    .unwrap();
    assert_eq!(result, "applied");
    assert_eq!(current().await.unwrap(), (2, None, 0));
    transaction.rollback().await.unwrap();
    let leaked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT FROM relay_hosted.service_notice_operations WHERE operation_id=$1)",
    )
    .bind(aborted)
    .fetch_one(admin)
    .await
    .unwrap();
    assert!(!leaked);
    assert_eq!(current().await.unwrap(), (2, None, 0));
    fs::write(exchange.join("notice-confirm"), []).unwrap();
    id
}

pub(super) async fn expire_private_notice(
    admin: &PgPool,
    runtime: &PgPool,
    browser: &mut Process,
    exchange: &Path,
    id: Uuid,
) {
    let delivered: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM relay_hosted.service_notice_inbox),
            (SELECT count(*) FROM relay_hosted.service_notice_inbox WHERE read_at IS NOT NULL),
            (SELECT count(*) FROM relay_hosted.service_notice_operations WHERE action='publish'),
            (SELECT count(*) FROM relay_hosted.service_notice_operations WHERE confirmed_by_passkey_id IS NOT NULL)",
    )
    .fetch_one(admin)
    .await
    .unwrap();
    assert_eq!(delivered, (3, 2, 2, 2));
    sqlx::query(
        "UPDATE relay_hosted.service_notices SET published_at=now()-interval '31 days',
            updated_at=now()-interval '31 days',expires_at=now()-interval '1 day' WHERE id=$1",
    )
    .bind(id)
    .execute(admin)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE relay_hosted.service_notice_operations SET applied_at=now()-interval '31 days' WHERE notice_id=$1",
    )
    .bind(id)
    .execute(admin)
    .await
    .unwrap();
    fs::write(exchange.join("notice-expired"), []).unwrap();
    checkpoint(browser, &exchange.join("notice-expiry-seen")).await;
    db::prune_service_notices(runtime).await.unwrap();
    let scoped_counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM relay_hosted.service_notices WHERE id=$1),
            (SELECT count(*) FROM relay_hosted.service_notice_inbox WHERE notice_id=$1),
            (SELECT count(*) FROM relay_hosted.service_notice_operations WHERE notice_id=$1)",
    )
    .bind(id)
    .fetch_one(admin)
    .await
    .unwrap();
    assert_eq!(scoped_counts, (0, 0, 0));
}
