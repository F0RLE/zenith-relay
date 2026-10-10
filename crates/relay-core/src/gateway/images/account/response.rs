use super::*;

pub(in crate::gateway::images) fn translate_account_response(
    bytes: &[u8],
    response_format: &str,
    stream_prefix: &str,
) -> Result<TranslatedImageResponse, ImageFailure> {
    let mut output_items = Vec::new();
    let mut partial_events = Vec::new();
    let mut completed_response = None;

    if let Ok(response_payload) = serde_json::from_slice::<Value>(bytes) {
        if let Some(failure) = image_failure_from_event(&response_payload) {
            return Err(failure);
        }
        if image_event_is_completion(&response_payload) {
            completed_response = Some(
                response_payload
                    .get("response")
                    .cloned()
                    .unwrap_or(response_payload),
            );
        }
    } else {
        let mut offset = 0;
        while let Some(end) = sse_event_end(&bytes[offset..]) {
            let event = sse_json(&bytes[offset..offset + end]);
            offset += end;
            let Some(event) = event else {
                continue;
            };
            if let Some(failure) = image_failure_from_event(&event) {
                return Err(failure);
            }
            match event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "response.image_generation_call.partial_image" => partial_events.push(event),
                "response.output_item.done" => {
                    if let Some(output_item) = event.get("item") {
                        output_items.push(output_item.clone());
                    }
                }
                "response.completed" | "response.done"
                    // `response.done` is success only when its status agrees.
                    // A failed, incomplete, or unknown status must not turn
                    // partial image bytes into a completed generation.
                    if image_event_is_completion(&event) => {
                        completed_response =
                            Some(event.get("response").cloned().unwrap_or(event));
                        break;
                    }
                _ => {}
            }
        }
    }

    let Some(mut completed_response) = completed_response else {
        return Err(ImageFailure {
            status: StatusCode::BAD_GATEWAY,
            category: error_codes::STREAM_INCOMPLETE,
            upstream_error: None,
            code: error_codes::STREAM_INCOMPLETE.to_string(),
            message: "upstream image stream ended before completion".to_string(),
            retryable: true,
            cooldown_hint: RateLimitBodyHint::default(),
        });
    };
    if completed_response
        .get("output")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
        && !output_items.is_empty()
    {
        completed_response["output"] = Value::Array(output_items);
    }
    let images = completed_response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(image_result)
        .collect::<Vec<_>>();
    if images.is_empty() {
        return Err(ImageFailure {
            status: StatusCode::BAD_GATEWAY,
            category: error_codes::IMAGE_OUTPUT_MISSING,
            upstream_error: None,
            code: error_codes::IMAGE_OUTPUT_MISSING.to_string(),
            message: "upstream did not return image output".to_string(),
            retryable: true,
            cooldown_hint: RateLimitBodyHint::default(),
        });
    }
    let created = completed_response
        .get("created_at")
        .or_else(|| completed_response.get("created"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| crate::usage::sql_u64(now_ms() / 1_000));
    let usage = completed_response
        .pointer("/tool_usage/image_gen")
        .or_else(|| completed_response.get("usage"))
        .cloned();
    let image_payloads = images
        .iter()
        .map(|image| image_api_item(image, response_format))
        .collect::<Vec<_>>();
    let mut json_body = json!({"created": created, "data": image_payloads});
    if let Some(usage) = usage.clone() {
        json_body["usage"] = usage;
    }
    if let Some(first) = images.first() {
        for field_name in ["background", "output_format", "quality", "size"] {
            if let Some(metadata_value) = first
                .get(field_name)
                .filter(|metadata_value| !metadata_value.is_null())
            {
                json_body[field_name] = metadata_value.clone();
            }
        }
    }

    let mut stream_body = Vec::new();
    for partial_event in partial_events {
        let Some(partial_image_b64) = partial_event
            .get("partial_image_b64")
            .and_then(Value::as_str)
            .filter(|partial_image| !partial_image.is_empty())
        else {
            continue;
        };
        let output_format = partial_event
            .get("output_format")
            .and_then(Value::as_str)
            .unwrap_or("png");
        let event_name = format!("{stream_prefix}.partial_image");
        let mut event_data = json!({
            "type": event_name,
            "partial_image_index": partial_event.get("partial_image_index").and_then(Value::as_u64).unwrap_or(0),
        });
        insert_image_payload(
            &mut event_data,
            partial_image_b64,
            output_format,
            response_format,
        );
        push_sse(&mut stream_body, &event_name, &event_data);
    }
    let event_name = format!("{stream_prefix}.completed");
    for image in &images {
        let mut event_data = image_api_item(image, response_format);
        event_data["type"] = Value::String(event_name.clone());
        if let Some(usage) = usage.clone() {
            event_data["usage"] = usage;
        }
        push_sse(&mut stream_body, &event_name, &event_data);
    }
    stream_body.extend_from_slice(b"data: [DONE]\n\n");

    Ok(TranslatedImageResponse {
        json: serde_json::to_vec(&json_body).unwrap_or_else(|_| b"{}".to_vec()),
        stream: stream_body,
        usage,
    })
}

fn sse_json(event: &[u8]) -> Option<Value> {
    let sse_payload = crate::protocol::sse_data(event);
    (!sse_payload.is_empty() && sse_payload != b"[DONE]")
        .then(|| serde_json::from_slice(&sse_payload).ok())
        .flatten()
}

fn image_result(output_item: &Value) -> Option<Value> {
    (output_item.get("type").and_then(Value::as_str) == Some("image_generation_call"))
        .then(|| output_item.get("result").and_then(Value::as_str))
        .flatten()
        .filter(|image_data| !image_data.trim().is_empty())
        .map(|_| output_item.clone())
}

