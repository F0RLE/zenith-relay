use super::*;
use crate::NativeResponsesReplayStore;
use std::{hint::black_box, time::Instant};

fn measure_replay_operation(label: &str, bytes: usize, mut operation: impl FnMut(bool)) {
    const ITERATIONS: u32 = 100;
    for _ in 0..5 {
        operation(false);
        operation(true);
    }
    let mut samples = [Vec::new(), Vec::new()];
    for trial in 0..7 {
        // Alternate order within one process to reduce allocator/CPU drift
        // between separate before/after builds. Keep setup outside the timer.
        for variant in [trial % 2, 1 - trial % 2] {
            let started = Instant::now();
            for _ in 0..ITERATIONS {
                operation(variant == 1);
            }
            samples[variant].push(started.elapsed().as_nanos() / u128::from(ITERATIONS));
        }
    }
    for (variant, mut samples) in samples.into_iter().enumerate() {
        samples.sort_unstable();
        let variant = if variant == 0 { "shared" } else { "reference" };
        eprintln!(
            "native_replay operation={label} variant={variant} bytes={bytes} warmup=5 trials=7 iterations={ITERATIONS} median_ns={} min_ns={} max_ns={}",
            samples[3], samples[0], samples[6]
        );
    }
}

// Reference the original clone-on-lookup work without changing the live store.
// The reference also pays for this wrapper's Arc allocation; production lookups
// do not. Before/after runs of the actual implementation remain the baseline.
fn deep_copy(state: NativeResponsesReplayState) -> NativeResponsesReplayState {
    NativeResponsesReplayState {
        history: Arc::new(NativeResponsesReplayHistory {
            model: state.history.model.clone(),
            initial_input: state.history.initial_input.clone(),
            completed_output: state.history.completed_output.clone(),
        }),
    }
}

#[test]
fn native_replay_clone_shares_immutable_history() {
    let (_, state) = NativeResponsesReplayState::from_response(
        &json!({"input":"Synthetic history"}),
        "synthetic-model",
        &json!({"id":"synthetic-response","output":[]}),
    )
    .unwrap();
    let cloned = state.clone();
    assert!(Arc::ptr_eq(&state.history, &cloned.history));
    assert_eq!(deep_copy(cloned), state);
}

#[test]
#[ignore = "manual synthetic native replay lookup, materialization and prefix measurements"]
fn measure_native_replay_history() {
    let continuation = json!({
        "previous_response_id":"synthetic-response",
        "input":[{"role":"user","content":"Synthetic next turn"}],
        "instructions":"Synthetic current instructions", "tools":[], "tool_choice":"none"
    });
    let response = json!({
        "id":"synthetic-response",
        "output":[{"type":"message","role":"assistant",
            "content":[{"type":"output_text","text":"Synthetic previous answer"}]}]
    });
    for text_bytes in [64 * 1024, 1024 * 1024, 8 * 1024 * 1024] {
        let request = json!({"input":(0..64).map(|index| json!({
            "role":if index % 2 == 0 { "user" } else { "assistant" },
            "content":format!("Synthetic turn {index}: {}", "x".repeat(text_bytes / 64))
        })).collect::<Vec<_>>()});
        let (_, state) =
            NativeResponsesReplayState::from_response(&request, "synthetic-model", &response)
                .unwrap();
        let bytes = serde_json::to_vec(&state).unwrap().len();
        let mut store = NativeResponsesReplayStore::default();
        store.insert("key", "response", "route", state, 100);
        let replay = store.get("key", "response", "route", 100).unwrap();
        assert_eq!(
            replay
                .replay_request(&continuation, "synthetic-model", false)
                .unwrap(),
            deep_copy(replay.clone())
                .replay_request(&continuation, "synthetic-model", false)
                .unwrap()
        );
        measure_replay_operation("lookup", bytes, |reference| {
            let state = store.get("key", "response", "route", 100).unwrap();
            black_box(if reference { deep_copy(state) } else { state });
        });
        measure_replay_operation("lookup_and_materialize", bytes, |reference| {
            let state = store.get("key", "response", "route", 100).unwrap();
            let replay = if reference { deep_copy(state) } else { state };
            black_box(
                replay
                    .replay_request(black_box(&continuation), "synthetic-model", false)
                    .unwrap(),
            );
        });

        let text = "s".repeat(text_bytes);
        let (_, previous) = NativeResponsesReplayState::from_response(
            &json!({"input":text}),
            "synthetic-model",
            &response,
        )
        .unwrap();
        let materialized = previous
            .replay_request(&continuation, "synthetic-model", false)
            .unwrap();
        let (_, next) =
            NativeResponsesReplayState::from_response(&materialized, "synthetic-model", &response)
                .unwrap();
        assert!(previous.is_materialized_predecessor_of(&next));
        let message = &next.history.initial_input[0];
        measure_replay_operation("string_message_match", text_bytes, |reference| {
            black_box(if reference {
                black_box(message)
                    == &json!({"role":"user","content":[{"type":"input_text","text":black_box(&text)}]})
            } else {
                is_replay_text_message(black_box(message), black_box(&text))
            });
        });
    }
}
