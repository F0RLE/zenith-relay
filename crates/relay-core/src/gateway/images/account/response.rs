use super::*;

pub(in crate::gateway::images) fn translate_account_response(
    bytes: &[u8],
    response_format: &str,
    stream_prefix: &str,
) -> Result<TranslatedImageResponse, ImageFailure> {
    let mut output = Vec::new();
    let mut partials = Vec::new();
    let mut completed = None;

    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        if let Some(failure) = image_failure_from_event(&value) {
            return Err(failure);
        }
        if image_event_is_completion(&value) {
            completed = Some(value.get("response").cloned().unwrap_or(value));
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
                "response.image_generation_call.partial_image" => partials.push(event),
                "response.output_item.done" => {
                    if let Some(item) = event.get("item") {
                        output.push(item.clone());
                    }
                }
                "response.completed" | "response.done"
                    // `response.done` is success only when its status agrees.
                    // A failed, incomplete, or unknown status must not turn
                    // partial image bytes into a completed generation.
                    if image_event_is_completion(&event) => {
                        completed = Some(event.get("response").cloned().unwrap_or(event));
                        break;
                    }
                _ => {}
            }
        }
    }

    let Some(mut completed) = completed else {
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
    if completed
        .get("output")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
        && !output.is_empty()
    {
        completed["output"] = Value::Array(output);
    }
    let images = completed
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
    let created = completed
        .get("created_at")
        .or_else(|| completed.get("created"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| crate::usage::sql_u64(now_ms() / 1_000));
    let usage = completed
        .pointer("/tool_usage/image_gen")
        .or_else(|| completed.get("usage"))
        .cloned();
    let data = images
        .iter()
        .map(|image| image_api_item(image, response_format))
        .collect::<Vec<_>>();
    let mut json_body = json!({"created": created, "data": data});
    if let Some(usage) = usage.clone() {
        json_body["usage"] = usage;
    }
    if let Some(first) = images.first() {
        for name in ["background", "output_format", "quality", "size"] {
            if let Some(value) = first.get(name).filter(|value| !value.is_null()) {
                json_body[name] = value.clone();
            }
        }
    }

    let mut stream_body = Vec::new();
    for partial in partials {
        let Some(result) = partial
            .get("partial_image_b64")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let output_format = partial
            .get("output_format")
            .and_then(Value::as_str)
            .unwrap_or("png");
        let event_name = format!("{stream_prefix}.partial_image");
        let mut data = json!({
            "type": event_name,
            "partial_image_index": partial.get("partial_image_index").and_then(Value::as_u64).unwrap_or(0),
        });
        insert_image_payload(&mut data, result, output_format, response_format);
        push_sse(&mut stream_body, &event_name, &data);
    }
    let event_name = format!("{stream_prefix}.completed");
    for image in &images {
        let mut data = image_api_item(image, response_format);
        data["type"] = Value::String(event_name.clone());
        if let Some(usage) = usage.clone() {
            data["usage"] = usage;
        }
        push_sse(&mut stream_body, &event_name, &data);
    }
    stream_body.extend_from_slice(b"data: [DONE]\n\n");

    Ok(TranslatedImageResponse {
        json: serde_json::to_vec(&json_body).unwrap_or_else(|_| b"{}".to_vec()),
        stream: stream_body,
        usage,
    })
}

fn sse_json(event: &[u8]) -> Option<Value> {
    let data = crate::protocol::sse_data(event);
    (!data.is_empty() && data != b"[DONE]")
        .then(|| serde_json::from_slice(&data).ok())
        .flatten()
}

fn image_result(item: &Value) -> Option<Value> {
    (item.get("type").and_then(Value::as_str) == Some("image_generation_call"))
        .then(|| item.get("result").and_then(Value::as_str))
        .flatten()
        .filter(|result| !result.trim().is_empty())
        .map(|_| item.clone())
}

fn image_api_item(image: &Value, response_format: &str) -> Value {
    let result = image
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let output_format = image
        .get("output_format")
        .and_then(Value::as_str)
        .unwrap_or("png");
    let mut item = Value::Object(Map::new());
    insert_image_payload(&mut item, result, output_format, response_format);
    if let Some(prompt) = image
        .get("revised_prompt")
        .and_then(Value::as_str)
        .filter(|prompt| !prompt.is_empty())
    {
        item["revised_prompt"] = Value::String(prompt.to_string());
    }
    item
}

fn insert_image_payload(target: &mut Value, result: &str, output_format: &str, format: &str) {
    if format.eq_ignore_ascii_case("url") {
        target["url"] = Value::String(format!(
            "data:{};base64,{result}",
            image_mime_type(output_format)
        ));
    } else {
        target["b64_json"] = Value::String(result.to_string());
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
            let response = event.get("response").unwrap_or(event);
            matches!(
                response.get("status").and_then(Value::as_str),
                Some("completed") | None
            )
        }
        Some(_) => false,
        None => true,
    }
}

fn push_sse(target: &mut Vec<u8>, event_name: &str, data: &Value) {
    target.extend_from_slice(b"event: ");
    target.extend_from_slice(event_name.as_bytes());
    target.extend_from_slice(b"\ndata: ");
    target.extend_from_slice(data.to_string().as_bytes());
    target.extend_from_slice(b"\n\n");
}

fn image_failure_from_event(value: &Value) -> Option<ImageFailure> {
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let response = value.get("response").unwrap_or(value);
    let error = value
        .get("error")
        .or_else(|| response.get("error"))
        .filter(|error| !error.is_null());
    let incomplete_reason = response
        .pointer("/incomplete_details/reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let response_status = response
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
        .filter(|value| !value.is_empty())
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
        .filter(|value| !value.is_empty())
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
        upstream_status_from_value(value).unwrap_or(StatusCode::BAD_GATEWAY),
        value,
    );
    let classified_status = upstream_status_from_value(value)
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
            None, value,
        ))),
        status,
        category,
        code: code.to_string(),
        message,
        retryable,
        cooldown_hint: rate_limit_body_hint_value(value, SystemTime::now()),
    })
}

pub(in crate::gateway::images) fn image_capability_unavailable(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    text.contains("image generation is not enabled")
        || text.contains(error_codes::IMAGE_GENERATION_NOT_ENABLED)
}

pub(in crate::gateway::images) fn image_error_response(failure: ImageFailure) -> Response<Body> {
    (
        failure.status,
        Json(json!({
            "error": {
                "message": failure.message,
                "type": api_error_type(failure.status, &failure.code),
                "code": failure.code,
                "param": null,
            }
        })),
    )
        .into_response()
}
