use crate::usage::{FullRequestReason, SocketOutputMismatch};
use serde_json::{Value, json};

mod output_proof;

const MAX_ITEMS: usize = 16_384;

pub(super) type Fingerprint = [u8; 32];

pub(super) fn fingerprint(value: &Value) -> Fingerprint {
    crate::json_fingerprint::canonical_sha256(value).expect("JSON serializes")
}

fn item_fingerprint(value: &Value) -> Fingerprint {
    fingerprint(&replay_item(value))
}

fn replay_item(value: &Value) -> std::borrow::Cow<'_, Value> {
    let Some(item) = value.as_object() else {
        return std::borrow::Cow::Borrowed(value);
    };
    let kind = item.get("type").and_then(Value::as_str);
    let mut replay = std::borrow::Cow::Borrowed(value);
    for (field, field_value) in item {
        // Codex omits completed reasoning status and empty reasoning content;
        // absent reasoning content serializes as null. Optional Codex input
        // fields omit None on replay. Normalize only the comparison proof.
        let completed_reasoning_status =
            kind == Some("reasoning") && field == "status" && field_value == "completed";
        let absent = field_value.is_null()
            && (crate::protocol::response_snapshots::is_nullable_item_field(kind, field)
                || matches!(
                    (kind, field.as_str()),
                    (Some("reasoning"), "content")
                        | (
                            Some("message" | "function_call" | "custom_tool_call"),
                            "encrypted_content"
                        )
                        | (
                            Some("function_call" | "custom_tool_call"),
                            "namespace" | "async"
                        )
                        | (Some("function_call"), "encrypted_function_args")
                ));
        let empty_reasoning_content = kind == Some("reasoning")
            && field == "content"
            && field_value.as_array().is_some_and(Vec::is_empty);
        if completed_reasoning_status || absent || empty_reasoning_content {
            replay
                .to_mut()
                .as_object_mut()
                .expect("replay object")
                .remove(field);
        }
        if field == "internal_chat_message_metadata_passthrough"
            && matches!(
                kind,
                Some("message" | "reasoning" | "function_call" | "custom_tool_call")
            )
        {
            // Codex compares continuation without client-only attribution and
            // telemetry. Preserve tool-result records and unknown extensions.
            let known = |field: &str| {
                matches!(
                    field,
                    "turn_id"
                        | "create_time"
                        | "content_token_count"
                        | "content_item_kinds"
                        | "content_item_metadata"
                )
            };
            let metadata = if field_value.is_null() {
                None
            } else if let Some(metadata) = field_value.as_object() {
                let mut metadata = metadata.clone();
                metadata.retain(|field, _| !known(field));
                (!metadata.is_empty()).then_some(Value::Object(metadata))
            } else {
                continue;
            };
            if metadata.as_ref() != Some(field_value) {
                let item = replay.to_mut().as_object_mut().expect("replay object");
                if let Some(metadata) = metadata {
                    item.insert(field.clone(), metadata);
                } else {
                    item.remove(field);
                }
            }
        }
    }
    replay
}

/// Content-free proof of the complete native history, after all adapters.
pub(super) struct RequestState {
    settings: Fingerprint,
    input: Vec<Fingerprint>,
    prewarm: bool,
    diagnostics: bool,
}

pub(super) struct Checkpoint {
    request: RequestState,
    output: Vec<Fingerprint>,
    output_proofs: Vec<output_proof::OutputProof>,
    response_id: String,
}

pub(super) struct Plan {
    pub(super) payload: String,
    pub(super) source_model: Option<String>,
    pub(super) state: RequestState,
    pub(super) full_request_reason: Option<FullRequestReason>,
    pub(super) output_mismatch: Option<SocketOutputMismatch>,
}

pub(super) struct PreparedRequest {
    request: Value,
    state: RequestState,
}

