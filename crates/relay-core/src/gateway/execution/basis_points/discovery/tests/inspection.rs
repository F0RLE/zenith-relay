use super::*;

fn client_response(bytes: usize, stringified: bool, mixed: bool) -> Value {
    let unit = "synthetic relay.catalog \"quoted\"\\path\r\nстрока\t";
    let mut reply = response(
        "files.write",
        json!({"text":unit.repeat(bytes / unit.len() + 1)}),
    );
    if !stringified {
        reply["output"][0]["arguments"] =
            serde_json::from_str(reply["output"][0]["arguments"].as_str().unwrap()).unwrap();
    }
    if mixed {
        let mut lookup =
            response(LOOKUP_TOOL, json!({"names":["files.read"]}))["output"][0].clone();
        lookup["call_id"] = json!("call_lookup");
        lookup["id"] = json!("fc_lookup");
        reply["output"].as_array_mut().unwrap().insert(0, lookup);
    }
    reply
}

#[test]
fn discovery_inspection_preserves_payload_and_client_result_roundtrip() {
    for stringified in [false, true] {
        for mixed in [false, true] {
            let request = request();
            let mut upstream = prepare_request(&request).unwrap();
            let before = upstream.clone();
            let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
            let mut reply = client_response(4096, stringified, mixed);
            let client = reply["output"].as_array().unwrap().last().unwrap().clone();
            let mut expected = reply.clone();
            expected["output"] = json!([client]);
            assert!(!session.advance(&reply, &mut upstream).unwrap());
            session.finish(&mut reply);
            assert_eq!(upstream, before);
            assert_eq!(
                serde_json::to_vec(&reply).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
            assert_eq!(session.rounds, 0);

            let translated: Value = serde_json::from_slice(
                &translate_response(&serde_json::to_vec(&reply).unwrap(), &request).unwrap(),
            )
            .unwrap();
            let call = &translated["output"][0];
            assert_eq!(call["call_id"], "call_synthetic");
            assert_eq!(call["name"], "write");
            assert_eq!(call["namespace"], "files");
            let arguments: Value =
                serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
            let mut continued = request.clone();
            continued["input"] = json!([
                {"role":"user","content":"Synthetic task"}, call,
                {"type":"function_call_output","call_id":"call_synthetic","output":"synthetic_result"}
            ]);
            let prepared = prepare_request(&continued).unwrap();
            let input = prepared["input"].as_array().unwrap();
            let replayed = input
                .iter()
                .find(|item| item["type"] == "function_call" && item["call_id"] == "call_synthetic")
                .unwrap();
            let (envelope, _) = parse_transport_envelope(replayed.as_object().unwrap()).unwrap();
            assert_eq!(envelope["tool"], "files.write");
            assert_eq!(
                super::super::super::codec::function_arguments(&envelope["args"]).unwrap(),
                arguments
            );
            assert!(
                input
                    .iter()
                    .any(|item| item["type"] == "function_call_output"
                        && item["call_id"] == "call_synthetic"
                        && item["output"] == "synthetic_result")
            );
            let final_reply = json!({"id":"resp_final","status":"completed","output":[{
                "type":"message","id":"msg_final","role":"assistant",
                "content":[{"type":"output_text","text":"Synthetic completion"}]
            }]});
            let final_body: Value = serde_json::from_slice(
                &translate_response(&serde_json::to_vec(&final_reply).unwrap(), &continued)
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(final_body["output"], final_reply["output"]);
        }
    }
}

#[test]
fn discovery_inspection_does_not_hide_malformed_client_arguments() {
    for arguments in [
        r#"{"references":["files.write"],"code":"\uD800"}"#,
        r#"{"references":["files.write"],"code":"{\"text\":"}"#,
        r#"{"references":["files.write"],"code":"{\"names\":[\"files.read\"]}"}"#,
        r#"{"references":["files.write"],"code":"{}"} trailing"#,
    ] {
        let request = request();
        let mut upstream = prepare_request(&request).unwrap();
        let before = upstream.clone();
        let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
        let mut reply = client_response(1, true, false);
        reply["output"][0]["arguments"] = json!(arguments);
        let expected = reply.clone();
        assert!(!session.advance(&reply, &mut upstream).unwrap());
        session.finish(&mut reply);
        assert_eq!(reply, expected);
        assert_eq!(upstream, before);
        assert!(translate_response(&serde_json::to_vec(&reply).unwrap(), &request).is_err());
    }
}

#[test]
#[ignore = "local BPS discovery inspection benchmark; no provider or TTFT measurement"]
fn measure_bps_discovery_inspection() {
    use std::{hint::black_box, time::Instant};

    let request = request();
    let prepared = prepare_request(&request).unwrap();
    for bytes in [4096, 1024 * 1024] {
        for stringified in [false, true] {
            for mixed in [false, true] {
                let source = client_response(bytes, stringified, mixed);
                let wire_bytes = serde_json::to_vec(&source).unwrap().len();
                let mut expected = source.clone();
                if mixed {
                    expected["output"].as_array_mut().unwrap().remove(0);
                }
                let mut samples = Vec::new();
                for trial in 0..16 {
                    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
                    let mut upstream = prepared.clone();
                    let mut reply = source.clone();
                    let started = Instant::now();
                    assert!(!session.advance(black_box(&reply), &mut upstream).unwrap());
                    session.finish(black_box(&mut reply));
                    let elapsed = started.elapsed().as_nanos();
                    assert_eq!(reply, expected);
                    assert_eq!(upstream, prepared);
                    if trial > 0 {
                        samples.push(elapsed);
                    }
                }
                samples.sort_unstable();
                eprintln!(
                    "bps-discovery-inspection stringified={stringified} mixed={mixed} wire_bytes={wire_bytes} trials=15 median_ns={} p90_ns={}",
                    samples[7], samples[13]
                );
            }
        }
    }
}
