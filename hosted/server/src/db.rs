use crate::config::Config;
use sqlx::{
    ConnectOptions, PgExecutor, PgPool,
    migrate::Migrator,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::time::Duration;
use uuid::Uuid;

fn migrations() -> Migrator {
    let mut m = sqlx::migrate!("./migrations");
    m.create_schema("relay_hosted");
    m.dangerous_set_table_name("relay_hosted._sqlx_migrations");
    m
}

static EXPECTED: std::sync::LazyLock<(Vec<i64>, Vec<Vec<u8>>)> = std::sync::LazyLock::new(|| {
    migrations()
        .iter()
        .filter(|m| !m.migration_type.is_down_migration())
        .map(|m| (m.version, m.checksum.to_vec()))
        .unzip()
});

#[derive(Debug, PartialEq, Eq)]
pub struct UpgradePlan {
    pub installed: i64,
    pub target: i64,
    pub pending: Vec<i64>,
}

pub async fn upgrade_plan(pool: &PgPool, origin: &str) -> Result<UpgradePlan, &'static str> {
    const UNAVAILABLE: &str = "cannot read installation migration history";
    let mut tx = pool.begin().await.map_err(|_| UNAVAILABLE)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(|_| UNAVAILABLE)?;
    let matches: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT FROM relay_hosted.closed_installation WHERE public_origin=$1)",
    )
    .bind(origin)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| UNAVAILABLE)?;
    if !matches {
        return Err("incompatible installation or public origin");
    }
    let rows: Vec<(i64, bool, Vec<u8>)> = sqlx::query_as(
        "SELECT version,success,checksum FROM relay_hosted._sqlx_migrations ORDER BY version LIMIT $1",
    )
    .bind((EXPECTED.0.len() + 1) as i64)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| UNAVAILABLE)?;
    tx.commit().await.map_err(|_| UNAVAILABLE)?;

    let installed = rows.last().ok_or("migration history is empty")?.0;
    let target = *EXPECTED.0.last().ok_or("no embedded migrations")?;
    if installed > target {
        return Err("database schema is newer than this binary");
    }
    if rows.iter().any(|(_, success, _)| !success) {
        return Err("migration history contains a failed migration");
    }
    // Only a contiguous prefix of this binary's exact history is upgradeable.
    if rows.len() > EXPECTED.0.len()
        || rows
            .iter()
            .zip(EXPECTED.0.iter().zip(&EXPECTED.1))
            .any(|((v, _, digest), (expected, checksum))| v != expected || digest != checksum)
    {
        return Err("migration history does not match this binary");
    }
    Ok(UpgradePlan {
        installed,
        target,
        pending: EXPECTED.0[rows.len()..].to_vec(),
    })
}

pub async fn connect(c: &Config, dsn: &str) -> Result<PgPool, &'static str> {
    let db = &c.database;
    let options = dsn
        .parse::<PgConnectOptions>()
        .map_err(|_| "invalid database connection settings")?
        .disable_statement_logging()
        .options([
            ("search_path", "pg_catalog".to_owned()),
            ("lock_timeout", db.lock_timeout_ms.to_string()),
            ("statement_timeout", db.statement_timeout_ms.to_string()),
        ]);
    PgPoolOptions::new()
        .max_connections(db.max_connections)
        .acquire_timeout(Duration::from_millis(db.connect_timeout_ms))
        .connect_with(options)
        .await
        .map_err(|_| "database unavailable")
}

pub async fn migrate(pool: &PgPool, origin: &str) -> Result<(), &'static str> {
    {
        let mut connection = pool.acquire().await.map_err(|_| "database unavailable")?;
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT to_regclass('relay_hosted.installations') IS NOT NULL",
        )
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| "cannot inspect installation")?;
        if exists && !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM relay_hosted.installations WHERE singleton AND public_origin=$1)",
        ).bind(origin).fetch_one(&mut *connection).await.map_err(|_| "cannot inspect existing installation")? {
            return Err("installation origin mismatch");
        }
        sqlx::query("SELECT set_config('relay_hosted.initial_public_origin', $1, false)")
            .bind(origin)
            .execute(&mut *connection)
            .await
            .map_err(|_| "cannot initialize installation")?;
        migrations()
            .run(&mut *connection)
            .await
            .map_err(|_| "migration failed")?;
    }
    installation_schema(pool, origin).await.map(|_| ())
}

