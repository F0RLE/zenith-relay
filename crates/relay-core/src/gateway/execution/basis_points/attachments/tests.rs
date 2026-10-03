use super::*;
use serde_json::json;

fn png_bytes() -> Vec<u8> {
    STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")
        .unwrap()
}

fn data_url(bytes: &[u8]) -> String {
    format!("data:image/png;base64,{}", STANDARD.encode(bytes))
}

fn failure_message(result: Result<Vec<LocatedJob>, AttachmentFailure>) -> &'static str {
    match result {
        Err(AttachmentFailure::Reject(failure)) => failure.message,
        Err(AttachmentFailure::Retry(failure)) => panic!("{}", failure.message),
        Ok(_) => panic!("expected a request rejection"),
    }
}

fn expect_jobs(result: Result<Vec<LocatedJob>, AttachmentFailure>) -> Vec<LocatedJob> {
    result.unwrap_or_else(|failure| match failure {
        AttachmentFailure::Reject(failure) | AttachmentFailure::Retry(failure) => {
            panic!("{}", failure.message)
        }
    })
}

fn expect_image(result: Result<InlineImage, AttachmentFailure>) -> InlineImage {
    result.unwrap_or_else(|failure| match failure {
        AttachmentFailure::Reject(failure) | AttachmentFailure::Retry(failure) => {
            panic!("{}", failure.message)
        }
    })
}

#[test]
fn user_data_url_is_staged_and_existing_file_id_drops_detail() {
    let png = png_bytes();
    let body = json!({
        "input": [
            {
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "look"},
                    {"type": "input_image", "image_url": data_url(&png), "detail": "high"}
                ]
            },
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_image", "file_id": "file-existing", "detail": "low"}]
            },
            {
                "role": "assistant",
                "content": [{"type": "input_image", "image_url": "https://example.com/a.png", "detail": "auto"}]
            }
        ]
    });
    let jobs = expect_jobs(stage_input_images(&body));
    assert!(matches!(jobs[0].job, ImageJob::Upload(_)));
    assert!(matches!(&jobs[1].job, ImageJob::File(file_id) if file_id == "file-existing"));
    assert_eq!(jobs.len(), 2);
    let mut rewritten = body;
    set_image_part(&mut rewritten, 1, 0, "file-existing");
    assert_eq!(
        rewritten["input"][1]["content"][0],
        json!({"type": "input_image", "file_id": "file-existing"})
    );
    assert_eq!(
        rewritten["input"][2]["content"][0]["image_url"],
        "https://example.com/a.png"
    );
}

#[test]
fn image_url_object_and_supported_signatures_decode() {
    let png = png_bytes();
    let body = json!({
        "input": [{
            "role": "user",
            "content": [{"type": "input_image", "image_url": {"url": data_url(&png), "detail": "auto"}, "detail": "low"}]
        }]
    });
    let jobs = expect_jobs(stage_input_images(&body));
    let ImageJob::Upload(image) = &jobs[0].job else {
        panic!("upload");
    };
    assert_eq!(image.media_type, "image/png");
    assert_eq!(image.filename, "image.png");
    assert_eq!(image.data, png);

    let jpeg = expect_image(decode_inline_image("data:image/jpg;base64,/9j/AA=="));
    assert_eq!(jpeg.filename, "image.jpeg");
    let gif = expect_image(decode_inline_image("data:image/gif;base64,R0lGODlh"));
    assert_eq!(gif.filename, "image.gif");
    let webp = expect_image(decode_inline_image(
        "data:image/webp;base64,UklGRgAAAABXRUJQ",
    ));
    assert_eq!(webp.media_type, "image/webp");
}

#[test]
fn conflicting_remote_and_unknown_images_are_rejected() {
    let both = json!({
        "input": [{"role": "user", "content": [{
            "type": "input_image",
            "file_id": "file-1",
            "image_url": "data:image/png;base64,AA=="
        }]}]
    });
    assert_eq!(
        failure_message(stage_input_images(&both)),
        "input_image cannot contain both image_url and file_id"
    );
    let remote = json!({
        "input": [{"role": "user", "content": [{
            "type": "input_image",
            "image_url": "https://example.com/a.png"
        }]}]
    });
    assert_eq!(
        failure_message(stage_input_images(&remote)),
        "input_image must be a data URL or file_id"
    );
    let text = json!({
        "input": [{"role": "user", "content": [{
            "type": "input_image",
            "image_url": "data:image/png;base64,aGVsbG8="
        }]}]
    });
    assert_eq!(
        failure_message(stage_input_images(&text)),
        "input_image must be PNG, JPEG, GIF, or WebP"
    );
}

#[test]
fn unicode_image_urls_are_rejected_without_panicking() {
    for url in ["🖼🖼", "图像图片", "data🖼"] {
        let input = json!({"input": [{"role": "user", "content": [{
            "type": "input_image", "image_url": url
        }]}]});
        assert_eq!(
            failure_message(stage_input_images(&input)),
            "input_image must be a data URL or file_id"
        );
    }
}

#[test]
fn attachment_url_replaces_the_responses_segment() {
    let responses = url::Url::parse("https://bps.openai.com/basispoints/api/responses").unwrap();
    assert_eq!(
        attachment_url(&responses).unwrap(),
        "https://bps.openai.com/basispoints/api/attachments"
    );
    let credentials =
        url::Url::parse("https://user:secret@bps.openai.com/basispoints/api/responses").unwrap();
    assert!(attachment_url(&credentials).is_err());
}

#[test]
fn multipart_names_the_detected_file_and_contains_only_the_image() {
    let image = expect_image(decode_inline_image(&format!(
        "DATA:image/png;BASE64,{}",
        STANDARD.encode(png_bytes())
    )));
    let (content_type, body) = multipart_body(&image);
    let header = "multipart/form-data; boundary=";
    assert!(content_type.starts_with(header));
    let boundary = content_type.trim_start_matches(header);
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("filename=\"image.png\""));
    assert!(text.contains("Content-Type: image/png"));
    assert!(text.contains(boundary));
    assert!(body
        .windows(png_bytes().len())
        .any(|window| window == png_bytes()));
    assert!(!text.contains("Bearer"));
}

#[test]
fn attachment_cache_reuses_the_file_id_without_storing_bytes() {
    let key = attachment_key(
        "https://bps.openai.com/attachments",
        "account",
        "image/png",
        b"png-bytes",
    );
    remember_file_id(key, "file-cached".to_string());
    assert_eq!(cached_file_id(&key).as_deref(), Some("file-cached"));
}
