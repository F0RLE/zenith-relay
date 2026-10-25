use super::*;

fn synthetic_request(images: usize, image_bytes: usize) -> Value {
    let mut bytes = png_bytes();
    bytes.resize(image_bytes.max(bytes.len()), 42);
    json!({"model":"synthetic-model","stream":true,"input":[
        {"role":"developer","content":[{"type":"input_text","text":"Synthetic context. ".repeat(8192)}]},
        {"role":"user","content":(0..images).map(|_| json!({
            "type":"input_image","image_url":data_url(&bytes),"detail":"high"
        })).collect::<Vec<_>>()}
    ]})
}

async fn rewrite(request: &mut Value) {
    let jobs = expect_jobs(stage_input_images(request));
    assert!(resolve_input_images(request, jobs, |_| async {
        Ok("file-synthetic".to_string())
    })
    .await
    .is_ok());
}

// Mirror the removed JSON work, not provider/upload time. Preparation encoded
// once before the catalog step; dispatch encoded again and reparsed for images
// and discovery. Text-only requests skipped only the image rewrite roundtrip.
async fn roundtrip(mut request: Value, legacy: bool) -> (Vec<u8>, Value) {
    if legacy {
        std::hint::black_box(serde_json::to_vec(&request).unwrap());
        let encoded = serde_json::to_vec(&request).unwrap();
        if input_image_parts(&request).next().is_none() {
            let continuation = serde_json::from_slice(&encoded).unwrap();
            return (encoded, continuation);
        }
        request = serde_json::from_slice(&encoded).unwrap();
    }
    rewrite(&mut request).await;
    let encoded = serde_json::to_vec(&request).unwrap();
    if legacy {
        request = serde_json::from_slice(&encoded).unwrap();
    }
    (encoded, request)
}

#[test]
fn image_scan_ignores_non_user_items_and_tool_schema_strings() {
    let request = json!({"tools":[{"enum":["input_image"]}],"input":[
        {"role":"developer","content":[{"type":"input_image","file_id":"file-system"}]},
        {"role":"assistant","content":[{"type":"input_image","file_id":"file-assistant"}]},
        {"role":"user","type":"function_call_output","content":[{"type":"input_image"}]},
        {"role":"user","content":[{"type":"input_text","text":"input_image"}]}
    ]});
    assert!(input_image_parts(&request).next().is_none());
    assert!(expect_jobs(stage_input_images(&request)).is_empty());
}

#[tokio::test]
async fn existing_image_references_preserve_canonical_wire_and_continuation() {
    let request = json!({"input":[{"role":"user","content":[
        {"type":"input_image","file_id":"file-existing","detail":"high"}
    ]}]});
    let before = roundtrip(request.clone(), true).await;
    let after = roundtrip(request, false).await;
    assert_eq!(before, after);
    assert_eq!(
        after.1["input"][0]["content"][0],
        json!({"type":"input_image","file_id":"file-existing"})
    );
}

#[tokio::test]
async fn attachment_json_roundtrips_preserve_wire_and_continuation() {
    for images in [0, 1, 4] {
        let request = synthetic_request(images, 128);
        let before = roundtrip(request.clone(), true).await;
        let after = roundtrip(request, false).await;
        assert_eq!(before, after);
        assert!(!String::from_utf8(after.0).unwrap().contains("data:image"));
    }
}

#[tokio::test]
#[ignore = "manual synthetic attachment JSON overhead, not upload/provider latency"]
async fn measure_bps_attachment_json_roundtrips() {
    use std::{hint::black_box, time::Instant};

    for (images, image_bytes) in [(0, 0), (4, 256 * 1024), (8, 1024 * 1024)] {
        let request = synthetic_request(images, image_bytes);
        let encoded_bytes = serde_json::to_vec(&request).unwrap().len();
        let mut legacy = Vec::new();
        let mut direct = Vec::new();
        for trial in 0..9 {
            // Alternate order and exclude fixture cloning from both timings.
            for old in if trial % 2 == 0 {
                [true, false]
            } else {
                [false, true]
            } {
                let input = request.clone();
                let started = Instant::now();
                black_box(roundtrip(input, old).await);
                let elapsed = started.elapsed().as_micros();
                if trial > 0 {
                    if old {
                        legacy.push(elapsed);
                    } else {
                        direct.push(elapsed);
                    }
                }
            }
        }
        legacy.sort_unstable();
        direct.sort_unstable();
        eprintln!("bps-attachment-json images={images} input_bytes={encoded_bytes} trials=8 legacy_median_us={} direct_median_us={}", legacy[4], direct[4]);
    }
}
