use super::*;

const SNAPSHOT_STAGES: [(&str, f64); 12] = [
    ("sources", 2.0),
    ("accounts", 3.0),
    ("quota_windows", 4.0),
    ("pricing_context", 5.0),
    ("source_summaries", 6.0),
    ("model_configuration", 7.0),
    ("catalog_projection", 8.0),
    ("routing", 9.0),
    ("gateway", 10.0),
    ("proxy", 11.0),
    ("capabilities", 12.0),
    ("snapshot", 13.0),
];

fn snapshot_samples() -> [PerformanceSample<'static>; 12] {
    SNAPSHOT_STAGES.map(|(stage, duration_ms)| PerformanceSample {
        name: "snapshot_stage",
        duration_ms,
        context: Some(stage),
    })
}

fn write_snapshot_stages(database: &TelemetryDb) {
    database
        .record_performance_batch(&snapshot_samples())
        .unwrap();
}

fn write_individual_stages(database: &TelemetryDb) {
    for (stage, duration_ms) in SNAPSHOT_STAGES {
        database
            .record_performance("snapshot_stage", duration_ms, Some(stage))
            .unwrap();
    }
}

fn stored_samples(database: &TelemetryDb) -> Vec<(String, f64, Option<String>)> {
    let connection = database.connection.lock().unwrap();
    let mut statement = connection
        .prepare("SELECT name, duration_ms, context FROM performance_samples ORDER BY id")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn with_database(test: impl FnOnce(&TelemetryDb)) {
    let root = crate::files::test_temp_root("relay-stage-samples");
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    test(&database);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn performance_sample_does_not_block_navigation_while_history_is_locked() {
    use std::{
        future::Future,
        sync::{Arc, mpsc},
        task::{Context, Waker},
        time::Duration,
    };

    let root = crate::files::test_temp_root("relay-navigation-metric");
    let database = Arc::new(TelemetryDb::open(&root.join("usage.sqlite")).unwrap());
    let (locked_tx, locked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let history = database.clone();
    let reader = std::thread::spawn(move || {
        let _connection = history.connection.lock().unwrap();
        locked_tx.send(()).unwrap();
        // A bounded watchdog lets a regression fail instead of hanging CI.
        let _ = release_rx.recv_timeout(Duration::from_secs(2));
    });
    locked_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut write =
        Box::pin(database.record_performance_async("page_open".into(), 12.5, Some("usage".into())));
    let first_poll = write.as_mut().poll(&mut Context::from_waker(Waker::noop()));
    let yielded = first_poll.is_pending();
    let _ = release_tx.send(());
    reader.join().unwrap();
    match first_poll {
        std::task::Poll::Ready(result) => result.unwrap(),
        std::task::Poll::Pending => write.as_mut().await.unwrap(),
    }
    drop(write);
    let samples = stored_samples(&database);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        yielded,
        "navigation telemetry must yield while a Usage query owns SQLite"
    );
    assert_eq!(samples, [("page_open".into(), 12.5, Some("usage".into()))]);
}

#[tokio::test]
async fn performance_sample_worker_preserves_validation_and_storage_errors() {
    let root = crate::files::test_temp_root("relay-metric-errors");
    let database = std::sync::Arc::new(TelemetryDb::open(&root.join("usage.sqlite")).unwrap());
    for (name, duration_ms, context) in [
        ("unknown_metric", 1.0, None),
        ("page_open", f64::NAN, None),
        ("page_open", -1.0, None),
        ("page_open", 600_001.0, None),
        ("page_open", 1.0, Some("invalid\ncontext")),
    ] {
        let error = database
            .record_performance_async(name.into(), duration_ms, context.map(str::to_owned))
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidState);
    }
    assert!(stored_samples(&database).is_empty());
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TEMP TRIGGER reject_metric BEFORE INSERT ON performance_samples
         BEGIN SELECT RAISE(ABORT, 'synthetic write failure'); END;",
        )
        .unwrap();
    let error = database
        .record_performance_async("page_open".into(), 1.0, Some("usage".into()))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Io);
    assert!(stored_samples(&database).is_empty());
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn performance_sample_batch_matches_individual_writes_and_survives_reopen() {
    let root = crate::files::test_temp_root("relay-stage-reopen");
    let batched = TelemetryDb::open(&root.join("batch.sqlite")).unwrap();
    let individual = TelemetryDb::open(&root.join("individual.sqlite")).unwrap();
    write_snapshot_stages(&batched);
    write_individual_stages(&individual);
    let expected = stored_samples(&individual);
    assert_eq!(expected.len(), SNAPSHOT_STAGES.len());
    assert_eq!(stored_samples(&batched), expected);
    drop(batched);
    let reopened = TelemetryDb::open(&root.join("batch.sqlite")).unwrap();
    assert_eq!(stored_samples(&reopened), expected);
    drop(reopened);
    drop(individual);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn performance_sample_batch_accepts_empty_single_and_boundary_values() {
    with_database(|database| {
        database.record_performance_batch(&[]).unwrap();
        assert!(stored_samples(database).is_empty());
        let samples = [
            PerformanceSample {
                name: "first_frame",
                duration_ms: 0.0,
                context: None,
            },
            PerformanceSample {
                name: "mode_switch",
                duration_ms: 600_000.0,
                context: Some("routing"),
            },
        ];
        database.record_performance_batch(&samples[..1]).unwrap();
        database.record_performance_batch(&samples[1..]).unwrap();
        database.record_performance_batch(&samples).unwrap();
        let expected = vec![
            ("first_frame".into(), 0.0, None),
            ("mode_switch".into(), 600_000.0, Some("routing".into())),
        ];
        let mut repeated = expected.clone();
        repeated.extend(expected);
        assert_eq!(stored_samples(database), repeated);
    });
}

#[test]
fn performance_sample_batch_validates_every_sample_before_writing() {
    with_database(|database| {
        database
            .record_performance("first_frame", 1.0, None)
            .unwrap();
        let previous = stored_samples(database);
        let valid = snapshot_samples()[0];
        let invalid = [
            PerformanceSample {
                name: "unknown_metric",
                ..valid
            },
            PerformanceSample {
                duration_ms: f64::NAN,
                ..valid
            },
            PerformanceSample {
                duration_ms: f64::INFINITY,
                ..valid
            },
            PerformanceSample {
                duration_ms: f64::NEG_INFINITY,
                ..valid
            },
            PerformanceSample {
                duration_ms: -1.0,
                ..valid
            },
            PerformanceSample {
                duration_ms: 600_000.1,
                ..valid
            },
            PerformanceSample {
                context: Some("invalid\ncontext"),
                ..valid
            },
            PerformanceSample {
                context: Some("не-ascii"),
                ..valid
            },
            PerformanceSample {
                context: Some("xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"),
                ..valid
            },
        ];
        for sample in invalid {
            let error = database
                .record_performance_batch(&[valid, sample])
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidState);
            assert_eq!(stored_samples(database), previous);
            assert_eq!(
                database
                    .record_performance(sample.name, sample.duration_ms, sample.context)
                    .unwrap_err()
                    .code,
                error.code,
            );
        }
    });
}

