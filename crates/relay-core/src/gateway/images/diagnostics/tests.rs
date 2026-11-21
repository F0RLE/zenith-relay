use super::*;
use crate::usage::{ImageDimensions, ImageHeaderStatus};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;

fn png(width: u32, height: u32) -> String {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    STANDARD.encode(bytes)
}

fn options(payload: Value) -> ImageOptionsDiagnostics {
    let bytes = serde_json::to_vec(&payload).unwrap();
    serde_json::from_slice::<Observation<'_>>(&bytes)
        .unwrap()
        .options()
}

#[test]
fn image_diagnostics_requested_model_does_not_use_the_resolved_alias() {
    let prepared = PreparedImageRequest {
        requested_model: "gpt-image-2.5-sunburst".into(),
        resolved_model: "gpt-image-2.6-flare".into(),
        fields: serde_json::from_value(json!({"model":"gpt-image-2.6-flare","quality":"high"}))
            .unwrap(),
        input_images: Vec::new(),
        mask_image: None,
        raw_body: Bytes::new(),
        content_type: axum::http::HeaderValue::from_static("application/json"),
        stream: false,
        response_format: "b64_json".into(),
        client_context_id: None,
    };
    assert_eq!(
        requested_options(&prepared).model.as_deref(),
        Some("gpt-image-2.5-sunburst")
    );
}

#[test]
fn image_diagnostics_separate_requested_sent_reported_and_header_dimensions() {
    let requested = options(
        json!({"model":"gpt-image-2.6-sunburst","size":"3840x2160","quality":"max","complexity":"high","n":1,"output_format":"png"}),
    );
    let mut diagnostics = ImageDiagnostics {
        requested: requested.clone(),
        sent: Some(requested),
        ..Default::default()
    };
    let response = serde_json::to_vec(&json!({
        "model":"gpt-image-2-codex", "size":"auto", "quality":"auto",
        "data":[{"b64_json":png(1672, 941), "size":"1672x941", "quality":"high"}]
    }))
    .unwrap();
    let original = response.clone();
    inspect_response(&mut diagnostics, &response);
    assert_eq!(response, original);
    assert_eq!(diagnostics.requested.size.as_deref(), Some("3840x2160"));
    assert_eq!(
        diagnostics.sent.as_ref().unwrap().quality.as_deref(),
        Some("max")
    );
    assert_eq!(diagnostics.reported[0].size.as_deref(), Some("auto"));
    assert_eq!(
        diagnostics.reported[0].model.as_deref(),
        Some("gpt-image-2-codex")
    );
    let output = &diagnostics.outputs[0];
    assert_eq!(output.reported.quality.as_deref(), Some("high"));
    assert_eq!(
        output.dimensions,
        Some(ImageDimensions {
            width: 1672,
            height: 941
        })
    );
    assert_eq!(output.sent_size_matches, Some(false));
    assert_eq!(output.detected_format.as_deref(), Some("png"));
}

#[test]
fn image_diagnostics_unknown_or_invalid_metadata_is_not_inferred() {
    for size in [None, Some("auto")] {
        let mut diagnostics = ImageDiagnostics {
            sent: Some(ImageOptionsDiagnostics {
                size: size.map(str::to_owned),
                ..Default::default()
            }),
            ..Default::default()
        };
        let response = serde_json::to_vec(&json!({"quality":{"private":"synthetic-secret"}, "model":"synthetic-secret", "output_format":42, "data":[{"b64_json":png(1254,1254), "url":"https://example.invalid/private", "revised_prompt":"synthetic-secret"}]})).unwrap();
        inspect_response(&mut diagnostics, &response);
        assert_eq!(
            diagnostics.reported,
            vec![ImageOptionsDiagnostics::default()]
        );
        assert_eq!(diagnostics.outputs[0].sent_size_matches, None);
        assert_eq!(
            diagnostics.outputs[0].reported,
            ImageOptionsDiagnostics::default()
        );
        let serialized = serde_json::to_string(&diagnostics).unwrap();
        let exported = serde_json::to_value(&diagnostics).unwrap();
        assert!(exported["outputs"][0].get("reported").is_none());
        let restored: ImageDiagnostics = serde_json::from_value(exported).unwrap();
        assert_eq!(restored, diagnostics);
        for forbidden in [
            "synthetic-secret",
            "example.invalid",
            "b64_json",
            "revised_prompt",
        ] {
            assert!(!serialized.contains(forbidden));
        }
    }
}

