use super::ModelCapabilities;
use serde_json::{json, Value};

impl ModelCapabilities {
    pub fn unknown_model() -> Self {
        Self {
            input_modalities: vec!["text".into(), "image".into()],
            output_modalities: vec!["text".into()],
            attachment: Some(true),
            tool_call: Some(true),
            structured_output: Some(true),
            ..Self::default()
        }
    }

    /// Fill absent reference fields with Relay's baseline. An explicit false
    /// from a trusted reference remains false. Numeric limits, prices and
    /// unknown reasoning enums are never guessed.
    pub fn with_defaults(mut self) -> Self {
        let baseline = Self::unknown_model();
        if self.input_modalities.is_empty() {
            self.input_modalities = if self.attachment == Some(false) {
                vec!["text".into()]
            } else {
                baseline.input_modalities
            };
        }
        if self.output_modalities.is_empty() {
            self.output_modalities = baseline.output_modalities;
        }
        self.tool_call = self.tool_call.or(baseline.tool_call);
        self.structured_output = self.structured_output.or(baseline.structured_output);
        self.attachment = self.attachment.or_else(|| {
            Some(
                self.input_modalities
                    .iter()
                    .any(|modality| modality == "image"),
            )
        });
        self.reasoning_effort_levels =
            crate::canonicalize_reasoning_levels(self.reasoning_effort_levels);
        if self.reasoning == Some(false) {
            self.reasoning_effort_levels.clear();
        } else if !self.reasoning_effort_levels.is_empty() {
            self.reasoning = Some(true);
        }
        if !self
            .default_reasoning_effort
            .as_ref()
            .is_some_and(|level| self.reasoning_effort_levels.contains(level))
        {
            self.default_reasoning_effort = self
                .reasoning_effort_levels
                .iter()
                .find(|level| level.as_str() == "medium")
                .or_else(|| self.reasoning_effort_levels.first())
                .cloned();
        }
        self
    }

    pub fn protocol_features(
        &self,
    ) -> std::collections::BTreeMap<crate::ProtocolFeature, crate::CapabilityStatus> {
        use crate::{CapabilityStatus, ProtocolFeature};
        let capability_status = |capability_flag| match capability_flag {
            Some(true) => CapabilityStatus::Declared,
            Some(false) => CapabilityStatus::Unsupported,
            None => CapabilityStatus::Unknown,
        };
        std::collections::BTreeMap::from([
            (ProtocolFeature::Text, CapabilityStatus::Declared),
            (ProtocolFeature::Streaming, CapabilityStatus::Declared),
            (ProtocolFeature::Images, capability_status(self.attachment)),
            (
                ProtocolFeature::FunctionTools,
                capability_status(self.tool_call),
            ),
            (
                ProtocolFeature::ToolChoice,
                capability_status(self.tool_call),
            ),
            (
                ProtocolFeature::StructuredOutput,
                capability_status(self.structured_output),
            ),
            (
                ProtocolFeature::Reasoning,
                capability_status(self.reasoning),
            ),
        ])
    }

    /// Resolve the capabilities that are safe to use for one selected route.
    ///
    /// Reference metadata describes the model family, while the route record
    /// describes the exact upstream protocol exposed by a source. Explicit
    /// unsupported evidence from either layer is a hard deny. Route evidence
    /// may fill an unknown reference field, but it cannot turn a reference
    /// exclusion into support.
    pub(crate) fn protocol_features_for_route(
        &self,
        route: Option<&crate::ModelEndpointCapability>,
    ) -> std::collections::BTreeMap<crate::ProtocolFeature, crate::CapabilityStatus> {
        use crate::{CapabilityStatus, ProtocolFeature};

        let reference_features = self.clone().with_defaults().protocol_features();
        let Some(route) = route else {
            return reference_features;
        };
        if route.status == CapabilityStatus::Unsupported {
            return ProtocolFeature::ALL
                .into_iter()
                .map(|feature| (feature, CapabilityStatus::Unsupported))
                .collect();
        }

        ProtocolFeature::ALL
            .into_iter()
            .map(|feature| {
                let reference_feature_status = reference_features
                    .get(&feature)
                    .copied()
                    .unwrap_or(CapabilityStatus::Unknown);
                let route_feature_status = route
                    .features
                    .get(&feature)
                    .copied()
                    .unwrap_or(CapabilityStatus::Unknown);
                (
                    feature,
                    merge_route_capability_status(reference_feature_status, route_feature_status),
                )
            })
            .collect()
    }