#[test]
fn performance_sample_batch_rolls_back_inserts_and_retention_on_sql_failure() {
    with_database(|database| {
        database
            .record_performance("first_frame", 1.0, Some("startup"))
            .unwrap();
        database
            .connection
            .lock()
            .unwrap()
            .execute_batch(
                "UPDATE performance_samples SET created_at = datetime('now', '-31 days');
             CREATE TEMP TRIGGER reject_snapshot_stage BEFORE INSERT ON performance_samples
             WHEN NEW.context = 'routing'
             BEGIN SELECT RAISE(ABORT, 'synthetic stage failure'); END;",
            )
            .unwrap();
        let previous = stored_samples(database);
        let error = database
            .record_performance_batch(&snapshot_samples())
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Io);
        assert_eq!(stored_samples(database), previous);
        database
            .connection
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_snapshot_stage;")
            .unwrap();
        write_snapshot_stages(database);
        let recorded = stored_samples(database);
        assert_eq!(recorded.len(), SNAPSHOT_STAGES.len());
        assert!(recorded.iter().all(|sample| sample.0 == "snapshot_stage"));
    });
}

#[test]
fn snapshot_total_and_stages_commit_or_roll_back_together() {
    with_database(|database| {
        let mut samples = snapshot_samples().to_vec();
        samples.push(PerformanceSample {
            name: "full_snapshot_native",
            duration_ms: 250.0,
            context: Some("local"),
        });
        database
            .connection
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TEMP TRIGGER reject_total BEFORE INSERT ON performance_samples
                 WHEN NEW.name = 'full_snapshot_native'
                 BEGIN SELECT RAISE(ABORT, 'synthetic total failure'); END;",
            )
            .unwrap();
        assert!(database.record_performance_batch(&samples).is_err());
        assert!(stored_samples(database).is_empty());
        database
            .connection
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_total;")
            .unwrap();
        database.record_performance_batch(&samples).unwrap();
        let recorded = stored_samples(database);
        assert_eq!(recorded.len(), samples.len());
        for (row, sample) in recorded.iter().zip(&samples) {
            assert_eq!(row.0, sample.name);
            assert_eq!(row.1, sample.duration_ms);
            assert_eq!(row.2.as_deref(), sample.context);
        }
    });
}

#[test]
fn performance_sample_batch_preserves_existing_retention_limit() {
    with_database(|database| {
        let samples = (0..2_200)
            .map(|index| PerformanceSample {
                name: "snapshot_stage",
                duration_ms: f64::from(index),
                context: Some("routing"),
            })
            .collect::<Vec<_>>();
        database.record_performance_batch(&samples).unwrap();
        let recorded = stored_samples(database);
        assert_eq!(recorded.len(), 2_048);
        assert_eq!(recorded.first().unwrap().1, 152.0);
        assert_eq!(recorded.last().unwrap().1, 2_199.0);
    });
}

#[test]
#[ignore = "observational SQLite timing probe; no latency threshold"]
fn measure_snapshot_stage_writes() {
    let root = crate::files::test_temp_root("relay-stage-writes");
    for (mode, write) in [
        ("individual", write_individual_stages as fn(&TelemetryDb)),
        ("batch", write_snapshot_stages as fn(&TelemetryDb)),
    ] {
        let database = TelemetryDb::open(&root.join(format!("{mode}.sqlite"))).unwrap();
        for _ in 0..5 {
            write(&database);
        }
        let mut elapsed_ms = (0..40)
            .map(|_| {
                let started = Instant::now();
                write(&database);
                started.elapsed().as_secs_f64() * 1_000.0
            })
            .collect::<Vec<_>>();
        elapsed_ms.sort_by(f64::total_cmp);
        eprintln!(
            "snapshot_stage_writes mode={mode} stages={} samples={} median_ms={:.4} p95_ms={:.4}",
            SNAPSHOT_STAGES.len(),
            elapsed_ms.len(),
            elapsed_ms[elapsed_ms.len() / 2],
            elapsed_ms[elapsed_ms.len() * 95 / 100],
        );
        let recorded: i64 = database
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM performance_samples WHERE name = 'snapshot_stage'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recorded, (5 + 40) * SNAPSHOT_STAGES.len() as i64);
        drop(database);
    }
    std::fs::remove_dir_all(root).unwrap();
}
