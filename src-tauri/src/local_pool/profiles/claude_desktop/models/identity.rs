use super::tests::model;
use super::*;
use zenith_relay_core::model_metadata::ModelMetadataCatalog;
use zenith_relay_core::{CapabilityStatus, ProtocolFeature};

fn catalog_model() -> ModelSummary {
    catalog_model_for("claude-opus-4-6", &["low", "high", "xhigh", "max"])
}

fn catalog_model_for(identity: &str, levels: &[&str]) -> ModelSummary {
    let catalog = identity_catalog(identity, levels);
    let mut models = vec![model("gateway-model", true, WireApi::Messages)];
    zenith_relay_core::protocol::apply_model_metadata(&mut models, &catalog);
    models[0].protocol_routes[0].reasoning_efforts =
        models[0].catalog_reasoning_effort_levels.clone();
    models.remove(0)
}

fn identity_catalog(identity: &str, levels: &[&str]) -> ModelMetadataCatalog {
    ModelMetadataCatalog::from_models_dev_json(
        &json!({ (format!("anthropic/{identity}")): {
            "aliases": ["gateway-model"],
            "reasoning": true,
            "reasoning_effort_levels": levels
        }})
        .to_string(),
    )
    .unwrap()
}

#[test]
fn trusted_claude_identity_uses_a_neutral_alias_only_on_supported_clients() {
    let configured = catalog_model();
    let original = configured.clone();
    let models = std::slice::from_ref(&configured);
    let entries = inference_models_for_client(models, true).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0]["name"],
        zenith_relay_core::claude_desktop_catalog_model_alias(&configured.id)
    );
    assert_eq!(entries[0]["behavesAs"], "claude-opus-4-6");
    assert_eq!(entries[0]["maxEffort"], "max");
    assert_eq!(entries[0]["labelOverride"], configured.id);
    assert_eq!(configured, original);

    let baseline = inference_models_for_client(models, false).unwrap();
    assert_eq!(
        baseline[0]["name"],
        zenith_relay_core::claude_desktop_reasoning_model_alias(&configured.id)
    );
    assert!(baseline[0].get("behavesAs").is_none());
}

#[test]
fn model_names_and_foreign_catalog_records_do_not_supply_claude_identity() {
    for (provider, source) in [
        (None, None),
        (Some("anthropic"), None),
        (None, Some("anthropic/claude-opus-4-6")),
        (Some("foreign"), Some("anthropic/claude-opus-4-6")),
        (Some("anthropic"), Some("foreign/claude-opus-4-6")),
        (Some("anthropic"), Some("anthropic/gpt-test")),
        (Some("anthropic"), Some("anthropic/claude-")),
        (Some("anthropic"), Some("anthropic/claude-opus/other")),
    ] {
        let mut configured = catalog_model();
        configured.id = "claude-opus-4-6".into();
        configured.catalog_name = Some("Claude Opus".into());
        configured.catalog_provider = provider.map(str::to_string);
        configured.catalog_source_model_id = source.map(str::to_string);
        let entries = inference_models_for_client(&[configured], true).unwrap();
        assert!(
            entries[0].get("behavesAs").is_none(),
            "{provider:?}: {source:?}"
        );
        assert_eq!(
            entries[0]["name"],
            zenith_relay_core::claude_desktop_reasoning_model_alias("claude-opus-4-6")
        );
    }
}

#[test]
fn claude_identity_does_not_revive_disabled_or_unsupported_reasoning() {
    for denial in ["reference", "route", "policy"] {
        let mut configured = catalog_model();
        match denial {
            "reference" => {
                configured.catalog_capabilities.as_mut().unwrap().reasoning = Some(false)
            }
            "route" => {
                configured.protocol_routes[0]
                    .features
                    .insert(ProtocolFeature::Reasoning, CapabilityStatus::Unsupported);
            }
            _ => {
                configured.reasoning_configurable = true;
                configured.reasoning_allowed_levels.clear();
            }
        }
        let entries = inference_models_for_client(&[configured], true).unwrap();
        assert!(entries[0].get("behavesAs").is_none(), "{denial}");
        assert!(entries[0].get("maxEffort").is_none(), "{denial}");
        assert_eq!(
            entries[0]["name"],
            zenith_relay_core::claude_desktop_model_alias("gateway-model")
        );
    }
}

#[test]
fn identity_preserves_distinct_routes_and_policy_caps() {
    let mut first = catalog_model();
    first.reasoning_configurable = true;
    first.reasoning_allowed_levels = vec!["low".into(), "high".into()];
    let mut second = first.clone();
    second.id = "other-gateway-model".into();
    let entries = inference_models_for_client(&[first, second], true).unwrap();
    assert_eq!(entries.len(), 2);
    assert_ne!(entries[0]["name"], entries[1]["name"]);
    for entry in entries {
        assert_eq!(entry["behavesAs"], "claude-opus-4-6");
        assert_eq!(entry["maxEffort"], "high");
    }
}

#[test]
#[ignore = "requires ZENITH_CLAUDE_CODE_BIN; runs the installed Code client against loopback fixtures only"]
fn installed_claude_code_preserves_generated_identity_and_native_effort() {
    let cli = std::env::var_os("ZENITH_CLAUDE_CODE_BIN")
        .expect("Set ZENITH_CLAUDE_CODE_BIN to the exact installed Claude Code executable");
    for (identity, levels, efforts) in [
        ("claude-opus-4-6", vec!["low", "high", "max"], "max"),
        (
            "claude-opus-4-7",
            vec!["low", "high", "xhigh", "max"],
            "max,xhigh",
        ),
    ] {
        let models =
            inference_models_for_client(&[catalog_model_for(identity, &levels)], true).unwrap();
        let mut command = installed_wire_command(&cli, &models[0], efforts);
        command.arg("--with-native-control");
        let status = command
            .status()
            .expect("Start the isolated Code wire probe");
        assert!(
            status.success(),
            "installed Code wire qualification: {identity}"
        );
    }
}

fn installed_wire_command(
    cli: &std::ffi::OsStr,
    model: &Value,
    efforts: &str,
) -> std::process::Command {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let mut command = std::process::Command::new("bun");
    command
        .current_dir(repository)
        .args(["scripts/check/check-claude-effort-wire.mjs", "--cli"])
        .arg(cli)
        .args([
            "--model",
            &serde_json::to_string(model).unwrap(),
            "--efforts",
            efforts,
        ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    command
}

mod tool_cycle_tests;
mod wire_gateway_tests;
