use crate::{db, inference};
use sqlx::{PgPool, Postgres, postgres::PgArguments, query::Query};
use std::time::Duration;

#[derive(Default)]
struct Sink {
    missed: bool,
}

impl Sink {
    async fn record(&mut self, pool: &PgPool, query: Query<'_, Postgres, PgArguments>) {
        let result = tokio::time::timeout(
            Duration::from_millis(250),
            query.bind(self.missed).execute(pool),
        )
        .await;
        let missed = !matches!(result, Ok(Ok(_)));
        if missed && !self.missed {
            eprintln!("relay-hosted: background job observations unavailable");
        }
        self.missed = missed;
    }
}

#[derive(Default)]
pub struct Reconciliation(Sink);

impl Reconciliation {
    pub async fn run(&mut self, pool: &PgPool) -> Result<usize, sqlx::Error> {
        let result = inference::reconcile_due(pool).await;
        let observation =
            sqlx::query("SELECT relay_hosted.observe_reconciliation($1,$2)").bind(result.is_ok());
        self.0.record(pool, observation).await;
        result
    }
}

pub async fn prune_projections(pool: &PgPool) -> ! {
    let mut failed = [false; 7];
    let mut sink = Sink::default();
    loop {
        tokio::time::sleep(Duration::from_secs(15)).await;
        let (reports, notices, quotes, resumes, diagnostics, completions, observations) = tokio::join!(
            db::prune_usage_reports(pool),
            db::prune_service_notices(pool),
            db::prune_checkout_quotes(pool),
            db::prune_checkout_resume_checks(pool),
            prune_diagnostics(pool),
            sqlx::query("SELECT relay_hosted.prune_model_status_completions()").execute(pool),
            sqlx::query("SELECT relay_hosted.prune_model_status_observations()").execute(pool),
        );
        for ((name, result), was_failed) in [
            ("usage report", reports),
            ("notification", notices),
            ("checkout quote", quotes),
            ("checkout resume", resumes),
            ("request diagnostics", diagnostics),
            ("model status timing", completions.map(|_| ())),
            ("model availability", observations.map(|_| ())),
        ]
        .into_iter()
        .zip(&mut failed)
        {
            let next = result.is_err();
            if next && !*was_failed {
                eprintln!("relay-hosted: {name} cleanup unavailable");
            }
            *was_failed = next;
        }
        let observation =
            sqlx::query("SELECT relay_hosted.observe_projection_cleanup($1,$2,$3,$4,$5,$6)")
                .bind(!failed[0])
                .bind(!failed[1])
                .bind(!failed[2])
                .bind(!failed[3])
                .bind(!failed[4]);
        sink.record(pool, observation).await;
    }
}

async fn prune_diagnostics(pool: &PgPool) -> Result<(), sqlx::Error> {
    let (normal, sweep, events) = tokio::join!(
        db::prune_request_diagnostics(pool),
        sqlx::query("SELECT relay_hosted.sweep_diagnostic_capture()").execute(pool),
        sqlx::query("SELECT relay_hosted.prune_capture_events()").execute(pool),
    );
    // Disarm commits independently, even when the closure audit cannot be written.
    let audit = sqlx::query("SELECT relay_hosted.prune_diagnostic_captures()")
        .execute(pool)
        .await;
    normal.and(sweep).and(events).and(audit).map(|_| ())
}
