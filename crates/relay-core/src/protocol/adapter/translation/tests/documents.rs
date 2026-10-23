use super::*;

const PDF: &str = "JVBERi0xLjQK";

fn request(client: WireApi) -> Value {
    let data = format!("data:application/pdf;base64,{PDF}");
    match client {
        WireApi::Responses => {
            json!({"input":[{"role":"user","content":[{"type":"input_file","filename":"synthetic.pdf","file_data":data}]}]})
        }
        WireApi::ChatCompletions => {
            json!({"messages":[{"role":"user","content":[{"type":"file","file":{"filename":"synthetic.pdf","file_data":data}}]}]})
        }
        WireApi::Messages => {
            json!({"max_tokens":64,"messages":[{"role":"user","content":[{"type":"document","title":"synthetic.pdf","source":{"type":"base64","media_type":"application/pdf","data":PDF}}]}]})
        }
        WireApi::Gemini => {
            json!({"contents":[{"role":"user","parts":[{"inlineData":{"mimeType":"application/pdf","data":PDF,"displayName":"synthetic.pdf"}}]}]})
        }
    }
}

#[test]
fn pdf_documents_preserve_native_bytes_and_labels_across_all_twelve_conversions() {
    for client in WireApi::ALL {
        let original = request(client);
        let decoded = decode::request(client, &original).unwrap();
        assert!(
            matches!(&decoded.messages[0].blocks[0], Block::Document { filename: Some(filename), .. } if filename == "synthetic.pdf")
        );
        for upstream in WireApi::ALL
            .into_iter()
            .filter(|upstream| *upstream != client)
        {
            for stream in [false, true] {
                let prepared = prepare(client, upstream, &original, stream);
                let converted = decode::request(upstream, prepared.upstream_body()).unwrap();
                assert_eq!(
                    converted.messages, decoded.messages,
                    "{client:?} -> {upstream:?}"
                );
            }
        }
        assert_eq!(original, request(client));
    }
}

#[test]
fn pdf_urls_convert_only_without_fetching_or_relabeling_provider_file_ids() {
    let url = "https://example.invalid/synthetic.pdf";
    let source =
        json!({"input":[{"role":"user","content":[{"type":"input_file","file_url":url}]}]});
    let prepared = prepare(WireApi::Responses, WireApi::Messages, &source, false);
    assert_eq!(
        prepared.upstream_body()["messages"][0]["content"][0]["source"],
        json!({"type":"url","url":url})
    );
    let restored = prepare(
        WireApi::Messages,
        WireApi::Responses,
        prepared.upstream_body(),
        false,
    );
    assert_eq!(
        restored.upstream_body()["input"][0]["content"][0]["file_url"],
        url
    );
    for upstream in [WireApi::ChatCompletions, WireApi::Gemini] {
        assert_eq!(
            try_convert(WireApi::Responses, upstream, &source, false)
                .unwrap_err()
                .parameter(),
            Some("document.file_url")
        );
    }
    let mut file_id = source;
    file_id["input"][0]["content"][0] = json!({"type":"input_file","file_id":"synthetic-file-id"});
    for upstream in [WireApi::Messages, WireApi::ChatCompletions, WireApi::Gemini] {
        assert_eq!(
            try_convert(WireApi::Responses, upstream, &file_id, false)
                .unwrap_err()
                .parameter(),
            Some("input.file_id")
        );
    }
    assert!(prepare(WireApi::Responses, WireApi::Responses, &file_id, false).is_passthrough());
}

#[test]
fn pdf_rendering_and_citation_controls_are_not_silently_dropped() {
    for detail in ["low", "high"] {
        let mut source = request(WireApi::Responses);
        source["input"][0]["content"][0]["detail"] = detail.into();
        for upstream in [WireApi::Messages, WireApi::ChatCompletions, WireApi::Gemini] {
            assert_eq!(
                try_convert(WireApi::Responses, upstream, &source, false)
                    .unwrap_err()
                    .parameter(),
                Some("document.detail")
            );
        }
    }
    let mut source = request(WireApi::Messages);
    source["messages"][0]["content"][0]["citations"] = json!({"enabled":true});
    for upstream in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        assert_eq!(
            try_convert(WireApi::Messages, upstream, &source, false)
                .unwrap_err()
                .parameter(),
            Some("document.citations")
        );
    }
}

#[test]
fn pdf_tool_results_retain_their_call_identity_between_responses_and_messages() {
    let convert = |client, upstream, request: &Value| {
        SourceAdapter::between(client, upstream)
            .unwrap()
            .prepare_request_with_capabilities(
                AdapterRequestContext {
                    client_wire_api: client,
                    request,
                    model: "test",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: None,
                    response_scope: "document-test",
                    response_id_seed: "document-test",
                },
                &crate::model_metadata::ModelCapabilities {
                    reasoning: Some(false),
                    ..Default::default()
                },
            )
            .unwrap()
    };
    let source = json!({"tools":[{"type":"function","name":"read_pdf","parameters":{"type":"object"}}],"input":[
        {"role":"user","content":"Read the file"},
        {"type":"function_call","call_id":"call_pdf","name":"read_pdf","arguments":"{}"},
        {"type":"function_call_output","call_id":"call_pdf","output":[{"type":"input_file","filename":"synthetic.pdf","file_data":format!("data:application/pdf;base64,{PDF}")}]}
    ]});
    let prepared = convert(WireApi::Responses, WireApi::Messages, &source);
    let converted = decode::request(WireApi::Messages, prepared.upstream_body()).unwrap();
    assert!(
        matches!(&converted.messages.last().unwrap().blocks[0], Block::ToolResult { id, content, .. } if id == "call_pdf" && matches!(&content[0], Block::Document { .. }))
    );
    let restored = convert(
        WireApi::Messages,
        WireApi::Responses,
        prepared.upstream_body(),
    );
    let converted = decode::request(WireApi::Responses, restored.upstream_body()).unwrap();
    assert!(
        matches!(&converted.messages.last().unwrap().blocks[0], Block::ToolResult { id, content, .. } if id == "call_pdf" && matches!(&content[0], Block::Document { .. }))
    );
}

#[test]
fn gemini_native_cache_and_continuation_references_are_not_translated_to_foreign_owners() {
    for parameter in ["cachedContent", "continuationToken"] {
        let mut source = input(WireApi::Gemini);
        source[parameter] = if parameter == "cachedContent" {
            "cachedContents/synthetic"
        } else {
            "c3ludGhldGlj"
        }
        .into();
        let native = prepare(WireApi::Gemini, WireApi::Gemini, &source, true);
        assert!(native.is_passthrough());
        assert_eq!(native.upstream_body()[parameter], source[parameter]);
        for upstream in [
            WireApi::Responses,
            WireApi::Messages,
            WireApi::ChatCompletions,
        ] {
            assert_eq!(
                try_convert(WireApi::Gemini, upstream, &source, false)
                    .unwrap_err()
                    .parameter(),
                Some(parameter)
            );
        }
    }
}
