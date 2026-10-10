use super::{ModelDiscoveryFailure, ModelDiscoveryFailureCode, MAX_MODELS};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use url::Url;

pub const BASIS_POINTS_ACCESS_ENDPOINT: &str =
    "https://bps.openai.com/basispoints/api/responses/access";
pub const MAX_BASIS_POINTS_ACCESS_BYTES: usize = 1024 * 1024;

/// Declared executable access, separate from reference capabilities and pricing.
#[derive(Clone, Debug, Deserialize)]
pub struct BasisPointsModel {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    available: Option<bool>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    model_picker_enabled: Option<bool>,
    #[serde(default)]
    policy: Option<ModelPolicy>,
    #[serde(default)]
    efforts: Option<Vec<ModelEffort>>,
}

#[derive(Clone, Debug, Deserialize)]
struct ModelPolicy {
    state: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ModelEffort {
    value: String,
}

impl BasisPointsModel {
    pub fn reasoning_levels(&self) -> Option<Vec<String>> {
        self.efforts.as_ref().map(|efforts| {
            crate::catalog::canonicalize_reasoning_levels(
                efforts.iter().map(|effort| &effort.value),
            )
        })
    }

    pub(crate) fn supports_reasoning_effort(&self, effort: &str) -> bool {
        let normalized = normalize_basis_points_reasoning_effort(effort);
        self.reasoning_levels().is_none_or(|levels| {
            levels
                .iter()
                .any(|level| normalize_basis_points_reasoning_effort(level) == normalized)
        })
    }
}

/// The request codec and access checks must use the same BPS wire identifier.
pub(crate) fn normalize_basis_points_reasoning_effort(effort: &str) -> String {
    let effort = effort.trim().to_ascii_lowercase();
    match effort.as_str() {
        "x-high" | "extra-high" | "extra_high" | "max" => "xhigh".to_string(),
        _ => effort,
    }
}

#[derive(Clone, Debug)]
pub struct BasisPointsModelAccess {
    pub models: Vec<BasisPointsModel>,
}

impl BasisPointsModelAccess {
    pub fn model_ids(&self) -> Vec<String> {
        self.models.iter().map(|model| model.id.clone()).collect()
    }

