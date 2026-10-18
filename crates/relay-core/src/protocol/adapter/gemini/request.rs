//! Test convenience entry points; all conversion belongs to the shared codecs.
use super::{AdapterContinuationState, AdapterResult, MessagesReasoningMode, TranslationRequest};
use crate::{AdapterRequestContext, CacheWriteTtl, PreparedAdapterRequest, SourceAdapter, WireApi};
use serde_json::Value;

pub fn prepare_responses_to_gemini(
    request: &Value,
    model: &str,
    stream: bool,
    scope: &str,
    seed: &str,
) -> AdapterResult<TranslationRequest> {
    prepare_responses_to_gemini_with_reasoning(
        request,
        model,
        stream,
        MessagesReasoningMode::Disabled,
        None,
        scope,
        seed,
    )
}

pub(crate) fn prepare_responses_to_gemini_with_reasoning(
    request: &Value,
    model: &str,
    stream: bool,
    reasoning_mode: MessagesReasoningMode,
    previous: Option<AdapterContinuationState>,
    response_scope: &str,
    response_id_seed: &str,
) -> AdapterResult<TranslationRequest> {
    let prepared = SourceAdapter::ResponsesToGemini.prepare_request(AdapterRequestContext {
        client_wire_api: WireApi::Responses,
        request,
        model,
        stream,
        reasoning_mode,
        cache_write_ttl: CacheWriteTtl::Provider,
        previous,
        response_scope,
        response_id_seed,
    })?;
    let PreparedAdapterRequest::Translated { request } = prepared else {
        unreachable!()
    };
    Ok(*request)
}
