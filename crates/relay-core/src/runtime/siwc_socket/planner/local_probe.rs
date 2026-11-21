use super::output_proof::{MAX_UNKNOWN_FIELDS, is_extension};
use crate::usage::{SocketOutputMismatch, SocketReplayField, SocketReplayValueKind};
use serde_json::Value;
use std::io::{self, IsTerminal, Write};
use std::sync::{Once, OnceLock};

static SHOWN: Once = Once::new();

/// Only a selected debug conversation retains schema names, never their values.
#[derive(Default)]
pub(super) struct FieldProbe(Vec<Box<str>>);

impl FieldProbe {
    pub(super) fn for_context(context: &str) -> Option<Self> {
        static CONTEXTS: OnceLock<Vec<String>> = OnceLock::new();
        if SHOWN.is_completed() || !io::stderr().is_terminal() {
            return None;
        }
        let selected = CONTEXTS.get_or_init(|| {
            std::env::var("ZENITH_RELAY_SIWC_PROBE_SESSION")
                .map(|session| crate::gateway::siwc_probe_contexts(&session))
                .unwrap_or_default()
        });
        selected.iter().any(|id| id == context).then(Self::default)
    }

    pub(super) fn capture(&mut self, first: &Value) {
        let Some(object) = first.as_object() else {
            return;
        };
        self.0 = object
            .iter()
            .filter(|(name, value)| {
                value.is_object() && is_extension(name) && is_schema_identifier(name)
            })
            .take(MAX_UNKNOWN_FIELDS)
            .map(|(name, _)| name.as_str().into())
            .collect();
    }

    pub(super) fn removed_name<'a>(
        &'a self,
        mismatch: &SocketOutputMismatch,
        replay: Option<&Value>,
    ) -> Option<&'a str> {
        if mismatch.output_index != 0
            || mismatch.fields != [SocketReplayField::Other]
            || mismatch.unknown_fields?.single_removed_type != Some(SocketReplayValueKind::Object)
        {
            return None;
        }
        // The proof already ruled out truncation and any other unknown difference.
        let replay = replay?.as_object()?;
        self.0
            .iter()
            .find(|name| !replay.contains_key(name.as_ref()))
            .map(AsRef::as_ref)
    }
}

fn is_schema_identifier(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=64).contains(&bytes.len())
        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

pub(super) fn show_once(name: &str) {
    let mut terminal = io::stderr().lock();
    if terminal.is_terminal() {
        SHOWN.call_once(|| {
            let _ = writeln!(terminal, "SIWC omitted output field: {name}");
        });
    }
}

#[cfg(test)]
mod tests;
