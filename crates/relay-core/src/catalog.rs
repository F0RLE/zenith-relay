mod claude_desktop;
mod codex;
mod context;
mod order;
mod registry;
mod rules;
mod speed;

pub use crate::model_metadata::ModelMetadataCatalog;
pub use claude_desktop::{claude_desktop_model_alias, claude_desktop_reasoning_model_alias};
pub(crate) use claude_desktop::{
    claude_desktop_model_alias_matches, is_claude_desktop_model_alias,
    is_claude_desktop_reasoning_model_alias,
};
pub(crate) use codex::set_codex_service_tiers;
pub use codex::{
    apply_codex_ultra_from_official_model, codex_catalog_entry_is_compatible, codex_model_alias,
    codex_model_display_name, codex_model_is_picker_eligible, codex_model_is_picker_eligible_for,
    decode_codex_model_alias, normalize_codex_catalog_priorities,
    normalize_native_codex_catalog_entry, normalize_upstream_codex_catalog_entry,
    publish_routed_codex_context, routed_codex_catalog_entry, source_row_declares_reasoning,
    CODEX_CATALOG_PRIORITY_BASE, CODEX_RELAY_CATALOG_HASH,
};
pub use context::{
    deserialize_model_reasoning_allowed_levels, normalize_model_reasoning_allowed_levels,
    source_model_declares_image_input,
};
pub(crate) use order::{canonical_reasoning_level, merge_model_display_order_with_groups};
pub use order::{
    canonicalize_reasoning_levels, is_degraded_route_model, is_valid_model_id,
    is_valid_model_token, merge_model_display_order, model_id_key, normalize_bounded_model_ids,
    normalize_model_ids, reasoning_level_rank, reasoning_policy_key, reasoning_policy_levels,
    ModelIdListError, MAX_MODEL_LIST_LEN,
};
pub use registry::ModelRegistry;
pub use rules::ModelRules;
pub use speed::model_service_tiers;
pub(crate) use speed::{reference_service_tiers, transport_supports_service_tier};
pub mod context_policy;
pub use context_policy::{normalize_model_context_policies, ModelContextPolicy};
