use crate::usage::{
    CacheContextSection, CacheHistoryComparison, CacheHistoryDiagnostics, CacheInputKind,
    LeadingContextAlignment,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{self, BufWriter, Write};

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
        let mut fingerprints = JsonFingerprinter::new(salt);
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
            let value = request.get(field).or_else(|| {
                // BPS exposes the effective effort as a top-level string.
                // Observe it without changing the provider request shape.
                (field == "reasoning")
                    .then(|| request.get("reasoning_effort"))
                    .flatten()
            });
            sections.push((section, fingerprints.fingerprint(&value)?.0));
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
            sections.push((section, fingerprints.fingerprint(&value)?.0));
        }
        // comparison_response_id requests diagnostics; it does not alter caching.
        let cache_policy = (
            request.get("prompt_cache_retention"),
            request.pointer("/prompt_cache_options/mode"),
            request.pointer("/prompt_cache_options/ttl"),
        );
        sections.push((
            CacheContextSection::CachePolicy,
            fingerprints.fingerprint(&cache_policy)?.0,
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
            let (digest, bytes) = fingerprints.fingerprint(item)?;
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
        let mut diagnostics = self.compare_history(Some(client), true);
        // Never align a continuation delta with materialized full history.
        if self.continuation || client.continuation {
            return diagnostics;
        }
        let leading = |items: &[InputFingerprint]| {
            items
                .iter()
                .take_while(|item| item.kind == CacheInputKind::Developer)
                .count()
        };
        let client_prefix = leading(&client.input);
        let upstream_prefix = leading(&self.input);
        if diagnostics.shared_prefix_items.unwrap_or(0) >= client_prefix.max(upstream_prefix) as u32
            || (client_prefix == client.input.len() && upstream_prefix == self.input.len())
        {
            return diagnostics;
        }
        let (comparison, shared, _) = compare_items(
            &self.input[upstream_prefix..],
            &client.input[client_prefix..],
        );
        diagnostics.leading_context_alignment = Some(LeadingContextAlignment {
            skipped_client_items: client_prefix as u32,
            skipped_upstream_items: upstream_prefix as u32,
            shared_prefix_items: shared as u32,
            comparison,
        });
        diagnostics
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
        let (comparison, shared, changed) = compare_items(&self.input, &previous.input);
        diagnostics.shared_prefix_items = Some(shared as u32);
        diagnostics.comparison = comparison;
        diagnostics.first_changed_item_kind = changed;
        diagnostics
    }
}

fn compare_items(
    current: &[InputFingerprint],
    previous: &[InputFingerprint],
) -> (CacheHistoryComparison, usize, Option<CacheInputKind>) {
    let shared = current
        .iter()
        .zip(previous)
        .take_while(|(current, old)| current.digest == old.digest)
        .count();
    let mut changed = None;
    let comparison = if shared < current.len().min(previous.len()) {
        changed = current.get(shared).map(|item| item.kind);
        CacheHistoryComparison::Rewritten
    } else if current.len() < previous.len() {
        CacheHistoryComparison::Truncated
    } else if current.len() > previous.len() {
        CacheHistoryComparison::Appended
    } else {
        CacheHistoryComparison::Unchanged
    };
    (comparison, shared, changed)
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

struct JsonFingerprinter<'a> {
    salt: &'a [u8; 32],
    writer: BufWriter<FingerprintWriter>,
}

impl<'a> JsonFingerprinter<'a> {
    fn new(salt: &'a [u8; 32]) -> Self {
        Self {
            salt,
            writer: BufWriter::with_capacity(
                8 * 1024,
                FingerprintWriter {
                    digest: Sha256::new(),
                    bytes: 0,
                    remaining: MAX_FINGERPRINT_BYTES,
                },
            ),
        }
    }

    fn fingerprint(&mut self, value: &impl Serialize) -> Option<(Fingerprint, u64)> {
        // Reuse one small buffer across sections/items. Flush before finalizing
        // each digest so boundaries, byte counts and the shared budget stay exact.
        self.writer.get_mut().digest.update(self.salt);
        serde_json::to_writer(&mut self.writer, value).ok()?;
        self.writer.flush().ok()?;
        let inner = self.writer.get_mut();
        Some((
            std::mem::take(&mut inner.digest).finalize().into(),
            std::mem::take(&mut inner.bytes),
        ))
    }
}

struct FingerprintWriter {
    digest: Sha256,
    bytes: u64,
    remaining: u64,
}

impl Write for FingerprintWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(io::Error::other("fingerprint budget exceeded"));
        }
        self.digest.update(bytes);
        self.bytes += bytes.len() as u64;
        self.remaining -= bytes.len() as u64;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bps_cache_fingerprint_detects_effective_reasoning_changes() {
        let salt = [7; 32];
        let before =
            RequestFingerprint::capture(&json!({"reasoning_effort":"low","input":[]}), &salt)
                .unwrap();
        let after =
            RequestFingerprint::capture(&json!({"reasoning_effort":"high","input":[]}), &salt)
                .unwrap();
        assert_eq!(
            after.changes_from(&before),
            vec![CacheContextSection::Reasoning]
        );
    }

    #[test]
    fn buffered_fingerprints_match_original_json_bytes_and_boundaries() {
        let salt = [7; 32];
        let mut fingerprints = JsonFingerprinter::new(&salt);
        let mut bytes = 0;
        for value in [
            Value::Null,
            json!({"text":"Synthetic \"quotes\", \\paths,\n\tкириллица\u{0}"}),
            serde_json::from_str::<Value>(r#"{"large":90071992547409931234567,"small":1e-100}"#)
                .unwrap(),
            json!({"text":"Synthetic \"escaped\" text. ".repeat(4096)}),
            json!([true, false, null, {"ordered":1,"keys":2}]),
        ] {
            let serialized = serde_json::to_vec(&value).unwrap();
            let mut expected = Sha256::new();
            expected.update(salt);
            expected.update(&serialized);
            let expected: Fingerprint = expected.finalize().into();
            assert_eq!(
                fingerprints.fingerprint(&value),
                Some((expected, serialized.len() as u64))
            );
            bytes += serialized.len() as u64;
            assert_eq!(
                fingerprints.writer.get_ref().remaining,
                MAX_FINGERPRINT_BYTES - bytes
            );
        }
    }

    #[test]
    fn buffering_does_not_bypass_aggregate_byte_limits_or_serialization_errors() {
        let salt = [7; 32];
        let mut fingerprints = JsonFingerprinter::new(&salt);
        fingerprints.writer.get_mut().remaining = 8;
        assert!(fingerprints.fingerprint(&Value::Null).is_some());
        assert!(fingerprints.fingerprint(&Value::Null).is_some());
        assert!(fingerprints.fingerprint(&Value::Null).is_none());

        let mut fingerprints = JsonFingerprinter::new(&salt);
        fingerprints.writer.get_mut().remaining = 1;
        assert!(fingerprints.fingerprint(&json!("x")).is_none());

        struct Invalid;
        impl Serialize for Invalid {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("synthetic serialization failure"))
            }
        }
        assert!(
            JsonFingerprinter::new(&salt)
                .fingerprint(&Invalid)
                .is_none()
        );
        assert!(
            RequestFingerprint::capture(
                &json!({"input":vec![Value::Null; MAX_INPUT_ITEMS + 1]}),
                &salt
            )
            .is_none()
        );
    }

    mod measurements;
}
