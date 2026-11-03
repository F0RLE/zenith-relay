use super::*;
use serde_json::json;

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(in crate::protocol::adapter) struct UrlCitation {
    pub start: usize,
    pub end: usize,
    url: String,
    title: String,
}

impl UrlCitation {
    pub(super) fn parse(value: &Value) -> AdapterResult<Self> {
        let invalid = AdapterError::upstream_response_invalid;
        checked(value, &["type", "start_index", "end_index", "url", "title"])
            .map_err(|_| invalid())?;
        if value.get("type").and_then(Value::as_str) != Some("url_citation") {
            return Err(invalid());
        }
        let index = |name| {
            value
                .get(name)
                .and_then(Value::as_u64)
                .and_then(|index| usize::try_from(index).ok())
                .ok_or_else(invalid)
        };
        let citation = Self {
            start: index("start_index")?,
            end: index("end_index")?,
            url: value
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?
                .into(),
            title: value
                .get("title")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?
                .into(),
        };
        let url = url::Url::parse(&citation.url).map_err(|_| invalid())?;
        if citation.start > citation.end
            || !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
        {
            return Err(invalid());
        }
        Ok(citation)
    }

    pub(super) fn responses_value(&self) -> Value {
        json!({"type":"url_citation","start_index":self.start,"end_index":self.end,
            "url":self.url,"title":self.title})
    }

    fn chat_value(&self, offset: usize) -> AdapterResult<Value> {
        let invalid = AdapterError::upstream_response_invalid;
        Ok(json!({"type":"url_citation","url_citation":{
            "start_index":offset.checked_add(self.start).ok_or_else(invalid)?,
            "end_index":offset.checked_add(self.end).ok_or_else(invalid)?,
            "url":self.url,"title":self.title}}))
    }
}

pub(super) fn parse_annotations(
    value: Option<&Value>,
    text: &str,
) -> AdapterResult<Vec<UrlCitation>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let invalid = AdapterError::upstream_response_invalid;
    let text_length = text.chars().count();
    value
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .map(|value| {
            let citation = UrlCitation::parse(value)?;
            if citation.end > text_length {
                return Err(invalid());
            }
            Ok(citation)
        })
        .collect()
}

pub(super) fn set_chat_annotations(
    response: &mut Response,
    value: Option<&Value>,
) -> AdapterResult<()> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let invalid = AdapterError::upstream_response_invalid;
    let annotations = value.as_array().ok_or_else(invalid)?;
    if annotations.is_empty() {
        return Ok(());
    }
    // Chat offsets address one concatenated message. Its decoder keeps that
    // text in one block so citations never cross generated Responses items.
    let mut text_blocks = response
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| {
            if let Block::Text(text) = block {
                Some((index, text))
            } else {
                None
            }
        });
    let (index, text) = text_blocks.next().ok_or_else(invalid)?;
    if text_blocks.next().is_some() {
        return Err(invalid());
    }
    let flattened = annotations
        .iter()
        .map(|annotation| {
            checked(annotation, &["type", "url_citation"]).map_err(|_| invalid())?;
            if annotation.get("type").and_then(Value::as_str) != Some("url_citation") {
                return Err(invalid());
            }
            let mut citation = annotation
                .get("url_citation")
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(invalid)?;
            citation.insert("type".into(), "url_citation".into());
            Ok(Value::Object(citation))
        })
        .collect::<AdapterResult<Vec<_>>>()?;
    let citations = parse_annotations(Some(&Value::Array(flattened)), text)?;
    response.citations.insert(index, citations);
    Ok(())
}

pub(super) fn chat_annotations(response: &Response) -> AdapterResult<Vec<Value>> {
    let mut offset = 0usize;
    let mut annotations = Vec::new();
    for (index, block) in response.blocks.iter().enumerate() {
        if let Block::Text(text) = block {
            if let Some(citations) = response.citations.get(&index) {
                for citation in citations {
                    annotations.push(citation.chat_value(offset)?);
                }
            }
            // Citation positions are characters, never UTF-8 byte offsets.
            offset = offset
                .checked_add(text.chars().count())
                .ok_or_else(AdapterError::upstream_response_invalid)?;
        }
    }
    Ok(annotations)
}
