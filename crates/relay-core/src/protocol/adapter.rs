//! Protocol adapters are organized by lifecycle: contract preparation,
//! volatile continuation storage, JSON translation, and SSE translation.
//! Keeping these boundaries explicit prevents the gateway from acquiring
//! provider-specific behavior.

mod contracts;
mod gemini;
mod messages;
mod store;
mod stream;
#[cfg(test)]
mod tests;

pub(crate) use contracts::{
    remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids,
    repair_custom_tool_item_ids,
};
pub use contracts::{
    AdapterError, AdapterRequestContext, AdapterResponse, AdapterResult, MessagesBridgeRequest,
    MessagesBridgeResponse, MessagesBridgeState, MessagesReasoningMode, NativeResponsesReplayState,
    PreparedAdapterRequest, SourceAdapter, UpstreamProtocol,
};
pub use gemini::{GeminiBridgeRequest, GeminiBridgeResponse};
pub use messages::{
    bridged_response_id, bridged_response_id_scoped, prepare_responses_to_messages,
    prepare_responses_to_messages_scoped, translate_messages_response,
};
pub use store::{MessagesBridgeStore, NativeResponsesReplayStore};
pub use stream::{AdapterStreamBridge, GeminiStreamBridge, MessagesStreamBridge};
