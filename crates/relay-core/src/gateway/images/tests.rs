use super::account::{direct_account_request_body, direct_request_body, image_endpoint_url};
use super::*;
use serde_json::json;

fn request() -> PreparedImageRequest {
    PreparedImageRequest {
        requested_model: "gpt-image-test".into(), resolved_model: "gpt-image-test".into(),
        fields: serde_json::from_value(json!({"model":"gpt-image-test", "prompt":"synthetic", "quality":"low", "response_format":"url", "future_option":true})).unwrap(),
        input_images: vec!["data:image/png;base64,YQ==".into(), "data:image/png;base64,Yg==".into()],
        mask_image: Some("data:image/png;base64,bWFzaw==".into()), raw_body: Bytes::new(),
        content_type: HeaderValue::from_static("application/json"), stream: false,
        response_format: "url".into(), client_context_id: None,
    }
}

#[test]
fn image_capability_requires_an_explicit_error_not_echoed_request_fields() {
    use super::account::image_capability_unavailable;
    for error in [
        json!({"code":"image_generation_not_enabled"}),
        json!({"type":"image_generation_not_enabled"}),
        json!({"message":"Image generation is not enabled for this account."}),
        json!("Image generation is not enabled"),
    ] {
        assert!(image_capability_unavailable(
            &serde_json::to_vec(&json!({"error":error})).unwrap()
        ));
    }
    for payload in [
        json!({"prompt":"image generation is not enabled", "error":{"message":"Invalid size"}}),
        json!({"metadata":{"code":"image_generation_not_enabled"}}),
        json!({"error":{"message":"Invalid prompt: image generation is not enabled"}}),
        json!({"error":{"code":"not_image_generation_not_enabled"}}),
    ] {
        assert!(!image_capability_unavailable(
            &serde_json::to_vec(&payload).unwrap()
        ));
    }
    assert!(!image_capability_unavailable(
        b"invalid JSON image generation is not enabled"
    ));
}

#[test]
fn image_execution_evidence_includes_results_and_svg_authoring() {
    for payload in [
        json!({"data":[{"b64_json":"aW1hZ2U="}]}),
        json!({"data":[{"url":"https://example.invalid/image"}]}),
        json!({"b64_json":"aW1hZ2U="}),
        json!({"usage":{"output_tokens":1}}),
        json!({"svg_usage":{"output_tokens":1}}),
        json!({"svg_usage":{"reasoning_tokens":1}}),
    ] {
        let bytes = serde_json::to_vec(&payload).unwrap();
        let envelope: native::ImageEnvelope<'_> = serde_json::from_slice(&bytes).unwrap();
        assert!(envelope.has_generation_evidence(), "{payload}");
    }
    for payload in [
        json!({"data":[{"b64_json":" \n", "url":" "}]}),
        json!({"usage":{"output_tokens":0}, "svg_usage":{"output_tokens":0}}),
        json!({"error":{"message":"Image generation is not enabled"}}),
    ] {
        let bytes = serde_json::to_vec(&payload).unwrap();
        let envelope: native::ImageEnvelope<'_> = serde_json::from_slice(&bytes).unwrap();
        assert!(!envelope.has_generation_evidence(), "{payload}");
    }
}

#[test]
fn svg_cache_observations_remain_rendering_scoped_in_either_frame_order() {
    let rendering = json!({"usage":{"input_tokens":10,"output_tokens":6,
        "input_tokens_details":{"cached_tokens":8}}});
    let svg =
        json!({"svg_usage":{"model":"synthetic-svg-author","input_tokens":30,"output_tokens":5}});
    for svg_first in [false, true] {
        let mut state = native::ImageUsageState::default();
        let mut event = crate::gateway::test_support::test_usage_event();
        event.image_usage = None;
        event.cached_input_tokens = None;
        let frames = if svg_first {
            [svg.clone(), rendering.clone()]
        } else {
            [rendering.clone(), svg.clone()]
        };
        for (index, frame) in frames
            .into_iter()
            .chain([rendering.clone(), svg.clone()])
            .enumerate()
        {
            let bytes = serde_json::to_vec(&frame).unwrap();
            let envelope: native::ImageEnvelope<'_> = serde_json::from_slice(&bytes).unwrap();
            state.observe(&mut event, &envelope);
            if index == 0 && !svg_first {
                assert_eq!(event.cached_input_tokens, Some(8));
            } else {
                assert_eq!(event.cached_input_tokens, None);
            }
        }
        assert_eq!(event.input_tokens, Some(40));
        assert_eq!(
            event.image_usage.as_ref().unwrap().cached_input_tokens,
            Some(8)
        );
    }
}

