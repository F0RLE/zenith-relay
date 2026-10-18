use super::*;

#[test]
fn every_adapter_preserves_long_parallel_tool_chains_and_reversed_results() {
    let capabilities = crate::model_metadata::ModelCapabilities {
        reasoning: Some(false),
        ..Default::default()
    };
    for client in WireApi::ALL {
        for upstream in WireApi::ALL
            .into_iter()
            .filter(|upstream| *upstream != client)
        {
            for streaming in [false, true] {
                let adapter = SourceAdapter::between(client, upstream).unwrap();
                let mut request = input(client);
                request["tools"] = tool_history(client)["tools"].clone();
                let mut previous = None;
                for round in 0..=6 {
                    let prepared = adapter.prepare_request_with_capabilities(AdapterRequestContext {
                        client_wire_api: client, request: &request, model: "test", stream: streaming,
                        reasoning_mode: MessagesReasoningMode::Disabled,
                        cache_write_ttl: CacheWriteTtl::Provider, previous,
                        response_scope: "long-chain-test", response_id_seed: &format!("round-{round}"),
                    }, &capabilities).unwrap_or_else(|error| panic!("{client:?} -> {upstream:?}, stream={streaming}, round={round}: {error}"));
                    let mut decoded = decode::request(upstream, prepared.upstream_body()).unwrap();
                    decode::resolve_tool_history(&mut decoded.messages).unwrap();
                    let count = decoded
                        .messages
                        .iter()
                        .flat_map(|message| &message.blocks)
                        .filter(|block| matches!(block, Block::ToolResult { .. }))
                        .count();
                    assert_eq!(count, round * 2);
                    let final_round = round == 6;
                    let completed = if streaming {
                        let bytes = if final_round {
                            sse(upstream)
                        } else {
                            tool_stream(upstream)
                        };
                        let wire = String::from_utf8(bytes)
                            .unwrap()
                            .replace("call_new_", &format!("call_round_{round}_"));
                        let mut bridge = prepared.into_stream_bridge().unwrap();
                        for fragment in wire.as_bytes().chunks(3) {
                            bridge.push(fragment);
                        }
                        bridge.finish();
                        bridge
                            .completed()
                            .unwrap_or_else(|| panic!("{client:?} -> {upstream:?}, round={round}"))
                            .clone()
                    } else {
                        let payload = if final_round {
                            output(upstream)
                        } else {
                            super::architecture::tool_reply(upstream)
                        };
                        let payload = payload
                            .to_string()
                            .replace("call_new_", &format!("call_round_{round}_"));
                        prepared
                            .translate_response_bytes(payload.as_bytes())
                            .unwrap()
                            .unwrap()
                            .translated_response()
                            .clone()
                    };
                    if final_round {
                        assert!(response::decode(client, &completed.response_body, "test")
                            .unwrap()
                            .blocks
                            .iter()
                            .any(|block| matches!(block, Block::Text(text) if text == "Hi")));
                    } else {
                        request = super::architecture::append_tool_results(
                            client, request, &completed, true,
                        );
                    }
                    previous = Some(completed.continuation);
                }
            }
        }
    }
}
