use super::*;
use serde_json::{Value, json};

mod diagnostics;
mod idle_lifecycle;
mod replay;
mod settings;
mod wire;

fn request(input: Value) -> Value {
    json!({"model":"synthetic-model","store":false,"stream":true,
        "reasoning":{"effort":"high"},"input":input})
}

fn plan(body: &Value, checkpoint: Option<&planner::Checkpoint>) -> planner::Plan {
    planner::parse(&serde_json::to_vec(body).unwrap())
        .unwrap()
        .with_diagnostics(true)
        .plan(checkpoint)
}

#[test]
fn continuation_requires_exact_history_and_completed_output() {
    let first = request(json!([{"role":"user","content":"synthetic request"}]));
    let output = json!({"type":"function_call","id":"fc_test","call_id":"call_test","name":"lookup","arguments":"{}","status":"completed"});
    let checkpoint = plan(&first, None)
        .state
        .complete(&json!({"id":"resp_test","status":"completed","output":[output]}))
        .unwrap();
    let result =
        json!({"type":"function_call_output","call_id":"call_test","output":"synthetic result"});
    let mut next = first.clone();
    next["input"]
        .as_array_mut()
        .unwrap()
        .extend([output, result.clone()]);
    let delta: Value = serde_json::from_str(&plan(&next, Some(&checkpoint)).payload).unwrap();
    assert_eq!(delta["previous_response_id"], "resp_test");
    assert_eq!(delta["input"], json!([result]));
    assert_eq!(delta["type"], "response.create");
    assert!(delta.get("stream").is_none());
    assert_eq!(delta["reasoning"], first["reasoning"]);
    for changed in ["model", "reasoning", "tools"] {
        let mut edited = next.clone();
        edited[changed] = json!("changed");
        let full: Value = serde_json::from_str(&plan(&edited, Some(&checkpoint)).payload).unwrap();
        assert!(full.get("previous_response_id").is_none());
        assert_eq!(full["input"], edited["input"]);
    }
    next["input"][0]["content"] = json!("edited history");
    let full: Value = serde_json::from_str(&plan(&next, Some(&checkpoint)).payload).unwrap();
    assert!(full.get("previous_response_id").is_none());
    assert_eq!(full["input"], next["input"]);
}

#[test]
fn continuation_compares_objects_without_rewriting_the_wire() {
    let first: Value = serde_json::from_str(r#"{
        "store":false,"stream":true,"input":[{"role":"user","content":"synthetic"}],
        "model":"synthetic-model","tools":[{"type":"function","name":"lookup","parameters":{"type":"object","properties":{"z":{"type":"number"},"a":{"type":"string"}}}}]
    }"#).unwrap();
    let output = json!({"type":"function_call","id":"fc_test","call_id":"call_test",
        "name":"lookup","arguments":"{}","status":"completed"});
    let checkpoint = plan(&first, None)
        .state
        .complete(&json!({"id":"first","status":"completed","output":[output]}))
        .unwrap();
    let mut next = first.clone();
    next["input"].as_array_mut().unwrap().extend([
        output,
        json!({"type":"function_call_output","call_id":"call_test","output":"ok"}),
    ]);
    next["prompt_cache_options"] = json!({"comparison_response_id":"first"});
    next.sort_all_objects();
    let expected_tools = next["tools"].clone();
    let encoded: Value = serde_json::from_str(&plan(&next, Some(&checkpoint)).payload).unwrap();
    assert_eq!(encoded["previous_response_id"], "first");
    assert_eq!(encoded["input"].as_array().unwrap().len(), 1);
    assert_eq!(
        serde_json::to_string(&encoded["tools"]).unwrap(),
        serde_json::to_string(&expected_tools).unwrap()
    );
}

