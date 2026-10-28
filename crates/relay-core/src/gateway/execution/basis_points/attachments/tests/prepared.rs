use super::*;

#[tokio::test]
async fn file_references_skip_credential_reads_without_skipping_validation() {
    for valid in [true, false] {
        let mut body = json!({"input":[{"role":"user","content":[{
            "type":"input_image", "file_id":if valid { " file-existing " } else { " " },
            "image_url":null, "detail":"high"
        }]}]});
        let staged = stage_input_images_with_credentials(
            &body,
            "https://example.test/reference-only/attachments",
            "reference-only-test",
            async { panic!("file references must not read account credentials") },
        )
        .await;
        if !valid {
            assert!(staged.is_err());
            continue;
        }
        let Ok((jobs, credential)) = staged else {
            panic!("valid reference must be staged");
        };
        assert!(credential.is_none());
        assert!(
            resolve_input_images(&mut body, jobs, |_| async {
                panic!("file references must not upload")
            })
            .await
            .is_ok()
        );
        assert_eq!(
            body["input"][0]["content"][0],
            json!({"type":"input_image","file_id":"file-existing"})
        );
    }
}

#[tokio::test]
async fn inline_cache_hits_still_read_the_current_credential() {
    let body = json!({"input":[{"role":"user","content":[{
        "type":"input_image", "image_url":data_url(&png_bytes())
    }]}]});
    let reads = std::sync::atomic::AtomicUsize::new(0);
    for (generation, cached) in [(31, false), (31, true), (32, false)] {
        let Ok((jobs, credential)) = stage_input_images_with_credentials(
            &body,
            "https://example.test/credential-stage/attachments",
            "credential-stage-test",
            async {
                reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some([generation; 32])
            },
        )
        .await
        else {
            panic!("valid inline image must be staged");
        };
        assert_eq!(credential, Some([generation; 32]));
        assert_eq!(matches!(&jobs[0].job, ImageJob::File(_)), cached);
        if let ImageJob::Upload(image) = &jobs[0].job {
            remember_file_id(image.source_key.unwrap(), "file-staged".into());
        }
    }
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 3);
}

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
