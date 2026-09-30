use super::contracts::{
    bridged_namespace_tool_name, custom_tool_item_id, prepare_bridge_state, request_tool_catalog,
    AdapterError, AdapterResult, ClientToolTarget, MessagesBridgeRequest, MessagesBridgeResponse,
    MessagesBridgeState, MessagesReasoningMode, ResponsesToolKind, TranslatedTools,
};
mod request;
mod response;

pub(in crate::protocol::adapter) use request::custom_tool_input_schema;
pub(crate) use request::{
    apply_cache_write_ttl, prepare_responses_to_messages_scoped_with_cache_ttl,
};
pub use request::{prepare_responses_to_messages, prepare_responses_to_messages_scoped};
pub use response::{bridged_response_id, bridged_response_id_scoped, translate_messages_response};
pub(in crate::protocol::adapter) use response::{
    custom_tool_input, messages_response_terminal, responses_output_from_messages_content,
    responses_usage, set_message_output_id, validate_messages_tool_calls,
};
