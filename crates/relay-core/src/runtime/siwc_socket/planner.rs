use crate::usage::{FullRequestReason, SocketOutputMismatch, SocketOutputReconciliation};
use serde_json::{Value, json};

mod catalog;
pub(crate) use catalog::CatalogReplay;
#[cfg(debug_assertions)]
mod local_probe;
mod output_proof;
mod replay_item;
use replay_item::{CompletedItem, ItemFingerprint, ReplayItem};

const MAX_ITEMS: usize = 16_384;

pub(super) type Fingerprint = [u8; 32];

pub(super) fn fingerprint(value: &Value) -> Fingerprint {
    crate::json_fingerprint::canonical_sha256(value).expect("JSON serializes")
}

/// Content-free proof of the complete native history, after all adapters.
pub(super) struct RequestState {
    settings: Fingerprint,
    private_catalog: Option<Fingerprint>,
    receipt: Option<catalog::Receipt>,
    input: Vec<ItemFingerprint>,
    prewarm: bool,
    diagnostics: bool,
    #[cfg(debug_assertions)]
    field_probe: Option<local_probe::FieldProbe>,
}

pub(super) struct Checkpoint {
    pub(super) catalog_approval: Option<super::CatalogCheckpointApproval>,
    request: RequestState,
    output: Vec<CompletedItem>,
    function_projections: Vec<(usize, Fingerprint, SocketOutputReconciliation)>,
    output_proofs: Vec<output_proof::OutputProof>,
    response_id: String,
}

pub(super) struct Plan {
    pub(super) payload: String,
    pub(super) web_search_declared: Option<bool>,
    pub(super) source_model: Option<String>,
    pub(super) state: RequestState,
    pub(super) full_request_reason: Option<FullRequestReason>,
    pub(super) output_mismatch: Option<SocketOutputMismatch>,
    pub(super) output_reconciled: Option<SocketOutputReconciliation>,
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
        private_catalog: None,
        receipt: None,
        input: items
            .iter()
            .map(|item| ReplayItem::new(item).proof())
            .collect(),
        prewarm,
        diagnostics: false,
        #[cfg(debug_assertions)]
        field_probe: None,
    };
    object.insert("input".into(), input);
    if prewarm {
        object.insert("generate".into(), Value::Bool(false));
    }
    Some(PreparedRequest { request, state })
}

impl PreparedRequest {
    pub(super) fn session_key(
        &self,
        local_key: &str,
        candidate: &str,
        context: &str,
        url: &url::Url,
        workload: crate::protocol::UsageWorkload,
    ) -> Option<Fingerprint> {
        let model = self.request.get("model")?.as_str()?;
        if model.trim().is_empty() {
            return None;
        }
        Some(fingerprint(&json!([
            local_key,
            candidate,
            context,
            url.as_str(),
            workload,
            model
        ])))
    }

    pub(super) fn with_diagnostics(mut self, enabled: bool, _client_context: Option<&str>) -> Self {
        self.state.diagnostics = enabled;
        #[cfg(debug_assertions)]
        {
            self.state.field_probe = _client_context
                .filter(|_| enabled)
                .and_then(local_probe::FieldProbe::for_context);
        }
        self
    }

    #[cfg(test)]
    pub(super) fn plan(self, previous: Option<&Checkpoint>) -> Plan {
        self.plan_catalog(previous, None)
    }

