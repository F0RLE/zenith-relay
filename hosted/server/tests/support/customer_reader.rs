use super::*;

pub(super) struct Reader {
    pub pool: PgPool,
    pub provider: Uuid,
    pub tag: [u8; 32],
}

impl Reader {
    pub(super) async fn new(f: &Fixture, space: Uuid, n: u8) -> Self {
        let tag = [n; 32];
        let provider = sqlx::query_scalar(
            "INSERT INTO relay_hosted.customer_sessions
             SELECT $2,q.scope_id,e.provider_id,e.subject_digest,e.user_id,q.workspace_id,i.authorization_epoch,u.authorization_epoch,
                now(),now(),now()+interval '30 minutes'
             FROM relay_hosted.qualification_subjects q JOIN relay_hosted.configuration_revisions c ON c.id=q.scope_id
             JOIN relay_hosted.external_identities e ON (e.provider_id,e.subject_digest)=(c.provider_id,q.subject_digest)
             JOIN relay_hosted.users u ON u.id=e.user_id CROSS JOIN relay_hosted.installations i WHERE q.workspace_id=$1
             RETURNING provider_id",
        ).bind(space).bind(&tag[..]).fetch_one(&f.admin).await.unwrap();
        let pool = db::connect(&fixture_config(), &f._cluster.dsn("report_reader"))
            .await
            .unwrap();
        Self {
            pool,
            provider,
            tag,
        }
    }

    pub(super) async fn current_receipt(&self, id: Uuid) -> Value {
        sqlx::query_scalar(
            "SELECT relay_hosted.read_usage_report($1,$2,$3,$4,NULL,NULL,NULL,$5,'page')",
        )
        .bind(ORIGIN)
        .bind(self.provider)
        .bind(&[0x11_u8; 32][..])
        .bind(self.tag.as_slice())
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
}

pub(super) async fn setup_reader(f: &Fixture) {
    f._cluster.create_logins(&f.admin, &["report_reader"]).await;
    sqlx::raw_sql(
        "GRANT USAGE ON SCHEMA relay_hosted TO report_reader;
         GRANT SELECT ON relay_hosted.closed_installation,relay_hosted._sqlx_migrations TO report_reader;",
    ).execute(&f.admin).await.unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(
        include_str!("../../runtime-grants.sql").replace(":\"runtime_role\"", "report_reader"),
    ))
    .execute(&f.admin)
    .await
    .unwrap();
}

pub(super) async fn add_workspace(f: &Fixture) -> (Uuid, Uuid) {
    let space: Uuid = sqlx::query_scalar(
        "WITH customer AS (INSERT INTO relay_hosted.users(id) VALUES(gen_random_uuid()) RETURNING id),
         workspace AS (INSERT INTO relay_hosted.workspaces(owner_user_id) SELECT id FROM customer RETURNING *),
         identity AS (INSERT INTO relay_hosted.external_identities(provider_id,subject_digest,user_id)
             SELECT c.provider_id,decode(repeat('23',32),'hex'),w.owner_user_id FROM relay_hosted.configuration_revisions c,workspace w
             WHERE c.id=$1 RETURNING *)
         INSERT INTO relay_hosted.qualification_subjects(scope_id,subject_digest,workspace_id)
             SELECT $1,i.subject_digest,w.id FROM identity i JOIN workspace w ON w.owner_user_id=i.user_id RETURNING workspace_id",
    ).bind(f.scope).fetch_one(&f.admin).await.unwrap();
    (space, issue(&f.admin, f.scope, space).await.0)
}