    pub fn manifest(&self) -> Value {
        json!({"models":self.models.iter().enumerate().map(|(priority, model)| {
            let display_name = model
                .label
                .as_deref()
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .unwrap_or(&model.id);
            let mut entry = json!({"slug":model.id,"priority":priority,"display_name":display_name});
            if let Some(levels) = model.reasoning_levels() {
                entry["supported_reasoning_levels"] = json!(levels.into_iter().map(|effort| json!({"effort":effort,"description":""})).collect::<Vec<_>>());
            }
            entry
        }).collect::<Vec<_>>()})
    }
}

/// Derive access beside the configured Responses endpoint, including loopback
/// fixtures. Never substitute the native ChatGPT discovery endpoint.
pub fn basis_points_access_url(responses_url: &Url) -> Option<Url> {
    let mut url = responses_url.clone();
    url.path_segments_mut().ok()?.pop_if_empty().push("access");
    url.set_query(Some("include_models=true"));
    Some(url)
}

pub fn parse_basis_points_model_access(
    bytes: &[u8],
) -> Result<BasisPointsModelAccess, ModelDiscoveryFailure> {
    #[derive(Deserialize)]
    struct Envelope {
        allowed: Option<bool>,
        #[serde(default)]
        model_catalog: Option<Catalog>,
    }
    #[derive(Deserialize)]
    struct Catalog {
        models: Option<Vec<Value>>,
        #[serde(default)]
        restricted_models: Vec<String>,
    }
    let invalid = || ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::InvalidResponse);
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let allowed = envelope.allowed.ok_or_else(invalid)?;
    if !allowed {
        return Ok(BasisPointsModelAccess { models: Vec::new() });
    }
    let catalog = envelope.model_catalog.ok_or_else(invalid)?;
    let raw_models = catalog.models.ok_or_else(invalid)?;
    if raw_models.len() > MAX_MODELS || catalog.restricted_models.len() > MAX_MODELS {
        return Err(invalid());
    }
    let restricted = catalog
        .restricted_models
        .iter()
        .map(|id| crate::model_id_key(id))
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let models = raw_models
        .into_iter()
        // A malformed row does not invalidate the provider's envelope. The
        // access decision and catalog shape remain authoritative while one
        // bad model entry is ignored safely.
        .filter_map(|raw_model| serde_json::from_value::<BasisPointsModel>(raw_model).ok())
        .filter_map(|mut model| {
            let normalized_id = model.id.trim();
            if !crate::is_valid_model_token(normalized_id)
                || restricted.contains(&crate::model_id_key(normalized_id))
                || model.available == Some(false)
                || model.enabled == Some(false)
                || model.model_picker_enabled == Some(false)
                || model.policy.as_ref().is_some_and(|policy| {
                    matches!(
                        policy.state.as_str(),
                        "disabled" | "unconfigured" | "denied" | "blocked"
                    )
                })
                || !seen.insert(crate::model_id_key(normalized_id))
            {
                return None;
            }
            model.id = normalized_id.to_string();
            Some(model)
        })
        .collect();
    Ok(BasisPointsModelAccess { models })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_access_matches_wire_aliases_without_inventing_levels() {
        let access = parse_basis_points_model_access(
            br#"{"allowed":true,"model_catalog":{"models":[
                {"id":"explicit","efforts":[{"value":"max"},{"value":"future"}]},
                {"id":"empty","efforts":[]},
                {"id":"unspecified"}
            ]}}"#,
        )
        .unwrap();
        for alias in [
            "max",
            "xhigh",
            "x-high",
            "extra-high",
            "extra_high",
            " MAX ",
        ] {
            assert!(access.models[0].supports_reasoning_effort(alias), "{alias}");
            assert_eq!(normalize_basis_points_reasoning_effort(alias), "xhigh");
        }
        assert!(access.models[0].supports_reasoning_effort(" FUTURE "));
        assert!(!access.models[0].supports_reasoning_effort("medium"));
        assert!(!access.models[1].supports_reasoning_effort("max"));
        assert!(access.models[2].supports_reasoning_effort("future"));
        assert_eq!(
            access.models[0].reasoning_levels().unwrap(),
            ["max", "future"]
        );
    }

    #[test]
    fn access_is_authoritative_preserves_source_order_and_all_efforts() {
        let access = parse_basis_points_model_access(&serde_json::to_vec(&json!({
            "allowed":true,"model_catalog":{"models":[
                {"id":"gpt-test-b","efforts":[{"value":"high"},{"value":"ultra"},{"value":"future"}]},
                {"id":"gpt-test-a"},{"id":"restricted"},{"id":"disabled","enabled":false},
                {"id":"unconfigured","policy":{"state":"unconfigured"}},
                {"id":"gpt-test-b"}
            ],"restricted_models":["restricted"]}
        })).unwrap()).unwrap();
        assert_eq!(access.model_ids(), ["gpt-test-b", "gpt-test-a"]);
        assert_eq!(
            access.models[0].reasoning_levels().unwrap(),
            ["high", "ultra", "future"]
        );
        assert_eq!(access.models[1].reasoning_levels(), None);
        assert_eq!(access.manifest()["models"][0]["slug"], "gpt-test-b");
        assert_eq!(
            access.manifest()["models"][0]["display_name"].as_str(),
            Some("gpt-test-b")
        );
        assert!(parse_basis_points_model_access(br#"{"allowed":false}"#)
            .unwrap()
            .models
            .is_empty());
        assert!(parse_basis_points_model_access(
            br#"{"allowed":true,"model_catalog":{"models":[]}}"#
        )
        .unwrap()
        .models
        .is_empty());
        let malformed_row = parse_basis_points_model_access(
            br#"{"allowed":true,"model_catalog":{"models":[
                {"id":"gpt-valid"},
                {"id":"gpt-bad","enabled":"yes"},
                {"id":"gpt-valid-2","policy":{"state":42}},
                {"id":"gpt-valid-3","label":"  Readable label  "},
                {"id":"  gpt-trimmed  "},
                {"id":"gpt has-space"},
                {"id":"gpt\tcontrol"}
            ]}}"#,
        )
        .unwrap();
        assert_eq!(
            malformed_row.model_ids(),
            ["gpt-valid", "gpt-valid-3", "gpt-trimmed"]
        );
        assert_eq!(
            malformed_row.manifest()["models"][1]["display_name"],
            "Readable label"
        );
        for raw in [
            br#"{}"#.as_slice(),
            br#"{"allowed":true}"#,
            br#"{"allowed":true,"model_catalog":{"models":null}}"#,
        ] {
            assert!(parse_basis_points_model_access(raw).is_err());
        }
    }
}
