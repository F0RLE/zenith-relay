use super::super::errors::api_error;
use super::super::now_ms;
use super::super::request::client_context_fingerprint;
use super::{ImageEndpoint, ParsedImageFields, PreparedImageRequest, IMAGE_PROTOCOLS};
use crate::error_codes;
use crate::runtime::{is_image_model_id, AuthenticatedKey, GatewayRuntime, IMAGE_API_MODEL};
use axum::body::{Body, Bytes};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use base64::{engine::general_purpose::STANDARD, Engine as _};
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
    body: Body,
    endpoint: ImageEndpoint,
) -> Result<PreparedImageRequest, Response<Body>> {
    let raw_body = axum::body::to_bytes(body, usize::MAX).await.map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "request body could not be read",
            error_codes::INVALID_REQUEST,
        )
    })?;
    let content_type = headers
        .get(CONTENT_TYPE)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static("application/json"));
    let content_type_text = content_type.to_str().unwrap_or_default();
    let (mut fields, input_images, mask_image) = if endpoint == ImageEndpoint::Edits
        && content_type_text
            .to_ascii_lowercase()
            .starts_with("multipart/form-data")
    {
        parse_multipart(content_type_text, raw_body.clone()).await?
    } else {
        parse_json(&raw_body, endpoint)?
    };

    let prompt = fields
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

    let requested_model = fields
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
    fields.insert("model".to_string(), Value::String(resolved_model.clone()));

    let stream = match fields.get("stream") {
        Some(Value::Bool(stream)) => *stream,
        Some(_) => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "stream must be a boolean",
                error_codes::INVALID_REQUEST,
            ))
        }
        None => false,
    };
    let response_format = fields
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

    Ok(PreparedImageRequest {
        requested_model,
        resolved_model,
        fields,
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
fn parse_json(body: &[u8], endpoint: ImageEndpoint) -> Result<ParsedImageFields, Response<Body>> {
    let Ok(Value::Object(fields)) = serde_json::from_slice(body) else {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "request body must be a JSON object",
            error_codes::INVALID_REQUEST,
        ));
    };
    if endpoint == ImageEndpoint::Generations {
        return Ok((fields, Vec::new(), None));
    }

    let mut images = Vec::new();
    if let Some(image) = fields.get("image").and_then(Value::as_str) {
        push_non_empty(&mut images, image);
    }
    if let Some(values) = fields.get("images").and_then(Value::as_array) {
        for image in values {
            if let Some(url) = image
                .get("image_url")
                .and_then(Value::as_str)
                .or_else(|| image.as_str())
            {
                push_non_empty(&mut images, url);
            }
        }
    }
    let mask = fields.get("mask").and_then(|mask| {
        mask.get("image_url")
            .and_then(Value::as_str)
            .or_else(|| mask.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    Ok((fields, images, mask))
}

#[allow(clippy::result_large_err)]
pub(super) async fn parse_multipart(
    content_type: &str,
    body: Bytes,
) -> Result<ParsedImageFields, Response<Body>> {
    let boundary = multer::parse_boundary(content_type).map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "multipart boundary is invalid",
            error_codes::INVALID_REQUEST,
        )
    })?;
    let body = stream::once(async move { Ok::<Bytes, io::Error>(body) });
    let mut multipart = Multipart::new(body, boundary);
    let mut fields = Map::new();
    let mut images = Vec::new();
    let mut mask = None;

    while let Some(field) = multipart.next_field().await.map_err(multipart_error)? {
        let name = field.name().unwrap_or_default().to_string();
        let file_name = field.file_name().map(str::to_string);
        let content_type = field.content_type().map(ToString::to_string);
        let bytes = field.bytes().await.map_err(multipart_error)?;
        if matches!(name.as_str(), "image" | "image[]" | "mask") && file_name.is_some() {
            if bytes.is_empty() {
                return Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "uploaded image must not be empty",
                    error_codes::INVALID_REQUEST,
                ));
            }
            let data_url = image_data_url(&bytes, content_type.as_deref());
            if name == "mask" {
                mask = Some(data_url);
            } else {
                images.push(data_url);
            }
            continue;
        }
        let value = String::from_utf8(bytes.to_vec()).map_err(|_| {
            api_error(
                StatusCode::BAD_REQUEST,
                "multipart text fields must be UTF-8",
                error_codes::INVALID_REQUEST,
            )
        })?;
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if matches!(name.as_str(), "image" | "image[]") {
            images.push(value.to_string());
        } else if name == "mask" {
            mask = Some(value.to_string());
        } else if matches!(name.as_str(), "stream") {
            let parsed = match value.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => {
                    return Err(api_error(
                        StatusCode::BAD_REQUEST,
                        "stream must be a boolean",
                        error_codes::INVALID_REQUEST,
                    ))
                }
            };
            fields.insert(name, Value::Bool(parsed));
        } else if matches!(name.as_str(), "n" | "output_compression" | "partial_images") {
            let parsed = value.parse::<u64>().map_err(|_| {
                api_error(
                    StatusCode::BAD_REQUEST,
                    "numeric multipart fields must be positive integers",
                    error_codes::INVALID_REQUEST,
                )
            })?;
            fields.insert(name, Value::Number(parsed.into()));
        } else {
            fields.insert(name, Value::String(value.to_string()));
        }
    }
    Ok((fields, images, mask))
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
        .filter(|value| !value.trim().is_empty() && *value != "application/octet-stream")
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

fn push_non_empty(target: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        target.push(value.to_string());
    }
}
