use super::*;
use crate::local_pool::store::telemetry_db::PerformanceSample;
use std::sync::Arc;
use zenith_relay_core::{
    GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeLocalKey,
    RuntimeSource, WireApi,
};

pub(super) fn print_stages(stages: &[PerformanceSample<'_>]) {
    if std::env::var_os("RELAY_MEASURE_SNAPSHOT_STAGES").is_some() {
        for sample in stages {
            println!(
                "{} context={} elapsed_ms={:.2}",
                sample.name,
                sample.context.unwrap_or_default(),
                sample.duration_ms
            );
        }
    }
}

#[tokio::test]
#[ignore = "synthetic full snapshot timing; no provider requests or real credentials"]
async fn measure_full_snapshot_projection() {
    let root = crate::files::test_temp_root("snapshot-timing");
    let state = DesktopState::open(root.clone()).unwrap();
    let sources = (0..3)
        .map(|index| {
            let mut source = crate::local_pool::models::synthetic_responses_source();
            source.id = format!("synthetic-{index}");
            source.secret_ref = format!("source:snapshot-test-{}-{index}", uuid::Uuid::new_v4());
            source.models = (0..200)
                .map(|model| format!("gpt-synthetic-{index}-{model}"))
                .collect();
            source
        })
        .collect::<Vec<_>>();
    let runtime = Arc::new(
        GatewayRuntime::from_pool(
            sources
                .iter()
                .map(|source| {
                    RuntimeSource::unrestricted(ProviderSource {
                        id: source.id.clone(),
                        name: "Synthetic snapshot source".into(),
                        base_url: "http://127.0.0.1:9/v1".into(),
                        api_key: "synthetic-upstream".into(),
                        wire_api: WireApi::Responses,
                        models: source.models.clone(),
                        proxy_url: None,
                    })
                })
                .collect(),
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: crate::local_pool::commands::pool::SYSTEM_GATEWAY_KEY_ID.into(),
                secret: "synthetic-local".into(),
            })],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap(),
    );
    state
        .store()
        .unwrap()
        .replace_records(sources, vec![])
        .unwrap();
    // A separate ephemeral loopback listener exercises routing projection too;
    // the fixture never sends requests or touches the user's running gateway.
    state.gateway.start(runtime, 0).await.unwrap();
    for phase in ["cold", "warm_1", "warm_2", "warm_3"] {
        let started = Instant::now();
        let snapshot = build_local_runtime_state(&state).await.unwrap();
        assert_eq!(snapshot.sources.len(), 3);
        assert!(snapshot.gateway.running);
        assert_eq!(snapshot.gateway.routing_order.len(), 3);
        assert_eq!(
            snapshot
                .sources
                .iter()
                .map(|source| source.models.len())
                .sum::<usize>(),
            600
        );
        println!(
            "snapshot_projection phase={phase} elapsed_ms={:.2}",
            started.elapsed().as_secs_f64() * 1_000.0
        );
    }
    state.gateway.stop().await;
    remove_measurement_root(state, root).await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "synthetic store-lock timing; no provider requests or real credentials"]
async fn measure_snapshot_store_lock_wait() {
    use std::{sync::mpsc, time::Duration};

    let root = crate::files::test_temp_root("snapshot-store-lock-timing");
    let state = DesktopState::open(root.clone()).unwrap();
    let started = Instant::now();
    let baseline = state.snapshot_inputs().await.unwrap();
    let baseline_elapsed = started.elapsed();
    assert!(baseline.store_lock_wait <= baseline_elapsed);
    println!(
        "snapshot_store_lock phase=uncontended elapsed_ms={:.3} lock_wait_ms={:.3}",
        baseline_elapsed.as_secs_f64() * 1_000.0,
        baseline.store_lock_wait.as_secs_f64() * 1_000.0,
    );

    let (locked_tx, locked_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let writer_state = state.clone();
    let writer = std::thread::spawn(move || {
        let _store = writer_state.store().unwrap();
        locked_tx.send(()).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
    });
    locked_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let started = Instant::now();
    started_tx.send(()).unwrap();
    let contended = state.snapshot_inputs().await.unwrap();
    let elapsed = started.elapsed();
    writer.join().unwrap();
    println!(
        "snapshot_store_lock phase=contended elapsed_ms={:.3} lock_wait_ms={:.3}",
        elapsed.as_secs_f64() * 1_000.0,
        contended.store_lock_wait.as_secs_f64() * 1_000.0,
    );
    assert!(contended.store_lock_wait >= Duration::from_millis(50));
    assert!(contended.store_lock_wait <= elapsed);
    remove_measurement_root(state, root).await;
}

async fn remove_measurement_root(state: DesktopState, root: std::path::PathBuf) {
    drop(state);
    // Optional metric jobs may briefly own SQLite after the state is dropped.
    for attempt in 0..20 {
        match std::fs::remove_dir_all(&root) {
            Ok(()) => return,
            Err(_) if attempt < 19 => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await
            }
            Err(error) => panic!("temporary snapshot cleanup failed: {error}"),
        }
    }
}