#[test]
fn codex_reasoning_replay_omits_completed_status_without_changing_ciphertext() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let reasoning = json!({"type":"reasoning","id":"rs_test","status":"completed",
        "summary":[],"encrypted_content":"synthetic-cipher"});
    let call = json!({"type":"function_call","id":"fc_test","status":"completed",
        "call_id":"call_test","name":"lookup","arguments":"{}"});
    let checkpoint = plan(&body, None)
        .state
        .complete(&json!({"id":"first",
        "status":"completed","output":[reasoning,call]}))
        .unwrap();
    let mut replay = reasoning.clone();
    replay.as_object_mut().unwrap().remove("status");
    let result =
        json!({"type":"function_call_output","call_id":"call_test","output":"synthetic result"});
    let mut next = body;
    next["input"]
        .as_array_mut()
        .unwrap()
        .extend([replay, call, result.clone()]);
    let encoded: Value = serde_json::from_str(&plan(&next, Some(&checkpoint)).payload).unwrap();
    assert_eq!(encoded["previous_response_id"], "first");
    assert_eq!(encoded["input"], json!([result]));
    for (field, value) in [
        ("encrypted_content", json!("different-cipher")),
        ("id", json!("rs_foreign")),
        ("summary", json!([{"type":"summary_text","text":"changed"}])),
        ("status", json!("in_progress")),
    ] {
        let mut changed = next.clone();
        changed["input"][1][field] = value;
        let encoded: Value =
            serde_json::from_str(&plan(&changed, Some(&checkpoint)).payload).unwrap();
        assert!(
            encoded.get("previous_response_id").is_none(),
            "changed {field} must not reuse server state"
        );
    }
}

#[test]
fn full_history_diagnostics_distinguish_checkpoint_settings_input_and_output() {
    use crate::usage::FullRequestReason;
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    assert_eq!(
        plan(&body, None).full_request_reason,
        Some(FullRequestReason::SocketCheckpointUnavailable)
    );
    let output = json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic answer"}]});
    let checkpoint = plan(&body, None)
        .state
        .complete(&json!({"id":"first","status":"completed","output":[output]}))
        .unwrap();
    let mut next = body;
    next["input"]
        .as_array_mut()
        .unwrap()
        .extend([output, json!({"role":"user","content":"next"})]);
    assert_eq!(plan(&next, Some(&checkpoint)).full_request_reason, None);
    let mut changed = next.clone();
    changed["reasoning"]["effort"] = json!("low");
    assert_eq!(
        plan(&changed, Some(&checkpoint)).full_request_reason,
        Some(FullRequestReason::SocketSettingsChanged)
    );
    changed = next.clone();
    changed["input"][0]["content"] = json!("edited prefix");
    assert_eq!(
        plan(&changed, Some(&checkpoint)).full_request_reason,
        Some(FullRequestReason::SocketHistoryChanged)
    );
    changed = next.clone();
    changed["input"][1]["content"][0]["text"] = json!("edited answer");
    assert_eq!(
        plan(&changed, Some(&checkpoint)).full_request_reason,
        Some(FullRequestReason::SocketOutputChanged)
    );
    next["input"].as_array_mut().unwrap().pop();
    assert_eq!(
        plan(&next, Some(&checkpoint)).full_request_reason,
        Some(FullRequestReason::SocketNoNewInput)
    );
}

#[test]
fn failed_incomplete_and_missing_output_do_not_checkpoint() {
    let body = request(json!([]));
    for terminal in [
        json!({"id":"r","status":"failed","output":[]}),
        json!({"id":"r","status":"incomplete","output":[]}),
        json!({"id":"r","status":"completed"}),
        json!({"id":"r","status":"completed","output":[]}),
        json!({"id":"r","status":"completed","output":[{
            "type":"message","role":"assistant","content":[{
                "type":"output_text","text":"Synthetic output"}]}],
            "error":{"code":"synthetic_provider_error"}}),
    ] {
        assert!(plan(&body, None).state.complete(&terminal).is_none());
    }
}

