//! A provider-hosted search is not a client function. Native search blocks and
//! encrypted result handles stay in the existing route-scoped continuation.
use super::*;
use serde::Serialize;
use serde_json::json;

const ID_PREFIX: &str = "ws_relay_claude_";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(in crate::protocol::adapter) struct SearchCall {
    pub(super) id: String,
    pub(super) query: String,
    pub(super) sources: Option<Vec<String>>,
}

impl SearchCall {
    pub(super) fn item(&self, completed: bool) -> AdapterResult<Value> {
        if completed && self.sources.is_none() {
            return Err(
                AdapterError::upstream_response_invalid().with_parameter("web_search.result")
            );
        }
        let mut item = json!({"type":"web_search_call","id":self.id,"status":if completed {"completed"} else {"in_progress"},"action":{"type":"search","query":self.query}});
        if completed {
            item["action"]["sources"] = self
                .sources
                .as_ref()
                .unwrap()
                .iter()
                .map(|url| json!({"type":"url","url":url}))
                .collect();
        }
        Ok(item)
    }
}

pub(super) fn tool(value: &Value) -> AdapterResult<Value> {
    checked(
        value,
        &["type", "filters", "user_location", "external_web_access"],
    )?;
    if value
        .get("external_web_access")
        .is_some_and(|value| value != true)
    {
        return Err(AdapterError::parameter_unsupported_for(
            "tools.external_web_access",
        ));
    }
    let mut mapped = json!({"type":"web_search_20250305","name":"web_search"});
    if let Some(filters) = value.get("filters").filter(|value| !value.is_null()) {
        checked(filters, &["allowed_domains"])?;
        if let Some(domains) = filters
            .get("allowed_domains")
            .filter(|value| !value.is_null())
        {
            if domains.as_array().is_none_or(|items| {
                items
                    .iter()
                    .any(|item| item.as_str().is_none_or(|domain| domain.is_empty()))
            }) {
                return Err(
                    AdapterError::invalid_request().with_parameter("tools.filters.allowed_domains")
                );
            }
            mapped["allowed_domains"] = domains.clone();
        }
    }
    if let Some(location) = value.get("user_location").filter(|value| !value.is_null()) {
        checked(location, &["type", "city", "country", "region", "timezone"])?;
        if location
            .get("type")
            .is_some_and(|kind| kind != "approximate")
            || ["city", "country", "region", "timezone"]
                .iter()
                .any(|name| {
                    location
                        .get(name)
                        .is_some_and(|value| !value.is_null() && !value.is_string())
                })
        {
            return Err(AdapterError::invalid_request().with_parameter("tools.user_location"));
        }
        mapped["user_location"] = location.clone();
        mapped["user_location"]["type"] = "approximate".into();
    }
    Ok(mapped)
}

fn direct_caller(value: &Value) -> AdapterResult<()> {
    if let Some(caller) = value.get("caller").filter(|value| !value.is_null()) {
        checked(caller, &["type"])?;
        if caller.get("type") != Some(&json!("direct")) {
            return Err(
                AdapterError::upstream_response_invalid().with_parameter("web_search.caller")
            );
        }
    }
    Ok(())
}

pub(in crate::protocol::adapter) fn start(native: &Value) -> AdapterResult<SearchCall> {
    let invalid = AdapterError::upstream_response_invalid;
    checked(native, &["type", "id", "name", "input", "caller"]).map_err(|_| invalid())?;
    direct_caller(native)?;
    let id = required_text(native, "id").map_err(|_| invalid())?;
    if native.get("name") != Some(&json!("web_search")) || !id.starts_with("srvtoolu_") {
        return Err(invalid().with_parameter("server_tool_use"));
    }
    Ok(SearchCall {
        id: format!("{ID_PREFIX}{id}"),
        query: String::new(),
        sources: None,
    })
}

pub(super) fn query(input: &Value) -> AdapterResult<String> {
    checked(input, &["query"]).map_err(|_| AdapterError::upstream_response_invalid())?;
    required_text(input, "query")
        .map(str::to_owned)
        .map_err(|_| AdapterError::upstream_response_invalid())
}

