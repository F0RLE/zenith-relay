use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn image_sequence(count: usize) -> Value {
    let content: Vec<_> = (0..count)
        .map(|index| {
            let mut bytes = png_bytes();
            bytes.push(index as u8);
            json!({"type":"input_image", "image_url":data_url(&bytes)})
        })
        .collect();
    json!({
        "model":"synthetic-model", "reasoning_effort":"high",
        "input":[{"role":"user", "content":content}]
    })
}

struct ActiveUpload<'a>(&'a AtomicUsize);

impl<'a> ActiveUpload<'a> {
    fn start(active: &'a AtomicUsize, peak: &AtomicUsize) -> Self {
        let count = active.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(count, Ordering::SeqCst);
        Self(active)
    }
}

impl Drop for ActiveUpload<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn uploads_overlap_with_a_bound_and_preserve_bytes_and_input_order() {
    let mut body = image_sequence(9);
    body["input"][0]["content"]
        .as_array_mut()
        .unwrap()
        .insert(2, json!({"type":"input_text", "text":"keep this in place"}));
    body["input"].as_array_mut().unwrap().push(json!({
        "role":"user", "content":[{"type":"input_image", "file_id":"file-existing"}]
    }));
    let jobs = expect_jobs(stage_input_images(&body));
    let mut expected = body.clone();
    for job in &jobs {
        let id = match &job.job {
            ImageJob::Upload(image) => format!("file-{}", image.bytes.last().unwrap()),
            ImageJob::File(id) => id.clone(),
        };
        set_image_part(&mut expected, job.input_index, job.content_index, &id);
    }
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let calls = AtomicUsize::new(0);
    let result = resolve_input_images(&mut body, jobs, |image| {
        let active = &active;
        let peak = &peak;
        let calls = &calls;
        async move {
            let _guard = ActiveUpload::start(active, peak);
            calls.fetch_add(1, Ordering::SeqCst);
            let index = *image.bytes.last().unwrap();
            let mut expected_bytes = png_bytes();
            expected_bytes.push(index);
            assert_eq!(image.bytes, expected_bytes);
            assert_eq!(image.media_type, "image/png");
            // Later input images may finish first without moving any content.
            tokio::time::sleep(Duration::from_millis(40 - u64::from(index) * 3)).await;
            Ok(format!("file-{index}"))
        }
    })
    .await;
    assert!(result.is_ok());
    assert_eq!(body, expected);
    assert_eq!(calls.load(Ordering::SeqCst), 9);
    assert_eq!(peak.load(Ordering::SeqCst), MAX_CONCURRENT_UPLOADS);
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn mixed_uploads_refill_slots_without_waiting_for_input_order() {
    let mut body = image_sequence(8);
    let jobs = expect_jobs(stage_input_images(&body));
    let mut expected = body.clone();
    for (index, job) in jobs.iter().enumerate() {
        set_image_part(
            &mut expected,
            job.input_index,
            job.content_index,
            &format!("file-{index}"),
        );
    }
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let calls = AtomicUsize::new(0);
    let started = tokio::time::Instant::now();
    let result = resolve_input_images(&mut body, jobs, |image| {
        let active = &active;
        let peak = &peak;
        let calls = &calls;
        async move {
            let _guard = ActiveUpload::start(active, peak);
            calls.fetch_add(1, Ordering::SeqCst);
            let index = *image.bytes.last().unwrap();
            let delay = if index == 0 || index == 4 { 200 } else { 40 };
            tokio::time::sleep(Duration::from_millis(delay)).await;
            Ok(format!("file-{index}"))
        }
    })
    .await;
    let elapsed = started.elapsed();
    assert!(result.is_ok());
    assert_eq!(body, expected);
    assert_eq!(calls.load(Ordering::SeqCst), 8);
    assert_eq!(peak.load(Ordering::SeqCst), MAX_CONCURRENT_UPLOADS);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    println!(
        "synthetic mixed uploads=8, unchanged bytes/output, concurrency=4: {}ms (virtual time, not provider latency)",
        elapsed.as_millis()
    );
    assert_eq!(elapsed, Duration::from_millis(240));
}

#[tokio::test(start_paused = true)]
async fn out_of_order_failure_stops_new_work_and_preserves_earlier_results() {
    let mut body = image_sequence(MAX_CONCURRENT_UPLOADS + 3);
    let mut expected = body.clone();
    set_image_part(&mut expected, 0, 0, "file-0");
    let jobs = expect_jobs(stage_input_images(&body));
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let calls = AtomicUsize::new(0);
    let result = resolve_input_images(&mut body, jobs, |image| {
        let active = &active;
        let peak = &peak;
        let calls = &calls;
        async move {
            let _guard = ActiveUpload::start(active, peak);
            calls.fetch_add(1, Ordering::SeqCst);
            let index = *image.bytes.last().unwrap();
            let delay = match index {
                0 => 200,
                1 => 20,
                _ => 40,
            };
            tokio::time::sleep(Duration::from_millis(delay)).await;
            if index == 1 {
                Err(invalid_image("synthetic out-of-order upload rejection"))
            } else {
                Ok(format!("file-{index}"))
            }
        }
    })
    .await;
    assert!(matches!(result, Err(AttachmentFailure::Reject(failure))
        if failure.message == "synthetic out-of-order upload rejection"));
    assert_eq!(body, expected);
    assert_eq!(calls.load(Ordering::SeqCst), MAX_CONCURRENT_UPLOADS);
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn upload_error_keeps_input_order_and_drops_pending_work() {
    let mut body = image_sequence(MAX_CONCURRENT_UPLOADS + 3);
    let jobs = expect_jobs(stage_input_images(&body));
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let calls = AtomicUsize::new(0);
    let result = resolve_input_images(&mut body, jobs, |image| {
        let active = &active;
        let peak = &peak;
        let calls = &calls;
        async move {
            let _guard = ActiveUpload::start(active, peak);
            calls.fetch_add(1, Ordering::SeqCst);
            match image.bytes.last().copied().unwrap() {
                0 => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Err(invalid_image("synthetic first upload rejection"))
                }
                1 => Err(upload_failed()),
                _ => std::future::pending().await,
            }
        }
    })
    .await;
    assert!(matches!(result, Err(AttachmentFailure::Reject(failure))
        if failure.message == "synthetic first upload rejection"));
    assert_eq!(calls.load(Ordering::SeqCst), MAX_CONCURRENT_UPLOADS);
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancelling_image_resolution_drops_all_in_flight_uploads() {
    let mut body = image_sequence(MAX_CONCURRENT_UPLOADS + 3);
    let jobs = expect_jobs(stage_input_images(&body));
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let calls = AtomicUsize::new(0);
    let mut pending = Box::pin(resolve_input_images(&mut body, jobs, |_| async {
        let _guard = ActiveUpload::start(&active, &peak);
        calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending::<Result<String, AttachmentFailure>>().await
    }));
    assert!(futures_util::poll!(&mut pending).is_pending());
    assert_eq!(active.load(Ordering::SeqCst), MAX_CONCURRENT_UPLOADS);
    drop(pending);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(calls.load(Ordering::SeqCst), MAX_CONCURRENT_UPLOADS);
}

#[tokio::test(start_paused = true)]
async fn cancellation_drops_refilled_uploads_and_out_of_order_results() {
    let mut body = image_sequence(8);
    let original = body.clone();
    let jobs = expect_jobs(stage_input_images(&body));
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let calls = AtomicUsize::new(0);
    let mut pending = Box::pin(resolve_input_images(&mut body, jobs, |image| {
        let active = &active;
        let peak = &peak;
        let calls = &calls;
        async move {
            let _guard = ActiveUpload::start(active, peak);
            calls.fetch_add(1, Ordering::SeqCst);
            if image.bytes.last() == Some(&1) {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok("file-1".to_string())
            } else {
                std::future::pending().await
            }
        }
    }));
    assert!(futures_util::poll!(&mut pending).is_pending());
    tokio::time::advance(Duration::from_millis(20)).await;
    assert!(futures_util::poll!(&mut pending).is_pending());
    assert_eq!(calls.load(Ordering::SeqCst), MAX_CONCURRENT_UPLOADS + 1);
    assert_eq!(active.load(Ordering::SeqCst), MAX_CONCURRENT_UPLOADS);
    drop(pending);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(body, original);
}

#[tokio::test(start_paused = true)]
async fn repeated_images_share_one_upload_and_warm_requests_reuse_it() {
    let key = attachment_key(
        "synthetic-image-resolution",
        "account",
        &[0; 32],
        "image/png",
        &png_bytes(),
    );
    let input = json!({"input":[{"role":"user", "content":vec![
        json!({"type":"input_image", "image_url":data_url(&png_bytes())}); 8
    ]}]});
    let calls = AtomicUsize::new(0);
    for warm in [false, true] {
        let mut body = input.clone();
        let jobs = expect_jobs(stage_input_images(&body));
        let result = resolve_input_images(&mut body, jobs, |image| {
            let calls = &calls;
            async move {
                assert_eq!(image.bytes, png_bytes());
                cached_upload(key, || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(40)).await;
                    Ok("file-shared-sequence".to_string())
                })
                .await
            }
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1, "warm={warm}");
        assert!(body["input"][0]["content"]
            .as_array()
            .unwrap()
            .iter()
            .all(|part| *part == json!({"type":"input_image", "file_id":"file-shared-sequence"})));
    }
}

#[tokio::test(start_paused = true)]
async fn cold_upload_schedule_matches_serial_output_with_fewer_waits() {
    let input = image_sequence(8);
    let upload = |image: InlineImage| async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        Ok::<_, AttachmentFailure>(format!("file-{}", image.bytes.last().unwrap()))
    };
    let mut serial = input.clone();
    let started = tokio::time::Instant::now();
    for job in expect_jobs(stage_input_images(&serial)) {
        let ImageJob::Upload(image) = job.job else {
            panic!("synthetic cold upload");
        };
        let id = upload(image).await.unwrap_or_else(|_| panic!("upload"));
        set_image_part(&mut serial, job.input_index, job.content_index, &id);
    }
    let serial_elapsed = started.elapsed();
    let mut parallel = input;
    let jobs = expect_jobs(stage_input_images(&parallel));
    let started = tokio::time::Instant::now();
    assert!(resolve_input_images(&mut parallel, jobs, upload)
        .await
        .is_ok());
    let parallel_elapsed = started.elapsed();
    assert_eq!(parallel, serial);
    assert_eq!(serial_elapsed, Duration::from_millis(320));
    assert_eq!(parallel_elapsed, Duration::from_millis(80));
    println!(
        "synthetic cold uploads=8, unchanged bytes/output: serial={}ms, bounded={}ms (virtual time, not provider latency)",
        serial_elapsed.as_millis(), parallel_elapsed.as_millis()
    );
}
