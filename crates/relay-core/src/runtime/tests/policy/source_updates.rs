use super::*;

fn policy(priority: i32) -> RuntimeCandidatePolicy {
    RuntimeCandidatePolicy {
        enabled: true,
        draining: false,
        priority,
        weight: 1,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
    }
}

#[test]
fn concurrent_source_edits_keep_routing_and_recovery_configuration_from_the_same_update() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "synthetic-key",
            &["model-a"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key(
            "key-1",
            "synthetic-local-key",
        ))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let barrier = std::sync::Barrier::new(3);
    const ROUNDS: usize = 128;
    let mut first_mismatch = None;
    std::thread::scope(|scope| {
        let workers = [1, 2].map(|priority| {
            let runtime = &runtime;
            let barrier = &barrier;
            scope.spawn(move || {
                let mut succeeded = true;
                for _ in 0..ROUNDS {
                    barrier.wait();
                    succeeded &= runtime.update_source_policy(
                        "source-1",
                        policy(priority),
                        u64::try_from(priority).unwrap(),
                    );
                    barrier.wait();
                }
                succeeded
            })
        });
        for _ in 0..ROUNDS {
            let delays = crate::poison::mutex(&runtime.source_state.recovery_delays_ms);
            barrier.wait();
            std::thread::sleep(std::time::Duration::from_millis(1));
            drop(delays);
            barrier.wait();
            let priority = runtime
                .lock_scheduler()
                .candidate("source-1")
                .unwrap()
                .priority;
            let recovery_delay = runtime.source_recovery_delay_ms("source-1");
            if recovery_delay != Some(u64::try_from(priority).unwrap() * 1_000) {
                first_mismatch.get_or_insert((priority, recovery_delay));
            }
        }
        for worker in workers {
            assert!(worker.join().unwrap());
        }
    });
    assert!(
        first_mismatch.is_none(),
        "a later routing policy adopted another update's recovery delay: {first_mismatch:?}"
    );
}

#[test]
fn rejected_source_batch_preserves_routing_and_recovery_configuration() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "synthetic-key",
            &["model-a"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key(
            "key-1",
            "synthetic-local-key",
        ))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    assert!(runtime.update_source_policy("source-1", policy(1), 1));
    assert!(!runtime.update_source_policies(&[
        RuntimeSourcePolicyUpdate {
            source_id: "source-1".into(),
            policy: policy(2),
            recovery_delay_seconds: 2
        },
        RuntimeSourcePolicyUpdate {
            source_id: "missing-source".into(),
            policy: policy(3),
            recovery_delay_seconds: 3
        },
    ]));
    assert_eq!(
        runtime
            .lock_scheduler()
            .candidate("source-1")
            .unwrap()
            .priority,
        1
    );
    assert_eq!(runtime.source_recovery_delay_ms("source-1"), Some(1_000));
}
