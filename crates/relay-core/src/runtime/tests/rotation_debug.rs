use super::*;
use crate::scheduler::rotation::{RotationOperation, SharedRequestBudget};
use crate::{RotationDebugCapture, RotationDebugSink};
use std::sync::atomic::AtomicBool;

#[derive(Default)]
struct Sink {
    enabled: AtomicBool,
    captures: Mutex<Vec<RotationDebugCapture>>,
    scheduler: Mutex<Option<std::sync::Weak<Mutex<PoolScheduler>>>>,
}

impl RotationDebugSink for Sink {
    fn epoch(&self) -> Option<u64> {
        self.enabled.load(Ordering::Acquire).then_some(1)
    }
    fn submit(&self, epoch: u64, capture: RotationDebugCapture) {
        assert_eq!(epoch, 1);
        let scheduler = self
            .scheduler
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .upgrade()
            .unwrap();
        assert!(
            scheduler.try_lock().is_ok(),
            "diagnostics run outside the live scheduler lock"
        );
        self.captures.lock().unwrap().push(capture);
    }
}

#[test]
fn rotation_debug_toggle_captures_exact_reservations_and_empty_selection_without_dispatch() {
    let sink = Arc::new(Sink::default());
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-a",
            "synthetic-secret",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions {
            rotation_debug: Some(sink.clone()),
            ..Default::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    *sink.scheduler.lock().unwrap() = Some(Arc::downgrade(&runtime.scheduler));
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let request = super::super::admission::AdmissionRequest {
        key: authenticated,
        model: "gpt-test".into(),
        protocols: vec![WireApi::Responses],
        tried: HashSet::new(),
        response_affinity: None,
        prompt_affinity: None,
        operation: RotationOperation::Text,
        budget: SharedRequestBudget::for_incoming_request(runtime.request_dispatch_budget()),
    };
    drop(runtime.try_reserve_admission(&request, 100).0.unwrap());
    assert!(sink.captures.lock().unwrap().is_empty());

    sink.enabled.store(true, Ordering::Release);
    let (selected, lease) = runtime.try_reserve_admission(&request, 100).0.unwrap();
    let snapshot = sink.captures.lock().unwrap().pop().unwrap();
    assert!(snapshot.reserved);
    assert_eq!(snapshot.request_id(), request.budget.request_id().0);
    let trace = snapshot.into_trace();
    assert_eq!(
        trace.selected_candidate_id.as_deref(),
        Some(selected.candidate_id.as_str())
    );
    assert_eq!(
        trace.candidates[0].in_flight, 0,
        "snapshot precedes reservation"
    );
    drop(lease);

    let mut excluded = request.clone();
    excluded.tried.insert("source-a".into());
    assert!(runtime.try_reserve_admission(&excluded, 101).0.is_none());
    let snapshot = sink.captures.lock().unwrap().pop().unwrap();
    assert!(!snapshot.reserved);
    let trace = snapshot.into_trace();
    assert!(trace.selected_candidate_id.is_none());
    assert_eq!(trace.candidates.len(), 1);

    sink.enabled.store(false, Ordering::Release);
    drop(runtime.try_reserve_admission(&request, 102).0.unwrap());
    assert!(sink.captures.lock().unwrap().is_empty());
}
