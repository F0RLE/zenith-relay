use super::*;
use crate::usage::{RequestPayloadMode, SocketOutputReconciliation};

#[tokio::test]
async fn sparse_terminal_and_omitted_duplicate_turn_metadata_send_only_the_tool_result() {
    let call = json!({"type":"function_call","name":"lookup","id":"fc_test",
        "call_id":"call_test","arguments":"{}","status":"completed"});
    let mut upstream = call.clone();
    let trace = "00000000-0000-4000-8000-000000000001";
    upstream["metadata"] = json!({"turn_id":trace});
    upstream["internal_chat_message_metadata_passthrough"] = json!({"turn_id":trace});
    let fixture = Fixture::start(vec![
        vec![
            json!({"type":"response.output_item.done","output_index":0,"item":upstream}),
            completed("first", json!([])),
        ],
        vec![completed("second", json!([call]))],
    ])
    .await;
    let first = request(json!([{"role":"user","content":"Synthetic history.".repeat(60_000)}]));
    let store = SocketStore::default();
    let original = fixture.pending(&store, &first).await.send().await.unwrap();
    assert!(original.text().await.unwrap().contains(trace));
    let mut next = first;
    let result = json!({"type":"function_call_output","call_id":"call_test","output":"ok"});
    next["input"]
        .as_array_mut()
        .unwrap()
        .extend([call, result.clone()]);
    let full_bytes = serde_json::to_vec(&next).unwrap().len();
    let response = fixture.pending(&store, &next).await.send().await.unwrap();
    let payload = response
        .extensions()
        .get::<RequestPayloadDiagnostics>()
        .unwrap();
    assert_eq!(payload.mode, RequestPayloadMode::Continuation);
    assert_eq!(
        payload.socket_output_reconciled,
        Some(SocketOutputReconciliation::OmittedDuplicateTurnMetadata)
    );
    assert!(!serde_json::to_string(payload).unwrap().contains(trace));
    assert!(full_bytes > 1_000_000 && payload.body_bytes < 512);
    eprintln!(
        "sparse synthetic full: {full_bytes} B; WebSocket suffix: {} B",
        payload.body_bytes
    );
    response.text().await.unwrap();
    let sent = fixture.requests();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1]["previous_response_id"], "first");
    assert_eq!(sent[1]["input"], json!([result]));
}

#[tokio::test]
async fn one_omitted_object_stays_full_and_private_on_the_wire() {
    let context = &crate::gateway::siwc_probe_contexts("synthetic-siwc-probe-session")[0];
    let replay = json!({"type":"reasoning","id":"rs_probe","summary":[],
        "encrypted_content":"synthetic cipher"});
    let mut original = replay.clone();
    original["fixture_annotation"] = json!({"private_fixture_value":true});
    let fixture = Fixture::start(vec![
        vec![completed("first", json!([original]))],
        vec![completed("second", json!([original]))],
        vec![completed("third", json!([original]))],
    ])
    .await;
    let store = SocketStore::default();
    let mut body = request(json!([{"role":"user","content":"synthetic"}]));
    fixture
        .pending_in_context(&store, &body, context)
        .await
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    for _ in 0..2 {
        body["input"]
            .as_array_mut()
            .unwrap()
            .extend([replay.clone(), json!({"role":"user","content":"next"})]);
        let response = fixture
            .pending_in_context(&store, &body, context)
            .await
            .send()
            .await
            .unwrap();
        let payload = response
            .extensions()
            .get::<RequestPayloadDiagnostics>()
            .unwrap();
        assert_eq!(
            payload.full_request_reason,
            Some(crate::usage::FullRequestReason::SocketOutputChanged)
        );
        let mismatch = payload.socket_output_mismatch.as_ref().unwrap();
        assert_eq!(mismatch.fields, [crate::usage::SocketReplayField::Other]);
        let encoded = serde_json::to_string(mismatch).unwrap();
        assert!(!encoded.contains("fixture"));
        response.text().await.unwrap();
        let sent = fixture.requests();
        let last = sent.last().unwrap();
        assert_eq!(last["input"], body["input"]);
        assert!(last.get("previous_response_id").is_none());
    }
}

#[tokio::test]
async fn large_native_history_sends_only_the_tool_result_after_root_attribution_is_omitted() {
    let first = request(json!([{"role":"user","content":"Synthetic history ".repeat(64_000)}]));
    let reasoning = json!({"type":"reasoning","id":"rs_test","summary":[],"status":"completed",
        "encrypted_content":"opaque".repeat(40_000),"agent":{"agent_name":"/root"}});
    let call = json!({"type":"function_call","id":"fc_test","call_id":"call_test","name":"lookup",
        "arguments":"{}","parsed_arguments":{},"status":"completed","agent":{"agent_name":"/root"}});
    let mut answer = json!({"type":"message","role":"assistant","id":"msg_test","status":"completed",
        "content":[{"type":"output_text","text":"synthetic answer"}]});
    let fixture = Fixture::start(vec![
        vec![completed("first", json!([reasoning, call]))],
        vec![completed("second", json!([answer]))],
        vec![completed("third", json!([answer]))],
    ])
    .await;
    let store = SocketStore::default();
    let original = fixture
        .pending(&store, &first)
        .await
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(original.contains("\"agent_name\":\"/root\""));
    let mut next = first;
    let mut replay = [reasoning, call];
    for item in &mut replay {
        for field in ["agent", "status", "parsed_arguments"] {
            item.as_object_mut().unwrap().remove(field);
        }
    }
    let result = json!({"type":"function_call_output","call_id":"call_test","output":"ok"});
    next["input"].as_array_mut().unwrap().extend(replay);
    next["input"].as_array_mut().unwrap().push(result.clone());
    let full_bytes = serde_json::to_vec(&next).unwrap().len();
    let response = fixture.pending(&store, &next).await.send().await.unwrap();
    let diagnostics = response
        .extensions()
        .get::<RequestPayloadDiagnostics>()
        .unwrap();
    assert!(
        response
            .extensions()
            .get::<SocketDiagnostics>()
            .unwrap()
            .reused
    );
    assert_eq!(diagnostics.mode, RequestPayloadMode::Continuation);
    assert_eq!(
        diagnostics.socket_output_reconciled,
        Some(SocketOutputReconciliation::OmittedAllowlistedOutputFields)
    );
    assert!(full_bytes > 1_000_000 && diagnostics.body_bytes < 512);
    eprintln!(
        "synthetic full JSON: {full_bytes} B; WebSocket suffix: {} B",
        diagnostics.body_bytes
    );
    response.text().await.unwrap();
    let followup = json!({"role":"user","content":"synthetic next"});
    answer.as_object_mut().unwrap().remove("status");
    next["input"]
        .as_array_mut()
        .unwrap()
        .extend([answer, followup.clone()]);
    let response = fixture.pending(&store, &next).await.send().await.unwrap();
    let diagnostics = response
        .extensions()
        .get::<RequestPayloadDiagnostics>()
        .unwrap();
    assert_eq!(diagnostics.mode, RequestPayloadMode::Continuation);
    assert_eq!(
        diagnostics.socket_output_reconciled,
        Some(SocketOutputReconciliation::OmittedCompletedMessageStatus)
    );
    response.text().await.unwrap();
    let sent = fixture.requests();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[1]["previous_response_id"], "first");
    assert_eq!(sent[1]["input"], json!([result]));
    assert_eq!(sent[2]["previous_response_id"], "second");
    assert_eq!(sent[2]["input"], json!([followup]));
}