pub(super) fn finish(blocks: &mut [Block], native: &Value) -> AdapterResult<()> {
    let invalid = AdapterError::upstream_response_invalid;
    checked(native, &["type", "tool_use_id", "content", "caller"]).map_err(|_| invalid())?;
    direct_caller(native)?;
    let id = format!(
        "{ID_PREFIX}{}",
        required_text(native, "tool_use_id").map_err(|_| invalid())?
    );
    let Some(search) = blocks.iter_mut().find_map(|block| match block {
        Block::WebSearch(search) if search.id == id => Some(search),
        _ => None,
    }) else {
        return Err(invalid().with_parameter("web_search.tool_use_id"));
    };
    if search.sources.is_some() || search.query.is_empty() {
        return Err(invalid().with_parameter("web_search.result"));
    }
    let mut sources = Vec::new();
    for result in native
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid().with_parameter("web_search.result"))?
    {
        checked(
            result,
            &["type", "url", "title", "encrypted_content", "page_age"],
        )
        .map_err(|_| invalid())?;
        if result.get("type") != Some(&json!("web_search_result"))
            || !result.get("title").is_some_and(Value::is_string)
            || !result
                .get("encrypted_content")
                .is_some_and(Value::is_string)
            || result
                .get("page_age")
                .is_some_and(|value| !value.is_null() && !value.is_string())
        {
            return Err(invalid());
        }
        let url = required_text(result, "url").map_err(|_| invalid())?;
        validate_url(url)?;
        sources.push(url.to_owned());
    }
    search.sources = Some(sources);
    Ok(())
}

fn validate_url(value: &str) -> AdapterResult<()> {
    let url = url::Url::parse(value).map_err(|_| AdapterError::upstream_response_invalid())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(AdapterError::upstream_response_invalid());
    }
    Ok(())
}

pub(in crate::protocol::adapter) fn validate_citations(value: &Value) -> AdapterResult<()> {
    if value.is_null() {
        return Ok(());
    }
    let invalid = AdapterError::upstream_response_invalid;
    for citation in value.as_array().ok_or_else(invalid)? {
        checked(
            citation,
            &["type", "url", "title", "encrypted_index", "cited_text"],
        )
        .map_err(|_| invalid())?;
        if citation.get("type") != Some(&json!("web_search_result_location"))
            || !citation
                .get("encrypted_index")
                .is_some_and(Value::is_string)
            || !citation.get("cited_text").is_some_and(Value::is_string)
            || citation
                .get("title")
                .is_some_and(|value| !value.is_null() && !value.is_string())
        {
            return Err(invalid());
        }
        validate_url(required_text(citation, "url").map_err(|_| invalid())?)?;
    }
    Ok(())
}

pub(super) fn citations(native: &Value, text: &str) -> AdapterResult<Vec<citations::UrlCitation>> {
    let Some(items) = native.get("citations").filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    validate_citations(items)?;
    items.as_array().unwrap().iter().map(|item| {
        // Messages citations qualify their text block; cited_text quotes the
        // source and must not be searched for inside the assistant's answer.
        citations::UrlCitation::parse(&json!({"type":"url_citation","start_index":0,"end_index":text.chars().count(),"url":item["url"],"title":item.get("title").and_then(Value::as_str).unwrap_or("")}))
    }).collect()
}

pub(super) fn history(item: &Value) -> AdapterResult<SearchCall> {
    checked(item, &["type", "id", "status", "action"])?;
    let id = required_text(item, "id")?;
    if !id.starts_with(ID_PREFIX) || item.get("status") != Some(&json!("completed")) {
        return Err(AdapterError::parameter_unsupported_for("input.type"));
    }
    let action = item
        .get("action")
        .ok_or_else(AdapterError::invalid_request)?;
    checked(action, &["type", "query", "sources"])?;
    if action.get("type") != Some(&json!("search")) {
        return Err(AdapterError::invalid_request());
    }
    let query = required_text(action, "query")?.to_owned();
    let sources = action
        .get("sources")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::invalid_request)?
        .iter()
        .map(|source| {
            checked(source, &["type", "url"])?;
            if source.get("type") != Some(&json!("url")) {
                return Err(AdapterError::invalid_request());
            }
            let url = required_text(source, "url")?;
            validate_url(url).map_err(|_| {
                AdapterError::invalid_request()
                    .with_parameter("input.web_search_call.action.sources")
            })?;
            Ok(url.to_owned())
        })
        .collect::<AdapterResult<Vec<_>>>()?;
    Ok(SearchCall {
        id: id.into(),
        query,
        sources: Some(sources),
    })
}

pub(super) fn validate_output(blocks: &[Block], allowed: bool) -> AdapterResult<()> {
    let mut seen = BTreeSet::new();
    for block in blocks {
        if let Block::WebSearch(search) = block
            && (!allowed || search.sources.is_none() || !seen.insert(&search.id))
        {
            return Err(AdapterError::upstream_response_invalid().with_parameter("web_search"));
        }
    }
    Ok(())
}
