use sqlx::PgPool;
use std::time::Duration;

pub(super) async fn subscribed(db: &PgPool, role: &str) {
    // A poll begun after this point sees the already committed start.
    let after: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
            .fetch_one(db)
            .await
            .unwrap();
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            let seen: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT FROM pg_stat_activity WHERE usename=$1 AND state='idle'
                    AND query LIKE 'SELECT * FROM relay_hosted.capture_target(%'
                    AND floor(extract(epoch FROM query_start)*1000)::bigint>$2)",
            )
            .bind(role)
            .bind(after)
            .fetch_one(db)
            .await
            .unwrap();
            if seen {
                break;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("collector did not poll the committed capture");
}
