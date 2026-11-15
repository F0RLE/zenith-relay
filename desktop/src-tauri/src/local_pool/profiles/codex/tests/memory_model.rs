use super::*;

#[test]
fn memory_policy_defaults_and_stage_input_preserve_the_contract() {
    use crate::local_pool::commands::gateway::settings::{
        SetCodexMemoryEnabledInput, SetCodexMemoryModelInput,
    };
    use crate::local_pool::models::GatewaySettings;

    let mut stored = serde_json::to_value(GatewaySettings::default()).unwrap();
    let object = stored.as_object_mut().unwrap();
    object.remove("codexMemoryExtractionEnabled");
    object.remove("codexMemoryConsolidationEnabled");
    let mut settings: GatewaySettings = serde_json::from_value(stored).unwrap();
    assert!(settings.codex_memory_extraction_enabled);
    assert!(settings.codex_memory_consolidation_enabled);
    settings.codex_memory_extraction_enabled = false;
    let restored: GatewaySettings =
        serde_json::from_value(serde_json::to_value(settings).unwrap()).unwrap();
    assert!(!restored.codex_memory_extraction_enabled);
    assert!(restored.codex_memory_consolidation_enabled);

    for stage in ["extraction", "consolidation"] {
        assert!(
            serde_json::from_value::<SetCodexMemoryModelInput>(json!({
                "stage":stage, "model":"synthetic-model", "expectedModel":null
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<SetCodexMemoryEnabledInput>(json!({
                "stage":stage, "enabled":false
            }))
            .is_ok()
        );
    }
    for invalid in [
        json!({"stage":"other", "enabled":false}),
        json!({"stage":"extraction", "enabled":"false"}),
        json!({"stage":"consolidation", "enabled":false, "model":"synthetic"}),
    ] {
        assert!(serde_json::from_value::<SetCodexMemoryEnabledInput>(invalid).is_err());
    }
}

#[test]
fn memory_model_updates_the_official_key_without_changing_other_settings() {
    let (root, home, backups) = profile_dirs("memory-model");
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "fixture-key",
        &secrets,
    )
    .unwrap();
    let path = home.join(CONFIG_FILE);
    let before = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{before}\n[memories]\nextract_model = 'synthetic-extract'\nmax_rollouts_per_startup = 4\n")).unwrap();
    assert_eq!(
        memory_model(&home, CodexMemoryStage::Consolidation).unwrap(),
        None
    );
    assert_eq!(
        set_memory_model(
            &home,
            &backups,
            CodexMemoryStage::Consolidation,
            Some("synthetic-summary"),
            None
        )
        .unwrap(),
        Some("synthetic-summary".to_owned())
    );
    assert_eq!(
        memory_model(&home, CodexMemoryStage::Consolidation)
            .unwrap()
            .as_deref(),
        Some("synthetic-summary")
    );
    let document = parse_config(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        document["memories"]["extract_model"].as_str(),
        Some("synthetic-extract")
    );
    assert_eq!(
        document["memories"]["max_rollouts_per_startup"].as_integer(),
        Some(4)
    );
    assert_eq!(document["model_provider"].as_str(), Some(PROVIDER_ID));
    let current = fs::read(&path).unwrap();
    assert!(
        set_memory_model(&home, &backups, CodexMemoryStage::Consolidation, None, None).is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), current);
    set_memory_model(
        &home,
        &backups,
        CodexMemoryStage::Consolidation,
        None,
        Some("synthetic-summary"),
    )
    .unwrap();
    let document = parse_config(&fs::read_to_string(&path).unwrap()).unwrap();
    assert!(document["memories"].get("consolidation_model").is_none());
    assert_eq!(
        document["memories"]["extract_model"].as_str(),
        Some("synthetic-extract")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn memory_model_does_not_modify_a_missing_or_native_profile() {
    let (root, home, backups) = profile_dirs("memory-model-unmanaged");
    assert!(
        set_memory_model(
            &home,
            &backups,
            CodexMemoryStage::Consolidation,
            Some("synthetic"),
            None
        )
        .is_err()
    );
    assert!(!home.join(CONFIG_FILE).exists());
    let original = "model_provider = 'openai'\nmodel = 'synthetic-chat'\n";
    fs::write(home.join(CONFIG_FILE), original).unwrap();
    assert!(
        set_memory_model(
            &home,
            &backups,
            CodexMemoryStage::Extraction,
            Some("synthetic"),
            None
        )
        .is_err()
    );
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        original
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn memory_model_rejects_invalid_table_and_preserves_inline_settings() {
    let (root, home, backups) = profile_dirs("memory-model-table");
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "fixture-key",
        &secrets,
    )
    .unwrap();
    let path = home.join(CONFIG_FILE);
    let before = fs::read_to_string(&path).unwrap();
    for setting in [
        "memories = false",
        "memories = { consolidation_model = 42 }",
        "private_setting = 'synthetic-unclosed",
    ] {
        let invalid = format!("{setting}\n{before}");
        fs::write(&path, &invalid).unwrap();
        let error = memory_model(&home, CodexMemoryStage::Consolidation).unwrap_err();
        assert!(!error.message.contains("synthetic-unclosed"));
        let error = set_memory_model(
            &home,
            &backups,
            CodexMemoryStage::Consolidation,
            Some("synthetic"),
            None,
        )
        .unwrap_err();
        assert!(!error.message.contains("synthetic-unclosed"));
        assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
    }
    fs::write(
        &path,
        format!("memories = {{ extract_model = 'synthetic-extract' }}\n{before}"),
    )
    .unwrap();
    set_memory_model(
        &home,
        &backups,
        CodexMemoryStage::Consolidation,
        Some("synthetic"),
        None,
    )
    .unwrap();
    assert_eq!(
        memory_model(&home, CodexMemoryStage::Consolidation)
            .unwrap()
            .as_deref(),
        Some("synthetic")
    );
    let document = parse_config(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        document["memories"]["extract_model"].as_str(),
        Some("synthetic-extract")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn memory_model_does_not_modify_a_managed_direct_api_profile() {
    let (root, home, backups) = profile_dirs("memory-model-direct-api");
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "fixture-key",
        &secrets,
    )
    .unwrap();
    let path = home.join(CONFIG_FILE);
    let backup_path = backup_path(&backups);
    let mut backup =
        parse_backup_snapshot(&read_optional_bytes(&backup_path).unwrap(), &backup_path)
            .unwrap()
            .unwrap();
    let mut document = parse_config(&fs::read_to_string(&path).unwrap()).unwrap();
    let provider = document["model_providers"]
        .as_table_mut()
        .unwrap()
        .remove(PROVIDER_ID)
        .unwrap();
    document["model_providers"][READY_API_PROVIDER_ID] = provider;
    document["model_providers"][READY_API_PROVIDER_ID]["name"] = value(READY_API_PROVIDER_NAME);
    document["model_provider"] = value(READY_API_PROVIDER_ID);
    backup.managed_provider_id = READY_API_PROVIDER_ID.to_owned();
    assert!(managed_config_matches(&document, &backup));
    let original = document.to_string();
    fs::write(&path, &original).unwrap();
    fs::write(&backup_path, serialize_backup(&backup).unwrap()).unwrap();
    for stage in [
        CodexMemoryStage::Extraction,
        CodexMemoryStage::Consolidation,
    ] {
        assert!(set_memory_model(&home, &backups, stage, Some("synthetic"), None).is_err());
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn memory_extraction_model_updates_and_resets_independently_with_conflict_protection() {
    let (root, home, backups) = profile_dirs("memory-extraction-model");
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "fixture-key",
        &secrets,
    )
    .unwrap();
    let path = home.join(CONFIG_FILE);
    let before = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{before}\n[memories]\nconsolidation_model = 'synthetic-summary'\ngenerate_memories = false\nuse_memories = false\n")).unwrap();
    set_memory_model(
        &home,
        &backups,
        CodexMemoryStage::Extraction,
        Some("synthetic-extract"),
        None,
    )
    .unwrap();
    assert_eq!(
        memory_model(&home, CodexMemoryStage::Extraction)
            .unwrap()
            .as_deref(),
        Some("synthetic-extract")
    );
    assert_eq!(
        memory_model(&home, CodexMemoryStage::Consolidation)
            .unwrap()
            .as_deref(),
        Some("synthetic-summary")
    );
    let current = fs::read(&path).unwrap();
    assert!(set_memory_model(&home, &backups, CodexMemoryStage::Extraction, None, None).is_err());
    assert_eq!(fs::read(&path).unwrap(), current);
    set_memory_model(
        &home,
        &backups,
        CodexMemoryStage::Extraction,
        None,
        Some("synthetic-extract"),
    )
    .unwrap();
    let document = parse_config(&fs::read_to_string(&path).unwrap()).unwrap();
    assert!(document["memories"].get("extract_model").is_none());
    assert_eq!(
        document["memories"]["consolidation_model"].as_str(),
        Some("synthetic-summary")
    );
    assert_eq!(
        document["memories"]["generate_memories"].as_bool(),
        Some(false)
    );
    assert_eq!(document["memories"]["use_memories"].as_bool(), Some(false));
    assert_eq!(
        document.get("model").map(ToString::to_string),
        parse_config(&before)
            .unwrap()
            .get("model")
            .map(ToString::to_string)
    );
    let invalid = format!("{before}\n[memories]\nextract_model = false\n");
    fs::write(&path, &invalid).unwrap();
    assert!(memory_model(&home, CodexMemoryStage::Extraction).is_err());
    assert!(set_memory_model(&home, &backups, CodexMemoryStage::Extraction, None, None).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
    fs::remove_dir_all(root).unwrap();
}
