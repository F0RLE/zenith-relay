use super::*;

#[tokio::test]
async fn native_interactions_preserves_tools_and_scopes_json_and_sse_continuations() {
    for first_stream in [false, true] {
        for next_stream in [false, true] {
            let observed = Arc::new(Mutex::new(Vec::new()));
            let captured = observed.clone();
            let upstream = spawn(Router::new().route("/v1beta/interactions", post(move |headers:HeaderMap, Json(body):Json<Value>| {
                assert_eq!(headers.get("x-goog-api-key").unwrap(), SOURCE_KEY);
                assert!(headers.get(AUTHORIZATION).is_none());
                captured.lock().unwrap().push(body.clone());
                async move {
                    let resumed = body.get("previous_interaction_id").is_some();
                    let id = if resumed {"interaction-next"} else {"interaction-first"};
                    let outputs = if resumed {json!([{"type":"text","text":"Synthetic final answer"}])} else {json!([
                        {"type":"function_call","id":"call-a","name":"lookup","arguments":{"value":1}},
                        {"type":"function_call","id":"call-b","name":"lookup","arguments":{"value":2}}
                    ])};
                    let status = if resumed {"completed"} else {"requires_action"};
                    let usage = json!({"total_input_tokens":3,"total_output_tokens":2,"total_thought_tokens":1,"total_cached_tokens":0,"total_tokens":6});
                    let response = json!({"id":id,"model":"interactions-test","status":status,"outputs":outputs,"usage":usage});
                    if body["stream"] == true {
                        let mut events = vec![json!({"event_type":"interaction.created","interaction":{"id":id,"status":"in_progress"}})];
                        for output in outputs.as_array().unwrap() {
                            events.push(json!({"event_type":"content.start","content":output}));
                        }
                        events.push(json!({"event_type":"interaction.completed","interaction":{"id":id,"status":status,"usage":usage}}));
                        Response::builder().header(CONTENT_TYPE,"text/event-stream").body(Body::from(events.iter().map(|event|format!("data: {event}\r\n\r\n")).collect::<String>())).unwrap()
                    } else { Json(response).into_response() }
                }
            }))).await;
            let events = Arc::new(Mutex::new(Vec::new()));
            let sink = events.clone();
            let source = ProviderSource {
                id: "native-interactions".into(),
                name: "Synthetic Interactions".into(),
                base_url: format!("{}/v1beta/interactions", upstream.base_url),
                api_key: SOURCE_KEY.into(),
                proxy_url: None,
                wire_api: WireApi::GeminiInteractions,
                models: vec!["interactions-test".into(), "other-model".into()],
            };
            let runtime = Arc::new(
                GatewayRuntime::from_pool(
                    vec![RuntimeSource::unrestricted(source)],
                    vec![
                        RuntimeLocalKey::unrestricted(LocalGatewayKey {
                            id: "local-a".into(),
                            secret: LOCAL_KEY.into(),
                        }),
                        RuntimeLocalKey::unrestricted(LocalGatewayKey {
                            id: "local-b".into(),
                            secret: "other-local-key".into(),
                        }),
                    ],
                    GatewayRuntimeOptions::default(),
                    Arc::new(move |event| sink.lock().unwrap().push(event)),
                )
                .unwrap(),
            );
            let gateway = spawn(gateway::router(runtime)).await;
            let client = reqwest::Client::new();
            let url = format!("{}/v1beta/interactions", gateway.base_url);
            let original = json!({"model":"interactions-test","input":"Synthetic task","stream":first_stream,
                "generation_config":{"seed":9007199254740993_u64},"tools":[{"type":"function","name":"lookup","parameters":{"type":"object","properties":{"value":{"type":"integer"}}}}]});
            let first = client
                .post(&url)
                .bearer_auth(LOCAL_KEY)
                .json(&original)
                .send()
                .await
                .unwrap();
            let status = first.status();
            let text = first.text().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{text}");
            assert!(
                text.contains("interaction-first"),
                "first_stream={first_stream} next_stream={next_stream} response={text}"
            );
            let next = json!({"model":"interactions-test","previous_interaction_id":"interaction-first","stream":next_stream,
                "input":[{"type":"function_result","call_id":"call-b","result":"second"},{"type":"function_result","call_id":"call-a","result":"first"}],"tools":original["tools"]});
            let response = client
                .post(&url)
                .bearer_auth(LOCAL_KEY)
                .json(&next)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let text = response.text().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{text}");
            assert!(text.contains("interaction-next"), "{text}");
            for (secret, mut invalid) in [
                ("other-local-key", next.clone()),
                (LOCAL_KEY, next.clone()),
                (
                    LOCAL_KEY,
                    json!({"model":"interactions-test","input":"x","background":true}),
                ),
            ] {
                if secret == LOCAL_KEY && invalid.get("previous_interaction_id").is_some() {
                    invalid["model"] = "other-model".into();
                }
                let response = client
                    .post(&url)
                    .bearer_auth(secret)
                    .json(&invalid)
                    .send()
                    .await
                    .unwrap();
                let status = response.status();
                let text = response.text().await.unwrap();
                assert!(
                    !status.is_success(),
                    "unexpected foreign/background continuation: first_stream={first_stream} next_stream={next_stream} model={} background={} foreign_key={} body={text}",
                    invalid["model"],
                    invalid["background"],
                    secret != LOCAL_KEY
                );
            }
            let requests = observed.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0]["input"], original["input"]);
            assert_eq!(requests[0]["tools"], original["tools"]);
            assert_eq!(
                requests[0]["generation_config"],
                original["generation_config"]
            );
            assert_eq!(requests[1]["input"], next["input"]);
            assert_eq!(requests[1]["previous_interaction_id"], "interaction-first");
            let ledger = events.lock().unwrap();
            let successes = ledger
                .iter()
                .filter(|event| event.success)
                .collect::<Vec<_>>();
            assert_eq!(successes.len(), 2, "{ledger:?}");
            assert!(
                successes
                    .iter()
                    .all(|event| event.wire_api == WireApi::GeminiInteractions
                        && event.input_tokens == Some(3)
                        && event.output_tokens == Some(3)
                        && event.reasoning_tokens == Some(1)
                        && event.cached_input_tokens == Some(0))
            );
        }
    }
}
