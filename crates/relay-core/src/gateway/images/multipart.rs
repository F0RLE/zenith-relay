use super::PreparedImageRequest;
use axum::body::Bytes;
use futures_util::stream;
use multer::Multipart;

/// Retain uploaded file bytes and ordering, but do not send the public model
/// alias (or GPT Image's unsupported legacy response_format) to an API source.
pub(super) async fn source_body(
    request: &PreparedImageRequest,
    model: &str,
) -> Result<Vec<u8>, ()> {
    let boundary =
        multer::parse_boundary(request.content_type.to_str().map_err(|_| ())?).map_err(|_| ())?;
    let mut parts = Multipart::new(
        stream::once(std::future::ready(Ok::<Bytes, std::io::Error>(
            request.raw_body.clone(),
        ))),
        &boundary,
    );
    let mut output = Vec::with_capacity(request.raw_body.len());
    while let Some(field) = parts.next_field().await.map_err(|_| ())? {
        let name = field.name().unwrap_or_default().to_owned();
        let filename = field.file_name().map(str::to_owned);
        let content_type = field.content_type().map(ToString::to_string);
        let bytes = field.bytes().await.map_err(|_| ())?;
        if name == "model"
            || (name == "response_format" && crate::model_id_key(model).starts_with("gpt-image-"))
        {
            continue;
        }
        output.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"",
                quoted(&name)
            )
            .as_bytes(),
        );
        if let Some(filename) = filename {
            output.extend_from_slice(format!("; filename=\"{}\"", quoted(&filename)).as_bytes());
        }
        output.extend_from_slice(b"\r\n");
        if let Some(content_type) = content_type {
            output.extend_from_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
        }
        output.extend_from_slice(b"\r\n");
        output.extend_from_slice(&bytes);
        output.extend_from_slice(b"\r\n");
    }
    output.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{model}\r\n--{boundary}--\r\n").as_bytes());
    Ok(output)
}

fn quoted(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\r', '\n'], "")
}