    pub(super) fn plan_catalog(
        self,
        previous: Option<&Checkpoint>,
        catalog: Option<&CatalogReplay>,
    ) -> Plan {
        let Self {
            mut request,
            mut state,
        } = self;
        let contract = catalog.map(|catalog| catalog.contract);
        state.receipt = catalog.and_then(CatalogReplay::receipt);
        state.private_catalog = state.receipt.as_ref().and(contract);
        let object = request.as_object_mut().expect("parsed request object");
        let replayed = object["input"].as_array().expect("parsed input array");
        let previous = previous.filter(|checkpoint| {
            checkpoint
                .catalog_approval
                .as_ref()
                .is_none_or(super::CatalogCheckpointApproval::is_approved)
        });
        let mut mismatch_index = None;
        let continuation = previous
            .ok_or(FullRequestReason::SocketCheckpointUnavailable)
            .and_then(|previous| {
                if state.settings != previous.request.settings
                    || previous
                        .request
                        .private_catalog
                        .is_some_and(|required| Some(required) != contract)
                {
                    return Err(FullRequestReason::SocketSettingsChanged);
                }
                let mut public = if state.input.starts_with(&previous.request.input) {
                    None
                } else {
                    Some(
                        catalog
                            .and_then(|catalog| catalog.cursor(previous, &state.input, replayed))
                            .ok_or(FullRequestReason::SocketHistoryChanged)?,
                    )
                };
                let mut reconciled = None;
                let mut offset = public
                    .as_ref()
                    .map_or(previous.request.input.len(), |p| p.index);
                for (index, expected) in previous.output.iter().enumerate() {
                    if let Some(cursor) = &mut public {
                        offset = cursor
                            .before_output(offset, replayed)
                            .ok_or(FullRequestReason::SocketHistoryChanged)?;
                    }
                    let at = offset;
                    offset += 1;
                    let Some((replay, omitted_turn)) = state.input.get(at).and_then(|replay| {
                        Some((replay, expected.match_metadata(replay, &replayed[at])?))
                    }) else {
                        mismatch_index = Some((index, at));
                        return Err(FullRequestReason::SocketOutputChanged);
                    };
                    if replay.content != expected.proof.content {
                        let Some((_, _, reason)) = previous
                            .function_projections
                            .iter()
                            .find(|(i, proof, _)| *i == index && replay.content == *proof)
                        else {
                            mismatch_index = Some((index, at));
                            return Err(FullRequestReason::SocketOutputChanged);
                        };
                        reconciled = Some(merge_reconciliation(reconciled, *reason));
                    }
                    if state.diagnostics && omitted_turn {
                        reconciled = Some(merge_reconciliation(
                            reconciled,
                            SocketOutputReconciliation::OmittedDuplicateTurnMetadata,
                        ));
                    }
                    if state.diagnostics
                        && let Some(omission) = expected.proof.markers.omission(replay.markers)
                    {
                        reconciled = Some(merge_reconciliation(reconciled, omission));
                    }
                    if state.diagnostics
                        && let Some(omission) =
                            previous.output_proofs.get(index).and_then(|proof| {
                                proof.recognized_omission(&replayed[at], omitted_turn)
                            })
                    {
                        reconciled = Some(merge_reconciliation(reconciled, omission));
                    }
                }
                if public.is_some_and(|cursor| !cursor.complete()) {
                    return Err(FullRequestReason::SocketHistoryChanged);
                }
                let prewarmed =
                    previous.request.prewarm && previous.output.is_empty() && !state.prewarm;
                if state.input.len() <= offset && !(prewarmed && state.input.len() == offset) {
                    return Err(FullRequestReason::SocketNoNewInput);
                }
                Ok((offset, previous, reconciled))
            });
        let full_request_reason = continuation.as_ref().err().copied();
        let output_mismatch = (state.diagnostics
            && full_request_reason == Some(FullRequestReason::SocketOutputChanged))
        .then(|| {
            let previous = previous?;
            let (index, at) = mismatch_index?;
            let replay = replayed.get(at);
            let mismatch = previous.output_proofs.get(index)?.mismatch(index, replay);
            #[cfg(debug_assertions)]
            if state.field_probe.is_some()
                && let Some(probe) = &previous.request.field_probe
                && let Some(name) = probe.removed_name(&mismatch, replay)
            {
                local_probe::show_once(name);
            }
            Some(mismatch)
        })
        .flatten();
        let mut output_reconciled = None;
        if let Ok((offset, previous, reconciled)) = continuation {
            // A delta keeps that socket's private catalog even on subsequent public turns.
            state.private_catalog = state.private_catalog.or(previous.request.private_catalog);
            if state.diagnostics {
                output_reconciled = reconciled;
            }
            object.insert("previous_response_id".into(), json!(previous.response_id));
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
            web_search_declared: state
                .diagnostics
                .then(|| crate::tool_policy::responses_web_search_declared(&request)),
            payload: serde_json::to_string(&request).expect("JSON serializes"),
            state,
            full_request_reason,
            output_mismatch,
            output_reconciled,
        }
    }
}

fn merge_reconciliation(
    previous: Option<SocketOutputReconciliation>,
    next: SocketOutputReconciliation,
) -> SocketOutputReconciliation {
    use SocketOutputReconciliation::*;
    match (previous, next) {
        (None, _) => next,
        (Some(prior), _) if prior == next => prior,
        (Some(prior), _) => {
            let function_only = |reason| {
                matches!(
                    reason,
                    OmittedCompletedFunctionStatus
                        | OmittedParsedFunctionArguments
                        | OmittedOptionalFunctionFields
                )
            };
            if function_only(prior) && function_only(next) {
                OmittedOptionalFunctionFields
            } else {
                OmittedAllowlistedOutputFields
            }
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
        let mut checkpoint = Checkpoint {
            catalog_approval: None,
            request: self,
            output: Vec::with_capacity(output.len()),
            function_projections: Vec::new(),
            output_proofs: Vec::new(),
            response_id: id.to_owned(),
        };
        #[cfg(debug_assertions)]
        if let Some(probe) = &mut checkpoint.request.field_probe
            && let Some(first) = output.first()
        {
            probe.capture(first);
        }
        for (index, item) in output.iter().enumerate() {
            let replay = ReplayItem::new(item);
            checkpoint.output.push(replay.completed());
            if checkpoint.request.diagnostics && index < output_proof::MAX_ITEMS {
                checkpoint
                    .output_proofs
                    .push(output_proof::OutputProof::new(&replay));
            }
            let remaining = 3 * output_proof::MAX_ITEMS - checkpoint.function_projections.len();
            if remaining != 0 {
                checkpoint.function_projections.extend(
                    output_proof::function_projections(&replay)
                        .take(remaining)
                        .map(|(proof, reason)| (index, proof, reason)),
                );
            }
        }
        Some(checkpoint)
    }
}
