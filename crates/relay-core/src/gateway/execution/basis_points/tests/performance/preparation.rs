use super::dispatch_runtime;
use crate::gateway::execution::basis_points::{
    PreparedClientRequest, attach_input_images,
    discovery::{capture_prepared_context, execution},
};
use crate::scheduler::rotation::SharedRequestBudget;
use crate::usage::{RequestContextSize, ToolUseDiagnostics};
use serde_json::{Value, json};
use std::{hint::black_box, time::Instant};

fn synthetic_request() -> Value {
    let tools = (0..313)
        .map(|index| {
            json!({"type":"function","name":format!("tool_{index}"),
            "description":"Synthetic description. ".repeat(16),
            "parameters":{"type":"object","properties":{"path":{"type":"string"}},
                "required":["path"],"additionalProperties":false}})
        })
        .collect::<Vec<_>>();
    let text = "Synthetic history: Привет, \"quoted\" \\path\n".repeat(192);
    let mut input = Vec::new();
    for index in 0..32 {
        let call_id = format!("call_synthetic_{index}");
        input.extend([
            json!({"type":"function_call","call_id":call_id,"name":format!("tool_{}",index%3),
                "arguments":"{\"path\":\"synthetic.rs\"}"}),
            json!({"type":"function_call_output","call_id":call_id,"output":text}),
            json!({"type":"reasoning","id":format!("rs_synthetic_{index}"),
                "summary":[{"type":"summary_text","text":text}]}),
            json!({"role":"user","content":text}),
        ]);
    }
    input.push(json!({"role":"user","content":[
        {"type":"input_image","file_id":"file-synthetic-existing","detail":"high"}
    ]}));
    json!({"model":"synthetic-model","relay_bps_tool_catalog":"discovery",
        "input":input,"tools":tools})
}

#[tokio::test]
#[ignore = "synthetic local preparation; excludes admission, credentials, uploads and provider latency"]
async fn measure_bps_local_preparation_phases() {
    let runtime = dispatch_runtime();
    let request = synthetic_request();
    let original = serde_json::to_vec(&request).unwrap();
    let endpoint = url::Url::parse("https://example.test/v1/responses").unwrap();
    let headers = reqwest::header::HeaderMap::new();
    let mut expected = None;
    let mut samples = [Vec::<[u128; 6]>::new(), Vec::new()];
    for trial in 0..13 {
        for diagnostics in if trial % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            // Fixture cloning and admission-time context accounting are excluded.
            let input = request.clone();
            let mut tool_use = ToolUseDiagnostics {
                client_context: diagnostics.then(|| RequestContextSize::capture(&input)),
                ..Default::default()
            };
            let budget = SharedRequestBudget::for_incoming_request(8);
            let mut schema_ms = None;
            let started = Instant::now();
            let (prepared, body) =
                PreparedClientRequest::prepare(input, &mut schema_ms, diagnostics).unwrap();
            let body_done = Instant::now();
            let (mut body, _) = execution::prepare_dispatch_with_catalog(
                (&prepared, prepared.catalog_snapshot()),
                body,
                &budget,
                &mut tool_use,
                &runtime,
                "synthetic-key",
                None,
            )
            .unwrap();
            let catalog_done = Instant::now();
            assert!(
                attach_input_images(&runtime, "synthetic-source", &endpoint, &headers, &mut body)
                    .await
                    .is_ok()
            );
            let attachments_done = Instant::now();
            capture_prepared_context(&prepared, &body, &mut tool_use);
            let context_done = Instant::now();
            let encoded = black_box(serde_json::to_vec(&body).unwrap());
            let encoded_done = Instant::now();
            let timing = [
                body_done.duration_since(started).as_nanos(),
                catalog_done.duration_since(body_done).as_nanos(),
                attachments_done.duration_since(catalog_done).as_nanos(),
                context_done.duration_since(attachments_done).as_nanos(),
                encoded_done.duration_since(context_done).as_nanos(),
                encoded_done.duration_since(started).as_nanos(),
            ];
            assert_eq!(schema_ms.is_some(), diagnostics);
            assert_eq!(tool_use.upstream_context.is_some(), diagnostics);
            assert_eq!(expected.get_or_insert_with(|| encoded.clone()), &encoded);
            if let Some(context) = tool_use.upstream_context {
                assert_eq!(context.body_bytes, encoded.len() as u64);
                assert_eq!(context.conversation.unwrap().attachment_count, 1);
            }
            if trial >= 3 {
                samples[usize::from(diagnostics)].push(timing);
            }
        }
    }
    assert_eq!(serde_json::to_vec(&request).unwrap(), original);
    for (diagnostics, samples) in samples.into_iter().enumerate() {
        let medians: Vec<_> = (0..6)
            .map(|phase| {
                let mut values: Vec<_> = samples.iter().map(|sample| sample[phase]).collect();
                values.sort_unstable();
                (values[4] + values[5]) / 2
            })
            .collect();
        eprintln!(
            "bps-local-preparation debug_build={} diagnostics={} tools=313 input_bytes={} wire_bytes={} trials=10 phases_ns=[body,catalog,attachments,context,encoding,total] median_ns={medians:?}",
            cfg!(debug_assertions),
            diagnostics != 0,
            original.len(),
            expected.as_ref().unwrap().len()
        );
    }
}
