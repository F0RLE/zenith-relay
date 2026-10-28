//! Protocol adapters are organized by lifecycle: contract preparation,
//! volatile continuation storage, JSON translation, and SSE translation.
//! Keeping these boundaries explicit prevents the gateway from acquiring
//! provider-specific behavior.

#[cfg(test)]
mod codex_controls;
mod compaction;
mod contracts;
mod gemini;
mod messages;
mod responses_usage;
mod store;
mod stream;
#[cfg(test)]
mod tests;
mod translation;

pub(crate) use compaction::{
    BridgedCompaction, ensure_compaction_trigger, is_compaction_checkpoint_type,
    prepare_bridged_compaction, wrap_compaction_response_bytes,
};
pub use contracts::{
    AdapterContinuationState, AdapterError, AdapterRequestContext, AdapterResponse, AdapterResult,
    MessagesReasoningMode, NativeResponsesReplayState, PreparedAdapterRequest, SourceAdapter,
    TranslatedResponse, UpstreamProtocol,
};
pub(crate) use contracts::{
    remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids,
    repair_custom_tool_item_ids,
};
pub(crate) use gemini::{candidate_incomplete_reason, gemini_incomplete, prompt_blocked};
pub use messages::{
    bridged_response_id, bridged_response_id_scoped, prepare_responses_to_messages,
    prepare_responses_to_messages_scoped, translate_messages_response,
};
pub use store::{AdapterContinuationStore, NativeResponsesReplayStore};
pub use stream::AdapterStreamBridge;
pub(crate) use stream::merge_usage;
pub(crate) use translation::continuation_key as portable_history_key;
pub use translation::{TranslationRequest, TranslationStream};
