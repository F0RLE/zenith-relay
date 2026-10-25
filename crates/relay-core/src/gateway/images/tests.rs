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
    let result: Value = serde_json::from_slice(&native::normalize_response(bytes, "url")).unwrap();
    assert_eq!(result["data"][0]["url"], "data:image/webp;base64,aW1hZ2U=");
    assert!(result["data"][0].get("b64_json").is_none());
    assert_eq!(result["usage"]["output_tokens"], 5);
}

#[tokio::test]
async fn multipart_edit_parses_multiple_images_and_mask() {
    let boundary = "zenith-test-boundary";
    let bytes = Bytes::from(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nedit\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"image[]\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\nPNG-A\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"image[]\"; filename=\"b.png\"\r\nContent-Type: image/png\r\n\r\nPNG-B\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"mask\"; filename=\"mask.png\"\r\nContent-Type: image/png\r\n\r\nMASK\r\n--{boundary}--\r\n"));
    let (fields, images, mask) =
        prepare::parse_multipart(&format!("multipart/form-data; boundary={boundary}"), bytes)
            .await
            .unwrap();
    assert_eq!(fields["prompt"], "edit");
    assert_eq!(images.len(), 2);
    assert!(images
        .iter()
        .all(|image| image.starts_with("data:image/png;base64,")));
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