#[test]
fn image_diagnostics_do_not_fetch_urls_and_cap_output_metadata() {
    let mut diagnostics = ImageDiagnostics::default();
    let response = serde_json::to_vec(
        &json!({"data":vec![json!({"url":"https://example.invalid/private"}); 20]}),
    )
    .unwrap();
    inspect_response(&mut diagnostics, &response);
    assert_eq!(diagnostics.outputs.len(), 10);
    assert!(diagnostics.truncated);
    assert!(diagnostics.outputs.iter().all(|output| output.header_status
        == ImageHeaderStatus::UrlNotFetched
        && output.dimensions.is_none()));
    for _ in 0..20 {
        inspect_response(&mut diagnostics, br#"{"b64_json":"bad"}"#);
    }
    assert_eq!(diagnostics.reported.len(), 10);
    assert_eq!(diagnostics.outputs.len(), 10);
}

#[tokio::test]
async fn image_diagnostics_read_final_multipart_options_without_file_or_prompt_data() {
    let body = Bytes::from_static(b"--test\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngpt-image-2.6-flare\r\n--test\r\nContent-Disposition: form-data; name=\"size\"\r\n\r\n3840x2160\r\n--test\r\nContent-Disposition: form-data; name=\"n\"\r\n\r\n2\r\n--test\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nsynthetic-private-prompt\r\n--test\r\nContent-Disposition: form-data; name=\"image\"; filename=\"private.png\"\r\n\r\nsynthetic-private-file\r\n--test--\r\n");
    let result = multipart_options(&body, "multipart/form-data; boundary=test")
        .await
        .unwrap();
    assert_eq!(result.model.as_deref(), Some("gpt-image-2.6-flare"));
    assert_eq!(result.size.as_deref(), Some("3840x2160"));
    assert_eq!(result.n, Some(2));
    assert!(!serde_json::to_string(&result).unwrap().contains("private"));
}

#[test]
fn image_diagnostics_probe_formats_and_escaped_base64() {
    let fixtures = [
        (b"GIF89a\x80\x02\xe0\x01".to_vec(), "gif"),
        (
            b"\xff\xd8\xff\xc0\0\x08\x08\x01\xe0\x02\x80\0".to_vec(),
            "jpeg",
        ),
        (
            b"RIFF\x16\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0\x7f\x02\0\xdf\x01\0".to_vec(),
            "webp",
        ),
        (
            b"RIFF\x12\0\0\0WEBPVP8 \x0a\0\0\0\0\0\0\x9d\x01\x2a\x80\x02\xe0\x01".to_vec(),
            "webp",
        ),
        (
            {
                let packed = (639_u32 | (479 << 14)).to_le_bytes();
                let mut bytes = b"RIFF\x12\0\0\0WEBPVP8L\x05\0\0\0\x2f".to_vec();
                bytes.extend(packed);
                bytes
            },
            "webp",
        ),
    ];
    for (bytes, expected_format) in fixtures {
        let text = serde_json::to_string(&STANDARD.encode(bytes))
            .unwrap()
            .replace('/', "\\u002f");
        let raw = RawValue::from_string(text).unwrap();
        let (status, format, dimensions) = headers::inspect(Some(&raw), None);
        assert_eq!(status, ImageHeaderStatus::Observed);
        assert_eq!(format, Some(expected_format));
        assert_eq!(
            dimensions,
            Some(ImageDimensions {
                width: 640,
                height: 480
            })
        );
    }
}

#[test]
fn image_diagnostics_probe_is_bounded_and_never_requires_valid_image_tail() {
    let mut bytes = STANDARD.decode(png(1672, 941)).unwrap();
    bytes.resize(100_000, 0);
    let encoded = STANDARD.encode(&bytes);
    let raw = RawValue::from_string(format!("\"{encoded}not-base64-at-the-tail!\"")).unwrap();
    assert_eq!(
        headers::inspect(Some(&raw), None).0,
        ImageHeaderStatus::Observed
    );
    for raw in ["\"!invalid!\"", "\"a\"", "42", "{}"] {
        let raw = RawValue::from_string(raw.into()).unwrap();
        assert_eq!(
            headers::inspect(Some(&raw), None).0,
            ImageHeaderStatus::InvalidBase64
        );
    }
    let mut jpeg = b"\xff\xd8\xff\xe1\xff\xff".to_vec();
    jpeg.resize(65539, 0);
    jpeg.extend_from_slice(b"\xff\xc0\0\x08\x08\x01\xe0\x02\x80\0");
    let raw =
        RawValue::from_string(serde_json::to_string(&STANDARD.encode(jpeg)).unwrap()).unwrap();
    let (status, format, dimensions) = headers::inspect(Some(&raw), None);
    assert_eq!(status, ImageHeaderStatus::UnknownOrIncomplete);
    assert_eq!(format, Some("jpeg"));
    assert_eq!(dimensions, None);
}

#[test]
fn image_diagnostics_export_reapplies_allowlist_to_untrusted_history() {
    let unsafe_options = ImageOptionsDiagnostics {
        model: Some("synthetic-private".into()),
        size: Some("synthetic-private".into()),
        quality: Some("synthetic-private".into()),
        complexity: Some("synthetic-private".into()),
        output_format: Some("synthetic-private".into()),
        n: Some(255),
    };
    let diagnostics = ImageDiagnostics {
        requested: unsafe_options.clone(),
        sent: Some(unsafe_options.clone()),
        reported: vec![unsafe_options; 20],
        ..Default::default()
    }
    .sanitized();
    assert_eq!(diagnostics.requested, ImageOptionsDiagnostics::default());
    assert_eq!(diagnostics.reported.len(), 10);
    assert!(diagnostics.truncated);
    assert!(
        !serde_json::to_string(&diagnostics)
            .unwrap()
            .contains("synthetic-private")
    );
}
