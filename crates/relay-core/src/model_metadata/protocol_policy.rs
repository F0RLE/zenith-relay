use super::{MetadataFieldProvenance, ModelMetadata};
use crate::WireApi;
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};

const POLICY_JSON: &str = include_str!("../../../../catalog/protocol-policy.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Policy {
    schema_version: u32,
    revision: String,
    preferences: BTreeMap<String, Vec<WireApi>>,
}

fn policy() -> &'static Policy {
    static POLICY: OnceLock<Policy> = OnceLock::new();
    POLICY.get_or_init(|| {
        let policy: Policy = serde_json::from_str(POLICY_JSON).expect("checked protocol policy");
        assert_eq!(policy.schema_version, 1);
        for preferences in policy.preferences.values() {
            assert!(!preferences.is_empty());
            assert_eq!(
                preferences
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
                preferences.len()
            );
        }
        policy
    })
}

fn group(metadata: &ModelMetadata) -> Option<&'static str> {
    let namespace = metadata
        .canonical_model_id
        .as_deref()
        .and_then(|model| model.split_once('/'))
        .map(|(provider, _)| provider)
        .unwrap_or(&metadata.provider)
        .to_ascii_lowercase();
    let family = metadata
        .family
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match namespace.as_str() {
        "openai" => Some("openai"),
        "anthropic" => Some("anthropic"),
        "google" => Some("google"),
        "xai" | "x-ai" => Some("xai"),
        "zai" | "z-ai" | "zhipuai" | "zhipuai-coding-plan" => Some("zai"),
        _ if family.starts_with("claude") => Some("anthropic"),
        _ if family.starts_with("gemini") => Some("google"),
        _ if family.starts_with("grok") => Some("xai"),
        _ if family.starts_with("glm") => Some("zai"),
        _ if !namespace.is_empty() || !family.is_empty() => Some("default"),
        _ => None,
    }
}

pub(super) fn native_protocol(metadata: &ModelMetadata) -> Option<WireApi> {
    let group = group(metadata)?;
    (group != "default").then(|| policy().preferences[group][0])
}

pub(super) fn apply(metadata: &mut ModelMetadata) {
    if !metadata.protocol_preferences.is_empty() {
        return;
    }
    let Some(group) = group(metadata) else { return };
    metadata.protocol_preferences = policy().preferences[group].clone();
    metadata.field_provenance.insert(
        "protocolPreferences".into(),
        MetadataFieldProvenance {
            source: "relay_protocol_policy".into(),
            revision: Some(policy().revision.clone()),
            conflicts: Vec::new(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_protocol_policy_contains_each_protocol_once() {
        for group in ["openai", "anthropic", "google", "xai", "zai", "default"] {
            assert_eq!(policy().preferences[group].len(), WireApi::ALL.len());
        }
    }
}