#[test]
fn busy_lanes_are_isolated_and_store_is_bounded() {
    let store = SocketStore::default();
    let guards: Vec<_> = (0..MAX_SESSIONS)
        .map(|id| store.acquire(planner::fingerprint(&json!(id))).unwrap())
        .collect();
    assert!(store.acquire(planner::fingerprint(&json!(0))).is_none());
    let mut isolated = store.acquire_or_isolate(Some(planner::fingerprint(&json!(0))));
    isolated.authority = Some(SocketAuthority {
        credential: [7; 32],
        incarnation: crate::runtime::AuthorizationIncarnation::Agent(1),
    });
    assert!(guards[0].authority.is_none());
    assert!(store.acquire_or_isolate(None).authority.is_none());
    assert_eq!(
        store.acquire_or_isolate(None).reset_reason,
        Some(ConnectionResetReason::MissingContext)
    );
    assert_eq!(
        isolated.reset_reason,
        Some(ConnectionResetReason::SessionUnavailable)
    );
    assert!(
        store
            .acquire(planner::fingerprint(&json!(MAX_SESSIONS)))
            .is_none()
    );
    assert!(
        store
            .acquire_or_isolate(Some(planner::fingerprint(&json!(MAX_SESSIONS))))
            .idle
            .is_none()
    );
    assert_eq!(crate::poison::mutex(&store.lanes).len(), MAX_SESSIONS);
    drop(guards);
    assert!(store.acquire(planner::fingerprint(&json!(0))).is_some());
}

#[test]
fn capacity_reclaims_the_oldest_idle_lane_without_touching_busy_owners() {
    let store = SocketStore::default();
    let mut guards: Vec<_> = (0..MAX_SESSIONS)
        .map(|id| Some(store.acquire(planner::fingerprint(&json!(id))).unwrap()))
        .collect();
    let oldest = planner::fingerprint(&json!(0));
    let recent = planner::fingerprint(&json!(1));
    guards[0].as_mut().unwrap().touched = Instant::now() - Duration::from_secs(60);
    drop(guards[0].take());
    drop(guards[1].take());
    let new_key = planner::fingerprint(&json!("new"));
    let new_lane = store.acquire(new_key).expect("reclaim an idle lane");
    let lanes = crate::poison::mutex(&store.lanes);
    assert_eq!(lanes.len(), MAX_SESSIONS);
    assert!(!lanes.contains_key(&oldest));
    assert!(lanes.contains_key(&recent));
    assert!(lanes.contains_key(&new_key));
    for (id, guard) in guards.iter().enumerate().skip(2) {
        assert!(lanes.contains_key(&planner::fingerprint(&json!(id))));
        assert!(guard.is_some());
    }
    assert_eq!(
        new_lane.reset_reason,
        Some(ConnectionResetReason::NewSession)
    );
}

#[test]
fn expiry_evicts_idle_but_preserves_busy_lanes() {
    let store = SocketStore::default();
    let key = planner::fingerprint(&json!("expired"));
    let mut guard = store.acquire(key).unwrap();
    guard.touched = Instant::now() - IDLE_TTL;
    assert!(store.acquire(key).is_none());
    drop(guard);
    let lane = store.acquire(key).unwrap();
    assert!(lane.touched.elapsed() < IDLE_TTL);
    assert_eq!(
        lane.reset_reason,
        Some(ConnectionResetReason::SessionExpired)
    );
}

