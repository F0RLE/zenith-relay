use super::super::errors::api_error;
use super::super::now_ms;
use super::super::request::client_context_fingerprint;
use super::{IMAGE_PROTOCOLS, ImageEndpoint, ParsedImageFields, PreparedImageRequest};
use crate::error_codes;
use crate::runtime::{AuthenticatedKey, GatewayRuntime, IMAGE_API_MODEL, is_image_model_id};
use axum::body::{Body, Bytes};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::stream;
use multer::Multipart;
use serde_json::{Map, Value};
use std::io;

#[expect(
    clippy::result_large_err,
    reason = "The bounded Axum response is the existing image-request short-circuit contract."
)]
pub(super) async fn prepare_request(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
    request_body: Body,
    endpoint: ImageEndpoint,
) -> Result<PreparedImageRequest, Response<Body>> {
    let raw_body = axum::body::to_bytes(request_body, super::MAX_IMAGE_BODY_BYTES)
        .await
        .map_err(|_| {
            api_error(
                StatusCode::BAD_REQUEST,
                "image request body could not be read or exceeds 128 MiB",
                error_codes::INVALID_REQUEST,
            )
        })?;
    let content_type = headers
        .get(CONTENT_TYPE)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static("application/json"));
    let content_type_text = content_type.to_str().unwrap_or_default();
    let (mut request_fields, input_images, mask_image) = if endpoint == ImageEndpoint::Edits
        && content_type_text
            .to_ascii_lowercase()
            .starts_with("multipart/form-data")
    {
        parse_multipart(content_type_text, raw_body.clone()).await?
    } else {
        parse_json(&raw_body, endpoint)?
    };

    let prompt = request_fields
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty());
    if prompt.is_none() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "prompt must be a non-empty string",
            error_codes::INVALID_REQUEST,
        ));
    }
    if endpoint == ImageEndpoint::Edits && input_images.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "image edits require at least one image",
            error_codes::INVALID_REQUEST,
        ));
    }

    let requested_model = request_fields
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| match key.model_prefix.as_deref() {
            Some(prefix) => format!("{prefix}/{IMAGE_API_MODEL}"),
            None => IMAGE_API_MODEL.to_string(),
        });
    let Some(resolved_model) =
        runtime.resolve_visible_model(key, &requested_model, IMAGE_PROTOCOLS, now_ms())
    else {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "model is not available in this managed pool",
            error_codes::MODEL_NOT_FOUND,
        ));
    };
    if !is_image_model_id(&resolved_model) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "images endpoints require the configured image-generation model",
            error_codes::INVALID_IMAGE_MODEL,
        ));
    }
    request_fields.insert("model".to_string(), Value::String(resolved_model.clone()));

    let stream = match request_fields.get("stream") {
        Some(Value::Bool(stream)) => *stream,
        Some(_) => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "stream must be a boolean",
                error_codes::INVALID_REQUEST,
            ));
        }
        None => false,
    };
    let response_format = request_fields
        .get("response_format")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|format| !format.is_empty())
        .unwrap_or("b64_json")
        .to_ascii_lowercase();
    if !matches!(response_format.as_str(), "b64_json" | "url") {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "response_format must be b64_json or url",
            error_codes::INVALID_REQUEST,
        ));
    }
    if stream && response_format == "url" {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "streaming images use b64_json; response_format=url requires stream=false",
            error_codes::INVALID_REQUEST,
        ));
    }
    if let Some(count) = request_fields.get("n")
        && count.as_u64().is_none_or(|count| count == 0 || count > 10)
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "n must be an integer from 1 to 10",
            error_codes::INVALID_REQUEST,
        ));
    }

    Ok(PreparedImageRequest {
        requested_model,
        resolved_model,
        fields: request_fields,
        input_images,
        mask_image,
        raw_body,
        content_type,
        stream,
        response_format,
        client_context_id: client_context_fingerprint(headers),
    })
}

#[allow(clippy::result_large_err)]
fn parse_json(
    request_body_bytes: &[u8],
    endpoint: ImageEndpoint,
) -> Result<ParsedImageFields, Response<Body>> {
    let Ok(Value::Object(request_fields)) = serde_json::from_slice(request_body_bytes) else {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "request body must be a JSON object",
            error_codes::INVALID_REQUEST,
        ));
    };
    if endpoint == ImageEndpoint::Generations {
        return Ok((request_fields, Vec::new(), None));
    }

    let mut images = Vec::new();
    if let Some(image) = request_fields.get("image").filter(|value| !value.is_null()) {
        let Some(image) = image.as_str().filter(|value| !value.trim().is_empty()) else {
            return Err(invalid_image_reference());
        };
        push_non_empty(&mut images, image);
    }
    if let Some(image_entries) = request_fields
        .get("images")
        .filter(|value| !value.is_null())
    {
        let Some(image_entries) = image_entries.as_array() else {
            return Err(invalid_image_reference());
        };
        for image in image_entries {
            let Some(url) = image
                .get("image_url")
                .and_then(Value::as_str)
                .or_else(|| image.as_str())
                .filter(|value| !value.trim().is_empty())
            else {
                return Err(invalid_image_reference());
            };
            push_non_empty(&mut images, url);
        }
    }
    let mask = request_fields.get("mask").and_then(|mask| {
        mask.get("image_url")
            .and_then(Value::as_str)
            .or_else(|| mask.as_str())
            .map(str::trim)
            .filter(|mask_value| !mask_value.is_empty())
            .map(str::to_string)
    });
    if request_fields
        .get("mask")
        .is_some_and(|value| !value.is_null())
        && mask.is_none()
    {
        return Err(invalid_image_reference());
    }
    Ok((request_fields, images, mask))
}