fn image_api_item(image: &Value, response_format: &str) -> Value {
    let image_data = image
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let output_format = image
        .get("output_format")
        .and_then(Value::as_str)
        .unwrap_or("png");
    let mut image_payload = Value::Object(Map::new());
    insert_image_payload(
        &mut image_payload,
        image_data,
        output_format,
        response_format,
    );
    if let Some(prompt) = image
        .get("revised_prompt")
        .and_then(Value::as_str)
        .filter(|prompt| !prompt.is_empty())
    {
        image_payload["revised_prompt"] = Value::String(prompt.to_string());
    }
    image_payload
}

fn insert_image_payload(
    target: &mut Value,
    image_data: &str,
    output_format: &str,
    response_format: &str,
) {
    if response_format.eq_ignore_ascii_case("url") {
        target["url"] = Value::String(format!(
            "data:{};base64,{image_data}",
            image_mime_type(output_format)
        ));
    } else {
        target["b64_json"] = Value::String(image_data.to_string());
    }
}

fn image_mime_type(output_format: &str) -> &'static str {
    match output_format.trim().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

fn image_event_is_completion(event: &Value) -> bool {
    match event.get("type").and_then(Value::as_str) {
        Some("response.completed" | "response.done") => {
            let completion_response = event.get("response").unwrap_or(event);
            matches!(
                completion_response.get("status").and_then(Value::as_str),
                Some("completed") | None
            )
        }
        Some(_) => false,
        None => true,
    }
}

fn push_sse(target: &mut Vec<u8>, event_name: &str, event_data: &Value) {
    target.extend_from_slice(b"event: ");
    target.extend_from_slice(event_name.as_bytes());
    target.extend_from_slice(b"\ndata: ");
    target.extend_from_slice(event_data.to_string().as_bytes());
    target.extend_from_slice(b"\n\n");
}

fn image_failure_from_event(event_payload: &Value) -> Option<ImageFailure> {
    let event_type = event_payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let completion_response = event_payload.get("response").unwrap_or(event_payload);
    let error = event_payload
        .get("error")
        .or_else(|| completion_response.get("error"))
        .filter(|error| !error.is_null());
    let incomplete_reason = completion_response
        .pointer("/incomplete_details/reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let response_status = completion_response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let done_terminal = matches!(event_type, "response.completed" | "response.done")
        && matches!(
            response_status,
            "failed" | "cancelled" | "canceled" | "incomplete"
        );
    if error.is_none()
        && !done_terminal
        && !matches!(
            event_type,
            "response.failed" | "response.incomplete" | "error"
        )
    {
        return None;
    }
    let error = error.unwrap_or(&Value::Null);
    let error_type = error
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .filter(|error_code| !error_code.is_empty())
        .unwrap_or(
            if event_type == "response.incomplete" || response_status == "incomplete" {
                error_codes::RESPONSE_INCOMPLETE
            } else {
                error_codes::UPSTREAM_ERROR
            },
        );
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .filter(|error_message| !error_message.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if incomplete_reason.is_empty() {
                "upstream image generation failed".to_string()
            } else {
                format!("upstream image generation incomplete: {incomplete_reason}")
            }
        });
    let normalized =
        format!("{error_type} {code} {message} {incomplete_reason}").to_ascii_lowercase();
    let classification = classify_upstream_error_value(
        upstream_status_from_value(event_payload).unwrap_or(StatusCode::BAD_GATEWAY),
        event_payload,
    );
    let classified_status = upstream_status_from_value(event_payload)
        .filter(|status| !status.is_success())
        .unwrap_or_else(|| upstream_failure_status(classification.category));
    let classified_status = canonical_upstream_status(classified_status, classification.category);
    let capability = normalized.contains("image generation is not enabled")
        || normalized.contains(error_codes::IMAGE_GENERATION_NOT_ENABLED);
    let user_error = error_type.eq_ignore_ascii_case(error_codes::IMAGE_GENERATION_USER_ERROR)
        || normalized.contains("moderation")
        || normalized.contains("content_policy")
        || normalized.contains("content filter")
        || normalized.contains("policy_violation")
        || normalized.contains("safety_violation");
    let (status, category, retryable) = if capability {
        (
            StatusCode::BAD_GATEWAY,
            error_codes::IMAGE_GENERATION_NOT_ENABLED,
            true,
        )
    } else if user_error {
        (
            StatusCode::BAD_REQUEST,
            error_codes::IMAGE_GENERATION_USER_ERROR,
            false,
        )
    } else {
        (
            classified_status,
            classification.category,
            retryable_failure(classified_status, classification.category, false),
        )
    };
    Some(ImageFailure {
        upstream_error: Some(Box::new(crate::usage::UpstreamErrorDetails::from_value(
            None,
            event_payload,
        ))),
        status,
        category,
        code: code.to_string(),
        message,
        retryable,
        cooldown_hint: rate_limit_body_hint_value(event_payload, SystemTime::now()),
    })
}

pub(in crate::gateway::images) fn image_capability_unavailable(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    text.contains("image generation is not enabled")
        || text.contains(error_codes::IMAGE_GENERATION_NOT_ENABLED)
}

pub(in crate::gateway::images) fn image_error_response(
    failure: ImageFailure,
    origin: crate::ErrorOrigin,
    request_id: &str,
) -> Response<Body> {
    super::super::super::errors::api_error_with_origin_and_category(
        failure.status,
        &failure.message,
        &failure.code,
        failure.category,
        origin,
        Some(request_id),
    )
}
