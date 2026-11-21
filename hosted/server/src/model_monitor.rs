mod prometheus;

use crate::{config, db};
use serde::Deserialize;
use sqlx::{PgPool, types::Json};
use std::{collections::HashSet, net::SocketAddr, time::Duration};
use uuid::Uuid;

const CAPACITY: usize = 64;
const SLOT_SECONDS: i64 = 1800;

#[derive(Default)]
pub struct Summary {
    pub accepted: usize,
    pub unobserved: usize,
    pub ineligible: usize,
    pub conflicts: usize,
}

#[derive(Deserialize)]
struct Targets {
    items: Vec<Target>,
}

#[derive(Deserialize)]
struct Target {
    publication_id: Uuid,
    published_at_ms: i64,
}

pub async fn collect(c: &config::Config, address: SocketAddr) -> Result<Summary, &'static str> {
    let transport = prometheus::Transport::new(address)?;
    let dsn = config::role_url("ZENITH_RELAY_HOSTED_MODEL_MONITOR_DATABASE_URL_FILE")?;
    tokio::time::timeout(Duration::from_secs(30), async {
        let pool = db::connect(c, &dsn).await?;
        let summary = collect_slots(&pool, &c.server.public_origin, &transport).await;
        pool.close().await;
        summary
    })
    .await
    .map_err(|_| "model status collection timed out")?
}

async fn collect_slots(
    pool: &PgPool,
    origin: &str,
    transport: &prometheus::Transport,
) -> Result<Summary, &'static str> {
    db::closed_setup(pool, origin).await?;
    check_role(pool).await?;
    let (now, Json(targets)): (i64, Json<Targets>) = sqlx::query_as(
        "SELECT floor(extract(epoch FROM statement_timestamp()))::bigint,
            relay_hosted.model_monitor_targets($1)",
    )
    .bind(origin)
    .fetch_one(pool)
    .await
    .map_err(|_| "model monitor targets unavailable or over capacity")?;
    let mut unique = HashSet::new();
    if targets.items.len() > CAPACITY
        || targets.items.iter().any(|t| {
            t.publication_id.is_nil() || t.published_at_ms < 0 || !unique.insert(t.publication_id)
        })
    {
        return Err("invalid model monitor targets");
    }

    let mut summary = Summary::default();
    let boundary = now.div_euclid(SLOT_SECONDS) * SLOT_SECONDS;
    for slot in [boundary - 2 * SLOT_SECONDS, boundary - SLOT_SECONDS] {
        // The write window covers only the two most recent completed slots.
        let ids: Vec<_> = targets
            .items
            .iter()
            .filter(|t| t.published_at_ms <= slot * 1000)
            .map(|t| t.publication_id)
            .collect();
        if ids.is_empty() {
            continue;
        }
        let states = transport.read_slot(slot, &ids).await?;
        for (id, state) in ids.into_iter().zip(states) {
            let Some(state) = state else {
                summary.unobserved += 1;
                continue;
            };
            let outcome: String = sqlx::query_scalar(
                "SELECT relay_hosted.attest_model_status($1,$2,to_timestamp($3::bigint::double precision),$4)",
            )
            .bind(origin)
            .bind(id)
            .bind(slot)
            .bind(state.as_str())
            .fetch_one(pool)
            .await
            .map_err(|_| "could not record model status")?;
            match outcome.as_str() {
                "recorded" => summary.accepted += 1,
                "inactive" | "invalid" => summary.ineligible += 1,
                "conflict" => summary.conflicts += 1,
                _ => return Err("unexpected model status result"),
            }
        }
    }
    Ok(summary)
}

async fn check_role(pool: &PgPool) -> Result<(), &'static str> {
    let permitted: bool = sqlx::query_scalar(
        "WITH allowed(id) AS (VALUES ('relay_hosted.model_monitor_targets(text)'::regprocedure),
            ('relay_hosted.attest_model_status(text,uuid,timestamptz,text)'::regprocedure))
         SELECT EXISTS(SELECT FROM pg_roles WHERE rolname=current_user AND rolcanlogin
             AND NOT (rolsuper OR rolcreaterole OR rolcreatedb OR rolreplication OR rolbypassrls))
         AND NOT EXISTS(SELECT FROM pg_roles WHERE rolname<>current_user AND pg_has_role(oid,'MEMBER'))
         AND NOT has_schema_privilege('relay_hosted','CREATE')
         AND NOT has_database_privilege(current_database(),'CREATE')
         AND NOT EXISTS(SELECT FROM allowed WHERE NOT has_function_privilege(id,'EXECUTE'))
         AND NOT EXISTS(SELECT FROM pg_proc WHERE pronamespace='relay_hosted'::regnamespace
             AND has_function_privilege(oid,'EXECUTE') AND oid NOT IN (SELECT id FROM allowed))
         AND NOT EXISTS(SELECT FROM pg_class WHERE relnamespace='relay_hosted'::regnamespace
             AND relkind IN ('r','v','p','m','f','S') AND
             (CASE WHEN relkind='S' THEN has_sequence_privilege(oid,'USAGE, SELECT, UPDATE')
              ELSE has_table_privilege(oid,'DELETE, TRUNCATE, TRIGGER')
                OR has_any_column_privilege(oid,'INSERT, UPDATE, REFERENCES')
                OR (relname NOT IN ('closed_installation','_sqlx_migrations')
                    AND has_any_column_privilege(oid,'SELECT')) END))",
    )
    .fetch_one(pool)
    .await
    .map_err(|_| "cannot verify model monitor database role")?;
    if !permitted {
        return Err("a separate restricted model monitor role is required");
    }
    Ok(())
}
