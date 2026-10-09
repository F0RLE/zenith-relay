use crate::usage::{
    CacheContextSection, CacheHistoryComparison, CacheHistoryDiagnostics, CacheInputKind,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{self, Write};

const MAX_FINGERPRINT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_INPUT_ITEMS: usize = 1_024;

pub(super) type Fingerprint = [u8; 32];

// Deliberately neither Debug nor Serialize: fingerprints never leave RAM.
pub(crate) struct RequestFingerprint {
    pub(super) sections: Vec<(CacheContextSection, Fingerprint)>,
    input: Vec<InputFingerprint>,
    input_bytes: u64,
    continuation: bool,
}

struct InputFingerprint {
    digest: Fingerprint,
    kind: CacheInputKind,
}

impl RequestFingerprint {
    pub(super) fn capture(request: &Value, salt: &[u8; 32]) -> Option<Self> {
        let mut remaining = MAX_FINGERPRINT_BYTES;
        let mut sections = Vec::new();
        for (section, field) in [
            (CacheContextSection::Model, "model"),
            (CacheContextSection::Tools, "tools"),
            (CacheContextSection::Instructions, "instructions"),
            (CacheContextSection::Reasoning, "reasoning"),
            (CacheContextSection::ToolChoice, "tool_choice"),
            (
                CacheContextSection::ParallelToolCalls,
                "parallel_tool_calls",
            ),
            (CacheContextSection::CacheKey, "prompt_cache_key"),
            (CacheContextSection::ServiceTier, "service_tier"),
            (CacheContextSection::ContextManagement, "context_management"),
            (CacheContextSection::Truncation, "truncation"),
        ] {
            sections.push((
                section,
                fingerprint(&request.get(field), salt, &mut remaining)?.0,
            ));
        }
        for (section, value) in [
            (
                CacheContextSection::OutputFormat,
                request.pointer("/text/format"),
            ),
            (
                CacheContextSection::Verbosity,
                request.pointer("/text/verbosity"),
            ),
        ] {
            sections.push((section, fingerprint(&value, salt, &mut remaining)?.0));
        }
        // comparison_response_id requests diagnostics; it does not alter caching.
        let cache_policy = (
            request.get("prompt_cache_retention"),
            request.pointer("/prompt_cache_options/mode"),
            request.pointer("/prompt_cache_options/ttl"),
        );
        sections.push((
            CacheContextSection::CachePolicy,
            fingerprint(&cache_policy, salt, &mut remaining)?.0,
        ));
        let mut input = Vec::new();
        let mut input_bytes = 0;
        let items: &[Value] = match request.get("input") {
            Some(Value::Array(items)) => items,
            Some(item) => std::slice::from_ref(item),
            None => &[],
        };
        if items.len() > MAX_INPUT_ITEMS {
            return None;
        }
        for item in items {
            let (digest, bytes) = fingerprint(item, salt, &mut remaining)?;
            input.push(InputFingerprint {
                digest,
                kind: input_kind(item),
            });
            input_bytes += bytes;
        }
        if request.get("input").is_some_and(Value::is_array) {
            input_bytes += 2 + items.len().saturating_sub(1) as u64;
        }
        Some(Self {
            sections,
            input,
            input_bytes,
            continuation: request
                .get("previous_response_id")
                .and_then(Value::as_str)
                .is_some(),
        })
    }

    pub(super) fn changes_from(&self, previous: &Self) -> Vec<CacheContextSection> {
        self.sections
            .iter()
            .zip(&previous.sections)
            .filter_map(|(current, old)| (current.1 != old.1).then_some(current.0))
            .collect()
    }

    pub(super) fn history(&self, previous: Option<&Self>) -> CacheHistoryDiagnostics {
        self.compare_history(previous, false)
    }

    pub(super) fn relay_history(&self, client: &Self) -> CacheHistoryDiagnostics {
        self.compare_history(Some(client), true)
    }

    fn compare_history(
        &self,
        previous: Option<&Self>,
        same_request: bool,
    ) -> CacheHistoryDiagnostics {
        let mut diagnostics = CacheHistoryDiagnostics {
            comparison: if self.continuation {
                CacheHistoryComparison::Continuation
            } else {
                CacheHistoryComparison::NotCompared
            },
            input_items: Some(self.input.len() as u32),
            input_bytes: Some(self.input_bytes),
            ..CacheHistoryDiagnostics::default()
        };
        let Some(previous) =
            previous.filter(|old| same_request || (!old.continuation && !self.continuation))
        else {
            return diagnostics;
        };
        let shared = self
            .input
            .iter()
            .zip(&previous.input)
            .take_while(|(current, old)| current.digest == old.digest)
            .count();
        diagnostics.shared_prefix_items = Some(shared as u32);
        diagnostics.comparison = if shared < self.input.len().min(previous.input.len()) {
            diagnostics.first_changed_item_kind = self.input.get(shared).map(|item| item.kind);
            CacheHistoryComparison::Rewritten
        } else if self.input.len() < previous.input.len() {
            CacheHistoryComparison::Truncated
        } else if self.input.len() > previous.input.len() {
            CacheHistoryComparison::Appended
        } else {
            CacheHistoryComparison::Unchanged
        };
        diagnostics
    }
}

pub(super) fn identity(salt: &[u8; 32], parts: &[&[u8]]) -> Fingerprint {
    let mut digest = Sha256::new();
    digest.update(salt);
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    digest.finalize().into()
}

fn fingerprint(
    value: &impl Serialize,
    salt: &[u8; 32],
    remaining: &mut u64,
) -> Option<(Fingerprint, u64)> {
    let mut writer = FingerprintWriter {
        digest: Sha256::new(),
        bytes: 0,
        remaining,
    };
    writer.digest.update(salt);
    serde_json::to_writer(&mut writer, value).ok()?;
    Some((writer.digest.finalize().into(), writer.bytes))
}

struct FingerprintWriter<'a> {
    digest: Sha256,
    bytes: u64,
    remaining: &'a mut u64,
}

impl Write for FingerprintWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > *self.remaining {
            return Err(io::Error::other("fingerprint budget exceeded"));
        }
        self.digest.update(bytes);
        self.bytes += bytes.len() as u64;
        *self.remaining -= bytes.len() as u64;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn input_kind(item: &Value) -> CacheInputKind {
    match item.get("type").and_then(Value::as_str) {
        Some("function_call_output" | "custom_tool_call_output") => CacheInputKind::ToolResult,
        Some("function_call" | "custom_tool_call") => CacheInputKind::ToolCall,
        Some("reasoning") => CacheInputKind::Reasoning,
        Some("compaction" | "compaction_trigger") => CacheInputKind::Compaction,
        Some("additional_tools") => CacheInputKind::AdditionalTools,
        Some("configuration_update") => CacheInputKind::ConfigurationUpdate,
        _ => match item.get("role").and_then(Value::as_str) {
            Some("developer" | "system") => CacheInputKind::Developer,
            Some("user") => CacheInputKind::User,
            Some("assistant") => CacheInputKind::Assistant,
            Some("tool") => CacheInputKind::ToolResult,
            _ => CacheInputKind::Other,
        },
    }
}