fn invalid_image_reference() -> Response<Body> {
    api_error(
        StatusCode::BAD_REQUEST,
        "image and mask inputs require non-empty URL/data URL references or multipart files",
        error_codes::INVALID_REQUEST,
    )
}

#[allow(clippy::result_large_err)]
pub(super) async fn parse_multipart(
    content_type: &str,
    multipart_bytes: Bytes,
) -> Result<ParsedImageFields, Response<Body>> {
    let boundary = multer::parse_boundary(content_type).map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "multipart boundary is invalid",
            error_codes::INVALID_REQUEST,
        )
    })?;
    let multipart_body_stream =
        stream::once(async move { Ok::<Bytes, io::Error>(multipart_bytes) });
    let mut multipart = Multipart::new(multipart_body_stream, boundary);
    let mut multipart_fields = Map::new();
    let mut images = Vec::new();
    let mut mask = None;

    while let Some(field) = multipart.next_field().await.map_err(multipart_error)? {
        let field_name = field.name().unwrap_or_default().to_string();
        let file_name = field.file_name().map(str::to_string);
        let content_type = field.content_type().map(ToString::to_string);
        let field_bytes = field.bytes().await.map_err(multipart_error)?;
        if matches!(field_name.as_str(), "image" | "image[]" | "mask") && file_name.is_some() {
            if field_bytes.is_empty() {
                return Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "uploaded image must not be empty",
                    error_codes::INVALID_REQUEST,
                ));
            }
            let data_url = image_data_url(&field_bytes, content_type.as_deref());
            if field_name == "mask" {
                mask = Some(data_url);
            } else {
                images.push(data_url);
            }
            continue;
        }
        let field_text = String::from_utf8(field_bytes.to_vec()).map_err(|_| {
            api_error(
                StatusCode::BAD_REQUEST,
                "multipart text fields must be UTF-8",
                error_codes::INVALID_REQUEST,
            )
        })?;
        let field_text = field_text.trim();
        if field_text.is_empty() {
            continue;
        }
        if matches!(field_name.as_str(), "image" | "image[]") {
            images.push(field_text.to_string());
        } else if field_name == "mask" {
            mask = Some(field_text.to_string());
        } else if matches!(field_name.as_str(), "stream") {
            let parsed = match field_text.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => {
                    return Err(api_error(
                        StatusCode::BAD_REQUEST,
                        "stream must be a boolean",
                        error_codes::INVALID_REQUEST,
                    ));
                }
            };
            multipart_fields.insert(field_name, Value::Bool(parsed));
        } else if matches!(
            field_name.as_str(),
            "n" | "output_compression" | "partial_images"
        ) {
            let parsed = field_text.parse::<u64>().map_err(|_| {
                api_error(
                    StatusCode::BAD_REQUEST,
                    "numeric multipart fields must be positive integers",
                    error_codes::INVALID_REQUEST,
                )
            })?;
            multipart_fields.insert(field_name, Value::Number(parsed.into()));
        } else {
            multipart_fields.insert(field_name, Value::String(field_text.to_string()));
        }
    }
    Ok((multipart_fields, images, mask))
}

fn multipart_error(_error: multer::Error) -> Response<Body> {
    api_error(
        StatusCode::BAD_REQUEST,
        "multipart image upload is invalid",
        error_codes::INVALID_REQUEST,
    )
}

fn image_data_url(bytes: &[u8], content_type: Option<&str>) -> String {
    let content_type = content_type
        .filter(|content_type| {
            !content_type.trim().is_empty() && *content_type != "application/octet-stream"
        })
        .unwrap_or_else(|| detect_image_content_type(bytes));
    format!("data:{content_type};base64,{}", STANDARD.encode(bytes))
}

fn detect_image_content_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else {
        "application/octet-stream"
    }
}

fn push_non_empty(target: &mut Vec<String>, text_value: &str) {
    let text_value = text_value.trim();
    if !text_value.is_empty() {
        target.push(text_value.to_string());
    }
}

#[cfg(test)]
mod image_reference_tests {
    use super::*;

    #[test]
    fn image_edit_inputs_do_not_silently_drop_unsupported_references() {
        let (_, images, mask) = parse_json(
            br#"{"images":[{"image_url":"data:image/png;base64,YQ=="}],"image":null,"mask":null}"#,
            ImageEndpoint::Edits,
        )
        .unwrap();
        assert_eq!(images.len(), 1);
        assert!(mask.is_none());
        for body in [
            br#"{"images":[{"image_url":"data:image/png;base64,YQ=="},{"file_id":"unsupported"}]}"#
                .as_slice(),
            br#"{"image":"valid","mask":{"file_id":"unsupported"}}"#.as_slice(),
        ] {
            assert!(parse_json(body, ImageEndpoint::Edits).is_err());
        }
    }
}
