use super::*;

fn optional_text<'a>(fields: &'a Value, key: &str) -> AdapterResult<Option<&'a str>> {
    fields
        .get(key)
        .filter(|value| !value.is_null())
        .map(|_| required_text(fields, key))
        .transpose()
}

pub(super) fn decode(part: &Value, protocol: WireApi) -> AdapterResult<Block> {
    match (protocol, required_text(part, "type")?) {
        (WireApi::Responses, "input_file") => {
            checked(
                part,
                &[
                    "type",
                    "file_data",
                    "file_url",
                    "file_id",
                    "filename",
                    "detail",
                ],
            )?;
            if part.get("file_id").is_some_and(|value| !value.is_null()) {
                return Err(AdapterError::parameter_unsupported_for("input.file_id"));
            }
            let data = optional_text(part, "file_data")?;
            let url = optional_text(part, "file_url")?;
            if data.is_some_and(|data| !data.starts_with("data:application/pdf;base64,"))
                || url.is_some_and(|url| !url.starts_with("https://"))
            {
                return Err(AdapterError::parameter_unsupported_for("document.source"));
            }
            let source = match (data, url) {
                (Some(value), None) | (None, Some(value)) => value,
                _ => return Err(AdapterError::invalid_request()),
            };
            let detail = optional_text(part, "detail")?;
            if detail.is_some_and(|detail| !matches!(detail, "auto" | "low" | "high")) {
                return Err(AdapterError::invalid_request().with_parameter("document.detail"));
            }
            document(
                source.into(),
                optional_text(part, "filename")?.map(str::to_owned),
                detail.map(str::to_owned),
            )
        }
        (WireApi::ChatCompletions, "file") => {
            checked(part, &["type", "file"])?;
            let file = part.get("file").ok_or_else(AdapterError::invalid_request)?;
            checked(file, &["file_data", "file_id", "filename"])?;
            if file.get("file_id").is_some_and(|value| !value.is_null()) {
                return Err(AdapterError::parameter_unsupported_for("input.file_id"));
            }
            let data = required_text(file, "file_data")?;
            if !data.starts_with("data:application/pdf;base64,") {
                return Err(AdapterError::parameter_unsupported_for("document.source"));
            }
            document(
                data.into(),
                optional_text(file, "filename")?.map(str::to_owned),
                None,
            )
        }
        (WireApi::Messages, "document") => {
            super::super::super::options::messages_fields(
                part,
                &["type", "source", "title", "context", "citations"],
            )?;
            if part.get("context").is_some_and(|value| !value.is_null()) {
                return Err(AdapterError::parameter_unsupported_for("document.context"));
            }
            if let Some(citations) = part.get("citations").filter(|value| !value.is_null()) {
                checked(citations, &["enabled"])?;
                if super::super::super::optional_bool(citations, "enabled")? == Some(true) {
                    return Err(AdapterError::parameter_unsupported_for(
                        "document.citations",
                    ));
                }
            }
            let source = part
                .get("source")
                .ok_or_else(AdapterError::invalid_request)?;
            checked(source, &["type", "media_type", "data", "url"])?;
            let url = match required_text(source, "type")? {
                "base64" => {
                    if required_text(source, "media_type")? != "application/pdf" {
                        return Err(AdapterError::parameter_unsupported_for(
                            "document.source.media_type",
                        ));
                    }
                    if source.get("url").is_some_and(|value| !value.is_null()) {
                        return Err(AdapterError::invalid_request());
                    }
                    format!(
                        "data:application/pdf;base64,{}",
                        required_text(source, "data")?
                    )
                }
                "url" => {
                    checked(source, &["type", "url"])?;
                    required_text(source, "url")?.into()
                }
                _ => return Err(AdapterError::parameter_unsupported_for("document.source")),
            };
            document(url, optional_text(part, "title")?.map(str::to_owned), None)
        }
        _ => Err(AdapterError::parameter_unsupported()),
    }
}

pub(super) fn document(
    url: String,
    filename: Option<String>,
    detail: Option<String>,
) -> AdapterResult<Block> {
    if let Some(data) = url.strip_prefix("data:application/pdf;base64,") {
        if data.is_empty() {
            return Err(AdapterError::invalid_request().with_parameter("document.source"));
        }
    } else if !url.starts_with("https://") {
        return Err(AdapterError::parameter_unsupported_for("document.source"));
    }
    Ok(Block::Document {
        url,
        filename,
        detail,
    })
}