pub(super) fn parse(bytes: &[u8]) -> Option<PreparedRequest> {
    let mut request = crate::strict_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    let object = request.as_object_mut()?;
    if object.get("store") != Some(&Value::Bool(false))
        || object.get("stream") != Some(&Value::Bool(true))
        || object.contains_key("previous_response_id")
        || object.contains_key("background")
        || object.contains_key("stream_id")
        || object.contains_key("type")
        || object.contains_key("generate")
    {
        return None;
    }
    let input = object.remove("input")?;
    let items = input.as_array()?;
    if items.len() > MAX_ITEMS {
        return None;
    }
    object.remove("stream");
    let prewarm = object
        .get("prompt_cache_options")
        .and_then(|v| v.get("prewarm"))
        == Some(&Value::Bool(true));
    if prewarm
        && let Some(options) = object
            .get_mut("prompt_cache_options")
            .and_then(Value::as_object_mut)
    {
        options.remove("prewarm");
        if options.is_empty() {
            object.remove("prompt_cache_options");
        }
    }
    let mut settings = Value::Object(object.clone());
    // Per-request client tracing is not part of the referenced model prefix.
    // Keep malformed shapes strict; never alter what is sent to the provider.
    if settings.get("client_metadata").is_some_and(|metadata| {
        metadata.is_null()
            || metadata
                .as_object()
                .is_some_and(|fields| fields.values().all(Value::is_string))
    }) {
        settings.as_object_mut()?.remove("client_metadata");
    }
    // Obfuscation changes delivery, not the context held by this socket.
    // Unknown stream controls and malformed values remain part of the proof.
    if let Some(options) = settings
        .get_mut("stream_options")
        .and_then(Value::as_object_mut)
    {
        if options
            .get("include_obfuscation")
            .is_some_and(Value::is_boolean)
        {
            options.remove("include_obfuscation");
        }
        if options.is_empty() {
            settings.as_object_mut()?.remove("stream_options");
        }
    }
    // A provider comparison requests diagnostics, not a different model prefix.
    if let Some(options) = settings
        .get_mut("prompt_cache_options")
        .and_then(Value::as_object_mut)
    {
        options.remove("comparison_response_id");
        if options.get("prewarm") == Some(&Value::Bool(false)) {
            options.remove("prewarm");
        }
        if options.is_empty() {
            settings.as_object_mut()?.remove("prompt_cache_options");
        }
    }
    let state = RequestState {
        settings: fingerprint(&settings),
        input: items.iter().map(item_fingerprint).collect(),
        prewarm,
        diagnostics: false,
    };
    object.insert("input".into(), input);
    if prewarm {
        object.insert("generate".into(), Value::Bool(false));
    }
    Some(PreparedRequest { request, state })
}

impl PreparedRequest {
    pub(super) fn with_diagnostics(mut self, enabled: bool) -> Self {
        self.state.diagnostics = enabled;
        self
    }

    pub(super) fn plan(self, previous: Option<&Checkpoint>) -> Plan {
        let Self { mut request, state } = self;
        let object = request.as_object_mut().expect("parsed request object");
        let continuation = previous
            .ok_or(FullRequestReason::SocketCheckpointUnavailable)
            .and_then(|previous| {
                let offset = previous.request.input.len() + previous.output.len();
                if state.settings != previous.request.settings {
                    return Err(FullRequestReason::SocketSettingsChanged);
                }
                if !state.input.starts_with(&previous.request.input) {
                    return Err(FullRequestReason::SocketHistoryChanged);
                }
                if state.input.get(previous.request.input.len()..offset)
                    != Some(previous.output.as_slice())
                {
                    return Err(FullRequestReason::SocketOutputChanged);
                }
                let prewarmed =
                    previous.request.prewarm && previous.output.is_empty() && !state.prewarm;
                if state.input.len() <= offset && !(prewarmed && state.input.len() == offset) {
                    return Err(FullRequestReason::SocketNoNewInput);
                }
                Ok((offset, previous.response_id.as_str()))
            });
        let full_request_reason = continuation.as_ref().err().copied();
        let output_mismatch = (state.diagnostics
            && full_request_reason == Some(FullRequestReason::SocketOutputChanged))
        .then(|| {
            let previous = previous?;
            let replayed = object.get("input")?.as_array()?;
            let start = previous.request.input.len();
            let index = previous
                .output
                .iter()
                .enumerate()
                .find(|(index, expected)| state.input.get(start + *index) != Some(*expected))?
                .0;
            previous
                .output_proofs
                .get(index)
                .map(|proof| proof.mismatch(index, replayed.get(start + index)))
        })
        .flatten();
        if let Ok((offset, id)) = continuation {
            object.insert("previous_response_id".into(), json!(id));
            object
                .get_mut("input")
                .and_then(Value::as_array_mut)
                .expect("parsed input array")
                .drain(..offset);
        }
        object.insert("type".into(), json!("response.create"));
        Plan {
            source_model: object
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_owned),
            payload: serde_json::to_string(&request).expect("JSON serializes"),
            state,
            full_request_reason,
            output_mismatch,
        }
    }
}

impl RequestState {
    pub(super) fn complete(self, response: &Value) -> Option<Checkpoint> {
        if crate::protocol::completion::ResponseCompletion::for_protocol(
            crate::WireApi::Responses,
            response,
        ) != crate::protocol::completion::ResponseCompletion::Completed
        {
            return None;
        }
        let id = response.get("id")?.as_str()?;
        let output = response.get("output")?.as_array()?;
        if id.is_empty()
            || id.len() > 256
            || (output.is_empty() && !self.prewarm)
            || output.len() > MAX_ITEMS
        {
            return None;
        }
        Some(Checkpoint {
            output_proofs: if self.diagnostics {
                output
                    .iter()
                    .take(output_proof::MAX_ITEMS)
                    .map(output_proof::OutputProof::new)
                    .collect()
            } else {
                Vec::new()
            },
            request: self,
            output: output.iter().map(item_fingerprint).collect(),
            response_id: id.to_owned(),
        })
    }
}