/// Requires the migration owner's credential and an offline, isolated restore.
/// An acknowledged operation can be retried without another epoch change.
pub async fn fence_restore(
    pool: &PgPool,
    origin: &str,
    installation: Uuid,
    operation: Uuid,
) -> Result<(), &'static str> {
    const FAILED: &str = "could not fence restored installation";
    if installation.is_nil() || operation.is_nil() {
        return Err("restore fence requires non-nil installation and operation IDs");
    }
    let mut tx = pool.begin().await.map_err(|_| FAILED)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .map_err(|_| FAILED)?;
    installation_schema(&mut *tx, origin).await?;
    let prior: Option<Uuid> = sqlx::query_scalar(
        "SELECT s.restore_operation
         FROM relay_hosted.service_control s JOIN relay_hosted.installations i ON i.id=s.installation_id
         WHERE i.singleton AND i.id=$1 AND i.public_origin=$2 FOR UPDATE OF s,i",
    )
    .bind(installation)
    .bind(origin)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| FAILED)?
    .ok_or("restore installation ID does not match")?;
    match prior {
        Some(prior) if prior != operation => {
            return Err("restore already fenced under another operation");
        }
        Some(_) => (),
        None => {
            sqlx::query(
                "UPDATE relay_hosted.service_control SET mode='maintenance',qualification_scope_ref=NULL,
                 revision=revision+1,admission_epoch=admission_epoch+1,restore_operation=$2,
                 changed_at=clock_timestamp() WHERE installation_id=$1",
            )
            .bind(installation)
            .bind(operation)
            .execute(&mut *tx)
            .await
            .map_err(|_| FAILED)?;
            sqlx::query("UPDATE relay_hosted.installations SET authorization_epoch=authorization_epoch+1 WHERE id=$1")
                .bind(installation)
                .execute(&mut *tx)
                .await
                .map_err(|_| FAILED)?;
        }
    }
    tx.commit().await.map_err(|_| FAILED)
}

