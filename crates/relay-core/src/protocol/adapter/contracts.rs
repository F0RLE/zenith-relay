mod bridge;
mod error;
mod prepared;
mod reasoning;
mod replay;
mod source;
mod tools;
mod validation;

pub(in crate::protocol::adapter) use bridge::prepare_bridge_state;
pub use bridge::{MessagesBridgeRequest, MessagesBridgeResponse, MessagesBridgeState};
pub use error::{AdapterError, AdapterResult};
pub use prepared::{AdapterResponse, PreparedAdapterRequest};
pub use reasoning::MessagesReasoningMode;
pub use source::{AdapterRequestContext, SourceAdapter, UpstreamProtocol};
pub(in crate::protocol::adapter) use tools::{
    bridged_namespace_tool_name, request_tool_catalog, ClientToolTarget, ResponsesToolKind,
    TranslatedTools,
};

pub(in crate::protocol::adapter) use replay::custom_tool_item_id;
pub use replay::NativeResponsesReplayState;
pub(crate) use replay::{
    remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids,
    repair_custom_tool_item_ids,
};
pub(in crate::protocol::adapter) use validation::{
    validate_bridge_fields, validate_responses_bridge_request,
};