#[test]
fn native_account_images_do_not_invoke_a_text_model() {
    let request = request();
    for endpoint in [ImageEndpoint::Edits, ImageEndpoint::Generations] {
        let body: Value = serde_json::from_slice(&direct_account_request_body(
            &request,
            endpoint,
            "gpt-image-test",
        ))
        .unwrap();
        assert_eq!(body["model"], "gpt-image-test");
        assert_eq!(body["quality"], "low");
        assert_eq!(body["future_option"], true);
        for field in ["tools", "reasoning", "response_format", "size"] {
            assert!(body.get(field).is_none());
        }
        if endpoint == ImageEndpoint::Edits {
            assert_eq!(body["images"][0]["image_url"], request.input_images[0]);
            assert_eq!(body["images"][1]["image_url"], request.input_images[1]);
            assert_eq!(
                body["mask"]["image_url"],
                request.mask_image.as_deref().unwrap()
            );
        }
    }
    let url = image_endpoint_url(
        url::Url::parse("https://example.invalid/backend-api/codex/responses").unwrap(),
        ImageEndpoint::Edits,
    )
    .unwrap();
    assert_eq!(url.path(), "/backend-api/codex/images/edits");
}

#[test]
fn direct_provider_request_uses_canonical_source_model() {
    let body: Value =
        serde_json::from_slice(&direct_request_body(&request(), "gpt-image-canonical")).unwrap();
    assert_eq!(body["model"], "gpt-image-canonical");
    assert!(body.get("response_format").is_none());
}

#[test]
fn native_image_url_compatibility_preserves_usage_and_output_format() {
    let bytes = serde_json::to_vec(&json!({"output_format":"webp", "data":[{"b64_json":"aW1hZ2U="}], "usage":{"output_tokens":5}})).unwrap();
    let result: Value =
        serde_json::from_slice(&native::normalize_response(bytes, "url", None)).unwrap();
    assert_eq!(result["data"][0]["url"], "data:image/webp;base64,aW1hZ2U=");
    assert!(result["data"][0].get("b64_json").is_none());
    assert_eq!(result["usage"]["output_tokens"], 5);
}

#[test]
fn native_image_url_formats_follow_output_metadata_then_request() {
    for (format, mime) in [
        ("png", "image/png"),
        ("jpeg", "image/jpeg"),
        ("webp", "image/webp"),
        ("svg", "image/svg+xml"),
        ("gif", "image/gif"),
        ("future-format", "application/octet-stream"),
    ] {
        for location in ["item", "envelope", "request"] {
            let mut payload = json!({"data": [{"b64_json": "aW1hZ2U=", "generation_id": "synthetic"}], "usage": {"output_tokens": 5}});
            if location == "item" {
                payload["data"][0]["output_format"] = json!(format);
                payload["output_format"] = json!("png");
            } else if location == "envelope" {
                payload["output_format"] = json!(format);
            }
            let requested = if location == "request" {
                format
            } else {
                "jpeg"
            };
            let bytes = serde_json::to_vec(&payload).unwrap();
            assert_eq!(
                native::normalize_response(bytes.clone(), "b64_json", Some(requested)),
                bytes
            );
            let result: Value =
                serde_json::from_slice(&native::normalize_response(bytes, "url", Some(requested)))
                    .unwrap();
            assert_eq!(
                result["data"][0]["url"],
                format!("data:{mime};base64,aW1hZ2U="),
                "{format} {location}"
            );
            assert!(result["data"][0].get("b64_json").is_none());
            assert_eq!(result["data"][0]["generation_id"], "synthetic");
            assert_eq!(result["usage"], payload["usage"]);
        }
    }
    let bytes =
        br#"{"data":[{"b64_json":"aW1hZ2U="},{"url":"https://example.invalid/image"}]}"#.to_vec();
    let result: Value =
        serde_json::from_slice(&native::normalize_response(bytes, "url", None)).unwrap();
    assert_eq!(result["data"][0]["url"], "data:image/png;base64,aW1hZ2U=");
    assert_eq!(result["data"][1]["url"], "https://example.invalid/image");
}

#[tokio::test]
async fn multipart_edit_parses_multiple_images_and_mask() {
    let boundary = "zenith-test-boundary";
    let bytes = Bytes::from(format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nedit\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"image[]\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\nPNG-A\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"image[]\"; filename=\"b.png\"\r\nContent-Type: image/png\r\n\r\nPNG-B\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"mask\"; filename=\"mask.png\"\r\nContent-Type: image/png\r\n\r\nMASK\r\n--{boundary}--\r\n"
    ));
    let (fields, images, mask) =
        prepare::parse_multipart(&format!("multipart/form-data; boundary={boundary}"), bytes)
            .await
            .unwrap();
    assert_eq!(fields["prompt"], "edit");
    assert_eq!(images.len(), 2);
    assert!(
        images
            .iter()
            .all(|image| image.starts_with("data:image/png;base64,"))
    );
    assert!(mask.unwrap().starts_with("data:image/png;base64,"));
}