pub async fn check_runtime(pool: &PgPool, origin: &str) -> Result<(), &'static str> {
    let privileged = sqlx::query_scalar::<_, bool>(
        "SELECT has_schema_privilege('relay_hosted','CREATE') OR has_database_privilege(current_database(),'CREATE') OR
            has_function_privilege('relay_hosted.bind_first_owner(uuid,bigint,text,uuid,bytea,jsonb,bigint)', 'EXECUTE') OR
            has_function_privilege('relay_hosted.post_verified_payment(uuid,text,text,text,bigint)', 'EXECUTE') OR
            has_function_privilege('relay_hosted.key_authority(text,uuid,bytea,bytea,boolean)', 'EXECUTE') OR
            has_function_privilege('relay_hosted.qualified_key_workspace(text,uuid,bytea)', 'EXECUTE') OR
            EXISTS (SELECT FROM pg_proc WHERE pronamespace='relay_hosted'::regnamespace
                AND proname IN ('connection_page','text_offer_config','reserve_text','claim_text','reserve_published_text','claim_published_text','key_text_models','record_text_check','text_check_step','text_check_scope','finish_text','due_text','key_text_status','close_text','lock_text_authority','record_request_journal',
                    'source_copy_access','finish_source_copy','source_copy_route','project_text_purchase','text_price_inputs',
                    'request_text_candidates','claim_text_candidate','record_text_cooldown','text_inventory_fenced','text_offer_supply',
                    'text_source_state','text_source_in_flight','text_cost_bound','fence_text_route','text_compatible_supply','text_basis_supply',
                    'text_final_value','value_text_purchase','record_late_text_usage','post_text_credit',
                    'capture_text_terminal','reconcile_text_attempt','capture_text_stream_usage','text_stream_check',
                    'usage_receipt','save_usage_report','usage_report_rows','usage_report_metrics','usage_report_keys',
                    'usage_report_utc','usage_report_payload','customer_catalog_listings','model_status_snapshot',
                    'claim_checkout_invoice','checkout_invoice_for_lookup','record_checkout_invoice','checkout_summary','local_checkout',
                    'register_checkout_profile','checkout_draft_methods','valid_checkout_draft','publish_checkout',
                    'claim_payment_refund','pending_refund_lookups','refund_for_lookup','observe_payment_refund',
                    'refund_lines','record_refund_journal','refund_summary','refundable_credit','platform_usage_overview',
                    'owner_usage_session','owner_request_issues','record_request_diagnostic',
                    'request_diagnostic_payload','request_diagnostic_matches','customer_request_diagnostic',
                    'capture_target','append_capture_milestone','lock_capture_owner','capture_stop_condition',
                    'close_diagnostic_capture','capture_millis','capture_summary',
                    'observe_operation','operation_conditions','observe_reconciliation')
                AND has_function_privilege(oid,'EXECUTE')) OR
            EXISTS (SELECT 1 FROM pg_proc WHERE pronamespace = 'relay_hosted'::regnamespace
                AND proname IN ('owner_login_context','owner_verifier','issue_admin_session','read_admin_overview','owner_usage_overview','end_admin_session',
                    'owner_request_page','owner_request_detail','prune_request_diagnostics',
                    'owner_operations_status','owner_operations_events',
                    'owner_diagnostic_captures','start_diagnostic_capture','stop_diagnostic_capture',
                    'sweep_diagnostic_capture','prune_diagnostic_captures','prune_capture_events','reset_owner_browser_sessions',
                    'customer_binding_matches','customer_login_context','finish_customer_login','customer_account','end_customer_session','owner_service_view','transition_customer_scope',
                    'read_customer_keys','issue_customer_key','revoke_customer_key','customer_history','owner_text_rules','change_text_rule','owner_text_price_inputs','customer_text_models','owner_connections',
                    'owner_devices','change_device','device_connections','review_source_copy','owner_purchase_limit','change_purchase_limit',
                    'start_usage_report','read_usage_report','prune_usage_reports','customer_model_status',
                    'owner_service_notices','change_service_notice','customer_service_notices','read_service_notices','prune_service_notices',
                    'customer_checkout_methods','quote_checkout','confirm_checkout','customer_checkout_history','prune_checkout_quotes',
                    'owner_checkout_builder','change_checkout_builder',
                    'customer_wallet','request_payment_refund','owner_refund_cases','review_payment_refund')
                AND NOT has_function_privilege(oid, 'EXECUTE')) OR
            EXISTS (SELECT 1 FROM pg_class WHERE relnamespace = 'relay_hosted'::regnamespace
            AND relkind IN ('r','v') AND (
                has_table_privilege(oid, 'DELETE, TRUNCATE, TRIGGER') OR
                has_any_column_privilege(oid, 'INSERT, UPDATE, REFERENCES') OR
                (relname NOT IN ('_sqlx_migrations','closed_installation') AND has_any_column_privilege(oid, 'SELECT'))
            ))",
    )
    .fetch_one(pool)
    .await
    .map_err(|_| "cannot verify runtime database role")?;
    if privileged {
        return Err("serve requires the scoped runtime database grants");
    }
    closed_setup(pool, origin).await
}

pub async fn prune_usage_reports(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT relay_hosted.prune_usage_reports()")
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn prune_service_notices(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT relay_hosted.prune_service_notices()")
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn prune_request_diagnostics(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT relay_hosted.prune_request_diagnostics()")
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn prune_checkout_quotes(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT relay_hosted.prune_checkout_quotes()")
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn prune_checkout_resume_checks(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT relay_hosted.prune_checkout_resume_checks()")
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn closed_setup(pool: &PgPool, origin: &str) -> Result<(), &'static str> {
    if installation_schema(pool, origin).await? {
        return Err("installation restore is fenced");
    }
    Ok(())
}

async fn installation_schema<'e>(
    executor: impl PgExecutor<'e>,
    origin: &str,
) -> Result<bool, &'static str> {
    sqlx::query_scalar(
        "SELECT i.restore_pending FROM relay_hosted.closed_installation i WHERE i.public_origin=$1
            AND NOT EXISTS (
                SELECT 1 FROM relay_hosted._sqlx_migrations m
                FULL JOIN unnest($2::bigint[], $3::bytea[]) e(version, checksum) USING (version)
                WHERE e.version IS NULL OR m.version IS NULL OR NOT m.success OR m.checksum <> e.checksum
            )",
    )
    .bind(origin)
    .bind(&EXPECTED.0)
    .bind(&EXPECTED.1)
    .fetch_optional(executor)
    .await
    .map_err(|_| "installation unavailable")?
    .ok_or("incompatible installation or public origin")
}
