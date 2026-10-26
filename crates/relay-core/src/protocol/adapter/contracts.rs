mod bridge;
mod error;
mod history;
mod prepared;
mod reasoning;
mod replay;
mod source;
mod tools;
mod validation;

pub use bridge::{AdapterContinuationState, TranslatedResponse};
pub use error::{AdapterError, AdapterResult};
pub use prepared::{AdapterResponse, PreparedAdapterRequest};
pub use reasoning::MessagesReasoningMode;
pub(in crate::protocol::adapter) use reasoning::{ReasoningPolicy, reasoning_effort_budget};
pub use source::{AdapterRequestContext, SourceAdapter, UpstreamProtocol};
pub(in crate::protocol::adapter) use tools::{
    ClientToolTarget, ResponsesToolKind, ToolChoice, bridged_namespace_tool_name,
    bridged_tool_description, request_tool_catalog, resolve_responses_tool_choice,
};

pub use replay::NativeResponsesReplayState;
pub(in crate::protocol::adapter) use replay::custom_tool_item_id;
pub(crate) use replay::{
    remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids,
    repair_custom_tool_item_ids,
};
pub(in crate::protocol::adapter) use validation::{
    validate_bridge_fields, validate_responses_bridge_request,
};
