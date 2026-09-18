use super::ModelCapabilities;
use serde_json::{json, Value};

impl ModelCapabilities {
    pub fn unknown_model() -> Self {
        Self {
            input_modalities: vec!["text".into(), "image".into()],
            output_modalities: vec!["text".into()],
            attachment: Some(true),
            reasoning: Some(false),
            tool_call: Some(false),
            structured_output: Some(false),
            ..Self::default()
        }
    }

    /// Replace model capability fields, including stale fields from provider
    /// templates. Routing IDs and native transport settings are left intact.
    pub fn apply_to_codex(&self, entry: &mut Value) {
        let Some(object) = entry.as_object_mut() else {
            return;
        };
        for field in [
            "default_reasoning_level",
            "context_window",
            "max_context_window",
            "auto_compact_token_limit",
            "additional_speed_tiers",
            "service_tiers",
            "default_service_tier",
            "default_verbosity",
        ] {
            object.remove(field);
        }
        // The Codex catalog schema accepts text/image/audio, not models.dev's
        // video/pdf inputs. Keep the full set in management/OpenCode, but do
        // not make an otherwise usable text model disappear from this client.
        let input_modalities = self
            .input_modalities
            .iter()
            .filter(|modality| matches!(modality.as_str(), "text" | "image" | "audio"))
            .collect::<Vec<_>>();
        object.insert("input_modalities".into(), json!(input_modalities));
        object.insert("output_modalities".into(), json!(self.output_modalities));
        object.insert(
            "supports_parallel_tool_calls".into(),
            json!(self.tool_call == Some(true)),
        );
        object.insert("supports_search_tool".into(), json!(false));
        object.insert("supports_image_detail_original".into(), json!(false));
        object.insert("supports_reasoning_summaries".into(), json!(false));
        object.insert("supports_reasoning_summary_parameter".into(), json!(false));
        object.insert("default_reasoning_summary".into(), json!("none"));
        object.insert("support_verbosity".into(), json!(false));
        object.insert("default_verbosity".into(), Value::Null);
        object.insert("experimental_supported_tools".into(), json!([]));
        // A boolean reasoning capability is not an enum. Do not invent
        // effort levels when registries only say that reasoning exists.
        let levels = if self.reasoning == Some(false) {
            Vec::new()
        } else {
            crate::canonicalize_reasoning_levels(&self.reasoning_effort_levels)
        };
        object.insert(
            "supported_reasoning_levels".into(),
            json!(levels
                .iter()
                .map(|level| json!({"effort":level, "description":level}))
                .collect::<Vec<_>>()),
        );
        if let Some(default) = self
            .default_reasoning_effort
            .as_ref()
            .filter(|default| levels.iter().any(|level| level == *default))
        {
            object.insert("default_reasoning_level".into(), json!(default));
        }
        // Context/compaction is a Codex client policy.  External model
        // catalogs are evidence for display and routing only; advertising a
        // model's theoretical maximum here makes Codex expand a conversation
        // to (for example) one million tokens instead of using its own limit.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_metadata::ModelMetadataCatalog;

    #[test]
    fn client_projection_does_not_drop_a_model_with_video_or_pdf_inputs() {
        let capabilities = ModelCapabilities {
            input_modalities: vec!["text".into(), "image".into(), "video".into(), "pdf".into()],
            output_modalities: vec!["text".into()],
            ..ModelCapabilities::default()
        };
        let mut entry = crate::routed_codex_catalog_entry(None, "multimodal", 1000, None);
        capabilities.apply_to_codex(&mut entry);
        assert_eq!(entry["input_modalities"], json!(["text", "image"]));
        assert!(crate::codex_catalog_entry_is_compatible(&entry));
        assert_eq!(capabilities.input_modalities.len(), 4);
    }

    #[test]
    fn exact_model_catalog_overrides_conflicting_provider_metadata() {
        let catalog = ModelMetadataCatalog::from_models_dev_json(
            r#"{
            "test/astra":{"modalities":{"input":["text"],"output":["text"]},
                "reasoning":true,"reasoning_effort_levels":["low","high"],
                "limit":{"context":123456},"tool_call":true}
        }"#,
        )
        .unwrap();
        let mut entry = crate::routed_codex_catalog_entry(None, "astra", 1000, None);
        entry["input_modalities"] = json!(["text", "image"]);
        entry["context_window"] = json!(999999);
        catalog.apply_codex_capabilities("astra", &mut entry);
        assert_eq!(entry["input_modalities"], json!(["text"]));
        assert!(entry.get("context_window").is_none());
        assert_eq!(
            entry["supported_reasoning_levels"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(crate::codex_catalog_entry_is_compatible(&entry));
        catalog.apply_codex_capabilities("astra-other", &mut entry);
        assert_eq!(entry["input_modalities"], json!(["text", "image"]));
        assert_eq!(entry["supported_reasoning_levels"], json!([]));
        assert_eq!(entry["supports_parallel_tool_calls"], false);
        assert!(entry.get("context_window").is_none());
        assert!(crate::codex_catalog_entry_is_compatible(&entry));
    }
}
