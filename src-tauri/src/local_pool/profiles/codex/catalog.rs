use super::MODELS_CACHE_FILE;
use crate::local_pool::error::{LocalPoolError, Result};
#[cfg(test)]
use serde_json::json;
use serde_json::Value;
use std::{collections::HashSet, fs, path::Path};
use zenith_relay_core::model_metadata::ModelMetadataCatalog;
use zenith_relay_core::{
    codex_catalog_entry_is_compatible, codex_model_display_name, codex_model_is_picker_eligible,
    decode_codex_model_alias, normalize_upstream_codex_catalog_entry, routed_codex_catalog_entry,
    CODEX_RELAY_CATALOG_HASH,
};

const MAX_MODEL_CATALOG_BYTES: usize = 512 * 1024;
const DIRECT_SOURCE_FALLBACK_PRIORITY: u64 = 1_000;

mod installed;

pub(super) use installed::bundled_codex_ultra_models;
#[cfg(test)]
use installed::{
    codex_cli_file_name, newest_installed_codex_executable, official_codex_ultra_rows,
};
#[cfg(test)]
use std::time::SystemTime;

mod direct;

#[cfg(test)]
pub(in crate::local_pool::profiles::codex) use direct::direct_source_model_catalog_with_manifest;
use direct::{cached_native_catalog_models, catalog_entry_is_picker_eligible, model_slug};
pub(in crate::local_pool::profiles::codex) use direct::{
    direct_source_model_catalog_with_capabilities, is_native_catalog_entry,
};

mod managed;

#[cfg(test)]
pub(super) use managed::build_managed_model_catalog_with_bundled;
pub(super) use managed::{build_managed_model_catalog, read_catalog_values};
use managed::{
    collect_native_catalog_template, configured_model_catalog_path, normalize_model_catalog_values,
};

#[cfg(test)]
mod ultra_tests {
    use super::*;