#[test]
fn cache_controls_are_preserved_and_prewarm_uses_websocket_warmup() {
    let mut body = request(json!([{"role":"user","content":"synthetic"}]));
    let output = json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic"}]});
    body["prompt_cache_options"] =
        json!({"comparison_response_id":"old","report_prefix_length":true});
    let checkpoint = plan(&body, None)
        .state
        .complete(&json!({"status":"completed","id":"resp_test","output":[output]}))
        .unwrap();
    body["prompt_cache_options"]["comparison_response_id"] = json!("new");
    body["input"]
        .as_array_mut()
        .unwrap()
        .extend([output, json!({"role":"user","content":"next"})]);
    let encoded: Value = serde_json::from_str(&plan(&body, Some(&checkpoint)).payload).unwrap();
    assert_eq!(encoded["previous_response_id"], "resp_test");
    assert_eq!(
        encoded["prompt_cache_options"],
        body["prompt_cache_options"]
    );
    body["prompt_cache_options"]["prewarm"] = json!(true);
    let warmup = plan(&body, None);
    let encoded: Value = serde_json::from_str(&warmup.payload).unwrap();
    assert_eq!(encoded["generate"], false);
    assert!(encoded["prompt_cache_options"].get("prewarm").is_none());
    assert_eq!(
        encoded["prompt_cache_options"]["comparison_response_id"],
        "new"
    );
    assert_eq!(
        encoded["prompt_cache_options"]["report_prefix_length"],
        true
    );
    let checkpoint = warmup
        .state
        .complete(&json!({"status":"completed","id":"resp_warmup","output":[]}))
        .unwrap();
    body["prompt_cache_options"]
        .as_object_mut()
        .unwrap()
        .remove("prewarm");
    body["input"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role":"user","content":"after warmup"}));
    let encoded: Value = serde_json::from_str(&plan(&body, Some(&checkpoint)).payload).unwrap();
    assert_eq!(encoded["previous_response_id"], "resp_warmup");
    assert_eq!(encoded["input"].as_array().unwrap().len(), 1);
    assert!(encoded.get("generate").is_none());
}

#[test]
fn external_continuations_and_unsupported_wire_fields_are_rejected() {
    for (name, value) in [
        ("previous_response_id", json!("foreign")),
        ("background", json!(false)),
        ("stream_id", json!("foreign")),
        ("generate", json!(true)),
        ("store", json!(true)),
        ("stream", json!(false)),
        ("input", json!("text")),
    ] {
        let mut body = request(json!([]));
        body[name] = value;
        assert!(
            planner::parse(&serde_json::to_vec(&body).unwrap()).is_none(),
            "{name}"
        );
    }
}

#[test]
fn session_keys_separate_local_keys_accounts_contexts_and_endpoints() {
    let url = "https://api.openai.com/v1/responses".parse().unwrap();
    let key = session_key("key", "account", "context", &url);
    for scoped in [
        session_key("other", "account", "context", &url),
        session_key("key", "other", "context", &url),
        session_key("key", "account", "other", &url),
        session_key(
            "key",
            "account",
            "context",
            &"https://other.invalid/v1/responses".parse().unwrap(),
        ),
    ] {
        assert_ne!(key, scoped);
    }
}

#[test]
fn renewed_authority_rebinds_the_lane() {
    let store = SocketStore::default();
    let mut lane = store.acquire([0; 32]).unwrap();
    let authority = |token, revision| SocketAuthority {
        credential: [token; 32],
        incarnation: crate::runtime::AuthorizationIncarnation::Agent(revision),
    };
    lane.bind(authority(1, 1));
    assert_eq!(lane.reset_reason, Some(ConnectionResetReason::NewSession));
    lane.bind(authority(1, 1));
    assert!(lane.authority.as_ref() == Some(&authority(1, 1)));
    lane.bind(authority(1, 2));
    assert_eq!(
        lane.reset_reason,
        Some(ConnectionResetReason::AuthorizationChanged)
    );
    assert!(lane.authority.as_ref() == Some(&authority(1, 2)));
    lane.bind(authority(2, 2));
    assert!(lane.authority.as_ref() == Some(&authority(2, 2)));
}

#[test]
fn generation_requires_the_public_responses_endpoint() {
    let client = reqwest::Client::new();
    assert!(eligible_endpoint(
        &client
            .post("https://api.openai.com/v1/responses")
            .build()
            .unwrap()
    ));
    for endpoint in [
        "http://api.openai.com/v1/responses",
        "https://api.openai.com:444/v1/responses",
        "https://provider.invalid/v1/responses",
        "https://api.openai.com/v1/responses?other=1",
        "https://api.openai.com/v1/models",
    ] {
        assert!(!eligible_endpoint(&client.post(endpoint).build().unwrap()));
    }
}

#[test]
fn text_socket_policy_does_not_restrict_image_endpoints_or_catalog_reads() {
    let client = reqwest::Client::new();
    for path in ["images/generations", "images/edits", "images/variations"] {
        let request = client
            .post(format!("https://api.openai.com/v1/{path}"))
            .build()
            .unwrap();
        assert!(!requires_text_socket(&request));
    }
    assert!(!requires_text_socket(
        &client
            .get("https://api.openai.com/v1/models")
            .build()
            .unwrap()
    ));
    assert!(requires_text_socket(
        &client
            .post("https://api.openai.com/v1/responses")
            .build()
            .unwrap()
    ));
}
