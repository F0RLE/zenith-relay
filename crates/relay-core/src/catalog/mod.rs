mod codex;
mod context;
mod registry;
mod rules;

pub use codex::{
    codex_catalog_entry_is_compatible, codex_model_alias, codex_model_display_name,
    codex_model_is_picker_eligible, decode_codex_model_alias,
    normalize_upstream_codex_catalog_entry, routed_codex_catalog_entry, CODEX_RELAY_CATALOG_HASH,
};
pub(crate) use context::source_context_windows;
pub use registry::ModelRegistry;
pub use rules::ModelRules;
