use super::{Fingerprint, fingerprint};
use crate::usage::SocketOutputReconciliation;
use serde_json::Value;
use std::borrow::Cow;

pub(super) const METADATA: &str = "internal_chat_message_metadata_passthrough";

#[derive(PartialEq, Eq)]
pub(super) struct ItemFingerprint {
    pub(super) content: Fingerprint,
    pub(super) markers: OutputMarkers,
    turn_id: Option<Box<Fingerprint>>,
}

pub(super) struct CompletedItem {
    pub(super) proof: ItemFingerprint,
    duplicate_turn: bool,
}

impl CompletedItem {
    /// Some(true) means the replay dropped the exact duplicate trace from this output.
    pub(super) fn match_metadata(&self, replay: &ItemFingerprint, raw: &Value) -> Option<bool> {
        if !self.proof.markers.permits(replay.markers, raw) {
            return None;
        }
        if self.proof.turn_id == replay.turn_id {
            return Some(false);
        }
        (self.duplicate_turn
            && raw.get("metadata").is_none()
            && raw[METADATA]
                .get("turn_id")
                .is_none_or(|id| Some(&fingerprint(id)) == self.proof.turn_id.as_deref()))
        .then_some(true)
    }
}

/// Part of exact prior-input equality. Only a completed output may lose a marker.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct OutputMarkers {
    root_agent: bool,
    completed_message: bool,
}

impl OutputMarkers {
    pub(super) fn permits(self, replay: Self, raw: &Value) -> bool {
        (self.root_agent == replay.root_agent
            || (self.root_agent && raw.get("agent").is_none() && compatible_creator(raw)))
            && (self.completed_message == replay.completed_message
                || (self.completed_message && raw.get("status").is_none()))
    }

    pub(super) fn omission(self, replay: Self) -> Option<SocketOutputReconciliation> {
        use SocketOutputReconciliation::*;
        match (
            self.root_agent && !replay.root_agent,
            self.completed_message && !replay.completed_message,
        ) {
            (true, true) => Some(OmittedAllowlistedOutputFields),
            (true, false) => Some(OmittedRootAgentAttribution),
            (false, true) => Some(OmittedCompletedMessageStatus),
            (false, false) => None,
        }
    }
}

/// Comparison-only view. Text, tool arguments and ciphertext stay borrowed.
pub(super) struct ReplayItem<'a> {
    pub(super) raw: &'a Value,
    kind: Option<&'a str>,
    metadata: Option<Cow<'a, Value>>,
    markers: OutputMarkers,
    turn_id: Option<&'a Value>,
}

impl<'a> ReplayItem<'a> {
    pub(super) fn new(raw: &'a Value) -> Self {
        let kind = raw.get("type").and_then(Value::as_str);
        let metadata = raw.get(METADATA).and_then(|value| match kind {
            Some(
                "message" | "reasoning" | "function_call" | "custom_tool_call" | "web_search_call",
            ) => replay_metadata(value),
            _ => Some(Cow::Borrowed(value)),
        });
        // The SDK's producing agent is distinct from a tool caller or message recipient.
        let identified_output = id_present(raw, "id")
            && match kind {
                Some("reasoning") => true,
                Some("message") => raw["role"] == "assistant",
                Some("function_call" | "custom_tool_call") => id_present(raw, "call_id"),
                _ => false,
            };
        let root_attribution = identified_output
            && raw
                .get("agent")
                .and_then(Value::as_object)
                .is_some_and(|agent| {
                    agent.len() == 1
                        && agent.get("agent_name").and_then(Value::as_str) == Some("/root")
                })
            && compatible_creator(raw);
        let completed_search = kind == Some("web_search_call")
            && raw["status"] == "completed"
            && id_present(raw, "id");
        Self {
            raw,
            kind,
            metadata,
            turn_id: if identified_output || completed_search {
                item_turn_id(raw)
            } else {
                None
            },
            markers: OutputMarkers {
                root_agent: root_attribution,
                completed_message: kind == Some("message")
                    && raw["role"] == "assistant"
                    && raw["status"] == "completed"
                    && id_present(raw, "id")
                    && raw["content"].is_array(),
            },
        }
    }

    pub(super) fn proof(&self) -> ItemFingerprint {
        ItemFingerprint {
            content: self.projected_hash(&[]),
            markers: self.markers,
            turn_id: self.turn_id.map(|id| Box::new(fingerprint(id))),
        }
    }

    pub(super) fn completed(&self) -> CompletedItem {
        CompletedItem {
            proof: self.proof(),
            duplicate_turn: self
                .turn_id
                .is_some_and(|id| self.raw[METADATA].get("turn_id") == Some(id)),
        }
    }

    pub(super) fn projected_hash(&self, omit: &[&str]) -> Fingerprint {
        let Some(object) = self.raw.as_object() else {
            return fingerprint(self.raw);
        };
        let fields = object.iter().filter_map(|(name, value)| {
            if (self.markers.root_agent && name == "agent")
                || (self.markers.completed_message && name == "status")
                || (self.turn_id.is_some() && name == "metadata")
                || omit.contains(&name.as_str())
            {
                return None;
            }
            self.project(name, value).map(|value| (name, value))
        });
        crate::json_fingerprint::canonical_fields_sha256(fields).expect("JSON serializes")
    }

    pub(super) fn get(&self, field: &str) -> Option<&Value> {
        self.project(field, self.raw.get(field)?)
    }

    fn project<'b>(&'b self, field: &str, value: &'b Value) -> Option<&'b Value> {
        if field == METADATA {
            return self.metadata.as_deref();
        }
        let kind = self.kind;
        let absent = value.is_null()
            && (crate::protocol::response_snapshots::is_nullable_item_field(kind, field)
                || matches!(
                    (kind, field),
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
        let reasoning_default = kind == Some("reasoning")
            && ((field == "status" && value == "completed")
                || (field == "content" && value.as_array().is_some_and(Vec::is_empty)));
        let provenance = field == "created_by"
            && value.is_string()
            && (matches!(
                kind,
                Some("reasoning" | "function_call" | "custom_tool_call" | "web_search_call")
            ) || (kind == Some("message") && self.raw["role"] == "assistant"));
        (!absent && !reasoning_default && !provenance).then_some(value)
    }
}

fn id_present(item: &Value, field: &str) -> bool {
    item[field].as_str().is_some_and(|id| !id.trim().is_empty())
}

/// The SIWC item trace observed on the wire is exactly one hyphenated UUID.
fn item_turn_id(item: &Value) -> Option<&Value> {
    let metadata = item.get("metadata")?.as_object()?;
    let id = metadata.get("turn_id")?;
    let text = id.as_str()?;
    (metadata.len() == 1
        && text.len() == 36
        && text.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        }))
    .then_some(id)
}

fn compatible_creator(item: &Value) -> bool {
    item.get("created_by")
        .is_none_or(|creator| creator == "/root")
}

fn replay_metadata(value: &Value) -> Option<Cow<'_, Value>> {
    let Some(object) = value.as_object() else {
        return (!value.is_null()).then_some(Cow::Borrowed(value));
    };
    let is_telemetry = |name: &str| {
        matches!(
            name,
            "turn_id"
                | "create_time"
                | "content_token_count"
                | "content_item_kinds"
                | "content_item_metadata"
        )
    };
    if object.keys().all(|key| is_telemetry(key)) {
        return None;
    }
    if !object.keys().any(|key| is_telemetry(key)) {
        return Some(Cow::Borrowed(value));
    }
    Some(Cow::Owned(Value::Object(
        object
            .iter()
            .filter(|(key, _)| !is_telemetry(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )))
}
