use super::{AdapterError, AdapterResult};
#[cfg(test)]
use super::{TranslatedResponse, TranslationRequest};
use serde_json::Value;

#[cfg(test)]
pub fn translate_gemini_response(
    request: TranslationRequest,
    upstream: &Value,
) -> AdapterResult<TranslatedResponse> {
    request.translate(upstream)
}

pub(crate) fn candidate_incomplete_reason(
    reason: Option<&str>,
) -> AdapterResult<Option<&'static str>> {
    match reason {
        Some("STOP") => Ok(None),
        Some("MAX_TOKENS") => Ok(Some("max_output_tokens")),
        Some(
            "SAFETY"
            | "RECITATION"
            | "LANGUAGE"
            | "BLOCKLIST"
            | "PROHIBITED_CONTENT"
            | "SPII"
            | "IMAGE_SAFETY"
            | "IMAGE_PROHIBITED_CONTENT"
            | "IMAGE_RECITATION"
            | "ESCALATION",
        ) => Ok(Some("content_filter")),
        _ => Err(AdapterError::upstream_response_invalid()),
    }
}

pub(crate) fn prompt_blocked(upstream: &Value) -> Result<bool, ()> {
    let Some(reason) = upstream.pointer("/promptFeedback/blockReason") else {
        return Ok(false);
    };
    if reason.as_str() == Some("BLOCK_REASON_UNSPECIFIED") {
        return Ok(false);
    }
    if !matches!(
        reason.as_str(),
        Some("SAFETY" | "OTHER" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "IMAGE_SAFETY")
    ) || upstream
        .get("candidates")
        .is_some_and(|value| value.as_array().is_none_or(|items| !items.is_empty()))
    {
        return Err(());
    }
    Ok(true)
}

pub(crate) fn gemini_incomplete(upstream: &Value) -> bool {
    crate::protocol::completion::ResponseCompletion::for_protocol(crate::WireApi::Gemini, upstream)
        == crate::protocol::completion::ResponseCompletion::Incomplete
}
