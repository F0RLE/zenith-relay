mod codex;
mod context;
mod order;
mod registry;
mod rules;

pub use codex::{
    codex_catalog_entry_is_compatible, codex_model_alias, codex_model_display_name,
    codex_model_is_picker_eligible, decode_codex_model_alias, normalize_codex_catalog_priorities,
    normalize_upstream_codex_catalog_entry, routed_codex_catalog_entry,
    CODEX_CATALOG_PRIORITY_BASE, CODEX_RELAY_CATALOG_HASH,
};
pub(crate) use context::source_context_windows;
pub use order::canonicalize_model_ids;
pub use registry::ModelRegistry;
pub use rules::ModelRules;
