use super::super::catalog::client_tools;
use super::*;

fn dispatch_runtime() -> crate::GatewayRuntime {
    crate::GatewayRuntime::from_pool(
        vec![crate::RuntimeSource::unrestricted(crate::ProviderSource {
            id: "synthetic".into(),
            name: "Synthetic".into(),
            base_url: "https://example.test/v1".into(),
            api_key: "synthetic".into(),
            proxy_url: None,
            wire_api: crate::WireApi::Responses,
            models: vec!["synthetic-model".into()],
        })],
        vec![crate::RuntimeLocalKey::unrestricted(
            crate::LocalGatewayKey {
                id: "synthetic-key".into(),
                secret: "synthetic-secret".into(),
            },
        )],
        crate::GatewayRuntimeOptions::default(),
        std::sync::Arc::new(|_| {}),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "manual local BPS dispatch preparation; no provider or TTFT measurement"]
async fn measure_bps_dispatch_preparation() {
    use super::super::discovery::execution;
    use crate::scheduler::rotation::SharedRequestBudget;
    use crate::usage::ToolUseDiagnostics;
    use std::{hint::black_box, time::Instant};

    let runtime = dispatch_runtime();
    for size in [4096, 1024 * 1024] {
        for debug in [false, true] {
            let original = json!({"model":"synthetic-model", "relay_bps_tool_catalog":"discovery",
                "input":"Synthetic stable history. ".repeat(size / 26 + 1),
                "tools":[{"type":"function","name":"lookup","parameters":{"type":"object",
                    "properties":{"path":{"type":"string"}},"required":["path"]}}]});
            let upstream = prepare_request(&original).unwrap();
            let expected = serde_json::to_vec(&upstream).unwrap();
            let mut samples = Vec::new();
            for trial in 0..16 {
                // Construction and wire-byte assertions are outside the measured phase.
                let prepared = upstream.clone();
                let mut diagnostics = ToolUseDiagnostics::default();
                if debug {
                    diagnostics.client_context =
                        Some(crate::usage::RequestContextSize::capture(&original));
                }
                let budget = SharedRequestBudget::for_incoming_request(8);
                let started = Instant::now();
                let (body, session) = execution::prepare_dispatch(
                    black_box(&original),
                    black_box(prepared),
                    &budget,
                    &mut diagnostics,
                    &runtime,
                    "synthetic-key",
                    Some("synthetic-chat"),
                )
                .unwrap();
                if trial > 0 {
                    samples.push(started.elapsed().as_nanos());
                }
                assert!(session.is_some());
                assert_eq!(serde_json::to_vec(&body).unwrap(), expected);
            }
            samples.sort_unstable();
            eprintln!(
                "bps-dispatch bytes={} diagnostics={debug} trials=15 median_ns={} p90_ns={}",
                expected.len(),
                samples[7],
                samples[13]
            );
        }
    }
}

#[tokio::test]
#[ignore = "manual local catalog reuse benchmark; no provider or TTFT measurement"]
async fn measure_bps_request_catalog_reuse() {
    use super::super::{
        PreparedClientRequest, discovery::execution, response::translate_response_value,
    };
    use crate::{scheduler::rotation::SharedRequestBudget, usage::ToolUseDiagnostics};
    use std::time::Instant;

    let runtime = dispatch_runtime();
    let tools = (0..313).map(|index| json!({"type":"function","name":format!("tool_{index}"),
        "description":"Synthetic description. ".repeat(16),
        "parameters":{"type":"object","properties":{"path":{"type":"string"},"count":{"type":"integer"}},
            "required":["path"],"additionalProperties":false}})).collect::<Vec<_>>();
    let request = json!({"model":"synthetic-model","relay_bps_tool_catalog":"discovery",
        "input":"Synthetic stable history. ".repeat(40_000),"tools":tools});
    let expected = prepare_request(&request).unwrap();
    for cached in [false, true] {
        let mut samples = Vec::new();
        for trial in 0..12 {
            let request = request.clone();
            let response = json!({"id":"resp_synthetic","status":"completed","output":[]});
            let budget = SharedRequestBudget::for_incoming_request(8);
            let mut diagnostics = ToolUseDiagnostics::default();
            let started = Instant::now();
            let (body, translated) = if cached {
                let (context, body) =
                    PreparedClientRequest::prepare(request, &mut None, false).unwrap();
                let (body, _) = execution::prepare_dispatch_with_catalog(
                    (&context, context.catalog_snapshot()),
                    body,
                    &budget,
                    &mut diagnostics,
                    &runtime,
                    "synthetic-key",
                    None,
                )
                .unwrap();
                let translated = context.translate_value(response).unwrap();
                (body, translated)
            } else {
                let body = prepare_request(&request).unwrap();
                let (body, _) = execution::prepare_dispatch(
                    &request,
                    body,
                    &budget,
                    &mut diagnostics,
                    &runtime,
                    "synthetic-key",
                    None,
                )
                .unwrap();
                let translated = translate_response_value(response, &request).unwrap();
                (body, translated)
            };
            let elapsed = started.elapsed().as_nanos();
            assert_eq!(body, expected);
            assert_eq!(translated["status"], "completed");
            if trial > 0 {
                samples.push(elapsed);
            }
        }
        samples.sort_unstable();
        eprintln!(
            "bps-request-catalog reused={cached} tools=313 history_bytes={} median_ns={} p90_ns={}",
            request["input"].as_str().unwrap().len(),
            samples[5],
            samples[9]
        );
    }
}

#[tokio::test]
async fn bps_dispatch_moves_history_without_changing_wire_or_catalog_diagnostics() {
    use super::super::discovery::{capture, execution};
    use crate::scheduler::rotation::SharedRequestBudget;
    use crate::usage::ToolUseDiagnostics;

    let runtime = dispatch_runtime();
    for mode in ["compact", "discovery"] {
        let mut original = request_with_tool();
        original["relay_bps_tool_catalog"] = json!(mode);
        original["input"] = json!("Synthetic stable history. ".repeat(4096));
        let upstream = prepare_request(&original).unwrap();
        let text = upstream["input"].as_array().unwrap().last().unwrap()["content"][0]["text"]
            .as_str()
            .unwrap();
        let pointer = text.as_ptr();
        let expected = serde_json::to_vec(&upstream).unwrap();
        let mut diagnostics = ToolUseDiagnostics {
            client_context: Some(crate::usage::RequestContextSize::capture(&original)),
            ..Default::default()
        };
        let mut previous = diagnostics.clone();
        capture(&original, &mut previous);
        let (body, _) = execution::prepare_dispatch(
            &original,
            upstream,
            &SharedRequestBudget::for_incoming_request(8),
            &mut diagnostics,
            &runtime,
            "synthetic-key",
            None,
        )
        .unwrap();
        assert_eq!(serde_json::to_vec(&body).unwrap(), expected);
        assert_eq!(
            body["input"].as_array().unwrap().last().unwrap()["content"][0]["text"]
                .as_str()
                .unwrap()
                .as_ptr(),
            pointer
        );
        assert_eq!(
            diagnostics
                .basis_points_catalog
                .as_ref()
                .unwrap()
                .catalog_prompt_bytes,
            previous
                .basis_points_catalog
                .as_ref()
                .unwrap()
                .catalog_prompt_bytes
        );
    }
}

#[test]
fn bps_tool_history_preserves_exact_wire_bytes() {
    use super::super::{
        codec::{encode_function_arguments, parse_function_arguments},
        history::transport_call,
    };

    let script = "Write-Output \"synthetic\"\r\n$x = 'C:\\synthetic\\q'\n😀\t";
    let arguments = json!({"cmd":script,"workdir":r"C:\synthetic\project","options":{"limit":3}});
    let expected_code = format!(
        "{}{{\"field\":\"cmd\",\"args\":{{\"options\":{{\"limit\":3}},\"workdir\":\"C:\\\\synthetic\\\\project\"}}}}\n{script}",
        super::super::FUNCTION_STRING_PREFIX
    );
    assert_eq!(
        encode_function_arguments(&arguments).unwrap(),
        expected_code
    );
    let request = json!({"tools":[{"type":"namespace","name":"synthetic","tools":[
        {"type":"function","name":"exec_command","parameters":{"type":"object"}},
        {"type":"custom","name":"raw_input"}
    ]}]});
    let tools = client_tools(&request).unwrap();
    for tool in tools.iter() {
        for encoded in [false, true] {
            let mut call = json!({"call_id":"call_wire"});
            let code = if tool.kind == "custom" {
                call["input"] = json!(script);
                script
            } else {
                call["arguments"] = if encoded {
                    json!(arguments.to_string())
                } else {
                    arguments.clone()
                };
                if !encoded {
                    let parsed = parse_function_arguments(call.as_object().unwrap()).unwrap();
                    assert!(std::ptr::eq(parsed.as_ref(), &call["arguments"]));
                }
                &expected_code
            };
            let expected_envelope = json!({
                "summary":format!("Run client tool {}", tool.call_name()),
                "extended_summary":"Relay client tool through the Excel / Basis Points transport",
                "destructive":false,"references":[tool.call_name()],"code":code
            });
            let expected = json!({
                "type":"function_call","id":"fc_call_wire","call_id":"call_wire",
                "name":"run_officejs","arguments":expected_envelope.to_string(),"status":"completed"
            });
            let actual = transport_call(call.as_object().unwrap(), tool).unwrap();
            assert_eq!(
                serde_json::to_vec(&actual).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
        }
    }
}

#[test]
#[ignore = "manual synthetic tool-history encoding; no provider or TTFT measurement"]
fn measure_bps_tool_history_replay() {
    use super::super::catalog::ClientTool;
    use super::super::codec::parse_json_object;
    use super::super::history::transport_call;
    use std::{hint::black_box, time::Instant};

    // Retain the replaced clone/Value-envelope path for paired measurements.
    // Fixture construction and output comparison stay outside both timers.
    fn cloned_replay(call: &serde_json::Map<String, Value>, tool: &ClientTool) -> Value {
        let arguments = parse_json_object(call.get("arguments")).unwrap();
        let fields = arguments.as_object().unwrap();
        let (field, raw, _) = fields
            .iter()
            .filter_map(|(field, value)| {
                let raw = value.as_str()?;
                let escapes = raw
                    .bytes()
                    .filter(|byte| matches!(*byte, b'"' | b'\\' | 0..=0x1f))
                    .count();
                (escapes > 0).then_some((field, raw, escapes))
            })
            .max_by_key(|(_, _, escapes)| *escapes)
            .unwrap();
        let mut other_arguments = fields.clone();
        other_arguments.remove(field);
        let header = json!({"field":field,"args":other_arguments});
        let code = format!("{}{}\n{raw}", super::super::FUNCTION_STRING_PREFIX, header);
        let envelope = json!({
            "summary":format!("Run client tool {}", tool.call_name()),
            "extended_summary":"Relay client tool through the Excel / Basis Points transport",
            "destructive":false,"references":[tool.call_name()],"code":code
        });
        json!({
            "type":"function_call","id":"fc_call_measure","call_id":"call_measure",
            "name":"run_officejs","arguments":envelope.to_string(),"status":"completed"
        })
    }

    let tools = client_tools(&request_with_tool()).unwrap();
    for bytes in [4096, 1024 * 1024] {
        let script = "Write-Output \"synthetic\"\n$x = 'C:\\synthetic\\q'\n".repeat(bytes / 48 + 1);
        let arguments = json!({"cmd":script,"workdir":r"C:\synthetic\project","limit":3});
        for encoded in [false, true] {
            let call = json!({"call_id":"call_measure","arguments":if encoded {
                json!(arguments.to_string())
            } else {arguments.clone()}});
            let call = call.as_object().unwrap();
            let expected = transport_call(call, &tools[0]).unwrap();
            assert_eq!(
                cloned_replay(call, &tools[0]).to_string(),
                expected.to_string()
            );
            let mut legacy = Vec::new();
            let mut direct = Vec::new();
            for trial in 0..10 {
                for old in if trial % 2 == 0 {
                    [true, false]
                } else {
                    [false, true]
                } {
                    let started = Instant::now();
                    for _ in 0..10 {
                        if old {
                            black_box(cloned_replay(black_box(call), &tools[0]));
                        } else {
                            black_box(transport_call(black_box(call), &tools[0]).unwrap());
                        }
                    }
                    if trial > 0 {
                        if old {
                            legacy.push(started.elapsed().as_micros() / 10);
                        } else {
                            direct.push(started.elapsed().as_micros() / 10);
                        }
                    }
                }
            }
            legacy.sort_unstable();
            direct.sort_unstable();
            assert_eq!(transport_call(call, &tools[0]).unwrap(), expected);
            eprintln!(
                "bps-tool-history bytes={} json_string={encoded} warmup=1 paired_trials=9 iterations=10 legacy_median_us={} direct_median_us={}",
                script.len(),
                legacy[4],
                direct[4]
            );
        }
    }
}

#[test]
#[ignore = "manual synthetic BPS preparation and stable-prefix measurement"]
fn measure_bps_turn_prefix_reuse() {
    use std::{hint::black_box, time::Instant};

    for mode in ["compact", "discovery"] {
        let mut request = json!({
            "model":"synthetic-model", "relay_bps_tool_catalog":mode,
            "input":[
                {"role":"user","content":"Synthetic stable history. ".repeat(1024)},
                {"role":"assistant","content":"Synthetic previous answer."},
                {"role":"user","content":"Synthetic next turn."}
            ],
            "tools":(0..48).map(|index| json!({
                "type":"function","name":format!("tool_{index:02}"),
                "description":"Synthetic tool contract.",
                "parameters":{"type":"object","properties":{"text":{"type":"string"}},
                    "required":["text"],"additionalProperties":false}
            })).collect::<Vec<_>>()
        });
        let baseline = prepare_request(&request).unwrap();
        let prefix = baseline["input"].as_array().unwrap();
        let choices = [
            ("auto", json!("auto")),
            ("none", json!("none")),
            ("named", json!({"type":"function","name":"tool_00"})),
            (
                "allowed",
                json!({"type":"allowed_tools","mode":"auto",
                "tools":[{"type":"function","name":"tool_00"}]}),
            ),
        ];
        for (choice_name, choice) in choices {
            request["tool_choice"] = choice;
            let prepared = prepare_request(&request).unwrap();
            let shared = prefix
                .iter()
                .zip(prepared["input"].as_array().unwrap())
                .take_while(|(before, after)| before == after)
                .count();
            let shared_bytes = if shared == 0 {
                0
            } else {
                serde_json::to_vec(&prefix[..shared]).unwrap().len()
            };
            for _ in 0..5 {
                black_box(prepare_request(black_box(&request)).unwrap());
            }
            let mut samples = Vec::new();
            for _ in 0..7 {
                let started = Instant::now();
                for _ in 0..25 {
                    black_box(prepare_request(black_box(&request)).unwrap());
                }
                samples.push(started.elapsed().as_nanos() / 25);
            }
            samples.sort_unstable();
            eprintln!(
                "bps mode={mode} choice={choice_name} warmup=5 trials=7 iterations=25 shared_items={shared}/{} shared_bytes={shared_bytes} median_ns={} min_ns={} max_ns={}",
                prefix.len(),
                samples[3],
                samples[0],
                samples[6]
            );
        }
    }
}

#[test]
#[ignore = "manual synthetic large-history preparation; no provider or TTFT measurement"]
fn measure_bps_large_history_preparation() {
    use std::{hint::black_box, time::Instant};

    for size in [4096_usize, 256 * 1024, 1024 * 1024] {
        let seed = "Synthetic coding history: Привет, \"quoted\" \\path\r\n";
        let text = seed.repeat(size.div_ceil(seed.len()));
        let request = json!({"model":"synthetic-model","input":[
            {"role":"user","content":text},
            {"type":"reasoning","id":"rs_synthetic",
                "summary":[{"type":"summary_text","text":text}]},
            {"type":"function_call","call_id":"call_read","name":"read_file",
                "arguments":"{\"path\":\"synthetic.rs\"}"},
            {"type":"function_call_output","call_id":"call_read","output":text},
            {"role":"user","content":text}
        ],"tools":[{"type":"function","name":"read_file",
            "parameters":{"type":"object","properties":{"path":{"type":"string"}},
                "required":["path"]}}]});
        let original = serde_json::to_vec(&request).unwrap();
        let expected = serde_json::to_vec(&prepare_request(&request).unwrap()).unwrap();
        for _ in 0..5 {
            black_box(prepare_request(black_box(&request)).unwrap());
        }
        let mut samples = Vec::new();
        for _ in 0..9 {
            let started = Instant::now();
            for _ in 0..10 {
                black_box(prepare_request(black_box(&request)).unwrap());
            }
            samples.push(started.elapsed().as_nanos() / 10);
        }
        samples.sort_unstable();
        assert_eq!(serde_json::to_vec(&request).unwrap(), original);
        assert_eq!(
            serde_json::to_vec(&prepare_request(&request).unwrap()).unwrap(),
            expected
        );
        eprintln!(
            "bps-large-history input_bytes={} output_bytes={} warmup=5 trials=9 iterations=10 median_ns={} min_ns={} max_ns={}",
            original.len(),
            expected.len(),
            samples[4],
            samples[0],
            samples[8]
        );
    }
}
