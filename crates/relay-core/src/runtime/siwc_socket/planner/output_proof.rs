use super::replay_item::METADATA;
use super::{Fingerprint, ReplayItem, fingerprint};
use crate::usage::{
    SocketOutputMismatch, SocketOutputReconciliation, SocketReplayField, SocketReplayItemKind,
    SocketReplayValueKind, SocketUnknownFieldChanges,
};
use serde_json::Value;
use std::collections::BTreeMap;

/// Diagnostics add at most 32 small field proofs per socket; never retain bodies.
pub(super) const MAX_ITEMS: usize = 32;
pub(super) const MAX_UNKNOWN_FIELDS: usize = 16;

/// Only a drop of optional completion status or redundant SDK parse data
/// can match. Keep opaque or changed fields and the stored response intact.
pub(super) fn function_projections(
    replay: &ReplayItem<'_>,
) -> impl Iterator<Item = (Fingerprint, SocketOutputReconciliation)> + use<> {
    use SocketOutputReconciliation::*;
    let mut proofs = [None; 3];
    let value = replay.raw;
    if value["type"] == "function_call"
        && value
            .get("status")
            .is_none_or(|status| status == "completed")
        && ["id", "call_id", "name"].into_iter().all(|field| {
            value[field]
                .as_str()
                .is_some_and(|id| !id.trim().is_empty())
        })
        && let Some(arguments) = value["arguments"].as_str()
    {
        let status = value.get("status");
        let parsed = value.get("parsed_arguments").filter(|decoration| {
            decoration.is_null()
                || crate::strict_json::parse(arguments).is_ok_and(|args| args == **decoration)
        });
        if status.is_some() {
            proofs[0] = Some((
                replay.projected_hash(&["status"]),
                OmittedCompletedFunctionStatus,
            ));
        }
        if parsed.is_some() {
            proofs[1] = Some((
                replay.projected_hash(&["parsed_arguments"]),
                OmittedParsedFunctionArguments,
            ));
            if status.is_some() {
                proofs[2] = Some((
                    replay.projected_hash(&["parsed_arguments", "status"]),
                    OmittedOptionalFunctionFields,
                ));
            }
        }
    }
    proofs.into_iter().flatten()
}

const FIELDS: [(&str, SocketReplayField); 29] = [
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
    (METADATA, SocketReplayField::Metadata),
    ("metadata", SocketReplayField::ItemMetadata),
    ("execution", SocketReplayField::Execution),
    ("tools", SocketReplayField::Tools),
    ("action", SocketReplayField::Action),
    ("author", SocketReplayField::Author),
    ("recipient", SocketReplayField::Recipient),
    ("result", SocketReplayField::Result),
    ("revised_prompt", SocketReplayField::RevisedPrompt),
    ("created_by", SocketReplayField::CreatedBy),
    ("agent", SocketReplayField::Agent),
];

pub(super) fn is_extension(name: &str) -> bool {
    !FIELDS.iter().any(|(known, _)| name == *known)
}

/// Hashes remain volatile and are deliberately not serializable or debuggable.
pub(super) struct OutputProof {
    fields: [Option<Fingerprint>; FIELDS.len()],
    other: Option<Fingerprint>,
    unknown: BTreeMap<Fingerprint, UnknownProof>,
    truncated: bool,
    kind: SocketReplayItemKind,
    omittable_created_by: bool,
    omittable_metadata: bool,
}

struct UnknownProof {
    value: Fingerprint,
    kind: SocketReplayValueKind,
}

impl OutputProof {
    pub(super) fn new(replay: &ReplayItem<'_>) -> Self {
        let value = replay.raw;
        let omittable_created_by =
            value.get("created_by").is_some() && replay.get("created_by").is_none();
        let omittable_metadata = value.get(METADATA).is_some() && replay.get(METADATA).is_none();
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
        let fields = std::array::from_fn(|index| replay.get(FIELDS[index].0).map(fingerprint));
        let mut unknown = BTreeMap::new();
        let mut unknown_count = 0;
        let other = if let Some(object) = value.as_object() {
            let extensions = object.iter().filter(|(name, _)| is_extension(name));
            for (name, value) in extensions.clone() {
                unknown_count += 1;
                let key = crate::json_fingerprint::sha256(name).expect("JSON string serializes");
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
                        kind: SocketReplayValueKind::from(value),
                    },
                );
                if unknown.len() > MAX_UNKNOWN_FIELDS {
                    unknown.pop_last();
                }
            }
            (unknown_count != 0).then(|| {
                crate::json_fingerprint::canonical_fields_sha256(extensions)
                    .expect("JSON serializes")
            })
        } else {
            Some(fingerprint(value))
        };
        Self {
            fields,
            other,
            unknown,
            truncated: !value.is_object() || unknown_count > MAX_UNKNOWN_FIELDS,
            kind,
            omittable_created_by,
            omittable_metadata,
        }
    }

    pub(super) fn recognized_omission(
        &self,
        replayed: &Value,
        paired_turn_omitted: bool,
    ) -> Option<SocketOutputReconciliation> {
        if !paired_turn_omitted && self.omittable_metadata && replayed.get(METADATA).is_none() {
            return Some(SocketOutputReconciliation::OmittedAllowlistedOutputFields);
        }
        (self.omittable_created_by && replayed.get("created_by").is_none()).then_some(
            if self.kind == SocketReplayItemKind::Reasoning {
                SocketOutputReconciliation::OmittedReasoningCreatedBy
            } else {
                SocketOutputReconciliation::OmittedAllowlistedOutputFields
            },
        )
    }

    pub(super) fn mismatch(&self, index: usize, current: Option<&Value>) -> SocketOutputMismatch {
        let current = current.map(|item| Self::new(&ReplayItem::new(item)));
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
                    changes.null_only &= before.kind == SocketReplayValueKind::Null;
                    changes.single_removed_type = Some(before.kind);
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
                    changes.null_only &= after.kind == SocketReplayValueKind::Null;
                }
            }
        }
        if changes.truncated || changes.removed != 1 || changes.added != 0 || changes.changed != 0 {
            changes.single_removed_type = None;
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
