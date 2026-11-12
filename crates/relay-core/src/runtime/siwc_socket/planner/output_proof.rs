use super::{Fingerprint, fingerprint, replay_item};
use crate::usage::{
    SocketOutputMismatch, SocketReplayField, SocketReplayItemKind, SocketUnknownFieldChanges,
};
use serde_json::Value;
use std::collections::BTreeMap;

/// Diagnostics add at most 32 small field proofs per socket; never retain bodies.
pub(super) const MAX_ITEMS: usize = 32;
const MAX_UNKNOWN_FIELDS: usize = 16;

const FIELDS: [(&str, SocketReplayField); 26] = [
    ("type", SocketReplayField::Type),
    ("id", SocketReplayField::Id),
    ("status", SocketReplayField::Status),
    ("role", SocketReplayField::Role),
    ("content", SocketReplayField::Content),
    ("summary", SocketReplayField::Summary),
    ("encrypted_content", SocketReplayField::EncryptedContent),
    ("phase", SocketReplayField::Phase),
    ("name", SocketReplayField::Name),
    ("namespace", SocketReplayField::Namespace),
    ("arguments", SocketReplayField::Arguments),
    ("encrypted_arguments", SocketReplayField::EncryptedArguments),
    (
        "encrypted_function_args",
        SocketReplayField::EncryptedFunctionArgs,
    ),
    ("async", SocketReplayField::Async),
    ("call_id", SocketReplayField::CallId),
    ("caller", SocketReplayField::Caller),
    ("input", SocketReplayField::Input),
    ("output", SocketReplayField::Output),
    (
        "internal_chat_message_metadata_passthrough",
        SocketReplayField::Metadata,
    ),
    ("execution", SocketReplayField::Execution),
    ("tools", SocketReplayField::Tools),
    ("action", SocketReplayField::Action),
    ("author", SocketReplayField::Author),
    ("recipient", SocketReplayField::Recipient),
    ("result", SocketReplayField::Result),
    ("revised_prompt", SocketReplayField::RevisedPrompt),
];

/// Hashes remain volatile and are deliberately not serializable or debuggable.
pub(super) struct OutputProof {
    fields: [Option<Fingerprint>; FIELDS.len()],
    other: Option<Fingerprint>,
    unknown: BTreeMap<Fingerprint, UnknownProof>,
    truncated: bool,
    kind: SocketReplayItemKind,
}

struct UnknownProof {
    value: Fingerprint,
    null: bool,
}

impl OutputProof {
    pub(super) fn new(value: &Value) -> Self {
        let value = replay_item(value);
        let kind = match value.get("type").and_then(Value::as_str) {
            Some("message") => SocketReplayItemKind::Message,
            Some("reasoning") => SocketReplayItemKind::Reasoning,
            Some("function_call") => SocketReplayItemKind::FunctionCall,
            Some("custom_tool_call") => SocketReplayItemKind::CustomToolCall,
            Some("web_search_call") => SocketReplayItemKind::WebSearchCall,
            Some("tool_search_call") => SocketReplayItemKind::ToolSearchCall,
            Some("tool_search_output") => SocketReplayItemKind::ToolSearchOutput,
            _ => SocketReplayItemKind::Other,
        };
        let fields = std::array::from_fn(|index| value.get(FIELDS[index].0).map(fingerprint));
        let mut other = value.into_owned();
        if let Some(object) = other.as_object_mut() {
            for (name, _) in FIELDS {
                object.remove(name);
            }
        }
        let mut unknown = BTreeMap::new();
        if let Some(object) = other.as_object() {
            for (name, value) in object {
                let key = fingerprint(&Value::String(name.clone()));
                if unknown.len() == MAX_UNKNOWN_FIELDS
                    && unknown
                        .last_key_value()
                        .is_some_and(|(last, _)| key > *last)
                {
                    continue;
                }
                unknown.insert(
                    key,
                    UnknownProof {
                        value: fingerprint(value),
                        null: value.is_null(),
                    },
                );
                if unknown.len() > MAX_UNKNOWN_FIELDS {
                    unknown.pop_last();
                }
            }
        }
        Self {
            fields,
            other: (!other.as_object().is_some_and(|object| object.is_empty()))
                .then(|| fingerprint(&other)),
            unknown,
            truncated: other
                .as_object()
                .is_none_or(|object| object.len() > MAX_UNKNOWN_FIELDS),
            kind,
        }
    }

    pub(super) fn mismatch(&self, index: usize, current: Option<&Value>) -> SocketOutputMismatch {
        let current = current.map(Self::new);
        let mut fields = FIELDS
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                self.fields[*index] != current.as_ref().and_then(|proof| proof.fields[*index])
            })
            .map(|(_, (_, field))| *field)
            .collect::<Vec<_>>();
        let unknown_fields = if self.other != current.as_ref().and_then(|proof| proof.other) {
            fields.push(SocketReplayField::Other);
            Some(self.unknown_changes(current.as_ref()))
        } else {
            None
        };
        SocketOutputMismatch {
            output_index: index as u32,
            fields,
            item_kind: Some(self.kind),
            unknown_fields,
        }
    }

    fn unknown_changes(&self, current: Option<&Self>) -> SocketUnknownFieldChanges {
        let mut changes = SocketUnknownFieldChanges {
            null_only: true,
            truncated: self.truncated || current.is_some_and(|proof| proof.truncated),
            ..Default::default()
        };
        for (name, before) in &self.unknown {
            match current.and_then(|proof| proof.unknown.get(name)) {
                None if current.is_none_or(|proof| proof.covers(name)) => {
                    changes.removed += 1;
                    changes.null_only &= before.null;
                }
                Some(after) if before.value != after.value => {
                    changes.changed += 1;
                    changes.null_only = false;
                }
                _ => {}
            }
        }
        if let Some(current) = current {
            for (name, after) in &current.unknown {
                if !self.unknown.contains_key(name) && self.covers(name) {
                    changes.added += 1;
                    changes.null_only &= after.null;
                }
            }
        }
        changes.null_only &= !changes.truncated;
        changes
    }

    fn covers(&self, key: &Fingerprint) -> bool {
        !self.truncated
            || self
                .unknown
                .last_key_value()
                .is_some_and(|(last, _)| key <= last)
    }
}