    /// Return reasoning effort levels that are valid for the selected route.
    /// An exact route list narrows the reference list; when the reference does
    /// not publish levels, the route list is still useful evidence.
    pub(crate) fn reasoning_effort_levels_for_route(
        &self,
        route: Option<&crate::ModelEndpointCapability>,
    ) -> Vec<String> {
        use crate::{CapabilityStatus, ProtocolFeature};

        let reference_capabilities = self.clone().with_defaults();
        let Some(route) = route else {
            return reference_capabilities.reasoning_effort_levels;
        };
        // A route catalog can describe the protocol it exposes, but it cannot
        // revive reasoning that the trusted model reference explicitly marks
        // as unsupported. Keep this invariant in the level projection too;
        // callers such as management snapshots do not always inspect the
        // feature map first.
        if reference_capabilities.reasoning == Some(false) {
            return Vec::new();
        }
        if route.status == CapabilityStatus::Unsupported
            || route.features.get(&ProtocolFeature::Reasoning)
                == Some(&CapabilityStatus::Unsupported)
        {
            return Vec::new();
        }

        let route_levels = crate::canonicalize_reasoning_levels(&route.reasoning_efforts);
        if route_levels.is_empty() {
            return reference_capabilities.reasoning_effort_levels;
        }
        if reference_capabilities.reasoning_effort_levels.is_empty() {
            return route_levels;
        }
        route_levels
            .into_iter()
            .filter(|level| {
                reference_capabilities
                    .reasoning_effort_levels
                    .iter()
                    .any(|reference_level| reference_level.eq_ignore_ascii_case(level))
            })
            .collect()
    }

    /// Replace model capability fields, including stale fields from provider
    /// templates. Routing IDs and native transport settings are left intact.
    pub fn apply_to_codex(&self, catalog_entry: &mut Value) {
        let Some(catalog_object) = catalog_entry.as_object_mut() else {
            return;
        };
        // The projected row is served by Relay's API. The account's flag for
        // its vendor API does not describe this pool endpoint or its key scope.
        catalog_object.insert("supported_in_api".into(), json!(true));
        for field in [
            "default_reasoning_level",
            "context_window",
            "max_context_window",
            "auto_compact_token_limit",
            "effective_context_window_percent",
            "additional_speed_tiers",
            "service_tiers",
            "default_service_tier",
            "default_verbosity",
        ] {
            catalog_object.remove(field);
        }
        // The Codex catalog schema accepts text/image/audio, not models.dev's
        // video/pdf inputs. Keep the full set in management/OpenCode, but do
        // not make an otherwise usable text model disappear from this client.
        let input_modalities = self
            .input_modalities
            .iter()
            .filter(|modality| matches!(modality.as_str(), "text" | "image" | "audio"))
            .collect::<Vec<_>>();
        catalog_object.insert("input_modalities".into(), json!(input_modalities));
        catalog_object.insert("output_modalities".into(), json!(self.output_modalities));
        catalog_object.insert(
            "supports_parallel_tool_calls".into(),
            json!(self.tool_call == Some(true)),
        );
        catalog_object.insert("supports_search_tool".into(), json!(false));
        catalog_object.insert("supports_image_detail_original".into(), json!(false));
        catalog_object.insert("supports_reasoning_summaries".into(), json!(false));
        catalog_object.insert("supports_reasoning_summary_parameter".into(), json!(false));
        catalog_object.insert("default_reasoning_summary".into(), json!("none"));
        catalog_object.insert("support_verbosity".into(), json!(false));
        catalog_object.insert("default_verbosity".into(), Value::Null);
        catalog_object.insert("experimental_supported_tools".into(), json!([]));
        // A boolean reasoning capability is not an enum. Do not invent
        // effort levels when registries only say that reasoning exists.
        let levels = if self.reasoning == Some(false) {
            Vec::new()
        } else {
            crate::canonicalize_reasoning_levels(&self.reasoning_effort_levels)
        };
        catalog_object.insert(
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
            catalog_object.insert("default_reasoning_level".into(), json!(default));
        }
        // Native Codex cards keep Codex's own window. Advertising a theoretical
        // catalog maximum here makes Codex expand a conversation instead.
        // Non-native cards publish the reference limit after this call.
    }
}

