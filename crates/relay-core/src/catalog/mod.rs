mod codex;
mod context;
mod order;
mod registry;
mod rules;

pub use codex::{
    codex_catalog_entry_is_compatible, codex_model_alias, codex_model_display_name,
    codex_model_is_picker_eligible, decode_codex_model_alias, normalize_codex_catalog_priorities,
    normalize_native_codex_catalog_entry, normalize_upstream_codex_catalog_entry,
    routed_codex_catalog_entry, CODEX_CATALOG_PRIORITY_BASE, CODEX_RELAY_CATALOG_HASH,
};
pub use context::{
    deserialize_model_reasoning_allowed_levels, normalize_model_reasoning_allowed_levels,
    source_model_declares_image_input,
};
pub(crate) use context::{
    source_context_windows, source_image_input_capabilities, source_reasoning_capabilities,
    source_reasoning_probe_progress, union_source_reasoning_capabilities,
    SourceReasoningCapabilities, SourceReasoningProbeProgress,
};
pub use order::{
    anthropic_max_implies_ultra, canonicalize_model_ids, canonicalize_reasoning_levels,
    is_valid_model_id, is_valid_model_token, known_model_reasoning_levels, normalize_model_ids,
    reasoning_policy_key, reasoning_policy_levels,
};
pub use registry::ModelRegistry;
pub use rules::ModelRules;