#[tokio::test]
async fn source_multipart_rewrites_alias_without_changing_file_bytes() {
    let mut request = request();
    request.content_type = HeaderValue::from_static("multipart/form-data; boundary=synthetic");
    request.raw_body = Bytes::from_static(b"--synthetic\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\npublic/gpt-image-test\r\n--synthetic\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\nurl\r\n--synthetic\r\nContent-Disposition: form-data; name=\"image\"; filename=\"image.png\"\r\nContent-Type: image/png\r\n\r\n\x89PNG\0\xff\r\n--synthetic--\r\n");
    let bytes = multipart::source_body(&request, "gpt-image-canonical")
        .await
        .unwrap();
    let (fields, images, _) =
        prepare::parse_multipart(request.content_type.to_str().unwrap(), Bytes::from(bytes))
            .await
            .unwrap();
    assert_eq!(fields["model"], "gpt-image-canonical");
    assert!(fields.get("response_format").is_none());
    assert_eq!(images, ["data:image/png;base64,iVBORwD/"]);
}

#[test]
fn native_image_json_allows_escaped_base64_characters() {
    let envelope: native::ImageEnvelope<'_> =
        serde_json::from_str(r#"{"data":[{"b64_json":"a\/b"}]}"#).unwrap();
    assert_eq!(envelope.output_count(), 1);
}

#[test]
fn image_svg_accounting_merges_snapshots_without_double_counting() {
    let mut state = native::ImageUsageState::default();
    let mut event = crate::gateway::test_support::test_usage_event();
    event.image_usage = None;
    let observe =
        |state: &mut native::ImageUsageState, event: &mut crate::UsageEvent, value: Value| {
            let bytes = serde_json::to_vec(&value).unwrap();
            let envelope: native::ImageEnvelope<'_> = serde_json::from_slice(&bytes).unwrap();
            state.observe(event, &envelope);
        };
    observe(
        &mut state,
        &mut event,
        json!({
            "usage":{"input_tokens":10},
            "svg_usage":{"model":"synthetic-svg-author","input_tokens":3}
        }),
    );
    assert_eq!(event.input_tokens, Some(13));
    assert_eq!(event.output_tokens, None);
    assert_eq!(event.total_tokens, None);
    let complete = json!({
        "usage":{"output_tokens":6,"total_tokens":16,"output_tokens_details":{"image_tokens":4,"text_tokens":2}},
        "svg_usage":{"output_tokens":5,"reasoning_tokens":2}
    });
    for value in [
        complete.clone(),
        complete,
        json!({"svg_usage":{}}),
        json!({"usage":{}}),
    ] {
        observe(&mut state, &mut event, value);
        assert_eq!(event.input_tokens, Some(13));
        assert_eq!(event.output_tokens, Some(11));
        assert_eq!(event.total_tokens, Some(24));
        assert_eq!(event.reasoning_tokens, Some(2));
    }
    let image = event.image_usage.as_ref().unwrap();
    assert_eq!(image.output_image_tokens, Some(4));
    assert_eq!(image.output_text_tokens, Some(2));
    assert_eq!(
        image.svg_usage.as_ref().unwrap().model.as_deref(),
        Some("synthetic-svg-author")
    );
    observe(
        &mut state,
        &mut event,
        json!({"usage":{"input_tokens":u64::MAX,"output_tokens":6},"svg_usage":{"input_tokens":3}}),
    );
    assert_eq!(event.input_tokens, None);
    assert_eq!(event.total_tokens, None);
}

#[test]
fn svg_only_usage_remains_observed_without_inventing_rendering_cost() {
    let bytes = br#"{"svg_usage":{"model":"synthetic-svg-author","input_tokens":3,"output_tokens":5,"reasoning_tokens":2,"total_tokens":8}}"#;
    let envelope: native::ImageEnvelope<'_> = serde_json::from_slice(bytes).unwrap();
    let mut event = crate::gateway::test_support::test_usage_event();
    event.image_usage = None;
    envelope.observe_usage(&mut event);
    assert_eq!(event.input_tokens, None);
    assert_eq!(event.output_tokens, None);
    assert_eq!(event.total_tokens, None);
    assert_eq!(
        event.image_usage.unwrap().svg_usage.unwrap().total_tokens,
        Some(8)
    );
}

#[test]
fn image_url_results_are_not_hidden_or_overwritten_by_empty_base64() {
    let payload = json!({"data":[
        {"b64_json":"", "url":"https://example.invalid/image"},
        {"b64_json":"   ", "url":"https://example.invalid/second"},
        {"b64_json":"aW1hZ2U=", "url":"https://example.invalid/preferred"},
        {"b64_json":"aW1hZ2U=", "url":""},
        {"b64_json":" \n ", "url":"  "}
    ]});
    let bytes = serde_json::to_vec(&payload).unwrap();
    let envelope: native::ImageEnvelope<'_> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(envelope.output_count(), 4);
    let normalized: Value =
        serde_json::from_slice(&native::normalize_response(bytes, "url", None)).unwrap();
    for index in 0..3 {
        assert_eq!(normalized["data"][index], payload["data"][index]);
    }
    assert_eq!(
        normalized["data"][3]["url"],
        "data:image/png;base64,aW1hZ2U="
    );
    assert_eq!(normalized["data"][4], payload["data"][4]);
}
