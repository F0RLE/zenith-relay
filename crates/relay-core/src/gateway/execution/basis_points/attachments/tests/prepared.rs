use super::*;

#[test]
fn prepared_image_hit_skips_decoding_and_remains_credential_scoped() {
    let body = json!({"input":[{"role":"user","content":[{
        "type":"input_image", "image_url":data_url(&png_bytes())
    }]}]});
    let scope = AttachmentScope {
        endpoint: "https://example.test/prepared-cache/attachments",
        candidate_id: "prepared-cache-test",
        credential: &[17; 32],
    };
    let first = expect_jobs(stage_input_images_with_cache(&body, Some(&scope)));
    let ImageJob::Upload(image) = &first[0].job else {
        panic!("first image must be validated");
    };
    let key = image.source_key.unwrap();
    remember_file_id(key, "file-prepared".to_string());
    let second = expect_jobs(stage_input_images_with_cache(&body, Some(&scope)));
    assert!(matches!(&second[0].job, ImageJob::File(id) if id == "file-prepared"));
    for scope in [
        AttachmentScope {
            credential: &[18; 32],
            ..scope
        },
        AttachmentScope {
            candidate_id: "different-account",
            ..scope
        },
        AttachmentScope {
            endpoint: "https://other.test/attachments",
            ..scope
        },
    ] {
        let jobs = expect_jobs(stage_input_images_with_cache(&body, Some(&scope)));
        assert!(matches!(jobs[0].job, ImageJob::Upload(_)));
    }
}

#[test]
fn invalid_images_never_populate_the_prepared_cache() {
    let body = json!({"input":[{"role":"user","content":[{
        "type":"input_image", "image_url":"data:image/png;base64,not-valid!"
    }]}]});
    let scope = AttachmentScope {
        endpoint: "https://example.test/invalid-prepared/attachments",
        candidate_id: "invalid-prepared-test",
        credential: &[19; 32],
    };
    for _ in 0..2 {
        assert!(stage_input_images_with_cache(&body, Some(&scope)).is_err());
    }
}
