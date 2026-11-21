//! Standalone Office Images, independent of the Responses tool bridge.
use super::{ImageEndpoint, PreparedImageRequest};
use axum::http::HeaderValue;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

pub(super) async fn request_body(
    request: &PreparedImageRequest,
    endpoint: ImageEndpoint,
    model: &str,
) -> Result<(Vec<u8>, HeaderValue), ()> {
    if endpoint == ImageEndpoint::Edits
        && request.content_type.to_str().is_ok_and(|value| {
            value
                .to_ascii_lowercase()
                .starts_with("multipart/form-data")
        })
    {
        return Ok((
            super::multipart::rewritten_body(request, model, true).await?,
            request.content_type.clone(),
        ));
    }
    // Inline image edits use the multipart transport observed in the Office
    // client. Remote locators stay JSON: never fetch arbitrary client URLs here.
    if endpoint == ImageEndpoint::Edits
        && request
            .input_images
            .iter()
            .all(|image| image.starts_with("data:"))
        && request
            .mask_image
            .as_ref()
            .is_none_or(|image| image.starts_with("data:"))
        && !request.fields.contains_key("markup")
    {
        return inline_edit_body(request, model);
    }
    let mut fields = request.fields.clone();
    fields.insert("model".into(), Value::String(model.to_owned()));
    fields.remove("response_format");
    if fields
        .get("stream")
        .is_some_and(|value| value.is_null() || value == false)
    {
        fields.remove("stream");
    }
    // Explicit models and every meaningful option are preserved. Unsupported
    // streaming, sizes, or formats receive the provider's refusal, not a retry
    // through another model or account transport.
    Ok((
        serde_json::to_vec(&fields).map_err(|_| ())?,
        HeaderValue::from_static("application/json"),
    ))
}

fn inline_edit_body(
    request: &PreparedImageRequest,
    model: &str,
) -> Result<(Vec<u8>, HeaderValue), ()> {
    let boundary = format!("relay-image-{}", super::super::request::request_id());
    let mut body = Vec::new();
    for (name, value) in &request.fields {
        if matches!(
            name.as_str(),
            "model" | "response_format" | "image" | "image[]" | "images" | "mask"
        ) || (name == "stream" && (value.is_null() || value == false))
        {
            continue;
        }
        let value = value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string());
        field(&mut body, &boundary, name, &value);
    }
    field(&mut body, &boundary, "model", model);
    for (index, image) in request.input_images.iter().enumerate() {
        image_part(
            &mut body,
            &boundary,
            if request.input_images.len() == 1 {
                "image"
            } else {
                "image[]"
            },
            index,
            image,
        )?;
    }
    if let Some(mask) = &request.mask_image {
        image_part(&mut body, &boundary, "mask", 0, mask)?;
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    if body.len() > super::MAX_IMAGE_BODY_BYTES {
        return Err(());
    }
    Ok((
        body,
        HeaderValue::from_str(&format!("multipart/form-data; boundary={boundary}"))
            .map_err(|_| ())?,
    ))
}

fn field(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    let name = name
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\r', '\n'], "");
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
        .as_bytes(),
    );
}

fn image_part(
    body: &mut Vec<u8>,
    boundary: &str,
    name: &str,
    index: usize,
    image: &str,
) -> Result<(), ()> {
    let (media, encoded) = image
        .strip_prefix("data:")
        .ok_or(())?
        .split_once(";base64,")
        .ok_or(())?;
    if !media.starts_with("image/")
        || !media
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/+-._".contains(&byte))
    {
        return Err(());
    }
    let bytes = STANDARD.decode(encoded).map_err(|_| ())?;
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"image-{index}\"\r\nContent-Type: {media}\r\n\r\n").as_bytes());
    body.extend_from_slice(&bytes);
    body.extend_from_slice(b"\r\n");
    Ok(())
}
