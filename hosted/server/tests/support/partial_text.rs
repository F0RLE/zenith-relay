use sqlx::PgPool;
use uuid::Uuid;

pub struct PartialText {
    pub key: Uuid,
    pub offer: Uuid,
    pub item: Uuid,
    pub rate_revision: i64,
    pub claimed_at: String,
    pub input_bound: i32,
    pub output_bound: i32,
    pub observed_output: i64,
}

pub struct Attempt {
    pub offer: Uuid,
    pub request: Uuid,
    pub id: Uuid,
}

impl PartialText {
    /// Seed the cut point after a partial observation but before the close worker ran.
    pub async fn seed(self, db: &PgPool) -> Attempt {
        let mut tx = db.begin().await.unwrap();
        let (request, id): (Uuid, Uuid) = sqlx::query_as(
            "WITH sized AS (
                SELECT k.workspace_id,k.id,k.verifier,s.admission_epoch,$3::text::timestamptz AS stamp,
                    relay_hosted.text_cost_bound($4,$5,o.input_rate,o.cached_rate,o.output_rate) AS bound
                FROM relay_hosted.customer_keys k JOIN relay_hosted.text_offers o ON o.scope_id=k.scope_id
                JOIN relay_hosted.service_control s ON s.qualification_scope_ref=o.scope_id
                WHERE k.id=$1 AND o.id=$2
             ), held AS (
                UPDATE relay_hosted.wallets w SET available_micro_usd=w.available_micro_usd-p.bound,
                    reserved_micro_usd=w.reserved_micro_usd+p.bound,revision=w.revision+1
                FROM sized p WHERE w.workspace_id=p.workspace_id RETURNING w.workspace_id
             ) INSERT INTO relay_hosted.text_requests(workspace_id,key_id,offer_id,authority_epoch,input_tag,send_tag,
                 input_bound,output_bound,held,started_at,claim_by,close_by,claimed_at,execution,accounting,output_tokens)
             SELECT p.workspace_id,p.id,$2,p.admission_epoch,p.verifier,p.verifier,$4,$5,p.bound,p.stamp-interval '1 second',
                 p.stamp+interval '29 seconds',p.stamp+interval '64 seconds',p.stamp,'incomplete','pending',$6
             FROM sized p JOIN held h USING(workspace_id) RETURNING id,attempt_id",
        )
        .bind(self.key)
        .bind(self.offer)
        .bind(&self.claimed_at)
        .bind(self.input_bound)
        .bind(self.output_bound)
        .bind(self.observed_output)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "SELECT relay_hosted.record_request_journal(r,'request_hold') FROM relay_hosted.text_requests r WHERE r.id=$1",
        )
        .bind(request)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO relay_hosted.text_purchase_attempts(attempt_id,item,rate_revision,month_utc,bound_micro_usd)
             SELECT r.attempt_id,p.item,p.revision,date_trunc('month',r.claimed_at AT TIME ZONE 'UTC')::date,
                relay_hosted.text_cost_bound(r.input_bound,r.output_bound,p.input_rate,p.cached_rate,p.output_rate)
             FROM relay_hosted.text_requests r CROSS JOIN relay_hosted.text_purchase_rates p
             WHERE r.id=$1 AND (p.item,p.revision)=($2,$3)",
        )
        .bind(request)
        .bind(self.item)
        .bind(self.rate_revision)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        Attempt {
            offer: self.offer,
            request,
            id,
        }
    }
}

impl Attempt {
    pub async fn capture(&self, db: &PgPool, outcome: &str, counts: [Option<i64>; 3]) -> bool {
        sqlx::query_scalar("SELECT relay_hosted.capture_text_terminal($1,$2,$3,$4,$5,$6,$7)")
            .bind(self.offer)
            .bind(self.request)
            .bind(self.id)
            .bind(outcome)
            .bind(counts[0])
            .bind(counts[1])
            .bind(counts[2])
            .fetch_one(db)
            .await
            .unwrap()
    }

    pub async fn reconcile(&self, db: &PgPool, action: &str) -> String {
        sqlx::query_scalar("SELECT relay_hosted.reconcile_text_attempt($1,$2,$3,$4)")
            .bind(self.offer)
            .bind(self.request)
            .bind(self.id)
            .bind(action)
            .fetch_one(db)
            .await
            .unwrap()
    }

    /// Exercise supplemental usage and migrations without pretending it came from the one JSON reply.
    pub async fn finish(&self, db: &PgPool, outcome: &str, counts: [Option<i64>; 3]) -> String {
        sqlx::query_scalar("SELECT relay_hosted.finish_text($1,$2,$3,$4,$5,$6,$7)")
            .bind(self.offer)
            .bind(self.request)
            .bind(self.id)
            .bind(outcome)
            .bind(counts[0])
            .bind(counts[1])
            .bind(counts[2])
            .fetch_one(db)
            .await
            .unwrap()
    }
}