fn merge_route_capability_status(
    reference: crate::CapabilityStatus,
    route: crate::CapabilityStatus,
) -> crate::CapabilityStatus {
    use crate::CapabilityStatus;
    if reference == CapabilityStatus::Unsupported || route == CapabilityStatus::Unsupported {
        return CapabilityStatus::Unsupported;
    }
    if reference == CapabilityStatus::Confirmed || route == CapabilityStatus::Confirmed {
        return CapabilityStatus::Confirmed;
    }
    if reference == CapabilityStatus::Declared || route == CapabilityStatus::Declared {
        return CapabilityStatus::Declared;
    }
    CapabilityStatus::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_metadata::ModelMetadataCatalog;

    #[test]
    fn defaults_fill_missing_fields_without_undoing_reference_exclusions() {
        let catalog = ModelMetadataCatalog::from_models_dev_json(
            r#"{
            "vendor/text-only":{"attachment":false,"tool_call":false,"reasoning":false}
        }"#,
        )
        .unwrap();
        let known = catalog.capabilities_for("text-only");
        assert_eq!(known.input_modalities, ["text"]);
        assert_eq!(known.tool_call, Some(false));
        assert_eq!(known.structured_output, Some(true));
        assert!(known.reasoning_effort_levels.is_empty());
        assert_eq!(known.context_limit, None);
        let unknown = catalog.capabilities_for("new-model");
        assert_eq!(unknown.input_modalities, ["text", "image"]);
        assert_eq!(unknown.tool_call, Some(true));
        assert_eq!(unknown.context_limit, None);
    }

    #[test]
    fn client_projection_does_not_drop_a_model_with_video_or_pdf_inputs() {
        let capabilities = ModelCapabilities {
            input_modalities: vec!["text".into(), "image".into(), "video".into(), "pdf".into()],
            output_modalities: vec!["text".into()],
            ..ModelCapabilities::default()
        };
        let mut catalog_entry = crate::routed_codex_catalog_entry(None, "multimodal", 1000, None);
        capabilities.apply_to_codex(&mut catalog_entry);
        assert_eq!(catalog_entry["input_modalities"], json!(["text", "image"]));
        assert!(crate::codex_catalog_entry_is_compatible(&catalog_entry));
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
        let mut catalog_entry = crate::routed_codex_catalog_entry(None, "astra", 1000, None);
        catalog_entry["input_modalities"] = json!(["text", "image"]);
        catalog_entry["context_window"] = json!(999999);
        catalog.apply_codex_capabilities("astra", &mut catalog_entry);
        assert_eq!(catalog_entry["input_modalities"], json!(["text"]));
        assert!(catalog_entry.get("context_window").is_none());
        assert_eq!(
            catalog_entry["supported_reasoning_levels"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(crate::codex_catalog_entry_is_compatible(&catalog_entry));
        catalog.apply_codex_capabilities("astra-other", &mut catalog_entry);
        assert_eq!(catalog_entry["input_modalities"], json!(["text", "image"]));
        assert_eq!(catalog_entry["supported_reasoning_levels"], json!([]));
        assert_eq!(catalog_entry["supports_parallel_tool_calls"], true);
        assert!(catalog_entry.get("context_window").is_none());
        assert!(crate::codex_catalog_entry_is_compatible(&catalog_entry));
    }

    #[test]
    fn route_evidence_narrows_reference_features_and_reasoning_levels() {
        let reference = ModelCapabilities {
            reasoning: Some(true),
            reasoning_effort_levels: vec!["low".into(), "high".into()],
            tool_call: Some(false),
            ..ModelCapabilities::default()
        };
        let route = crate::ModelEndpointCapability {
            model_id: "claude-sonnet".into(),
            upstream_wire_api: crate::WireApi::Messages,
            status: crate::CapabilityStatus::Declared,
            origin: crate::CapabilityOrigin::Catalog,
            checked_at_ms: 1,
            features: std::collections::BTreeMap::from([
                (
                    crate::ProtocolFeature::FunctionTools,
                    crate::CapabilityStatus::Declared,
                ),
                (
                    crate::ProtocolFeature::Reasoning,
                    crate::CapabilityStatus::Declared,
                ),
            ]),
            reasoning_efforts: vec!["low".into(), "max".into()],
        };
        let features = reference.protocol_features_for_route(Some(&route));
        assert_eq!(
            features.get(&crate::ProtocolFeature::FunctionTools),
            Some(&crate::CapabilityStatus::Unsupported)
        );
        assert_eq!(
            features.get(&crate::ProtocolFeature::Reasoning),
            Some(&crate::CapabilityStatus::Declared)
        );
        assert_eq!(
            reference.reasoning_effort_levels_for_route(Some(&route)),
            ["low"]
        );
    }

    #[test]
    fn explicit_route_exclusion_overrides_optimistic_unknown_baseline() {
        let route = crate::ModelEndpointCapability {
            model_id: "gemini-flash".into(),
            upstream_wire_api: crate::WireApi::Gemini,
            status: crate::CapabilityStatus::Declared,
            origin: crate::CapabilityOrigin::Catalog,
            checked_at_ms: 1,
            features: std::collections::BTreeMap::from([(
                crate::ProtocolFeature::Images,
                crate::CapabilityStatus::Unsupported,
            )]),
            reasoning_efforts: Vec::new(),
        };
        let features = ModelCapabilities::default().protocol_features_for_route(Some(&route));
        assert_eq!(
            features.get(&crate::ProtocolFeature::Images),
            Some(&crate::CapabilityStatus::Unsupported)
        );
    }

    #[test]
    fn explicit_reference_reasoning_exclusion_cannot_be_revived_by_route_levels() {
        let reference = ModelCapabilities {
            reasoning: Some(false),
            reasoning_effort_levels: vec!["low".into(), "high".into()],
            ..ModelCapabilities::default()
        };
        let route = crate::ModelEndpointCapability {
            model_id: "text-model".into(),
            upstream_wire_api: crate::WireApi::Messages,
            status: crate::CapabilityStatus::Declared,
            origin: crate::CapabilityOrigin::Catalog,
            checked_at_ms: 1,
            features: std::collections::BTreeMap::from([(
                crate::ProtocolFeature::Reasoning,
                crate::CapabilityStatus::Declared,
            )]),
            reasoning_efforts: vec!["low".into(), "high".into()],
        };
        assert!(reference
            .reasoning_effort_levels_for_route(Some(&route))
            .is_empty());
    }
}