    #[test]
    fn bundled_codex_metadata_uses_exact_models_and_only_client_owned_fields() {
        let catalog = json!({"models": [
            {"slug": "gpt-future", "supported_reasoning_levels": [
                {"effort": "max"}, {"effort": "ultra"}
            ], "multi_agent_version": "v2", "multi_agent_reasoning_effort": "xhigh",
               "base_instructions": "not a Relay instruction"},
            {"slug": "gpt-short", "context_window": 272000, "max_context_window": 872000,
               "auto_compact_token_limit": 244800, "effective_context_window_percent": 95,
               "supported_reasoning_levels": [{"effort": "max"}]},
            {"slug": "gpt-other", "supported_reasoning_levels": [{"effort": "max"}]}
        ]});
        let official = official_codex_ultra_rows(&catalog);
        assert!(official.contains_key("gpt-future"));
        assert!(!official.contains_key("gpt-other"));
        assert_eq!(official["gpt-short"]["context_window"], 272_000);
        assert_eq!(
            official["gpt-short"]["supported_reasoning_levels"],
            json!([])
        );
        assert!(official["gpt-short"].get("max_context_window").is_none());
        assert!(official["gpt-short"]
            .get("auto_compact_token_limit")
            .is_none());
        assert!(official["gpt-short"]
            .get("effective_context_window_percent")
            .is_none());
        let mut relay = routed_codex_catalog_entry(None, "gpt-future", 1_000, None);
        relay["slug"] = json!("gpt-future");
        relay["supported_reasoning_levels"] = json!([{"effort": "xhigh"}, {"effort": "max"}]);
        let mut qualified = routed_codex_catalog_entry(None, "vendor/gpt-future", 1_001, None);
        qualified["supported_reasoning_levels"] = json!([{"effort": "xhigh"}, {"effort": "max"}]);
        let relay_catalog = json!({"models": [relay, qualified]});
        let home = std::env::temp_dir().join(format!("relay-codex-ultra-{}", std::process::id()));
        let managed = build_managed_model_catalog_with_bundled(
            &home,
            None,
            None,
            &relay_catalog.to_string(),
            &official,
        )
        .unwrap();
        let managed: Value = serde_json::from_str(&managed).unwrap();
        assert_eq!(
            managed["models"][0]["supported_reasoning_levels"][2]["effort"],
            "ultra"
        );
        assert_eq!(managed["models"][0]["multi_agent_version"], "v2");
        assert_eq!(
            managed["models"][0]["multi_agent_reasoning_effort"],
            "xhigh"
        );
        assert_ne!(
            managed["models"][0]["base_instructions"],
            "not a Relay instruction"
        );
        assert_eq!(
            managed["models"][1]["supported_reasoning_levels"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn managed_catalog_adds_gpt_6_sol_ultra_without_a_child_effort() {
        let official = official_codex_ultra_rows(&json!({"models": [{
            "slug": "gpt-6-sol",
            "supported_reasoning_levels": [{"effort": "ultra"}],
            "multi_agent_version": "v2",
            "base_instructions": "not a Relay instruction"
        }]}));
        let slug = zenith_relay_core::codex_model_alias("gpt-6-sol");
        let mut relay = routed_codex_catalog_entry(None, "gpt-6-sol", 1_000, None);
        relay["slug"] = json!(slug);
        relay["supported_reasoning_levels"] = json!([
            {"effort": "high", "description": "high"},
            {"effort": "max", "description": "max"}
        ]);
        let mut grok = routed_codex_catalog_entry(None, "grok-4.7", 1_001, None);
        grok["supported_reasoning_levels"] = json!([
            {"effort": "high", "description": "high"},
            {"effort": "max", "description": "max"}
        ]);
        let home =
            std::env::temp_dir().join(format!("relay-codex-ultra-sol-{}", std::process::id()));
        let managed = build_managed_model_catalog_with_bundled(
            &home,
            None,
            None,
            &json!({"models": [relay, grok]}).to_string(),
            &official,
        )
        .unwrap();
        let managed: Value = serde_json::from_str(&managed).unwrap();
        let sol = &managed["models"][0];
        assert_eq!(sol["supported_reasoning_levels"][2]["effort"], "ultra");
        assert_eq!(
            sol["supported_reasoning_levels"][2]["description"],
            "Ultra (agents)"
        );
        assert_eq!(sol["multi_agent_version"], "v2");
        assert!(sol.get("multi_agent_reasoning_effort").is_none());
        assert_ne!(sol["base_instructions"], "not a Relay instruction");
        assert!(managed["models"][1]["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .iter()
            .all(|level| level["effort"] != "ultra"));
    }

    #[test]
    fn installed_codex_cli_is_the_only_external_ultra_metadata_source() {
        let cli = json!({"models": [{"slug": "gpt-6-sol", "supported_reasoning_levels": [
            {"effort": "max"}
        ]}]});
        assert!(official_codex_ultra_rows(&cli).is_empty());

        let root = std::env::temp_dir().join(format!(
            "relay-codex-bin-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let older = root.join("older");
        let newer = root.join("newer");
        fs::create_dir_all(&older).unwrap();
        fs::create_dir_all(&newer).unwrap();
        let older_cli = older.join(codex_cli_file_name());
        let newer_cli = newer.join(codex_cli_file_name());
        fs::write(&older_cli, b"old").unwrap();
        fs::write(&newer_cli, b"new").unwrap();
        fs::write(root.join(codex_cli_file_name()), b"not a version directory").unwrap();
        fs::write(newer.join("codex.cmd"), b"ignore").unwrap();
        let now = SystemTime::now();
        fs::File::options()
            .write(true)
            .open(&older_cli)
            .unwrap()
            .set_modified(now - std::time::Duration::from_secs(120))
            .unwrap();
        fs::File::options()
            .write(true)
            .open(&newer_cli)
            .unwrap()
            .set_modified(now)
            .unwrap();
        assert_eq!(
            newest_installed_codex_executable(&root).as_deref(),
            Some(newer_cli.as_path())
        );

        fs::remove_dir_all(root).unwrap();
    }
}
