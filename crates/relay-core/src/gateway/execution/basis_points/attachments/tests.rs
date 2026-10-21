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
    assert_eq!(image.bytes, png);

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
fn tool_result_images_use_the_existing_attachment_validation() {
    use super::super::history::separate_tool_images;
    for output_kind in ["function_call_output", "custom_tool_call_output"] {
        let items = separate_tool_images(vec![
            json!({"type":output_kind,"call_id":"call_test","output":[
                {"type":"input_image","image_url":data_url(&png_bytes())},
                {"type":"input_image","file_id":"file-synthetic"}
            ]}),
        ]);
        let mut body = json!({"input":items});
        let jobs = expect_jobs(stage_input_images(&body));
        assert_eq!(jobs.len(), 2);
        assert!(matches!(jobs[0].job, ImageJob::Upload(_)));
        assert!(matches!(&jobs[1].job, ImageJob::File(id) if id == "file-synthetic"));
        body["input"][1]["content"][2]["image_url"] = json!("https://example.invalid/tool.png");
        assert!(matches!(
            stage_input_images(&body),
            Err(AttachmentFailure::Reject(_))
        ));
        body["input"][1]["content"][2]["image_url"] = json!("data:image/png;base64,not-an-image");
        assert!(matches!(
            stage_input_images(&body),
            Err(AttachmentFailure::Reject(_))
        ));
    }
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
        &[0; 32],
        "image/png",
        b"png-bytes",
    );
    remember_file_id(key, "file-cached".to_string());
    assert_eq!(cached_file_id(&key).as_deref(), Some("file-cached"));
}

#[tokio::test]
async fn concurrent_uploads_share_success_and_failure_without_caching_failures() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for succeeds in [false, true] {
        let key = attachment_key(
            "synthetic-concurrent",
            "account",
            &[0; 32],
            "image/png",
            &[u8::from(succeeds)],
        );
        let uploads = AtomicUsize::new(0);
        let release = tokio::sync::Notify::new();
        let mut pending = Box::pin(futures_util::future::join_all((0..8).map(|_| {
            cached_upload(key, || async {
                uploads.fetch_add(1, Ordering::SeqCst);
                release.notified().await;
                if succeeds {
                    Ok("file-shared".to_string())
                } else {
                    Err(upload_failed())
                }
            })
        })));
        assert!(futures_util::poll!(&mut pending).is_pending());
        assert_eq!(uploads.load(Ordering::SeqCst), 1);
        release.notify_one();
        let results = pending.await;
        assert_eq!(results.len(), 8);
        assert!(results.iter().all(|result| result.is_ok() == succeeds));
        if succeeds {
            assert!(results
                .iter()
                .all(|result| result.as_ref().is_ok_and(|id| id == "file-shared")));
        }
        let retried = cached_upload(key, || async {
            uploads.fetch_add(1, Ordering::SeqCst);
            Ok("file-explicit-retry".to_string())
        })
        .await;
        assert!(retried.is_ok());
        assert_eq!(uploads.load(Ordering::SeqCst), if succeeds { 1 } else { 2 });
    }
}

#[tokio::test]
async fn cancelled_upload_does_not_leave_waiters_or_detached_work() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let key = attachment_key(
        "synthetic-cancel",
        "account",
        &[0; 32],
        "image/png",
        b"image",
    );
    let uploads = AtomicUsize::new(0);
    let mut owner = Box::pin(cached_upload(key, || async {
        uploads.fetch_add(1, Ordering::SeqCst);
        std::future::pending::<Result<String, AttachmentFailure>>().await
    }));
    assert!(futures_util::poll!(&mut owner).is_pending());
    let mut waiter = Box::pin(cached_upload(key, || async {
        uploads.fetch_add(1, Ordering::SeqCst);
        Ok("file-waiter".to_string())
    }));
    assert!(futures_util::poll!(&mut waiter).is_pending());
    drop(owner);
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .unwrap();
    assert!(result.as_ref().is_ok_and(|id| id == "file-waiter"));
    assert_eq!(uploads.load(Ordering::SeqCst), 2);
}

#[test]
fn pending_uploads_are_bounded_released_and_scoped() {
    let mut cache = AttachmentCache::default();
    let key = attachment_key("endpoint", "account", &[0; 32], "image/png", b"image");
    let first = cache.pending_upload(key);
    assert!(Arc::ptr_eq(&first, &cache.pending_upload(key)));
    for other in [
        attachment_key("other-endpoint", "account", &[0; 32], "image/png", b"image"),
        attachment_key("endpoint", "other-account", &[0; 32], "image/png", b"image"),
        attachment_key("endpoint", "account", &[1; 32], "image/png", b"image"),
        attachment_key("endpoint", "account", &[0; 32], "image/png", b"other-image"),
    ] {
        assert!(!Arc::ptr_eq(&first, &cache.pending_upload(other)));
    }
    let held: Vec<_> = (0..MAX_ATTACHMENT_CACHE + 1)
        .map(|i| {
            cache.pending_upload(attachment_key(
                "bounded",
                "account",
                &[0; 32],
                "image/png",
                &i.to_be_bytes(),
            ))
        })
        .collect();
    assert_eq!(cache.pending.len(), MAX_ATTACHMENT_CACHE);
    assert!(Arc::ptr_eq(&first, &cache.pending_upload(key)));
    drop(held);
    drop(first);
    let _next = cache.pending_upload(key);
    assert_eq!(cache.pending.len(), 1);
}
